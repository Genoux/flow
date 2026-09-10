//! `flow install` - fetch the local speech model and seed the config templates.
//!
//! The model is pinned to an immutable commit rather than a branch, and
//! verified by sha256 before it is put in place. A partial download lands at
//! `.part` and is only renamed once it hashes correctly, so an interrupted
//! install can never look like a finished one.
//!
//! Every step is also reported through an [`Event`], which is what lets the
//! console's setup screen drive this same installer instead of running a
//! second download path of its own: `--plan` prints the total up front,
//! `--porcelain` streams one line per event on stdout, in the format
//! [`to_console`] writes and the console's own parser expects.

use anyhow::{Context, Result, bail};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, Debug)]
pub struct Asset {
    pub repo: &'static str,
    pub revision: &'static str,
    pub file: &'static str,
    /// Relative to the models root.
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

const REPO: &str = "altunenes/parakeet-rs";
const REVISION: &str = "a61d2818df4659c956b9661a9447f46e98c15126";

/// NVIDIA Nemotron 3.5 ASR streaming multilingual 0.6B, ONNX export, licensed
/// OpenMDW-1.1. The whole recogniser now: it runs on CPU, and there is no
/// refining model on this machine any more to keep the GPU free for.
pub const SPEECH: &[Asset] = &[
    Asset {
        repo: REPO,
        revision: REVISION,
        file: "nemotron-3.5-asr-streaming-0.6b-onnx/config.json",
        dest: "nemotron/config.json",
        bytes: 2_979,
        sha256: "b0289e196d11a17e3c661bbadfe455c87de4baffc1a5e652a5779f5d687c5db0",
    },
    Asset {
        repo: REPO,
        revision: REVISION,
        file: "nemotron-3.5-asr-streaming-0.6b-onnx/tokenizer.model",
        dest: "nemotron/tokenizer.model",
        bytes: 406_554,
        sha256: "ce3895e40806f02a26c3a225161b96ef682d6c0054bae32a245dec4258d7d291",
    },
    Asset {
        repo: REPO,
        revision: REVISION,
        file: "nemotron-3.5-asr-streaming-0.6b-onnx/encoder.onnx",
        dest: "nemotron/encoder.onnx",
        bytes: 42_164_972,
        sha256: "d569fbe78b48fbb04e169d324f5d25463838ceed7b5fc3bfe209872441979bd9",
    },
    Asset {
        repo: REPO,
        revision: REVISION,
        file: "nemotron-3.5-asr-streaming-0.6b-onnx/decoder_joint.onnx",
        dest: "nemotron/decoder_joint.onnx",
        bytes: 97_590_054,
        sha256: "634dfadf24cb4f73c2fae170b36611d68db48186426882cbc8f7e02ed9f2bb29",
    },
    Asset {
        repo: REPO,
        revision: REVISION,
        file: "nemotron-3.5-asr-streaming-0.6b-onnx/encoder.onnx.data",
        dest: "nemotron/encoder.onnx.data",
        bytes: 2_454_405_120,
        sha256: "7584f85df76bc9ae6fbdfa53aa8d97b07a842525d1c501d536d77fd9e4f57ac7",
    },
];

pub fn total_bytes(assets: &[Asset]) -> u64 {
    assets.iter().map(|asset| asset.bytes).sum()
}

pub fn models_root() -> PathBuf {
    flow_paths::models_dir()
}

// ponytail: sha256sum from coreutils rather than a hashing crate. Already
// shelling out to curl for the download, and this keeps a 2.4GB verify out of
// process memory. Swap in the sha2 crate if flow ever needs to run somewhere
// without it.
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

/// What an install is doing, as it does it.
///
/// Reported rather than printed because there are two audiences: a person
/// watching a terminal, and the setup screen in the console, which needs
/// numbers it can draw a ring from rather than a bar someone else already
/// drew. `to_console` renders these as the porcelain lines the console reads.
pub enum Event<'a> {
    /// Everything this install will fetch, before any of it starts.
    Planned {
        total: u64,
    },
    /// Hashing - either checking what is already on disk, or verifying what
    /// just came down.
    Verifying {
        asset: &'a Asset,
    },
    Fetching {
        asset: &'a Asset,
    },
    /// Bytes done across the whole install, not this asset.
    Progress {
        done: u64,
    },
    Installed {
        asset: &'a Asset,
    },
    /// A template landed (`written`) or an existing file was left alone.
    Seeded {
        path: PathBuf,
        written: bool,
    },
    Finished,
}

