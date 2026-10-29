//! The macOS keyboard source: a listen-only `CGEventTap`, translated into the
//! same `(KeyCode, pressed)` edges the evdev readers produce so the one
//! push-to-talk state machine serves both.
//!
//! `evdev::KeyCode` stays the vocabulary because `Chord` and the config already
//! speak it. Super is Command and Alt is Option.

use anyhow::{Context, Result, anyhow};
use evdev::KeyCode;
use objc2_core_foundation::{CFMachPort, CFRunLoop, kCFRunLoopCommonModes};
use objc2_core_graphics::{
    CGEvent, CGEventField, CGEventFlags, CGEventSource, CGEventSourceStateID, CGEventTapLocation,
    CGEventTapOptions, CGEventTapPlacement, CGEventTapProxy, CGEventType,
    CGPreflightListenEventAccess, CGRequestListenEventAccess,
};
use std::cell::{OnceCell, RefCell};
use std::collections::HashSet;
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::mpsc::{Receiver, Sender, channel};

/// kVK_* virtual keycode to evdev key. Spelled out like `LETTERS` in the parent
/// module: Apple's codes follow the physical layout, so arithmetic on them is
/// wrong in the same way it is for evdev.
///
/// Only keys a chord can name, plus the ones worth telling apart from a stray
/// press. Anything else reaches the state machine as `KEY_UNKNOWN`.
pub const KEYMAP: [(u16, KeyCode); 83] = [
    (0x00, KeyCode::KEY_A),
    (0x01, KeyCode::KEY_S),
    (0x02, KeyCode::KEY_D),
    (0x03, KeyCode::KEY_F),
    (0x04, KeyCode::KEY_H),
    (0x05, KeyCode::KEY_G),
    (0x06, KeyCode::KEY_Z),
    (0x07, KeyCode::KEY_X),
    (0x08, KeyCode::KEY_C),
    (0x09, KeyCode::KEY_V),
    (0x0B, KeyCode::KEY_B),
    (0x0C, KeyCode::KEY_Q),
    (0x0D, KeyCode::KEY_W),
    (0x0E, KeyCode::KEY_E),
    (0x0F, KeyCode::KEY_R),
    (0x10, KeyCode::KEY_Y),
    (0x11, KeyCode::KEY_T),
    (0x12, KeyCode::KEY_1),
    (0x13, KeyCode::KEY_2),
    (0x14, KeyCode::KEY_3),
    (0x15, KeyCode::KEY_4),
    (0x16, KeyCode::KEY_6),
    (0x17, KeyCode::KEY_5),
    (0x18, KeyCode::KEY_EQUAL),
    (0x19, KeyCode::KEY_9),
    (0x1A, KeyCode::KEY_7),
    (0x1B, KeyCode::KEY_MINUS),
    (0x1C, KeyCode::KEY_8),
    (0x1D, KeyCode::KEY_0),
    (0x1E, KeyCode::KEY_RIGHTBRACE),
    (0x1F, KeyCode::KEY_O),
    (0x20, KeyCode::KEY_U),
    (0x21, KeyCode::KEY_LEFTBRACE),
    (0x22, KeyCode::KEY_I),
    (0x23, KeyCode::KEY_P),
    (0x24, KeyCode::KEY_ENTER),
    (0x25, KeyCode::KEY_L),
    (0x26, KeyCode::KEY_J),
    (0x27, KeyCode::KEY_APOSTROPHE),
    (0x28, KeyCode::KEY_K),
    (0x29, KeyCode::KEY_SEMICOLON),
    (0x2A, KeyCode::KEY_BACKSLASH),
    (0x2B, KeyCode::KEY_COMMA),
    (0x2C, KeyCode::KEY_SLASH),
    (0x2D, KeyCode::KEY_N),
    (0x2E, KeyCode::KEY_M),
    (0x2F, KeyCode::KEY_DOT),
    (0x30, KeyCode::KEY_TAB),
    (0x31, KeyCode::KEY_SPACE),
    (0x32, KeyCode::KEY_GRAVE),
    (0x33, KeyCode::KEY_BACKSPACE),
    (0x35, KeyCode::KEY_ESC),
    (0x36, KeyCode::KEY_RIGHTMETA),
    (0x37, KeyCode::KEY_LEFTMETA),
    (0x38, KeyCode::KEY_LEFTSHIFT),
    (0x39, KeyCode::KEY_CAPSLOCK),
    (0x3A, KeyCode::KEY_LEFTALT),
    (0x3B, KeyCode::KEY_LEFTCTRL),
    (0x3C, KeyCode::KEY_RIGHTSHIFT),
    (0x3D, KeyCode::KEY_RIGHTALT),
    (0x3E, KeyCode::KEY_RIGHTCTRL),
    (0x3F, KeyCode::KEY_FN),
    (0x60, KeyCode::KEY_F5),
    (0x61, KeyCode::KEY_F6),
    (0x62, KeyCode::KEY_F7),
    (0x63, KeyCode::KEY_F3),
    (0x64, KeyCode::KEY_F8),
    (0x65, KeyCode::KEY_F9),
    (0x67, KeyCode::KEY_F11),
    (0x6D, KeyCode::KEY_F10),
    (0x6F, KeyCode::KEY_F12),
    (0x73, KeyCode::KEY_HOME),
    (0x74, KeyCode::KEY_PAGEUP),
    (0x75, KeyCode::KEY_DELETE),
    (0x76, KeyCode::KEY_F4),
    (0x77, KeyCode::KEY_END),
    (0x78, KeyCode::KEY_F2),
    (0x79, KeyCode::KEY_PAGEDOWN),
    (0x7A, KeyCode::KEY_F1),
    (0x7B, KeyCode::KEY_LEFT),
    (0x7C, KeyCode::KEY_RIGHT),
    (0x7D, KeyCode::KEY_DOWN),
    (0x7E, KeyCode::KEY_UP),
];

