use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

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
    /// takes exactly `FADE_OUT`, so there is nothing to tune per machine.
    settled: Arc<AtomicBool>,
}

/// Down quickly, since recording has already begun; back up more gently, which
/// is how a returning track is expected to sound.
const FADE_OUT: Duration = Duration::from_millis(200);
const FADE_IN: Duration = Duration::from_millis(260);

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

/// Bumped by every new fade so an in-flight one knows it has been superseded.
static GENERATION: AtomicUsize = AtomicUsize::new(0);

fn state_file() -> PathBuf {
    flow_paths::duck_state_file()
}

/// Every playback stream whose channels share one volume, as (index, percent).
///
/// ponytail: streams with per-channel differences are skipped rather than
/// flattened. Restoring them would need the channel order, which pactl's JSON
/// object does not reliably preserve, and silently destroying someone's
/// left/right balance is worse than leaving one stream loud.
fn streams() -> Result<Vec<(u32, u32)>> {
    let output = Command::new("pactl")
        .args(["--format=json", "list", "sink-inputs"])
        .output()
        .context("running pactl - is PipeWire or PulseAudio available?")?;

    let parsed: serde_json::Value = serde_json::from_slice(&output.stdout)?;
    let mut found = Vec::new();

    for stream in parsed.as_array().into_iter().flatten() {
        let Some(index) = stream.get("index").and_then(serde_json::Value::as_u64) else {
            continue;
        };
        let Some(channels) = stream.get("volume").and_then(serde_json::Value::as_object) else {
            continue;
        };

        let percents: Vec<u32> = channels
            .values()
            .filter_map(|c| c.get("value_percent")?.as_str())
            .filter_map(|p| p.trim_end_matches('%').parse().ok())
            .collect();

        if let Some(first) = percents.first()
            && percents.iter().all(|p| p == first)
        {
            found.push((index as u32, *first));
        }
    }
    Ok(found)
}

fn set_volume(index: u32, percent: u32) {
    // A stream that ended is expected, not an error - a track can finish
    // mid-dictation, and stale ids are normal at startup recovery. pactl still
    // complains on stderr, so its output is discarded rather than logged.
    let _ = Command::new("pactl")
        .args([
            "set-sink-input-volume",
            &index.to_string(),
            &format!("{percent}%"),
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

fn apply(streams: &[(u32, u32)], level: u32) {
    LEVEL.store(level, Ordering::SeqCst);
    for (index, original) in streams {
        set_volume(*index, original * level / 100);
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
        }
    });
}

/// The volume a new stream should be restored to.
///
/// A track change often inherits the already-ducked level of the stream it
/// replaced. Treating that as the original is how music came back at 50% after
/// a dictation. If the new stream is sitting where a known original would be
/// after the duck, remember that original; if it appeared exactly at the duck
/// target, it was a 100% stream that PipeWire cloned already quiet.
pub fn original_volume(current: u32, level: u32, known: &[(u32, u32)]) -> u32 {
    if !(1..100).contains(&level) {
        return current;
    }
    if let Some((_, original)) = known
        .iter()
        .find(|(_, original)| current.abs_diff(*original * level / 100) <= 2)
    {
        return *original;
    }
    if current.abs_diff(level) <= 2 {
        return 100;
    }
    current
}

/// True when a stream we already ducked has gone louder than the duck allows.
/// Players do this on a track change without changing the stream id.
pub fn escaped(current: u32, original: u32, level: u32) -> bool {
    current > original * level / 100 + 2
}

/// Catch streams that appear after ducking began, and streams that jump back
/// up. A track change may reuse an id or mint a new one; either way the music
/// must stay down for the rest of the hold and come back at the volume it had
/// before we touched it.
fn watch(known: Arc<Mutex<Vec<(u32, u32)>>>, active: Arc<AtomicBool>) {
    std::thread::spawn(move || {
        while active.load(Ordering::SeqCst) {
            std::thread::sleep(WATCH_INTERVAL);
            if !active.load(Ordering::SeqCst) {
                return;
            }

            let Ok(current) = streams() else { continue };
            let mut known = known.lock().unwrap();
            let level = LEVEL.load(Ordering::SeqCst);
            let mut dirty = false;

            for (index, volume) in current {
                if let Some((_, original)) = known.iter().find(|(seen, _)| *seen == index) {
                    if escaped(volume, *original, level) {
                        set_volume(index, *original * level / 100);
                    }
                    continue;
                }
                let original = original_volume(volume, level, &known);
                set_volume(index, original * level / 100);
                known.push((index, original));
                dirty = true;
            }

            if dirty && let Ok(encoded) = serde_json::to_vec(&*known) {
                let _ = std::fs::write(state_file(), encoded);
            }
        }
    });
}

impl Ducker {
    /// Lower every playback stream to `percent` of its current level.
    /// Relative, because an absolute target would make an already-quiet
    /// stream louder.
    pub fn duck(percent: u32) -> Result<Self> {
        let found = streams()?;

        // Persisted first: a kill -9 skips Drop, and the next start reads this
        // rather than leaving the user's music mysteriously quiet.
        let _ = std::fs::write(state_file(), serde_json::to_vec(&found)?);

        let settled = Arc::new(AtomicBool::new(false));
        fade(found.clone(), percent.min(100), FADE_OUT, false, Some(settled.clone()));

        let known = Arc::new(Mutex::new(found));
        let active = Arc::new(AtomicBool::new(true));
        watch(known.clone(), active.clone());

        Ok(Self {
            restored: false,
            known,
            active,
            settled,
        })
    }

    /// True once the opening fade-out has actually landed on target. Recording
    /// waits on this rather than a fixed sleep - `FADE_OUT` is the only real
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
        fade(known, 100, FADE_IN, true, None);
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
    let Ok(raw) = std::fs::read(&path) else { return };

    if let Ok(saved) = serde_json::from_slice::<Vec<(u32, u32)>>(&raw) {
        eprintln!(
            "restoring {} stream volume(s) from an interrupted run",
            saved.len()
        );
        // Immediate, not ramped: this is repair at startup, not a transition.
        for (index, volume) in saved {
            set_volume(index, volume);
        }
    }
    let _ = std::fs::remove_file(path);
}
