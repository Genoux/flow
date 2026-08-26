//! Per-app microphone routing, against the real PipeWire graph.
//!
//! Ignored by default, like the other tests here that need hardware: there is
//! no way to fake this one usefully. The whole feature rests on a single claim
//! about a live system - that Flow's capture appears in the graph as a stream
//! that can be moved - and a mock would only ever confirm that this file agrees
//! with itself.
//!
//! Run with `cargo test --test routing -- --ignored --nocapture`.

use cpal::traits::DeviceTrait;
use std::process::Command;

fn pactl(args: &[&str]) -> String {
    let output = Command::new("pactl").args(args).output().expect("pactl");
    String::from_utf8_lossy(&output.stdout).into_owned()
}

/// Real microphones, skipping the monitor sources that make up most of the
/// list. Mirrors the console's own filter; kept separate because a test that
/// imports the thing it is checking proves nothing about the machine.
fn microphones() -> Vec<String> {
    let listing = pactl(&["list", "sources"]);
    listing
        .lines()
        .filter_map(|line| line.trim().strip_prefix("Name: "))
        .map(str::trim)
        .filter(|name| !name.ends_with(".monitor"))
        .map(str::to_owned)
        .collect()
}

/// The claim the whole picker is built on: cpal's capture shows up as a
/// movable PipeWire stream, so choosing a microphone needs no cpal changes and
/// no second audio path.
///
/// Rebuilding the stream against a chosen device is the obvious alternative
/// and does not work - `Capture::attach` asks for 16kHz mono f32, which only
/// succeeds because it is talking to PipeWire rather than to the hardware.
#[test]
#[ignore = "needs a real PipeWire graph and a microphone"]
fn flows_own_stream_can_be_moved_between_microphones() {
    let device = flow::audio::open_device().expect("no input device");
    if device.default_input_config().is_err() {
        eprintln!("skipping: no usable input hardware");
        return;
    }
    let capture = flow::audio::Capture::open(&device).expect("open capture");

    let mics = microphones();
    assert!(!mics.is_empty(), "no non-monitor sources to move between");
    eprintln!("microphones offered: {mics:#?}");

    // Nothing pinned means nothing moved: PipeWire follows the default source
    // by itself, and asking it to do so again would be a shell-out per
    // dictation for no change at all.
    capture.set_source(None);
    assert_eq!(
        capture.current_source(),
        None,
        "an unpinned capture moved its own stream"
    );

    for mic in &mics {
        capture.set_source(Some(mic));
        assert_eq!(
            capture.current_source().as_ref(),
            Some(mic),
            "could not route the capture to {mic} - if this is the only failure, \
             the stream is not being found in `pactl list source-outputs`"
        );
    }

    // The point of the feature: Flow moved, the system default did not.
    let default_before = pactl(&["get-default-source"]);
    capture.set_source(Some(&mics[0]));
    assert_eq!(
        pactl(&["get-default-source"]),
        default_before,
        "routing Flow changed the system default, which is what this exists to avoid"
    );

    // And back to Automatic, which has to name the default explicitly - the
    // stream is sitting on a pinned mic and will stay there otherwise.
    capture.set_source(None);
    assert_eq!(
        capture.current_source().as_deref(),
        Some(default_before.trim()),
        "switching back to Automatic left the capture on the pinned microphone"
    );
}
