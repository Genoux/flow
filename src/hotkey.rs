use anyhow::{Result, anyhow};
use evdev::{Device, EventType, KeyCode};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Sender, channel};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

/// A hold shorter than this is a tap, not a dictation, and its audio is thrown
/// away. Applies to every binding: exempting deliberate chords meant a frustrated
/// double-tap injected whatever the recogniser made of 200ms of pre-roll, which
/// is where a stream of "Yeah." and "Mm." came from.
///
/// Measured from real use, where held time is the recorded length minus PRE_ROLL:
/// stray taps ran 0 to 400ms held, real dictations started at 2.2s. 500ms sat
/// too high - a fast, deliberate "yes" or "okay" landed in the gap and vanished
/// silently, which is what "released too quickly, nothing pasted" turned out to
/// be. 200ms sits at the top of the stray-tap range but still under any real
/// utterance the recogniser can make sense of.
const MIN_HOLD: Duration = Duration::from_millis(200);

/// If the chord is not visible by then, the tap already ended and we stop.
const CHORD_APPEAR: Duration = Duration::from_millis(40);

/// Poll interval while waiting for the compositor chord to break.
const CHORD_POLL: Duration = Duration::from_millis(4);

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

/// One modifier position, satisfied by the key on either side of the board.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Modifier {
    Super,
    Shift,
    Ctrl,
    Alt,
}

impl Modifier {
    fn keys(self) -> [KeyCode; 2] {
        match self {
            Self::Super => [KeyCode::KEY_LEFTMETA, KeyCode::KEY_RIGHTMETA],
            Self::Shift => [KeyCode::KEY_LEFTSHIFT, KeyCode::KEY_RIGHTSHIFT],
            Self::Ctrl => [KeyCode::KEY_LEFTCTRL, KeyCode::KEY_RIGHTCTRL],
            Self::Alt => [KeyCode::KEY_LEFTALT, KeyCode::KEY_RIGHTALT],
        }
    }

    fn parse(name: &str) -> Option<Self> {
        match name {
            // "meta" and "win" are the same physical key as "super"; people
            // reach for whichever word their desktop taught them.
            "super" | "meta" | "win" | "cmd" | "command" => Some(Self::Super),
            "shift" => Some(Self::Shift),
            "ctrl" | "control" => Some(Self::Ctrl),
            "alt" | "option" => Some(Self::Alt),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Super => "super",
            Self::Shift => "shift",
            Self::Ctrl => "ctrl",
            Self::Alt => "alt",
        }
    }
}

/// The push-to-talk binding: a trigger key, plus modifiers that must already be
/// held when it arrives.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chord {
    pub trigger: KeyCode,
    pub modifiers: Vec<Modifier>,
}

impl Default for Chord {
    /// Super+Shift+D - the combination people already bind in their compositor,
    /// and reachable on every keyboard. Super is one evdev code regardless of
    /// what the keycap says, and unlike Right Ctrl it exists on boards that drop
    /// the right-hand modifiers entirely.
    ///
    /// Super+D alone is "show desktop" on most desktops; adding Shift steps
    /// around that.
    fn default() -> Self {
        Self {
            trigger: KeyCode::KEY_D,
            modifiers: vec![Modifier::Super, Modifier::Shift],
        }
    }
}

impl Chord {
    /// A lone key, like the Right Ctrl this used to hardcode.
    pub fn bare(trigger: KeyCode) -> Self {
        Self {
            trigger,
            modifiers: Vec::new(),
        }
    }

    /// Modifiers make a press unambiguous, so it needs no minimum hold.
    pub fn deliberate(&self) -> bool {
        !self.modifiers.is_empty()
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut parts = text
            .split('+')
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .peekable();
        if parts.peek().is_none() {
            return Err(anyhow!("empty binding"));
        }

        let lowered: Vec<String> = parts.map(|p| p.to_lowercase()).collect();
        let (last, leading) = lowered.split_last().expect("checked non-empty");

        let mut modifiers = Vec::new();
        for name in leading {
            let modifier = Modifier::parse(name)
                .ok_or_else(|| anyhow!("{name:?} is not a modifier (super, shift, ctrl, alt)"))?;
            if modifiers.contains(&modifier) {
                return Err(anyhow!("{name:?} is listed twice"));
            }
            modifiers.push(modifier);
        }

        // A trailing modifier name would mean the chord can never complete: the
        // key that triggers it would also be the one holding it.
        if Modifier::parse(last).is_some() && !modifiers.is_empty() {
            return Err(anyhow!(
                "{last:?} is a modifier, so there is no key to press"
            ));
        }

        Ok(Self {
            trigger: trigger_key(last)?,
            modifiers,
        })
    }

