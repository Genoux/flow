//! Focused Wayland client that records every keystroke it receives, for the
//! virtual-keyboard stress harness.
//!
//! This is **only** a test fixture. It maps an `xdg_toplevel`, waits for the
//! compositor to give it keyboard focus, then decodes every `wl_keyboard.key`
//! press through the compositor-uploaded keymap (the same keymap the
//! `WaylandVkKeyboard` backend uploads) into the resulting Unicode text — the
//! exact text a real focused application would have received.
//!
//! It speaks a tiny line protocol on **stdout** so the driver
//! (`vk_stress` example) can synchronise with it:
//!
//! * `READY` — printed once the toplevel has keyboard focus and is ready to
//!   receive injected keys.
//! * `TEXT <json-string>` — printed in response to a `DUMP` line on **stdin**;
//!   the JSON string is the accumulated typed text since the last `RESET`.
//! * `OK` — printed in response to a `RESET` line on stdin (clears the buffer).
//!
//! The driver writes `RESET`, drives the injector, writes `DUMP`, and reads the
//! `TEXT` line back. Running both in one process would deadlock the single
//! compositor connection, so they are separate processes sharing the headless
//! display.
//!
//! SAFETY: this never injects anything. It only *receives*. It is spawned by
//! the harness against an isolated headless compositor.

use std::io::{BufRead, Write};
use std::os::fd::AsFd;

use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::wl_buffer::WlBuffer;
use wayland_client::protocol::wl_compositor::WlCompositor;
use wayland_client::protocol::wl_keyboard::{self, WlKeyboard};
use wayland_client::protocol::wl_registry::WlRegistry;
use wayland_client::protocol::wl_seat::{self, WlSeat};
use wayland_client::protocol::wl_shm::{self, WlShm};
use wayland_client::protocol::wl_shm_pool::WlShmPool;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::xdg::shell::client::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::client::xdg_toplevel::{self, XdgToplevel};
use wayland_protocols::xdg::shell::client::xdg_wm_base::{self, XdgWmBase};
use xkbcommon::xkb;

/// Shared, captured state of the recorder client.
struct Recorder {
    /// xkb context for compiling the compositor-supplied keymap.
    xkb_ctx: xkb::Context,
    /// Active xkb state built from the latest uploaded keymap. `None` until the
    /// first `wl_keyboard.keymap` arrives.
    xkb_state: Option<xkb::State>,
    /// Accumulated decoded text from every key press received so far.
    typed: String,
    /// Number of distinct keymaps the compositor has pushed to us (one per
    /// backend re-upload). Useful as a re-upload counter in the harness.
    keymap_count: u32,
}

impl Recorder {
    fn new() -> Self {
        Self {
            xkb_ctx: xkb::Context::new(xkb::CONTEXT_NO_FLAGS),
            xkb_state: None,
            typed: String::new(),
            keymap_count: 0,
        }
    }
}

/// Create a 1x1 ARGB8888 shm buffer (the minimum needed to map a surface).
fn create_shm_buffer(
    shm: &WlShm,
    qh: &QueueHandle<AppData>,
) -> anyhow::Result<(WlShmPool, WlBuffer)> {
    use rustix::fs::{memfd_create, MemfdFlags};
    use std::io::Write as _;

    let stride = 4i32; // 1px * 4 bytes (ARGB8888)
    let size = stride; // 1 row
    let fd = memfd_create("whisrs-vk-recorder-shm", MemfdFlags::CLOEXEC)?;
    let mut file = std::fs::File::from(fd);
    file.write_all(&[0u8; 4])?;
    file.flush().ok();
    let fd: std::os::fd::OwnedFd = file.into();

    let pool = shm.create_pool(fd.as_fd(), size, qh, ());
    let buffer = pool.create_buffer(0, 1, 1, stride, wl_shm::Format::Argb8888, qh, ());
    Ok((pool, buffer))
}

struct AppData {
    rec: Recorder,
    /// Set true once the toplevel has been configured (mapped) — the signal we
    /// use to emit `READY` to the driver. Keyboard focus comes later, once the
    /// injector attaches its virtual keyboard to the seat. Single-threaded, so a
    /// plain bool suffices (the Wayland event queue is pumped on this thread).
    configured: bool,
    qh: QueueHandle<AppData>,
    surface: WlSurface,
    buffer: WlBuffer,
    /// Kept alive so the protocol objects aren't dropped mid-run. Never read.
    _wm_base: Option<XdgWmBase>,
    /// The keyboard proxy, acquired lazily once the seat advertises the
    /// keyboard capability (which only happens after the injector's virtual
    /// keyboard is attached to the seat).
    keyboard: Option<WlKeyboard>,
}

