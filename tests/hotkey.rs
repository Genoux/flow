use evdev::KeyCode;
use flow::hotkey::{chord_broken, chord_released, Chord, Event, Modifier, PttState};
use std::collections::HashSet;
use std::time::Duration;

const OTHER: KeyCode = KeyCode::KEY_C;
const BARE: KeyCode = KeyCode::KEY_RIGHTCTRL;
const SUPER: KeyCode = KeyCode::KEY_LEFTMETA;
const SHIFT: KeyCode = KeyCode::KEY_LEFTSHIFT;
const D: KeyCode = KeyCode::KEY_D;

fn bare() -> PttState {
    PttState::new(Chord::bare(BARE))
}

// -- the default binding ----------------------------------------------------

/// Super+Shift+D, matching the compositor binding people already use, and
/// reachable on every keyboard: Super is one evdev code (KEY_LEFTMETA) whether
/// the keycap says Windows or Command.
#[test]
fn the_default_is_super_shift_d() {
    let chord = Chord::default();
    assert_eq!(chord.trigger, D);
    assert_eq!(chord.modifiers, vec![Modifier::Super, Modifier::Shift]);
    assert_eq!(chord.to_string(), "super+shift+d");
}

/// Every binding needs a floor, chords included. Exempting them meant a
/// frustrated double-tap shipped whatever the recogniser made of 200ms of
/// pre-roll, which is where a run of stray "Yeah." and "Mm." came from.
///
/// The numbers are the measured gap: stray taps held for up to 400ms, real
/// dictations for 2.2s and up.
#[test]
fn a_tap_too_short_to_hold_speech_is_discarded() {
    use flow::hotkey::was_long_enough;

    for stray in [0, 120, 250, 400] {
        assert!(
            !was_long_enough(Duration::from_millis(stray)),
            "{stray}ms should not count as a dictation"
        );
    }
    for real in [700, 2_200, 25_000] {
        assert!(
            was_long_enough(Duration::from_millis(real)),
            "{real}ms is a real hold"
        );
    }
}

/// `deliberate()` still exists, but now only decides whether a stray key cancels.
#[test]
fn only_bare_keys_cancel_on_a_stray_key() {
    assert!(Chord::default().deliberate());
    assert!(!Chord::bare(BARE).deliberate());
}

// -- chord press and release ------------------------------------------------

#[test]
fn the_chord_starts_only_once_every_key_is_down() {
    let mut state = PttState::new(Chord::default());
    assert_eq!(state.apply(SUPER, true), None, "super alone");
    assert_eq!(state.apply(SHIFT, true), None, "super+shift alone");
    assert_eq!(state.apply(D, true), Some(Event::Pressed));
}

#[test]
fn the_trigger_without_its_modifiers_does_nothing() {
    let mut state = PttState::new(Chord::default());
    assert_eq!(state.apply(D, true), None, "a plain d must stay a plain d");
    assert_eq!(state.apply(D, false), None);
}

#[test]
fn a_missing_modifier_does_not_start() {
    let mut state = PttState::new(Chord::default());
    state.apply(SUPER, true);
    assert_eq!(state.apply(D, true), None, "shift was never held");
}

#[test]
fn releasing_the_trigger_ends_the_hold() {
    let mut state = PttState::new(Chord::default());
    state.apply(SUPER, true);
    state.apply(SHIFT, true);
    state.apply(D, true);
    assert!(matches!(state.apply(D, false), Some(Event::Released { .. })));
}

/// The hold ends when any finger lifts, not only the letter. Hyprland's release
/// bind is unreliable with modifier chords, which is why Flow watches this
/// itself - and why lifting Super first has to stop the recording.
#[test]
fn releasing_any_modifier_ends_the_hold() {
    for lifted in [SUPER, SHIFT] {
        let mut state = PttState::new(Chord::default());
        state.apply(SUPER, true);
        state.apply(SHIFT, true);
        state.apply(D, true);
        assert!(
            matches!(state.apply(lifted, false), Some(Event::Released { .. })),
            "lifting {lifted:?} should end the hold"
        );
        assert_eq!(state.apply(D, false), None, "already ended");
    }
}

