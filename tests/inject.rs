use evdev::KeyCode;

/// The injector must never emit Super or a letter. Those are still held on the
/// physical board when `d` comes up, and a typed character would become a
/// shortcut - SUPER+m exits the session on this machine.
#[test]
fn paste_is_a_chord_not_typed_characters() {
    let normal = flow::inject::paste_keys(false);
    assert_eq!(normal, vec![KeyCode::KEY_LEFTCTRL, KeyCode::KEY_V]);
    assert!(!normal.contains(&KeyCode::KEY_LEFTMETA));
    assert!(!normal.contains(&KeyCode::KEY_LEFTSHIFT));
}

#[test]
fn terminal_paste_adds_shift() {
    assert_eq!(
        flow::inject::paste_keys(true),
        vec![KeyCode::KEY_LEFTCTRL, KeyCode::KEY_LEFTSHIFT, KeyCode::KEY_V]
    );
}
