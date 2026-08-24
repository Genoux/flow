//! Three layers, in order: these defaults, then `~/.config/flow/config.toml`,
//! then command-line flags. A fresh install has no config file and needs none.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    /// Hold the chord while speaking (`true`), or tap it to start and tap it
    /// again to stop (`false`). The chord is watched either way.
    pub push_to_talk: bool,
    /// Percentage of its current volume each other app is held at while
    /// recording. 0 disables ducking.
    pub duck: u32,
    /// How much the refining model may change what you said. `Light` is the
    /// default: every speaker wants their fillers gone, not every speaker wants
    /// their sentences rewritten.
    pub cleanup: super::refine::Cleanup,
    /// Key combination that starts a dictation, held or tapped depending on
    /// `push_to_talk`.
    pub chord: super::hotkey::Chord,
    /// Which GPU runs the refining model. `None` picks the roomiest discrete one,
    /// which is right on every machine tested so far; an index is the escape hatch
    /// for when it is not.
    pub gpu: Option<usize>,
    /// Run RNNoise denoising between capture and STT. Off by default so the A/B
    /// case is the current, known-good behaviour; on, it runs the utterance
    /// through nnnoiseless to strip hiss and fan noise before Parakeet sees it.
    pub denoise: bool,
    /// Play the island's arrive and leave chimes. On by default: the island is
    /// silent feedback, and a dictation started over a full-screen window is
    /// otherwise unacknowledged until the text lands.
    pub sound: bool,
    /// Save every dictation's audio as WAV files to `~/.local/share/flow/recordings/`,
    /// one raw and (when denoise is on) one denoised. Off by default because
    /// long sessions add up on disk fast; on, it is the only way to A/B the
    /// denoiser on the same source audio.
    pub record_debug: bool,
}

impl Default for Config {
    /// Hold-to-talk is on out of the box: Flow watches the chord itself, so a
    /// fresh install dictates with no compositor configuration at all. Off is
    /// the same chord as a tap-on, tap-off switch, not a chord that does
    /// nothing - the keys are watched either way.
    fn default() -> Self {
        Self {
            push_to_talk: true,
            duck: 50,
            cleanup: super::refine::Cleanup::default(),
            chord: super::hotkey::Chord::default(),
            gpu: None,
            denoise: false,
            sound: true,
            record_debug: false,
        }
    }
}

/// Every line worth applying, paired with the way its errors name themselves.
/// Blank lines and comments are gone; the number is the one in the file.
fn numbered(text: &str) -> impl Iterator<Item = (String, String)> + '_ {
    text.lines().enumerate().filter_map(|(index, raw)| {
        let line = raw.split_once('#').map_or(raw, |(before, _)| before).trim();
        (!line.is_empty()).then(|| (format!("line {}", index + 1), line.to_string()))
    })
}

impl Config {
    /// How every `flow` command reads the config: it starts.
    ///
    /// Complaints go to the journal, one line each, and whatever the file got
    /// right is kept. `load_from` stays strict for the live reload, which has
    /// a last good config to fall back on and so can afford to say no.
    pub fn load() -> Self {
        let text = match std::fs::read_to_string(path()) {
            Ok(text) => text,
            // Absent is the normal state: a machine part-way through setup has
            // no config file, and the defaults are the product.
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Self::default(),
            Err(err) => {
                eprintln!(
                    "flow: {} unreadable ({err}), using defaults",
                    path().display()
                );
                return Self::default();
            }
        };

        let (config, complaints) = Self::parse_forgiving(&text);
        for complaint in complaints {
            eprintln!("flow: ignoring {complaint}");
        }
        config
    }

