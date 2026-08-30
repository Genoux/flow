//! Capturing a push-to-talk chord by reading the keyboard directly.
//!
//! # Why not the window's own key events
//!
//! Because they never arrive. A compositor binds chords globally and consumes
//! them before any client sees them - on a stock Hyprland setup nearly every
//! Super combination is already taken - and a Wayland client is not permitted
//! to grab keys. So a settings window that listened for its own key events
//! could only ever capture the chords nobody wants, and would sit there
//! looking broken for exactly the chord the user reached for first.
//!
//! The daemon already reads `/dev/input` directly, below the compositor, which
//! is how push-to-talk works at all. This reads the same way, for as long as
//! the user is choosing a chord and no longer.

use evdev::{Device, EventSummary, KeyCode};
use std::collections::HashSet;
use std::path::PathBuf;

/// Modifier keycodes, and the word the daemon's config uses for each. Both
/// sides of the keyboard map to the same word: a chord does not care which
/// Shift you hold, and neither does the daemon.
const MODIFIERS: [(KeyCode, &str); 8] = [
    (KeyCode::KEY_LEFTMETA, "super"),
    (KeyCode::KEY_RIGHTMETA, "super"),
    (KeyCode::KEY_LEFTCTRL, "ctrl"),
    (KeyCode::KEY_RIGHTCTRL, "ctrl"),
    (KeyCode::KEY_LEFTALT, "alt"),
    (KeyCode::KEY_RIGHTALT, "alt"),
    (KeyCode::KEY_LEFTSHIFT, "shift"),
    (KeyCode::KEY_RIGHTSHIFT, "shift"),
];

/// The order modifiers are written in, so the same chord always spells the
/// same way regardless of the order they were pressed.
const ORDER: [&str; 4] = ["super", "ctrl", "alt", "shift"];

fn modifier_word(key: KeyCode) -> Option<&'static str> {
    MODIFIERS
        .iter()
        .find(|(code, _)| *code == key)
        .map(|(_, word)| *word)
}

/// The keyboards worth reading, which is not the same as all of them.
///
/// A remapper like keyd grabs the physical keyboard and re-emits its own,
/// remapped events on a virtual device. Reading both means seeing every press
/// twice - once as the key that was physically struck, once as whatever it was
/// remapped to - and a chord assembled from that mixture is a chord nobody
/// pressed. Reading Ctrl+Alt+F9 on this machine produced "super+ctrl+f9".
///
/// So when a remapper is present its device is the only truth, exactly as the
/// daemon decides in `discover_chord_paths`: what it emits is what the rest of
/// the system - including the daemon that will have to match this chord - sees.
///
/// # Why the kernel's own listing and not the devices
///
/// Because opening a device to ask what it is costs a second and a half of the
/// window's startup. Closing an evdev file descriptor makes the kernel wait out
/// an RCU grace period - 25 to 100ms per device, measured, and there are 25
/// nodes under `/dev/input` on this machine for four keyboards, the rest being
/// power buttons, HDMI jack sensors and an RGB controller. `available` is read
/// in `Console::new`, so that 1.4s was paid before the first frame every single
/// time the window opened.
///
/// `/proc/bus/input/devices` carries the three things this needs - the name, the
/// event node, and the key bitmap - for every device in one read, with no
/// descriptor opened on any of them.
fn keyboard_paths() -> Vec<PathBuf> {
    let Ok(listing) = std::fs::read_to_string("/proc/bus/input/devices") else {
        return Vec::new();
    };

    let mut remapped = Vec::new();
    let mut physical = Vec::new();
    for (name, path) in listed_keyboards(&listing) {
        // ydotool's own device is excluded outright: it exists to inject
        // synthetic keys, and capturing those would let anything scripting the
        // desktop rebind the chord out from under the user.
        if name.contains("ydotool") {
            continue;
        }
        if name.contains("keyd") {
            remapped.push(path);
        } else {
            physical.push(path);
        }
    }

    if remapped.is_empty() {
        physical
    } else {
        remapped
    }
}

/// Every keyboard in a `/proc/bus/input/devices` listing, as its lowercased
/// name and the event node to read it from.
///
/// Devices are separated by a blank line, and a device with no `eventN` handler
/// cannot be read at all - a keyboard behind a driver that only exposes `kbd`
/// is not one this window can offer.
fn listed_keyboards(listing: &str) -> Vec<(String, PathBuf)> {
    listing
        .split("\n\n")
        .filter_map(|block| {
            let mut name = None;
            let mut node = None;
            let mut letters = false;
            for line in block.lines() {
                if let Some(value) = line.strip_prefix("N: Name=") {
                    name = Some(value.trim().trim_matches('"').to_ascii_lowercase());
                } else if let Some(value) = line.strip_prefix("H: Handlers=") {
                    node = value
                        .split_whitespace()
                        .find(|handler| handler.starts_with("event"))
                        .map(|handler| PathBuf::from("/dev/input").join(handler));
                } else if let Some(value) = line.strip_prefix("B: KEY=") {
                    letters = types_letters(value);
                }
            }
            letters.then(|| Some((name?, node?))).flatten()
        })
        .collect()
}

