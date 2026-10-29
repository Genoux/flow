//! The LaunchAgent that runs Flow on a Mac.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::path::PathBuf;
use std::time::Duration;

use flow_paths::launchd::{Agent, Verb};

use super::{reopen_flow, run, run_for, Channel, BUDGET};

const LABEL: &str = "com.genoux.flow";

pub(crate) const OPENER: &str = "open";

/// The daemon sits beside the console inside the bundle. The app is opened from
/// Finder, whose PATH is `/usr/bin:/bin:...`, so a bare `flow` finds nothing.
pub(crate) fn flow_program() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join("flow")))
        .map_or_else(
            || "flow".to_owned(),
            |path| path.to_string_lossy().into_owned(),
        )
}

/// Every input, as (name, label). CoreAudio's own name is both: it is what the
/// daemon matches `input_device` against and what a person knows the mic by.
///
/// Asked of the daemon binary rather than CoreAudio, so the names are the ones
/// `audio::open_input` will find - see `flow inputs`.
pub(crate) fn input_sources() -> Vec<(String, String)> {
    run(&flow_program(), &["inputs"])
        .map(|output| inputs(&String::from_utf8_lossy(&output.stdout)))
        .unwrap_or_default()
}

/// What Automatic records from: the input the system has as its default.
pub(crate) fn default_input() -> Option<String> {
    let output = run(&flow_program(), &["inputs", "--default"])?;
    let name = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    (!name.is_empty()).then_some(name)
}

/// One name per line, in the order printed, which is already sorted.
fn inputs(listing: &str) -> Vec<(String, String)> {
    listing
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(|name| (name.to_owned(), name.to_owned()))
        .collect()
}

const AGENT: Agent = Agent { label: LABEL };
const TRAY: Agent = Agent {
    label: "com.genoux.flow.tray",
};

fn launchctl_for(budget: Duration) -> impl Fn(&[&str]) -> Option<std::process::Output> {
    move |args| run_for("launchctl", args, budget)
}

fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_owned())
}

pub(super) fn autostart_enabled_for(budget: Duration) -> Option<bool> {
    if !AGENT.plist_installed().ok()? {
        return None;
    }
    AGENT
        .is_disabled(&launchctl_for(budget))
        .ok()
        .map(|disabled| !disabled)
}

/// Only governs loading at login: a job that is already loaded keeps running
/// when disabled, and stays loaded when enabled.
pub(crate) fn set_autostart(enable: bool) -> Result<(), String> {
    AGENT.set_enabled(enable, &launchctl_for(BUDGET))
}

pub(super) fn control(verb: &str) -> Result<(), String> {
    let verb = match verb {
        "start" => Verb::Start,
        "stop" => Verb::Stop,
        "restart" => Verb::Restart,
        other => return Err(format!("unknown service action {other}")),
    };
    AGENT.control(verb, &launchctl_for(BUDGET))
}

pub(super) fn ensure_running() -> Result<(), String> {
    for _ in 0..5 {
        std::thread::sleep(Duration::from_millis(100));
        if AGENT.is_running(&launchctl_for(BUDGET))? {
            return Ok(());
        }
    }
    Err("Flow did not finish starting. Run `flow logs` for details.".into())
}

/// The tray is its own agent, KeepAlive, so it is nearly always up already.
/// This is for after Quit Flow unloaded it. Without its plist - a dev build, or
/// an update that has not rewritten it yet - there is nothing to start, and
/// that is not worth a banner every time the console opens.
pub(crate) fn start_tray() -> Result<(), String> {
    if !TRAY.plist_installed()? {
        return Ok(());
    }
    TRAY.control(Verb::Start, &launchctl_for(BUDGET))
}

/// `ps` gives a bare name or a full path depending on how the process started.
fn comm_is_flow(comm: &str) -> bool {
    std::path::Path::new(comm.trim())
        .file_name()
        .is_some_and(|name| name == "flow")
}

pub(super) fn is_flow_process(pid: u32) -> bool {
    run("ps", &["-p", &pid.to_string(), "-o", "comm="])
        .is_some_and(|output| comm_is_flow(&String::from_utf8_lossy(&output.stdout)))
}

/// `open -n` because the old window is still alive for the moment it takes to
/// exit, and LaunchServices would otherwise just refocus it.
pub(crate) fn restart_app() -> Result<(), String> {
    let bundle = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.ancestors().nth(3).map(std::path::Path::to_path_buf))
        .filter(|path| path.extension().is_some_and(|ext| ext == "app"))
        .ok_or("Flow is not running from an app bundle. Reopen it from Applications.")?;
    super::service("restart")?;
    reopen_flow("open", &["-n", &bundle.to_string_lossy()])
}

pub(crate) fn session() -> String {
    "macOS".to_owned()
}

pub(super) fn reveal(path: &std::path::Path) -> Result<(), String> {
    match run("open", &["-R", &path.display().to_string()]) {
        Some(output) if output.status.success() => Ok(()),
        Some(_) => Err("open could not reveal it".into()),
        None => Err("open is not available".into()),
    }
}

// ponytail: one app slot, so the build in it is stable by definition; an
// experimental channel here needs a second bundle.
pub(crate) fn channel() -> Channel {
    Channel::Stable
}

pub(crate) fn set_channel(_channel: Channel) -> Result<(), String> {
    Ok(())
}

pub(crate) fn installed_binaries(_channel: Channel) -> Result<(PathBuf, String), String> {
    Ok((
        home()?.join("Applications/Flow.app/Contents/MacOS"),
        String::new(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_line_of_the_listing_is_one_input() {
        assert_eq!(
            inputs("MacBook Pro Microphone\nAirPods Pro\n"),
            [
                (
                    "MacBook Pro Microphone".to_owned(),
                    "MacBook Pro Microphone".to_owned()
                ),
                ("AirPods Pro".to_owned(), "AirPods Pro".to_owned()),
            ]
        );
    }

    #[test]
    fn blank_lines_and_padding_are_not_inputs() {
        assert_eq!(
            inputs("\n  Studio Display Microphone  \n\n"),
            [(
                "Studio Display Microphone".to_owned(),
                "Studio Display Microphone".to_owned()
            )]
        );
        assert!(inputs("").is_empty());
    }

    #[test]
    fn ps_may_name_the_process_by_path_or_bare() {
        assert!(comm_is_flow("flow\n"));
        assert!(comm_is_flow(
            "/Users/me/Applications/Flow.app/Contents/MacOS/flow\n"
        ));
        assert!(!comm_is_flow(
            "/Users/me/Applications/Flow.app/Contents/MacOS/flow-console"
        ));
        assert!(!comm_is_flow("workflow"));
        assert!(!comm_is_flow(""));
    }
}
