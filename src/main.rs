use anyhow::{bail, Context, Result};
use flow::{
    audio, config, denoise, duck, history, hotkey, inject, install, ipc, notify, overlay, refine,
    status, stt, wav,
};
use std::time::{Duration, Instant};

/// Audio that must be spoken before any of it is transcribed early. Long enough
/// that short dictations never pay an extra encoder pass, short enough that a
/// rambling one gets several pieces done before the key comes up.
const PREFIX_MIN: usize = 8 * audio::SAMPLE_RATE as usize;

/// How often to look for a pause worth cutting at. The speaker is still talking,
/// so there is nothing to race.
const PREFIX_POLL: Duration = Duration::from_millis(400);

/// How long a bare `flow` records. Long enough to say a sentence into, short
/// enough that running it by accident is not a wait.
const DEFAULT_RECORD_SECONDS: u64 = 5;

const USAGE: &str = "\
flow - hold a key, talk, and the text appears where your cursor is

USAGE
    flow [FLAGS] [COMMAND]

COMMANDS
    daemon           Watch the hotkey and dictate. What flow.service runs.
    start | stop     Begin or end a dictation without holding the chord
    install          Download the speech and refining models
    probe            Where refining would run on this machine
    logs [ARGS..]    The daemon's journal. Arguments go straight to journalctl,
                     so `flow logs -f` and `flow logs --since today` both work.
    retry [N]        Replay a saved dictation, counting back from the newest.
                     Needs record_debug in the config.
    inject [TEXT]    Type text after 3s, to test injection on its own
    overlay [SECS]   Show the island on its own
    SECONDS          Record, transcribe and print. Default 5.
    FILE.wav         Transcribe a file and time the recogniser
    help             This text
    version          Print the version

FLAGS
    --speech-only    install: the speech model only
    --refine-only    install: the refining model only
    --porcelain      install: report progress as lines, for the setup screen
    --plan           install: print what would be fetched, and fetch nothing
    --raw            Skip the refining model for this run
    --terminal       Type the text out instead of pasting it
    --no-ptt         Do not watch the hotkey
    --denoise | --no-denoise
    --record-debug   Keep this run's audio for `flow retry`
    --duck PERCENT   Volume of other apps while recording. 0 turns it off.

