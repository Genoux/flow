use super::{
    BUTTON_RADIUS, CANCEL_CENTRE, Canvas, Face, HANDS_FREE_LEFT, HEIGHT, MARGIN_BOTTOM,
    STOP_CENTRE, SURFACE_WIDTH, click_event, render,
};
use crate::hotkey;
use anyhow::{Context, Result, anyhow};
use std::fs::File;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::FileExt;
use std::sync::mpsc::Sender;
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_pointer, wl_region, wl_registry, wl_seat, wl_shm, wl_shm_pool,
    wl_surface,
};
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, WEnum, delegate_noop};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

#[derive(Default)]
struct Wayland {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    seat: Option<wl_seat::WlSeat>,
    pointer: Option<wl_pointer::WlPointer>,
    /// Surface-local logical px, as the compositor last reported it.
    pointer_at: (f64, f64),
    clickable: bool,
    clicks: Option<Sender<hotkey::Event>>,
    scale: i32,
    configured: bool,
    closed: bool,
}

/// BTN_LEFT from linux/input-event-codes.h.
const BUTTON_LEFT: u32 = 0x110;

/// The mapped surface. Dropped the moment recording stops, so nothing of flow
/// is on screen - or composited - between dictations.
struct Island {
    surface: wl_surface::WlSurface,
    layer: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
}

impl Island {
    fn map(
        compositor: &wl_compositor::WlCompositor,
        shell: &zwlr_layer_shell_v1::ZwlrLayerShellV1,
        queue: &QueueHandle<Wayland>,
    ) -> Self {
        let surface = compositor.create_surface(queue, ());
        let layer = shell.get_layer_surface(
            &surface,
            None,
            zwlr_layer_shell_v1::Layer::Overlay,
            "flow".into(),
            queue,
            (),
        );
        layer.set_size(SURFACE_WIDTH, HEIGHT);
        layer.set_anchor(zwlr_layer_surface_v1::Anchor::Bottom);
        layer.set_margin(0, 0, MARGIN_BOTTOM, 0);
        layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);

        let island = Self { surface, layer };
        island.accept_clicks(compositor, queue, false);
        island.surface.commit();
        island
    }

    /// An empty input region makes the island click-through; an indicator that
    /// swallows a click on whatever is underneath is worse than none. Only the
    /// two buttons of a latched recording are ever given to the pointer, so
    /// the rest of the pill still passes clicks to what is behind it.
    fn accept_clicks(
        &self,
        compositor: &wl_compositor::WlCompositor,
        queue: &QueueHandle<Wayland>,
        on: bool,
    ) {
        let region = compositor.create_region(queue, ());
        if on {
            for (x, y) in [CANCEL_CENTRE, STOP_CENTRE] {
                let side = (BUTTON_RADIUS * 2.0) as i32;
                region.add(
                    (HANDS_FREE_LEFT + x - BUTTON_RADIUS) as i32,
                    (y - BUTTON_RADIUS) as i32,
                    side,
                    side,
                );
            }
        }
        self.surface.set_input_region(Some(&region));
        region.destroy();
    }
}

impl Drop for Island {
    fn drop(&mut self) {
        self.layer.destroy();
        self.surface.destroy();
    }
}

/// Two buffers so the compositor is never reading the frame we are painting.
struct Buffers {
    file: File,
    pool: wl_shm_pool::WlShmPool,
    buffers: [wl_buffer::WlBuffer; 2],
    canvas: Canvas,
    next: usize,
    scale: i32,
}

impl Buffers {
    fn create(shm: &wl_shm::WlShm, queue: &QueueHandle<Wayland>, scale: i32) -> Result<Self> {
        let width = SURFACE_WIDTH as i32 * scale;
        let height = HEIGHT as i32 * scale;
        let stride = width * 4;
        let frame = stride * height;

        let file = File::from(shared_memory(frame as usize * 2)?);
        let pool = shm.create_pool(file.as_fd(), frame * 2, queue, ());
        let buffers = [0, 1].map(|slot| {
            pool.create_buffer(
                slot * frame,
                width,
                height,
                stride,
                wl_shm::Format::Argb8888,
                queue,
                (),
            )
        });

        Ok(Self {
            file,
            pool,
            buffers,
            canvas: Canvas::new(width as usize, height as usize),
            next: 0,
            scale,
        })
    }