/// The keys of a chord do not arrive in the order they were pressed. keyd and
/// friends rewrite events and can deliver the letter before the modifier they
/// mapped, and a chord that only starts on the trigger's own event then never
/// starts at all - the user presses, nothing records, and they press again.
#[test]
fn the_chord_starts_whichever_key_completes_it() {
    let orders: [[evdev::KeyCode; 3]; 3] =
        [[D, SUPER, SHIFT], [SHIFT, D, SUPER], [D, SHIFT, SUPER]];

    for order in orders {
        let mut state = PttState::new(Chord::default());
        let events: Vec<_> = order.iter().map(|key| state.apply(*key, true)).collect();
        assert_eq!(
            events.iter().filter(|e| **e == Some(Event::Pressed)).count(),
            1,
            "expected exactly one Pressed for {order:?}, got {events:?}"
        );
        assert_eq!(events[2], Some(Event::Pressed), "must start on the last key of {order:?}");
    }
}

#[test]
fn either_side_of_a_modifier_satisfies_it() {
    let mut state = PttState::new(Chord::default());
    state.apply(KeyCode::KEY_RIGHTMETA, true);
    state.apply(KeyCode::KEY_RIGHTSHIFT, true);
    assert_eq!(state.apply(D, true), Some(Event::Pressed));
}

/// Key repeat while the chord is held must not restart anything.
#[test]
fn autorepeat_on_the_trigger_is_ignored() {
    let mut state = PttState::new(Chord::default());
    state.apply(SUPER, true);
    state.apply(SHIFT, true);
    assert_eq!(state.apply(D, true), Some(Event::Pressed));
    assert_eq!(state.apply(D, true), None, "autorepeat");
    assert_eq!(state.apply(SUPER, true), None, "modifier repeat");
}

/// Losing a dictation is the worst thing this program can do, and cancelling
/// discards the audio silently. A deliberate chord is already unambiguous -
/// nobody reaches for super+shift+d+x - so an unrelated key must not throw the
/// recording away. A remapper echoing a physical key alongside its virtual one
/// is enough to produce exactly that key, which is how whole dictations vanished
/// with nothing in the log.
#[test]
fn a_deliberate_chord_never_discards_on_an_unrelated_key() {
    for intruder in [OTHER, KeyCode::KEY_LEFTCTRL, KeyCode::KEY_LEFTALT, KeyCode::KEY_X] {
        let mut state = PttState::new(Chord::default());
        state.apply(SUPER, true);
        state.apply(SHIFT, true);
        state.apply(D, true);

        assert_eq!(state.apply(intruder, true), None, "{intruder:?} cancelled the hold");
        state.apply(intruder, false);
        assert!(
            matches!(state.apply(D, false), Some(Event::Released { .. })),
            "recording did not survive {intruder:?}"
        );
    }
}

/// The bare-key case keeps cancelling, and must: Right Ctrl is one finger on a
/// key that exists to modify other keys, so Right Ctrl + C is a copy.
#[test]
fn a_bare_key_still_cancels_on_an_extra_key() {
    let mut state = bare();
    state.apply(BARE, true);
    assert_eq!(state.apply(OTHER, true), Some(Event::Cancelled));
    assert_eq!(state.apply(BARE, false), None, "cancelled hold must not dictate");
}

/// After a bare-key combo the trigger is often still down. Pressing further keys
/// must not suddenly start recording just because the trigger happens to be held.
#[test]
fn a_held_bare_key_does_not_start_on_someone_elses_shortcut() {
    let mut state = bare();
    state.apply(BARE, true);
    state.apply(OTHER, true);
    state.apply(OTHER, false);
    state.apply(BARE, false);

    state.apply(BARE, true);
    assert_eq!(state.apply(OTHER, true), Some(Event::Cancelled));
    assert_eq!(state.apply(KeyCode::KEY_X, true), None, "second key");
}

/// Modifiers pressed before the trigger are part of getting to the chord, so
/// they must never look like a cancelling keystroke.
#[test]
fn building_up_to_the_chord_never_cancels() {
    let mut state = PttState::new(Chord::default());
    assert_eq!(state.apply(KeyCode::KEY_LEFTCTRL, true), None);
    assert_eq!(state.apply(KeyCode::KEY_LEFTCTRL, false), None);
    state.apply(SUPER, true);
    state.apply(SHIFT, true);
    assert_eq!(state.apply(D, true), Some(Event::Pressed), "still usable");
}

/// A duplicate report from a second device (keyd leaves the original ungrabbed)
/// must not look like a second press.
#[test]
fn duplicate_chord_reports_are_ignored() {
    let mut state = PttState::new(Chord::default());
    state.apply(SUPER, true);
    state.apply(SUPER, true);
    state.apply(SHIFT, true);
    assert_eq!(state.apply(D, true), Some(Event::Pressed));
    assert_eq!(state.apply(D, true), None);
}

