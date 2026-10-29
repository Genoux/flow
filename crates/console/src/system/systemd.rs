//! The user units that run Flow on a Linux session.

use std::time::Duration;

use super::{reopen_flow, require_success, run, run_for, Channel};

pub(crate) const OPENER: &str = "xdg-open";

pub(crate) fn flow_program() -> String {
    "flow".to_owned()
}

pub(super) fn autostart_enabled_for(budget: Duration) -> Option<bool> {
    let output = run_for(
        "systemctl",
        &["--user", "is-enabled", "flow.service"],
        budget,
    )?;
    let state = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    match state.as_str() {
        // `linked` and `static` are enabled-ish; anything else we do not
        // understand well enough to claim a state for.
        "enabled" | "enabled-runtime" | "linked" | "linked-runtime" | "static" => Some(true),
        "disabled" => Some(false),
        _ => None,
    }
}

/// Enable or disable the user unit. Returns the error text on failure so the
/// window can show why rather than silently springing the switch back.
pub(crate) fn set_autostart(enable: bool) -> Result<(), String> {
    let verb = if enable { "enable" } else { "disable" };
    require_success(
        "systemctl",
        &["--user", verb, "flow.service"],
        &format!("systemctl {verb} failed"),
    )
}

pub(super) fn control(verb: &str) -> Result<(), String> {
    require_success(
        "systemctl",
        &["--user", verb, "flow.service"],
        &format!("systemctl {verb} failed"),
    )
}

/// `systemctl start` can succeed before a short-lived daemon exits. Confirm the
/// unit survives startup so the UI never reports Flow as running when it has
/// already failed.
pub(super) fn ensure_running() -> Result<(), String> {
    for _ in 0..5 {
        std::thread::sleep(Duration::from_millis(100));
        let output = run("systemctl", &["--user", "is-active", "flow.service"])
            .ok_or_else(|| "systemctl did not respond".to_string())?;
        let state = String::from_utf8_lossy(&output.stdout).trim().to_owned();
        match state.as_str() {
            "active" => return Ok(()),
            "activating" => continue,
            _ => {
                return Err(format!(
                    "Flow stopped during startup (systemd reports {state}). Run `flow logs` for details."
                ));
            }
        }
    }

    Err("Flow did not finish starting. Run `flow logs` for details.".into())
}

/// Ensure the independent tray controller exists. Starting an already active
/// unit is a no-op, so the window can ask every time it opens.
pub(crate) fn start_tray() -> Result<(), String> {
    require_success(
        "systemctl",
        &["--user", "start", "flow-tray.service"],
        "systemctl could not start the Flow tray",
    )
}

pub(super) fn is_flow_process(pid: u32) -> bool {
    std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|comm| comm.trim() == "flow")
}

pub(crate) fn restart_app() -> Result<(), String> {
    let console = bin_dir().join("flow-console");
    if !console.is_file() {
        return Err("The selected console is missing. Install the release again.".into());
    }
    super::service("stop")?;
    let _ = run("systemctl", &["--user", "try-restart", "flow-tray.service"]);
    // A child in the old console's transient unit dies when that window exits.
    reopen_flow(
        "systemd-run",
        &[
            "--user",
            "--quiet",
            "--collect",
            "--property=Type=exec",
            "--",
            console.to_str().ok_or("Invalid console path")?,
        ],
    )
}

/// What is actually running the session, e.g. "Hyprland · Wayland". Read from
/// the environment rather than assumed: the About screen used to say Hyprland
/// on every machine, including the ones where it was wrong.
pub(crate) fn session() -> String {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP")
        .or_else(|_| std::env::var("XDG_SESSION_DESKTOP"))
        .unwrap_or_default();
    let kind = match std::env::var("XDG_SESSION_TYPE").as_deref() {
        Ok("wayland") => "Wayland",
        Ok("x11") => "X11",
        _ if std::env::var_os("WAYLAND_DISPLAY").is_some() => "Wayland",
        _ if std::env::var_os("DISPLAY").is_some() => "X11",
        _ => "unknown",
    };
    if desktop.is_empty() {
        kind.to_string()
    } else {
        format!("{desktop} · {kind}")
    }
}

/// `xdg-open` on a `.toml` would launch an editor; About wants the folder.
pub(super) fn reveal(path: &std::path::Path) -> Result<(), String> {
    let folder = if path.is_dir() {
        path
    } else {
        path.parent().unwrap_or(path)
    };
    super::open(folder)
}

/// Which build the `flow` symlink currently points at.
///
/// The link is the setting. There is deliberately no `channel = ` line in the
/// config: two sources for one fact drift, and a config that claimed
/// experimental while the link pointed at stable would be lying about which
/// binary is about to run. Reading the link cannot be wrong, and someone who
/// repoints it by hand gets a window that agrees with them.
pub(crate) fn channel() -> Channel {
    channel_in(&bin_dir())
}

