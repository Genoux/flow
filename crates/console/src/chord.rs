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
fn keyboards() -> Vec<Device> {
    let Ok(entries) = std::fs::read_dir("/dev/input") else {
        return Vec::new();
    };

    let mut remapped = Vec::new();
    let mut physical = Vec::new();

    for path in entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("event"))
        })
        .collect::<Vec<PathBuf>>()
    {
        let Ok(device) = Device::open(&path) else {
            continue;
        };
        // Probed with a letter rather than a specific key: any key can be the
        // trigger, and requiring one in particular skips real keyboards.
        if !device
            .supported_keys()
            .is_some_and(|keys| keys.contains(KeyCode::KEY_A))
        {
            continue;
        }
        let name = device.name().unwrap_or_default().to_ascii_lowercase();
        // ydotool's own device is excluded outright: it exists to inject
        // synthetic keys, and capturing those would let anything scripting the
        // desktop rebind the chord out from under the user.
        if name.contains("ydotool") {
            continue;
        }
        if name.contains("keyd") {
            remapped.push(device);
        } else {
            physical.push(device);
        }
    }

    if remapped.is_empty() { physical } else { remapped }
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
pub fn available() -> bool {
    !keyboards().is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn both_sides_of_a_modifier_mean_the_same_word() {
        assert_eq!(modifier_word(KeyCode::KEY_LEFTMETA), Some("super"));
        assert_eq!(modifier_word(KeyCode::KEY_RIGHTMETA), Some("super"));
        assert_eq!(modifier_word(KeyCode::KEY_RIGHTSHIFT), Some("shift"));
        assert_eq!(modifier_word(KeyCode::KEY_D), None);
    }
}
