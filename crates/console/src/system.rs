//! The parts of the machine the window reports on or changes: whether Offhand
//! starts with the session, and which microphone PipeWire is actually handing
//! it.
//!
//! Everything here shells out to the tools that own this state - `systemctl`
//! and `pactl`, or `launchctl` on a Mac - rather than keeping a copy of it. A
//! settings window that remembers what it set is a window that disagrees with
//! the system the moment anything else touches it.

use std::time::Duration;

pub(crate) use offhand_paths::process::{run_for, BUDGET};
/// Launch-time reads are advisory. If one owner is wedged, an absent answer is
/// better than holding the first frame for the full interactive-action budget.
const STARTUP_BUDGET: Duration = Duration::from_millis(250);

fn run(program: &str, args: &[&str]) -> Option<std::process::Output> {
    run_for(program, args, BUDGET)
}

#[cfg(not(target_os = "macos"))]
fn failure_reason(output: &std::process::Output, fallback: &str) -> String {
    let reason = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if reason.is_empty() {
        fallback.to_owned()
    } else {
        reason
    }
}

/// Run a service manager command, failing with what it said on stderr.
#[cfg(not(target_os = "macos"))]
fn require_success(program: &str, args: &[&str], fallback: &str) -> Result<(), String> {
    let output = run(program, args).ok_or_else(|| format!("{program} did not respond"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(failure_reason(&output, fallback))
    }
}

/// The last step of `restart_app`, once the daemon is back up.
fn reopen_offhand(program: &str, args: &[&str]) -> Result<(), String> {
    let output = run(program, args)
        .ok_or("The replacement window could not be started. Reopen Offhand to retry.")?;
    if !output.status.success() {
        return Err(format!(
            "Could not reopen Offhand: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(())
}

// launchd is also built on Linux under test, so its parsers and its state
// machine run in CI rather than only on a Mac.
#[cfg(any(target_os = "macos", test))]
mod launchd;
#[cfg(not(target_os = "macos"))]
mod systemd;
#[cfg(target_os = "macos")]
use launchd as platform;
#[cfg(not(target_os = "macos"))]
use systemd as platform;

use platform::{autostart_enabled_for, control, ensure_running, is_offhand_process, OPENER};
pub(crate) use platform::{
    channel, default_input, input_sources, installed_binaries, offhand_program, restart_app,
    session, set_autostart, set_channel, start_tray,
};

/// Whether the user unit is enabled, or `None` when systemd cannot answer -
/// the unit is not installed, or this is not a systemd session. `None` means
/// "do not offer this control", not "off".
pub fn autostart_enabled() -> Option<bool> {
    autostart_enabled_for(BUDGET)
}

/// The same read with a launch-sized deadline. A stuck user bus must not keep
/// the window invisible; the full budget remains available after interaction.
pub fn startup_autostart_enabled() -> Option<bool> {
    autostart_enabled_for(STARTUP_BUDGET)
}

/// How many of the installed speech-model files are missing or the wrong
/// length, straight from the daemon binary - which is the one that pins their
/// names and sizes.
///
/// `None` means no verdict: no `offhand` on PATH, or it did not answer inside the
/// budget. The window falls back to what it can see for itself (whether the
/// model directory exists at all) rather than claiming an install is whole on
/// the strength of a command that never ran.
///
/// Costs about three milliseconds, which is what makes it something the
/// window can do every time it opens. The hashing pass is Repair's job.
pub fn damage() -> Option<usize> {
    damage_for(BUDGET)
}

/// Installation health shapes the Overview layout, so it is read before the
/// first frame rather than arriving later and moving the page. Its deadline is
/// nevertheless launch-sized: a wedged binary is no reason to hide the window.
pub fn startup_damage() -> Option<usize> {
    damage_for(STARTUP_BUDGET)
}

fn damage_for(budget: Duration) -> Option<usize> {
    let output = run_for(&offhand_program(), &["check", "--porcelain"], budget)?;
    let text = String::from_utf8_lossy(&output.stdout);

    // The verdict line is the handshake: without it this is an older `offhand`
    // that has no idea what was asked of it, and an empty stdout would
    // otherwise read as a clean bill of health.
    let answered = text
        .lines()
        .any(|line| matches!(line.trim(), "whole" | "broken"));

    answered.then(|| {
        text.lines()
            .filter(|line| line.starts_with("damaged "))
            .count()
    })
}

/// Long enough for a real round trip to OpenRouter, not just a local read -
/// every other budget in this file is milliseconds because everything else it
/// asks leaves the machine.
const PROBE_BUDGET: Duration = Duration::from_secs(15);

/// Whether the saved OpenRouter key works, straight from the daemon binary -
/// same reasoning as [`damage_for`]: the key lives in the daemon's config, and
/// a second HTTP call from this window could disagree with the one dictation
/// actually gets.
///
/// `None` means no verdict, the same as [`damage`]: no `offhand` on PATH, or it
/// did not answer inside the budget.
pub fn probe_router() -> Option<crate::openrouter::Outcome> {
    let output = run_for(&offhand_program(), &["probe", "--porcelain"], PROBE_BUDGET)?;
    crate::openrouter::parse_probe(&String::from_utf8_lossy(&output.stdout))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Channel {
    Stable,
    Experimental,
}

impl Channel {
    pub fn suffix(self) -> &'static str {
        match self {
            Channel::Stable => "stable",
            Channel::Experimental => "experimental",
        }
    }
}

/// Start, stop or restart the daemon.
///
/// The window is not how you dictate, but it is a reasonable place to turn the
/// thing on - especially when it is not running, which is the one state where
/// the keybinding cannot help you.
pub fn service(verb: &str) -> Result<(), String> {
    if verb == "start" && daemon_listening(&offhand_paths::socket()) {
        return Ok(());
    }
    control(verb)?;

    if matches!(verb, "start" | "restart") {
        ensure_running()
    } else {
        terminate_daemon();
        Ok(())
    }
}

fn daemon_listening(path: &std::path::Path) -> bool {
    std::os::unix::net::UnixStream::connect(path).is_ok()
}

/// Stop the daemon itself, and not only the unit systemd knows about.
///
/// `systemctl stop` reaches a daemon systemd started and nothing else. One
/// launched any other way - `offhand daemon` in a terminal, the checkout that
/// `offhand-dev` runs - keeps its grab on the chord, so the window reported Offhand
/// stopped while the trigger key still opened the microphone. Stop is the one
/// word here that has to be believed: it is the user saying not now, and the
/// only way back is the Start button beside it.
///
/// The pid file is the daemon's own answer to which process it is, which is
/// what `offhand start` and `offhand stop` already signal. Reading `comm` before
/// signalling is what makes that safe from this side: the file outlives a
/// crash, and by then the pid may belong to somebody else entirely.
fn terminate_daemon() {
    let path = offhand_paths::pid_file();
    let Some(pid) = std::fs::read_to_string(&path)
        .ok()
        .and_then(|raw| raw.trim().parse::<u32>().ok())
    else {
        return;
    };
    if !is_offhand_process(pid) {
        return;
    }
    run("kill", &["-TERM", &pid.to_string()]);
    // SIGTERM leaves the daemon no chance to tidy up after itself, so the file
    // it wrote is ours to clear. A stale one is only ever a wrong answer.
    let _ = std::fs::remove_file(&path);
}

/// Hand a path to the desktop's own handler. Used by the buttons that used to
/// do nothing at all.
pub fn open(path: &std::path::Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("{} does not exist yet", path.display()));
    }
    match run(OPENER, &[&path.display().to_string()]) {
        Some(output) if output.status.success() => Ok(()),
        Some(_) => Err(format!("{OPENER} could not open it")),
        None => Err(format!("{OPENER} is not available")),
    }
}

/// Show a path in the file manager rather than opening the file itself.
pub fn reveal(path: &std::path::Path) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("{} does not exist yet", path.display()));
    }
    platform::reveal(path)
}