    fn satisfied(&self, held: &HashSet<KeyCode>) -> bool {
        self.modifiers
            .iter()
            .all(|modifier| modifier.keys().iter().any(|key| held.contains(key)))
    }

    /// Every key of the chord is physically down, in any order.
    fn fully_held(&self, held: &HashSet<KeyCode>) -> bool {
        held.contains(&self.trigger) && self.satisfied(held)
    }

    /// Every key this chord could involve, both sides of each modifier. What the
    /// release watcher has to look at - it used to be a hardcoded list of
    /// super/shift/d, so any other configured hotkey was invisible to it and the
    /// recording died 40ms after it started.
    pub fn keys(&self) -> HashSet<KeyCode> {
        let mut keys = HashSet::from([self.trigger]);
        for modifier in &self.modifiers {
            keys.extend(modifier.keys());
        }
        keys
    }

    fn contains(&self, key: KeyCode) -> bool {
        key == self.trigger || self.modifiers.iter().any(|m| m.keys().contains(&key))
    }
}

impl std::fmt::Display for Chord {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for modifier in &self.modifiers {
            write!(f, "{}+", modifier.name())?;
        }
        f.write_str(&trigger_name(self.trigger))
    }
}

/// Alphabetical order, which evdev is not: the codes follow the QWERTY rows, so
/// KEY_A is 30 and KEY_D is 32. Arithmetic on KEY_A silently yields the wrong
/// letter, so both directions go through this one table.
const LETTERS: [KeyCode; 26] = [
    KeyCode::KEY_A,
    KeyCode::KEY_B,
    KeyCode::KEY_C,
    KeyCode::KEY_D,
    KeyCode::KEY_E,
    KeyCode::KEY_F,
    KeyCode::KEY_G,
    KeyCode::KEY_H,
    KeyCode::KEY_I,
    KeyCode::KEY_J,
    KeyCode::KEY_K,
    KeyCode::KEY_L,
    KeyCode::KEY_M,
    KeyCode::KEY_N,
    KeyCode::KEY_O,
    KeyCode::KEY_P,
    KeyCode::KEY_Q,
    KeyCode::KEY_R,
    KeyCode::KEY_S,
    KeyCode::KEY_T,
    KeyCode::KEY_U,
    KeyCode::KEY_V,
    KeyCode::KEY_W,
    KeyCode::KEY_X,
    KeyCode::KEY_Y,
    KeyCode::KEY_Z,
];

/// F11 and F12 sit apart from F1-F10, so this is a table too.
const FUNCTION_KEYS: [KeyCode; 12] = [
    KeyCode::KEY_F1,
    KeyCode::KEY_F2,
    KeyCode::KEY_F3,
    KeyCode::KEY_F4,
    KeyCode::KEY_F5,
    KeyCode::KEY_F6,
    KeyCode::KEY_F7,
    KeyCode::KEY_F8,
    KeyCode::KEY_F9,
    KeyCode::KEY_F10,
    KeyCode::KEY_F11,
    KeyCode::KEY_F12,
];

/// Named keys usable as a trigger. Letters and digits cover almost everything;
/// the modifier names are here so a bare-modifier binding stays expressible.
fn trigger_key(name: &str) -> Result<KeyCode> {
    if let Some(number) = name.strip_prefix('f').and_then(|n| n.parse::<usize>().ok())
        && (1..=FUNCTION_KEYS.len()).contains(&number)
    {
        return Ok(FUNCTION_KEYS[number - 1]);
    }
    let named = match name {
        "space" => KeyCode::KEY_SPACE,
        "tab" => KeyCode::KEY_TAB,
        "enter" | "return" => KeyCode::KEY_ENTER,
        "capslock" => KeyCode::KEY_CAPSLOCK,
        "leftctrl" => KeyCode::KEY_LEFTCTRL,
        "rightctrl" => KeyCode::KEY_RIGHTCTRL,
        "leftalt" => KeyCode::KEY_LEFTALT,
        "rightalt" => KeyCode::KEY_RIGHTALT,
        "leftshift" => KeyCode::KEY_LEFTSHIFT,
        "rightshift" => KeyCode::KEY_RIGHTSHIFT,
        "leftmeta" => KeyCode::KEY_LEFTMETA,
        "rightmeta" => KeyCode::KEY_RIGHTMETA,
        single if single.chars().count() == 1 => {
            let character = single.chars().next().expect("length checked");
            match character {
                'a'..='z' => LETTERS[character as usize - 'a' as usize],
                // Digits, unlike letters, really are sequential: KEY_1 is 2.
                '1'..='9' => KeyCode(KeyCode::KEY_1.0 + (character as u16 - '1' as u16)),
                '0' => KeyCode::KEY_0,
                _ => return Err(anyhow!("{name:?} is not a key flow can watch")),
            }
        }
        _ => return Err(anyhow!("{name:?} is not a key flow can watch")),
    };
    Ok(named)
}

