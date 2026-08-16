use anyhow::{bail, Result};
use flow::{
    audio, cleanup, config, denoise, duck, hotkey, inject, install, ipc, overlay, status, stt, wav,
};
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
            // Shared so the file watcher can swap in new values while the
            // daemon runs. The chord and push_to_talk are read once below:
            // both own a thread that would have to be torn down and rebuilt,
            // which is a restart's job.
            let live = std::sync::Arc::new(std::sync::Mutex::new(settings.clone()));
            config::watch(std::sync::Arc::clone(&live));
            daemon(
                &mut engine,
                settings.chord.clone(),
                settings.push_to_talk,
                cleaner,
                live,
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
/// The live config, shared with the file watcher. Read at the point of use so a
/// change lands on the next dictation rather than the next restart.
type Live = std::sync::Arc<std::sync::Mutex<config::Config>>;

fn daemon(
    engine: &mut stt::Stt,
    chord: hotkey::Chord,
    ptt: bool,
    cleaner: Option<cleanup::Cleaner>,
    live: Live,
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
    let reporter = status::Reporter::spawn();

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
    // Everything that could still fail has happened by here, so this is the
    // first honest moment to tell a watching console the daemon is up.
    reporter.ready();

    let (jobs, job_rx) = std::sync::mpsc::channel();
    let island = &overlay;
    let status = &reporter;
    // Both the job thread and the event loop read the live config, so they
    // share a borrow rather than the Arc being moved into the first one.
    let live = &live;
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
            while let Ok(samples) = job_rx.recv() {
                // Waits out any in-flight early piece, which is also what
                // guarantees its text is already in `early` before this reads it.
                let mut engine = engine.lock().expect("stt engine");
                let done = std::mem::take(&mut *early.lock().expect("early transcripts"));
                if let Err(err) = handle(
                    *engine,
                    &mut injector,
                    samples,
                    done,
                    cleaner.as_ref(),
                    island,
                    status,
                    live,
                ) {
                    // The console shows this until the next dictation lands, so
                    // a failure the user would otherwise only find in the
                    // journal has somewhere to appear.
                    status.problem(err.to_string());
                    eprintln!("{err}");
                }
                drop(engine);
                // The island is showing the sweep until the text lands, error
                // or not - a failed transcription must not leave it spinning.
                island.finish();
                status.ready();
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
                    // A press while a session is already live is the physical
                    // chord catching up with a `flow start`, or key repeat -
                    // not a new dictation. `Event::Start` has always guarded
                    // this way; this arm did not, so the second begin replaced
                    // the live session and threw away everything said up to
                    // that point. Resetting hold_started was the other half of
                    // the bug: it restarted the clock, so a real hold could
                    // then measure under MIN_HOLD and be discarded as a tap.
                    if session.is_none() {
                        hold_started = Some(Instant::now());
                        begin(&capture, &mut session, live, &overlay, &reporter, early);
                    }
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
                        begin(&capture, &mut session, live, &overlay, &reporter, early);
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
                (Some(_), _, _) => {
                    overlay.queued();
                    reporter.working();
                }
                // A recording ended with nothing usable - a cancel, a tap too
                // short. End the island here or it would sweep forever.
                (None, false, true) => {
                    overlay.cancel();
                    reporter.ready();
                }
                // Nothing started and nothing ended: a stray `flow stop`, a
                // duplicate start. The island may be sweeping for a dictation
                // still being transcribed, so it is not ours to take down.
                (None, false, false) => {}
                (None, true, _) => {}
            }

            if let Some(samples) = finished
                && jobs.send(samples).is_err()
            {
                break;
            }
        }
        drop(jobs);
        listening.store(false, std::sync::atomic::Ordering::Relaxed);
    });

    ipc::remove_pid();
    status::remove_socket();
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
    live: &Live,
    overlay: &overlay::Overlay,
    reporter: &status::Reporter,
    early: &std::sync::Mutex<Vec<String>>,
) {
    // Read once, here, so the whole of this recording uses one consistent set
    // of values even if the file changes while it runs.
    let (duck, settle) = {
        let config = live.lock().expect("config");
        (
            config.ducking(),
            Duration::from_millis(config.duck_settle_ms),
        )
    };
    // A new recording abandons whatever came before it, including anything already
    // transcribed early - otherwise those words would prepend to this dictation.
    // Reachable with both trigger paths live: a signal starts a session and the
    // physical chord then starts another.
    early.lock().expect("early transcripts").clear();

    // The island goes up first so the key press is acknowledged immediately,
    // even though capture may be a moment behind it while other apps are
    // turned down.
    overlay.record();
    reporter.listening();
    eprintln!("recording...");
    *slot = Some(Session { ducker: None });

    let mut ducked = false;
    if let Some(percent) = duck {
        match duck::Ducker::duck(percent) {
            Ok(ducker) => {
                ducked = true;
                if let Some(session) = slot.as_mut() {
                    session.ducker = Some(ducker);
                }
            }
            Err(err) => eprintln!("could not duck other apps: {err}"),
        }
    }

    // Ducking used to happen *after* capture started, so the opening moments of
    // every recording held whatever was playing at full volume - and the
    // pre-roll ring, being the 200ms before the key went down, could never be
    // anything else. Both are dropped here: turn the room down, let the volume
    // ramp land, then start listening.
    if ducked {
        std::thread::sleep(settle);
        capture.begin_without_pre_roll();
    } else {
        // Nothing was turned down, so there is nothing to wait for and the
        // pre-roll is pure gain: it catches a first word spoken on the press.
        capture.begin();
    }
}