#[cfg(test)]
mod tests {
    #[test]
    fn an_existing_daemon_is_detected_but_a_stale_socket_is_not() {
        let dir = std::env::temp_dir().join(format!("offhand-startup-test-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let socket = dir.join("offhand.sock");
        assert!(!super::daemon_listening(&socket));
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        assert!(super::daemon_listening(&socket));
        drop(listener);
        // Polled, not asserted once: a process another test spawns at this
        // moment holds a copy of the listener between its fork and its exec,
        // and the socket answers until that copy closes.
        let stale = (0..100).any(|_| {
            std::thread::sleep(std::time::Duration::from_millis(10));
            !super::daemon_listening(&socket)
        });
        assert!(stale);
        std::fs::remove_dir_all(dir).unwrap();
    }

    use super::*;

    #[test]
    fn a_launch_probe_gets_less_time_than_a_button_press() {
        assert!(STARTUP_BUDGET < BUDGET);
    }

    /// Stop signals whatever the pid file names, so the one thing standing
    /// between that and killing an innocent process is this check. A recycled
    /// pid is exactly what a stale pid file looks like from here.
    #[test]
    fn only_a_process_called_offhand_is_signalled() {
        assert!(!is_offhand_process(std::process::id()));
        assert!(!is_offhand_process(u32::MAX));
    }
}
