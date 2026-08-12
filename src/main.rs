use anyhow::{bail, Result};
use flow::{audio, stt, wav};
use std::time::{Duration, Instant};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let dir = stt::model_dir();
    if !dir.is_dir() {
        bail!("model not found at {}", dir.display());
    }

    let mut engine = stt::Stt::load(&dir)?;

    // A .wav argument benchmarks against a fixture; a number records live.
    match args.first() {
        Some(path) if path.ends_with(".wav") => benchmark(&mut engine, path),
        other => {
            let seconds = other.and_then(|s| s.parse().ok()).unwrap_or(5);
            record(&mut engine, seconds)
        }
    }
}

fn record(engine: &mut stt::Stt, seconds: u64) -> Result<()> {
    use cpal::traits::DeviceTrait;

    let device = audio::open_device()?;
    let label = device.id().map(|id| id.to_string()).unwrap_or_default();
    eprintln!("input: {label}");
    eprintln!("recording {seconds}s - speak now...");

    let samples = audio::record(&device, Duration::from_secs(seconds))?;
    eprintln!(
        "captured {:.1}s, peak {:.3}",
        samples.len() as f32 / audio::SAMPLE_RATE as f32,
        audio::peak(&samples)
    );

    let started = Instant::now();
    let text = engine.transcribe(samples)?;
    println!("\n{text}\n");
    eprintln!("transcribed in {:?}", started.elapsed());
    Ok(())
}

/// One warmup pass then two timed passes, so the reported figure excludes
/// first-call allocation inside onnxruntime.
fn benchmark(engine: &mut stt::Stt, path: &str) -> Result<()> {
    let samples = wav::read_16k_mono(path)?;
    let duration = samples.len() as f32 / audio::SAMPLE_RATE as f32;
    eprintln!("{path}: {duration:.1}s of audio\n");

    let mut text = String::new();
    for run in 1..=3 {
        let started = Instant::now();
        text = engine.transcribe(samples.clone())?;
        let elapsed = started.elapsed();
        eprintln!(
            "{} {elapsed:?}  ({:.1}x realtime)",
            if run == 1 { "warmup" } else { "timed " },
            duration / elapsed.as_secs_f32()
        );
    }
    println!("\n{text}\n");
    Ok(())
}
