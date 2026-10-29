#![cfg(target_os = "macos")]

//! The macOS event tap speaks CGKeyCodes and modifier flag bits; everything
//! downstream speaks evdev. These tables are the whole seam, so they are pinned
//! against Apple's `kVK_*` values, which are literals here on purpose: a test
//! that read the same constants it checks would agree with any typo in them.

use evdev::KeyCode;
use flow::hotkey::macos::{
    KEYMAP, key_from_virtual_keycode, modifier_transition, modifiers_from_flags, stale_keys,
    virtual_keycode_from_key,
};
use flow::hotkey::{Chord, any_modifier_in};
use std::collections::HashSet;

const SHIFT: u64 = 0x0002_0000;
const CONTROL: u64 = 0x0004_0000;
const OPTION: u64 = 0x0008_0000;
const COMMAND: u64 = 0x0010_0000;
const CAPS_LOCK: u64 = 0x0001_0000;
const SECONDARY_FN: u64 = 0x0080_0000;

const LEFT_CONTROL: u64 = 0x0000_0001;
const LEFT_SHIFT: u64 = 0x0000_0002;
const RIGHT_SHIFT: u64 = 0x0000_0004;
const LEFT_COMMAND: u64 = 0x0000_0008;
const RIGHT_COMMAND: u64 = 0x0000_0010;
const LEFT_OPTION: u64 = 0x0000_0020;
const RIGHT_OPTION: u64 = 0x0000_0040;
const RIGHT_CONTROL: u64 = 0x0000_2000;

#[test]
fn every_entry_round_trips_in_both_directions() {
    assert!(!KEYMAP.is_empty());
    for (code, key) in KEYMAP {
        assert_eq!(key_from_virtual_keycode(code), Some(key), "{code:#04x}");
        assert_eq!(virtual_keycode_from_key(key), Some(code), "{key:?}");
    }
}

#[test]
fn no_code_or_key_appears_twice() {
    let codes: HashSet<u16> = KEYMAP.iter().map(|(code, _)| *code).collect();
    let keys: HashSet<KeyCode> = KEYMAP.iter().map(|(_, key)| *key).collect();
    assert_eq!(codes.len(), KEYMAP.len(), "a CGKeyCode is listed twice");
    assert_eq!(keys.len(), KEYMAP.len(), "a KeyCode is listed twice");
}

/// ANSI letters follow the physical QWERTY layout on a Mac too, not the
/// alphabet - kVK_ANSI_A is 0x00 and kVK_ANSI_S is 0x01 - so a spot check is
/// the only thing that catches arithmetic creeping in.
#[test]
fn letters_match_the_ansi_virtual_keycodes() {
    let expected = [
        (0x00, KeyCode::KEY_A),
        (0x01, KeyCode::KEY_S),
        (0x02, KeyCode::KEY_D),
        (0x06, KeyCode::KEY_Z),
        (0x09, KeyCode::KEY_V),
        (0x0B, KeyCode::KEY_B),
        (0x0C, KeyCode::KEY_Q),
        (0x2E, KeyCode::KEY_M),
        (0x10, KeyCode::KEY_Y),
    ];
    for (code, key) in expected {
        assert_eq!(key_from_virtual_keycode(code), Some(key), "{code:#04x}");
    }
}

#[test]
fn digits_are_not_sequential() {
    let expected = [
        (0x12, KeyCode::KEY_1),
        (0x13, KeyCode::KEY_2),
        (0x15, KeyCode::KEY_4),
        (0x17, KeyCode::KEY_5),
        (0x16, KeyCode::KEY_6),
        (0x19, KeyCode::KEY_9),
        (0x1D, KeyCode::KEY_0),
    ];
    for (code, key) in expected {
        assert_eq!(key_from_virtual_keycode(code), Some(key), "{code:#04x}");
    }
}

#[test]
fn function_keys_match_the_virtual_keycodes() {
    let expected = [
        (0x7A, KeyCode::KEY_F1),
        (0x78, KeyCode::KEY_F2),
        (0x63, KeyCode::KEY_F3),
        (0x60, KeyCode::KEY_F5),
        (0x6D, KeyCode::KEY_F10),
        (0x67, KeyCode::KEY_F11),
        (0x6F, KeyCode::KEY_F12),
    ];
    for (code, key) in expected {
        assert_eq!(key_from_virtual_keycode(code), Some(key), "{code:#04x}");
    }
}

