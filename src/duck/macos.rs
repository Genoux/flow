//! Ducking on a Mac, where there is no volume per app: what can be lowered is
//! the default output device's own volume, and everything playing goes down
//! with it.
//!
//! The device's "streams" are one entry, `(device id, volume)`, so the ramp,
//! the settled handshake and the crash recovery in the parent are the same
//! code the Linux ducker runs. The volume is held in millionths rather than
//! percent so that restoring is exact: a read, a duck and a restore must leave
//! the slider where the user had it.

use super::{LEVEL, WATCH_INTERVAL, state_file};
use anyhow::{Result, anyhow};
use objc2_core_audio::{
    AudioObjectGetPropertyData, AudioObjectIsPropertySettable, AudioObjectPropertyAddress,
    AudioObjectSetPropertyData, kAudioHardwarePropertyDefaultOutputDevice,
    kAudioObjectPropertyElementMain, kAudioObjectPropertyScopeGlobal,
    kAudioObjectPropertyScopeOutput, kAudioObjectSystemObject,
};
use std::ffi::c_void;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// `kAudioHardwareServiceDeviceProperty_VirtualMainVolume`: the one volume a
/// device's own slider drives, whatever its channels are doing. The constant is
/// not in the bindings because it lives in the deprecated hardware-service
/// header, and the property itself is current.
const VIRTUAL_MAIN_VOLUME: u32 = u32::from_be_bytes(*b"vmvc");

/// One for the device's whole range, in the millionths [`volume`] reports.
const FULL: f32 = 1_000_000.0;

/// How far the volume may sit from what the duck last wrote before it is the
/// user's hand and not ours. A step of the system slider is 1/16; this is a
/// fraction of one, enough to ignore the device rounding what it was given.
const MOVED: u32 = 20_000;

/// What the duck last wrote, which is what the watcher compares the device
/// against.
static WRITTEN: AtomicU32 = AtomicU32::new(0);

fn address(selector: u32, scope: u32) -> AudioObjectPropertyAddress {
    AudioObjectPropertyAddress {
        mSelector: selector,
        mScope: scope,
        mElement: kAudioObjectPropertyElementMain,
    }
}

fn default_output() -> Option<u32> {
    let mut device = 0u32;
    let mut size = size_of::<u32>() as u32;
    let mut at = address(
        kAudioHardwarePropertyDefaultOutputDevice,
        kAudioObjectPropertyScopeGlobal,
    );
    // SAFETY: the address, size and out-buffer are live locals, and the buffer
    // is the u32 an AudioDeviceID is.
    let status = unsafe {
        AudioObjectGetPropertyData(
            kAudioObjectSystemObject as u32,
            NonNull::from(&mut at),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut device).cast::<c_void>(),
        )
    };
    (status == 0 && device != 0).then_some(device)
}

fn settable(device: u32) -> bool {
    let mut at = address(VIRTUAL_MAIN_VOLUME, kAudioObjectPropertyScopeOutput);
    let mut settable = 0u8;
    // SAFETY: both pointers are live locals.
    let status = unsafe {
        AudioObjectIsPropertySettable(device, NonNull::from(&mut at), NonNull::from(&mut settable))
    };
    status == 0 && settable != 0
}

/// The device's volume in millionths, or `None` if it has none to read.
fn volume(device: u32) -> Option<u32> {
    let mut scalar = 0f32;
    let mut size = size_of::<f32>() as u32;
    let mut at = address(VIRTUAL_MAIN_VOLUME, kAudioObjectPropertyScopeOutput);
    // SAFETY: the address, size and out-buffer are live locals, and the buffer
    // is the f32 this property holds.
    let status = unsafe {
        AudioObjectGetPropertyData(
            device,
            NonNull::from(&mut at),
            0,
            std::ptr::null(),
            NonNull::from(&mut size),
            NonNull::from(&mut scalar).cast::<c_void>(),
        )
    };
    (status == 0).then(|| (scalar.clamp(0.0, 1.0) * FULL).round() as u32)
}

/// Whether the default output has a volume to lower. An HDMI or USB output
/// often has none, and ducking it would cost the pre-roll for nothing.
pub fn available() -> bool {
    default_output().is_some_and(settable)
}

pub fn streams() -> Result<Vec<(u32, u32)>> {
    let device = default_output().ok_or_else(|| anyhow!("no output device"))?;
    if !settable(device) {
        return Err(anyhow!("the output device has no volume to lower"));
    }
    let now = volume(device).ok_or_else(|| anyhow!("could not read the output volume"))?;
    WRITTEN.store(now, Ordering::SeqCst);
    Ok(vec![(device, now)])
}

/// Linear in the device's own scale, which is already perceptual: its slider
/// is what the user means by half.
pub fn ducked(original: u32, level: u32) -> u32 {
    ((original as u64 * level as u64 + 50) / 100) as u32
}

