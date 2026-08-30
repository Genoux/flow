use crate::hotkey;
use crate::wayland_vk::VirtualKeyboard;
use anyhow::{Context, Result};
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, KeyCode, KeyEvent};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;
use wl_clipboard_rs::copy::{self, MimeSource, MimeType as CopyMime, Options, Source};
use wl_clipboard_rs::paste::{self, ClipboardType, MimeType as PasteMime, Seat};

/// uinput devices need a moment for udev to create the node and for compositors
/// to pick them up; emitting immediately after build silently drops events.
const DEVICE_SETTLE: Duration = Duration::from_millis(300);

/// Give the compositor time to observe each key transition. Below ~8ms some
/// clients coalesce the press and release and miss the combo entirely.
const KEY_DELAY: Duration = Duration::from_millis(12);

/// Wait cap for physical modifiers to release before firing the paste chord.
///
/// Only a portable paste with held modifiers waits - see
/// [`compositor_paste`], which optionally avoids that wait where the compositor
/// exposes a suitable capability. Everywhere else the chord is a key event
/// like any other, and a still-held Super rides along with it.
///
/// Measured, not assumed: `tests/paste_live.rs` holds super+shift on a
/// synthetic keyboard and pastes. Through the virtual keyboard the window
/// receives nothing; the client is sent super+shift+ctrl+v, which is not a
/// paste. That is the whole of "it only works when I release every key".
///
/// Long enough to cover a hand coming off a chord at its own pace, since
/// nothing is lost by waiting: the recording is already over and the island is
/// already showing its sweep. 3s was not enough - a deliberate "release only
/// the trigger" hold sat on super+shift well past it.
///
/// If even this runs out the transcript is left on the clipboard rather than
/// restored over, so the dictation survives as a Ctrl+V - see [`Injector::inject`].
const MODIFIER_WAIT: Duration = Duration::from_secs(15);

/// How long the clipboard has to answer before its contents are written off.
/// A responsive owner answers in single-digit milliseconds; this is generous
/// enough that a real image round-trips, and short enough that an unresponsive
/// one is barely felt.
const CLIPBOARD_BUDGET: Duration = Duration::from_millis(500);

/// How long the compositor has to say which window has focus. Only decides
/// which paste chord to send, so falling back to the configured default costs
/// far less than waiting.
const FOCUS_BUDGET: Duration = Duration::from_millis(200);

/// How long to leave the transcript on the clipboard before putting back
/// whatever was there before, once the paste chord has been sent.
///
/// `wl_clipboard_rs::copy` hands the data over on request, in the background,
/// with no signal back here when that request actually happens - the target
/// app has to receive the keystroke, decide to paste, and ask the compositor
/// for the data, all of which takes real time and gets slower under load.
/// Restoring too soon wins that race: the old clipboard content replaces the
/// transcript before the app ever reads it, so the paste keystroke fires,
/// nothing appears, and this code reports success because sending the
/// keystroke is all it actually checked.
///
/// Measured on this machine, from the journal: a dictation whose clipboard
/// snapshot was empty (no restore at all) pasted fine, while the very next one
/// with a restore did not - and a clipboard watch caught the transcript being
/// overwritten 424ms after it landed, with nothing pasted. So 48ms
/// (`KEY_DELAY * 4`, the original) and 400ms both lose this race. Seconds,
/// not milliseconds, is the right order of magnitude for "an app got round to
/// reading the clipboard", and since the wait no longer blocks anything (see
/// below) there is nothing to trade off against.
const CLIPBOARD_RESTORE_DELAY: Duration = Duration::from_secs(3);

/// Which injection currently owns the clipboard. A restore only fires if its
/// own dictation is still the newest: without this, dictating again inside
/// [`CLIPBOARD_RESTORE_DELAY`] would let the previous restore wake up and
/// clobber the *new* transcript - the same race this delay exists to avoid,
/// just with a different loser.
static CLIPBOARD_GENERATION: AtomicUsize = AtomicUsize::new(0);

/// Two ways to deliver the paste chord to the focused window.
///
/// The Wayland virtual-keyboard protocol (`zwp_virtual_keyboard_v1`) is the
/// preferred path. Uinput remains as the fallback for X11 or the handful of
/// Wayland compositors that don't implement the protocol. Both are portable
/// keyboard backends, so both need the physical modifiers to be clear before
/// sending a paste chord.
enum Backend {
    Wayland(VirtualKeyboard),
    Uinput(VirtualDevice),
}

pub struct Injector {
    backend: Backend,
}

#[derive(Debug, PartialEq, Eq)]
enum InitialPasteRoute {
    Portable,
    CompositorRescue,
}

