use anyhow::{anyhow, Result};
use evdev::{Device, EventType, KeyCode};
use std::sync::mpsc::{channel, Receiver};
use std::time::{Duration, Instant};

/// Push-to-talk key. Chosen because keyd leaves Ctrl untouched on this machine
/// and a lone modifier emits no character, so passive reads need no device grab.
pub const PTT: KeyCode = KeyCode::KEY_RIGHTCTRL;

/// A press shorter than this is treated as a stray tap and discarded.
const MIN_HOLD: Duration = Duration::from_millis(300);

/// Modifiers that must be physically released before we inject. Injected keys
/// travel through the compositor's keybind layer, so a still-held modifier turns
/// dictated text into shortcuts - on this machine SUPER+m exits the session.
const MODIFIERS: [KeyCode; 8] = [
    KeyCode::KEY_LEFTCTRL,
    KeyCode::KEY_RIGHTCTRL,
    KeyCode::KEY_LEFTALT,
    KeyCode::KEY_RIGHTALT,
    KeyCode::KEY_LEFTMETA,
    KeyCode::KEY_RIGHTMETA,
    KeyCode::KEY_LEFTSHIFT,
    KeyCode::KEY_RIGHTSHIFT,
];

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    Pressed,
    /// Another key arrived while PTT was down, so this was a shortcut, not dictation.
    Cancelled,
    Released { held: Duration },
}

/// Every keyboard-capable device. A device grabbed by a remapper (keyd) delivers
/// nothing, and its virtual device delivers the post-remap events instead, so
/// reading all of them needs no special-casing for the user's input stack.
fn keyboards() -> Vec<(std::path::PathBuf, Device)> {
    evdev::enumerate()
        .filter(|(_, device)| {
            device
                .supported_keys()
                .is_some_and(|keys| keys.contains(PTT))
        })
        .collect()
}

/// Press/release bookkeeping, split out from device I/O so it can be tested
/// without a keyboard.
///
/// One physical press can surface on several devices at once - a remapper like
/// keyd may not grab the original, so both it and its virtual device report the
/// key. State therefore lives here, once, rather than per reader thread.
#[derive(Default)]
pub struct PttState {
    down_at: Option<Instant>,
    cancelled: bool,
}

impl PttState {
    /// Returns the transition this key caused, or `None` if it told us nothing
    /// new (autorepeat, a duplicate from a second device, keys while cancelled).
    pub fn apply(&mut self, key: KeyCode, pressed: bool) -> Option<Event> {
        if key == PTT {
            return match (pressed, self.down_at) {
                (true, None) => {
                    self.down_at = Some(Instant::now());
                    self.cancelled = false;
                    Some(Event::Pressed)
                }
                (false, Some(start)) => {
                    self.down_at = None;
                    (!self.cancelled).then(|| Event::Released {
                        held: start.elapsed(),
                    })
                }
                _ => None,
            };
        }

        // Any other key during the hold means this was a shortcut.
        if pressed && self.down_at.is_some() && !self.cancelled {
            self.cancelled = true;
            return Some(Event::Cancelled);
        }
        None
    }
}

pub struct Listener {
    keys: Receiver<(KeyCode, bool)>,
    state: PttState,
}

impl Listener {
    pub fn new() -> Result<Self> {
        let devices = keyboards();
        if devices.is_empty() {
            return Err(anyhow!(
                "no readable keyboard exposes {PTT:?} - is this user in the 'input' group?"
            ));
        }

        for (path, device) in &devices {
            eprintln!("watching {} ({})", path.display(), device.name().unwrap_or("?"));
        }

        let (tx, keys) = channel();
        for (_, mut device) in devices {
            let tx = tx.clone();
            std::thread::spawn(move || loop {
                let Ok(events) = device.fetch_events() else { return };
                for event in events {
                    if event.event_type() != EventType::KEY {
                        continue;
                    }
                    // 2 is autorepeat, which says nothing new about hold state.
                    let pressed = match event.value() {
                        0 => false,
                        1 => true,
                        _ => continue,
                    };
                    if tx.send((KeyCode(event.code()), pressed)).is_err() {
                        return;
                    }
                }
            });
        }

        Ok(Self {
            keys,
            state: PttState::default(),
        })
    }

    /// Blocks until the key stream produces a meaningful transition.
    pub fn next_event(&mut self) -> Option<Event> {
        while let Ok((key, pressed)) = self.keys.recv() {
            if let Some(event) = self.state.apply(key, pressed) {
                return Some(event);
            }
        }
        None
    }
}

pub fn was_long_enough(held: Duration) -> bool {
    held >= MIN_HOLD
}

/// Block until no modifier is physically held, so injected keystrokes are not
/// reinterpreted as compositor shortcuts. Gives up after `timeout` and reports
/// whether the keyboard actually came to rest.
pub fn wait_for_modifiers_released(timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    let devices = keyboards();

    loop {
        let held = devices.iter().any(|(_, device)| {
            device
                .get_key_state()
                .map(|state| MODIFIERS.iter().any(|m| state.contains(*m)))
                .unwrap_or(false)
        });

        if !held {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
