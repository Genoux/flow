//! The long-running process behind `flow daemon`: it owns the engine, the
//! chord watcher, and the overlay, and turns a held key into injected text.
//!
//! Lifted out of `main.rs`, which had grown to host both the CLI and this. The
//! split is not only for length: as a library module the daemon is reachable
//! from `tests/`, which it was not while it lived in the binary.

use crate::{
    audio, config, denoise, duck, history, hotkey, inject, ipc, notify, overlay, refine, status,
    stt, wav,
};
use anyhow::Result;
use std::time::{Duration, Instant};

/// that short dictations never pay an extra encoder pass, short enough that a
/// rambling one gets several pieces done before the key comes up.
const PREFIX_MIN: usize = 8 * audio::SAMPLE_RATE as usize;

/// How often to look for a pause worth cutting at. The speaker is still talking,
/// so there is nothing to race.
const PREFIX_POLL: Duration = Duration::from_millis(400);

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
pub type Live = std::sync::Arc<std::sync::Mutex<config::Config>>;

pub fn run(
    engine: &mut stt::Stt,
    chord: hotkey::Chord,
    refiner: Option<refine::Refiner>,
    live: Live,
    watch_keys: bool,
    reporter: status::Reporter,
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
    // Degrades rather than bails, the way a missing refining model does: the signal
    // path is independent, so a machine without /dev/input access should still
    // dictate from a compositor bind instead of refusing to start.
    // Shared with the config watcher, so rebinding the chord in the console
    // reaches this running daemon instead of waiting for a restart.
    let chord = std::sync::Arc::new(std::sync::Mutex::new(chord));
    {
        let chord = std::sync::Arc::clone(&chord);
        let live = std::sync::Arc::clone(&live);
        std::thread::spawn(move || {
            loop {
                std::thread::sleep(Duration::from_millis(400));
                let wanted = live.lock().expect("config").chord.clone();
                let mut current = chord.lock().expect("chord");
                if *current != wanted {
                    *current = wanted;
                }
            }
        });
    }
    // Spawned whichever way the chord is meant to work. Hold and tap are the
    // same key stream read differently, decided per event from the live config
    // below, so that setting no longer decides whether this thread exists - and
    // so no longer needs a restart to change. `--no-ptt` still does: it is the
    // escape hatch for "I have a compositor bind, leave the keys alone".
    let mut watching = watch_keys;
    if watch_keys && let Err(err) = hotkey::spawn(events.clone(), std::sync::Arc::clone(&chord)) {
        // The chord is the only way most people ever start a dictation, so
        // losing it looks exactly like Flow not running at all. Name the usual
        // cause: reading /dev/input needs membership of the input group.
        notify::failure(
            "Flow: the chord is not being watched",
            "Add yourself to the `input` group and log back in, or start \
             dictation with `flow start`.",
        );
        eprintln!("chord watcher disabled: {err}");
        watching = false;
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
        if watching {
            let chord = chord.lock().expect("chord");
            if live.lock().expect("config").push_to_talk {
                format!("hold {chord}, or trigger `flow start`")
            } else {
                format!("tap {chord} to start and again to stop, or trigger `flow start`")
            }
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
                    refiner.as_ref(),
                    island,
                    status,
                    live,
                ) {
                    // The console shows this until the next dictation lands, so
                    // a failure the user would otherwise only find in the
                    // journal has somewhere to appear. The notification is for
                    // when the console isn't open, which is nearly always.
                    //
                    // The clipboard line is not a guess: inject() stages the
                    // text before it touches the keyboard, so anything that
                    // fails from there on leaves it recoverable with Ctrl+V.
                    status.problem(err.to_string());
                    notify::failure(
                        "Dictation failed",
                        "If any text was recognised it is on your clipboard - press Ctrl+V.",
                    );
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
                let Some(prefix) = recording.take_prefix(PREFIX_MIN) else {
                    continue;
                };
                let spoken = prefix.len() as f32 / audio::SAMPLE_RATE as f32;
                let started = Instant::now();
                match engine.transcribe(prefix) {
                    // Pushed while still holding the engine, so the release path
                    // cannot read a partial set of pieces.
                    Ok(text) if !text.trim().is_empty() => {
                        crate::verbose!(
                            "transcribed {spoken:.1}s early in {:?}",
                            started.elapsed()
                        );
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
        // Last time tap-to-talk actually began or finished a session. Native
        // press and compositor `flow start` of the same tap land a few
        // milliseconds apart; without this the second one would toggle back.
        let mut last_tap: Option<Instant> = None;
        // An event peeked while debouncing a release (see `RELEASE_DEBOUNCE`
        // below) that turned out not to be chatter, and so still needs
        // handling on the next iteration instead of being dropped.
        let mut pending: Option<hotkey::Event> = None;
        loop {
            let event = match pending.take() {
                Some(event) => event,
                None => match incoming.recv() {
                    Ok(event) => event,
                    Err(_) => break,
                },
            };
            let was_recording = session.is_some();
            // Set by the paths where the user asked for a dictation and got
            // nothing back. Distinct from a cancel, which they asked for.
            let mut missed = false;
            // Hold or tap, read per event from the live config so switching
            // the two in the console lands on the next press. It cannot be
            // read once at startup: that is what used to make this setting
            // need a restart.
            let hold_to_talk = live.lock().expect("config").push_to_talk;
            let echo = last_tap.is_some_and(|at| at.elapsed() < hotkey::TAP_ECHO);
            // Ending a session drops its ducker, so other apps come back to volume
            // as soon as recording stops rather than after transcription.
            let finished = match event {
                // Tap to talk: the chord (and `flow start`) is a switch.
                // Release - including Hyprland's USR2 bind on the same chord -
                // must not end the session, or the setting does nothing.
                event @ (hotkey::Event::Pressed | hotkey::Event::Start) if !hold_to_talk => {
                    if let Some(watch) = chord_watch.take() {
                        watch.disarm();
                    }
                    match hotkey::tap_action(event, session.is_some(), echo) {
                        hotkey::TapAction::Ignore => None,
                        hotkey::TapAction::Finish => {
                            last_tap = Some(Instant::now());
                            hold_started.take();
                            session.take().map(|s| s.finish(&capture))
                        }
                        hotkey::TapAction::Begin => {
                            last_tap = Some(Instant::now());
                            hold_started = None;
                            if begin(
                                &capture,
                                &mut session,
                                live,
                                &overlay,
                                &reporter,
                                early,
                                &incoming,
                            )
                            .is_some()
                            {
                                last_tap = None;
                            }
                            None
                        }
                    }
                }
                hotkey::Event::Released { .. } | hotkey::Event::Stop if !hold_to_talk => None,
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
                        if begin(
                            &capture,
                            &mut session,
                            live,
                            &overlay,
                            &reporter,
                            early,
                            &incoming,
                        )
                        .is_some()
                        {
                            hold_started = None;
                        }
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
                        if begin(
                            &capture,
                            &mut session,
                            live,
                            &overlay,
                            &reporter,
                            early,
                            &incoming,
                        )
                        .is_none()
                        {
                            chord_watch = Some(hotkey::ChordWatch::arm(
                                events.clone(),
                                chord.lock().expect("chord").clone(),
                            ));
                        } else {
                            hold_started = None;
                        }
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
                    // A release this close to a re-press is chatter, not a
                    // deliberate let-go: keyd (or the keyboard itself) has
                    // been observed to emit a spurious up/down pair for the
                    // trigger key while it is still physically held, which
                    // used to tear a long, continuous dictation into a burst
                    // of fragments every one of which was short enough to be
                    // thrown away as an accidental tap. Peeking one event
                    // ahead and swallowing the pair keeps the same session -
                    // and the audio already in it - running straight through
                    // the blip instead.
                    if session.is_some()
                        && let Ok(next) = incoming.recv_timeout(RELEASE_DEBOUNCE)
                    {
                        if matches!(next, hotkey::Event::Pressed | hotkey::Event::Start) {
                            continue;
                        }
                        pending = Some(next);
                    }
                    // The true length of the hold, timed from the original
                    // press - not `held`, which only covers time since the
                    // last spurious re-press if any chatter was absorbed
                    // above, and would otherwise read as a tap every time.
                    let total = hold_started.take().map_or(held, |at| at.elapsed());
                    match session.take().map(|s| s.finish(&capture)) {
                        Some(samples) if hotkey::was_long_enough(total) => Some(samples),
                        Some(_) => {
                            early.lock().expect("early transcripts").clear();
                            eprintln!("discarded: {total:?} is too short to be a deliberate hold");
                            missed = true;
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
                // A recording ended with nothing usable. End the island here or
                // it would sweep forever - and say so, unless the user is the one
                // who called it off. Another key turning the hold into a shortcut
                // was deliberate; a tap too short to register was not, and leaves
                // them waiting for text that is never coming.
                (None, false, true) => {
                    match missed {
                        true => overlay.missed(),
                        false => overlay.cancel(),
                    }
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

/// How often to check whether the duck has settled while waiting to open the
/// mic. Cheap - just an atomic load - so this can be short without cost.
const ARM_POLL: Duration = Duration::from_millis(5);

/// How long a release is given to prove itself deliberate before the session
/// is actually torn down. Keyd - or the keyboard itself - has been observed
/// emitting a spurious release/re-press of the trigger key while it is still
/// physically held, in bursts 45-215ms apart; a real, considered re-press
/// (a new dictation right after the last one) does not happen anywhere near
/// that fast. Above the observed range with room to spare, still far under
/// the pause someone would actually leave between two separate holds.
const RELEASE_DEBOUNCE: Duration = Duration::from_millis(250);

/// Arms a recording, or - if the hold ends before the duck has actually
/// settled - aborts before the microphone ever opens. Returns the elapsed
/// hold when it aborted, so the caller logs a normal (if very short) release
/// rather than treating it as a completed dictation.
///
/// Waiting used to be a blind `sleep(duck_settle_ms)`, a value with no
/// principled setting: the ramp itself (`FADE_OUT` in duck.rs) always takes
/// the same fixed time, so guessing a number to sleep for was pure noise, and
/// a release that landed inside that sleep queued silently - the mic opened
/// anyway, capturing whatever the room sounded like after the user had
/// already let go. Polling `Ducker::settled` waits for the one real
/// condition, and reacts to a release on the spot instead of after the fact.
fn begin(
    capture: &audio::Capture,
    slot: &mut Option<Session>,
    live: &Live,
    overlay: &overlay::Overlay,
    reporter: &status::Reporter,
    early: &std::sync::Mutex<Vec<String>>,
    incoming: &std::sync::mpsc::Receiver<hotkey::Event>,
) -> Option<Duration> {
    let (duck, hold_to_talk) = {
        let config = live.lock().expect("config");
        (config.ducking(), config.push_to_talk)
    };

    // A new recording abandons whatever came before it, including anything already
    // transcribed early - otherwise those words would prepend to this dictation.
    // Reachable with both trigger paths live: a signal starts a session and the
    // physical chord then starts another.
    early.lock().expect("early transcripts").clear();

    // Up immediately so the key press is acknowledged, but armed rather than
    // listening: the microphone is not open until the ducking has settled, and
    // an island showing live bars that cannot move reads as a dead mic. It
    // spins until there is something to hear.
    overlay.arm();
    reporter.listening();
    eprintln!("recording...");
    *slot = Some(Session { ducker: None });

    let started = Instant::now();
    let mut ducker = None;
    if let Some(percent) = duck {
        match duck::Ducker::duck(percent) {
            Ok(d) => ducker = Some(d),
            Err(err) => eprintln!("could not duck other apps: {err}"),
        }
    }

    while ducker.as_ref().is_some_and(|d| !d.settled()) {
        match incoming.recv_timeout(ARM_POLL) {
            // Only a hold can be released too early. In tap mode the key is
            // let go a moment after the tap that started this, and treating
            // that as an abort would cancel every dictation before the mic
            // ever opened.
            Ok(hotkey::Event::Released { held }) if hold_to_talk => {
                *slot = None;
                overlay.missed();
                reporter.ready();
                eprintln!("released before the mic opened ({held:?}) - nothing recorded");
                return Some(held);
            }
            Ok(hotkey::Event::Stop) if hold_to_talk => {
                *slot = None;
                overlay.missed();
                reporter.ready();
                eprintln!("discarded: the hold ended before the mic opened");
                return Some(started.elapsed());
            }
            Ok(hotkey::Event::Cancelled) => {
                *slot = None;
                overlay.cancel();
                reporter.ready();
                eprintln!("discarded: the hold ended before the mic opened");
                return Some(started.elapsed());
            }
            // Key-repeat or a duplicate start while already arming - the same
            // thing the top-level loop already ignores once a session exists.
            Ok(_) | Err(_) => {}
        }
    }

    if let Some(session) = slot.as_mut() {
        session.ducker = ducker;
    }

    // Ducking used to happen *after* capture started, so the opening moments of
    // every recording held whatever was playing at full volume - and the
    // pre-roll ring, being the 200ms before the key went down, could never be
    // anything else. Both are dropped here: turn the room down, wait for the
    // ramp to land, then start listening.
    if duck.is_some() {
        capture.begin_without_pre_roll();
    } else {
        // Nothing was turned down, so there is nothing to wait for and the
        // pre-roll is pure gain: it catches a first word spoken on the press.
        capture.begin();
    }

    // The microphone is open. This is the moment the bars stop breathing and
    // start answering, which is the only honest signal that speaking will be
    // heard - and it lands here rather than on the keypress precisely because
    // that is when it became true.
    overlay.record();
    None
}

// One dictation needs all of this, and every parameter is a distinct
// collaborator rather than a field of some shared thing. Bundling them into a
// context struct would move the list rather than shorten it.
#[allow(clippy::too_many_arguments)]
fn handle(
    engine: &mut stt::Stt,
    injector: &mut inject::Injector,
    samples: Vec<f32>,
    early: Vec<String>,

    refiner: Option<&refine::Refiner>,
    island: &overlay::Overlay,
    reporter: &status::Reporter,
    live: &Live,
) -> Result<()> {
    // One read for the whole of this dictation, so a file change part-way
    // through cannot refine the text but paste it with the other chord.
    let (terminal, denoise_audio, record_debug, cleanup) = {
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
            // Not "you were quiet" - the samples are flat, so nothing reached
            // the mic at all. The user deliberately held the chord past
            // MIN_HOLD expecting text, and a muted source is the usual cause.
            notify::failure(
                "Flow heard nothing",
                "The microphone delivered silence. Check it isn't muted, and \
                 that the system default input is the one you're speaking into.",
            );
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
            if let Some(denoised) = denoised.as_ref()
                && let Err(err) =
                    wav::write_16k_mono(dir.join(format!("{n:04}_denoised.wav")), denoised)
            {
                eprintln!("record_debug: denoised wav write failed: {err:#}");
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
    if refine::is_only_filler(&text) {
        eprintln!("({spoken:.1}s, {level}, only hesitation - skipped: {text:?})");
        return Ok(());
    }

    // Words, and refining is about to take a while. Everything above this point
    // returns without drawing anything, so a cough gets no spinner.
    island.working();

    // A refining failure must never cost the user their words, so the raw
    // transcript stands in whenever the model errors or returns nothing.
    //
    // `cleanup` is read live, so lowering it takes effect on the next dictation.
    // Raising it off `none` only works if the model was loaded at startup -
    // loading one here would stall the paste for several seconds, which is
    // exactly the trade this whole path refuses to make.
    let final_text = match refiner.filter(|_| cleanup.wants_model()) {
        Some(refiner) => match refiner.refine(&text, cleanup) {
            Ok(refined) if !refined.trim().is_empty() => refined,
            Ok(_) => {
                eprintln!("refining returned nothing, using raw transcript");
                text.clone()
            }
            Err(err) => {
                eprintln!("refining failed ({err}), using raw transcript");
                text.clone()
            }
        },
        None => text.clone(),
    };
    let refined_at = started.elapsed();

    injector.inject(&final_text, terminal)?;
    // Injection is timed because it was once the largest term of the three and
    // nothing pointed at it: a device probe on every paste, invisible in a total.
    let injected = started.elapsed() - refined_at;

    reporter.finished(status::Dictation {
        text: final_text.clone(),
        spoken,
        paste_ms: injected.as_millis(),
    });
    // On disk as well as in the reporter: the console reads history from the
    // file, so it is there before the daemon starts and survives it stopping.
    history::append(
        &final_text,
        &text,
        spoken,
        injected.as_millis(),
        history::now(),
    );

    if final_text == text {
        eprintln!(
            "{spoken:.1}s{head} {level} -> {transcribed:?} stt, {injected:?} paste, {:?} total: {final_text}",
            started.elapsed()
        );
    } else {
        eprintln!(
            "{spoken:.1}s{head} {level} -> {transcribed:?} stt, {:?} refine, {injected:?} paste, {:?} total\n  raw:   {text}\n  refined: {final_text}",
            refined_at - transcribed,
            started.elapsed()
        );
    }
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
        flow_paths::recordings_dir().join(ts.to_string())
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