/// Fast taps are the case the project cares most about: a quick Super+Shift+D
/// has to produce a hold, however short.
#[test]
fn a_fast_chord_tap_still_dictates() {
    let mut state = PttState::new(Chord::default());
    state.apply(SUPER, true);
    state.apply(SHIFT, true);
    state.apply(D, true);
    let Some(Event::Released { held }) = state.apply(D, false) else {
        panic!("a tap must still release");
    };
    assert!(held < Duration::from_millis(50), "test tap took {held:?}");
}

#[test]
fn the_chord_can_be_used_twice() {
    let mut state = PttState::new(Chord::default());
    for round in 1..=2 {
        state.apply(SUPER, true);
        state.apply(SHIFT, true);
        assert_eq!(state.apply(D, true), Some(Event::Pressed), "round {round}");
        assert!(matches!(state.apply(D, false), Some(Event::Released { .. })));
        state.apply(SHIFT, false);
        state.apply(SUPER, false);
    }
}

/// Releasing the trigger before a modifier must leave nothing armed, or the next
/// bare modifier lift would fire a phantom release.
#[test]
fn a_modifier_lift_after_the_hold_ended_is_quiet() {
    let mut state = PttState::new(Chord::default());
    state.apply(SUPER, true);
    state.apply(SHIFT, true);
    state.apply(D, true);
    assert!(matches!(state.apply(D, false), Some(Event::Released { .. })));
    assert_eq!(state.apply(SHIFT, false), None);
    assert_eq!(state.apply(SUPER, false), None);
}

// -- bare key, the old behaviour, still intact -------------------------------

#[test]
fn hold_then_release_dictates() {
    let mut state = bare();
    assert_eq!(state.apply(BARE, true), Some(Event::Pressed));
    assert!(matches!(state.apply(BARE, false), Some(Event::Released { .. })));
}

/// Right Ctrl + C must stay a copy. Without this the PTT key would fire on
/// every shortcut that uses it.
#[test]
fn combo_cancels_and_emits_no_release() {
    let mut state = bare();
    state.apply(BARE, true);
    assert_eq!(state.apply(OTHER, true), Some(Event::Cancelled));
    assert_eq!(state.apply(OTHER, false), None);
    assert_eq!(state.apply(BARE, false), None, "cancelled hold must not dictate");
}

#[test]
fn duplicate_device_reports_are_ignored() {
    let mut state = bare();
    assert_eq!(state.apply(BARE, true), Some(Event::Pressed));
    assert_eq!(state.apply(BARE, true), None, "duplicate press");

    assert!(matches!(state.apply(BARE, false), Some(Event::Released { .. })));
    assert_eq!(state.apply(BARE, false), None, "duplicate release");
}

#[test]
fn keys_outside_a_hold_are_ignored() {
    let mut state = bare();
    assert_eq!(state.apply(OTHER, true), None);
    assert_eq!(state.apply(OTHER, false), None);
}

/// Only one Cancelled per hold, however many keys are typed.
#[test]
fn cancel_fires_once_per_hold() {
    let mut state = bare();
    state.apply(BARE, true);
    assert_eq!(state.apply(OTHER, true), Some(Event::Cancelled));
    assert_eq!(state.apply(KeyCode::KEY_X, true), None);
}

// -- writing the binding down ------------------------------------------------

#[test]
fn a_binding_round_trips_through_text() {
    for text in ["super+shift+d", "rightctrl", "ctrl+alt+space", "super+f1"] {
        let parsed = Chord::parse(text).expect(text);
        assert_eq!(parsed.to_string(), text, "{text} did not round-trip");
    }
}

/// evdev orders letters by QWERTY row, not alphabet, and parks F11/F12 away from
/// F1-F10. Both invite arithmetic that silently yields the wrong key.
#[test]
fn every_letter_and_function_key_round_trips() {
    for letter in 'a'..='z' {
        let text = letter.to_string();
        assert_eq!(Chord::parse(&text).expect(&text).to_string(), text);
    }
    for number in 1..=12 {
        let text = format!("f{number}");
        assert_eq!(Chord::parse(&text).expect(&text).to_string(), text);
    }
    for digit in '0'..='9' {
        let text = digit.to_string();
        assert_eq!(Chord::parse(&text).expect(&text).to_string(), text);
    }
    assert_eq!(Chord::parse("d").expect("d").trigger, KeyCode::KEY_D);
    assert_eq!(Chord::parse("s").expect("s").trigger, KeyCode::KEY_S);
}

