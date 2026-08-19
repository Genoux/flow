//! `flow install` - fetch the two models and seed the config templates.
//!
//! Every asset is pinned to an immutable commit rather than a branch, and
//! verified by sha256 before it is put in place. A partial download lives at
//! `.part` and is only renamed once it hashes correctly, so an interrupted
//! install can never look like a finished one.

use anyhow::{bail, Context, Result};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;

#[derive(Clone, Copy)]
pub struct Asset {
    pub repo: &'static str,
    pub revision: &'static str,
    pub file: &'static str,
    /// Relative to the models root: where the files go is the manifest's
    /// business, not the caller's.
    pub dest: &'static str,
    pub bytes: u64,
    pub sha256: &'static str,
}

impl Asset {
    pub fn url(&self) -> String {
        format!(
            "https://huggingface.co/{}/resolve/{}/{}",
            self.repo, self.revision, self.file
        )
    }
}

/// Parakeet TDT 0.6B v3, int8 ONNX. Runs on CPU at ~23x realtime, which is what
/// keeps the GPU free for refining. Multilingual (25 languages) - the int8 export
/// of the v2 English-only model has the same filenames, so the hashes below are
/// the only thing distinguishing them.
pub const SPEECH: &[Asset] = &[
    Asset {
        repo: "istupakov/parakeet-tdt-0.6b-v3-onnx",
        revision: "8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce",
        file: "encoder-model.int8.onnx",
        dest: "tdt/encoder-model.int8.onnx",
        bytes: 652_183_999,
        sha256: "6139d2fa7e1b086097b277c7149725edbab89cc7c7ae64b23c741be4055aff09",
    },
    Asset {
        repo: "istupakov/parakeet-tdt-0.6b-v3-onnx",
        revision: "8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce",
        file: "decoder_joint-model.int8.onnx",
        dest: "tdt/decoder_joint-model.int8.onnx",
        bytes: 18_202_004,
        sha256: "eea7483ee3d1a30375daedc8ed83e3960c91b098812127a0d99d1c8977667a70",
    },
    Asset {
        repo: "istupakov/parakeet-tdt-0.6b-v3-onnx",
        revision: "8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce",
        file: "nemo128.onnx",
        dest: "tdt/nemo128.onnx",
        bytes: 139_764,
        sha256: "a9fde1486ebfcc08f328d75ad4610c67835fea58c73ba57e3209a6f6cf019e9f",
    },
    Asset {
        repo: "istupakov/parakeet-tdt-0.6b-v3-onnx",
        revision: "8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce",
        file: "vocab.txt",
        dest: "tdt/vocab.txt",
        bytes: 93_939,
        sha256: "d58544679ea4bc6ac563d1f545eb7d474bd6cfa467f0a6e2c1dc1c7d37e3c35d",
    },
    Asset {
        repo: "istupakov/parakeet-tdt-0.6b-v3-onnx",
        revision: "8f23f0c03c8761650bdb5b40aaf3e40d2c15f1ce",
        file: "config.json",
        dest: "tdt/config.json",
        bytes: 97,
        sha256: "666903c76b9798caf2c210afd4f6cd60b08a8dbf9800ec8d7a3bc0d2148ac466",
    },
];

/// Qwen3 4B Instruct, Q4_K_M. Separate from [`SPEECH`] because it is optional:
/// refining degrades to the raw transcript, dictation does not degrade at all.
///
/// 4B is a deliberate floor, not a default. Refining's one unforgivable failure is
/// paraphrasing instead of punctuating, and that is instruction-following - the
/// first capability to go when a model shrinks. This one already needed the
/// language rule promoted out of a bullet list to stop it translating.
pub const REFINE: &[Asset] = &[Asset {
    repo: "unsloth/Qwen3-4B-Instruct-2507-GGUF",
    revision: "a06e946bb6b655725eafa393f4a9745d460374c9",
    file: "Qwen3-4B-Instruct-2507-Q4_K_M.gguf",
    dest: "qwen3-4b-instruct-q4km.gguf",
    bytes: 2_497_281_120,
    sha256: "3605803b982cb64aead44f6c1b2ae36e3acdb41d8e46c8a94c6533bc4c67e597",
}];