pub fn key_from_virtual_keycode(code: u16) -> Option<KeyCode> {
    KEYMAP
        .iter()
        .find_map(|(candidate, key)| (*candidate == code).then_some(*key))
}

pub fn virtual_keycode_from_key(key: KeyCode) -> Option<u16> {
    KEYMAP
        .iter()
        .find_map(|(code, candidate)| (*candidate == key).then_some(*code))
}

/// A modifier key: its virtual keycode, its own bit among the device-dependent
/// flags, and the device-independent flag every key of its kind shares.
///
/// The per-side bits are IOKit's `NX_DEVICE*KEYMASK` and are not exposed by
/// CoreGraphics. They matter because the shared flag stays set while either
/// shift is down, so releasing the left one with the right one still held is
/// invisible without them.
struct ModifierKey {
    code: u16,
    side: u64,
    family: u64,
}

const SHIFT: u64 = CGEventFlags::MaskShift.0;
const CONTROL: u64 = CGEventFlags::MaskControl.0;
const OPTION: u64 = CGEventFlags::MaskAlternate.0;
const COMMAND: u64 = CGEventFlags::MaskCommand.0;

/// kVK_Function, the Fn / globe key.
const KVK_FUNCTION: u16 = 0x3F;

/// Set on Fn's own `flagsChanged` event while it is down - and also on arrows,
/// F-keys, Home, End, PageUp, PageDown and forward delete, because macOS marks
/// those as Fn-chorded. So it means "Fn" only on Fn's own event, which is why
/// it is read there alone and kept out of [`modifiers_from_flags`].
const SECONDARY_FN: u64 = CGEventFlags::MaskSecondaryFn.0;

const MODIFIER_FLAG_BITS: [ModifierKey; 8] = [
    ModifierKey {
        code: 0x3B,
        side: 0x0000_0001,
        family: CONTROL,
    },
    ModifierKey {
        code: 0x3E,
        side: 0x0000_2000,
        family: CONTROL,
    },
    ModifierKey {
        code: 0x38,
        side: 0x0000_0002,
        family: SHIFT,
    },
    ModifierKey {
        code: 0x3C,
        side: 0x0000_0004,
        family: SHIFT,
    },
    ModifierKey {
        code: 0x3A,
        side: 0x0000_0020,
        family: OPTION,
    },
    ModifierKey {
        code: 0x3D,
        side: 0x0000_0040,
        family: OPTION,
    },
    ModifierKey {
        code: 0x37,
        side: 0x0000_0008,
        family: COMMAND,
    },
    ModifierKey {
        code: 0x36,
        side: 0x0000_0010,
        family: COMMAND,
    },
];

impl ModifierKey {
    fn key(&self) -> Option<KeyCode> {
        key_from_virtual_keycode(self.code)
    }

