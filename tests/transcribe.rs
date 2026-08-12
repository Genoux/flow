use std::time::Instant;

/// Ground truth for tests/fixtures/jfk.wav (11s, 16 kHz mono).
const EXPECTED: &str = "And so, my fellow Americans, ask not what your country can do for you. \
Ask what you can do for your country.";

/// Guards the whole STT chain: model load, feature extraction, TDT decode,
/// and punctuation/capitalisation. Skips when the model isn't downloaded.
#[test]
fn transcribes_fixture_accurately() {
    let dir = flow::stt::model_dir();
    if !dir.is_dir() {
        eprintln!("skipping: no model at {}", dir.display());
        return;
    }

    let mut engine = flow::stt::Stt::load(&dir).expect("load model");
    let samples = flow::wav::read_16k_mono("tests/fixtures/jfk.wav").expect("read fixture");
    let duration = samples.len() as f32 / flow::audio::SAMPLE_RATE as f32;

    let started = Instant::now();
    let text = engine.transcribe(samples).expect("transcribe");
    let elapsed = started.elapsed();

    assert_eq!(text, EXPECTED);

    // 5x realtime is far below the measured ~23x, so this catches a collapse
    // (silent CPU-thread starvation, a bad model swap) without being flaky.
    let ratio = duration / elapsed.as_secs_f32();
    assert!(ratio > 5.0, "transcription too slow: {ratio:.1}x realtime");
}
