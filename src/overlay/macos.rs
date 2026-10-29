//! The macOS island: a borderless `NSPanel` whose layer shows the canvas.
//!
//! AppKit belongs to the main thread, and the overlay's frame loop does not run
//! there. Every surface call therefore hands its AppKit work to the main queue,
//! and [`host`] is what keeps that thread serving it.

use super::{Canvas, Face, HEIGHT, MARGIN_BOTTOM, SURFACE_WIDTH, render};
use anyhow::{Context, Result};
use dispatch2::DispatchQueue;
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSBackingStoreType, NSColor, NSEvent, NSPanel,
    NSScreen, NSScreenSaverWindowLevel, NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_core_foundation::{CFData, CFType};
use objc2_core_graphics::{
    CGBitmapInfo, CGColorRenderingIntent, CGColorSpace, CGDataProvider, CGImage, CGImageAlphaInfo,
    CGImageByteOrderInfo,
};
use objc2_foundation::{NSPoint, NSRect, NSSize};
use objc2_quartz_core::CATransaction;
use std::cell::RefCell;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex, PoisonError};

/// The stack the daemon had when it was the main thread.
const WORKER_STACK: usize = 8 * 1024 * 1024;

thread_local! {
    /// Only ever touched from the main thread, which is the only one AppKit
    /// lets own a window.
    static PANEL: RefCell<Option<Retained<NSPanel>>> = const { RefCell::new(None) };
}

/// Run `work` on a thread of its own and the main thread as an accessory
/// application, so the overlay has an AppKit to draw with.
///
/// The process ends when `work` does, with the exit code and message the
/// runtime would have given had `work` been `main`. A panic must end it too:
/// the main thread is inside the app's event loop and would otherwise keep a
/// dead daemon alive.
pub fn host(work: fn() -> Result<()>) -> Result<()> {
    let mtm = MainThreadMarker::new().context("the app must start on the main thread")?;

    std::thread::Builder::new()
        .stack_size(WORKER_STACK)
        .spawn(move || {
            let code = match catch_unwind(AssertUnwindSafe(work)) {
                Ok(Ok(())) => 0,
                Ok(Err(err)) => {
                    eprintln!("Error: {err:?}");
                    1
                }
                Err(_) => 101,
            };
            std::process::exit(code);
        })
        .context("starting the daemon thread")?;

    let app = NSApplication::sharedApplication(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Accessory);
    app.run();
    Ok(())
}

/// One drawn frame, in the pixels `scale` makes of the island's points.
struct Frame {
    pixels: Vec<u8>,
    width: usize,
    height: usize,
    scale: f64,
}

type Latest = Arc<Mutex<Option<Frame>>>;

pub struct Surface {
    canvas: Canvas,
    scale: f64,
    mapped: bool,
    /// The newest frame the main thread has not drawn yet. The overlay paints
    /// far faster than the main queue is asked to draw, and a frame that has
    /// already been overtaken is not worth showing.
    latest: Latest,
}

impl Surface {
    pub fn connect() -> Result<Self> {
        Ok(Self {
            canvas: Canvas::new(0, 0),
            scale: 1.0,
            mapped: false,
            latest: Latest::default(),
        })
    }

    pub fn is_mapped(&self) -> bool {
        self.mapped
    }

    /// Synchronous, so the scale of the screen the island lands on is known
    /// before the first frame is painted at it.
    pub fn map(&mut self) {
        let mut scale = 1.0;
        DispatchQueue::main().exec_sync(|| scale = show());
        self.scale = scale;
        self.mapped = true;
    }

    pub fn dispatch(&mut self) -> Result<()> {
        Ok(())
    }

    /// A panel is only ever taken away by [`Surface::take_down`].
    pub fn is_closed(&self) -> bool {
        false
    }

    /// Mapped is configured: there is no handshake to wait for.
    pub fn is_ready(&self) -> bool {
        self.mapped
    }

