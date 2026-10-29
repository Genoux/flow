//! Starting and stopping a LaunchAgent the way Flow means it to be started
//! and stopped.
//!
//! Shared because two programs do it - the console's Start and Stop, and the
//! menu bar's - and they have to agree on the corners: `bootstrap` refuses a
//! label disabled for login, which is how Launch at login is stored, so a
//! start has to enable it for the instant of loading and put the choice back.
//!
//! Every launchctl call goes through a [`Runner`], so the caller decides how
//! long to wait for it. Nothing here is macOS-specific code, which is what lets
//! the decisions be tested anywhere.

use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::process::Output;
use std::time::Duration;

const NOT_FOUND_EXIT: i32 = 113;
const BOOTOUT_POLLS: u32 = 20;

/// Runs `launchctl` with these arguments, `None` when it did not answer.
pub type Runner<'a> = &'a dyn Fn(&[&str]) -> Option<Output>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verb {
    Start,
    Stop,
    Restart,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Job {
    NotLoaded,
    Loaded { running: bool },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Step {
    Bootout,
    Enable,
    Bootstrap,
    Disable,
    Kickstart,
}

/// A user LaunchAgent, named by its label. Its plist is where the installer
/// puts it.
#[derive(Debug, Clone, Copy)]
pub struct Agent {
    pub label: &'static str,
}

impl Agent {
    pub fn control(&self, verb: Verb, run: Runner) -> Result<(), String> {
        let job = self.job(run)?;
        let disabled = verb != Verb::Stop && self.is_disabled(run)?;
        let mut outcome = Ok(());
        for step in plan(verb, job, disabled) {
            // The restore runs even after a failed bootstrap, or the label would
            // stay enabled for login against the user's choice.
            if outcome.is_ok() || step == Step::Disable {
                outcome = outcome.and(self.apply(step, run));
            }
        }
        outcome
    }

    pub fn is_running(&self, run: Runner) -> Result<bool, String> {
        Ok(self.job(run)? == (Job::Loaded { running: true }))
    }

    /// Whether the label is disabled for login. Absent from `print-disabled`
    /// means never disabled, which launchd treats as enabled.
    pub fn is_disabled(&self, run: Runner) -> Result<bool, String> {
        let output = run(&["print-disabled", &domain()?])
            .ok_or_else(|| "launchctl did not respond".to_owned())?;
        if !output.status.success() {
            return Err(failure_reason(&output, "launchctl print-disabled failed"));
        }
        Ok(!enabled(
            &String::from_utf8_lossy(&output.stdout),
            self.label,
        ))
    }

    pub fn plist_installed(&self) -> Result<bool, String> {
        Ok(self.plist()?.is_file())
    }

    /// Only governs loading at login: a job that is already loaded keeps
    /// running when disabled, and stays loaded when enabled.
    pub fn set_enabled(&self, enable: bool, run: Runner) -> Result<(), String> {
        self.apply(if enable { Step::Enable } else { Step::Disable }, run)
    }

    fn target(&self) -> Result<String, String> {
        Ok(format!("{}/{}", domain()?, self.label))
    }

    fn plist(&self) -> Result<PathBuf, String> {
        Ok(home()?
            .join("Library/LaunchAgents")
            .join(format!("{}.plist", self.label)))
    }

    fn job(&self, run: Runner) -> Result<Job, String> {
        let output = run(&["print", &self.target()?])
            .ok_or_else(|| "launchctl did not respond".to_owned())?;
        classify_print(
            output.status.code(),
            &String::from_utf8_lossy(&output.stdout),
            &failure_reason(&output, "launchctl print failed"),
        )
    }

    fn apply(&self, step: Step, run: Runner) -> Result<(), String> {
        let target = self.target()?;
        match step {
            Step::Bootout => {
                launchctl(run, &["bootout", &target])?;
                self.wait_until_unloaded(run)
            }
            Step::Enable => launchctl(run, &["enable", &target]),
            Step::Disable => launchctl(run, &["disable", &target]),
            Step::Kickstart => launchctl(run, &["kickstart", &target]),
            Step::Bootstrap => {
                let plist = self.plist()?;
                if !plist.is_file() {
                    return Err(
                        "Flow's LaunchAgent is not installed. Run the installer again.".into(),
                    );
                }
                launchctl(run, &["bootstrap", &domain()?, &plist.to_string_lossy()])
            }
        }
    }

    /// `bootout` can return while a slow-exiting job is still listed, and
    /// bootstrapping the label then fails with EIO.
    fn wait_until_unloaded(&self, run: Runner) -> Result<(), String> {
        let mut polls = 0;
        while self.job(run)? != Job::NotLoaded {
            if polls == BOOTOUT_POLLS {
                return Err("Flow is still shutting down. Try again in a moment.".into());
            }
            polls += 1;
            std::thread::sleep(Duration::from_millis(100));
        }
        Ok(())
    }
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

fn failure_reason(output: &Output, fallback: &str) -> String {
    let reason = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    if reason.is_empty() {
        fallback.to_owned()
    } else {
        reason
    }
}

fn launchctl(run: Runner, args: &[&str]) -> Result<(), String> {
    let output = run(args).ok_or_else(|| "launchctl did not respond".to_owned())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(failure_reason(
            &output,
            &format!("launchctl {} failed", args[0]),
        ))
    }
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
fn enabled(listing: &str, label: &str) -> bool {
    !listing
        .lines()
        .any(|line| line.trim() == format!("\"{label}\" => disabled"))
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
        assert!(!enabled(listing, "com.genoux.flow"));
        assert!(enabled(listing, "com.other"));
        assert!(enabled(listing, "com.absent"));
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
}