fn handle(
    engine: &mut stt::Stt,
    injector: &mut inject::Injector,
    samples: Vec<f32>,
    early: Vec<String>,

    cleaner: Option<&cleanup::Cleaner>,
    island: &overlay::Overlay,
    reporter: &status::Reporter,
    live: &Live,
) -> Result<()> {
    // One read for the whole of this dictation, so a file change part-way
    // through cannot clean the text but paste it with the other chord.
    let (terminal, denoise_audio, record_debug, cleanup_wanted) = {
        let config = live.lock().expect("config");
        (
            config.terminal,
            config.denoise,
            config.record_debug,
            config.cleanup,
        )
    };
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
        // A/B pair. Recording happens BEFORE denoise so the raw wav is exactly
        // what the mic gave us and the denoised wav is exactly what the model
        // saw. Both write on best-effort - a full disk must not lose the
        // dictation. Naming uses a monotonic counter under a start-time root,
        // so files sort by utterance and never collide across a single run.
        let denoised = denoise_audio.then(|| denoise::denoise_16k_mono(&samples));
        if record_debug {
            let dir = debug_recording_dir();
            let n = next_recording_index();
            if let Err(err) = wav::write_16k_mono(dir.join(format!("{n:04}_raw.wav")), &samples) {
                eprintln!("record_debug: raw wav write failed: {err:#}");
            }
            if let Some(denoised) = denoised.as_ref() {
                if let Err(err) =
                    wav::write_16k_mono(dir.join(format!("{n:04}_denoised.wav")), denoised)
                {
                    eprintln!("record_debug: denoised wav write failed: {err:#}");
                }
            }
        }
        engine.transcribe(denoised.unwrap_or(samples))?
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
    //
    // `cleanup_wanted` is read live, so turning it off takes effect on the next
    // dictation. Turning it back on only works if the model was loaded at
    // startup - loading one here would stall the paste for several seconds,
    // which is exactly the trade this whole path refuses to make.
    let final_text = match cleaner.filter(|_| cleanup_wanted) {
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

    reporter.finished(status::Dictation {
        text: final_text.clone(),
        spoken,
        paste_ms: injected.as_millis(),
    });

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

/// Where `record_debug` dumps its A/B pairs. A fresh directory per daemon
/// start (`recordings/{unix_ts}/`) keeps runs from interleaving in the same
/// folder and makes it obvious which pair belongs to which session.
fn debug_recording_dir() -> std::path::PathBuf {
    use std::sync::OnceLock;
    static DIR: OnceLock<std::path::PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        stt::data_home().join(format!("flow/recordings/{ts}"))
    })
    .clone()
}

/// Monotonic per-utterance index for filenames. Reset per daemon start
/// alongside the directory, so pairs stay grouped inside their folder.
fn next_recording_index() -> u32 {
    use std::sync::atomic::{AtomicU32, Ordering};
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    COUNTER.fetch_add(1, Ordering::Relaxed)
}