/// Whether a `B: KEY=` bitmap has a letter key in it.
///
/// Probed with a letter rather than a specific key, for the reason the daemon
/// probes the same way: any key can be the trigger, and requiring one in
/// particular skips real keyboards. What it rules out is everything else the
/// kernel files under key events - both power buttons, the video bus, and the
/// nine HDMI jack sensors.
///
/// The bitmap is written as hex words, most significant first, so `KEY_A` at 30
/// is in the last word and no other word has to be read.
fn types_letters(bitmap: &str) -> bool {
    bitmap
        .split_whitespace()
        .next_back()
        .and_then(|word| u64::from_str_radix(word, 16).ok())
        .is_some_and(|word| word & (1 << KeyCode::KEY_A.code()) != 0)
}

/// The same keyboards, opened for reading. Only the two or three that are
/// really keyboards pay for a descriptor, which is what keeps the cost above
/// off the window's startup.
fn keyboards() -> Vec<Device> {
    keyboard_paths()
        .iter()
        .filter_map(|path| Device::open(path).ok())
        .collect()
}

/// The name the daemon's config uses for a trigger key, or `None` for keys it
/// has no spelling for. Kept deliberately narrow: writing a chord the daemon
/// cannot parse would save fine and then fail at its next startup.
fn trigger_word(key: KeyCode) -> Option<String> {
    let name = format!("{key:?}");
    let bare = name.strip_prefix("KEY_")?.to_lowercase();

    let usable = bare.len() == 1 && bare.chars().all(|c| c.is_ascii_alphanumeric())
        || matches!(bare.as_str(), "space" | "enter" | "tab")
        || (bare.starts_with('f')
            && bare.len() <= 3
            && bare[1..].chars().all(|c| c.is_ascii_digit())
            && !bare[1..].is_empty());

    usable.then_some(bare)
}

/// Watch every keyboard until a chord is pressed, then return its spelling.
///
/// A chord is at least one modifier held plus a normal key. Modifiers alone
/// are ignored rather than accepted, because a binding whose trigger is a
/// modifier can never complete - the key that fires it is the key holding it.
///
/// `cancel` is checked between reads so closing the window or pressing Cancel
/// stops this promptly rather than leaving a thread on the keyboard.
pub fn capture(cancel: &dyn Fn() -> bool) -> Option<String> {
    let mut devices = keyboards();
    if devices.is_empty() {
        return None;
    }
    for device in &mut devices {
        // Non-blocking so no single quiet keyboard can hold up the others, and
        // so cancelling is noticed straight away.
        let _ = device.set_nonblocking(true);
    }

    let mut held: HashSet<KeyCode> = HashSet::new();

    loop {
        if cancel() {
            return None;
        }

        for device in &mut devices {
            let Ok(events) = device.fetch_events() else {
                continue; // nothing pending on this one
            };
            for event in events {
                let EventSummary::Key(_, key, value) = event.destructure() else {
                    continue;
                };
                // 1 is press, 2 is autorepeat, 0 is release.
                if value == 0 {
                    held.remove(&key);
                    continue;
                }
                if modifier_word(key).is_some() {
                    held.insert(key);
                    continue;
                }

                let mut words: Vec<&str> = held.iter().filter_map(|k| modifier_word(*k)).collect();
                words.sort_by_key(|word| ORDER.iter().position(|o| o == word).unwrap_or(9));
                words.dedup();
                if words.is_empty() {
                    continue; // a bare key is not a chord to hold
                }

                let trigger = trigger_word(key)?;
                return Some(format!("{}+{trigger}", words.join("+")));
            }
        }

        std::thread::sleep(std::time::Duration::from_millis(15));
    }
}

