//! The parts of the machine the window reports on or changes: whether Flow
//! starts with the session, and which microphone PipeWire is actually handing
//! it.
//!
//! Everything here shells out to the tools that own this state - `systemctl`
//! and `pactl` - rather than keeping a copy of it. A settings window that
//! remembers what it set is a window that disagrees with the system the moment
//! anything else touches it.

use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

/// Nothing here is on the dictation path, but a hung `systemctl` would freeze
/// the UI thread just as effectively.
const BUDGET: Duration = Duration::from_secs(3);

fn run(program: &str, args: &[&str]) -> Option<std::process::Output> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;

    // wait_timeout is not in std, so poll: these commands return in
    // milliseconds, and the alternative is a window that can hang.
    let deadline = std::time::Instant::now() + BUDGET;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                return None;
            }
            Err(_) => return None,
        }
    }
}

/// Whether the user unit is enabled, or `None` when systemd cannot answer -
/// the unit is not installed, or this is not a systemd session. `None` means
/// "do not offer this control", not "off".
pub fn autostart_enabled() -> Option<bool> {
    let output = run("systemctl", &["--user", "is-enabled", "flow.service"])?;
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
pub fn set_autostart(enable: bool) -> Result<(), String> {
    let verb = if enable { "enable" } else { "disable" };
    let output = run("systemctl", &["--user", verb, "flow.service"])
        .ok_or_else(|| "systemctl did not respond".to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let reason = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(if reason.is_empty() {
        format!("systemctl {verb} failed")
    } else {
        reason
    })
}

/// Start, stop or restart the daemon.
///
/// The window is not how you dictate, but it is a reasonable place to turn the
/// thing on - especially when it is not running, which is the one state where
/// the keybinding cannot help you.
pub fn service(verb: &str) -> Result<(), String> {
    let output = run("systemctl", &["--user", verb, "flow.service"])
        .ok_or_else(|| "systemctl did not respond".to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let reason = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(if reason.is_empty() {
        format!("systemctl {verb} failed")
    } else {
        reason
    })
}

/// The description of the default PipeWire source, which is the microphone
/// Flow records from. Read-only on purpose: the daemon deliberately follows
/// the system default so that changing your microphone in your desktop's own
/// settings just works, and a second picker here could only ever disagree
/// with it.
pub fn default_input() -> Option<String> {
    let default = run("pactl", &["get-default-source"])?;
    let name = String::from_utf8_lossy(&default.stdout).trim().to_owned();
    if name.is_empty() {
        return None;
    }

    // Prefer the human description over the alsa_input.usb-... device id.
    let listed = run("pactl", &["list", "sources"])?;
    let text = String::from_utf8_lossy(&listed.stdout);
    Some(description_of(&text, &name).unwrap_or(name))
}

/// Pull the `Description:` belonging to the source called `name` out of
/// `pactl list sources` output. Split out so the parsing is testable without
/// PipeWire running.
fn description_of(listing: &str, name: &str) -> Option<String> {
    let mut current_is_ours = false;
    for line in listing.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("Name: ") {
            current_is_ours = value.trim() == name;
        } else if current_is_ours {
            if let Some(value) = trimmed.strip_prefix("Description: ") {
                return Some(value.trim().to_owned());
            }
        }
    }
    None
}

/// What is actually running the session, e.g. "Hyprland · Wayland". Read from
/// the environment rather than assumed: the About screen used to say Hyprland
/// on every machine, including the ones where it was wrong.
pub fn session() -> String {
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

pub fn data_home() -> PathBuf {
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into())).join(".local/share")
        })
}

/// A model directory as the window reports it: present or not, and how big.
pub struct Model {
    pub label: &'static str,
    pub detail: String,
    pub bytes: u64,
    pub installed: bool,
}