    /// Every side bit of this key's kind.
    fn family_sides(&self) -> u64 {
        MODIFIER_FLAG_BITS
            .iter()
            .filter(|other| other.family == self.family)
            .fold(0, |bits, other| bits | other.side)
    }

    /// Whether `flags` has this key down. Synthetic events can carry the shared
    /// flag with no side bits at all; then the flag is all there is to go on.
    fn down_in(&self, flags: u64) -> bool {
        if flags & self.family_sides() != 0 {
            flags & self.side != 0
        } else {
            flags & self.family != 0
        }
    }
}

/// What a `flagsChanged` event says about the modifier whose key it names: the
/// key, and whether it went down. `None` for anything that is not one of the
/// eight modifiers or Fn. Caps Lock is among those: macOS reports it as a toggle, not
/// as a key that is held.
pub fn modifier_transition(code: u16, flags: u64) -> Option<(KeyCode, bool)> {
    if code == KVK_FUNCTION {
        return Some((KeyCode::KEY_FN, flags & SECONDARY_FN != 0));
    }
    let modifier = MODIFIER_FLAG_BITS
        .iter()
        .find(|modifier| modifier.code == code)?;
    Some((modifier.key()?, modifier.down_in(flags)))
}

/// The modifiers `flags` has down. Flags that name only the shared bit cannot
/// say which side, so they name the left key.
pub fn modifiers_from_flags(flags: u64) -> Vec<KeyCode> {
    MODIFIER_FLAG_BITS
        .iter()
        .filter(|modifier| {
            let sided = flags & modifier.family_sides() != 0;
            let first_of_kind = MODIFIER_FLAG_BITS
                .iter()
                .find(|other| other.family == modifier.family)
                .is_some_and(|other| other.code == modifier.code);
            modifier.down_in(flags) && (sided || first_of_kind)
        })
        .filter_map(ModifierKey::key)
        .collect()
}

/// Keys believed held that the system says are not. `is_down` answers for a
/// virtual keycode; modifiers are judged from `flags` instead, because their
/// key state is per side and the flags already carry it.
pub fn stale_keys(
    held: &HashSet<KeyCode>,
    flags: u64,
    is_down: impl Fn(u16) -> bool,
) -> Vec<KeyCode> {
    let modifiers_down = modifiers_from_flags(flags);
    let is_modifier = |key: &KeyCode| {
        MODIFIER_FLAG_BITS
            .iter()
            .any(|modifier| modifier.key() == Some(*key))
    };
    held.iter()
        .copied()
        .filter(|key| {
            if is_modifier(key) {
                !modifiers_down.contains(key)
            } else {
                virtual_keycode_from_key(*key).is_none_or(|code| !is_down(code))
            }
        })
        .collect()
}

/// For the paste, when no tap is watching: what the system believes is held.
pub fn modifiers_held() -> bool {
    let state = CGEventSourceStateID::CombinedSessionState;
    !modifiers_from_flags(CGEventSource::flags_state(state).0).is_empty()
        || CGEventSource::key_state(state, KVK_FUNCTION)
}

struct TapContext {
    edges: Sender<(KeyCode, bool)>,
    /// What this tap has reported as down, to compare against the system's
    /// own view once the tap has been off.
    held: RefCell<HashSet<KeyCode>>,
    tap: OnceCell<objc2_core_foundation::CFRetained<CFMachPort>>,
}

/// Start the tap on its own thread, which then lives in a run loop for the rest
/// of the process, and hand back the key edges it sees.
///
/// Fails when the tap cannot be made, which is what macOS does without Input
/// Monitoring permission - there is no other signal.
pub fn listen() -> Result<Receiver<(KeyCode, bool)>> {
    let (edges, receiver) = channel();
    let (ready, started) = channel();
    std::thread::Builder::new()
        .name("hotkey-tap".into())
        .spawn(move || run_tap(edges, ready))
        .context("starting the keyboard tap thread")?;
    started
        .recv()
        .context("the keyboard tap thread ended before it started")??;
    Ok(receiver)
}