    fn present(&mut self, surface: &wl_surface::WlSurface, face: Face) -> Result<()> {
        render(&mut self.canvas, face, self.scale as f32);

        let slot = self.next;
        self.next = 1 - self.next;
        self.file
            .write_all_at(
                &self.canvas.pixels,
                (slot * self.canvas.pixels.len()) as u64,
            )
            .context("writing the overlay frame")?;

        surface.attach(Some(&self.buffers[slot]), 0, 0);
        surface.damage_buffer(0, 0, self.canvas.width as i32, self.canvas.height as i32);
        surface.commit();
        Ok(())
    }
}

impl Drop for Buffers {
    fn drop(&mut self) {
        for buffer in &self.buffers {
            buffer.destroy();
        }
        self.pool.destroy();
    }
}

fn shared_memory(size: usize) -> Result<OwnedFd> {
    // SAFETY: a NUL-terminated literal name, and the raw fd is taken into an
    // OwnedFd immediately so it is closed exactly once.
    let raw = unsafe { libc::memfd_create(c"flow-overlay".as_ptr(), libc::MFD_CLOEXEC) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error()).context("memfd_create");
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };

    // SAFETY: sizing a fresh memfd owned by `fd`.
    if unsafe { libc::ftruncate(fd.as_raw_fd(), size as libc::off_t) } != 0 {
        return Err(std::io::Error::last_os_error()).context("sizing the overlay buffer");
    }
    Ok(fd)
}

/// A connected display with everything the island needs off the registry, and
/// the island itself while it is up.
pub struct Surface {
    connection: Connection,
    queue: EventQueue<Wayland>,
    state: Wayland,
    compositor: wl_compositor::WlCompositor,
    shm: wl_shm::WlShm,
    shell: zwlr_layer_shell_v1::ZwlrLayerShellV1,
    island: Option<Island>,
    buffers: Option<Buffers>,
}

impl Surface {
    pub fn connect(clicks: Sender<hotkey::Event>) -> Result<Self> {
        let connection = Connection::connect_to_env().context("no wayland display")?;
        let mut queue = connection.new_event_queue();
        let handle = queue.handle();
        connection.display().get_registry(&handle, ());

        let mut state = Wayland {
            scale: 1,
            clicks: Some(clicks),
            ..Wayland::default()
        };
        queue.roundtrip(&mut state)?;

        let compositor = state
            .compositor
            .clone()
            .ok_or_else(|| anyhow!("compositor does not advertise wl_compositor"))?;
        let shm = state
            .shm
            .clone()
            .ok_or_else(|| anyhow!("compositor does not advertise wl_shm"))?;
        let shell = state
            .layer_shell
            .clone()
            .ok_or_else(|| anyhow!("compositor does not support wlr-layer-shell"))?;

        Ok(Self {
            connection,
            queue,
            state,
            compositor,
            shm,
            shell,
            island: None,
            buffers: None,
        })
    }

    pub fn is_mapped(&self) -> bool {
        self.island.is_some()
    }

    pub fn map(&mut self) {
        self.state.configured = false;
        self.state.closed = false;
        self.state.clickable = false;
        self.island = Some(Island::map(
            &self.compositor,
            &self.shell,
            &self.queue.handle(),
        ));
    }

    /// Hand the buttons to the pointer, or take them back. The region is
    /// double-buffered and lands with the next frame's commit.
    pub fn set_clickable(&mut self, on: bool) {
        if self.state.clickable == on {
            return;
        }
        self.state.clickable = on;
        if let Some(island) = &self.island {
            island.accept_clicks(&self.compositor, &self.queue.handle(), on);
        }
    }

    /// Process whatever the compositor has said since the last call.
    pub fn dispatch(&mut self) -> Result<()> {
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }

    /// The compositor can take the surface away at any point - a monitor
    /// unplugged mid-sentence.
    pub fn is_closed(&self) -> bool {
        self.state.closed
    }

