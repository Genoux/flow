//! Three layers, in order: these defaults, then `~/.config/flow/config.toml`,
//! then command-line flags. A fresh install has no config file and needs none.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub push_to_talk: bool,
    /// Percentage of its current volume each other app is held at while
    /// recording. 0 disables ducking.
    pub duck: u32,
    pub cleanup: bool,
    pub terminal: bool,
    /// Key combination held to dictate. Only consulted when `push_to_talk` is on.
    pub chord: super::hotkey::Chord,
    /// Which GPU runs the cleanup model. `None` picks the roomiest discrete one,
    /// which is right on every machine tested so far; an index is the escape hatch
    /// for when it is not.
    pub gpu: Option<usize>,
    /// Run RNNoise denoising between capture and STT. Off by default so the A/B
    /// case is the current, known-good behaviour; on, it runs the utterance
    /// through nnnoiseless to strip hiss and fan noise before Parakeet sees it.
    pub denoise: bool,
    /// Save every dictation's audio as WAV files to `~/.local/share/flow/recordings/`,
    /// one raw and (when denoise is on) one denoised. Off by default because
    /// long sessions add up on disk fast; on, it is the only way to A/B the
    /// denoiser on the same source audio.
    pub record_debug: bool,
}

impl Default for Config {
    /// Push-to-talk is on out of the box: Flow watches the chord itself, so a
    /// fresh install dictates with no compositor configuration at all.
    fn default() -> Self {
        Self {
            push_to_talk: true,
            duck: 50,
            cleanup: true,
            terminal: false,
            chord: super::hotkey::Chord::default(),
            gpu: None,
            denoise: false,
            record_debug: false,
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        Self::load_from(&path())
    }

    /// Absent is the normal state and yields the defaults. Present but broken is
    /// an error: silently ignoring a typo would leave the user reading a config
    /// that does nothing.
    pub fn load_from(path: &Path) -> Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(text) => {
                Self::parse(&text).with_context(|| format!("in {}", path.display()))
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(err) => Err(err).with_context(|| format!("reading {}", path.display())),
        }
    }

    // ponytail: flat `key = value` only, which every key here is. Swap in the
    // toml crate if the config ever needs tables, arrays, or strings with a `#`.
    pub fn parse(text: &str) -> Result<Self> {
        let mut config = Self::default();

        for (index, raw) in text.lines().enumerate() {
            let line = raw.split_once('#').map_or(raw, |(before, _)| before).trim();
            if line.is_empty() {
                continue;
            }

            let at = format!("line {}", index + 1);
            let Some((key, value)) = line.split_once('=') else {
                bail!("{at}: expected `key = value`, found {line:?}");
            };
            let (key, value) = (key.trim(), value.trim());

            match key {
                "push_to_talk" => config.push_to_talk = boolean(&at, key, value)?,
                "cleanup" => config.cleanup = boolean(&at, key, value)?,
                "terminal" => config.terminal = boolean(&at, key, value)?,
                "denoise" => config.denoise = boolean(&at, key, value)?,
                "record_debug" => config.record_debug = boolean(&at, key, value)?,
                "hotkey" => {
                    config.chord = super::hotkey::Chord::parse(value)
                        .with_context(|| format!("{at}: bad hotkey"))?
                }
                "gpu" => {
                    config.gpu = Some(value.parse().with_context(|| {
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
        }

        Ok(config)
    }

    /// Flags are the outermost layer, so `flow daemon --raw` can contradict the
    /// config file for one run without editing it.
    pub fn overridden_by(mut self, args: &[String]) -> Self {
        let present = |flag: &str| args.iter().any(|arg| arg == flag);

        self.push_to_talk &= !present("--no-ptt");
        self.cleanup &= !present("--raw");
        self.terminal |= present("--terminal");
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
    let at = args.iter().position(|arg| arg == name)?;
    args.get(at + 1)?.parse().ok()
}

fn boolean(at: &str, key: &str, value: &str) -> Result<bool> {
    match value {
        "true" => Ok(true),
        "false" => Ok(false),
        other => bail!("{at}: {key} wants true or false, found {other:?}"),
    }
}

pub fn path() -> PathBuf {
    config_home().join("flow/config.toml")
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

pub fn config_home() -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var("HOME").unwrap()).join(".config"))
}
