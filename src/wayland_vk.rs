//! The paste chord, sent through `zwp_virtual_keyboard_v1`.
//!
//! # Why this exists rather than uinput alone
//!
//! uinput types by pressing physical keys, and Flow pastes while the
//! push-to-talk chord may still be held. Ctrl+V pressed underneath a held
//! Super+Shift arrives at the compositor as Super+Shift+Ctrl+V, which is a
//! different chord and usually a window-manager binding rather than a paste.
//!
//! The virtual-keyboard protocol declares the depressed-modifier mask as a
//! number instead of pressing modifier keys. Flow can therefore state "Control
//! is down, nothing else is" regardless of what the user's fingers are doing,
//! and the application receives exactly the chord Flow meant to send.
//!
//! # Scope
//!
//! Flow pastes; it never types. The uploaded keymap defines the three keys the
//! paste chord can contain and nothing else, and [`VirtualKeyboard::send_combo`]
//! refuses anything outside it rather than silently sending a key the
//! compositor would read as blank.

use anyhow::{Context, Result, bail};
use evdev::KeyCode;
use std::fs::File;
use std::io::Write;
use std::os::fd::AsFd;
use std::time::{Duration, Instant};
use wayland_client::protocol::{wl_registry, wl_seat};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};

/// `XKB_KEYMAP_FORMAT_TEXT_V1`, the only format the protocol defines.
const KEYMAP_FORMAT_TEXT: u32 = 1;

const KEY_RELEASED: u32 = 0;
const KEY_PRESSED: u32 = 1;

/// XKB numbers the same physical key 8 higher than evdev does, but
/// `zwp_virtual_keyboard_v1.key` inherits `wl_keyboard.key`'s convention and
/// takes the evdev number, which the compositor adds 8 to before looking it up.
/// So the keymap is written in XKB numbering and the wire carries evdev
/// numbering, and the two must not be mixed up. Only the test that checks the
/// two agree needs to convert between them.
#[cfg(test)]
const XKB_KEYCODE_OFFSET: u32 = 8;

/// Bit positions of the real modifiers, fixed by XKB and relied on by the
/// `modifier_map` entries in [`KEYMAP`].
const SHIFT_MASK: u32 = 1 << 0;
const CONTROL_MASK: u32 = 1 << 2;

/// The keymap Flow uploads.
///
/// `include "complete"` pulls the stock types and compat rules from
/// xkeyboard-config, which every desktop that runs libxkbcommon already has.
/// Writing them out by hand would be a page of boilerplate to say "behave
/// normally".
///
/// The modifier keys are declared and bound with `modifier_map` because a mask
/// bit only means Control if some key in the keymap makes Control real. They
/// are never pressed - the mask is what Flow sends.
const KEYMAP: &str = r#"xkb_keymap {
xkb_keycodes "flow" {
minimum = 8;
maximum = 55;
<K37> = 37;
<K50> = 50;
<K55> = 55;
};
xkb_types "flow" { include "complete" };
xkb_compatibility "flow" { include "complete" };
xkb_symbols "flow" {
key <K37> { [ Control_L ] };
key <K50> { [ Shift_L ] };
key <K55> { [ v, V ] };
modifier_map Control { <K37> };
modifier_map Shift { <K50> };
};
};
"#;

/// Globals collected from the registry.
#[derive(Default)]
struct Globals {
    seat: Option<wl_seat::WlSeat>,
    manager: Option<ZwpVirtualKeyboardManagerV1>,
}

pub struct VirtualKeyboard {
    queue: EventQueue<Globals>,
    globals: Globals,
    keyboard: ZwpVirtualKeyboardV1,
    key_delay: Duration,
    /// The protocol wants a millisecond timestamp. Its only requirement is that
    /// it advances, so elapsed time since connect is as good as a clock and
    /// cannot jump backwards when the system clock is adjusted.
    started: Instant,
}

impl VirtualKeyboard {
    /// Fails when there is no Wayland session or the compositor does not
    /// implement the protocol, which is the caller's cue to fall back to
    /// uinput.
    pub fn new(key_delay: Duration) -> Result<Self> {
        let connection = Connection::connect_to_env().context("no wayland display")?;
        let mut queue = connection.new_event_queue();
        let handle = queue.handle();
        connection.display().get_registry(&handle, ());

        let mut globals = Globals::default();
        queue
            .roundtrip(&mut globals)
            .context("wayland registry roundtrip")?;

        let seat = globals.seat.clone().context("compositor offers no wl_seat")?;
        let manager = globals
            .manager
            .clone()
            .context("compositor does not implement zwp_virtual_keyboard_manager_v1")?;

        let keyboard = manager.create_virtual_keyboard(&seat, &handle, ());
        upload_keymap(&keyboard)?;

        // The keymap is only live once the compositor has processed it; a key
        // sent before that is looked up in whatever keymap preceded it.
        queue
            .roundtrip(&mut globals)
            .context("wayland keymap roundtrip")?;

        Ok(Self {
            queue,
            globals,
            keyboard,
            key_delay,
            started: Instant::now(),
        })
    }

    /// Send one chord: the modifiers as a mask, the single non-modifier key as
    /// a press and release.
    pub fn send_combo(&mut self, keys: &[KeyCode]) -> Result<()> {
        let (mask, key) = split_chord(keys)?;
        let evdev_keycode = u32::from(key.code());

        self.set_modifiers(mask)?;
        self.send_key(evdev_keycode, KEY_PRESSED)?;
        self.send_key(evdev_keycode, KEY_RELEASED)?;
        self.set_modifiers(0)
    }