#[test]
fn binding_text_is_forgiving_about_shape() {
    let expected = Chord::default();
    for text in ["SUPER+SHIFT+D", " super + shift + d ", "meta+shift+d", "win+shift+d"] {
        assert_eq!(Chord::parse(text).expect(text), expected, "{text}");
    }
}

#[test]
fn a_bad_binding_says_which_part_is_wrong() {
    let err = Chord::parse("super+shft+d").expect_err("typo");
    assert!(err.to_string().contains("shft"), "{err}");

    let err = Chord::parse("super+shift").expect_err("no trigger");
    assert!(err.to_string().contains("shift"), "{err}");

    assert!(Chord::parse("").is_err());
    assert!(Chord::parse("super+shift+dd").is_err());
}

/// A chord that is only modifiers can never fire, because the last one pressed
/// would have to be both trigger and modifier.
#[test]
fn a_modifier_cannot_also_be_the_trigger() {
    assert!(Chord::parse("super+super+d").is_err());
    assert!(Chord::parse("shift+shift").is_err());
}

// -- device-backed helpers ---------------------------------------------------

/// The rule injection waits on, tested without a keyboard because the device
/// answer cannot be trusted: a Keychron here reports LALT+LSHIFT held with
/// nothing pressed, and keyd mirrors it as LMETA+LSHIFT - two thirds of the
/// dictation chord. Every paste waited out its timeout and left the text on the
/// clipboard, so the daemon now tracks modifiers from events instead.
#[test]
fn modifiers_are_judged_from_what_was_actually_seen() {
    use flow::hotkey::any_modifier_in;

    assert!(!any_modifier_in(&HashSet::new()), "nothing held");
    assert!(!any_modifier_in(&HashSet::from([KeyCode::KEY_D, KeyCode::KEY_A])), "letters only");
    assert!(any_modifier_in(&HashSet::from([KeyCode::KEY_LEFTMETA])));
    assert!(any_modifier_in(&HashSet::from([KeyCode::KEY_RIGHTSHIFT, KeyCode::KEY_D])));
}

/// Kept as a device-backed smoke test, but it cannot assert timing: a stuck
/// kernel key bit is an environment fact, not a bug in this function.
#[test]
#[ignore]
fn the_device_fallback_still_answers() {
    let started = std::time::Instant::now();
    let released = flow::hotkey::wait_for_modifiers_released(Duration::from_millis(300));
    eprintln!("device fallback: released={released} in {:?}", started.elapsed());
}

/// Compositor hold ends when any finger of the original chord comes up - not
/// only when every key is released, and not only when the letter key lifts.
#[test]
fn chord_breaks_when_any_held_key_lifts() {
    let chord = HashSet::from([KeyCode::KEY_LEFTMETA, KeyCode::KEY_LEFTSHIFT, KeyCode::KEY_D]);
    assert!(!chord_broken(&chord, &chord));

    let mut after = chord.clone();
    after.remove(&KeyCode::KEY_D);
    assert!(chord_broken(&chord, &after));
    assert!(!chord_released(&chord, &after), "shift/super still down");
    assert!(!chord_broken(&chord, &HashSet::new()), "empty read is unknown, not a release");
    assert!(chord_released(&chord, &HashSet::new()));
}

/// The modifier check runs before every paste. It once rediscovered every input
/// device each time, which cost ~500ms of the wait between speaking and seeing
/// text. Needs a readable /dev/input, so it is opt-in:
///   cargo test --release --test hotkey -- --ignored --nocapture
#[test]
#[ignore]
fn the_modifier_check_is_not_a_device_scan() {
    use std::time::Instant;

    // First call may pay discovery once; the daemon warms it at startup.
    flow::hotkey::wait_for_modifiers_released(Duration::from_secs(1));

    let started = Instant::now();
    for _ in 0..5 {
        flow::hotkey::wait_for_modifiers_released(Duration::from_secs(1));
    }
    let each = started.elapsed() / 5;
    eprintln!("modifier check: {each:?} per call");
    assert!(each < Duration::from_millis(5), "{each:?} per call - rediscovering or reopening devices?");
}