    pub fn release(&mut self) {
        self.mapped = false;
    }

    pub fn take_down(&mut self) -> Result<()> {
        DispatchQueue::main().exec_async(hide);
        self.mapped = false;
        Ok(())
    }

    pub fn present(&mut self, face: Face) -> Result<()> {
        let width = SURFACE_WIDTH as usize * self.scale as usize;
        let height = HEIGHT as usize * self.scale as usize;
        if self.canvas.width != width {
            self.canvas = Canvas::new(width, height);
        }
        render(&mut self.canvas, face, self.scale as f32);

        let frame = Frame {
            pixels: self.canvas.pixels.clone(),
            width,
            height,
            scale: self.scale,
        };
        let idle = self
            .latest
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .replace(frame)
            .is_none();
        if idle {
            let latest = Arc::clone(&self.latest);
            DispatchQueue::main().exec_async(move || draw(&latest));
        }
        Ok(())
    }
}

/// Put the panel on screen and return the scale of that screen.
///
/// The screen is the one under the pointer, which is where the user is working.
/// `mainScreen` is the one with this app's key window, and an accessory app
/// never has one, so it is only the fallback. The island sits centred along the bottom of what the Dock and menu
/// bar leave, the same place a layer-shell surface anchored to the bottom edge
/// is given.
fn show() -> f64 {
    let Some(mtm) = MainThreadMarker::new() else {
        return 1.0;
    };
    let screen = screen_under_pointer(mtm);
    let usable = screen.as_ref().map(|screen| screen.visibleFrame());
    let scale = screen
        .as_ref()
        .map_or(1.0, |screen| screen.backingScaleFactor().round().max(1.0));

    PANEL.with_borrow_mut(|slot| {
        let panel = slot.get_or_insert_with(|| make_panel(mtm));
        if let Some(usable) = usable {
            let origin = NSPoint::new(
                usable.origin.x + (usable.size.width - SURFACE_WIDTH as f64) / 2.0,
                usable.origin.y + MARGIN_BOTTOM as f64,
            );
            panel.setFrameOrigin(origin);
        }
        clear_contents(panel);
        panel.orderFrontRegardless();
    });
    scale
}

fn screen_under_pointer(mtm: MainThreadMarker) -> Option<Retained<NSScreen>> {
    let pointer = NSEvent::mouseLocation();
    NSScreen::screens(mtm)
        .iter()
        .find(|screen| contains(screen.frame(), pointer))
        .or_else(|| NSScreen::mainScreen(mtm))
}

/// Inclusive on every edge: AppKit can report the pointer at exactly `maxX` or
/// `maxY` of the screen it is on, and a half-open check would then match no
/// screen at all. A point on the edge two screens share matches the first one
/// listed, which is as good as either.
fn contains(frame: NSRect, point: NSPoint) -> bool {
    point.x >= frame.origin.x
        && point.x <= frame.origin.x + frame.size.width
        && point.y >= frame.origin.y
        && point.y <= frame.origin.y + frame.size.height
}

fn hide() {
    PANEL.with_borrow(|slot| {
        if let Some(panel) = slot {
            panel.orderOut(None);
        }
    });
}

/// Never key, never main and never activating: the paste lands in whatever
/// app the user was typing in, and the island must not take that from it.
fn make_panel(mtm: MainThreadMarker) -> Retained<NSPanel> {
    let size = NSSize::new(SURFACE_WIDTH as f64, HEIGHT as f64);
    let panel = NSPanel::initWithContentRect_styleMask_backing_defer(
        mtm.alloc(),
        NSRect::new(NSPoint::new(0.0, 0.0), size),
        NSWindowStyleMask::Borderless | NSWindowStyleMask::NonactivatingPanel,
        NSBackingStoreType::Buffered,
        false,
    );
    panel.setOpaque(false);
    panel.setBackgroundColor(Some(&NSColor::clearColor()));
    panel.setHasShadow(false);
    panel.setIgnoresMouseEvents(true);
    panel.setFloatingPanel(true);
    panel.setHidesOnDeactivate(false);
    panel.setLevel(NSScreenSaverWindowLevel);
    panel.setCollectionBehavior(
        NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary
            | NSWindowCollectionBehavior::Stationary
            | NSWindowCollectionBehavior::IgnoresCycle,
    );
    if let Some(view) = panel.contentView() {
        view.setWantsLayer(true);
    }
    // The panel is released by the thread-local, not on close.
    // SAFETY: setReleasedWhenClosed is a plain flag on a window we own.
    unsafe { panel.setReleasedWhenClosed(false) };
    panel
}