pub fn total_bytes(assets: &[Asset]) -> u64 {
    assets.iter().map(|asset| asset.bytes).sum()
}

/// What an install is doing, as it does it.
///
/// Reported rather than printed because there are two audiences now: a person
/// watching a terminal, and the setup screen in the console, which needs
/// numbers it can draw a bar from rather than a bar someone else already drew.
/// Both get the same events, so the window can never show progress the terminal
/// disagrees with.
pub enum Event<'a> {
    /// Everything that will be fetched, before any of it starts. Sent once.
    Planned { total: u64 },
    /// One of the two models, and how many bytes it accounts for. Sent in the
    /// order they are fetched, straight after `Planned`.
    ///
    /// The window draws a bar per model rather than one bar for the pair, and
    /// this is what tells it where the boundary falls. It is sent rather than
    /// hardcoded there because the sizes live here, next to the assets they
    /// are the sum of.
    Group { label: &'static str, bytes: u64 },
    /// Hashing - either checking what is already on disk, or verifying what
    /// just came down. A 2.4 GB file takes long enough that a bar which simply
    /// stops moving reads as a hang.
    Verifying { asset: &'a Asset },
    Fetching { asset: &'a Asset },
    /// Bytes done across the whole install, not this asset.
    Progress { done: u64 },
    Installed { asset: &'a Asset },
    /// A template landed (`written`) or an existing file was left alone.
    Seeded { path: PathBuf, written: bool },
    Finished,
}

/// How often the download loop looks at the part file. Fast enough that the
/// bar moves like a download rather than a slideshow, slow enough that a
/// 2.4 GB fetch is not thousands of stats.
const POLL: Duration = Duration::from_millis(120);

/// The terminal's view: a line per asset, rewritten in place as it fills.
///
/// Flow draws this itself now instead of handing the job to curl's own bar.
/// Two renderers of one download disagree eventually, and the one the console
/// reads has to be the one that is right.
#[derive(Default)]
pub struct Terminal {
    total: u64,
}

impl Terminal {
    pub fn report(&mut self, event: Event) {
        match event {
            Event::Planned { total } => {
                self.total = total;
                eprintln!("  {} to fetch", size(total));
            }
            // The terminal draws one running figure, so the split is only
            // useful to it as a heading.
            Event::Group { label, bytes } => eprintln!("  {label} ({})", size(bytes)),
            Event::Verifying { asset } => self.line(&format!("{} - checking…", asset.dest)),
            Event::Fetching { asset } => {
                self.line(&format!("{} ({})", asset.dest, size(asset.bytes)));
            }
            Event::Progress { done } if self.total > 0 => {
                let percent = done * 100 / self.total;
                self.line(&format!("{} of {}  {percent}%", size(done), size(self.total)));
            }
            Event::Progress { .. } => {}
            // Ends the line the three above have been rewriting, so the next
            // asset starts on its own rather than overwriting this one.
            Event::Installed { asset } => eprintln!("\r  {} ✓\x1b[K", asset.dest),
            Event::Seeded { path, written } => {
                eprintln!("{} {}", if written { "wrote" } else { "kept your" }, path.display());
            }
            Event::Finished => {}
        }
    }

    /// Carriage return, then erase to end of line: without the erase, a short
    /// line leaves the tail of a longer one behind it.
    fn line(&self, text: &str) {
        eprint!("\r  {text}\x1b[K");
        let _ = std::io::stderr().flush();
    }
}

/// The console's view: one whitespace-delimited line per event on stdout.
///
/// stdout and nothing else, so curl's own errors on stderr can never be
/// mistaken for protocol. Destinations are relative paths with no spaces in
/// them, which is what lets this stay a split rather than a parser.
pub fn to_console(event: Event) {
    match event {
        Event::Planned { total } => println!("total {total}"),
        Event::Group { label, bytes } => println!("group {label} {bytes}"),
        Event::Verifying { asset } => println!("verifying {}", asset.dest),
        Event::Fetching { asset } => println!("fetching {} {}", asset.dest, asset.bytes),
        Event::Progress { done } => println!("progress {done}"),
        Event::Installed { asset } => println!("installed {}", asset.dest),
        Event::Seeded { path, written } => {
            println!("seeded {} {}", if written { "wrote" } else { "kept" }, path.display());
        }
        Event::Finished => println!("finished"),
    }
    // The window is reading this as it arrives; a block-buffered pipe would
    // deliver the whole install in one burst at the end.
    let _ = std::io::stdout().flush();
}

pub fn size(bytes: u64) -> String {
    match bytes {
        0..=999_999 => format!("{} KB", bytes / 1_000),
        1_000_000..=999_999_999 => format!("{} MB", bytes / 1_000_000),
        _ => format!("{:.1} GB", bytes as f64 / 1e9),
    }
}

// ponytail: sha256sum from coreutils rather than a hashing crate. Already
// shelling out to pactl and curl, and this keeps a 2.4GB verify out of process
// memory. Swap in the sha2 crate if flow ever needs to run somewhere without it.
pub fn sha256(path: &Path) -> Result<String> {
    let output = Command::new("sha256sum")
        .arg(path)
        .output()
        .context("running sha256sum - is coreutils installed?")?;
    if !output.status.success() {
        bail!("sha256sum failed on {}", path.display());
    }
    let text = String::from_utf8(output.stdout).context("sha256sum output")?;
    Ok(text
        .split_whitespace()
        .next()
        .context("empty sha256sum output")?
        .to_string())
}

/// Already correct on disk? Then it is done - this is what makes re-running
/// `flow install` cheap and makes a failed run resumable.
fn is_installed(path: &Path, asset: &Asset) -> bool {
    std::fs::metadata(path).is_ok_and(|meta| meta.len() == asset.bytes)
        && sha256(path).is_ok_and(|hash| hash == asset.sha256)
}

/// `base` is how many bytes the whole install had already finished before this
/// asset started, so progress is reported against the total rather than
/// restarting from zero on every file.
fn fetch(asset: &Asset, root: &Path, base: u64, report: &mut dyn FnMut(Event)) -> Result<()> {
    let path = root.join(asset.dest);
    report(Event::Verifying { asset });
    if is_installed(&path, asset) {
        report(Event::Installed { asset });
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    // ponytail: curl, for resume on a 2.4GB download without writing it. Its
    // own bar is off now - the part file is the progress, and reading it is
    // what lets the terminal and the window show the same number.
    let part = path.with_extension("part");
    report(Event::Fetching { asset });
    let mut child = Command::new("curl")
        .args(["-fL", "--silent", "--show-error", "-C", "-", "-o"])
        .arg(&part)
        .arg(asset.url())
        .stdin(std::process::Stdio::null())
        // stdout stays clear: it carries the console's protocol, and curl must
        // never be able to write a line onto it.
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .context("running curl - is it installed?")?;

    // `-C -` resumes into the part file, so its length already counts whatever
    // an interrupted run left there and this stays correct across a retry.
    let status = loop {
        if let Some(status) = child.try_wait().context("waiting for curl")? {
            break status;
        }
        let so_far = std::fs::metadata(&part).map(|meta| meta.len()).unwrap_or(0);
        report(Event::Progress { done: base + so_far });
        std::thread::sleep(POLL);
    };

    if !status.success() {
        // curl says why - a DNS failure and a 404 are different problems, and
        // "downloading failed" tells whoever hit it neither. Safe to read only
        // now that curl has exited: with `-sS` it writes nothing until it does.
        let mut reason = String::new();
        if let Some(mut stderr) = child.stderr.take() {
            use std::io::Read;
            let _ = stderr.read_to_string(&mut reason);
        }
        let reason = reason.trim();
        bail!(
            "downloading {} failed{} - rerun to resume",
            asset.dest,
            if reason.is_empty() { String::new() } else { format!(": {reason}") }
        );
    }

    // Verified before the rename, so a truncated or tampered file never lands at
    // the real path where the daemon would load it.
    let size = std::fs::metadata(&part)?.len();
    if size != asset.bytes {
        bail!(
            "{}: expected {} bytes, got {size} - delete {} and retry",
            asset.dest,
            asset.bytes,
            part.display()
        );
    }
    report(Event::Verifying { asset });
    let hash = sha256(&part)?;
    if hash != asset.sha256 {
        bail!(
            "{}: sha256 mismatch\n  expected {}\n  got      {hash}\ndelete {} and retry",
            asset.dest,
            asset.sha256,
            part.display()
        );
    }

    std::fs::rename(&part, &path)
        .with_context(|| format!("moving {} into place", asset.dest))?;
    report(Event::Installed { asset });
    Ok(())
}

/// `base` carries forward between assets so one download finishing does not
/// send the bar back to where the last one started.
fn fetch_all_from(
    assets: &[Asset],
    root: &Path,
    base: &mut u64,
    report: &mut dyn FnMut(Event),
) -> Result<()> {
    for asset in assets {
        fetch(asset, root, *base, report)?;
        *base += asset.bytes;
        report(Event::Progress { done: *base });
    }
    Ok(())
}

pub fn fetch_all(assets: &[Asset], root: &Path) -> Result<()> {
    let mut terminal = Terminal::default();
    let mut base = 0;
    fetch_all_from(assets, root, &mut base, &mut |event| terminal.report(event))
}

/// Create-if-absent, never overwrite: the target may be a symlink into a dotfiles
/// repo, and clobbering someone's settings to install a template is indefensible.
/// Returns whether the file was created.
pub fn seed(path: &Path, contents: &str) -> Result<bool> {
    if path.exists() {
        return Ok(false);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    std::fs::write(path, contents).with_context(|| format!("writing {}", path.display()))?;
    Ok(true)
}

pub fn models_root() -> PathBuf {
    flow_paths::models_dir()
}

/// What `run` will fetch, without fetching it. The setup screen asks so it can
/// name the download size before anyone commits to it.
pub fn planned_bytes(speech_only: bool) -> u64 {
    total_bytes(SPEECH) + if speech_only { 0 } else { total_bytes(REFINE) }
}

/// The whole install, reporting itself through `report`.
///
/// The terminal and the console pass different reporters and share every other
/// line of this, so there is one installer and not two that drift.
pub fn run_reported(speech_only: bool, report: &mut dyn FnMut(Event)) -> Result<()> {
    let root = models_root();
    report(Event::Planned { total: planned_bytes(speech_only) });
    report(Event::Group { label: "speech", bytes: total_bytes(SPEECH) });
    if !speech_only {
        report(Event::Group { label: "refine", bytes: total_bytes(REFINE) });
    }

    let mut base = 0;
    fetch_all_from(SPEECH, &root, &mut base, report)?;
    if !speech_only {
        fetch_all_from(REFINE, &root, &mut base, report)?;
    }

    // Seeded after the models, so a download that fails leaves no config
    // implying an install that finished.
    for (path, contents) in [
        (super::config::path(), include_str!("../packaging/config.template.toml")),
        (flow_paths::vocabulary_file(), include_str!("../packaging/vocabulary.template.txt")),
    ] {
        let written = seed(&path, contents)?;
        report(Event::Seeded { path, written });
    }

    report(Event::Finished);
    Ok(())
}

pub fn run(speech_only: bool) -> Result<()> {
    let root = models_root();
    eprintln!("installing into {}", root.display());
    if speech_only {
        eprintln!("speech recognition only (--speech-only)");
    }

    let mut terminal = Terminal::default();
    run_reported(speech_only, &mut |event| terminal.report(event))?;

    eprintln!("\ndone. `flow daemon` to run it, or install packaging/flow.service");
    Ok(())
}
