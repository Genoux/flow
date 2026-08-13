use anyhow::{Context, Result};
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, KeyCode, KeyEvent};
use std::time::Duration;
use wl_clipboard_rs::copy::{self, MimeType as CopyMime, Options, Source};
use wl_clipboard_rs::paste::{self, ClipboardType, MimeType as PasteMime, Seat};

/// uinput devices need a moment for udev to create the node and for compositors
/// to pick them up; emitting immediately after build silently drops events.
const DEVICE_SETTLE: Duration = Duration::from_millis(300);

/// Give the compositor time to observe each key transition. Below ~8ms some
/// clients coalesce the press and release and miss the combo entirely.
const KEY_DELAY: Duration = Duration::from_millis(12);

pub struct Injector {
    device: VirtualDevice,
}

impl Injector {
    /// Built once and kept alive - rebuilding per injection would pay the
    /// settle delay every time.
    pub fn new() -> Result<Self> {
        let mut keys = AttributeSet::<KeyCode>::new();
        for key in paste_keys(true) {
            keys.insert(key);
        }

        let device = VirtualDevice::builder()
            .context("opening /dev/uinput - is the uaccess udev rule present?")?
            .name("flow virtual keyboard")
            .with_keys(&keys)?
            .build()?;

        std::thread::sleep(DEVICE_SETTLE);
        Ok(Self { device })
    }

    /// Put `text` in the focused window by staging it on the clipboard and
    /// sending one paste chord from Flow's own keyboard.
    ///
    /// Do not wait for Super+Shift to come up. Releasing `d` is the end of the
    /// hold; the modifiers stay down because that is how a chord is released.
    /// Waiting for them was the 8s stall on "Okay."
    ///
    /// Safe because this is Ctrl+V on this device, not typed characters.
    /// SUPER+m cannot fire: we never emit M. Super+Shift live on the physical
    /// board, not on this one, so the chord the focused client sees is Ctrl+V.
    pub fn inject(&mut self, text: &str, terminal: bool) -> Result<()> {
        let saved = read_clipboard();

        copy::copy(
            Options::new(),
            Source::Bytes(text.as_bytes().into()),
            CopyMime::Text,
        )
        .context("staging text on the clipboard")?;

        self.paste(terminal)?;

        if let Some(previous) = saved {
            std::thread::sleep(KEY_DELAY * 4);
            let _ = copy::copy(
                Options::new(),
                Source::Bytes(previous.into_bytes().into()),
                CopyMime::Text,
            );
        }
        Ok(())
    }

    fn paste(&mut self, terminal: bool) -> Result<()> {
        let chord = paste_keys(terminal);
        for key in &chord {
            self.device.emit(&[*KeyEvent::new(*key, 1)])?;
            std::thread::sleep(KEY_DELAY);
        }
        for key in chord.iter().rev() {
            self.device.emit(&[*KeyEvent::new(*key, 0)])?;
            std::thread::sleep(KEY_DELAY);
        }
        Ok(())
    }
}

/// The paste chord, and only the paste chord. Super and letters stay off this
/// list on purpose: the physical hold is still down when we fire, and a typed
/// character would become a shortcut.
pub fn paste_keys(terminal: bool) -> Vec<KeyCode> {
    let mut chord = vec![KeyCode::KEY_LEFTCTRL];
    if terminal {
        chord.push(KeyCode::KEY_LEFTSHIFT);
    }
    chord.push(KeyCode::KEY_V);
    chord
}

/// Best effort - an empty or non-text clipboard is normal, not an error.
fn read_clipboard() -> Option<String> {
    let (mut reader, _) =
        paste::get_contents(ClipboardType::Regular, Seat::Unspecified, PasteMime::Text).ok()?;
    let mut buffer = String::new();
    std::io::Read::read_to_string(&mut reader, &mut buffer).ok()?;
    Some(buffer)
}