fn channel_in(dir: &std::path::Path) -> Channel {
    let link = dir.join("flow");
    match std::fs::read_link(&link) {
        Ok(target) if target.to_string_lossy().ends_with("-experimental") => Channel::Experimental,
        Ok(_) => Channel::Stable,
        // No link at all is an install that predates channels, which is a
        // stable one.
        Err(_) => Channel::Stable,
    }
}

fn bin_dir() -> std::path::PathBuf {
    std::env::var_os("XDG_BIN_HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| {
            std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".local/bin")
        })
}

/// Where an installed release's `flow` and `flow-console` are, and the suffix
/// they carry in that directory.
pub(crate) fn installed_binaries(channel: Channel) -> Result<(std::path::PathBuf, String), String> {
    Ok((bin_dir(), format!("-{}", channel.suffix())))
}

/// Point `flow` and `flow-console` at the other build.
///
/// Takes effect on restart, not now: the running daemon is the old binary and
/// this window is the old console. Switching is repointing two links, so going
/// back is the same operation - which is the whole reason both builds stay on
/// disk rather than one replacing the other.
pub(crate) fn set_channel(channel: Channel) -> Result<(), String> {
    set_channel_in(&bin_dir(), channel)
}

fn set_channel_in(dir: &std::path::Path, channel: Channel) -> Result<(), String> {
    for name in ["flow", "flow-console"] {
        let target = dir.join(format!("{name}-{}", channel.suffix()));
        use std::os::unix::fs::PermissionsExt;
        if !target
            .metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        {
            return Err(format!(
                "{} is not installed or executable",
                target.display()
            ));
        }
    }
    let stage = dir.join(".flow-channel-switch");
    std::fs::create_dir(&stage).map_err(|e| format!("Preparing channel switch: {e}"))?;
    let result = (|| -> std::io::Result<()> {
        for name in ["flow", "flow-console"] {
            let link = dir.join(name);
            if link.symlink_metadata().is_ok() {
                std::fs::hard_link(&link, stage.join(format!("{name}.old")))?;
            }
            std::os::unix::fs::symlink(format!("{name}-{}", channel.suffix()), stage.join(name))?;
        }
        std::fs::rename(stage.join("flow"), dir.join("flow"))?;
        if let Err(error) = std::fs::rename(stage.join("flow-console"), dir.join("flow-console")) {
            let backup = stage.join("flow.old");
            if backup.symlink_metadata().is_ok() {
                std::fs::rename(backup, dir.join("flow"))?;
            } else {
                std::fs::remove_file(dir.join("flow"))?;
            }
            return Err(error);
        }
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(stage);
    result.map_err(|e| format!("Switching release: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The link is the setting, so reading it back is the only check that
    /// matters: a switch that reported success while the link stayed put would
    /// be a window lying about which binary is about to run.
    #[test]
    fn switching_channels_repoints_both_links_and_is_reversible() {
        let root = std::env::temp_dir().join(format!("flow-channel-{}", std::process::id()));
        let bin = root.join(".local/bin");
        std::fs::create_dir_all(&bin).unwrap();
        for name in [
            "flow-stable",
            "flow-console-stable",
            "flow-experimental",
            "flow-console-experimental",
        ] {
            std::fs::write(bin.join(name), "").unwrap();
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(bin.join(name), std::fs::Permissions::from_mode(0o755))
                .unwrap();
        }
        // A pre-channels install: a real file where the link belongs.
        std::fs::write(bin.join("flow"), "").unwrap();

        assert_eq!(channel_in(&bin), Channel::Stable, "no link reads as stable");

        set_channel_in(&bin, Channel::Experimental).unwrap();
        assert_eq!(channel_in(&bin), Channel::Experimental);
        assert!(
            bin.join("flow").is_symlink(),
            "the regular file was replaced"
        );
        assert_eq!(
            std::fs::read_link(bin.join("flow-console"))
                .unwrap()
                .to_string_lossy(),
            "flow-console-experimental",
            "the console link must move with the daemon link"
        );

        set_channel_in(&bin, Channel::Stable).unwrap();
        assert_eq!(
            channel_in(&bin),
            Channel::Stable,
            "switching back is the same click"
        );

        std::fs::remove_file(bin.join("flow-console-experimental")).unwrap();
        assert!(set_channel_in(&bin, Channel::Experimental).is_err());
        assert_eq!(channel_in(&bin), Channel::Stable);
        assert_eq!(
            std::fs::read_link(bin.join("flow-console")).unwrap(),
            std::path::Path::new("flow-console-stable")
        );
        std::fs::remove_dir_all(&root).unwrap();
    }
}
