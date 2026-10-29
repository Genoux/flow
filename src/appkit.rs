//! The application the menu bar item and the overlay are drawn by.
//!
//! AppKit belongs to the main thread, and what Flow does does not run there -
//! so the main thread is given to the app and the work to a thread of its own.

use anyhow::{Context, Result};
use objc2::MainThreadMarker;
use objc2_app_kit::{NSApplication, NSApplicationActivationPolicy};
use std::panic::{AssertUnwindSafe, catch_unwind};

/// The stack the daemon had when it was the main thread.
const WORKER_STACK: usize = 8 * 1024 * 1024;

/// Run `work` on a thread of its own and the main thread as an accessory
/// application, so there is an AppKit to draw with.
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
