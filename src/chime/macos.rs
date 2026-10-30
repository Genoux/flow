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
            sound.setVolume(volume_at(crate::duck::level()));
            // Restarted if it is still sounding, so a quick second chord is
            // heard rather than swallowed by `play` refusing to overlap itself.
            sound.stop();
            if !sound.play() {
                eprintln!("the chime could not be played");
                return;
            }
            compensate(wav.as_ptr() as usize);
        });
    });
}

/// The cue plays through the output the duck is lowering, so it is raised by
/// as much as the duck has taken - up to full volume, where it stops being able
/// to keep up and only partly makes up the difference.
fn volume_at(level: u32) -> f32 {
    (VOLUME as f32 / 65_536.0 * 100.0 / level.max(1) as f32).min(1.0)
}

/// The duck is ramping while the cue sounds, since both answer the same
/// keypress, so one adjustment at the start would be right for an instant. The
/// volume follows the ramp until the cue is over.
fn compensate(key: usize) {
    std::thread::spawn(move || {
        for _ in 0..CUE_TICKS {
            std::thread::sleep(CUE_TICK);
            DispatchQueue::main().exec_async(move || {
                LOADED.with_borrow(|loaded| {
                    if let Some(sound) = loaded.get(&key) {
                        sound.setVolume(volume_at(crate::duck::level()));
                    }
                });
            });
        }
    });
}

/// A cue lasts about 230ms; this covers it with room to spare.
const CUE_TICK: std::time::Duration = std::time::Duration::from_millis(10);
const CUE_TICKS: u32 = 30;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unducked_cue_plays_at_its_own_volume() {
        assert!((volume_at(100) - VOLUME as f32 / 65_536.0).abs() < 1e-6);
    }

    #[test]
    fn a_ducked_cue_is_raised_by_what_the_duck_took() {
        assert!(volume_at(50) > volume_at(100));
        assert!((volume_at(50) - 2.0 * volume_at(100)).abs() < 1e-6);
    }

    #[test]
    fn the_cue_never_goes_past_full_volume() {
        assert_eq!(volume_at(1), 1.0);
        assert_eq!(volume_at(0), 1.0);
    }
}
