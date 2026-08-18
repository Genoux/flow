use crate::hotkey;
use anyhow::{Context, Result};
use evdev::uinput::VirtualDevice;
use evdev::{AttributeSet, KeyCode, KeyEvent};
use std::sync::atomic::{AtomicUsize, Ordering};
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

/// Wait cap for physical modifiers to release before firing the paste chord.
///
/// Applies to **both** backends. It used to be skipped on the Wayland vk path,
/// on the theory that declaring the modifier mask atomically through the
/// virtual-keyboard protocol stops the compositor conflating our Ctrl with a
/// still-held physical Super. Measured on Hyprland, that is not true: the
/// chord and the physical modifiers still meet somewhere before the focused
/// client sees them.
///
/// This is the whole of "it only works when I release every key at once".
/// Releasing just the trigger of a super+shift+d hold ends the recording
/// correctly - the transcript is right, the paste fires, the log says success -
/// but the client receives super+shift+ctrl+v, which is not a paste, so
/// nothing lands. Two dictations seconds apart, one released fully and one
/// released by the trigger alone, differed in nothing else.
///
/// Long enough to cover a hand coming off a chord at its own pace, since
/// nothing is lost by waiting: the recording is already over, the island is
/// already showing its sweep, and a paste that fires into held modifiers is a
/// dictation thrown away. 3s was not enough - a deliberate "release only the
/// trigger" hold sat on super+shift well past it, timed out, fired, and was
/// eaten exactly as the warning predicted.
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
    /// Both backends wait for the user's own modifiers to come up first - see
    /// [`MODIFIER_WAIT`] for why the Wayland path is no exception.
    pub fn inject(&mut self, text: &str, terminal_hint: bool) -> Result<()> {
        let generation = CLIPBOARD_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        let saved = snapshot_clipboard();

        copy::copy(
            Options::new(),
            Source::Bytes(text.as_bytes().into()),
            CopyMime::Text,
        )
        .context("staging text on the clipboard")?;

        let modifiers_clear = hotkey::wait_for_modifiers_released(MODIFIER_WAIT);
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
                "A held key swallowed the paste. Your text is on the clipboard \
                 - press Ctrl+V.",
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

        let terminal = detect_terminal_focus().unwrap_or(terminal_hint);
        self.paste(terminal)?;

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