/// A new mapping starts empty, so the last frame of the previous one is not
/// shown for the instant before the first of this one lands.
fn clear_contents(panel: &NSPanel) {
    set_contents(panel, None, 1.0);
}

fn draw(latest: &Latest) {
    let Some(frame) = latest.lock().unwrap_or_else(PoisonError::into_inner).take() else {
        return;
    };
    let Some(image) = image(&frame) else {
        return;
    };
    PANEL.with_borrow(|slot| {
        if let Some(panel) = slot {
            set_contents(panel, Some(&image), frame.scale);
        }
    });
}

/// The canvas is premultiplied ARGB8888 held as little-endian words, so its
/// bytes run B, G, R, A. CoreGraphics names that alpha-first with 32-bit
/// little-endian byte order.
fn image(frame: &Frame) -> Option<objc2_core_foundation::CFRetained<CGImage>> {
    let provider = CGDataProvider::with_cf_data(Some(&CFData::from_bytes(&frame.pixels)))?;
    let space = CGColorSpace::new_device_rgb()?;
    // SAFETY: a null `decode` array is the documented "use the default".
    unsafe {
        CGImage::new(
            frame.width,
            frame.height,
            8,
            32,
            frame.width * 4,
            Some(&space),
            CGBitmapInfo(
                CGImageByteOrderInfo::Order32Little.0 | CGImageAlphaInfo::PremultipliedFirst.0,
            ),
            Some(&provider),
            std::ptr::null(),
            false,
            CGColorRenderingIntent::RenderingIntentDefault,
        )
    }
}

/// Without the transaction CoreAnimation cross-fades every change of contents
/// into the next, which at this frame rate is a smear.
fn set_contents(panel: &NSPanel, image: Option<&CGImage>, scale: f64) {
    let Some(layer) = panel.contentView().and_then(|view| view.layer()) else {
        return;
    };
    let contents: Option<&AnyObject> = image.map(|image| {
        let object: &CFType = image.as_ref();
        object.as_ref()
    });
    CATransaction::begin();
    CATransaction::setDisableActions(true);
    layer.setContentsScale(scale);
    // SAFETY: a CGImage is the object CALayer.contents documents.
    unsafe { layer.setContents(contents) };
    CATransaction::commit();
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(x: f64, y: f64, width: f64, height: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(width, height))
    }

    #[test]
    fn a_point_inside_the_frame_is_contained() {
        assert!(contains(
            frame(0.0, 0.0, 100.0, 50.0),
            NSPoint::new(40.0, 10.0)
        ));
    }

    #[test]
    fn a_screen_to_the_left_has_negative_origin() {
        let left = frame(-1920.0, 0.0, 1920.0, 1080.0);
        assert!(contains(left, NSPoint::new(-10.0, 500.0)));
        assert!(!contains(left, NSPoint::new(10.0, 500.0)));
    }

    #[test]
    fn the_far_edges_are_contained() {
        let screen = frame(0.0, 0.0, 100.0, 50.0);
        assert!(contains(screen, NSPoint::new(100.0, 50.0)));
        assert!(contains(screen, NSPoint::new(0.0, 0.0)));
        assert!(!contains(screen, NSPoint::new(100.5, 50.0)));
    }
}
