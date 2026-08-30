//! The bar icon that says Flow is still listening after its window is closed.
//!
//! The daemon carries this and not the console, for the reason the icon exists
//! at all: the console is the part that goes away. Closing it has never stopped
//! dictation - the daemon is a separate process and a separate systemd unit -
//! but nothing on screen said so, so the only way to check was to open the
//! window again or read `systemctl --user status flow.service`.
//!
//! Wayland has no tray. What exists is StatusNotifierItem over D-Bus, and an
//! icon only appears if the bar runs a host for it (waybar's `tray` module,
//! Quickshell's `SystemTray`). On a desktop with no host this registers and is
//! never drawn, which is why every failure here is logged and swallowed: a
//! dictation tool that refused to start because a bar was missing would be
//! trading the whole product for its status light.

use ksni::blocking::TrayMethods;
use ksni::menu::StandardItem;
use ksni::{Icon, MenuItem, ToolTip, Tray};
use std::sync::LazyLock;

/// A 64px copy of the launcher icon rather than the installed 512px one.
///
/// Sent inline over D-Bus to every host that connects and drawn at bar height,
/// so the large one would be a quarter-megabyte of pixels to produce a 20px
/// square. Regenerate with:
///
/// ```sh
/// magick packaging/flow-console.png -resize 64x64 -strip PNG32:assets/tray.png
/// ```
const ICON: &[u8] = include_bytes!("../assets/tray.png");

/// Decoded once. Hosts re-read the property on reconnect, and the answer never
/// changes.
static PIXMAP: LazyLock<Vec<Icon>> = LazyLock::new(|| vec![argb(ICON)]);

/// Registers the item and leaves it running for the life of the daemon.
///
/// The returned handle is only worth holding on to: dropping it takes the icon
/// down with it.
pub fn spawn() -> Option<ksni::blocking::Handle<Flow>> {
    // The daemon starts with `graphical-session.target`, which can easily beat
    // the bar to it - and a watcher that is not up yet is not the same as a
    // desktop that will never have one. Without this, starting half a second
    // early is a permanent missing icon rather than one that appears when the
    // bar does.
    match Flow.assume_sni_available(true).spawn() {
        Ok(handle) => Some(handle),
        Err(err) => {
            eprintln!("tray icon unavailable ({err}); the daemon is unaffected");
            None
        }
    }
}

pub struct Flow;

impl Tray for Flow {
    fn id(&self) -> String {
        "flow".into()
    }

    fn title(&self) -> String {
        "Flow".into()
    }

    fn icon_pixmap(&self) -> Vec<Icon> {
        PIXMAP.clone()
    }

    fn tool_tip(&self) -> ToolTip {
        ToolTip {
            title: "Flow".into(),
            description: "Push-to-talk dictation. Hold your chord and speak.".into(),
            ..Default::default()
        }
    }

    fn activate(&mut self, _x: i32, _y: i32) {
        open_console();
    }

    fn menu(&self) -> Vec<MenuItem<Self>> {
        vec![
            StandardItem {
                label: "Open Flow".into(),
                activate: Box::new(|_: &mut Self| open_console()),
                ..Default::default()
            }
            .into(),
            MenuItem::Separator,
            StandardItem {
                label: "Quit Flow".into(),
                activate: Box::new(|_: &mut Self| quit()),
                ..Default::default()
            }
            .into(),
        ]
    }
}

/// Opens the console window, unless one is already open.
///
/// Nothing here can raise an existing window - a Wayland client cannot focus
/// itself and the compositor's way of doing it differs per compositor - so the
/// choice is between a second window and no visible response to the click. A
/// duplicate window is the worse of the two: it is a second copy of a page that
/// edits config files.
fn open_console() {
    if running("flow-console") {
        return;
    }
    // Resolved from PATH first so a system install and a ~/.local/bin one both
    // work, with the usual location as the fallback: a systemd user unit does
    // not always inherit a PATH that carries ~/.local/bin.
    let home = std::env::var("HOME").unwrap_or_default();
    let fallback = format!("{home}/.local/bin/flow-console");
    for program in ["flow-console", fallback.as_str()] {
        let Ok(mut console) = std::process::Command::new(program).spawn() else {
            continue;
        };
        // Waited on its own thread, the way `chime` waits on paplay. A child
        // nobody reaps stays in the process table under its own name after the
        // window closes, so `running` above went on answering yes for the life
        // of the daemon and every click after the first one did nothing.
        std::thread::spawn(move || {
            let _ = console.wait();
        });
        return;
    }
    eprintln!("tray: flow-console is not on PATH or in ~/.local/bin");
}

