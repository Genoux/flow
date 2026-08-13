use anyhow::{Context, Result};
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, KeyCode, KeyEvent};
use std::time::Duration;
use wl_clipboard_rs::copy::{self, MimeType as CopyMime, Options, Source};
use wl_clipboard_rs::paste::{self, ClipboardType, MimeType as PasteMime, Seat};

/// Modifiers must settle before we paste, or the keystroke is reinterpreted by
/// the compositor's keybind layer.
///
/// Generous on purpose. Releasing `d` ends a super+shift+d hold while both
/// modifiers are still down, and someone who pauses with their hand on the keys
/// used to lose the dictation to a 2s limit - it reached the clipboard and
/// nowhere else. Waiting costs nothing; giving up costs the user their words.
const MODIFIER_TIMEOUT: Duration = Duration::from_secs(20);

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
        keys.insert(KeyCode::KEY_LEFTCTRL);
        keys.insert(KeyCode::KEY_LEFTSHIFT);
        keys.insert(KeyCode::KEY_V);

        let device = VirtualDevice::builder()
            .context("opening /dev/uinput - is the uaccess udev rule present?")?
            .name("flow virtual keyboard")
            .with_keys(&keys)?
            .build()?;

        std::thread::sleep(DEVICE_SETTLE);
        Ok(Self { device })
    }

    /// Put `text` in the focused window by staging it on the clipboard and
    /// sending one paste chord. A single chord rather than per-character typing
    /// keeps the text off the keybind layer and is layout-independent.
    pub fn inject(&mut self, text: &str, terminal: bool) -> Result<()> {
        let saved = read_clipboard();

        copy::copy(
            Options::new(),
            Source::Bytes(text.as_bytes().into()),
            CopyMime::Text,
        )
        .context("staging text on the clipboard")?;

        if !super::hotkey::wait_for_modifiers_released(MODIFIER_TIMEOUT) {
            // Pasting now would fire shortcuts instead of inserting text. The
            // text stays on the clipboard, and says so loudly: silence here reads
            // as a dictation that simply vanished.
            anyhow::bail!(
                "modifiers still held after {MODIFIER_TIMEOUT:?} - not pasting, \
                 press ctrl+v to place the text yourself"
            );
        }

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
        let mut chord = vec![KeyCode::KEY_LEFTCTRL];
        if terminal {
            chord.push(KeyCode::KEY_LEFTSHIFT);
        }
        chord.push(KeyCode::KEY_V);

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

/// Best effort - an empty or non-text clipboard is normal, not an error.
fn read_clipboard() -> Option<String> {
    let (mut reader, _) =
        paste::get_contents(ClipboardType::Regular, Seat::Unspecified, PasteMime::Text).ok()?;
    let mut buffer = String::new();
    std::io::Read::read_to_string(&mut reader, &mut buffer).ok()?;
    Some(buffer)
}
