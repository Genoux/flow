//! The chime through `NSSound`, in this process.
//!
//! AppKit is for the main thread, and the caller is the one drawing the
//! island, so the work is handed to the main queue and never waited on - the
//! same arrangement as the overlay's surface.

use super::VOLUME;
use dispatch2::DispatchQueue;
use objc2::AnyThread;
use objc2::rc::Retained;
use objc2_app_kit::NSSound;
use objc2_foundation::NSData;
use std::cell::RefCell;
use std::collections::HashMap;
use std::collections::hash_map::Entry;

thread_local! {
    /// Kept for good: a sound released while it plays is cut off, and decoding
    /// the same two cues on every dictation is work with nothing to show.
    /// Main thread only, like everything that touches AppKit.
    static LOADED: RefCell<HashMap<usize, Retained<NSSound>>> = RefCell::new(HashMap::new());
}

pub fn play(wav: &'static [u8]) {
    DispatchQueue::main().exec_async(move || {
        LOADED.with_borrow_mut(|loaded| {
            let sound = match loaded.entry(wav.as_ptr() as usize) {
                Entry::Occupied(known) => known.into_mut(),
                Entry::Vacant(unknown) => {
                    let data = NSData::with_bytes(wav);
                    let Some(sound) = NSSound::initWithData(NSSound::alloc(), &data) else {
                        // Not a panic: this runs on the main queue, and
                        // unwinding out of a libdispatch block aborts the
                        // whole daemon.
                        eprintln!("the chime could not be decoded");
                        return;
                    };
                    unknown.insert(sound)
                }
            };
            sound.setVolume(VOLUME as f32 / 65_536.0);
            // Restarted if it is still sounding, so a quick second chord is
            // heard rather than swallowed by `play` refusing to overlap itself.
            sound.stop();
            if !sound.play() {
                eprintln!("the chime could not be played");
            }
        });
    });
}
