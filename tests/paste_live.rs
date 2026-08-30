//! Does a paste actually land in the focused window?
//!
//! `keymap_live` proves the compositor accepts the keymap, and the unit tests
//! prove the chord is split correctly, but neither proves the compositor maps
//! the keycodes Flow sends back to `v`. That needs a window that receives the
//! chord and can be read afterwards.
//!
//! So the test spawns one: kitty running `head -c` into a file, with the
//! terminal in raw mode so each byte arrives without waiting for a newline.
//! Flow's own `Injector` stages the sentinel on the clipboard and sends the
//! chord exactly as the daemon does - including the class lookup that decides
//! kitty needs Ctrl+Shift+V - and the file says whether it arrived.
//!
//! Takes focus for a second and briefly replaces the clipboard, which
//! `inject` puts back. Needs a Wayland session, kitty, and /dev/input:
//!   cargo test --release --test paste_live -- --ignored --nocapture

use flow::inject::Injector;
use std::process::{Child, Command};
use std::time::{Duration, Instant};

/// No trailing newline: raw mode means the bytes arrive as they are, and
/// `head -c` counts them, so nothing has to press Enter.
const SENTINEL: &str = "flow-paste-ok";

#[test]
#[ignore]
fn the_chord_pastes_into_the_focused_window() {
    let target = std::env::temp_dir().join("flow-paste-live");
    let _ = std::fs::remove_file(&target);

    // Left as the default class on purpose: `detect_terminal_focus` looks it up
    // to pick Ctrl+Shift+V, so overriding it would test the wrong chord.
    let mut kitty = Reaped(
        Command::new("kitty")
            .arg("--title")
            .arg("flow-paste-live")
            .arg("sh")
            .arg("-c")
            .arg(format!(
                "stty raw; head -c {} > {}",
                SENTINEL.len(),
                target.display()
            ))
            .spawn()
            .expect("kitty is needed to receive the paste"),
    );

    // Waiting on the title, not just "a terminal is focused": the user's own
    // kitty would satisfy that and the sentinel would land in their window.
    let focused = wait_until(Duration::from_secs(5), || {
        active_window_title().is_some_and(|title| title.contains("flow-paste-live"))
    });
    assert!(
        focused,
        "the spawned kitty never took focus, so the chord would have gone to \
         someone else's window"
    );
    assert_eq!(
        flow::inject::detect_terminal_focus(),
        Some(true),
        "kitty was not recognised as a terminal, so Flow would send Ctrl+V, \
         which kitty does not treat as paste"
    );

    let mut injector = Injector::new().expect("building the injector");
    // A uinput fallback would pass this test while proving nothing about the
    // protocol path, which is the whole point of the module.
    assert_eq!(
        injector.backend(),
        "wayland",
        "fell back to uinput, so the virtual-keyboard path was never exercised"
    );

    injector.inject(SENTINEL).expect("injecting");

    let landed = wait_until(Duration::from_secs(5), || {
        std::fs::read(&target).is_ok_and(|bytes| bytes.len() >= SENTINEL.len())
    });
    let received = std::fs::read_to_string(&target).unwrap_or_default();
    kitty.0.kill().ok();

    assert!(landed, "nothing arrived; the window received {received:?}");
    assert_eq!(received, SENTINEL, "the wrong bytes arrived");
    eprintln!("paste landed: {received:?}");
}

fn active_window_title() -> Option<String> {
    let output = Command::new("hyprctl")
        .args(["-j", "activewindow"])
        .output()
        .ok()?;
    let json = String::from_utf8(output.stdout).ok()?;
    // Reading it with a string search rather than a JSON dependency: the test
    // only needs to know whether its own title is the focused one.
    let start = json.find("\"title\": \"")? + "\"title\": \"".len();
    let rest = &json[start..];
    Some(rest[..rest.find('"')?].to_string())
}

fn wait_until(budget: Duration, mut ready: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + budget;
    while Instant::now() < deadline {
        if ready() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    false
}

/// A panic before the kill would otherwise leave kitty holding focus.
struct Reaped(Child);

impl Drop for Reaped {
    fn drop(&mut self) {
        self.0.kill().ok();
        self.0.wait().ok();
    }
}

/// The question the whole "release every finger" complaint turns on: with the
/// chord's own modifiers still physically down, does the paste land?
///
/// `MODIFIER_WAIT` exists because the answer was measured as no on an earlier
/// Hyprland. This holds super+shift on a synthetic keyboard for the duration of
/// the paste and reads the window back, so the answer is a fact rather than a
/// comment.
///
///   cargo test --release --test paste_live -- --ignored --nocapture held
#[test]
#[ignore]
fn a_paste_lands_with_the_chord_modifiers_held() {
    use evdev::uinput::VirtualDevice;
    use evdev::{AttributeSet, KeyCode};

    let target = std::env::temp_dir().join("flow-paste-held");
    let _ = std::fs::remove_file(&target);

    let mut keys = AttributeSet::<KeyCode>::new();
    for key in [KeyCode::KEY_LEFTMETA, KeyCode::KEY_LEFTSHIFT] {
        keys.insert(key);
    }
    let device = VirtualDevice::builder()
        .expect("open /dev/uinput")
        .name("flow held-modifier probe")
        .with_keys(&keys)
        .expect("declare keys")
        .build()
        .expect("build");
    // Releases on the way out however the test ends, so a panic cannot leave
    // Super stuck down on the user's session.
    let mut held = HeldKeys(device);
    std::thread::sleep(Duration::from_millis(500));

    let mut kitty = Reaped(
        Command::new("kitty")
            .arg("--title")
            .arg("flow-paste-held")
            .arg("sh")
            .arg("-c")
            .arg(format!(
                "stty raw; head -c {} > {}",
                SENTINEL.len(),
                target.display()
            ))
            .spawn()
            .expect("kitty is needed to receive the paste"),
    );

    assert!(
        wait_until(Duration::from_secs(5), || active_window_title()
            .is_some_and(|title| title.contains("flow-paste-held"))),
        "the spawned kitty never took focus"
    );

    let mut injector = Injector::new().expect("building the injector");
    held.press();
    std::thread::sleep(Duration::from_millis(100));

    injector.inject(SENTINEL).expect("injecting");

    let landed = wait_until(Duration::from_secs(3), || {
        std::fs::read(&target).is_ok_and(|bytes| bytes.len() >= SENTINEL.len())
    });
    held.release();
    let received = std::fs::read_to_string(&target).unwrap_or_default();
    kitty.0.kill().ok();

    eprintln!("=== with super+shift held, the window received {received:?}");
    assert!(
        landed && received == SENTINEL,
        "paste did not land with modifiers held; window received {received:?}"
    );
}

struct HeldKeys(evdev::uinput::VirtualDevice);

impl HeldKeys {
    fn press(&mut self) {
        self.emit(1);
    }

    fn release(&mut self) {
        self.emit(0);
    }

    fn emit(&mut self, value: i32) {
        use evdev::{KeyCode, KeyEvent};
        for key in [KeyCode::KEY_LEFTMETA, KeyCode::KEY_LEFTSHIFT] {
            let _ = self.0.emit(&[*KeyEvent::new(key, value)]);
        }
    }
}

impl Drop for HeldKeys {
    fn drop(&mut self) {
        self.release();
    }
}
