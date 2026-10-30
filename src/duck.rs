use anyhow::Result;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

#[cfg(target_os = "linux")]
mod linux;
#[cfg(target_os = "macos")]
mod macos;

#[cfg(target_os = "linux")]
use linux as platform;
#[cfg(target_os = "macos")]
use macos as platform;

#[cfg(target_os = "linux")]
pub use linux::{duckable, ducked, escaped, original_volume};

/// Volumes are moved through `pactl` on Linux and the default output device's
/// own volume on a Mac. A duck that cannot lower anything must not be asked
/// for: the daemon drops the pre-roll whenever it ducks, which would cost the
/// first word for nothing.
///
/// What can be lowered also depends on the machine - an HDMI output may have
/// no volume to set - so this is the question to ask before ducking, and not
/// just whether the platform has a way to.
pub fn available() -> bool {
    platform::available()
}

/// Playback streams are lowered while recording, then put back. This matters
/// beyond comfort: with a phone-over-network microphone the speakers bleed into
/// the capture, and music has been transcribed as if it were speech.
///
/// Both directions are ramped rather than stepped, because an abrupt cut is
/// more distracting than the sound it is trying to get out of the way.
pub struct Ducker {
    restored: bool,
    /// Every stream ducked so far, including ones that started after recording
    /// began. Restoring uses this rather than the opening snapshot.
    known: Arc<Mutex<Vec<(u32, u32)>>>,
    active: Arc<AtomicBool>,
    /// Flips true the instant the opening fade-out actually lands on target.
    /// This is what recording waits on instead of guessing a sleep: the ramp
    /// takes exactly `FADE`, so there is nothing to tune per machine.
    settled: Arc<AtomicBool>,
}

/// Both directions ride the island, in and out. The duck and the animation
/// answer the same keypress and end on the same event, so giving them separate
/// timings only made one lag the other - the music came back while the shape
/// was still on screen. Derived from [`crate::overlay::BLOOM`] rather than
/// copied from it, so the two cannot drift apart again.
const FADE: Duration = Duration::from_millis((crate::overlay::BLOOM * 1000.0) as u64);

/// A full step costs ~4ms per stream, so this is a target rather than a
/// guarantee - the ramp is interpolated against the clock, not the step count,
/// so a slow step drops frames instead of stretching the fade.
const STEP: Duration = Duration::from_millis(20);

/// How often to look for streams that started after ducking began, or that
/// jumped back to full volume. A scan is one pactl call; 250ms left a whole
/// chorus audible on a track change.
const WATCH_INTERVAL: Duration = Duration::from_millis(50);

/// Where the ramp currently is, as a percentage of each stream's own level.
/// Global because only one dictation runs at a time, and a new fade must be
/// able to pick up wherever an interrupted one left off.
static LEVEL: AtomicU32 = AtomicU32::new(100);

/// How far the duck currently is, as a percentage of the original level. For
/// what has to know how much quieter everything else is: the chime on a Mac
/// plays through the output being ducked.
pub fn level() -> u32 {
    LEVEL.load(Ordering::SeqCst)
}

/// Bumped by every new fade so an in-flight one knows it has been superseded.
static GENERATION: AtomicUsize = AtomicUsize::new(0);

fn state_file() -> PathBuf {
    flow_paths::duck_state_file()
}

/// The originals of a restore that is still ramping back. A dictation that
/// starts inside that ramp would otherwise read the half-restored volumes as
/// the originals and restore to those - music coming back at half.
static RESTORING: Mutex<Vec<(u32, u32)>> = Mutex::new(Vec::new());

/// `found` as read just now, with any stream a restore is still returning
/// swapped for the original that restore is returning it to.
fn reconcile(found: Vec<(u32, u32)>, restoring: &[(u32, u32)]) -> Vec<(u32, u32)> {
    found
        .into_iter()
        .map(|(index, volume)| {
            restoring
                .iter()
                .find(|(saved, _)| *saved == index)
                .copied()
                .unwrap_or((index, volume))
        })
        .collect()
}

/// What is persisted while ducked: the level asked for, so a later start can
/// tell a volume that is still ours from one the user has since moved, and the
/// originals to put back. The older bare list is still read.
fn record(level: u32, streams: &[(u32, u32)]) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({ "level": level, "streams": streams }))
        .unwrap_or_default()
}

/// `(index, volume)` per stream.
type Streams = Vec<(u32, u32)>;

fn parse_record(raw: &[u8]) -> Option<(Option<u32>, Streams)> {
    let value: serde_json::Value = serde_json::from_slice(raw).ok()?;
    if value.is_array() {
        return Some((None, serde_json::from_value(value).ok()?));
    }
    let level = u32::try_from(value.get("level")?.as_u64()?).ok()?;
    Some((
        Some(level),
        serde_json::from_value(value.get("streams")?.clone()).ok()?,
    ))
}

