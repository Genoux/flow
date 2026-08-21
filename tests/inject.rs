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
        vec![
            KeyCode::KEY_LEFTCTRL,
            KeyCode::KEY_LEFTSHIFT,
            KeyCode::KEY_V
        ]
    );
}

#[test]
fn terminal_classes_are_recognised() {
    for class in [
        "kitty",
        "alacritty",
        "foot",
        "footclient",
        "org.wezfurlong.wezterm",
        "com.mitchellh.ghostty",
        "konsole",
        "st",
    ] {
        assert!(
            flow::inject::is_terminal_class(class),
            "{class} should be a terminal",
        );
    }
}

#[test]
fn non_terminals_do_not_get_the_terminal_chord() {
    // Substring matching would misfire on any of these; the ends_with-on-dot
    // rule is what keeps them safe.
    for class in [
        "firefox",
        "com.anthropic.claude",
        "org.mozilla.firefox",
        "footnote",
        "kittykat",
        "steam",
    ] {
        assert!(
            !flow::inject::is_terminal_class(class),
            "{class} should not be a terminal",
        );
    }
}

/// Each compositor answers a different shape, and Flow has to read all of
/// them: Hyprland returns the focused window directly under `class`, niri the
/// same under `app_id`, and Sway a whole tree in which one node is `focused`.
/// Getting this wrong is silent - the paste falls back to the configured
/// chord and lands wrong in a terminal.
#[test]
fn the_focused_class_is_read_from_every_compositor_shape() {
    let hyprland = r#"{"class":"kitty","title":"nvim","focusHistoryID":0}"#;
    assert_eq!(
        flow::inject::parse_focused_class(hyprland).as_deref(),
        Some("kitty")
    );

    let niri = r#"{"id":3,"app_id":"Alacritty","title":"zsh"}"#;
    assert_eq!(
        flow::inject::parse_focused_class(niri).as_deref(),
        Some("alacritty")
    );

    let sway = r#"{"type":"root","nodes":[
        {"type":"output","nodes":[
            {"type":"con","app_id":"firefox","focused":false},
            {"type":"con","app_id":"foot","focused":true}
        ]}
    ]}"#;
    assert_eq!(
        flow::inject::parse_focused_class(sway).as_deref(),
        Some("foot")
    );
}

/// No focused window, or output that is not JSON at all, must yield nothing
/// rather than an arbitrary window's class - naming the wrong window is worse
/// than admitting we do not know.
#[test]
fn an_unfocused_or_unreadable_answer_yields_nothing() {
    let nothing_focused =
        r#"{"type":"root","nodes":[{"type":"con","app_id":"firefox","focused":false}]}"#;
    assert_eq!(flow::inject::parse_focused_class(nothing_focused), None);
    assert_eq!(flow::inject::parse_focused_class("not json"), None);
    assert_eq!(flow::inject::parse_focused_class("{}"), None);
    assert_eq!(flow::inject::parse_focused_class(r#"{"class":""}"#), None);
}
