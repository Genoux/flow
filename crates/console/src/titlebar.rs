//! The macOS unified title bar: an empty toolbar in the `Unified` style makes
//! AppKit draw the bar 52pt tall and centre the window buttons in it, inset
//! from the corner, which is what Finder and Notes look like. Neither iced
//! 0.14 nor winit 0.30 has a setting for it, so it is set on the NSWindow.

use iced::Task;

pub(crate) fn unify<T: Send + 'static>() -> Task<T> {
    #[cfg(target_os = "macos")]
    {
        iced::window::oldest()
            .and_then(|id| iced::window::run(id, macos::unify))
            .discard()
    }
    #[cfg(not(target_os = "macos"))]
    Task::none()
}

#[cfg(target_os = "macos")]
mod macos {
    use iced_runtime::window::raw_window_handle::RawWindowHandle;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSTitlebarSeparatorStyle, NSToolbar, NSView, NSWindowToolbarStyle};

    pub(super) fn unify(window: &dyn iced_runtime::window::Window) {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };
        let Ok(handle) = window.window_handle() else {
            return;
        };
        let RawWindowHandle::AppKit(handle) = handle.as_raw() else {
            return;
        };
        // SAFETY: the handle borrows `window` and the reference does not escape
        // this call; AppKit is only touched on the main thread (checked above).
        let view: &NSView = unsafe { handle.ns_view.cast().as_ref() };
        let Some(ns_window) = view.window() else {
            return;
        };
        ns_window.setToolbar(Some(&NSToolbar::new(mtm)));
        ns_window.setToolbarStyle(NSWindowToolbarStyle::Unified);
        ns_window.setTitlebarSeparatorStyle(NSTitlebarSeparatorStyle::None);
    }
}
