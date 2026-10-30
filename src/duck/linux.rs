//! Ducking through `pactl`: every playback stream is lowered on its own, so the
//! user's other apps go quiet and nothing else does.

use super::{LEVEL, WATCH_INTERVAL, state_file};
use anyhow::{Context, Result};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// A stream id from before a crash is only reused by something else after a
/// reboot, and the state file does not outlive one.
pub fn still_ours(_index: u32, _original: u32, _level: Option<u32>) -> bool {
    true
}

/// PipeWire can always be asked. Whether it is there at all is `streams`'
/// answer, and a duck that finds nothing to lower costs nothing.
pub fn available() -> bool {
    true
}

/// Every playback stream whose channels share one volume, as (index, percent).
///
/// ponytail: streams with per-channel differences are skipped rather than
/// flattened. Restoring them would need the channel order, which pactl's JSON
/// object does not reliably preserve, and silently destroying someone's
/// left/right balance is worse than leaving one stream loud.
pub fn streams() -> Result<Vec<(u32, u32)>> {
    let output = Command::new("pactl")
        .args(["--format=json", "list", "sink-inputs"])
        .output()
        .context("running pactl - is PipeWire or PulseAudio available?")?;

    Ok(duckable(&serde_json::from_slice(&output.stdout)?))
}

/// The streams in a `pactl list sink-inputs` payload that Flow may touch, as
/// `(index, volume percent)`. Split from the call above so the two ways a
/// stream is passed over - Flow's own chime, and an unbalanced player - can be
/// tested without an audio server.
pub fn duckable(parsed: &serde_json::Value) -> Vec<(u32, u32)> {
    let mut found = Vec::new();

    for stream in parsed.as_array().into_iter().flatten() {
        // Flow's own island chime. Ducking it would fade out the very sound
        // saying the microphone is open - see [`crate::chime::CLIENT`].
        if stream
            .get("properties")
            .and_then(|properties| properties.get("application.name"))
            .and_then(serde_json::Value::as_str)
            == Some(crate::chime::CLIENT)
        {
            continue;
        }

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
    found
}

/// The pactl percentage that leaves `original` playing at `level` percent of
/// its signal.
///
/// PulseAudio percentages are cubic in amplitude, so asking for a literal 50%
/// hands the stream -18 dB - an eighth of the signal, which is why music at
/// "half volume" was barely audible. The setting means half the sound, so the
/// linear fraction is cube-rooted back into the scale pactl actually speaks.
pub fn ducked(original: u32, level: u32) -> u32 {
    (original as f32 * (level as f32 / 100.0).cbrt()).round() as u32
}

pub fn set_volume(index: u32, percent: u32) {
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
        .find(|(_, original)| current.abs_diff(ducked(*original, level)) <= 2)
    {
        return *original;
    }
    if current.abs_diff(ducked(100, level)) <= 2 {
        return 100;
    }
    current
}

/// True when a stream we already ducked has gone louder than the duck allows.
/// Players do this on a track change without changing the stream id.
pub fn escaped(current: u32, original: u32, level: u32) -> bool {
    current > ducked(original, level) + 2
}

/// Catch streams that appear after ducking began, and streams that jump back
/// up. A track change may reuse an id or mint a new one; either way the music
/// must stay down for the rest of the hold and come back at the volume it had
/// before we touched it.
pub fn watch(known: Arc<Mutex<Vec<(u32, u32)>>>, active: Arc<AtomicBool>) {
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
                        set_volume(index, ducked(*original, level));
                    }
                    continue;
                }
                let original = original_volume(volume, level, &known);
                set_volume(index, ducked(original, level));
                known.push((index, original));
                dirty = true;
            }

            if dirty && let Ok(encoded) = serde_json::to_vec(&*known) {
                let _ = std::fs::write(state_file(), encoded);
            }
        }
    });
}
