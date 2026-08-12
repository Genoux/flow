use anyhow::{bail, Result};
use flow::{audio, hotkey, inject, stt, wav};
use std::time::{Duration, Instant};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let terminal = args.iter().any(|a| a == "--terminal");

    // Isolates injection from the mic and the model, so a silent uinput failure
    // is distinguishable from a transcription problem.
    if args.first().map(String::as_str) == Some("inject") {
        let text = args.get(1).cloned().unwrap_or_else(|| "flow test".into());
        eprintln!("focus a text field - injecting in 3s");
        std::thread::sleep(Duration::from_secs(3));
        return inject::Injector::new()?.inject(&text, terminal);
    }

    let dir = stt::model_dir();
    if !dir.is_dir() {
        bail!("model not found at {}", dir.display());
    }
    let mut engine = stt::Stt::load(&dir)?;

    match args.first().map(String::as_str) {
        Some(path) if path.ends_with(".wav") => benchmark(&mut engine, path),
        Some("daemon") => daemon(&mut engine, terminal),
        other => {
            let seconds = other.and_then(|s| s.parse().ok()).unwrap_or(5);
            record_once(&mut engine, seconds)
        }
    }
}

/// Hold the push-to-talk key, speak, release. Text lands in the focused window.
fn daemon(engine: &mut stt::Stt, terminal: bool) -> Result<()> {
    let device = audio::open_device()?;
    let mut listener = hotkey::Listener::new()?;
    let mut injector = inject::Injector::new()?;

    eprintln!("\nready - hold {:?} and speak\n", hotkey::PTT);

    let mut recorder = None;
    while let Some(event) = listener.next_event() {
        match event {
            hotkey::Event::Pressed => match audio::Recorder::start(&device) {
                Ok(started) => recorder = Some(started),
                Err(err) => eprintln!("could not open mic: {err}"),
            },

            // A shortcut, not dictation - throw the audio away.
            hotkey::Event::Cancelled => {
                recorder.take();
            }

            hotkey::Event::Released { held } => {
                let Some(active) = recorder.take() else { continue };
                let samples = active.stop();

                if !hotkey::was_long_enough(held) {
                    continue;
                }
                if let Err(err) = handle(engine, &mut injector, samples, terminal) {
                    eprintln!("{err}");
                }
            }
        }
    }
    Ok(())
}

fn handle(
    engine: &mut stt::Stt,
    injector: &mut inject::Injector,
    samples: Vec<f32>,
    terminal: bool,
) -> Result<()> {
    let spoken = samples.len() as f32 / audio::SAMPLE_RATE as f32;
    let started = Instant::now();

    let text = engine.transcribe(samples)?;
    let transcribed = started.elapsed();

    if text.is_empty() {
        eprintln!("({spoken:.1}s, nothing recognised)");
        return Ok(());
    }

    injector.inject(&text, terminal)?;
    eprintln!(
        "{spoken:.1}s -> {transcribed:?} stt, {:?} total: {text}",
        started.elapsed()
    );
    Ok(())
}

fn record_once(engine: &mut stt::Stt, seconds: u64) -> Result<()> {
    use cpal::traits::DeviceTrait;

    let device = audio::open_device()?;
    eprintln!("input: {}", device.id().map(|i| i.to_string()).unwrap_or_default());
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