/// One event, as the line the console's parser expects: a word, a space, and
/// whatever that event carries. Split out from [`to_console`] so the format
/// itself - not just the act of printing it - has something to assert against.
fn line(event: &Event) -> String {
    match event {
        Event::Planned { total } => format!("total {total}"),
        Event::Verifying { asset } => format!("verifying {}", asset.dest),
        Event::Fetching { asset } => format!("fetching {} {}", asset.dest, asset.bytes),
        Event::Progress { done } => format!("progress {done}"),
        Event::Installed { asset } => format!("installed {}", asset.dest),
        Event::Seeded { path, written } => format!(
            "seeded {} {}",
            if *written { "wrote" } else { "kept" },
            path.display()
        ),
        Event::Finished => "finished".to_string(),
    }
}

/// The console's view: one whitespace-delimited line per event on stdout.
///
/// stdout and nothing else, so curl's own errors on stderr can never be
/// mistaken for protocol.
pub fn to_console(event: Event) {
    println!("{}", line(&event));
    // The window is reading this as it arrives; a block-buffered pipe would
    // deliver the whole install in one burst at the end.
    let _ = std::io::stdout().flush();
}

/// `base` is how many bytes the whole install had already finished before
/// this asset started, so `Progress` reports against the total rather than
/// restarting from zero on every file.
fn fetch(asset: &Asset, root: &Path, base: u64, report: &mut dyn FnMut(Event)) -> Result<()> {
    let path = root.join(asset.dest);
    report(Event::Verifying { asset });
    if is_installed(&path, asset) {
        eprintln!("  {} - already installed", asset.dest);
        report(Event::Installed { asset });
        report(Event::Progress {
            done: base + asset.bytes,
        });
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    // `-C -` resumes from whatever a previous interrupted run left in the part
    // file, so a killed `flow install` can pick up where it stopped rather than
    // refetching 2.4GB from zero. curl draws its own progress bar here - there
    // is no second renderer to keep in step with any more.
    let part = path.with_extension("part");
    eprintln!("  fetching {} ({})", asset.dest, size(asset.bytes));
    report(Event::Fetching { asset });
    let status = Command::new("curl")
        .args(["-fL", "--show-error", "-C", "-", "-o"])
        .arg(&part)
        .arg(asset.url())
        .status()
        .context("running curl - is it installed?")?;
    if !status.success() {
        bail!("downloading {} failed", asset.dest);
    }

    // Verified before the rename, so a truncated or tampered file never lands
    // at the path the recogniser actually loads from.
    report(Event::Verifying { asset });
    let downloaded = std::fs::metadata(&part)
        .with_context(|| format!("checking {}", part.display()))?
        .len();
    let hash = if downloaded == asset.bytes {
        sha256(&part)?
    } else {
        String::new()
    };
    if downloaded != asset.bytes || hash != asset.sha256 {
        let _ = std::fs::remove_file(&part);
        bail!(
            "{} arrived damaged and was discarded. Try again.",
            asset.dest
        );
    }

    std::fs::rename(&part, &path).with_context(|| format!("moving {} into place", asset.dest))?;
    eprintln!("  {} done", asset.dest);
    report(Event::Installed { asset });
    report(Event::Progress {
        done: base + asset.bytes,
    });
    Ok(())
}

fn fetch_all_reported(assets: &[Asset], root: &Path, report: &mut dyn FnMut(Event)) -> Result<()> {
    let mut base = 0;
    for asset in assets {
        fetch(asset, root, base, report)?;
        base += asset.bytes;
    }
    Ok(())
}

pub fn fetch_all(assets: &[Asset], root: &Path) -> Result<()> {
    fetch_all_reported(assets, root, &mut |_| {})
}

fn size(bytes: u64) -> String {
    match bytes {
        0..=999_999 => format!("{} KB", bytes / 1_000),
        1_000_000..=999_999_999 => format!("{} MB", bytes / 1_000_000),
        _ => format!("{:.1} GB", bytes as f64 / 1e9),
    }
}

/// Write `contents` to `path` if nothing is there yet.
///
/// Create-if-absent, never overwrite: the target may be a symlink into a
/// dotfiles repo, and clobbering someone's settings to install a template is
/// indefensible.
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

fn seed_templates(report: &mut dyn FnMut(Event)) -> Result<()> {
    for (path, contents) in [
        (
            super::config::path(),
            include_str!("../packaging/config.template.toml"),
        ),
        (
            flow_paths::vocabulary_file(),
            include_str!("../packaging/vocabulary.template.txt"),
        ),
    ] {
        let written = seed(&path, contents)?;
        eprintln!(
            "{} {}",
            if written { "wrote" } else { "kept" },
            path.display()
        );
        report(Event::Seeded { path, written });
    }
    Ok(())
}

/// What a run will fetch, without fetching it. Always [`SPEECH`] now - there
/// is only one asset list to plan for.
pub fn planned_bytes() -> u64 {
    total_bytes(SPEECH)
}

/// The same `total` line a real run opens with, and nothing else. Notably no
/// `finished`, which would tell the console an install had happened.
pub fn plan_reported(report: &mut dyn FnMut(Event)) {
    report(Event::Planned {
        total: planned_bytes(),
    });
}

/// The whole install, reporting itself through `report`.
///
/// The terminal and the console share every line of this, so there is one
/// installer and not two that drift.
pub fn run_reported(report: &mut dyn FnMut(Event)) -> Result<()> {
    let root = models_root();
    plan_reported(report);
    fetch_all_reported(SPEECH, &root, report)?;
    // Seeded after the model, so a download that fails leaves no config
    // implying an install that finished.
    seed_templates(report)?;
    report(Event::Finished);
    Ok(())
}

pub fn run() -> Result<()> {
    let root = models_root();
    eprintln!("installing into {}", root.display());
    fetch_all(SPEECH, &root)?;
    seed_templates(&mut |_| {})?;
    eprintln!(
        "\ndone. add an OpenRouter key in the console's Settings for the cleanup model, then `flow daemon`."
    );
    Ok(())
}

/// Every asset that is not on disk at its pinned length.
///
/// Length only, and deliberately. This is what the console runs at launch,
/// and a sha256 pass over the 2.45GB `encoder.onnx.data` is real time to spend
/// on every open - length is what catches how installs actually break: a file
/// deleted, a download cut short, a disk that filled. Bytes that are wrong at
/// the right length are what `flow install` hashes for, once someone asks it
/// to repair.
pub fn damaged() -> Vec<&'static Asset> {
    damaged_in(&models_root())
}