fn main() -> anyhow::Result<()> {
    // SAFETY GUARD: refuse to run inside the user's real session runtime dir.
    // The harness always points us at an isolated XDG_RUNTIME_DIR under /tmp.
    let xrd = std::env::var("XDG_RUNTIME_DIR").unwrap_or_default();
    if xrd == "/run/user/1000" || xrd.starts_with("/run/user/") {
        eprintln!(
            "vk_recorder: refusing to run against the real runtime dir {xrd:?}; \
             the harness must use an isolated XDG_RUNTIME_DIR under /tmp"
        );
        std::process::exit(3);
    }

    let conn = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<AppData>(&conn)?;
    let qh = queue.handle();

    let compositor: WlCompositor = globals.bind(&qh, 1..=6, ())?;
    let wm_base: XdgWmBase = globals.bind(&qh, 1..=6, ())?;
    // Bound and kept alive so its `capabilities` event routes to our handler
    // (which lazily acquires the keyboard once the injector adds one).
    let _seat: WlSeat = globals.bind(&qh, 1..=8, ())?;
    let shm: WlShm = globals.bind(&qh, 1..=1, ())?;

    // A 1x1 ARGB8888 shm buffer so the surface can actually map (a surface with
    // no attached buffer never maps and never receives keyboard focus).
    let (pool, buffer) = create_shm_buffer(&shm, &qh)?;

    // Create a surface + xdg_toplevel.
    let surface: WlSurface = compositor.create_surface(&qh, ());
    let xdg_surface: XdgSurface = wm_base.get_xdg_surface(&surface, &qh, ());
    let toplevel: XdgToplevel = xdg_surface.get_toplevel(&qh, ());
    toplevel.set_title("whisrs-vk-recorder".to_string());
    toplevel.set_app_id("whisrs-vk-recorder".to_string());
    // Initial commit (no buffer) triggers the compositor's first configure.
    surface.commit();

    let mut app = AppData {
        rec: Recorder::new(),
        configured: false,
        qh: qh.clone(),
        surface: surface.clone(),
        buffer: buffer.clone(),
        _wm_base: Some(wm_base.clone()),
        keyboard: None,
    };
    // Keep the pool alive for the lifetime of the process.
    std::mem::forget(pool);

    // NOTE: in a pure headless wlroots backend the seat has no keyboard
    // capability until the injector attaches its `zwp_virtual_keyboard_v1`. We
    // therefore do NOT eagerly `get_keyboard` here (that is a protocol error
    // while no keyboard exists); the seat `capabilities` handler acquires it
    // once the capability appears.

    // Pump events until the toplevel is configured (mapped). The xdg_surface
    // Configure handler attaches the buffer and commits, completing the map.
    let start = std::time::Instant::now();
    while !app.configured {
        queue.blocking_dispatch(&mut app)?;
        if start.elapsed() > std::time::Duration::from_secs(15) {
            anyhow::bail!("vk_recorder: timed out waiting for toplevel configure");
        }
    }

    // Tell the driver we're ready (mapped). The driver now starts the injector,
    // which attaches a virtual keyboard to the seat → we acquire the keyboard
    // and receive focus. Subsequent RESET/DUMP commands dispatch events.
    println!("READY");
    std::io::stdout().flush().ok();

    let stdin = std::io::stdin();
    let mut lines = stdin.lock().lines();

    loop {
        // Drain any pending Wayland events without blocking forever: do a
        // bounded blocking dispatch so key events are decoded promptly.
        queue.flush().ok();
        // Read one command line (blocks on stdin). Between commands the driver
        // has injected keys; we must dispatch to decode them. We therefore
        // dispatch *before* responding to DUMP.
        let Some(line) = lines.next() else {
            break; // stdin closed -> shut down
        };
        let line = line?;
        match line.trim() {
            "RESET" => {
                // Process any pending events first (e.g. the keyboard.enter and
                // initial keymap that arrive after the injector connects), so a
                // stray keymap upload doesn't bleed into the next measurement.
                queue.roundtrip(&mut app)?;
                app.rec.typed.clear();
                println!("OK");
                std::io::stdout().flush().ok();
            }
            "DUMP" => {
                // Make sure every queued key event is decoded before dumping.
                // A roundtrip guarantees the server has processed all our prior
                // requests and flushed pending events to us.
                queue.roundtrip(&mut app)?;
                let json = serde_json::to_string(&app.rec.typed).unwrap();
                println!("TEXT {json}");
                std::io::stdout().flush().ok();
            }
            "KEYMAPS" => {
                queue.roundtrip(&mut app)?;
                println!("KEYMAPS {}", app.rec.keymap_count);
                std::io::stdout().flush().ok();
            }
            "QUIT" => break,
            "" => {}
            other => {
                eprintln!("vk_recorder: unknown command {other:?}");
            }
        }
    }

    Ok(())
}