Configuration    ~/.config/flow/config.toml
Word fixes       ~/.config/flow/vocabulary.txt
Verbose output   FLOW_DEBUG=1
";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Before the config, and before the 640 MB speech model: asking what the
    // commands are should not depend on the tool being installed correctly.
    if wants_usage(&args) {
        print!("{USAGE}");
        return Ok(());
    }

    // Same reason as help, and the same trap: `--version` is all flags, so it
    // reached the catch-all and recorded for five seconds.
    if args.iter().any(|arg| arg == "version" || arg == "--version" || arg == "-V") {
        println!("flow {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }

    // All of these run before the config is read. start/stop must survive a
    // broken config file so a daemon that is already recording can still be
    // stopped, `install` is what writes that file in the first place, and
    // `logs` is how you find out what a daemon that will not start is saying.
    match args.first().map(String::as_str) {
        Some("start") => return ipc::send(ipc::START),
        Some("stop") => return ipc::send(ipc::STOP),
        Some("install") => {
            let want = install::Want::from_args(&args);
            // What an install would fetch, without fetching it. Always the
            // machine-readable rendering: this exists for the Models screen,
            // which has to name a model's size before offering to fetch it.
            if args.iter().any(|a| a == "--plan") {
                install::plan_reported(want, &mut install::to_console);
                return Ok(());
            }
            // The console drives the same installer and needs numbers rather
            // than a bar, so it asks for the machine-readable rendering.
            if args.iter().any(|a| a == "--porcelain") {
                return install::run_reported(want, &mut install::to_console);
            }
            return install::run(want);
        }
        Some("probe") => return probe(),
        Some("logs") => return logs(&args[1..]),
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
        // Same order the daemon uses, so this shows the real arming phase
        // rather than a version of the island that only exists in this branch.
        overlay.arm();
        eprintln!("arming for 2s - the spinner turns while the mic is still shut");
        std::thread::sleep(Duration::from_secs(2));
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
        // Under systemd this is the difference between a daemon that failed
        // for a reason and one that simply never came up: nothing else on the
        // desktop reports a unit that exited before it did any work.
        notify::failure(
            "Flow can't start",
            "The speech model is missing. Run `flow install` to fetch it.",
        );
        bail!("model not found at {} - run `flow install`", dir.display());
    }
    // The cleanup model is required too, and required even at `cleanup = none`.
    //
    // Not a matter of taste: `cleanup` is read live, once per dictation, but
    // the refiner is loaded once at startup. A daemon that started without the
    // weights can never honour a later switch to Light - the level changes, the
    // output does not, and nothing on screen explains why. Demanding the file
    // here is what makes the Style screen's four levels mean anything at all.
    //
    // Refusing rather than degrading is also the honest version of what Flow
    // now is. Both models arrive together and a machine missing one is a
    // half-finished install, not a smaller Flow - so it stops, says which half
    // is missing, and the console offers to finish the job.
    // Only the daemon. `flow foo.wav` benchmarks the recogniser and `flow
    // retry` re-runs one dictation - both are diagnostics, and refusing to
    // measure STT because a second model is absent would be exactly the kind
    // of unhelpful strictness this gate exists to avoid. `install` returns long
    // before here, so there is no way to need the model in order to fetch it.
    let cleanup_model = refine::model_path();
    if matches!(command(&args), Some("daemon")) && !cleanup_model.is_file() {
        notify::failure(
            "Flow can't start",
            "The cleanup model is missing. Run `flow install` to fetch it.",
        );
        bail!("model not found at {} - run `flow install`", cleanup_model.display());
    }
    // Bound before the loading starts, not after it. The console shows
    // "Starting…" from the moment it asks systemd for a start, and the only
    // thing that can honestly end that is this process saying so. Between here
    // and `ready` sit a 650 MB recogniser, 2.5 GB of refining weights when
    // refining is on, and a two-second microphone warm-up - seconds in which a
    // socket that did not exist yet read as "Flow isn't running" in the middle
    // of Flow starting. The reporter already opens in `Starting` and sends
    // that snapshot to whoever connects, so the window has something true to
    // show for the whole wait.
    //
    // Only for the daemon: `spawn` unlinks the socket path before it binds, so
    // a `flow retry` doing this would cut the running daemon off from the
    // console it was talking to.
    let reporter = matches!(command(&args), Some("daemon")).then(status::Reporter::spawn);
    let mut engine = stt::Stt::load(&dir)?;

    match command(&args) {
        Some(path) if path.ends_with(".wav") => benchmark(&mut engine, path),
        Some("retry") => {
            let back = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            retry(&mut engine, back, settings.cleanup, settings.gpu)
        }
        Some("daemon") => {
            // Loaded whatever the level says, including `none`.
            //
            // The level is a live setting and this is a startup cost, so tying
            // the two together makes the setting a lie in one direction: a
            // daemon started at `none` could never be switched to Light without
            // a restart, which is exactly the trap the old `refine = false`
            // had. Someone who wants the VRAM back turns Flow off, not the dial
            // down.
            //
            // Still not fatal when it will not load, and now that is a real
            // fault rather than a choice: startup already refused to get this
            // far without the file, so reaching here means the weights exist
            // and something else is wrong - a corrupt download, or a card that
            // cannot hold them. Dictation carries on at raw transcripts, which
            // is worth more than no dictation, and the notification says so.
            let refiner = match refine::Refiner::load(
                &refine::model_path(),
                refine::vocabulary(),
                settings.gpu,
            ) {
                Ok(refiner) => {
                    refiner.warm_up();
                    Some(refiner)
                }
                Err(err) => {
                    notify::failure(
                        "Flow: cleanup disabled",
                        "Dictation works, but text will not be cleaned up. \
                         See `flow logs` for why the model would not load.",
                    );
                    eprintln!("cleanup model: {err}");
                    None
                }
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
                refiner,
                live,
                reporter.expect("bound above for the daemon command"),
            )
        }
        None => record_once(&mut engine, DEFAULT_RECORD_SECONDS),
        // A number is a duration; anything else is a mistake and is refused.
        // This used to fall through to `record_once`, so `flow dameon` opened
        // the microphone for five seconds instead of saying it had no idea
        // what that was - the one failure mode a dictation tool must not have.
        Some(other) => match other.parse() {
            Ok(seconds) => record_once(&mut engine, seconds),
            Err(_) => bail!("unknown command {other:?} - try `flow help`"),
        },
    }
}

/// The first argument that is not a flag.
///
/// So `flow --terminal 5` and `flow 5 --terminal` mean the same thing. Flags
/// are read separately by `Config::overridden_by`, which scans the whole list
/// and does not care where they sit.
fn command(args: &[String]) -> Option<&str> {
    args.iter()
        .map(String::as_str)
        .find(|arg| !arg.starts_with('-'))
}

/// Whether the user is asking what the commands are rather than running one.
fn wants_usage(args: &[String]) -> bool {
    args.iter()
        .any(|arg| arg == "help" || arg == "--help" || arg == "-h")
}

/// `flow retry [n]` - put a saved dictation back through the pipeline.
///
/// `record_debug` writes an A/B pair per dictation, raw and denoised, and until
/// now nothing read them back: answering "why did I get that text" meant
/// finding the file by hand and running `flow <path>.wav`, which reports the
/// raw transcript only. The interesting comparisons are exactly the ones that
/// needed the most typing.
///
/// `n` counts back from the newest, so `flow retry` is the last thing you said
/// and `flow retry 3` is three before it.
///
/// Unlike `benchmark` this runs the real pipeline rather than timing the
/// recogniser three times: the transcript people actually receive has been
/// through refining, so a retry that stopped at the raw text would answer a
/// question nobody asked.
fn retry(
    engine: &mut stt::Stt,
    back: usize,
    cleanup: refine::Cleanup,
    gpu: Option<usize>,
) -> Result<()> {
    let takes = recorded_takes()?;
    let Some(raw_path) = takes.iter().rev().nth(back) else {
        bail!(
            "only {} saved dictation(s) - is record_debug on in {}?",
            takes.len(),
            config::path().display()
        );
    };

    eprintln!("{}\n", raw_path.display());
    let raw_text = engine.transcribe(wav::read_16k_mono(raw_path)?)?;
    println!("raw       {raw_text}");

    // The denoised half of the pair, when there is one. This is the whole
    // reason both files are kept: the only way to tell whether the denoiser
    // helped this dictation is to hear the model's answer to both.
    let denoised_path = raw_path.with_file_name(
        raw_path
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .replace("_raw.wav", "_denoised.wav"),
    );
    if denoised_path.is_file() {
        let denoised_text = engine.transcribe(wav::read_16k_mono(&denoised_path)?)?;
        println!("denoised  {denoised_text}");
    }

    if !cleanup.wants_model() {
        return Ok(());
    }
    match refine::Refiner::load(&refine::model_path(), refine::vocabulary(), gpu) {
        Ok(refiner) => match refiner.refine(&raw_text, cleanup) {
            Ok(refined) => println!("refined   {refined}"),
            Err(err) => eprintln!("refining failed: {err}"),
        },
        Err(err) => eprintln!("refining unavailable: {err}"),
    }
    Ok(())
}

/// Every `*_raw.wav` under the recordings root, oldest first.
///
/// Sorted by path, which orders correctly because both halves of the name are
/// zero-padded or fixed-width: the directory is a unix timestamp and the file
/// is a four-digit counter. Walking every session rather than only the newest
/// means `flow retry 20` still reaches back past a daemon restart.
fn recorded_takes() -> Result<Vec<std::path::PathBuf>> {
    let root = flow_paths::recordings_dir();
    let mut takes: Vec<std::path::PathBuf> = std::fs::read_dir(&root)
        .with_context(|| format!("no recordings at {}", root.display()))?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().is_dir())
        .flat_map(|session| std::fs::read_dir(session.path()).into_iter().flatten())
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.to_string_lossy().ends_with("_raw.wav"))
        .collect();
    takes.sort();
    Ok(takes)
}

