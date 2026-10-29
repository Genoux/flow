//! Flow's persistent launcher and daemon controller in the system tray.
//!
//! This runs in its own small service. In particular it is not owned by the
//! dictation daemon: the icon is how a stopped daemon can be opened or started,
//! so tying their lifetimes together would remove the recovery control at the
//! exact moment it becomes useful.
//!
//! What the menu says and when the icon is shown is decided here. How it is
//! drawn and what the actions do is the platform's: StatusNotifierItem and
//! systemd on Linux, `NSStatusItem` and launchd on a Mac.

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;

use std::time::Duration;

/// What a menu entry does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Action {
    Open,
    ToggleDictation,
    RestartDictation,
    Quit,
}

/// How the dictation daemon, and only it, is told to change state.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verb {
    Start,
    Stop,
    Restart,
}

impl Verb {
    #[cfg(target_os = "linux")]
    fn as_str(self) -> &'static str {
        match self {
            Verb::Start => "start",
            Verb::Stop => "stop",
            Verb::Restart => "restart",
        }
    }
}

enum Entry {
    Item { label: &'static str, action: Action },
    Separator,
}

fn menu(running: bool) -> [Entry; 6] {
    [
        Entry::Item {
            label: "Open Flow",
            action: Action::Open,
        },
        Entry::Separator,
        Entry::Item {
            label: daemon_action_label(running),
            action: Action::ToggleDictation,
        },
        Entry::Item {
            label: "Restart Dictation",
            action: Action::RestartDictation,
        },
        Entry::Separator,
        Entry::Item {
            label: "Quit Flow",
            action: Action::Quit,
        },
    ]
}

/// A StatusNotifierItem carries one; an `NSStatusItem` shows the menu on click
/// and is not given one.
#[cfg(target_os = "linux")]
fn tooltip(running: bool) -> &'static str {
    if running {
        "Dictation is running. Hold your chord and speak."
    } else {
        "Dictation is stopped. Open Flow or start it from the menu."
    }
}

fn daemon_action_label(running: bool) -> &'static str {
    if running {
        "Stop Dictation"
    } else {
        "Start Dictation"
    }
}

fn perform(action: Action) {
    match action {
        Action::Open => platform::open_console(),
        Action::ToggleDictation => platform::dictation(if platform::dictation_running() {
            Verb::Stop
        } else {
            Verb::Start
        }),
        Action::RestartDictation => platform::dictation(Verb::Restart),
        Action::Quit => platform::quit(),
    }
}

/// Keep the published tray item in step with the live configuration forever.
///
/// The manager owns the handle and explicitly shuts it down when the icon is
/// hidden. Nothing else in the daemon is tied to that handle, so hiding it can
/// never acquire the meaning of the menu's explicit Quit action.
pub fn run(initial: crate::config::Config) -> anyhow::Result<()> {
    let config = std::sync::Arc::new(std::sync::Mutex::new(initial));
    crate::config::watch(std::sync::Arc::clone(&config));
    let mut visible = None;
    let mut icon = None;

    loop {
        let requested = config.lock().expect("config").show_tray;
        if visible != Some(requested) {
            if requested {
                icon = platform::show();
                // A failed registration is worth retrying: the user bus or
                // tray host may still be arriving during login.
                visible = icon.as_ref().map(|_| true);
            } else {
                if let Some(icon) = icon.take() {
                    icon.hide();
                }
                visible = Some(false);
            }
        }
        std::thread::sleep(Duration::from_millis(400));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stopping_dictation_does_not_call_it_quitting_flow() {
        assert_eq!(daemon_action_label(true), "Stop Dictation");
        assert_eq!(daemon_action_label(false), "Start Dictation");
    }

    #[test]
    fn the_menu_offers_what_the_daemon_can_do_now() {
        let labels = |running| -> Vec<&'static str> {
            menu(running)
                .into_iter()
                .filter_map(|entry| match entry {
                    Entry::Item { label, .. } => Some(label),
                    Entry::Separator => None,
                })
                .collect()
        };
        assert_eq!(
            labels(true),
            [
                "Open Flow",
                "Stop Dictation",
                "Restart Dictation",
                "Quit Flow"
            ]
        );
        assert_eq!(labels(false)[1], "Start Dictation");
    }

    /// Quit is the only entry that ends the controller itself, so it must stay
    /// last and be told apart from stopping dictation.
    #[test]
    fn quit_is_last_and_is_not_the_stop_entry() {
        let Some(Entry::Item { action, .. }) = menu(true).into_iter().last() else {
            panic!("the menu ends with a separator");
        };
        assert_eq!(action, Action::Quit);
    }
}