/// Pick a route before emitting anything. A keyboard backend cannot tell us
/// whether its events reached the focused client, so delivery is not something
/// that can be tried portably and retried through a compositor afterward.
fn initial_paste_route(modifiers_clear: bool) -> InitialPasteRoute {
    if modifiers_clear {
        InitialPasteRoute::Portable
    } else {
        InitialPasteRoute::CompositorRescue
    }
}

impl Injector {
    /// Built once and kept alive - rebuilding per injection would pay the
    /// settle delay every time.
    ///
    /// Prefers the Wayland virtual-keyboard protocol and falls back to a uinput
    /// device when the compositor doesn't advertise it (or there is no Wayland
    /// session, e.g. X11). Delivery waits for physical modifiers on either
    /// backend; choosing Wayland does not make seat-wide modifiers disappear.
    pub fn new() -> Result<Self> {
        match VirtualKeyboard::new(KEY_DELAY) {
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

    /// Which backend `new` settled on. The fallback is silent by design, so
    /// this is the only way a caller can tell that the Wayland path was lost.
    pub fn backend(&self) -> &'static str {
        match self.backend {
            Backend::Wayland(_) => "wayland",
            Backend::Uinput(_) => "uinput",
        }
    }

    /// Put `text` in the focused window by staging it on the clipboard and
    /// sending one paste chord.
    ///
    /// Flow's own keyboard is always the default. If the user's physical
    /// modifiers are still down, an optional compositor route may deliver the
    /// chord without inheriting them - see [`compositor_paste`]. Without one,
    /// Flow waits for the board to clear and uses its own keyboard as usual.
    pub fn inject(&mut self, text: &str) -> Result<()> {
        let generation = CLIPBOARD_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        let saved = snapshot_clipboard();

        copy::copy(
            Options::new(),
            Source::Bytes(text.as_bytes().into()),
            CopyMime::Text,
        )
        .context("staging text on the clipboard")?;

        let terminal = detect_terminal_focus().unwrap_or(UNKNOWN_IS_NOT_A_TERMINAL);
        let chord = paste_keys(terminal);

        // Sending first and falling back is impossible: a virtual keyboard can
        // report that it emitted events, but not whether the focused client
        // accepted them. Choose the portable path up front whenever its one
        // precondition is already satisfied.
        let mut modifiers_clear = hotkey::wait_for_modifiers_released(Duration::ZERO);
        match initial_paste_route(modifiers_clear) {
            InitialPasteRoute::Portable => self.paste(&chord)?,
            InitialPasteRoute::CompositorRescue if compositor_paste(&chord) => {
                // The compositor declared the shortcut's modifier mask
                // explicitly, so physical modifiers do not affect delivery.
                modifiers_clear = true;
            }
            InitialPasteRoute::CompositorRescue => {
                modifiers_clear = hotkey::wait_for_modifiers_released(MODIFIER_WAIT);
                if !modifiers_clear {
                    let stuck: Vec<String> = hotkey::currently_held_modifiers()
                        .iter()
                        .map(|key| format!("{key:?}"))
                        .collect();
                    // Nothing has failed and nothing will return an error: the chord
                    // fires, the compositor eats it, and the user watches an empty text
                    // field. The text is already staged, so say where it went.
                    crate::notify::failure(
                        "Flow couldn't paste",
                        "Your text is on the clipboard - press Ctrl+V.",
                    );
                    eprintln!(
                        "paste: firing after {MODIFIER_WAIT:?} with {} still held - the chord will \
                         probably be eaten, so the text is staying on the clipboard for a manual Ctrl+V",
                        if stuck.is_empty() {
                            "no observed modifier".into()
                        } else {
                            stuck.join(", ")
                        },
                    );
                }
                self.paste(&chord)?;
            }
        }

        // On its own thread, so the dictation is finished the moment the chord
        // is sent. Waiting here would put the whole delay on the critical path
        // - it is why a paste that takes 50ms of real work was reported as
        // 454ms - and would hold up the island and the next dictation for no
        // reason: nothing after this point depends on the old clipboard coming
        // back.
        //
        // Skipped entirely when the modifiers never came up: that paste is the
        // one most likely to have been eaten, and putting the old clipboard
        // back over it would turn a dictation the user can still rescue with
        // Ctrl+V into one that is simply gone.
        if !saved.is_empty() && modifiers_clear {
            std::thread::spawn(move || {
                std::thread::sleep(CLIPBOARD_RESTORE_DELAY);
                if CLIPBOARD_GENERATION.load(Ordering::SeqCst) == generation {
                    let _ = copy::copy_multi(Options::new(), saved);
                }
            });
        }
        Ok(())
    }

    fn paste(&mut self, chord: &[KeyCode]) -> Result<()> {
        match &mut self.backend {
            Backend::Wayland(kb) => kb.send_combo(chord).context("wayland vk send_combo"),
            Backend::Uinput(device) => {
                for key in chord {
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

/// What to send when no compositor answers and the focused window is a mystery.
///
/// Plain Ctrl+V, because the two guesses fail differently. Ctrl+V in a terminal
/// does nothing and the text is still on the clipboard. Ctrl+Shift+V outside one
/// is paste-as-plain-text in a browser but Markdown preview in VS Code and Paste
/// Special in LibreOffice - it does not fail quietly, it does something else.
///
/// This used to be a setting. It asked the user a question nobody can answer:
/// the right value depends on which window has focus at the instant of pasting,
/// which changes many times a minute, so a value fixed in a config file was
/// always wrong for something. Detection is the answer; this is only what to do
/// when every detector is silent.
const UNKNOWN_IS_NOT_A_TERMINAL: bool = false;

/// Ask the compositor what has focus and decide whether it needs the terminal
/// chord. Answered per injection, because the machine has a browser and a
/// terminal open at the same time.
///
/// Returns `None` when no compositor tool is installed or the window has no
/// readable class (e.g. a Wayland client that never set `app_id`).
pub fn detect_terminal_focus() -> Option<bool> {
    // Bounded: `.output()` waits for the child forever, and a compositor busy
    // enough not to answer would otherwise take the dictation down with it.
    let class = with_deadline(FOCUS_BUDGET, focused_window_class)??;
    Some(is_terminal_class(&class))
}

/// Ask whichever compositor is running which window has focus.
///
/// There is no Wayland protocol for this - a client is not allowed to know
/// what else is on screen - so the only way is to ask the compositor through
/// its own control socket. Each one has a different answer, so each is tried
/// in turn and the first that responds wins. Wanting the paste chord right on
/// Sway and niri is not optional: those are the same audience as Hyprland, and
/// hyprctl alone silently gave every one of them the wrong chord.
fn focused_window_class() -> Option<String> {
    let candidates: [(&str, &[&str]); 4] = [
        ("hyprctl", &["activewindow", "-j"]),
        ("swaymsg", &["-t", "get_tree", "-r"]),
        ("niri", &["msg", "--json", "focused-window"]),
        // Sends JSON on stdout for the focused toplevel on wlroots setups that
        // ship it; harmless where it is absent.
        ("lswt", &["--json"]),
    ];

    for (program, args) in candidates {
        let Ok(output) = std::process::Command::new(program).args(args).output() else {
            continue; // not installed - almost certainly not this compositor
        };
        if !output.status.success() {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&output.stdout) else {
            continue;
        };
        if let Some(class) = parse_focused_class(text) {
            return Some(class);
        }
    }
    None
}

/// Pull the focused window's class out of a compositor's JSON.
///
/// Deliberately key-based rather than schema-based: Hyprland calls it `class`,
/// Sway `app_id` (with `class` for XWayland), niri `app_id`. Rather than three
/// parsers that each rot separately, take the first of those keys belonging to
/// a focused window. Pure, so the shapes are testable without a compositor.
pub fn parse_focused_class(text: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(text).ok()?;
    let node = focused_node(&value)?;
    for key in ["app_id", "class", "initialClass"] {
        if let Some(name) = node.get(key).and_then(|v| v.as_str()) {
            let name = name.trim().to_ascii_lowercase();
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

/// The object describing the focused window. Hyprland and niri return it
/// directly; Sway returns a whole tree in which one node has `focused: true`.
fn focused_node(value: &serde_json::Value) -> Option<&serde_json::Value> {
    if value.get("focused").and_then(|f| f.as_bool()) == Some(true) {
        return Some(value);
    }
    // A bare object with a class and no `focused` key is Hyprland's or niri's
    // answer: it only ever describes the focused window.
    if value.is_object()
        && value.get("focused").is_none()
        && ["app_id", "class"].iter().any(|k| value.get(k).is_some())
    {
        return Some(value);
    }
    match value {
        serde_json::Value::Array(items) => items.iter().find_map(focused_node),
        serde_json::Value::Object(fields) => fields.values().find_map(focused_node),
        _ => None,
    }
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
/// Try compositor capabilities that can deliver a shortcut with an explicit
/// modifier mask. Absent or unsupported, the caller waits for physical
/// modifiers and returns to the portable keyboard backend.
fn compositor_paste(chord: &[KeyCode]) -> bool {
    hyprland_paste(chord)
}

/// Ask Hyprland to rescue a paste that Flow's own keyboard cannot safely send
/// while physical modifiers are held, and say whether it did.
///
/// # Why the compositor and not Flow's own keyboard
///
/// Because Hyprland merges the modifiers of every keyboard on the seat before
/// it hands a key event on. A chord sent from Flow's virtual keyboard while the
/// user still holds super+shift arrives at the client as super+shift+ctrl+v,
/// which no application treats as a paste, so the dictation lands nowhere. That
/// is why [`MODIFIER_WAIT`] exists at all: the fallback path genuinely has to
/// wait for the hand to leave the keys.
///
/// `send_shortcut` declares the mask for the event instead of inheriting it, so
/// the client is sent ctrl+v no matter what is physically down. Measured both
/// ways in `tests/paste_live.rs`, with super+shift held throughout: the virtual
/// keyboard delivers nothing, this delivers the text.
///
/// This is never the default delivery path. It is spoken over Hyprland's IPC
/// socket rather than by running `hyprctl`, which keeps a paste to one socket
/// write and does not require the binary to be installed. Absent or unhappy,
/// the caller waits and uses the portable keyboard backend.
fn hyprland_paste(chord: &[KeyCode]) -> bool {
    let Ok(runtime) = std::env::var("XDG_RUNTIME_DIR") else {
        return false;
    };
    let Ok(signature) = std::env::var("HYPRLAND_INSTANCE_SIGNATURE") else {
        return false;
    };

    let mods: Vec<&str> = chord
        .iter()
        .filter_map(|key| match *key {
            KeyCode::KEY_LEFTCTRL | KeyCode::KEY_RIGHTCTRL => Some("CTRL"),
            KeyCode::KEY_LEFTSHIFT | KeyCode::KEY_RIGHTSHIFT => Some("SHIFT"),
            _ => None,
        })
        .collect();
    // The one key that is not a modifier. Flow only ever pastes, so there is
    // exactly one, and a chord shaped otherwise is not ours to send this way.
    let [key] = chord
        .iter()
        .filter(|key| !MODIFIER_KEYS.contains(key))
        .collect::<Vec<_>>()[..]
    else {
        return false;
    };
    let Some(name) = key_name(*key) else {
        return false;
    };

    let request = format!(
        r#"dispatch hl.dsp.send_shortcut{{mods="{}",key="{name}"}}"#,
        mods.join(" ")
    );

    let socket = std::path::Path::new(&runtime)
        .join("hypr")
        .join(signature)
        .join(".socket.sock");
    let Ok(mut stream) = std::os::unix::net::UnixStream::connect(socket) else {
        return false;
    };
    let _ = stream.set_read_timeout(Some(HYPRLAND_REPLY));
    if std::io::Write::write_all(&mut stream, request.as_bytes()).is_err() {
        return false;
    }

    // Hyprland answers "ok", or a line beginning "error"/"warning" - notably
    // "window not found", which is a real miss and must fall back rather than
    // silently drop the dictation.
    let mut reply = String::new();
    if std::io::Read::read_to_string(&mut stream, &mut reply).is_err() {
        return false;
    }
    if reply.trim() != "ok" {
        eprintln!(
            "paste: hyprland refused the shortcut ({}), falling back",
            reply.trim()
        );
        return false;
    }
    crate::verbose!("paste: sent by hyprland with modifiers held");
    true
}

/// The name `send_shortcut` knows a key by. Only the keys Flow can paste with,
/// for the same reason the keymap it uploads has three keys in it.
fn key_name(key: KeyCode) -> Option<&'static str> {
    match key {
        KeyCode::KEY_V => Some("V"),
        _ => None,
    }
}

/// Modifiers a paste chord can contain, as opposed to the key it presses.
const MODIFIER_KEYS: [KeyCode; 4] = [
    KeyCode::KEY_LEFTCTRL,
    KeyCode::KEY_RIGHTCTRL,
    KeyCode::KEY_LEFTSHIFT,
    KeyCode::KEY_RIGHTSHIFT,
];

/// Hyprland answers a dispatch immediately or not at all; this only stops a
/// wedged compositor from holding the dictation up.
const HYPRLAND_REPLY: Duration = Duration::from_millis(200);

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

#[cfg(test)]
mod tests {
    use super::{InitialPasteRoute, initial_paste_route};

    #[test]
    fn the_portable_route_is_the_default() {
        assert_eq!(
            initial_paste_route(true),
            InitialPasteRoute::Portable,
            "a clear keyboard must not consult a compositor-specific path",
        );
        assert_eq!(
            initial_paste_route(false),
            InitialPasteRoute::CompositorRescue,
            "held modifiers make portable delivery unsafe until they lift",
        );
    }
}