// --- Dispatch impls ---

impl Dispatch<WlRegistry, GlobalListContents> for AppData {
    fn event(
        _: &mut Self,
        _: &WlRegistry,
        _: <WlRegistry as Proxy>::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlCompositor, ()> for AppData {
    fn event(
        _: &mut Self,
        _: &WlCompositor,
        _: <WlCompositor as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlShm, ()> for AppData {
    fn event(
        _: &mut Self,
        _: &WlShm,
        _: <WlShm as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlShmPool, ()> for AppData {
    fn event(
        _: &mut Self,
        _: &WlShmPool,
        _: <WlShmPool as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlBuffer, ()> for AppData {
    fn event(
        _: &mut Self,
        _: &WlBuffer,
        _: <WlBuffer as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSurface, ()> for AppData {
    fn event(
        _: &mut Self,
        _: &WlSurface,
        _: <WlSurface as Proxy>::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSeat, ()> for AppData {
    fn event(
        app: &mut Self,
        seat: &WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities { capabilities } = event {
            let has_kbd = match capabilities {
                WEnum::Value(caps) => caps.contains(wl_seat::Capability::Keyboard),
                WEnum::Unknown(bits) => bits & u32::from(wl_seat::Capability::Keyboard) != 0,
            };
            if has_kbd && app.keyboard.is_none() {
                // The capability appeared (the injector attached its virtual
                // keyboard): now it is safe to grab the keyboard.
                app.keyboard = Some(seat.get_keyboard(&app.qh, ()));
            }
        }
    }
}

impl Dispatch<XdgWmBase, ()> for AppData {
    fn event(
        _: &mut Self,
        wm_base: &XdgWmBase,
        event: xdg_wm_base::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm_base.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, ()> for AppData {
    fn event(
        app: &mut Self,
        xdg_surface: &XdgSurface,
        event: xdg_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            xdg_surface.ack_configure(serial);
            // Attach the shm buffer and commit to complete the map. This is the
            // point at which the toplevel becomes a real, focusable window.
            app.surface.attach(Some(&app.buffer), 0, 0);
            app.surface.damage(0, 0, 1, 1);
            app.surface.commit();
            app.configured = true;
        }
    }
}

impl Dispatch<XdgToplevel, ()> for AppData {
    fn event(
        _: &mut Self,
        _: &XdgToplevel,
        _: xdg_toplevel::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlKeyboard, ()> for AppData {
    fn event(
        app: &mut Self,
        _: &WlKeyboard,
        event: wl_keyboard::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let r = &mut app.rec;
        match event {
            wl_keyboard::Event::Keymap {
                format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1),
                fd,
                size,
            } => {
                // Compile the compositor-supplied keymap and build a fresh xkb
                // state from it. `new_from_fd` consumes the OwnedFd and mmaps
                // `size` bytes (reading `size - 1`, accounting for the trailing
                // NUL the backend writes).
                // SAFETY: `fd` is a read-only keymap fd handed to us by the
                // compositor; we transfer ownership to `new_from_fd`.
                let keymap = unsafe {
                    xkb::Keymap::new_from_fd(
                        &r.xkb_ctx,
                        fd,
                        size as usize,
                        xkb::KEYMAP_FORMAT_TEXT_V1,
                        xkb::KEYMAP_COMPILE_NO_FLAGS,
                    )
                };
                match keymap {
                    Ok(Some(km)) => {
                        r.xkb_state = Some(xkb::State::new(&km));
                        r.keymap_count += 1;
                    }
                    _ => eprintln!("vk_recorder: failed to compile uploaded keymap"),
                }
            }
            wl_keyboard::Event::Modifiers {
                mods_depressed,
                mods_latched,
                mods_locked,
                group,
                ..
            } => {
                if let Some(state) = r.xkb_state.as_mut() {
                    state.update_mask(mods_depressed, mods_latched, mods_locked, 0, 0, group);
                }
            }
            wl_keyboard::Event::Key {
                key,
                state: WEnum::Value(wl_keyboard::KeyState::Pressed),
                ..
            } => {
                // Only decode presses; releases would double-count.
                if let Some(xkbstate) = r.xkb_state.as_ref() {
                    // wl_keyboard.key carries an evdev keycode; xkb adds 8.
                    let utf8 = xkbstate.key_get_utf8(xkb::Keycode::new(key + 8));
                    if !utf8.is_empty() {
                        r.typed.push_str(&utf8);
                    }
                }
            }
            _ => {}
        }
    }
}
