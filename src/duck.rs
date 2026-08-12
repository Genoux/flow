use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Playback streams are lowered while recording, then put back. This matters
/// beyond comfort: with a phone-over-network microphone the speakers bleed into
/// the capture, and music has been transcribed as if it were speech.
///
/// Both directions are ramped rather than stepped, because an abrupt cut is
/// more distracting than the sound it is trying to get out of the way.
pub struct Ducker {
    restored: bool,
    original: Vec<(u32, u32)>,
}

/// Down quickly, since recording has already begun; back up more gently, which
/// is how a returning track is expected to sound.
const FADE_OUT: Duration = Duration::from_millis(140);
const FADE_IN: Duration = Duration::from_millis(260);

/// A full step costs ~4ms per stream, so this is a target rather than a
/// guarantee - the ramp is interpolated against the clock, not the step count,
/// so a slow step drops frames instead of stretching the fade.
const STEP: Duration = Duration::from_millis(20);

/// Where the ramp currently is, as a percentage of each stream's own level.
/// Global because only one dictation runs at a time, and a new fade must be
/// able to pick up wherever an interrupted one left off.
static LEVEL: AtomicU32 = AtomicU32::new(100);

/// Bumped by every new fade so an in-flight one knows it has been superseded.
static GENERATION: AtomicUsize = AtomicUsize::new(0);

fn state_file() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("flow-duck.json")
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
    // A stream that ended mid-dictation is expected, not an error.
    let _ = Command::new("pactl")
        .args([
            "set-sink-input-volume",
            &index.to_string(),
            &format!("{percent}%"),
        ])
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
fn fade(streams: Vec<(u32, u32)>, target: u32, duration: Duration, clear_state: bool) {
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

        if clear_state {
            let _ = std::fs::remove_file(state_file());
        }
    });
}

impl Ducker {
    /// Lower every playback stream to `percent` of its current level.
    /// Relative, because an absolute target would make an already-quiet
    /// stream louder.
    pub fn duck(percent: u32) -> Result<Self> {
        let original = streams()?;

        // Persisted first: a kill -9 skips Drop, and the next start reads this
        // rather than leaving the user's music mysteriously quiet.
        let _ = std::fs::write(state_file(), serde_json::to_vec(&original)?);

        fade(original.clone(), percent.min(100), FADE_OUT, false);

        Ok(Self {
            restored: false,
            original,
        })
    }

    pub fn restore(&mut self) {
        if self.restored {
            return;
        }
        self.restored = true;
        fade(self.original.clone(), 100, FADE_IN, true);
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
