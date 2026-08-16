use crate::hotkey;
use anyhow::{Context, Result};
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, KeyCode, KeyEvent};
use std::time::Duration;
use wl_clipboard_rs::copy::{self, MimeSource, MimeType as CopyMime, Options, Source};
use wl_clipboard_rs::paste::{self, ClipboardType, MimeType as PasteMime, Seat};
use xkb_type::wayland_vk::WaylandVkKeyboard;
use xkb_type::KeyInjector;

/// uinput devices need a moment for udev to create the node and for compositors
/// to pick them up; emitting immediately after build silently drops events.
const DEVICE_SETTLE: Duration = Duration::from_millis(300);

/// Give the compositor time to observe each key transition. Below ~8ms some
/// clients coalesce the press and release and miss the combo entirely.
const KEY_DELAY: Duration = Duration::from_millis(12);

/// Wait cap for physical modifiers to release before firing the paste chord on
/// the uinput fallback path. The Wayland vk path does not need this - the
/// virtual-keyboard protocol declares its modifier mask atomically, so the
/// compositor no longer conflates our Ctrl with a still-held physical Super.
const MODIFIER_WAIT: Duration = Duration::from_millis(500);

/// How long the clipboard has to answer before its contents are written off.
/// A responsive owner answers in single-digit milliseconds; this is generous
/// enough that a real image round-trips, and short enough that an unresponsive
/// one is barely felt.
const CLIPBOARD_BUDGET: Duration = Duration::from_millis(500);

/// How long the compositor has to say which window has focus. Only decides
/// which paste chord to send, so falling back to the configured default costs
/// far less than waiting.
const FOCUS_BUDGET: Duration = Duration::from_millis(200);

/// Two ways to deliver the paste chord to the focused window.
///
/// The Wayland virtual-keyboard protocol (`zwp_virtual_keyboard_v1`) is the
/// preferred path: it declares its depressed-modifier mask directly instead of
/// pressing physical modifier keys, so the compositor sees a clean Ctrl+V
/// even when the user's PTT chord still holds Super+Shift. Uinput remains as
/// the fallback for X11 or the handful of Wayland compositors that don't
/// implement the protocol.
enum Backend {
    Wayland(WaylandVkKeyboard),
    Uinput(VirtualDevice),
}

pub struct Injector {
    backend: Backend,
}

impl Injector {
    /// Built once and kept alive - rebuilding per injection would pay the
    /// settle delay every time.
    ///
    /// Prefers the Wayland virtual-keyboard protocol so the paste chord is
    /// unaffected by held physical modifiers; falls back to a uinput device
    /// when the compositor doesn't advertise the protocol (or there is no
    /// Wayland session, e.g. X11).
    pub fn new() -> Result<Self> {
        match WaylandVkKeyboard::new(KEY_DELAY) {
            Ok(kb) => {
                eprintln!("inject: using wayland virtual-keyboard");
                Ok(Self {
                    backend: Backend::Wayland(kb),
                })
            }
            Err(err) => {
                eprintln!("inject: wayland vk unavailable ({err}), falling back to uinput");
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
                Ok(Self {
                    backend: Backend::Uinput(device),
                })
            }
        }
    }

    /// Put `text` in the focused window by staging it on the clipboard and
    /// sending one paste chord from Flow's own keyboard.
    ///
    /// On the Wayland vk path the modifier mask is declared atomically at
    /// paste time, so no wait for physical modifiers is needed. On the uinput
    /// fallback the compositor aggregates our Ctrl with any still-held
    /// physical Super/Shift, so the wait is kept there.
    pub fn inject(&mut self, text: &str, terminal_hint: bool) -> Result<()> {
        let saved = snapshot_clipboard();

        copy::copy(
            Options::new(),
            Source::Bytes(text.as_bytes().into()),
            CopyMime::Text,
        )
        .context("staging text on the clipboard")?;

        if matches!(self.backend, Backend::Uinput(_))
            && !hotkey::wait_for_modifiers_released(MODIFIER_WAIT)
        {
            let stuck: Vec<String> = hotkey::currently_held_modifiers()
                .iter()
                .map(|key| format!("{key:?}"))
                .collect();
            eprintln!(
                "paste: firing after {MODIFIER_WAIT:?} with {} still held - compositor may eat the chord",
                if stuck.is_empty() {
                    "no observed modifier".into()
                } else {
                    stuck.join(", ")
                },
            );
        }

        let terminal = detect_terminal_focus().unwrap_or(terminal_hint);
        self.paste(terminal)?;

        if !saved.is_empty() {
            std::thread::sleep(KEY_DELAY * 4);
            let _ = copy::copy_multi(Options::new(), saved);
        }
        Ok(())
    }

    fn paste(&mut self, terminal: bool) -> Result<()> {
        let chord = paste_keys(terminal);
        match &mut self.backend {
            Backend::Wayland(kb) => kb.send_combo(&chord).context("wayland vk send_combo"),
            Backend::Uinput(device) => {
                for key in &chord {
                    device.emit(&[*KeyEvent::new(*key, 1)])?;
                    std::thread::sleep(KEY_DELAY);
                }
                for key in chord.iter().rev() {
                    device.emit(&[*KeyEvent::new(*key, 0)])?;
                    std::thread::sleep(KEY_DELAY);
                }
                Ok(())
            }
        }
    }
}