fn trigger_name(key: KeyCode) -> String {
    match key {
        KeyCode::KEY_SPACE => "space".into(),
        KeyCode::KEY_TAB => "tab".into(),
        KeyCode::KEY_ENTER => "enter".into(),
        KeyCode::KEY_CAPSLOCK => "capslock".into(),
        KeyCode::KEY_LEFTCTRL => "leftctrl".into(),
        KeyCode::KEY_RIGHTCTRL => "rightctrl".into(),
        KeyCode::KEY_LEFTALT => "leftalt".into(),
        KeyCode::KEY_RIGHTALT => "rightalt".into(),
        KeyCode::KEY_LEFTSHIFT => "leftshift".into(),
        KeyCode::KEY_RIGHTSHIFT => "rightshift".into(),
        KeyCode::KEY_LEFTMETA => "leftmeta".into(),
        KeyCode::KEY_RIGHTMETA => "rightmeta".into(),
        key if FUNCTION_KEYS.contains(&key) => {
            let at = FUNCTION_KEYS
                .iter()
                .position(|f| *f == key)
                .expect("checked");
            format!("f{}", at + 1)
        }
        key if LETTERS.contains(&key) => {
            let at = LETTERS.iter().position(|l| *l == key).expect("checked");
            char::from(b'a' + at as u8).to_string()
        }
        KeyCode::KEY_0 => "0".into(),
        KeyCode(code) if (KeyCode::KEY_1.0..=KeyCode::KEY_9.0).contains(&code) => {
            char::from(b'1' + (code - KeyCode::KEY_1.0) as u8).to_string()
        }
        other => format!("{other:?}"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Event {
    Pressed,
    /// Another key arrived while PTT was down, so this was a shortcut, not dictation.
    Cancelled,
    Released {
        held: Duration,
    },
    /// From `flow start` / `flow stop`. Start arms a chord watcher so hold works
    /// even when the compositor's release bind never fires.
    Start,
    Stop,
}

/// Native press and compositor `flow start` are the same physical tap, a few
/// milliseconds apart. Acting on both would start a dictation and stop it
/// before the mic opened.
pub const TAP_ECHO: Duration = Duration::from_millis(150);

/// What a tap-to-talk event does to the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TapAction {
    Begin,
    Finish,
    Ignore,
}

/// Tap-to-talk: the chord (or `flow start`) is a switch. Release does not
/// end the session - including Hyprland's release bind, which sends `flow
/// stop` and would otherwise make this setting a no-op.
pub fn tap_action(event: Event, recording: bool, echo: bool) -> TapAction {
    match event {
        Event::Pressed | Event::Start if echo => TapAction::Ignore,
        Event::Pressed | Event::Start if recording => TapAction::Finish,
        Event::Pressed | Event::Start => TapAction::Begin,
        Event::Released { .. } | Event::Stop => TapAction::Ignore,
        Event::Cancelled => TapAction::Ignore,
    }
}

/// Every keyboard-capable device. A device grabbed by a remapper (keyd) delivers
/// nothing, and its virtual device delivers the post-remap events instead, so
/// reading all of them needs no special-casing for the user's input stack.
/// Probed with a letter rather than a modifier: compact and Apple boards drop the
/// right-hand modifiers, so testing for one of those would skip a real keyboard.
fn keyboards() -> Vec<(std::path::PathBuf, Device)> {
    evdev::enumerate()
        .filter(|(_, device)| {
            device
                .supported_keys()
                .is_some_and(|keys| keys.contains(KeyCode::KEY_A))
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
    chord: Chord,
    /// Physically down right now. A set, so the same press arriving from two
    /// devices collapses to one entry.
    held: HashSet<KeyCode>,
    down_at: Option<Instant>,
    cancelled: bool,
}

impl PttState {
    pub fn new(chord: Chord) -> Self {
        Self {
            chord,
            ..Self::default()
        }
    }

    /// Adopt a new chord, but only between holds.
    ///
    /// Swapping mid-hold would strand the recording: the release watcher looks
    /// for the keys of the chord that started it, and those are no longer the
    /// keys being held. Waiting costs nothing - nobody rebinds their hotkey
    /// while dictating - and it keeps the state machine honest.
    ///
    /// Returns whether the swap happened, so the caller can say so once rather
    /// than every time it checks.
    pub fn retune(&mut self, chord: &Chord) -> bool {
        if self.chord == *chord || self.down_at.is_some() || !self.held.is_empty() {
            return false;
        }
        self.chord = chord.clone();
        true
    }

    /// Returns the transition this key caused, or `None` if it told us nothing
    /// new (autorepeat, a duplicate from a second device, keys while cancelled).
    pub fn apply(&mut self, key: KeyCode, pressed: bool) -> Option<Event> {
        let already = if pressed {
            !self.held.insert(key)
        } else {
            !self.held.remove(&key)
        };
        // Autorepeat, or the second device reporting the same physical edge.
        if already {
            return None;
        }

        // Whichever chord key lands last starts the hold, rather than the trigger
        // specifically. A remapper can deliver the letter before the modifier it
        // rewrote, and waiting for the trigger's own event means such a press
        // records nothing at all - the user presses, nothing happens, they press
        // again. Still requires every modifier down, so a plain `d` stays a plain
        // `d`, and requires the arriving key to belong to the chord, so holding a
        // bare Right Ctrl does not turn someone else's shortcut into a recording.
        if pressed
            && self.down_at.is_none()
            && self.chord.contains(key)
            && self.chord.fully_held(&self.held)
        {
            self.down_at = Some(Instant::now());
            self.cancelled = false;
            return Some(Event::Pressed);
        }

        // Lifting any finger of the chord ends the hold - not only the trigger.
        // Hyprland's release binds are unreliable with modifier chords, so this
        // is the path that actually stops a Super+Shift+D recording.
        if !pressed && self.down_at.is_some() && self.chord.contains(key) {
            let start = self.down_at.take().expect("checked above");
            return (!self.cancelled).then(|| Event::Released {
                held: start.elapsed(),
            });
        }

        // A bare key exists to modify other keys, so an unrelated press there is
        // a shortcut and Right Ctrl + C must stay a copy.
        //
        // A deliberate chord gets no such guard, because cancelling throws the
        // audio away silently and a three-key chord is already unambiguous:
        // nobody reaches for super+shift+d+x. A remapper echoing a physical key
        // beside its virtual one produces exactly this stray press, which is how
        // entire dictations disappeared leaving nothing in the log.
        if pressed
            && self.down_at.is_some()
            && !self.cancelled
            && !self.chord.deliberate()
            && !self.chord.contains(key)
        {
            self.cancelled = true;
            return Some(Event::Cancelled);
        }
        None
    }
}

/// Spawn a reader per keyboard, feeding push-to-talk transitions into `events`.
/// Shares the channel with the signal handler so the daemon has one input stream.
/// `chord` is shared rather than owned so a rebinding in the console reaches
/// the running daemon. The reader adopts it between holds - see
/// [`PttState::retune`].
pub fn spawn(events: Sender<Event>, chord: std::sync::Arc<std::sync::Mutex<Chord>>) -> Result<()> {
    let devices = keyboards();
    if devices.is_empty() {
        return Err(anyhow!(
            "no readable keyboard found - is this user in the 'input' group?"
        ));
    }
    eprintln!("push-to-talk: {}", chord.lock().expect("chord"));

    for (path, device) in &devices {
        crate::verbose!(
            "watching {} ({})",
            path.display(),
            device.name().unwrap_or("?")
        );
    }

    let (raw_tx, raw_rx) = channel();
    for (_, mut device) in devices {
        let raw_tx = raw_tx.clone();
        std::thread::spawn(move || {
            loop {
                let Ok(batch) = device.fetch_events() else {
                    return;
                };
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
            }
        });
    }

    WATCHING.store(true, Ordering::Relaxed);
    std::thread::spawn(move || {
        let mut state = PttState::new(chord.lock().expect("chord").clone());
        while let Ok((key, pressed)) = raw_rx.recv() {
            // Checked per event rather than on a timer: it is one comparison,
            // and it means a rebinding takes effect on the very next key
            // rather than at some interval after it was saved.
            {
                let wanted = chord.lock().expect("chord");
                if state.retune(&wanted) {
                    eprintln!("push-to-talk rebound: {wanted}");
                }
            }
            if MODIFIERS.contains(&key) {
                let mut observed = observed().lock().expect("observed modifiers");
                if pressed {
                    observed.insert(key);
                } else {
                    observed.remove(&key);
                }
            }
            if let Some(event) = state.apply(key, pressed)
                && events.send(event).is_err()
            {
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

/// Devices whose modifier state decides whether it is safe to paste, opened once
/// and held for the life of the daemon - the same thing [`spawn`] does with its
/// reader threads.
///
/// Reads the same preferred devices as [`chord_paths`], keyd's virtual keyboard
/// when there is one, rather than every keyboard. That is not a shortcut: the
/// question here is whether the compositor would reinterpret an injected Ctrl+V,
/// and the compositor sees a remapper's output, not the physical keys behind it.
/// Polling every device instead means a pre-remap physical key that the
/// compositor never sees can hold the paste back until it times out, and the
/// dictation reaches the clipboard and nowhere else.
///
/// Discovery costs ~400ms and opening costs ~50ms, both of which used to be paid
/// before every single paste.
fn modifier_devices() -> &'static Mutex<Vec<Device>> {
    static DEVICES: OnceLock<Mutex<Vec<Device>>> = OnceLock::new();
    DEVICES.get_or_init(|| {
        let paths = chord_paths();
        let devices: Vec<Device> = paths.iter().filter_map(|p| Device::open(p).ok()).collect();
        Mutex::new(if devices.is_empty() {
            keyboards().into_iter().map(|(_, device)| device).collect()
        } else {
            devices
        })
    })
}

/// Call once at daemon start so neither the first tap nor the first injection
/// pays device discovery.
pub fn warmup_devices() {
    let paths = chord_paths();
    if paths.is_empty() {
        eprintln!("chord watch: no keyboard with KEY_D");
    } else {
        for path in paths {
            crate::verbose!("chord watch: {}", path.display());
        }
    }
    let _ = modifier_devices();
}

fn discover_chord_paths() -> Vec<PathBuf> {
    let from_proc = keyd_paths_from_proc();
    if !from_proc.is_empty() {
        return from_proc;
    }

    let mut keyd = Vec::new();
    let mut others = Vec::new();
    for (path, device) in evdev::enumerate() {
        // Probed with a letter, not KEY_D: the configured trigger may be any
        // key, and requiring a specific one skipped real keyboards.
        if !device
            .supported_keys()
            .is_some_and(|keys| keys.contains(KeyCode::KEY_A))
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
            block.lines().any(|line| {
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

fn chord_snapshot(devices: &[Device], chord: &Chord) -> HashSet<KeyCode> {
    let mut keys = HashSet::new();
    for device in devices {
        if let Ok(state) = device.get_key_state() {
            for key in chord.keys() {
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
    pub fn arm(events: Sender<Event>, chord: Chord) -> Self {
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::spawn(move || watch_chord_release(events, flag, chord));
        Self { cancel }
    }

    pub fn disarm(self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn watch_chord_release(events: Sender<Event>, cancel: Arc<AtomicBool>, chord: Chord) {
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
    let mut held = chord_snapshot(&state_devices, &chord);
    while held.is_empty() && Instant::now() < appear_by && !cancel.load(Ordering::Relaxed) {
        std::thread::sleep(CHORD_POLL);
        held = chord_snapshot(&state_devices, &chord);
    }

    // Nothing held means this was not a key being held down at all - a script,
    // a foot pedal, a stream deck. Stopping here would cut those recordings to
    // the ~40ms it took to look, so the explicit `flow stop` is left to end them.
    if held.is_empty() {
        eprintln!("chord watch: no chord keys held - waiting for `flow stop`");
        return;
    }

    eprintln!("chord watch: holding until release ({held:?})");

    let (release_tx, release_rx) = channel();
    let watched = chord.keys();
    for mut device in event_devices {
        let release_tx = release_tx.clone();
        let watched = watched.clone();
        std::thread::spawn(move || {
            loop {
                let Ok(batch) = device.fetch_events() else {
                    return;
                };
                for event in batch {
                    if event.event_type() != EventType::KEY || event.value() != 0 {
                        continue;
                    }
                    let key = KeyCode(event.code());
                    if watched.contains(&key) {
                        let _ = release_tx.send(key);
                    }
                }
            }
        });
    }
    drop(release_tx);

    let mut gone_polls = 0u8;
    while !cancel.load(Ordering::Relaxed) {
        let now = chord_snapshot(&state_devices, &chord);
        if chord_released(&held, &now) {
            gone_polls += 1;
            if gone_polls >= 2 {
                crate::verbose!("chord watch: released (state)");
                let _ = events.send(Event::Stop);
                return;
            }
        } else {
            gone_polls = 0;
        }

        match release_rx.recv_timeout(CHORD_POLL) {
            Ok(key) => {
                let now = chord_snapshot(&state_devices, &chord);
                if chord_released(&held, &now) {
                    crate::verbose!("chord watch: released (event {key:?} + state)");
                    let _ = events.send(Event::Stop);
                    return;
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {}
        }
    }
}

/// Modifiers held according to the events flow has actually seen, and whether
/// anyone is watching.
///
/// `EVIOCGKEY` - what `get_key_state` reads - can be stale, and on this machine it
/// is: a Keychron reports LALT+LSHIFT held with nothing pressed, and keyd mirrors
/// that as LMETA+LSHIFT on its virtual keyboard, which is two thirds of the
/// dictation chord. Asking the devices whether a modifier is down therefore said
/// "yes" forever, and every paste waited out its timeout and left the text on the
/// clipboard.
///
/// The giveaway is that typing still works, so the compositor - the thing that
/// would actually reinterpret an injected Ctrl+V - does not believe those keys are
/// held either. This view starts empty at daemon start and only moves on real
/// events, so a bit that was already stuck cannot poison it.
fn observed() -> &'static Mutex<HashSet<KeyCode>> {
    static OBSERVED: OnceLock<Mutex<HashSet<KeyCode>>> = OnceLock::new();
    OBSERVED.get_or_init(|| Mutex::new(HashSet::new()))
}

static WATCHING: AtomicBool = AtomicBool::new(false);

/// True when a modifier is down. Pure, so the rule is testable without a keyboard.
pub fn any_modifier_in(held: &HashSet<KeyCode>) -> bool {
    MODIFIERS.iter().any(|modifier| held.contains(modifier))
}

/// Which modifiers are currently observed as held. Named for diagnostics when
/// paste fires with a modifier still down - the compositor eats the chord and
/// we need to know which key is stuck to fix it.
pub fn currently_held_modifiers() -> Vec<KeyCode> {
    let observed = observed().lock().expect("observed modifiers");
    MODIFIERS
        .iter()
        .filter(|key| observed.contains(key))
        .copied()
        .collect()
}

/// Block until no modifier is held. Injection no longer waits here: releasing
/// `d` is the end of the hold, and Flow's own keyboard sends Ctrl+V without
/// needing the physical modifiers up.
///
/// Still used by tests, and by anyone who needs to know whether the board is
/// actually at rest. Gives up after `timeout`.
pub fn wait_for_modifiers_released(timeout: Duration) -> bool {
    let started = Instant::now();
    let deadline = started + timeout;
    let watching = WATCHING.load(Ordering::Relaxed);
    // Only consulted when nothing is watching events - a daemon without
    // push-to-talk, where the compositor is the only trigger.
    let devices = (!watching).then(|| modifier_devices().lock().expect("modifier devices"));
    let mut announced = false;

    loop {
        // A wait this long is either someone resting a hand on the keys or a
        // release event this never saw. Injection no longer blocks on this;
        // the line is for whoever still asks.
        if !announced && started.elapsed() > Duration::from_millis(750) {
            announced = true;
            // Names them, because a wait this long is either someone resting a
            // hand on the keys or a release event this never saw, and the two
            // need opposite fixes.
            let stuck: Vec<String> = match &devices {
                None => observed()
                    .lock()
                    .expect("observed modifiers")
                    .iter()
                    .filter(|key| MODIFIERS.contains(key))
                    .map(|key| format!("{key:?}"))
                    .collect(),
                Some(_) => vec!["device state".into()],
            };
            eprintln!("waiting to paste - still held: {}", stuck.join(", "));
        }

        let held = match &devices {
            None => any_modifier_in(&observed().lock().expect("observed modifiers")),
            Some(devices) => devices.iter().any(|device| {
                device
                    .get_key_state()
                    .map(|state| any_modifier_in(&state.iter().collect()))
                    .unwrap_or(false)
            }),
        };

        if !held {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}