    fn set_modifiers(&mut self, depressed: u32) -> Result<()> {
        self.keyboard.modifiers(depressed, 0, 0, 0);
        self.flush()
    }

    fn send_key(&mut self, evdev_keycode: u32, state: u32) -> Result<()> {
        let time = self.started.elapsed().as_millis() as u32;
        self.keyboard.key(time, evdev_keycode, state);
        self.flush()
    }

    /// Push the request out and give the compositor time to see the transition
    /// as its own event; below roughly 8ms some clients coalesce the press and
    /// release and miss the chord.
    fn flush(&mut self) -> Result<()> {
        self.queue
            .flush()
            .context("flushing virtual keyboard request")?;
        std::thread::sleep(self.key_delay);
        // Keeps the queue from accumulating events (and surfaces a protocol
        // error as an error here rather than a silent disconnect later).
        self.queue
            .dispatch_pending(&mut self.globals)
            .context("dispatching virtual keyboard events")?;
        Ok(())
    }
}

/// Split a chord into its modifier mask and the one key that is actually
/// pressed.
fn split_chord(keys: &[KeyCode]) -> Result<(u32, KeyCode)> {
    let mut mask = 0;
    let mut pressed = None;
    for key in keys {
        match *key {
            KeyCode::KEY_LEFTCTRL => mask |= CONTROL_MASK,
            KeyCode::KEY_LEFTSHIFT => mask |= SHIFT_MASK,
            KeyCode::KEY_V if pressed.is_none() => pressed = Some(*key),
            other => bail!("{other:?} is not in the uploaded keymap"),
        }
    }
    let pressed = pressed.context("chord has no key to press, only modifiers")?;
    Ok((mask, pressed))
}

/// Hand the keymap to the compositor as a file descriptor it can mmap.
///
/// The size includes the trailing NUL: the compositor reads the mapping as a C
/// string, and without it the parse runs off the end of the mapping.
///
/// The file is unlinked as soon as it is open. The descriptor keeps it alive
/// for as long as either side needs it, and nothing is left in the runtime
/// directory if Flow is killed between here and exit.
///
/// It is opened readable as well as writable because the compositor mmaps the
/// descriptor with `PROT_READ`; on a write-only fd that call fails and the
/// compositor answers `wl_display.error` "no memory" instead of anything that
/// names the real problem.
fn upload_keymap(keyboard: &ZwpVirtualKeyboardV1) -> Result<()> {
    let directory = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/tmp".to_string());
    let path = format!("{directory}/flow-keymap-{}", std::process::id());

    let mut file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)
        .with_context(|| format!("creating {path}"))?;
    std::fs::remove_file(&path).with_context(|| format!("unlinking {path}"))?;

    file.write_all(KEYMAP.as_bytes())
        .and_then(|()| file.write_all(&[0]))
        .context("writing keymap")?;
    file.flush().context("flushing keymap")?;

    let size = (KEYMAP.len() + 1) as u32;
    keyboard.keymap(KEYMAP_FORMAT_TEXT, file.as_fd(), size);
    Ok(())
}

impl Dispatch<wl_registry::WlRegistry, ()> for Globals {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        match &interface[..] {
            "wl_seat" => state.seat = Some(registry.bind(name, version.min(7), queue, ())),
            "zwp_virtual_keyboard_manager_v1" => {
                state.manager = Some(registry.bind(name, 1, queue, ()));
            }
            _ => {}
        }
    }
}

delegate_noop!(Globals: ignore wl_seat::WlSeat);
delegate_noop!(Globals: ignore ZwpVirtualKeyboardManagerV1);
delegate_noop!(Globals: ignore ZwpVirtualKeyboardV1);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_v_is_control_masked_with_v_pressed() {
        let (mask, key) =
            split_chord(&[KeyCode::KEY_LEFTCTRL, KeyCode::KEY_V]).expect("valid chord");
        assert_eq!(mask, CONTROL_MASK);
        assert_eq!(key, KeyCode::KEY_V);
    }

    #[test]
    fn the_terminal_chord_adds_shift_without_changing_the_key() {
        let (mask, key) = split_chord(&[
            KeyCode::KEY_LEFTCTRL,
            KeyCode::KEY_LEFTSHIFT,
            KeyCode::KEY_V,
        ])
        .expect("valid chord");
        assert_eq!(mask, CONTROL_MASK | SHIFT_MASK);
        assert_eq!(key, KeyCode::KEY_V);
    }

    /// A key with no entry in the uploaded keymap would be looked up and found
    /// blank, so the paste would silently do nothing. Better to say so.
    #[test]
    fn a_key_outside_the_keymap_is_refused() {
        assert!(split_chord(&[KeyCode::KEY_LEFTCTRL, KeyCode::KEY_C]).is_err());
    }

    #[test]
    fn modifiers_alone_are_not_a_chord() {
        assert!(split_chord(&[KeyCode::KEY_LEFTCTRL]).is_err());
    }

    /// The keymap declares every keycode the chord can name, in XKB numbering.
    #[test]
    fn the_keymap_declares_every_key_the_chord_can_use() {
        for key in [
            KeyCode::KEY_LEFTCTRL,
            KeyCode::KEY_LEFTSHIFT,
            KeyCode::KEY_V,
        ] {
            let xkb = u32::from(key.code()) + XKB_KEYCODE_OFFSET;
            assert!(
                KEYMAP.contains(&format!("<K{xkb}>")),
                "{key:?} (XKB {xkb}) is missing from the keymap"
            );
        }
    }
}
