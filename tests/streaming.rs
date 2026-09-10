use flow::stt::Stt;
use std::time::{Duration, Instant};

#[test]
#[ignore = "requires the installed nemotron model and FLOW_TEST_WAV (16 kHz mono)"]
fn streaming_keeps_the_tail_and_resets_between_dictations() {
    let path = std::env::var("FLOW_TEST_WAV").expect("FLOW_TEST_WAV");
    let mut wav = hound::WavReader::open(path).unwrap();
    assert_eq!(wav.spec().sample_rate, 16_000);
    assert_eq!(wav.spec().channels, 1);
    let audio: Vec<f32> = wav
        .samples::<i16>()
        .map(|s| s.unwrap() as f32 / 32768.0)
        .collect();
    let dir = std::env::var("FLOW_TEST_MODEL")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|_| {
            std::path::PathBuf::from(std::env::var("HOME").unwrap())
                .join(".local/share/flow/models/nemotron")
        });
    let mut stt = Stt::load(&dir).unwrap();
    let started = Instant::now();
    let offline = stt.transcribe(audio.clone()).unwrap();
    let offline_time = started.elapsed();
    let size = stt.chunk_samples();
    assert!(audio.len() > size * 3);
    stt.start_stream();
    let started = Instant::now();
    let expected = stt.finish_stream(&audio).unwrap();
    let streaming_time = started.elapsed();
    assert!(!expected.is_empty());
    stt.start_stream();
    let split = (audio.len() / size - 1) * size;
    let mut worst = Duration::ZERO;
    for chunk in audio[..split].chunks_exact(size) {
        let started = Instant::now();
        stt.stream_chunk(chunk).unwrap();
        worst = worst.max(started.elapsed());
    }
    let started = Instant::now();
    let actual = stt.finish_stream(&audio[split..]).unwrap();
    let release_time = started.elapsed();
    assert_eq!(actual, expected);
    stt.start_stream();
    assert_eq!(stt.finish_stream(&vec![0.0; size * 2 + 137]).unwrap(), "");
    stt.start_stream();
    assert_eq!(stt.finish_stream(&audio).unwrap(), expected);
    eprintln!(
        "audio={:.2}s offline={offline_time:?} streaming={streaming_time:?} release={release_time:?} worst_chunk={worst:?} chunk_samples={size}",
        audio.len() as f32 / 16000.0
    );
    eprintln!("offline: {offline}\nstreaming: {actual}");
}
