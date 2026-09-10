use anyhow::{Context, Result, bail};
use flow::{
    audio, config, daemon, inject, install, ipc, notify, refine, router, status, stt, tray, wav,
};
use std::time::{Duration, Instant};

/// Audio that must be spoken before any of it is transcribed early. Long enough
/// How long a bare `flow` records. Long enough to say a sentence into, short
/// enough that running it by accident is not a wait.
const DEFAULT_RECORD_SECONDS: u64 = 5;

const USAGE: &str = "\
flow - hold a key, talk, and the text appears where your cursor is

USAGE
    flow [FLAGS] [COMMAND]

COMMANDS
    daemon           Watch the hotkey and dictate. What flow.service runs.
    tray             Publish the system tray icon. What flow-tray.service runs.
    start | stop     Begin or end a dictation without holding the chord
    install          Seed the config and vocabulary templates
    probe            Check the OpenRouter connection and model routes
    logs [ARGS..]    The daemon's journal. Arguments go straight to journalctl,
                     so `flow logs -f` and `flow logs --since today` both work.
    retry [N]        Replay a saved dictation, counting back from the newest.
                     Needs record_debug in the config.
    inject [TEXT]    Type text after 3s, to test injection on its own
    SECONDS          Record, transcribe and print. Default 5.
    FILE.wav         Transcribe a file and time the recogniser
    help             This text
    version          Print the version

FLAGS
    --raw            Skip the refining model for this run
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
    if args
        .iter()
        .any(|arg| arg == "version" || arg == "--version" || arg == "-V")
    {
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
        Some("install") => return install::run(),
        // There is nothing left to verify: the models it used to hash are
        // gone, and whether Flow can actually work is a question about the key
        // and the network, which `probe` answers.
        Some("check") => return probe(),
        Some("probe") => return probe(),
        Some("logs") => return logs(&args[1..]),
        _ => {}
    }

    let settings = config::Config::load().overridden_by(&args);

    // The tray is a controller for the daemon, not part of it. It deliberately
    // returns before either model check so a damaged or stopped dictation
    // service can still be opened and repaired from its icon.
    if command(&args) == Some("tray") {
        return tray::run(settings);
    }

    // Isolates injection from the mic and the model, so a silent uinput failure
    // is distinguishable from a transcription problem.
    if args.first().map(String::as_str) == Some("inject") {
        let text = args.get(1).cloned().unwrap_or_else(|| "flow test".into());
        eprintln!("focus a text field - injecting in 3s");
        std::thread::sleep(Duration::from_secs(3));
        return inject::Injector::new()?.inject(&text);
    }

    // The key is what the models used to be: without it there is nothing to
    // dictate to, and a daemon that starts anyway is one whose hotkey silently
    // does nothing. Under systemd this notification is the difference between
    // a unit that failed for a reason and one that never came up.
    //
    // Only the daemon. `flow foo.wav` and `flow retry` are diagnostics, and
    // both report their own failure with the request that made it - refusing
    // to start them here would hide the reason behind a second one.
    if matches!(command(&args), Some("daemon")) && settings.openrouter_key.is_none() {
        notify::failure(
            "Flow can't start",
            "No OpenRouter key. Add one in Settings.",
        );
        bail!("no OpenRouter key - add one in the console's Settings screen");
    }
    // Bound before the daemon starts, not after it. The console shows
    // "Starting…" from the moment it asks systemd for a start, and the only
    // thing that can honestly end that is this process saying so. The reporter
    // already opens in `Starting` and sends that snapshot to whoever connects,
    // so the window has something true to show for the whole wait.
    //
    // Only for the daemon: `spawn` unlinks the socket path before it binds, so
    // a `flow retry` doing this would cut the running daemon off from the
    // console it was talking to.
    let reporter = matches!(command(&args), Some("daemon")).then(status::Reporter::spawn);
    let mut engine = stt::Stt::new(settings.openrouter_key.clone().unwrap_or_default());

    match command(&args) {
        Some(path) if path.ends_with(".wav") => benchmark(&mut engine, path),
        Some("retry") => {
            let back = args.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
            retry(&mut engine, back, settings.cleanup)
        }
        Some("daemon") => {
            // Built whatever the level says, including `none`.
            //
            // The level is a live setting, so a daemon that only built a
            // refiner at Light could never honour a later switch - which is
            // exactly the trap the old `refine = false` had. Construction is
            // now just holding the key, so there is nothing left to make
            // conditional.
            let refiner = Some(refine::Refiner::new(
                settings.openrouter_key.clone().unwrap_or_default(),
            ));
            // Shared so the file watcher can swap in new values while the
            // daemon runs. Hold vs tap is read per event from that; --no-ptt
            // is the one thing that still decides whether the watcher thread
            // exists at all.
            let live = std::sync::Arc::new(std::sync::Mutex::new(settings.clone()));
            config::watch(std::sync::Arc::clone(&live));
            daemon::run(
                &mut engine,
                settings.chord.clone(),
                refiner,
                live,
                !args.iter().any(|arg| arg == "--no-ptt"),
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
/// So `flow --denoise 5` and `flow 5 --denoise` mean the same thing. Flags
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
fn retry(engine: &mut stt::Stt, back: usize, cleanup: refine::Cleanup) -> Result<()> {
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

    let refiner = refine::Refiner::new(config::Config::load().openrouter_key.unwrap_or_default());
    match refiner.refine(&raw_text, &refine::Style::current(cleanup)) {
        Ok(refined) => println!("refined   {refined}"),
        Err(err) => eprintln!("refining failed: {err}"),
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

/// Whether OpenRouter answers, as `key<TAB>value` lines.
///
/// The console asks the daemon binary rather than calling OpenRouter itself
/// because the key lives in the daemon's config and the answer has to be the
/// one dictation would actually get - a window that checked its own way could
/// report connected while the hotkey fails.
///
/// A real request, not a reachability ping. Missing, rejected and out-of-credit
/// keys all fail here, and only the last of those is invisible to a socket
/// test.
fn probe() -> Result<()> {
    let key = config::Config::load().openrouter_key;
    match key {
        None => {
            println!("router\tNo key");
            println!("detail\tAdd an OpenRouter key in Settings");
        }
        Some(key) => {
            if refine::Refiner::new(key).reachable() {
                println!("router\tConnected");
                println!("detail\t{} · {}", router::STT_MODEL, router::REFINE_MODEL);
            } else {
                println!("router\tDisconnected");
                println!("detail\tOpenRouter did not answer - check the key and the network");
            }
        }
    }
    Ok(())
}

fn record_once(engine: &mut stt::Stt, seconds: u64) -> Result<()> {
    use cpal::traits::DeviceTrait;

    let device = audio::open_device()?;
    eprintln!(
        "input: {}",
        device.id().map(|i| i.to_string()).unwrap_or_default()
    );
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

#[cfg(test)]
mod tests {
    use super::{command, wants_usage};

    fn args(line: &str) -> Vec<String> {
        line.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn flags_do_not_hide_the_command() {
        assert_eq!(command(&args("daemon")), Some("daemon"));
        assert_eq!(command(&args("--denoise 5")), Some("5"));
        assert_eq!(command(&args("5 --denoise")), Some("5"));
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