    /// Absent is the normal state and yields the defaults. Present but broken is
    /// an error: silently ignoring a typo would leave the user reading a config
    /// that does nothing.
    pub fn load_from(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => Self::parse(&text).with_context(|| format!("in {}", path.display())),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
        }
    }

    // ponytail: flat `key = value` only, which every key here is. Swap in the
    // toml crate if the config ever needs tables, arrays, or strings with a `#`.
    pub fn parse(text: &str) -> Result<Self> {
        let mut config = Self::default();
        for (at, line) in numbered(text) {
            config.apply(&at, &line)?;
        }
        Ok(config)
    }

    /// The same read, except that a line this version cannot make sense of is
    /// reported and skipped instead of refusing the whole file.
    ///
    /// This is how the daemon starts, and it exists because the strict read
    /// turned a version skew into a dead app: a `flow` older than the key the
    /// window had already written read `sound = false`, exited 1, and systemd
    /// restarted it into the same error for as long as anyone watched. A key
    /// from another version is not a reason to stop dictating - it is a reason
    /// to say so and carry on with everything else the file got right, which is
    /// also what the live reload has always done.
    pub fn parse_forgiving(text: &str) -> (Self, Vec<String>) {
        let mut config = Self::default();
        let mut complaints = Vec::new();
        for (at, line) in numbered(text) {
            if let Err(err) = config.apply(&at, &line) {
                complaints.push(format!("{err:#}"));
            }
        }
        (config, complaints)
    }

    /// One `key = value` line onto this config. The single place a key is
    /// spelled, so the strict read and the forgiving one cannot drift.
    fn apply(&mut self, at: &str, line: &str) -> Result<()> {
        let Some((key, value)) = line.split_once('=') else {
            bail!("{at}: expected `key = value`, found {line:?}");
        };
        let (key, value) = (key.trim(), value.trim());
        let config = self;

        match key {
            "push_to_talk" => config.push_to_talk = boolean(at, key, value)?,
            "cleanup" => {
                config.cleanup = super::refine::Cleanup::parse(value).ok_or_else(|| {
                    anyhow::anyhow!("{at}: cleanup wants none, light, or medium, found {value:?}")
                })?
            }
            // Accepted so an existing config keeps working across the rename.
            // `refine = false` was the only way to turn polish off before
            // levels existed, and it means exactly `cleanup = none`.
            "refine" => {
                config.cleanup = if boolean(at, key, value)? {
                    super::refine::Cleanup::default()
                } else {
                    super::refine::Cleanup::None
                }
            }
            "denoise" => config.denoise = boolean(at, key, value)?,
            "sound" => config.sound = boolean(at, key, value)?,
            "record_debug" => config.record_debug = boolean(at, key, value)?,
            "hotkey" => {
                config.chord = super::hotkey::Chord::parse(value)
                    .with_context(|| format!("{at}: bad hotkey"))?
            }
            "gpu" => {
                config.gpu =
                    Some(value.parse().with_context(|| {
                        format!("{at}: gpu wants a device index, found {value:?}")
                    })?)
            }
            "duck" => {
                config.duck = value
                    .parse()
                    .with_context(|| format!("{at}: duck wants a number, found {value:?}"))?;
                if config.duck > 100 {
                    bail!("{at}: duck is a percentage, found {value}");
                }
            }
            _ => bail!("{at}: unknown key {key:?}"),
        }

        Ok(())
    }

    /// Flags are the outermost layer, so `flow daemon --raw` can contradict the
    /// config file for one run without editing it.
    pub fn overridden_by(mut self, args: &[String]) -> Self {
        let present = |flag: &str| args.iter().any(|arg| arg == flag);

        if present("--raw") {
            self.cleanup = super::refine::Cleanup::None;
        }
        if let Some(level) = flag_str(args, "--cleanup").and_then(super::refine::Cleanup::parse) {
            self.cleanup = level;
        }
        self.denoise |= present("--denoise");
        self.denoise &= !present("--no-denoise");
        self.record_debug |= present("--record-debug");
        // Clamped rather than assigned: the file rejects anything over 100, and
        // without the same limit here `--duck 200` reached `Ducker`, which clamps
        // to 100 - and 100 means "hold every stream at its current volume", so the
        // typo turned ducking off rather than up.
        if let Some(percent) = flag_value(args, "--duck") {
            if percent > 100 {
                eprintln!("--duck {percent} is a percentage, using 100");
            }
            self.duck = percent.min(100);
        }
        self
    }

    /// `--duck 0` and `duck = 0` both mean "leave other apps alone".
    pub fn ducking(&self) -> Option<u32> {
        (self.duck > 0).then_some(self.duck)
    }
}

/// Value following `name`, e.g. `--duck 20`.
fn flag_value(args: &[String], name: &str) -> Option<u32> {
    flag_str(args, name)?.parse().ok()
}

/// Value following `name` left as text, e.g. `--cleanup light`.
fn flag_str<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let at = args.iter().position(|arg| arg == name)?;
    args.get(at + 1).map(String::as_str)
}

fn boolean(at: &str, key: &str, value: &str) -> Result<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => bail!("{at}: {key} wants true or false, found {other:?}"),
    }
}

pub fn path() -> PathBuf {
    flow_paths::config_file()
}

/// How often the config file is checked for changes. Fast enough that moving a
/// slider in the console feels immediate, slow enough to be free.
const POLL: std::time::Duration = std::time::Duration::from_millis(400);

/// Keep `shared` in step with the file, so a setting changed in the console or
/// in an editor takes effect without restarting the daemon.
///
/// # Why a poll and not inotify
///
/// A single save is not a single event. Writing this file in place fires
/// several `IN_MODIFY`s, and an editor that saves atomically renames a
/// temporary over it, which fires nothing for the original inode at all - so
/// an inotify watch has to be on the directory, and then debounced back down
/// to one reload. All of that machinery exists to answer "what does the file
/// say now", which is the only question here, and which a stat answers
/// directly. Polling is also inherently coalesced: half-written files are
/// simply read on the next tick.
///
/// A malformed file keeps the last good config and says so once, rather than
/// repeating itself four times a second: someone is mid-edit, and the daemon
/// must not lose its settings because a brace is briefly unbalanced.
pub fn watch(shared: std::sync::Arc<std::sync::Mutex<Config>>) {
    let path = path();
    std::thread::spawn(move || {
        let mut last = stamp(&path);
        let mut complained = false;
        loop {
            std::thread::sleep(POLL);
            let now = stamp(&path);
            if now == last {
                continue;
            }
            last = now;

            match Config::load_from(&path) {
                Ok(config) => {
                    complained = false;
                    let changed = {
                        let mut current = shared.lock().expect("config");
                        let changed = *current != config;
                        *current = config;
                        changed
                    };
                    if changed {
                        eprintln!("config reloaded");
                    }
                }
                Err(err) => {
                    if !complained {
                        complained = true;
                        eprintln!("config not reloaded, keeping the last good one: {err:#}");
                    }
                }
            }
        }
    });
}

/// Mtime and length together: either changing means the file did. Absent is a
/// state like any other, so deleting the config falls back to the defaults on
/// the next tick rather than being ignored.
fn stamp(path: &Path) -> Option<(std::time::SystemTime, u64)> {
    let meta = std::fs::metadata(path).ok()?;
    Some((meta.modified().ok()?, meta.len()))
}
