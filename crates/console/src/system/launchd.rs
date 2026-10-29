//! The LaunchAgent that runs Flow on a Mac.
#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::time::Duration;

use super::{failure_reason, reopen_flow, require_success, run, run_for, Channel, BUDGET};

const LABEL: &str = "com.genoux.flow";
const NOT_FOUND_EXIT: i32 = 113;
const BOOTOUT_POLLS: u32 = 20;

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

fn home() -> Result<PathBuf, String> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| "HOME is not set".to_owned())
}

// The owner of $HOME is the login user. `id -u` was the alternative, and a
// spawn on the synchronous startup path can block it.
fn domain() -> Result<String, String> {
    let home = home()?;
    let owner = std::fs::metadata(&home).map_err(|err| format!("{}: {err}", home.display()))?;
    Ok(format!("gui/{}", owner.uid()))
}

fn target() -> Result<String, String> {
    Ok(format!("{}/{LABEL}", domain()?))
}

fn launch_agent() -> Result<PathBuf, String> {
    Ok(home()?
        .join("Library/LaunchAgents")
        .join(format!("{LABEL}.plist")))
}

fn launchctl(args: &[&str]) -> Result<(), String> {
    require_success("launchctl", args, &format!("launchctl {} failed", args[0]))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Job {
    NotLoaded,
    Loaded { running: bool },
}

fn print_job() -> Result<Job, String> {
    let output = run("launchctl", &["print", &target()?])
        .ok_or_else(|| "launchctl did not respond".to_owned())?;
    classify_print(
        output.status.code(),
        &String::from_utf8_lossy(&output.stdout),
        &failure_reason(&output, "launchctl print failed"),
    )
}

/// Only exit 113 with launchd's own message means "not loaded"; a timeout or any
/// other failure says nothing about the job and must not read as absence.
fn classify_print(code: Option<i32>, stdout: &str, stderr: &str) -> Result<Job, String> {
    match code {
        Some(0) => Ok(Job::Loaded {
            running: launchd_state(stdout) == Some("running"),
        }),
        Some(NOT_FOUND_EXIT) if stderr.contains("Could not find service") => Ok(Job::NotLoaded),
        _ => Err(stderr.to_owned()),
    }
}

/// The job's own state out of `launchctl print`. Its nested blocks repeat the
/// key (`state = active`), so only the first line counts.
fn launchd_state(listing: &str) -> Option<&str> {
    listing
        .lines()
        .find_map(|line| line.trim().strip_prefix("state = "))
}

/// Absent from `print-disabled` means never disabled, which launchd treats as
/// enabled.
fn launchd_enabled(listing: &str, label: &str) -> bool {
    !listing
        .lines()
        .any(|line| line.trim() == format!("\"{label}\" => disabled"))
}

fn is_disabled(budget: Duration) -> Result<bool, String> {
    let output = run_for("launchctl", &["print-disabled", &domain()?], budget)
        .ok_or_else(|| "launchctl did not respond".to_owned())?;
    if !output.status.success() {
        return Err(failure_reason(&output, "launchctl print-disabled failed"));
    }
    Ok(!launchd_enabled(
        &String::from_utf8_lossy(&output.stdout),
        LABEL,
    ))
}

pub(super) fn autostart_enabled_for(budget: Duration) -> Option<bool> {
    if !launch_agent().ok()?.is_file() {
        return None;
    }
    is_disabled(budget).ok().map(|disabled| !disabled)
}

/// Only governs loading at login: a job that is already loaded keeps running
/// when disabled, and stays loaded when enabled.
pub(crate) fn set_autostart(enable: bool) -> Result<(), String> {
    apply(if enable { Step::Enable } else { Step::Disable })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verb {
    Start,
    Stop,
    Restart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Bootout,
    Enable,
    Bootstrap,
    Disable,
    Kickstart,
}

/// `bootout` and `bootstrap` rather than `kickstart -k`, so a plist rewritten
/// by an update is re-read. `bootstrap` refuses a label disabled for login, so
/// when Launch at login is off it is enabled for the instant of loading and
/// disabled again: that choice is the user's, not ours.
fn plan(verb: Verb, job: Job, disabled: bool) -> Vec<Step> {
    let load = || {
        if disabled {
            vec![Step::Enable, Step::Bootstrap, Step::Disable]
        } else {
            vec![Step::Bootstrap]
        }
    };
    match (verb, job) {
        (Verb::Stop, Job::NotLoaded) => vec![],
        (Verb::Stop, Job::Loaded { .. }) => vec![Step::Bootout],
        (Verb::Start, Job::NotLoaded) | (Verb::Restart, Job::NotLoaded) => load(),
        (Verb::Start, Job::Loaded { running: true }) => vec![],
        (Verb::Start, Job::Loaded { running: false }) => vec![Step::Kickstart],
        (Verb::Restart, Job::Loaded { .. }) => [vec![Step::Bootout], load()].concat(),
    }
}

pub(super) fn control(verb: &str) -> Result<(), String> {
    let verb = match verb {
        "start" => Verb::Start,
        "stop" => Verb::Stop,
        "restart" => Verb::Restart,
        other => return Err(format!("unknown service action {other}")),
    };
    let job = print_job()?;
    let disabled = verb != Verb::Stop && is_disabled(BUDGET)?;
    let mut outcome = Ok(());
    for step in plan(verb, job, disabled) {
        // The restore runs even after a failed bootstrap, or the label would
        // stay enabled for login against the user's choice.
        if outcome.is_ok() || step == Step::Disable {
            outcome = outcome.and(apply(step));
        }
    }
    outcome
}

fn apply(step: Step) -> Result<(), String> {
    let target = target()?;
    match step {
        Step::Bootout => {
            launchctl(&["bootout", &target])?;
            wait_until_unloaded()
        }
        Step::Enable => launchctl(&["enable", &target]),
        Step::Disable => launchctl(&["disable", &target]),
        Step::Kickstart => launchctl(&["kickstart", &target]),
        Step::Bootstrap => {
            let plist = launch_agent()?;
            if !plist.is_file() {
                return Err("Flow's LaunchAgent is not installed. Run the installer again.".into());
            }
            launchctl(&["bootstrap", &domain()?, &plist.to_string_lossy()])
        }
    }
}

/// `bootout` can return while a slow-exiting job is still listed, and
/// bootstrapping the label then fails with EIO.
fn wait_until_unloaded() -> Result<(), String> {
    let mut polls = 0;
    while print_job()? != Job::NotLoaded {
        if polls == BOOTOUT_POLLS {
            return Err("Flow is still shutting down. Try again in a moment.".into());
        }
        polls += 1;
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

pub(super) fn ensure_running() -> Result<(), String> {
    for _ in 0..5 {
        std::thread::sleep(Duration::from_millis(100));
        if print_job()? == (Job::Loaded { running: true }) {
            return Ok(());
        }
    }
    Err("Flow did not finish starting. Run `flow logs` for details.".into())
}

pub(crate) fn start_tray() -> Result<(), String> {
    Ok(())
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

    const LOADED: &str = "gui/501/com.genoux.flow = {\n\tactive count = 1\n\tpath = /x\n\tstate = running\n\n\truns = 1\n\t\tstate = active\n}";
    const STOPPED: &str = "gui/501/com.genoux.flow = {\n\tpath = /x\n\tstate = not running\n}";
    const NOT_FOUND: &str =
        "Could not find service \"com.genoux.flow\" in domain for user gui: 501";

    /// The nested `state = active` lines come after the job's own and must not
    /// be mistaken for it.
    #[test]
    fn the_job_state_is_the_first_state_line() {
        assert_eq!(launchd_state(LOADED), Some("running"));
        assert_eq!(launchd_state(STOPPED), Some("not running"));
        assert_eq!(launchd_state(""), None);
    }

    #[test]
    fn print_output_is_loaded_not_loaded_or_an_error() {
        assert_eq!(
            classify_print(Some(0), LOADED, ""),
            Ok(Job::Loaded { running: true })
        );
        assert_eq!(
            classify_print(Some(0), STOPPED, ""),
            Ok(Job::Loaded { running: false })
        );
        assert_eq!(
            classify_print(Some(NOT_FOUND_EXIT), "", NOT_FOUND),
            Ok(Job::NotLoaded)
        );
        assert!(classify_print(Some(NOT_FOUND_EXIT), "", "something else").is_err());
        assert!(classify_print(Some(1), "", NOT_FOUND).is_err());
        assert!(classify_print(None, "", "launchctl print failed").is_err());
    }

    #[test]
    fn a_label_is_enabled_unless_print_disabled_says_otherwise() {
        let listing = "disabled services = {\n\t\"com.genoux.flow\" => disabled\n\t\"com.other\" => enabled\n}";
        assert!(!launchd_enabled(listing, "com.genoux.flow"));
        assert!(launchd_enabled(listing, "com.other"));
        assert!(launchd_enabled(listing, "com.absent"));
    }

    #[test]
    fn nothing_loaded_is_bootstrapped_and_the_login_choice_restored() {
        let (stopped, running) = (
            Job::Loaded { running: false },
            Job::Loaded { running: true },
        );
        use Step::*;
        assert_eq!(plan(Verb::Start, Job::NotLoaded, false), [Bootstrap]);
        assert_eq!(
            plan(Verb::Start, Job::NotLoaded, true),
            [Enable, Bootstrap, Disable]
        );
        assert_eq!(plan(Verb::Start, running, true), []);
        assert_eq!(plan(Verb::Start, stopped, true), [Kickstart]);
        assert_eq!(plan(Verb::Stop, Job::NotLoaded, true), []);
        assert_eq!(plan(Verb::Stop, running, false), [Bootout]);
        assert_eq!(plan(Verb::Stop, stopped, false), [Bootout]);
    }

    #[test]
    fn restart_unloads_before_loading_again() {
        use Step::*;
        for job in [
            Job::Loaded { running: true },
            Job::Loaded { running: false },
        ] {
            assert_eq!(plan(Verb::Restart, job, false), [Bootout, Bootstrap]);
            assert_eq!(
                plan(Verb::Restart, job, true),
                [Bootout, Enable, Bootstrap, Disable]
            );
        }
        assert_eq!(plan(Verb::Restart, Job::NotLoaded, false), [Bootstrap]);
        assert_eq!(
            plan(Verb::Restart, Job::NotLoaded, true),
            [Enable, Bootstrap, Disable]
        );
    }

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