/// Stops dictation and closes the window, until Flow is started again.
///
/// The console goes with it because the console is Flow's only window: quitting
/// and leaving it on screen would say the app was gone while the app was still
/// sitting there. `SIGTERM` rather than anything gentler, which is what the
/// console itself already sends the daemon from its own Stop control.
///
/// The daemon then leaves by a clean exit, on purpose: the unit is
/// `Restart=on-failure`, so going out the front door stays gone, while anything
/// systemd reads as a crash would bring it straight back and make this menu
/// item look broken.
fn quit() {
    for pid in pids_of("flow-console") {
        // SAFETY: no memory is involved. The worst a stale pid can do is fail
        // with ESRCH, which is why the result is dropped rather than checked.
        unsafe { libc::kill(pid, libc::SIGTERM) };
    }
    crate::ipc::remove_pid();
    crate::status::remove_socket();
    std::process::exit(0);
}

/// Whether a window of this program is open, read from `/proc` rather than asked
/// of `pgrep`, which is not installed everywhere.
///
/// Unreaped children are not running. Whoever launched the console decides
/// whether it is waited on - this module does, a shell does, a launcher may
/// not - so a name in `/proc` is not on its own an open window.
fn running(name: &str) -> bool {
    !pids_of(name).is_empty()
}

/// Every live process of this program.
fn pids_of(name: &str) -> Vec<libc::pid_t> {
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let process = entry.path();
            let pid = process.file_name()?.to_str()?.parse().ok()?;
            let named =
                std::fs::read_to_string(process.join("comm")).is_ok_and(|comm| comm.trim() == name);
            let waiting =
                std::fs::read_to_string(process.join("stat")).is_ok_and(|stat| zombie(&stat));
            (named && !waiting).then_some(pid)
        })
        .collect()
}

/// Whether a `/proc/<pid>/stat` line describes a process waiting to be reaped.
///
/// Read from after the last `)` rather than by splitting on spaces: the second
/// field is the executable name in brackets, and a program free to have a space
/// in its name is free to move every field after it.
fn zombie(stat: &str) -> bool {
    stat.rsplit_once(')')
        .is_some_and(|(_, rest)| rest.trim_start().starts_with('Z'))
}

/// PNG bytes to the ARGB32 the tray protocol asks for.
///
/// Panics rather than degrades, and only ever on the icon compiled into this
/// binary: there is no input here that a running machine can get wrong, so a
/// failure is a broken build and is covered by a test.
fn argb(png: &[u8]) -> Icon {
    // Cursor rather than the slice itself: the decoder seeks, and `&[u8]` reads
    // forwards only.
    let mut reader = png::Decoder::new(std::io::Cursor::new(png))
        .read_info()
        .expect("tray png header");
    let mut data = vec![0; reader.output_buffer_size().expect("tray png size")];
    let info = reader.next_frame(&mut data).expect("tray png pixels");
    assert_eq!(
        info.color_type,
        png::ColorType::Rgba,
        "the tray icon must be RGBA - regenerate it with PNG32:"
    );
    data.truncate(info.buffer_size());
    // The two formats hold the same four bytes in a different order, so the
    // conversion is a rotation per pixel rather than a copy.
    for pixel in data.chunks_exact_mut(4) {
        pixel.rotate_right(1);
    }
    Icon {
        width: info.width as i32,
        height: info.height as i32,
        data,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The icon is an asset, so this fails at build time or never - which is
    /// the point: `argb` panics on a bad one rather than shipping a daemon that
    /// dies the first time a bar connects.
    #[test]
    fn the_tray_icon_decodes_to_argb() {
        let icon = argb(ICON);
        assert_eq!(icon.width, 64);
        assert_eq!(icon.height, 64);
        assert_eq!(icon.data.len(), 64 * 64 * 4);
        // Opaque somewhere: an all-zero alpha channel decodes and draws nothing,
        // which is the one broken icon that would otherwise look like a missing
        // tray host.
        assert!(
            icon.data.chunks_exact(4).any(|pixel| pixel[0] > 0),
            "every pixel is fully transparent"
        );
    }

    /// The reported failure, in the one line that caused it: a closed console
    /// left a zombie named `flow-console`, `running` counted it as an open
    /// window, and the tray icon went dead for the rest of the session.
    #[test]
    fn an_unreaped_console_does_not_count_as_running() {
        let dead = "2885662 (flow-console) Z 2874041 2874041 0 0 -1 4194560 0 0";
        let alive = "2874041 (flow) S 2039268 2874041 0 0 -1 4194304 0 0";
        assert!(zombie(dead));
        assert!(!zombie(alive));
    }

    /// A name with a space in it moves every field after it, which is why the
    /// state is read from the end of the brackets rather than by counting
    /// columns.
    #[test]
    fn a_process_name_with_a_space_does_not_shift_the_state() {
        assert!(zombie("42 (some name) Z 1 42 0"));
        assert!(!zombie("42 (some name) R 1 42 0"));
    }
}