/// `flow logs` - what the daemon has been saying.
///
/// Everything Flow prints goes to the journal, because it runs as a user unit
/// with no terminal attached. That is only useful if there is an obvious way to
/// read it, and `journalctl --user -u flow.service` is not something anyone
/// should have to remember.
///
/// Arguments are handed to journalctl untouched rather than parsed, so
/// `flow logs -f`, `flow logs --since today` and `flow logs -p err` all work
/// without this knowing they exist.
///
/// `-n 50` goes in front of them rather than only when none were given.
/// journalctl lets the last `-n` win, so an explicit `-n 200` still overrides
/// it, while `flow logs --no-pager` gets a useful tail instead of every line
/// the daemon has ever written - which is what the first version did, because
/// `--no-pager` counted as "the user asked for a range" when it is nothing of
/// the sort.
///
/// Replaces this process rather than spawning a child: journalctl then owns
/// the terminal outright, so its pager behaves, `-f` streams, and Ctrl+C stops
/// it rather than being caught halfway up a process tree.
fn logs(args: &[String]) -> Result<()> {
    use std::os::unix::process::CommandExt;

    // exec only returns when it failed to happen at all.
    Err(std::process::Command::new("journalctl")
        .args(["--user", "-u", "flow.service", "-n", "50"])
        .args(args)
        .exec())
    .context("running journalctl - is this a systemd machine?")
}