fn run_tap(edges: Sender<(KeyCode, bool)>, ready: Sender<Result<()>>) {
    let context = Box::new(TapContext {
        edges,
        held: RefCell::default(),
        tap: OnceCell::new(),
    });
    let wanted = [
        CGEventType::KeyDown,
        CGEventType::KeyUp,
        CGEventType::FlagsChanged,
    ]
    .iter()
    .fold(0u64, |mask, kind| mask | 1 << kind.0);

    // SAFETY: `on_event` matches CGEventTapCallBack, and `context` is only
    // freed when this function returns, which is after the run loop that calls
    // it has stopped.
    let tap = unsafe {
        CGEvent::tap_create(
            CGEventTapLocation::SessionEventTap,
            CGEventTapPlacement::HeadInsertEventTap,
            CGEventTapOptions::ListenOnly,
            wanted,
            Some(on_event),
            std::ptr::from_ref::<TapContext>(&context).cast_mut().cast(),
        )
    };
    let Some(tap) = tap else {
        // Prompts for Input Monitoring if it has never been asked, so the app
        // appears in the list to be ticked.
        if !CGPreflightListenEventAccess() {
            CGRequestListenEventAccess();
        }
        let _ = ready.send(Err(anyhow!(
            "could not watch the keyboard - grant Input Monitoring to the app running flow \
             in System Settings > Privacy & Security, then restart flow"
        )));
        return;
    };

    let source = CFMachPort::new_run_loop_source(None, Some(&tap), 0);
    let run_loop = CFRunLoop::current();
    let (Some(source), Some(run_loop)) = (source, run_loop) else {
        let _ = ready.send(Err(anyhow!(
            "could not attach the keyboard tap to a run loop"
        )));
        return;
    };
    // SAFETY: an immutable constant exported by CoreFoundation.
    run_loop.add_source(Some(&source), unsafe { kCFRunLoopCommonModes });
    CGEvent::tap_enable(&tap, true);
    let _ = context.tap.set(tap);

    let _ = ready.send(Ok(()));
    CFRunLoop::run();
}

/// macOS switches a tap off when its callback is slow or when it decides to,
/// and says so only through this callback. Without turning it back on, push to
/// talk stops working until the daemon restarts and nothing says why.
///
/// The event for those two notices is not a real one, so it is not read.
unsafe extern "C-unwind" fn on_event(
    _proxy: CGEventTapProxy,
    kind: CGEventType,
    event: NonNull<CGEvent>,
    context: *mut c_void,
) -> *mut CGEvent {
    // SAFETY: `context` is the `TapContext` that `run_tap` keeps alive.
    let context = unsafe { &*context.cast::<TapContext>() };
    match kind {
        CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput => {
            if let Some(tap) = context.tap.get() {
                CGEvent::tap_enable(tap, true);
            }
            release_stale(context);
        }
        CGEventType::KeyDown | CGEventType::KeyUp | CGEventType::FlagsChanged => {
            // SAFETY: a real event, valid for the duration of the callback.
            let edge = edge_of(kind, unsafe { event.as_ref() });
            if let Some(edge) = edge {
                send_edge(context, edge);
            }
        }
        _ => {}
    }
    event.as_ptr()
}

fn send_edge(context: &TapContext, (key, pressed): (KeyCode, bool)) {
    let mut held = context.held.borrow_mut();
    if key != KeyCode::KEY_UNKNOWN {
        if pressed {
            held.insert(key);
        } else {
            held.remove(&key);
        }
    }
    let _ = context.edges.send((key, pressed));
}

/// Releases that happened while the tap was off never arrive, and a key
/// believed down forever makes the next paste wait out its whole timeout and
/// the next press of that key a no-op. Looks at what the system holds now and
/// sends the missing releases the way real ones travel.
fn release_stale(context: &TapContext) {
    let state = CGEventSourceStateID::CombinedSessionState;
    let flags = CGEventSource::flags_state(state).0;
    let stale = stale_keys(&context.held.borrow(), flags, |code| {
        CGEventSource::key_state(state, code)
    });
    for key in stale {
        send_edge(context, (key, false));
    }
}

fn edge_of(kind: CGEventType, event: &CGEvent) -> Option<(KeyCode, bool)> {
    let code = CGEvent::integer_value_field(Some(event), CGEventField::KeyboardEventKeycode) as u16;
    let key = key_from_virtual_keycode(code).unwrap_or(KeyCode::KEY_UNKNOWN);
    match kind {
        CGEventType::KeyDown => Some((key, true)),
        CGEventType::KeyUp => Some((key, false)),
        CGEventType::FlagsChanged => modifier_transition(code, CGEvent::flags(Some(event)).0),
        _ => None,
    }
}
