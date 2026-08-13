use anyhow::{anyhow, Result};
use evdev::{Device, EventType, KeyCode};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Sender};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Push-to-talk key. Chosen because keyd leaves Ctrl untouched on this machine
/// and a lone modifier emits no character, so passive reads need no device grab.
pub const PTT: KeyCode = KeyCode::KEY_RIGHTCTRL;

/// A press shorter than this on the dedicated PTT key is treated as a stray
/// tap and discarded. Compositor start/stop is always intentional.
const MIN_HOLD: Duration = Duration::from_millis(300);

/// If the chord is not visible by then, the tap already ended and we stop.
const CHORD_APPEAR: Duration = Duration::from_millis(40);

/// Poll interval while waiting for the compositor chord to break.
const CHORD_POLL: Duration = Duration::from_millis(4);

/// Keys that make up the Hyprland dictation chord (SUPER+SHIFT+d). Physical
/// Alt is remapped to Super by keyd, so we see LEFTMETA on the virtual device.
const CHORD_KEYS: [KeyCode; 5] = [
    KeyCode::KEY_D,
    KeyCode::KEY_LEFTMETA,
    KeyCode::KEY_RIGHTMETA,
    KeyCode::KEY_LEFTSHIFT,
    KeyCode::KEY_RIGHTSHIFT,
];

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
    /// From `flow start` / `flow stop`. Start arms a chord watcher so hold works
    /// even when the compositor's release bind never fires.
    Start,
    Stop,
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

/// Spawn a reader per keyboard, feeding push-to-talk transitions into `events`.
/// Shares the channel with the signal handler so the daemon has one input stream.
pub fn spawn(events: Sender<Event>) -> Result<()> {
    let devices = keyboards();
    if devices.is_empty() {
        return Err(anyhow!(
            "no readable keyboard exposes {PTT:?} - is this user in the 'input' group?"
        ));
    }

    for (path, device) in &devices {
        eprintln!("watching {} ({})", path.display(), device.name().unwrap_or("?"));
    }

    let (raw_tx, raw_rx) = channel();
    for (_, mut device) in devices {
        let raw_tx = raw_tx.clone();
        std::thread::spawn(move || loop {
            let Ok(batch) = device.fetch_events() else { return };
            for event in batch {
                if event.event_type() != EventType::KEY {
                    continue;
                }
                // 2 is autorepeat, which says nothing new about hold state.
                let pressed = match event.value() {
                    0 => false,
                    1 => true,
                    _ => continue,
                };
                if raw_tx.send((KeyCode(event.code()), pressed)).is_err() {
                    return;
                }
            }
        });
    }

    std::thread::spawn(move || {
        let mut state = PttState::default();
        while let Ok((key, pressed)) = raw_rx.recv() {
            if let Some(event) = state.apply(key, pressed)
                && events.send(event).is_err() {
                    return;
                }
        }
    });

    Ok(())
}

pub fn was_long_enough(held: Duration) -> bool {
    held >= MIN_HOLD
}

/// True when a previously observed chord has actually broken.
/// An empty `now` is unknown (fresh fd, transient ioctl miss), not a release.
pub fn chord_broken(chord: &HashSet<KeyCode>, now: &HashSet<KeyCode>) -> bool {
    !chord.is_empty() && !now.is_empty() && chord.iter().any(|key| !now.contains(key))
}

/// True when every key we saw at press is up. Empty `now` after we have seen
/// keys down means the chord is gone; empty `now` before that is unknown.
pub fn chord_released(chord: &HashSet<KeyCode>, now: &HashSet<KeyCode>) -> bool {
    !chord.is_empty() && chord.iter().all(|key| !now.contains(key))
}

/// Prefer keyd's virtual keyboard: it is what Hyprland sees after remap, and
/// the physical device can report stale pressed keys while keyd has it grabbed.
fn chord_paths() -> &'static [PathBuf] {
    static PATHS: OnceLock<Vec<PathBuf>> = OnceLock::new();
    PATHS.get_or_init(discover_chord_paths)
}

/// Call once at daemon start so the first tap does not pay device discovery.
pub fn warmup_chord_devices() {
    let paths = chord_paths();
    if paths.is_empty() {
        eprintln!("chord watch: no keyboard with KEY_D");
    } else {
        for path in paths {
            eprintln!("chord watch: {}", path.display());
        }
    }
}

fn discover_chord_paths() -> Vec<PathBuf> {
    let from_proc = keyd_paths_from_proc();
    if !from_proc.is_empty() {
        return from_proc;
    }

    let mut keyd = Vec::new();
    let mut others = Vec::new();
    for (path, device) in evdev::enumerate() {
        if !device
            .supported_keys()
            .is_some_and(|keys| keys.contains(KeyCode::KEY_D))
        {
            continue;
        }
        if device
            .name()
            .is_some_and(|name| name.to_ascii_lowercase().contains("keyd"))
        {
            keyd.push(path);
        } else {
            others.push(path);
        }
    }
    if keyd.is_empty() { others } else { keyd }
}