/// Measure what is actually on disk. The sizes used to be written into the
/// source, so they stayed the same however much was really there - including
/// when nothing was.
pub fn models() -> Vec<Model> {
    let root = data_home().join("flow/models");

    // The speech model is a directory of onnx files; the cleanup model is a
    // single gguf beside it. Found by extension rather than by name so that
    // swapping the gguf for a different one does not turn this into a lie the
    // moment the daemon moves on.
    let speech = root.join("tdt");
    let cleanup = largest_gguf(&root);

    vec![
        Model {
            label: "Speech",
            detail: describe(&speech),
            bytes: size_of(&speech),
            installed: speech.is_dir(),
        },
        Model {
            label: "Cleanup",
            detail: cleanup
                .as_ref()
                .map(|path| describe(path))
                .unwrap_or_else(|| "not installed".to_string()),
            bytes: cleanup.as_deref().map(size_of).unwrap_or(0),
            installed: cleanup.is_some(),
        },
    ]
}

/// The biggest `.gguf` in `root`, which is the cleanup model. Biggest rather
/// than first so a leftover from an older, smaller model is not mistaken for
/// the one in use.
fn largest_gguf(root: &std::path::Path) -> Option<PathBuf> {
    std::fs::read_dir(root)
        .ok()?
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "gguf"))
        .max_by_key(|path| size_of(path))
}

/// The file or directory name, which is what identifies a model to a person -
/// the full path is already shown once at the bottom of the screen.
fn describe(path: &std::path::Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// Bytes on disk, counting a directory's contents recursively.
fn size_of(path: &std::path::Path) -> u64 {
    let Ok(meta) = std::fs::metadata(path) else {
        return 0;
    };
    if meta.is_file() {
        return meta.len();
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .filter_map(Result::ok)
        .map(|entry| size_of(&entry.path()))
        .sum()
}

/// Bytes as a human reads them. Kept here so the same rounding is used for a
/// single model and for the total.
pub fn human_bytes(bytes: u64) -> String {
    const UNITS: [(&str, u64); 3] = [("GB", 1 << 30), ("MB", 1 << 20), ("KB", 1 << 10)];
    for (unit, size) in UNITS {
        if bytes >= size {
            return format!("{:.1} {unit}", bytes as f64 / size as f64);
        }
    }
    format!("{bytes} B")
}

/// Hand a path to the desktop's own handler. Used by the buttons that used to
/// do nothing at all.
pub fn open(path: &std::path::Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("{} does not exist yet", path.display()));
    }
    match run("xdg-open", &[&path.display().to_string()]) {
        Some(output) if output.status.success() => Ok(()),
        Some(_) => Err("xdg-open could not open it".to_string()),
        None => Err("xdg-open is not available".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bytes_read_the_way_a_person_would() {
        assert_eq!(human_bytes(0), "0 B");
        assert_eq!(human_bytes(2048), "2.0 KB");
        assert_eq!(human_bytes(650 * (1 << 20)), "650.0 MB");
        assert_eq!(human_bytes(3 * (1 << 30) / 2), "1.5 GB");
    }

    const LISTING: &str = "\
Source #184594
        State: RUNNING
        Name: alsa_input.usb-webcam.iec958-stereo
        Description: Full HD webcam Digital Stereo
Source #184596
        State: SUSPENDED
        Name: alsa_input.platform-snd_aloop.0.analog-stereo
        Description: Loopback Analog Stereo
";

    #[test]
    fn the_description_matches_the_named_source() {
        assert_eq!(
            description_of(LISTING, "alsa_input.usb-webcam.iec958-stereo").as_deref(),
            Some("Full HD webcam Digital Stereo")
        );
        assert_eq!(
            description_of(LISTING, "alsa_input.platform-snd_aloop.0.analog-stereo").as_deref(),
            Some("Loopback Analog Stereo")
        );
    }

    /// A name that is not in the listing must not borrow the next source's
    /// description - that would label the microphone as something it is not.
    #[test]
    fn an_unknown_source_has_no_description() {
        assert_eq!(description_of(LISTING, "alsa_input.nonexistent"), None);
        assert_eq!(description_of("", "anything"), None);
    }
}
