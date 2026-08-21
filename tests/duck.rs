use std::process::Command;
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// A new stream sitting at the ducked level inherited it. Restoring that
/// number is how a track change left the music at 50%.
#[test]
fn a_stream_already_at_the_duck_remembers_full_volume() {
    let known = [(1, 100)];
    assert_eq!(flow::duck::original_volume(50, 50, &known), 100);
    assert_eq!(flow::duck::original_volume(50, 50, &[]), 100);
}

#[test]
fn a_stream_that_appears_loud_keeps_that_as_original() {
    assert_eq!(flow::duck::original_volume(80, 50, &[]), 80);
    assert_eq!(flow::duck::original_volume(100, 50, &[(1, 80)]), 100);
}

#[test]
fn a_stream_matching_a_known_original_reuses_it() {
    let known = [(1, 80)];
    assert_eq!(flow::duck::original_volume(40, 50, &known), 80);
}

#[test]
fn a_player_that_jumps_back_up_has_escaped() {
    assert!(flow::duck::escaped(100, 100, 50));
    assert!(flow::duck::escaped(80, 100, 50));
    assert!(!flow::duck::escaped(50, 100, 50));
    assert!(!flow::duck::escaped(52, 100, 50));
}

/// Ducking is process-global state, and cargo runs tests as parallel threads,
/// so two of these at once would fight over the same streams.
fn exclusive() -> MutexGuard<'static, ()> {
    static AUDIO: Mutex<()> = Mutex::new(());
    AUDIO
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Reads one live playback stream's volume, or None if nothing is playing.
fn sample() -> Option<(u32, u32)> {
    let out = Command::new("pactl")
        .args(["--format=json", "list", "sink-inputs"])
        .output()
        .ok()?;
    let parsed: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;

    for stream in parsed.as_array()? {
        let index = stream.get("index")?.as_u64()? as u32;
        let channels = stream.get("volume")?.as_object()?;
        let percent: Vec<u32> = channels
            .values()
            .filter_map(|c| c.get("value_percent")?.as_str())
            .filter_map(|p| p.trim_end_matches('%').parse().ok())
            .collect();
        if let Some(first) = percent.first()
            && percent.iter().all(|p| p == first)
        {
            return Some((index, *first));
        }
    }
    None
}

/// A track change gives the new track a new stream id. Without the watcher it
/// plays at full volume for the rest of the recording, which is the moment the
/// user least wants it.
///
/// Opt-in like the test below; plays a short tone as the "new" stream:
///   cargo test --release --test duck -- --ignored --nocapture
#[test]
#[ignore]
fn streams_started_after_ducking_are_caught() {
    let _serial = exclusive();
    let existing: Vec<u32> = sample().into_iter().map(|(index, _)| index).collect();
    let mut ducker = flow::duck::Ducker::duck(50).expect("duck");
    std::thread::sleep(Duration::from_millis(200));

    // speaker-test is in alsa-utils; skip cleanly when it is not installed.
    let Ok(mut tone) = Command::new("speaker-test")
        .args(["-t", "sine", "-f", "440", "-l", "1"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
    else {
        eprintln!("skipping: speaker-test not available");
        return;
    };

    // Long enough for the watcher to notice and act.
    std::thread::sleep(Duration::from_millis(900));

    let fresh: Vec<(u32, u32)> = {
        let out = Command::new("pactl")
            .args(["--format=json", "list", "sink-inputs"])
            .output()
            .expect("pactl");
        let parsed: serde_json::Value = serde_json::from_slice(&out.stdout).expect("json");
        parsed
            .as_array()
            .expect("array")
            .iter()
            .filter_map(|s| {
                let index = s.get("index")?.as_u64()? as u32;
                let first = s.get("volume")?.as_object()?.values().next()?;
                let percent: u32 = first
                    .get("value_percent")?
                    .as_str()?
                    .trim_end_matches('%')
                    .parse()
                    .ok()?;
                (!existing.contains(&index)).then_some((index, percent))
            })
            .collect()
    };

    ducker.restore();
    let _ = tone.kill();

    let Some((index, level)) = fresh.first() else {
        eprintln!("skipping: the tone did not register as its own stream");
        return;
    };
    assert!(
        *level < 100,
        "stream #{index} started mid-recording and was left at {level}%"
    );
}

/// Mutates real playback volume, so it is opt-in:
///     cargo test --release --test duck -- --ignored --nocapture
/// Needs something playing (music, a video) to observe.
#[test]
#[ignore]
fn duck_ramps_and_restores() {
    let _serial = exclusive();
    let Some((index, before)) = sample() else {
        eprintln!("skipping: nothing is playing");
        return;
    };
    eprintln!("stream #{index} starts at {before}%");

    let mut seen = Vec::new();
    {
        let mut ducker = flow::duck::Ducker::duck(50).expect("duck");

        let watching = Instant::now();
        while watching.elapsed() < Duration::from_millis(400) {
            if let Some((_, level)) = sample() {
                seen.push(level);
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        ducker.restore();
    }

    let target = before / 2;
    eprintln!("observed during fade: {seen:?}  (target {target}%)");

    // The point of the ramp: values strictly between start and target, rather
    // than a single jump from one to the other.
    let intermediate = seen
        .iter()
        .filter(|v| **v < before && **v > target + 1)
        .count();
    assert!(
        intermediate >= 2,
        "expected a gradual ramp, saw {seen:?} between {before}% and {target}%"
    );

    // Give the fade back up time to finish before checking.
    std::thread::sleep(Duration::from_millis(600));
    let (_, after) = sample().expect("stream still playing");
    assert!(
        after.abs_diff(before) <= 2,
        "volume not restored: {before}% -> {after}%"
    );
}