/// Whether any keyboard can be read at all. Used to hide the control rather
/// than offer one that cannot work: reading `/dev/input` needs group access
/// the user may not have.
///
/// One device is opened and no more - `any` stops at the first that answers -
/// because the listing says which keyboards exist and only a descriptor says
/// whether this user may have one.
pub fn available() -> bool {
    keyboard_paths()
        .iter()
        .any(|path| std::fs::File::open(path).is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The listing this window reads, cut down to one keyboard and the two
    /// kinds of device that also report key events and are not keyboards.
    const LISTING: &str = "\
I: Bus=0019 Vendor=0000 Product=0001 Version=0000
N: Name=\"Power Button\"
H: Handlers=kbd event0
B: EV=3
B: KEY=8000 10000000000000 0

I: Bus=0003 Vendor=25a7 Product=fa7c Version=0111
N: Name=\"Compx Pulsar Xlite Wireless Keyboard\"
H: Handlers=sysrq kbd event3
B: EV=10001f
B: KEY=733eff 0 0 483ffff17aff32d bfd4444600000000 1 130c730b17c007 ffbf7bfad941dfff febeffdfffefffff fffffffffffffffe

I: Bus=0000 Vendor=0000 Product=0000 Version=0000
N: Name=\"HDA NVidia HDMI/DP,pcm=3\"
H: Handlers=event13
B: EV=21
B: SW=140
";

    /// The device probe the window's startup used to pay 1.4s for, now answered
    /// from one file read: one keyboard out of three devices that all report
    /// key events.
    #[test]
    fn only_real_keyboards_are_read_from_the_listing() {
        let found = listed_keyboards(LISTING);
        assert_eq!(
            found,
            vec![(
                "compx pulsar xlite wireless keyboard".to_string(),
                PathBuf::from("/dev/input/event3")
            )]
        );
    }

    /// A power button reports key events and is not a keyboard, which is the
    /// whole reason the bitmap is read rather than the `EV=` line.
    #[test]
    fn a_device_with_keys_but_no_letters_is_not_a_keyboard() {
        assert!(types_letters(
            "733eff 0 0 483ffff17aff32d bfd4444600000000 1 130c730b17c007 ffbf7bfad941dfff febeffdfffefffff fffffffffffffffe"
        ));
        assert!(!types_letters("8000 10000000000000 0"));
        // keyd's virtual keyboard, which is the device this window prefers when
        // a remapper is present.
        assert!(types_letters(
            "10000000000000 0 ffffffffffffff0f ffffffffffffffff ffffffffffffffff fffffffffffffffe"
        ));
    }

    #[test]
    fn only_keys_the_daemon_can_parse_are_offered() {
        assert_eq!(trigger_word(KeyCode::KEY_D).as_deref(), Some("d"));
        assert_eq!(trigger_word(KeyCode::KEY_5).as_deref(), Some("5"));
        assert_eq!(trigger_word(KeyCode::KEY_SPACE).as_deref(), Some("space"));
        assert_eq!(trigger_word(KeyCode::KEY_F9).as_deref(), Some("f9"));
        // No spelling in the daemon's parser, so not offered at all.
        assert_eq!(trigger_word(KeyCode::KEY_LEFTBRACE), None);
        assert_eq!(trigger_word(KeyCode::KEY_KPPLUS), None);
    }

    /// End to end over a real uinput keyboard. Every interesting failure in
    /// `capture` lives in the device layer, which the spelling tests above
    /// cannot see. Ignored by default: it needs `/dev/uinput` and a readable
    /// `/dev/input`.
    #[test]
    #[ignore = "needs /dev/uinput and membership of the input group"]
    fn a_synthetic_chord_is_captured() {
        use evdev::uinput::VirtualDevice;
        use evdev::{AttributeSet, KeyEvent};
        use std::time::{Duration, Instant};

        let mut keys = AttributeSet::<KeyCode>::new();
        for key in [
            KeyCode::KEY_A,
            KeyCode::KEY_LEFTCTRL,
            KeyCode::KEY_LEFTSHIFT,
            KeyCode::KEY_F13,
        ] {
            keys.insert(key);
        }
        // Named for keyd on purpose: where a remapper is present `keyboards`
        // reads only its devices, so a test keyboard without the word would be
        // dropped as a physical duplicate and this would test nothing.
        let mut device = VirtualDevice::builder()
            .expect("open /dev/uinput")
            .name("keyd test chord")
            .with_keys(&keys)
            .expect("declare keys")
            .build()
            .expect("build");
        std::thread::sleep(Duration::from_millis(700));

        // Bounded, or a capture that never sees the chord hangs the suite.
        let deadline = Instant::now() + Duration::from_secs(5);
        let captured = std::thread::spawn(move || capture(&|| Instant::now() > deadline));
        std::thread::sleep(Duration::from_millis(300));

        for (key, value) in [
            (KeyCode::KEY_LEFTCTRL, 1),
            (KeyCode::KEY_LEFTSHIFT, 1),
            (KeyCode::KEY_F13, 1),
            (KeyCode::KEY_F13, 0),
            (KeyCode::KEY_LEFTSHIFT, 0),
            (KeyCode::KEY_LEFTCTRL, 0),
        ] {
            device.emit(&[*KeyEvent::new(key, value)]).expect("emit");
            std::thread::sleep(Duration::from_millis(30));
        }

        assert_eq!(
            captured.join().expect("capture thread").as_deref(),
            Some("ctrl+shift+f13")
        );
    }

    #[test]
    fn both_sides_of_a_modifier_mean_the_same_word() {
        assert_eq!(modifier_word(KeyCode::KEY_LEFTMETA), Some("super"));
        assert_eq!(modifier_word(KeyCode::KEY_RIGHTMETA), Some("super"));
        assert_eq!(modifier_word(KeyCode::KEY_RIGHTSHIFT), Some("shift"));
        assert_eq!(modifier_word(KeyCode::KEY_D), None);
    }
}
