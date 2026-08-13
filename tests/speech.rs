//! Whether a recording contains a voice at all.
//!
//! Level cannot answer this. Across 169 labelled recordings from the journal, junk
//! spans peak 0.000-1.276 and rms 0.0000-0.0291 while real dictations span peak
//! 0.129-1.125 and rms 0.0114-0.0634 - they overlap on both, so every threshold
//! either passes room tone or eats speech. Measured, what separates them is
//! movement: speech swings 30x between its vowels and the gaps between its words,
//! this room swings 2 to 3x.

use flow::audio::{sounds_like_speech, swing, SAMPLE_RATE};

const RATE: usize = SAMPLE_RATE as usize;

/// Deterministic noise at a fixed level, which is what a room is.
fn steady(level: f32, seconds: f32) -> Vec<f32> {
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    (0..(seconds * RATE as f32) as usize)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            ((state >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0) * level
        })
        .collect()
}

/// Loud bursts with quiet gaps between them, which is what speech is.
fn bursty(quiet: f32, loud: f32, seconds: f32) -> Vec<f32> {
    let mut out = steady(quiet, seconds);
    let syllable = RATE / 6;
    for (index, sample) in out.iter_mut().enumerate() {
        if (index / syllable) % 2 == 0 {
            *sample += loud * (index as f32 * 0.4).sin();
        }
    }
    out
}

#[test]
fn a_steady_room_is_not_a_voice() {
    for level in [0.002, 0.008, 0.02, 0.05] {
        let room = steady(level, 3.0);
        assert!(
            !sounds_like_speech(&room),
            "steady noise at {level} passed as speech (swing {:.1}x)",
            swing(&room)
        );
    }
}

/// The point of measuring movement rather than level: loud room tone is still not
/// a voice, and quiet speech still is.
#[test]
fn loudness_does_not_decide() {
    let loud_room = steady(0.05, 3.0);
    let quiet_speech = bursty(0.002, 0.02, 3.0);
    assert!(!sounds_like_speech(&loud_room), "loud room: {:.1}x", swing(&loud_room));
    assert!(sounds_like_speech(&quiet_speech), "quiet speech: {:.1}x", swing(&quiet_speech));
}

#[test]
fn real_speech_is_a_voice() {
    let speech = flow::wav::read_16k_mono("tests/fixtures/jfk.wav").expect("fixture");
    let measured = swing(&speech);
    assert!(sounds_like_speech(&speech), "the fixture is speech, swing {measured:.1}x");
    assert!(measured > 10.0, "expected a wide swing on real speech, got {measured:.1}x");
}

/// Too little audio to judge must never be rejected - losing a word is far worse
/// than passing a stray one through.
#[test]
fn too_short_to_judge_is_given_the_benefit_of_the_doubt() {
    assert!(sounds_like_speech(&[]));
    assert!(sounds_like_speech(&steady(0.008, 0.05)));
}
