use anyhow::{Context, Result};
use std::path::PathBuf;
use std::process::Command;

/// Playback streams are lowered while recording, then put back. This matters
/// beyond comfort: with a phone-over-network microphone the speakers bleed into
/// the capture, and music has been transcribed as if it were speech.
pub struct Ducker {
    restored: bool,
    original: Vec<(u32, u32)>,
}

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
            && percents.iter().all(|p| p == first) {
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

impl Ducker {
    /// Lower every playback stream to `percent` of its current level.
    /// Relative, because an absolute target would make an already-quiet
    /// stream louder.
    pub fn duck(percent: u32) -> Result<Self> {
        let original = streams()?;

        // Persisted first: a kill -9 skips Drop, and the next start reads this
        // rather than leaving the user's music mysteriously quiet.
        let _ = std::fs::write(state_file(), serde_json::to_vec(&original)?);

        for (index, volume) in &original {
            set_volume(*index, volume * percent / 100);
        }

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
        for (index, volume) in &self.original {
            set_volume(*index, *volume);
        }
        let _ = std::fs::remove_file(state_file());
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
        eprintln!("restoring {} stream volume(s) from an interrupted run", saved.len());
        for (index, volume) in saved {
            set_volume(index, volume);
        }
    }
    let _ = std::fs::remove_file(path);
}
