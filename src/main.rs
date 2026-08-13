use anyhow::{bail, Result};
use flow::{audio, cleanup, config, duck, hotkey, inject, install, ipc, overlay, stt, wav};
use std::time::{Duration, Instant};

/// Audio that must be spoken before any of it is transcribed early. Long enough
/// that short dictations never pay an extra encoder pass, short enough that a
/// rambling one gets several pieces done before the key comes up.
const PREFIX_MIN: usize = 8 * audio::SAMPLE_RATE as usize;

/// How often to look for a pause worth cutting at. The speaker is still talking,
/// so there is nothing to race.
const PREFIX_POLL: Duration = Duration::from_millis(400);

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // All three run before the config is read. start/stop must survive a broken
    // config file so a daemon that is already recording can still be stopped, and
    // `install` is what writes that file in the first place.
    match args.first().map(String::as_str) {
        Some("start") => return ipc::send(ipc::START),
        Some("stop") => return ipc::send(ipc::STOP),
        Some("install") => {
            return install::run(args.iter().any(|a| a == "--speech-only"))
        }
        _ => {}
    }

    let settings = config::Config::load()?.overridden_by(&args);
    let terminal = settings.terminal;

    // Isolates injection from the mic and the model, so a silent uinput failure
    // is distinguishable from a transcription problem.
    if args.first().map(String::as_str) == Some("inject") {
        let text = args.get(1).cloned().unwrap_or_else(|| "flow test".into());
        eprintln!("focus a text field - injecting in 3s");
        std::thread::sleep(Duration::from_secs(3));
        return inject::Injector::new()?.inject(&text, terminal);
    }

    // Isolates the island from the model and the hotkeys, the way `inject`
    // isolates uinput: a compositor blur rule or a bar colour can be looked at
    // without dictating anything into whatever window happens to have focus.
    if args.first().map(String::as_str) == Some("overlay") {
        let seconds = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(10);
        let capture = audio::Capture::open(&audio::open_device()?)?;
        let overlay = overlay::Overlay::spawn(capture.monitor());
        capture.begin();
        overlay.record();
        eprintln!("island shown for {seconds}s - speak to move the bars");
        std::thread::sleep(Duration::from_secs(seconds));
        overlay.queued();
        overlay.working();
        eprintln!("transcribing sweep for 4s");
        std::thread::sleep(Duration::from_secs(4));
        overlay.cancel();
        std::thread::sleep(Duration::from_millis(200));
        return Ok(());
    }

    let dir = stt::model_dir();
    if !dir.is_dir() {
        bail!("model not found at {} - run `flow install`", dir.display());
    }
    let mut engine = stt::Stt::load(&dir)?;

    match args.first().map(String::as_str) {
        Some(path) if path.ends_with(".wav") => benchmark(&mut engine, path),
        Some("daemon") => {
            // Cleanup is the point of the tool, so it stays on unless the config
            // or --raw turns it off, or the model is missing.
            let cleaner = if settings.cleanup {
                match cleanup::Cleaner::load(
                    &cleanup::model_path(),
                    cleanup::vocabulary(),
                    settings.gpu,
                ) {
                    Ok(cleaner) => {
                        cleaner.warm_up();
                        Some(cleaner)
                    }
                    Err(err) => {
                        eprintln!("cleanup disabled: {err}");
                        None
                    }
                }
            } else {
                None
            };
            daemon(
                &mut engine,
                terminal,
                settings.chord.clone(),
                settings.push_to_talk,
                settings.ducking(),
                cleaner,
            )
        }
        other => {
            let seconds = other.and_then(|s| s.parse().ok()).unwrap_or(5);
            record_once(&mut engine, seconds)
        }
    }
}