/// Ask Hyprland what has focus and decide whether it needs the terminal chord.
/// Config's `terminal` flag is static per-daemon; the user's real machine has
/// both a browser and a terminal open, so a static flag is always wrong for
/// one of them. This is the per-injection answer.
///
/// Returns `None` when hyprctl is missing or the window has no readable class
/// (e.g. a Wayland client that never set `app_id`); the caller falls back to
/// the configured hint in that case.
pub fn detect_terminal_focus() -> Option<bool> {
    // Bounded: `.output()` waits for the child forever, and a compositor busy
    // enough not to answer would otherwise take the dictation down with it.
    let class = with_deadline(FOCUS_BUDGET, focused_window_class)??;
    Some(is_terminal_class(&class))
}

fn focused_window_class() -> Option<String> {
    let output = std::process::Command::new("hyprctl")
        .arg("activewindow")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = std::str::from_utf8(&output.stdout).ok()?;
    let class = text
        .lines()
        .find_map(|line| line.trim().strip_prefix("class:"))
        .map(|s| s.trim().to_ascii_lowercase())?;
    (!class.is_empty()).then_some(class)
}

/// Match by exact class or reverse-DNS suffix so both `kitty` and
/// `com.mitchellh.ghostty` are recognised. Substring matches would misfire
/// (a `.footnote` app would look like `foot`).
pub fn is_terminal_class(class: &str) -> bool {
    const TERMINALS: &[&str] = &[
        "kitty",
        "alacritty",
        "foot",
        "footclient",
        "ghostty",
        "wezterm",
        "konsole",
        "xterm",
        "urxvt",
        "gnome-terminal",
        "tilix",
        "terminator",
        "st",
    ];
    TERMINALS
        .iter()
        .any(|t| class == *t || class.ends_with(&format!(".{t}")))
}

/// The paste chord, and only the paste chord. On the uinput path this list
/// governs which keys the virtual device advertises; on the Wayland vk path
/// modifiers are declared as a mask and only KEY_V is actually pressed.
pub fn paste_keys(terminal: bool) -> Vec<KeyCode> {
    let mut chord = vec![KeyCode::KEY_LEFTCTRL];
    if terminal {
        chord.push(KeyCode::KEY_LEFTSHIFT);
    }
    chord.push(KeyCode::KEY_V);
    chord
}

/// Snapshot the clipboard, or give up and return nothing after
/// [`CLIPBOARD_BUDGET`].
///
/// # Why this is not just a function call
///
/// Reading the clipboard means reading a pipe served by whatever application
/// owns the selection, and nothing obliges that application to answer. An app
/// that advertises a mime type and then never writes it - or that has since
/// stopped responding - leaves `read_to_end` blocked with no timeout of its
/// own. This runs on the dictation path while the STT engine lock is held, so
/// one unresponsive clipboard owner froze every subsequent dictation until the
/// daemon was restarted. That was the "recording... and then nothing" bug.
///
/// Losing the ability to restore a clipboard is a small cost. Losing every
/// dictation until a restart is not, so the budget wins and the snapshot is
/// abandoned.
///
/// The worker thread is deliberately left running if it never returns: it is
/// blocked in a kernel read that cannot be interrupted from here, it holds no
/// lock this path needs, and it exits on its own if the peer ever answers.
fn snapshot_clipboard() -> Vec<MimeSource> {
    with_deadline(CLIPBOARD_BUDGET, read_clipboard).unwrap_or_else(|| {
        eprintln!(
            "clipboard did not answer within {CLIPBOARD_BUDGET:?}; \
             pasting without saving what was there"
        );
        Vec::new()
    })
}

/// Run `work` on a throwaway thread and give up on it after `budget`.
///
/// Everything on the dictation path that talks to another process needs this.
/// A clipboard owner that never serves the mime it advertised, or a compositor
/// too busy to answer `hyprctl`, is not an error any of them will report - it
/// is simply a read that never returns, and one of those is enough to freeze
/// every dictation until the daemon restarts.
///
/// The abandoned thread is left alone on purpose: it is blocked in a kernel
/// read that cannot be interrupted from here, it holds no lock this path
/// needs, and it exits by itself if the peer ever answers.
fn with_deadline<T: Send + 'static>(
    budget: Duration,
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        // A send failure only means we already gave up on it.
        let _ = tx.send(work());
    });
    rx.recv_timeout(budget).ok()
}

/// Read every mime the clipboard is currently offering, so the restore after
/// paste can put back an image, files, or anything else - not just text. The
/// old text-only read returned None for an image and silently lost it, leaving
/// the transcript on the clipboard.
///
/// Best effort - an empty clipboard or a read failure on any mime is normal.
/// Always call this through [`snapshot_clipboard`], never directly: on its own
/// it can block forever.
fn read_clipboard() -> Vec<MimeSource> {
    let Ok(mimes) = paste::get_mime_types(ClipboardType::Regular, Seat::Unspecified) else {
        return Vec::new();
    };
    let mut out = Vec::with_capacity(mimes.len());
    for mime in mimes {
        let Ok((mut reader, _)) = paste::get_contents(
            ClipboardType::Regular,
            Seat::Unspecified,
            PasteMime::Specific(&mime),
        ) else {
            continue;
        };
        let mut bytes = Vec::new();
        if std::io::Read::read_to_end(&mut reader, &mut bytes).is_err() {
            continue;
        }
        out.push(MimeSource {
            source: Source::Bytes(bytes.into_boxed_slice()),
            mime_type: CopyMime::Specific(mime),
        });
    }
    out
}
