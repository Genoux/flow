//! Can word timestamps separate speech from a room? Level cannot: across 169
//! labelled recordings from the journal, junk spans peak 0.000-1.276 and rms
//! 0.0000-0.0291 while real dictations span peak 0.129-1.125 and rms
//! 0.0114-0.0634. Both overlap. But a spurious word floats alone in a second of
//! nothing, and real speech fills the time it took to say.
//!   cargo test --release --test voiced -- --ignored --nocapture
use parakeet_rs::{ParakeetTDT, TimestampMode, Transcriber};
use std::time::Duration;

/// Fraction of the recording covered by recognised words.
fn coverage(model: &mut ParakeetTDT, samples: &[f32]) -> (String, f32, usize) {
    let spoken = samples.len() as f32 / 16_000.0;
    let result = model
        .transcribe_samples(samples.to_vec(), 16_000, 1, Some(TimestampMode::Words))
        .expect("transcribe");
    let voiced: f32 = result
        .tokens
        .iter()
        .map(|t| (t.end - t.start).max(0.0))
        .sum();
    (
        result.text.trim().to_string(),
        voiced / spoken.max(0.001),
        result.tokens.len(),
    )
}

#[test]
#[ignore]
fn words_fill_speech_and_float_in_a_room() {
    let dir = flow::stt::model_dir();
    let mut model = ParakeetTDT::from_pretrained(&dir, None).expect("model");

    let speech = flow::wav::read_16k_mono("tests/fixtures/jfk.wav").expect("fixture");
    let (text, cov, n) = coverage(&mut model, &speech);
    eprintln!(
        "jfk.wav:  coverage {:.1}%  {n} words  {text:?}",
        cov * 100.0
    );

    for round in 1..=3 {
        let device = flow::audio::open_device().expect("device");
        let capture = flow::audio::Capture::open(&device).expect("open");
        capture.begin();
        eprintln!("\nround {round}: recording 3s of your room - say nothing");
        std::thread::sleep(Duration::from_secs(3));
        let room = capture.end();
        let (text, cov, n) = coverage(&mut model, &room);
        eprintln!("  room: coverage {:.1}%  {n} words  {text:?}", cov * 100.0);
    }
}