/// Two ways in, and the second is deliberately not compositor-specific.
///
/// Flow watches its own chord on the keyboard, so a fresh install dictates with
/// no setup. Everything else comes through `flow start` / `flow stop`, which is
/// just a signal to this process - a compositor bind, a script, a foot pedal and
/// a stream deck are all the same caller as far as the daemon is concerned.
///
/// Only `start` is needed from those callers: Flow watches the physical chord to
/// find the release itself, because Hyprland's release binds are unreliable with
/// modifier chords.
fn daemon(
    engine: &mut stt::Stt,
    terminal: bool,
    chord: hotkey::Chord,
    ptt: bool,
    duck: Option<u32>,
    cleaner: Option<cleanup::Cleaner>,
) -> Result<()> {
    let device = audio::open_device()?;
    if let Some(name) = audio::default_source_name() {
        eprintln!("mic source: {name}");
    }
    let capture = audio::Capture::open(&device)?;
    if capture.warmup(Duration::from_secs(2)) {
        eprintln!("mic is live");
    } else {
        eprintln!("mic is silent - check the source above is unmuted and actually streaming");
    }

    let mut injector = inject::Injector::new()?;
    let overlay = overlay::Overlay::spawn(capture.monitor());

    duck::restore_stale();

    let (events, incoming) = std::sync::mpsc::channel();
    // Degrades rather than bails, the way a missing cleanup model does: the signal
    // path is independent, so a machine without /dev/input access should still
    // dictate from a compositor bind instead of refusing to start.
    let mut ptt = ptt;
    if ptt && let Err(err) = hotkey::spawn(events.clone(), chord.clone()) {
        eprintln!("push-to-talk disabled: {err}");
        ptt = false;
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
    hotkey::warmup_devices();
    eprintln!(
        "\nready - {}\n",
        if ptt {
            format!("hold {chord}, or trigger `flow start`")
        } else {
            "trigger `flow start`".to_string()
        }
    );

    let (jobs, job_rx) = std::sync::mpsc::channel();
    let island = &overlay;
    // Shared so the start of a long dictation can be transcribed while the rest is
    // still being spoken. The lock is held only for the duration of one
    // transcription, and taking it before taking audio is what keeps the early
    // pieces and the final tail in order.
    let engine = std::sync::Mutex::new(engine);
    let early: std::sync::Mutex<Vec<String>> = std::sync::Mutex::new(Vec::new());
    let listening = std::sync::atomic::AtomicBool::new(true);
    let (engine, early, listening) = (&engine, &early, &listening);

    std::thread::scope(|scope| {
        scope.spawn(move || {
            while let Ok((samples, terminal)) = job_rx.recv() {
                // Waits out any in-flight early piece, which is also what
                // guarantees its text is already in `early` before this reads it.
                let mut engine = engine.lock().expect("stt engine");
                let done = std::mem::take(&mut *early.lock().expect("early transcripts"));
                if let Err(err) =
                    handle(*engine, &mut injector, samples, done, terminal, cleaner.as_ref(), island)
                {
                    eprintln!("{err}");
                }
                drop(engine);
                // The island is showing the sweep until the text lands, error
                // or not - a failed transcription must not leave it spinning.
                island.finish();
            }
        });

        // Transcribes whatever the speaker has already finished saying. Sleeping
        // first costs nothing: there is no prefix to take until several seconds in.
        let recording = &capture;
        scope.spawn(move || {
            while listening.load(std::sync::atomic::Ordering::Relaxed) {
                std::thread::sleep(PREFIX_POLL);
                let mut engine = engine.lock().expect("stt engine");
                let Some(prefix) = recording.take_prefix(PREFIX_MIN) else { continue };
                let spoken = prefix.len() as f32 / audio::SAMPLE_RATE as f32;
                let started = Instant::now();
                match engine.transcribe(prefix) {
                    // Pushed while still holding the engine, so the release path
                    // cannot read a partial set of pieces.
                    Ok(text) if !text.trim().is_empty() => {
                        eprintln!("transcribed {spoken:.1}s early in {:?}", started.elapsed());
                        early.lock().expect("early transcripts").push(text);
                    }
                    Ok(_) => {}
                    Err(err) => eprintln!("early transcription failed: {err}"),
                }
            }
        });

        let mut session: Option<Session> = None;
        let mut chord_watch: Option<hotkey::ChordWatch> = None;
        let mut hold_started: Option<Instant> = None;
        while let Ok(event) = incoming.recv() {
            let was_recording = session.is_some();
            // Ending a session drops its ducker, so other apps come back to volume
            // as soon as recording stops rather than after transcription.
            let finished = match event {
                hotkey::Event::Pressed => {
                    if let Some(watch) = chord_watch.take() {
                        watch.disarm();
                    }
                    hold_started = Some(Instant::now());
                    begin(&capture, &mut session, duck, &overlay, early);
                    None
                }
                hotkey::Event::Start => {
                    // A second start during an active hold is key-repeat - keep
                    // the capture. A quick tap that already released is stopped
                    // by the chord watcher seeing keys already up.
                    if session.is_some() {
                        None
                    } else {
                        hold_started = Some(Instant::now());
                        begin(&capture, &mut session, duck, &overlay, early);
                        chord_watch = Some(hotkey::ChordWatch::arm(events.clone(), chord.clone()));
                        None
                    }
                }
                hotkey::Event::Stop => {
                    if let Some(watch) = chord_watch.take() {
                        watch.disarm();
                    }
                    hold_started.take();
                    session.take().map(|s| s.finish(&capture))
                }
                // A shortcut, not dictation - throw the audio away. Says so:
                // a discard that logs nothing is a dictation that vanished.
                hotkey::Event::Cancelled => {
                    if let Some(watch) = chord_watch.take() {
                        watch.disarm();
                    }
                    hold_started = None;
                    if let Some(session) = session.take() {
                        session.discard(&capture);
                        // Anything already transcribed early belongs to the
                        // recording being thrown away, so it goes too.
                        early.lock().expect("early transcripts").clear();
                        eprintln!("discarded: another key turned the hold into a shortcut");
                    }
                    None
                }
                hotkey::Event::Released { held } => {
                    if let Some(watch) = chord_watch.take() {
                        watch.disarm();
                    }
                    hold_started = None;
                    match session.take().map(|s| s.finish(&capture)) {
                        Some(samples) if hotkey::was_long_enough(held) => Some(samples),
                        Some(_) => {
                            early.lock().expect("early transcripts").clear();
                            eprintln!("discarded: {held:?} is too short to be a deliberate hold");
                            None
                        }
                        None => None,
                    }
                }
            };

            // Audio on its way to the model is only counted here. Whether it earns
            // a sweep is decided in `handle`, once there is something to say.
            match (&finished, session.is_some(), was_recording) {
                (Some(_), _, _) => overlay.queued(),
                // A recording ended with nothing usable - a cancel, a tap too
                // short. End the island here or it would sweep forever.
                (None, false, true) => overlay.cancel(),
                // Nothing started and nothing ended: a stray `flow stop`, a
                // duplicate start. The island may be sweeping for a dictation
                // still being transcribed, so it is not ours to take down.
                (None, false, false) => {}
                (None, true, _) => {}
            }

            if let Some(samples) = finished
                && jobs.send((samples, terminal)).is_err()
            {
                break;
            }
        }
        drop(jobs);
        listening.store(false, std::sync::atomic::Ordering::Relaxed);
    });

    ipc::remove_pid();
    Ok(())
}

/// A recording in progress. Holding the ducker here ties the volume of other
/// apps to the life of the capture, including on cancel.
struct Session {
    ducker: Option<duck::Ducker>,
}

impl Session {
    fn finish(self, capture: &audio::Capture) -> Vec<f32> {
        let samples = capture.end();
        drop(self.ducker);
        samples
    }

    fn discard(self, capture: &audio::Capture) {
        let _ = capture.end();
        drop(self.ducker);
    }
}

fn begin(
    capture: &audio::Capture,
    slot: &mut Option<Session>,
    duck: Option<u32>,
    overlay: &overlay::Overlay,
    early: &std::sync::Mutex<Vec<String>>,
) {
    // A new recording abandons whatever came before it, including anything already
    // transcribed early - otherwise those words would prepend to this dictation.
    // Reachable with both trigger paths live: a signal starts a session and the
    // physical chord then starts another.
    early.lock().expect("early transcripts").clear();
    capture.begin();
    overlay.record();
    eprintln!("recording...");
    *slot = Some(Session { ducker: None });
    if let Some(percent) = duck {
        match duck::Ducker::duck(percent) {
            Ok(ducker) => {
                if let Some(session) = slot.as_mut() {
                    session.ducker = Some(ducker);
                }
            }
            Err(err) => eprintln!("could not duck other apps: {err}"),
        }
    }
}

fn handle(
    engine: &mut stt::Stt,
    injector: &mut inject::Injector,
    samples: Vec<f32>,
    early: Vec<String>,
    terminal: bool,
    cleaner: Option<&cleanup::Cleaner>,
    island: &overlay::Overlay,
) -> Result<()> {
    let spoken = samples.len() as f32 / audio::SAMPLE_RATE as f32;
    let peak = audio::peak(&samples);
    let rms = audio::rms(&samples);
    let level = format!("peak {peak:.3} rms {rms:.4}");

    let started = Instant::now();

    // Only catches a dead capture. Room tone still transcribes to confident
    // nonsense ("Uh", "See no lay no") and still gets pasted - see SILENCE_RMS
    // for why that needs VAD rather than a louder threshold.
    //
    // Nobody spoke. Checked before recognition rather than after, because the
    // recogniser is confident either way: room tone came back as "Oh" and "Yeah."
    // and no threshold on how loud it was could tell those from a real "Yeah."
    // What separates them is whether the level moved. See audio::sounds_like_speech.
    if early.is_empty() && !audio::sounds_like_speech(&samples) {
        eprintln!(
            "({spoken:.1}s, {level}, no voice - skipped: swing {:.1}x)",
            audio::swing(&samples)
        );
        return Ok(());
    }

    // A silent tail is only nothing when nothing came before it: the recording may
    // have ended in the pause that let its earlier half be transcribed already.
    let tail = if rms < audio::SILENCE_RMS {
        if early.is_empty() {
            eprintln!("({spoken:.1}s, {level}, no signal - skipped)");
            return Ok(());
        }
        String::new()
    } else {
        engine.transcribe(samples)?
    };
    let transcribed = started.elapsed();

    // Pieces in the order they were spoken, the tail last.
    let pieces = early.len();
    let mut spoken_text = early;
    if !tail.trim().is_empty() {
        spoken_text.push(tail);
    }
    let text = spoken_text.join(" ");
    let head = if pieces > 0 {
        format!(" ({pieces} early)")
    } else {
        String::new()
    };

    if text.is_empty() {
        eprintln!("({spoken:.1}s, {level}, nothing recognised)");
        return Ok(());
    }

    // Held the key and hesitated. Pasting "Um" would be noise and sending it to
    // the model produced worse - it deleted the filler and then answered "None."
    if cleanup::is_only_filler(&text) {
        eprintln!("({spoken:.1}s, {level}, only hesitation - skipped: {text:?})");
        return Ok(());
    }

    // Words, and cleanup is about to take a while. Everything above this point
    // returns without drawing anything, so a cough gets no spinner.
    island.working();

    // A cleanup failure must never cost the user their words, so the raw
    // transcript stands in whenever the model errors or returns nothing.
    let final_text = match cleaner {
        Some(cleaner) => match cleaner.clean(&text) {
            Ok(cleaned) if !cleaned.trim().is_empty() => cleaned,
            Ok(_) => {
                eprintln!("cleanup returned nothing, using raw transcript");
                text.clone()
            }
            Err(err) => {
                eprintln!("cleanup failed ({err}), using raw transcript");
                text.clone()
            }
        },
        None => text.clone(),
    };
    let cleaned_at = started.elapsed();

    injector.inject(&final_text, terminal)?;
    // Injection is timed because it was once the largest term of the three and
    // nothing pointed at it: a device probe on every paste, invisible in a total.
    let injected = started.elapsed() - cleaned_at;

    if final_text == text {
        eprintln!(
            "{spoken:.1}s{head} {level} -> {transcribed:?} stt, {injected:?} paste, {:?} total: {final_text}",
            started.elapsed()
        );
    } else {
        eprintln!(
            "{spoken:.1}s{head} {level} -> {transcribed:?} stt, {:?} clean, {injected:?} paste, {:?} total\n  raw:   {text}\n  clean: {final_text}",
            cleaned_at - transcribed,
            started.elapsed()
        );
    }
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