/// `/proc/bus/input/devices` names keyd without opening every evdev node.
fn keyd_paths_from_proc() -> Vec<PathBuf> {
    let Ok(text) = std::fs::read_to_string("/proc/bus/input/devices") else {
        return Vec::new();
    };
    text.split("\n\n")
        .filter(|block| {
            block
                .lines()
                .any(|line| {
                    let lower = line.to_ascii_lowercase();
                    lower.contains("name=\"keyd") && lower.contains("keyboard")
                })
        })
        .filter_map(|block| {
            block.lines().find_map(|line| {
                let handlers = line.strip_prefix("H: Handlers=")?;
                handlers.split_whitespace().find_map(|token| {
                    token
                        .strip_prefix("event")
                        .and_then(|n| n.parse::<u32>().ok())
                        .map(|n| PathBuf::from(format!("/dev/input/event{n}")))
                })
            })
        })
        .filter(|path| Path::new(path).exists())
        .collect()
}

fn chord_devices() -> Vec<Device> {
    chord_paths()
        .iter()
        .filter_map(|path| Device::open(path).ok())
        .collect()
}

fn chord_snapshot(devices: &[Device]) -> HashSet<KeyCode> {
    let mut keys = HashSet::new();
    for device in devices {
        if let Ok(state) = device.get_key_state() {
            for key in CHORD_KEYS {
                if state.contains(key) {
                    keys.insert(key);
                }
            }
        }
    }
    keys
}

/// Cancels a running [`watch_chord_release`] when the hold ends another way
/// (explicit `flow stop`, a replacement start, process exit).
pub struct ChordWatch {
    cancel: Arc<AtomicBool>,
}

impl ChordWatch {
    /// Snapshot the dictation chord at `flow start` and emit [`Event::Stop`]
    /// when any of those keys is released. Hyprland's release binds miss
    /// modifier chords; this is what makes compositor-driven hold reliable.
    pub fn arm(events: Sender<Event>) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::spawn(move || watch_chord_release(events, flag));
        Self { cancel }
    }

    pub fn disarm(self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn watch_chord_release(events: Sender<Event>, cancel: Arc<AtomicBool>) {
    // Two fds per device: fetch_events blocks one, get_key_state needs the other.
    let state_devices = chord_devices();
    let event_devices = chord_devices();
    if state_devices.is_empty() {
        eprintln!("chord watch: no readable keyboard - bind `flow stop` on release as fallback");
        return;
    }

    if cancel.load(Ordering::Relaxed) {
        return;
    }

    let appear_by = Instant::now() + CHORD_APPEAR;
    let mut chord = chord_snapshot(&state_devices);
    while chord.is_empty() && Instant::now() < appear_by && !cancel.load(Ordering::Relaxed) {
        std::thread::sleep(CHORD_POLL);
        chord = chord_snapshot(&state_devices);
    }

    // Quick tap: keys are already up by the time the start signal arrives.
    // Waiting for a key-up that already happened is what stranded the mic.
    if chord.is_empty() {
        eprintln!("chord watch: keys already up - stopping");
        let _ = events.send(Event::Stop);
        return;
    }

    eprintln!("chord watch: holding until release ({chord:?})");

    let (release_tx, release_rx) = channel();
    for mut device in event_devices {
        let release_tx = release_tx.clone();
        std::thread::spawn(move || {
            loop {
                let Ok(batch) = device.fetch_events() else { return };
                for event in batch {
                    if event.event_type() != EventType::KEY || event.value() != 0 {
                        continue;
                    }
                    let key = KeyCode(event.code());
                    if CHORD_KEYS.contains(&key) {
                        let _ = release_tx.send(key);
                    }
                }
            }
        });
    }
    drop(release_tx);

    let mut gone_polls = 0u8;
    while !cancel.load(Ordering::Relaxed) {
        let now = chord_snapshot(&state_devices);
        if chord_released(&chord, &now) {
            gone_polls += 1;
            if gone_polls >= 2 {
                eprintln!("chord watch: released (state)");
                let _ = events.send(Event::Stop);
                return;
            }
        } else {
            gone_polls = 0;
        }

        match release_rx.recv_timeout(CHORD_POLL) {
            Ok(key) => {
                let now = chord_snapshot(&state_devices);
                if chord_released(&chord, &now) {
                    eprintln!("chord watch: released (event {key:?} + state)");
                    let _ = events.send(Event::Stop);
                    return;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {}
        }
    }
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
