use anyhow::{bail, Result};
use flow::{audio, hotkey, inject, ipc, stt, wav};
use std::time::{Duration, Instant};

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    let terminal = args.iter().any(|a| a == "--terminal");

    // Fires at the running daemon; nothing else here needs to load.
    match args.first().map(String::as_str) {
        Some("start") => return ipc::send(ipc::START),
        Some("stop") => return ipc::send(ipc::STOP),
        _ => {}
    }

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
        // --no-ptt leaves Right Ctrl alone so a compositor bind is the only
        // trigger, which keeps A/B comparisons against another tool clean.
        Some("daemon") => daemon(&mut engine, terminal, !args.iter().any(|a| a == "--no-ptt")),
        other => {
            let seconds = other.and_then(|s| s.parse().ok()).unwrap_or(5);
            record_once(&mut engine, seconds)
        }
    }
}

/// Two ways in: hold the push-to-talk key, or send SIGUSR1 (`flow toggle`) from
/// a compositor keybind, which fires on press and so must toggle rather than hold.
fn daemon(engine: &mut stt::Stt, terminal: bool, ptt: bool) -> Result<()> {
    let device = audio::open_device()?;
    let mut injector = inject::Injector::new()?;

    let (events, incoming) = std::sync::mpsc::channel();
    if ptt {
        hotkey::spawn(events.clone())?;
    }

    let signals = events.clone();
    let mut listener = signal_hook::iterator::Signals::new([ipc::START, ipc::STOP])?;
    std::thread::spawn(move || {
        for signal in listener.forever() {
            let event = if signal == ipc::START {
                hotkey::Event::Start
            } else {
                hotkey::Event::Stop
            };
            if signals.send(event).is_err() {
                return;
            }
        }
    });

    ipc::write_pid()?;
    eprintln!(
        "\nready - {}hold a key bound to `flow start` / `flow stop`\n",
        if ptt {
            format!("hold {:?}, or ", hotkey::PTT)
        } else {
            String::new()
        }
    );

    let mut recorder: Option<audio::Recorder> = None;
    while let Ok(event) = incoming.recv() {
        // Recording ends on release, on a second toggle, or never for a cancel.
        let finished = match event {
            hotkey::Event::Pressed => {
                start(&device, &mut recorder);
                None
            }
            // Idempotent: a repeated start keeps the running capture rather
            // than restarting it and losing what was already said.
            hotkey::Event::Start => {
                if recorder.is_none() {
                    start(&device, &mut recorder);
                }
                None
            }
            hotkey::Event::Stop => recorder.take().map(|active| active.stop()),
            // A shortcut, not dictation - throw the audio away.
            hotkey::Event::Cancelled => {
                recorder.take();
                None
            }
            hotkey::Event::Released { held } => recorder
                .take()
                .map(|active| active.stop())
                .filter(|_| hotkey::was_long_enough(held)),
        };

        if let Some(samples) = finished
            && let Err(err) = handle(engine, &mut injector, samples, terminal) {
                eprintln!("{err}");
            }
    }

    ipc::remove_pid();
    Ok(())
}

fn start(device: &cpal::Device, slot: &mut Option<audio::Recorder>) {
    match audio::Recorder::start(device) {
        Ok(started) => {
            eprintln!("recording...");
            *slot = Some(started);
        }
        Err(err) => eprintln!("could not open mic: {err}"),
    }
}

fn handle(
    engine: &mut stt::Stt,
    injector: &mut inject::Injector,
    samples: Vec<f32>,
    terminal: bool,
) -> Result<()> {
    let spoken = samples.len() as f32 / audio::SAMPLE_RATE as f32;
    let peak = audio::peak(&samples);
    let rms = audio::rms(&samples);
    let level = format!("peak {peak:.3} rms {rms:.4}");

    // Only catches a dead capture. Room tone still transcribes to confident
    // nonsense ("Uh", "See no lay no") and still gets pasted - see SILENCE_RMS
    // for why that needs VAD rather than a louder threshold.
    if rms < audio::SILENCE_RMS {
        eprintln!("({spoken:.1}s, {level}, no signal - skipped)");
        return Ok(());
    }

    let started = Instant::now();
    let text = engine.transcribe(samples)?;
    let transcribed = started.elapsed();

    if text.is_empty() {
        eprintln!("({spoken:.1}s, {level}, nothing recognised)");
        return Ok(());
    }

    injector.inject(&text, terminal)?;
    eprintln!(
        "{spoken:.1}s {level} -> {transcribed:?} stt, {:?} total: {text}",
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