#[test]
fn named_keys_and_modifiers_match_the_virtual_keycodes() {
    let expected = [
        (0x24, KeyCode::KEY_ENTER),
        (0x30, KeyCode::KEY_TAB),
        (0x31, KeyCode::KEY_SPACE),
        (0x35, KeyCode::KEY_ESC),
        (0x36, KeyCode::KEY_RIGHTMETA),
        (0x37, KeyCode::KEY_LEFTMETA),
        (0x38, KeyCode::KEY_LEFTSHIFT),
        (0x3A, KeyCode::KEY_LEFTALT),
        (0x3B, KeyCode::KEY_LEFTCTRL),
        (0x3C, KeyCode::KEY_RIGHTSHIFT),
        (0x3D, KeyCode::KEY_RIGHTALT),
        (0x3E, KeyCode::KEY_RIGHTCTRL),
    ];
    for (code, key) in expected {
        assert_eq!(key_from_virtual_keycode(code), Some(key), "{code:#04x}");
    }
}

#[test]
fn a_key_nobody_mapped_is_unknown_not_a_guess() {
    assert_eq!(key_from_virtual_keycode(0x7F), None);
    assert_eq!(virtual_keycode_from_key(KeyCode::KEY_UNKNOWN), None);
}

/// Anything `Chord::parse` accepts as a trigger has to be reachable from the
/// tap, or the binding saves fine and never fires. Caps Lock is the exception:
/// macOS reports it as a toggle, not a hold.
#[test]
fn every_trigger_a_chord_can_name_has_a_virtual_keycode() {
    let letters = ('a'..='z').map(String::from);
    let digits = ('0'..='9').map(String::from);
    let function = (1..=12).map(|number| format!("f{number}"));
    let named = [
        "space",
        "tab",
        "enter",
        "leftctrl",
        "rightctrl",
        "leftalt",
        "rightalt",
        "leftshift",
        "rightshift",
        "leftmeta",
        "rightmeta",
        "fn",
    ]
    .map(String::from);
    for name in letters.chain(digits).chain(function).chain(named) {
        let chord = Chord::parse(&name).expect(&name);
        assert!(
            virtual_keycode_from_key(chord.trigger).is_some(),
            "{name} has no CGKeyCode"
        );
    }
}

#[test]
fn a_modifier_going_down_reports_its_own_side() {
    assert_eq!(
        modifier_transition(0x37, COMMAND | LEFT_COMMAND),
        Some((KeyCode::KEY_LEFTMETA, true))
    );
    assert_eq!(
        modifier_transition(0x36, COMMAND | RIGHT_COMMAND),
        Some((KeyCode::KEY_RIGHTMETA, true))
    );
    assert_eq!(
        modifier_transition(0x3B, CONTROL | LEFT_CONTROL),
        Some((KeyCode::KEY_LEFTCTRL, true))
    );
    assert_eq!(
        modifier_transition(0x3E, CONTROL | RIGHT_CONTROL),
        Some((KeyCode::KEY_RIGHTCTRL, true))
    );
    assert_eq!(
        modifier_transition(0x3A, OPTION | LEFT_OPTION),
        Some((KeyCode::KEY_LEFTALT, true))
    );
    assert_eq!(
        modifier_transition(0x3D, OPTION | RIGHT_OPTION),
        Some((KeyCode::KEY_RIGHTALT, true))
    );
}

#[test]
fn a_modifier_coming_up_leaves_the_flags_without_its_bit() {
    assert_eq!(
        modifier_transition(0x37, 0),
        Some((KeyCode::KEY_LEFTMETA, false))
    );
    assert_eq!(
        modifier_transition(0x38, SHIFT | RIGHT_SHIFT | COMMAND | LEFT_COMMAND),
        Some((KeyCode::KEY_LEFTSHIFT, false))
    );
}

/// Both shifts down, left released: the family flag is still set because the
/// right one holds it, so only the per-side bit can say the left one went up.
#[test]
fn releasing_one_shift_while_the_other_is_held_is_a_release() {
    assert_eq!(
        modifier_transition(0x38, SHIFT | RIGHT_SHIFT),
        Some((KeyCode::KEY_LEFTSHIFT, false))
    );
    assert_eq!(
        modifier_transition(0x3C, SHIFT | LEFT_SHIFT),
        Some((KeyCode::KEY_RIGHTSHIFT, false))
    );
}

/// Synthetic events, from a remapper or an accessibility tool, can carry the
/// family flag and no side bits at all.
#[test]
fn flags_without_side_bits_fall_back_to_the_family_flag() {
    assert_eq!(
        modifier_transition(0x38, SHIFT),
        Some((KeyCode::KEY_LEFTSHIFT, true))
    );
    assert_eq!(
        modifier_transition(0x38, 0),
        Some((KeyCode::KEY_LEFTSHIFT, false))
    );
}

#[test]
fn a_key_that_is_not_a_modifier_is_not_a_transition() {
    assert_eq!(modifier_transition(0x00, SHIFT), None);
    assert_eq!(modifier_transition(0x39, CAPS_LOCK), None);
}

#[test]
fn flags_name_the_modifiers_that_are_down() {
    let mut held = modifiers_from_flags(COMMAND | LEFT_COMMAND | SHIFT | RIGHT_SHIFT);
    held.sort_by_key(|key| key.0);
    let mut expected = vec![KeyCode::KEY_LEFTMETA, KeyCode::KEY_RIGHTSHIFT];
    expected.sort_by_key(|key| key.0);
    assert_eq!(held, expected);
}

