//! A fake live daemon, for looking at the window.
//!
//! The console's most interesting states are the ones that need a running
//! daemon: listening, working, a healthy microphone, models on disk. None of
//! those exist on a machine being used to lay the window out - the daemon is
//! Linux-only and the socket is simply not there - so every screen that
//! matters could only ever be seen in its offline, zeroed form.
//!
//! Set `FLOW_CONSOLE_DEMO` and the window opens as though everything were
//! running. `ready`, `listening` and `working` pick the activity; anything else
//! (`1`, `true`, an empty value) means `listening`, which is the state worth
//! looking at. Off unless the variable is set, so this can never be what a
//! real user sees.

use crate::{daemon, system};

/// What `FLOW_CONSOLE_DEMO` asks for, or `None` when it is unset.
pub fn requested() -> Option<daemon::Activity> {
    let value = std::env::var("FLOW_CONSOLE_DEMO").ok()?;
    Some(match value.trim().to_ascii_lowercase().as_str() {
        "ready" => daemon::Activity::Ready,
        "working" => daemon::Activity::Working,
        "off" | "offline" => daemon::Activity::Offline,
        _ => daemon::Activity::Listening,
    })
}

/// A daemon that is up, has nothing wrong with it, and has been used today.
pub fn daemon_state(activity: daemon::Activity) -> daemon::State {
    daemon::State {
        activity,
        problem: None,
        // Any non-zero value: the console only ever compares this against the
        // previous one to decide whether to re-read the history file.
        words: 1,
    }
}

/// The microphone a machine with a real capture graph would be reporting.
pub fn input() -> Option<String> {
    Some("Scarlett Solo USB Analog Stereo".to_string())
}

/// Whether to force the setup screen on or off, or `None` to let the models on
/// disk decide as usual.
///
/// `FLOW_CONSOLE_DEMO=setup` is the only way to look at first run twice: the
/// real thing happens once per machine and then deletes its own reason to
/// exist. Every other demo value forces it *off*, so the sections stay
/// reachable on a design machine that has no models at all.
pub fn setup() -> Option<bool> {
    let value = std::env::var("FLOW_CONSOLE_DEMO").ok()?;
    Some(value.trim().eq_ignore_ascii_case("setup"))
}

/// An install part-way through the refining model - the state with the most on
/// screen at once, and the only one where the skip control is live.
pub fn setup_state() -> crate::setup::State {
    crate::setup::State {
        total: 3_167_800_824,
        groups: vec![
            ("speech".into(), 670_619_706),
            ("refine".into(), 2_497_181_118),
        ],
        done: 1_402_000_000,
        phase: crate::setup::Phase::Fetching("qwen3-4b-instruct-q4km.gguf".into()),
        hardware: Some("NVIDIA GeForce RTX 3060 Ti".into()),
        ..crate::setup::State::default()
    }
}

/// Both models present and sized as the pinned releases actually are, so the
/// Models screen can be laid out against the numbers it will really show.
pub fn models() -> Vec<system::Model> {
    system::models()
        .into_iter()
        .zip([
            ("parakeet-tdt-0.6b-v3 int8", 652_000_000),
            ("qwen3-4b-instruct-q4km.gguf", 2_497_000_000),
        ])
        .map(|(model, (detail, bytes))| system::Model {
            detail: detail.to_string(),
            bytes,
            installed: true,
            ..model
        })
        .collect()
}
