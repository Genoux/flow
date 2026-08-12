use std::process::Command;
use std::time::{Duration, Instant};

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

/// Mutates real playback volume, so it is opt-in:
///     cargo test --release --test duck -- --ignored --nocapture
/// Needs something playing (music, a video) to observe.
#[test]
#[ignore]
fn duck_ramps_and_restores() {
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