/// Against the device it was taken from, not whatever is the default now: a
/// duck is undone on the output it lowered, even if the user has since plugged
/// in headphones.
pub fn set_volume(device: u32, amount: u32) {
    let mut scalar = amount as f32 / FULL;
    let mut at = address(VIRTUAL_MAIN_VOLUME, kAudioObjectPropertyScopeOutput);
    // SAFETY: the address and the f32 value are live locals. A device that has
    // gone away answers with an error, which is expected and not worth one.
    let _ = unsafe {
        AudioObjectSetPropertyData(
            device,
            NonNull::from(&mut at),
            0,
            std::ptr::null(),
            size_of::<f32>() as u32,
            NonNull::from(&mut scalar).cast::<c_void>(),
        )
    };
    // What the device holds, which is not always what it was given: an output
    // that steps in sixteenths rounds, and comparing against the request would
    // read its own rounding as a hand on the slider.
    WRITTEN.store(volume(device).unwrap_or(amount), Ordering::SeqCst);
}

/// Whether the device still sits where a duck to `level` left it, or on the way
/// there or back - so a crash mid-ramp is still repaired, and a volume the user
/// has moved to somewhere else is left alone.
pub fn still_ours(device: u32, original: u32, level: Option<u32>) -> bool {
    let (Some(now), Some(level)) = (volume(device), level) else {
        return true;
    };
    on_the_way(now, ducked(original, level), original)
}

/// Between the ducked volume and the original, give or take what the device
/// rounds to.
fn on_the_way(now: u32, ducked: u32, original: u32) -> bool {
    now >= ducked.min(original).saturating_sub(MOVED) && now <= ducked.max(original) + MOVED
}

/// Notice the user taking the volume into their own hands during a duck, and
/// let go of it.
///
/// Linux holds a stream down if it jumps back up, because that is a player
/// changing track. Here the only thing that moves the volume is a person, and
/// pushing the slider back against them is the one thing a duck must not do:
/// the restore is dropped too, so what they set is where it stays.
pub fn watch(known: Arc<Mutex<Vec<(u32, u32)>>>, active: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        let mut seen = None;
        while active.load(Ordering::SeqCst) {
            std::thread::sleep(WATCH_INTERVAL);
            // Re-checked after the sleep: a restore that started meanwhile
            // owns the volume, and a newer duck owns the shared record of what
            // was last written.
            if !active.load(Ordering::SeqCst) {
                return;
            }
            let Some(device) = known.lock().unwrap().first().map(|(device, _)| *device) else {
                return;
            };
            // While the ramp is writing, the device is a step behind what was
            // last written by design. Only a value that has held for a whole
            // poll - the ramp has landed - can say whose hand moved it.
            let written = WRITTEN.load(Ordering::SeqCst);
            if seen != Some(written) || LEVEL.load(Ordering::SeqCst) == 100 {
                seen = Some(written);
                continue;
            }
            let Some(now) = volume(device) else { continue };
            if now.abs_diff(written) > MOVED {
                // Only the one that ends the duck may act on it. Losing this
                // swap means a restore got there first, and the state file is
                // by now the next duck's.
                if active.swap(false, Ordering::SeqCst) {
                    eprintln!("output volume changed during the duck - leaving it where it is");
                    known.lock().unwrap().clear();
                    let _ = std::fs::remove_file(state_file());
                }
                return;
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The restore lands on `ducked(original, 100)`, and the slider must come
    /// back exactly where the user left it.
    #[test]
    fn a_full_level_is_the_original_exactly() {
        for original in [0, 1, 62_500, 333_333, 1_000_000] {
            assert_eq!(ducked(original, 100), original);
        }
    }

    #[test]
    fn the_duck_is_a_share_of_the_original() {
        assert_eq!(ducked(800_000, 25), 200_000);
        assert_eq!(ducked(800_000, 0), 0);
    }

    /// A crash during the ramp leaves the volume part way, and that is still
    /// the duck's to put back.
    #[test]
    fn a_volume_on_the_way_is_still_ours_to_restore() {
        assert!(on_the_way(300_000, 300_000, 750_000));
        assert!(on_the_way(520_000, 300_000, 750_000));
        assert!(on_the_way(750_000, 300_000, 750_000));
    }

    /// Where a person has put it since is theirs.
    #[test]
    fn a_volume_somewhere_else_is_left_alone() {
        assert!(!on_the_way(900_000, 300_000, 750_000));
        assert!(!on_the_way(100_000, 300_000, 750_000));
    }

    #[test]
    fn an_already_quiet_output_is_never_made_louder() {
        assert!(ducked(100_000, 99) <= 100_000);
    }
}
