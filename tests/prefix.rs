//! Where it is safe to cut a recording in half while it is still being spoken.
//!
//! Transcription runs at ~23x realtime on the CPU, so the start of a long
//! dictation can be transcribed while the rest is still arriving - measured, that
//! is 45% of the wait on anything over ten seconds. It is only worth doing if the
//! cut lands in silence: tests/chunking.rs shows a cut between sentences costs
//! nothing, and a cut through a word would cost a word.

use flow::audio::{split_at_silence, SAMPLE_RATE, SILENCE_RMS};

const RATE: usize = SAMPLE_RATE as usize;

fn speech(seconds: f32) -> Vec<f32> {
    (0..(seconds * RATE as f32) as usize)
        .map(|index| {
            // A loud tone, well above the silence floor.
            (index as f32 * 0.05).sin() * 0.3
        })
        .collect()
}

fn silence(seconds: f32) -> Vec<f32> {
    vec![0.0; (seconds * RATE as f32) as usize]
}

/// The ordinary case: someone pauses between sentences, and the pause is where
/// the recording gets handed off.
#[test]
fn a_pause_between_sentences_is_where_it_splits() {
    let mut samples = speech(9.0);
    samples.extend(silence(0.4));
    samples.extend(speech(5.0));

    let at = split_at_silence(&samples, 8 * RATE).expect("should find the pause");
    assert!(
        at >= 9 * RATE && at <= (9.4 * RATE as f32) as usize,
        "split at {:.2}s, expected inside the pause at 9.0-9.4s",
        at as f32 / RATE as f32
    );
}

/// Unbroken speech must not be cut. Paying the full transcription at the end is
/// the right answer here - the alternative is a word sliced in half.
#[test]
fn continuous_speech_is_never_cut() {
    let samples = speech(30.0);
    assert_eq!(split_at_silence(&samples, 8 * RATE), None, "no gap to split on");
}

/// Nothing to gain from splitting a short recording, and the tail would be
/// shorter than the encoder's own overhead.
#[test]
fn a_short_recording_is_left_alone() {
    let mut samples = speech(2.0);
    samples.extend(silence(0.5));
    samples.extend(speech(2.0));
    assert_eq!(split_at_silence(&samples, 8 * RATE), None, "too short to bother");
}

/// A gap has to be quiet enough to be a real gap. Quiet speech and room tone are
/// not pauses, and cutting there would clip a word.
#[test]
fn a_merely_quieter_passage_is_not_a_gap() {
    let mut samples = speech(9.0);
    // Above the silence floor: someone talking softly, not stopping.
    samples.extend(vec![SILENCE_RMS * 4.0; RATE / 2]);
    samples.extend(speech(5.0));
    assert_eq!(
        split_at_silence(&samples, 8 * RATE),
        None,
        "only split where it is genuinely silent"
    );
}

/// The tail after the split has to be worth transcribing on its own, or the split
/// buys nothing and costs an extra encoder pass.
#[test]
fn a_gap_at_the_very_end_is_not_worth_splitting() {
    let mut samples = speech(9.0);
    samples.extend(silence(0.2));
    assert_eq!(
        split_at_silence(&samples, 8 * RATE),
        None,
        "nothing meaningful left after the gap"
    );
}

/// Splitting repeatedly must keep making progress: each call has to hand over a
/// prefix strictly shorter than what it was given, or a long dictation would
/// loop.
#[test]
fn repeated_splitting_makes_progress() {
    let mut samples = Vec::new();
    for _ in 0..4 {
        samples.extend(speech(9.0));
        samples.extend(silence(0.4));
    }
    samples.extend(speech(5.0));

    let mut rest = samples.as_slice();
    let mut cuts = 0;
    while let Some(at) = split_at_silence(rest, 8 * RATE) {
        assert!(at > 0 && at < rest.len(), "cut at {at} of {}", rest.len());
        rest = &rest[at..];
        cuts += 1;
        assert!(cuts < 10, "not terminating");
    }
    assert!(cuts >= 3, "expected a cut per pause, got {cuts}");
}