fn apply(streams: &[(u32, u32)], level: u32) {
    LEVEL.store(level, Ordering::SeqCst);
    for (index, original) in streams {
        platform::set_volume(*index, platform::ducked(*original, level));
    }
}

/// Ramp from wherever the last fade reached to `target`, in a background thread
/// so neither the start of recording nor transcription waits on it.
fn fade(
    streams: Vec<(u32, u32)>,
    target: u32,
    duration: Duration,
    clear_state: bool,
    on_settled: Option<Arc<AtomicBool>>,
) {
    let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;

    std::thread::spawn(move || {
        let from = LEVEL.load(Ordering::SeqCst) as f32;
        let to = target as f32;
        let started = Instant::now();

        loop {
            // A newer fade owns the volume now; leaving it alone avoids the two
            // ramps fighting when a recording is very short.
            if GENERATION.load(Ordering::SeqCst) != generation {
                return;
            }

            let progress = started.elapsed().as_secs_f32() / duration.as_secs_f32();
            if progress >= 1.0 {
                break;
            }

            apply(&streams, (from + (to - from) * progress).round() as u32);
            std::thread::sleep(STEP);
        }

        // Land exactly on the target rather than wherever rounding stopped.
        apply(&streams, target);

        if let Some(flag) = on_settled {
            flag.store(true, Ordering::SeqCst);
        }

        if clear_state {
            let _ = std::fs::remove_file(state_file());
            RESTORING.lock().unwrap().clear();
        }
    });
}

impl Ducker {
    /// Lower every playback stream to `percent` of its current level.
    /// Relative, because an absolute target would make an already-quiet
    /// stream louder.
    pub fn duck(percent: u32) -> Result<Self> {
        let restoring = std::mem::take(&mut *RESTORING.lock().unwrap());
        let found = reconcile(platform::streams()?, &restoring);

        // Persisted first: a kill -9 skips Drop, and the next start reads this
        // rather than leaving the user's music mysteriously quiet.
        let _ = std::fs::write(state_file(), record(percent.min(100), &found));

        let settled = Arc::new(AtomicBool::new(false));
        fade(
            found.clone(),
            percent.min(100),
            FADE,
            false,
            Some(settled.clone()),
        );

        let known = Arc::new(Mutex::new(found));
        let active = Arc::new(AtomicBool::new(true));
        platform::watch(known.clone(), active.clone());

        Ok(Self {
            restored: false,
            known,
            active,
            settled,
        })
    }

    /// True once the opening fade-out has actually landed on target. Recording
    /// waits on this rather than a fixed sleep - `FADE` is the only real
    /// constraint, and it is the same on every machine.
    pub fn settled(&self) -> bool {
        self.settled.load(Ordering::SeqCst)
    }

    pub fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        // Stop watching before fading back, or a stream discovered mid-fade
        // would be pinned to a level the fade is about to leave behind.
        self.active.store(false, Ordering::SeqCst);

        let known = self.known.lock().unwrap().clone();
        *RESTORING.lock().unwrap() = known.clone();
        fade(known, 100, FADE, true, None);
    }
}

impl Drop for Ducker {
    fn drop(&mut self) {
        self.restore();
    }
}

/// Undo a duck left behind by a daemon that was killed mid-recording.
/// Called at startup, where the state file is the only record of what was lost.
pub fn restore_stale() {
    let path = state_file();
    let Ok(raw) = std::fs::read(&path) else {
        return;
    };

    if let Some((level, saved)) = parse_record(&raw) {
        eprintln!(
            "restoring {} stream volume(s) from an interrupted run",
            saved.len()
        );
        // Immediate, not ramped: this is repair at startup, not a transition.
        // Only what is still where the duck left it: a volume the user has
        // moved since the crash is theirs now.
        for (index, volume) in saved {
            if platform::still_ours(index, volume, level) {
                platform::set_volume(index, volume);
            }
        }
    }
    let _ = std::fs::remove_file(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A dictation that begins while the last restore is mid-ramp finds the
    /// volumes half way back. The originals are what it has to keep.
    #[test]
    fn a_restore_in_flight_hands_back_its_originals() {
        let mid_ramp = vec![(7, 31), (8, 90)];
        let restoring = [(7, 100)];
        assert_eq!(reconcile(mid_ramp, &restoring), vec![(7, 100), (8, 90)]);
    }

    #[test]
    fn with_no_restore_in_flight_what_is_read_is_the_original() {
        assert_eq!(reconcile(vec![(7, 64)], &[]), vec![(7, 64)]);
    }

    #[test]
    fn the_record_carries_the_level_and_the_old_list_still_reads() {
        let streams = [(3, 80), (4, 55)];
        assert_eq!(
            parse_record(&record(40, &streams)),
            Some((Some(40), streams.to_vec()))
        );
        assert_eq!(
            parse_record(b"[[3,80],[4,55]]"),
            Some((None, streams.to_vec()))
        );
        assert_eq!(parse_record(b"nonsense"), None);
    }
}