/// Where refining would run on this machine, as `key<TAB>value` lines.
///
/// The console cannot answer this itself: enumerating GPUs means llama.cpp,
/// and the whole reason the window is a second binary is that it does not carry
/// that tree. So it asks the daemon binary, which already knows.
///
/// The config is read leniently rather than with `?`. A machine part-way
/// through setup may have no config file at all, and a broken one is a reason
/// to ignore a `gpu = ` override, not a reason to refuse to say what hardware
/// is present.
fn probe() -> Result<()> {
    let gpu = config::Config::load().ok().and_then(|settings| settings.gpu);
    let plan = refine::plan(gpu);

    match plan.device {
        Some(device) => {
            println!("refine\t{}", device.description);
            println!(
                "detail\tVulkan · {:.1} GB free",
                device.free_bytes as f64 / 1e9
            );
        }
        None => {
            println!("refine\tCPU");
            println!(
                "detail\t{}",
                if plan.best_free == 0 {
                    format!("no GPU found · needs {:.1} GB", plan.needed as f64 / 1e9)
                } else {
                    format!(
                        "needs {:.1} GB, the roomiest card has {:.1} GB",
                        plan.needed as f64 / 1e9,
                        plan.best_free as f64 / 1e9
                    )
                }
            );
        }
    }
    Ok(())
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
    refiner: Option<refine::Refiner>,
    live: Live,
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
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_millis(400));
            let wanted = live.lock().expect("config").chord.clone();
            let mut current = chord.lock().expect("chord");
            if *current != wanted {
                *current = wanted;
            }
        });
    }
    let mut ptt = ptt;
    if ptt && let Err(err) = hotkey::spawn(events.clone(), std::sync::Arc::clone(&chord)) {
        // The chord is the only way most people ever start a dictation, so
        // losing it looks exactly like Flow not running at all. Name the usual
        // cause: reading /dev/input needs membership of the input group.
        notify::failure(
            "Flow: push-to-talk disabled",
            "The chord is not being watched. Add yourself to the `input` group \
             and log back in, or start dictation with `flow start`.",
        );
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
            format!("hold {}, or trigger `flow start`", chord.lock().expect("chord"))
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
                let Some(prefix) = recording.take_prefix(PREFIX_MIN) else { continue };
                let spoken = prefix.len() as f32 / audio::SAMPLE_RATE as f32;
                let started = Instant::now();
                match engine.transcribe(prefix) {
                    // Pushed while still holding the engine, so the release path
                    // cannot read a partial set of pieces.
                    Ok(text) if !text.trim().is_empty() => {
                        flow::verbose!("transcribed {spoken:.1}s early in {:?}", started.elapsed());
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
                        if begin(&capture, &mut session, live, &overlay, &reporter, early, &incoming)
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
                        if begin(&capture, &mut session, live, &overlay, &reporter, early, &incoming)
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
    let duck = live.lock().expect("config").ducking();

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
            Ok(hotkey::Event::Released { held }) => {
                *slot = None;
                overlay.cancel();
                reporter.ready();
                eprintln!("released before the mic opened ({held:?}) - nothing recorded");
                return Some(held);
            }
            Ok(hotkey::Event::Cancelled | hotkey::Event::Stop) => {
                *slot = None;
                overlay.cancel();
                reporter.ready();
                eprintln!("discarded: the hold ended before the mic opened");
                return Some(started.elapsed());
            }
            // Key-repeat or a duplicate start while already arming - the same
            // thing the top-level loop already ignores once a session exists.
            Ok(hotkey::Event::Pressed | hotkey::Event::Start) | Err(_) => {}
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

#[cfg(test)]
mod tests {
    use super::{command, wants_usage};

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn flags_do_not_hide_the_command() {
        assert_eq!(command(&args("daemon")), Some("daemon"));
        assert_eq!(command(&args("--terminal 5")), Some("5"));
        assert_eq!(command(&args("5 --terminal")), Some("5"));
        assert_eq!(command(&args("--raw daemon")), Some("daemon"));
        assert_eq!(command(&args("")), None);
        assert_eq!(command(&args("--raw")), None);
    }

    // The regression this exists for: every unknown argument used to fall
    // through to a five-second recording, so a typo opened the microphone.
    #[test]
    fn a_typo_is_not_a_duration() {
        assert!(command(&args("dameon")).is_some_and(|c| c.parse::<u64>().is_err()));
        assert!(command(&args("5")).is_some_and(|c| c.parse::<u64>().is_ok()));
    }

    // A list of nothing but flags yields no command, so it fell through to the
    // default five-second recording. `flow --version` used to open the mic.
    #[test]
    fn flags_only_is_not_a_command() {
        assert_eq!(command(&args("--version")), None);
        assert_eq!(command(&args("-V")), None);
    }

    #[test]
    fn usage_is_asked_for_in_the_three_usual_ways() {
        for line in ["help", "--help", "-h", "daemon --help"] {
            assert!(wants_usage(&args(line)), "{line}");
        }
        for line in ["daemon", "retry 3", ""] {
            assert!(!wants_usage(&args(line)), "{line}");
        }
    }
}
