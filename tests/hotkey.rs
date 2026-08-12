use evdev::KeyCode;
use flow::hotkey::{Event, PttState, PTT};
use std::time::Duration;

const OTHER: KeyCode = KeyCode::KEY_C;

#[test]
fn hold_then_release_dictates() {
    let mut state = PttState::default();
    assert_eq!(state.apply(PTT, true), Some(Event::Pressed));
    assert!(matches!(
        state.apply(PTT, false),
        Some(Event::Released { .. })
    ));
}

/// Right Ctrl + C must stay a copy. Without this the PTT key would fire on
/// every shortcut that uses it.
#[test]
fn combo_cancels_and_emits_no_release() {
    let mut state = PttState::default();
    state.apply(PTT, true);
    assert_eq!(state.apply(OTHER, true), Some(Event::Cancelled));
    assert_eq!(state.apply(OTHER, false), None);
    assert_eq!(state.apply(PTT, false), None, "cancelled hold must not dictate");
}

/// A second device reporting the same physical press must not start a second
/// recording - keyd may leave the original keyboard ungrabbed alongside its
/// virtual one.
#[test]
fn duplicate_device_reports_are_ignored() {
    let mut state = PttState::default();
    assert_eq!(state.apply(PTT, true), Some(Event::Pressed));
    assert_eq!(state.apply(PTT, true), None, "duplicate press");

    assert!(matches!(
        state.apply(PTT, false),
        Some(Event::Released { .. })
    ));
    assert_eq!(state.apply(PTT, false), None, "duplicate release");
}

#[test]
fn keys_outside_a_hold_are_ignored() {
    let mut state = PttState::default();
    assert_eq!(state.apply(OTHER, true), None);
    assert_eq!(state.apply(OTHER, false), None);
}

/// Only one Cancelled per hold, however many keys are typed.
#[test]
fn cancel_fires_once_per_hold() {
    let mut state = PttState::default();
    state.apply(PTT, true);
    assert_eq!(state.apply(OTHER, true), Some(Event::Cancelled));
    assert_eq!(state.apply(KeyCode::KEY_X, true), None);
}

/// With nothing held this must return promptly, not spin to the timeout -
/// injection waits on it, so a false negative would stall every dictation.
#[test]
fn idle_keyboard_reports_modifiers_released() {
    let started = std::time::Instant::now();
    let released = flow::hotkey::wait_for_modifiers_released(Duration::from_secs(2));
    let elapsed = started.elapsed();

    if !released {
        eprintln!("skipping: a modifier is physically held right now");
        return;
    }
    assert!(elapsed < Duration::from_millis(500), "took {elapsed:?}");
}