    /// Mapped and told its size, so a frame may be presented.
    pub fn is_ready(&self) -> bool {
        self.island.is_some() && self.state.configured
    }

    /// Forget the surface without telling the compositor, which has already
    /// done away with it.
    pub fn release(&mut self) {
        self.island = None;
    }

    /// Drop the surface and push the destroy out to the compositor.
    ///
    /// The roundtrip is the whole point. Dropping the [`Island`] only queues the
    /// destroy requests, and the next thing the loop does with no island up is
    /// block on `recv()` - so without a flush the surface stays on screen until
    /// the next chord arrives to wake the loop and send them.
    pub fn take_down(&mut self) -> Result<()> {
        self.island = None;
        self.queue.roundtrip(&mut self.state)?;
        Ok(())
    }

    pub fn present(&mut self, face: Face) -> Result<()> {
        let Some(island) = self.island.as_ref() else {
            return Ok(());
        };
        if self
            .buffers
            .as_ref()
            .is_none_or(|held| held.scale != self.state.scale)
        {
            self.buffers = Some(Buffers::create(
                &self.shm,
                &self.queue.handle(),
                self.state.scale,
            )?);
        }
        let Some(buffers) = self.buffers.as_mut() else {
            return Ok(());
        };

        island.surface.set_buffer_scale(self.state.scale);
        buffers.present(&island.surface, face)?;
        self.connection.flush()?;
        Ok(())
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for Wayland {
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
            // Version 6 carries preferred_buffer_scale, which is what keeps the
            // island sharp on a scaled output instead of upscaled and soft.
            "wl_compositor" => {
                state.compositor = Some(registry.bind(name, version.min(6), queue, ()));
            }
            "wl_shm" => state.shm = Some(registry.bind(name, 1, queue, ())),
            "wl_seat" => state.seat = Some(registry.bind(name, 1, queue, ())),
            "zwlr_layer_shell_v1" => {
                state.layer_shell = Some(registry.bind(name, version.min(4), queue, ()));
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for Wayland {
    fn event(
        state: &mut Self,
        _: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_surface::Event::PreferredBufferScale { factor } = event {
            state.scale = factor.max(1);
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for Wayland {
    fn event(
        state: &mut Self,
        layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, .. } => {
                layer.ack_configure(serial);
                state.configured = true;
            }
            zwlr_layer_surface_v1::Event::Closed => state.closed = true,
            _ => {}
        }
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for Wayland {
    fn event(
        state: &mut Self,
        seat: &wl_seat::WlSeat,
        event: wl_seat::Event,
        _: &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        if let wl_seat::Event::Capabilities {
            capabilities: WEnum::Value(capabilities),
        } = event
        {
            let has_pointer = capabilities.contains(wl_seat::Capability::Pointer);
            match (has_pointer, state.pointer.is_some()) {
                (true, false) => state.pointer = Some(seat.get_pointer(queue, ())),
                (false, true) => state.pointer = None,
                _ => {}
            }
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for Wayland {
    fn event(
        state: &mut Self,
        _: &wl_pointer::WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_pointer::Event::Enter {
                surface_x,
                surface_y,
                ..
            }
            | wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => state.pointer_at = (surface_x, surface_y),
            wl_pointer::Event::Button {
                button: BUTTON_LEFT,
                state: WEnum::Value(wl_pointer::ButtonState::Pressed),
                ..
            } if state.clickable => {
                let (x, y) = state.pointer_at;
                if let (Some(event), Some(clicks)) =
                    (click_event(x as f32, y as f32), &state.clicks)
                {
                    let _ = clicks.send(event);
                }
            }
            _ => {}
        }
    }
}

delegate_noop!(Wayland: ignore wl_compositor::WlCompositor);
delegate_noop!(Wayland: ignore wl_shm::WlShm);
delegate_noop!(Wayland: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Wayland: ignore wl_buffer::WlBuffer);
delegate_noop!(Wayland: ignore wl_region::WlRegion);
delegate_noop!(Wayland: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