#[test]
fn flags_with_only_family_bits_name_the_left_key() {
    assert_eq!(modifiers_from_flags(OPTION), vec![KeyCode::KEY_LEFTALT]);
}

#[test]
fn flags_that_are_not_modifiers_name_nothing() {
    assert!(modifiers_from_flags(0).is_empty());
    assert!(modifiers_from_flags(CAPS_LOCK | 0x0020_0000 | 0x0080_0000).is_empty());
}

fn stale(held: &[KeyCode], flags: u64, down: &[u16]) -> Vec<KeyCode> {
    let held: HashSet<KeyCode> = held.iter().copied().collect();
    let mut stale = stale_keys(&held, flags, |code| down.contains(&code));
    stale.sort_by_key(|key| key.0);
    stale
}

/// macOS turns a tap off when its callback is slow and says nothing about what
/// happened meanwhile, so any key released in that window is still believed
/// held until something looks at the real state.
#[test]
fn a_modifier_the_system_no_longer_holds_is_stale() {
    assert_eq!(
        stale(
            &[KeyCode::KEY_LEFTMETA, KeyCode::KEY_LEFTSHIFT],
            SHIFT | LEFT_SHIFT,
            &[]
        ),
        vec![KeyCode::KEY_LEFTMETA]
    );
}

#[test]
fn a_modifier_still_held_is_not_stale() {
    assert!(stale(&[KeyCode::KEY_LEFTMETA], COMMAND | LEFT_COMMAND, &[]).is_empty());
}

#[test]
fn a_trigger_key_is_judged_by_its_own_state() {
    assert_eq!(stale(&[KeyCode::KEY_D], 0, &[]), vec![KeyCode::KEY_D]);
    assert!(stale(&[KeyCode::KEY_D], 0, &[0x02]).is_empty());
}

#[test]
fn a_key_with_no_virtual_keycode_cannot_be_checked_so_it_is_released() {
    assert_eq!(
        stale(&[KeyCode::KEY_UNKNOWN], 0, &[]),
        vec![KeyCode::KEY_UNKNOWN]
    );
}

#[test]
fn nothing_believed_held_means_nothing_stale() {
    assert!(stale(&[], COMMAND | LEFT_COMMAND, &[0x00]).is_empty());
}

// -- Fn / globe ---------------------------------------------------------------

#[test]
fn the_function_key_is_in_the_table() {
    assert_eq!(key_from_virtual_keycode(0x3F), Some(KeyCode::KEY_FN));
    assert_eq!(virtual_keycode_from_key(KeyCode::KEY_FN), Some(0x3F));
}

#[test]
fn the_default_binding_is_fn_alone() {
    assert_eq!(Chord::default().to_string(), "fn");
    assert_eq!(Chord::default(), Chord::parse("fn").unwrap());
}

/// Fn reports through flagsChanged like a modifier, and the one bit that says
/// whether it is down is the secondary-fn flag.
#[test]
fn fn_goes_down_and_up_with_its_flag() {
    assert_eq!(
        modifier_transition(0x3F, SECONDARY_FN),
        Some((KeyCode::KEY_FN, true))
    );
    assert_eq!(modifier_transition(0x3F, 0), Some((KeyCode::KEY_FN, false)));
    assert_eq!(
        modifier_transition(0x3F, SHIFT | LEFT_SHIFT),
        Some((KeyCode::KEY_FN, false)),
        "another modifier's flags say nothing about Fn being down"
    );
}

/// macOS also sets the secondary-fn bit on arrows, F-keys, Home, End, PageUp,
/// PageDown and forward delete. It is only evidence of Fn on Fn's own event.
#[test]
fn the_flag_on_an_arrow_key_is_not_fn() {
    for arrow in [0x7B, 0x7C, 0x7D, 0x7E, 0x73, 0x77, 0x74, 0x79, 0x75, 0x7A] {
        assert_eq!(
            modifier_transition(arrow, SECONDARY_FN),
            None,
            "{arrow:#04x}"
        );
    }
    assert!(modifiers_from_flags(SECONDARY_FN).is_empty());
}

#[test]
fn a_held_fn_holds_back_the_paste() {
    let held: HashSet<KeyCode> = [KeyCode::KEY_FN].into();
    assert!(any_modifier_in(&held));
}

/// The flag is set while an arrow is held, so it cannot say whether Fn is: the
/// key's own state does.
#[test]
fn fn_is_stale_by_key_state_not_by_flag() {
    assert_eq!(
        stale(&[KeyCode::KEY_FN], SECONDARY_FN, &[]),
        vec![KeyCode::KEY_FN]
    );
    assert!(stale(&[KeyCode::KEY_FN], 0, &[0x3F]).is_empty());
}