/// The same check against an explicit root, which is what makes it testable
/// without 2.6GB of real model.
pub fn damaged_in(root: &Path) -> Vec<&'static Asset> {
    SPEECH
        .iter()
        .filter(|asset| {
            !std::fs::metadata(root.join(asset.dest)).is_ok_and(|meta| meta.len() == asset.bytes)
        })
        .collect()
}

/// What the console reads at launch: one line per file that is not right,
/// then a verdict it can act on without counting.
pub fn report_damage(damaged: &[&Asset]) {
    for asset in damaged {
        println!("damaged {}", asset.dest);
    }
    println!(
        "{}",
        if damaged.is_empty() {
            "whole"
        } else {
            "broken"
        }
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn seeding_never_overwrites_what_is_already_there() {
        let path = std::env::temp_dir().join(format!("flow-seed-{}.toml", std::process::id()));
        let _ = std::fs::remove_file(&path);

        assert!(seed(&path, "first").unwrap(), "a fresh path is written");
        assert!(!seed(&path, "second").unwrap(), "an existing path is kept");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "first");

        std::fs::remove_file(&path).unwrap();
    }

    #[test]
    fn asset_url_points_at_the_pinned_revision() {
        let asset = SPEECH[0];
        assert_eq!(
            asset.url(),
            format!(
                "https://huggingface.co/{}/resolve/{}/{}",
                asset.repo, asset.revision, asset.file
            )
        );
        assert!(
            asset
                .url()
                .contains("/resolve/a61d2818df4659c956b9661a9447f46e98c15126/")
        );
    }

    #[test]
    fn speech_totals_the_expected_byte_count() {
        assert_eq!(total_bytes(SPEECH), 2_594_569_679);
    }

    #[test]
    fn plan_reports_the_total_and_nothing_else() {
        let mut events = Vec::new();
        plan_reported(&mut |event| events.push(line(&event)));
        assert_eq!(events, vec![format!("total {}", total_bytes(SPEECH))]);
    }

    /// The format the console's `setup::parse` reads, restated as an assertion
    /// so a future change to `line` cannot silently break that parser without
    /// a failing test on this side too.
    #[test]
    fn porcelain_lines_match_the_consoles_protocol() {
        let asset = &SPEECH[0];
        assert_eq!(line(&Event::Planned { total: 42 }), "total 42");
        assert_eq!(line(&Event::Progress { done: 7 }), "progress 7");
        assert_eq!(line(&Event::Finished), "finished");
        assert_eq!(
            line(&Event::Verifying { asset }),
            format!("verifying {}", asset.dest)
        );
        assert_eq!(
            line(&Event::Fetching { asset }),
            format!("fetching {} {}", asset.dest, asset.bytes)
        );
        assert_eq!(
            line(&Event::Installed { asset }),
            format!("installed {}", asset.dest)
        );
        assert_eq!(
            line(&Event::Seeded {
                path: PathBuf::from("/tmp/x"),
                written: true
            }),
            "seeded wrote /tmp/x"
        );
    }
}
