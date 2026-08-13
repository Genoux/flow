//! `flow install` - fetch the two models and seed the config templates.
//!
//! Every asset is pinned to an immutable commit rather than a branch, and
//! verified by sha256 before it is put in place. A partial download lives at
//! `.part` and is only renamed once it hashes correctly, so an interrupted
//! install can never look like a finished one.

use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

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
/// keeps the GPU free for cleanup. Multilingual (25 languages) - the int8 export
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
/// cleanup degrades to the raw transcript, dictation does not degrade at all.
///
/// 4B is a deliberate floor, not a default. Cleanup's one unforgivable failure is
/// paraphrasing instead of punctuating, and that is instruction-following - the
/// first capability to go when a model shrinks. This one already needed the
/// language rule promoted out of a bullet list to stop it translating.
pub const CLEANUP: &[Asset] = &[Asset {
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

fn fetch(asset: &Asset, root: &Path) -> Result<()> {
    let path = root.join(asset.dest);
    if is_installed(&path, asset) {
        eprintln!("  {} already installed", asset.dest);
        return Ok(());
    }

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }

    // ponytail: curl, for resume and a progress bar on a 2.4GB download without
    // writing either. Inherits stdio so the user sees the bar.
    let part = path.with_extension("part");
    eprintln!("  {} ({})", asset.dest, size(asset.bytes));
    let status = Command::new("curl")
        .args(["-fL", "--progress-bar", "-C", "-", "-o"])
        .arg(&part)
        .arg(asset.url())
        .status()
        .context("running curl - is it installed?")?;
    if !status.success() {
        bail!("downloading {} failed - rerun to resume", asset.url());
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
    Ok(())
}

pub fn fetch_all(assets: &[Asset], root: &Path) -> Result<()> {
    for asset in assets {
        fetch(asset, root)?;
    }
    Ok(())
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
    super::stt::data_home().join("flow/models")
}

/// Speech first and always: cleanup is skippable, and a machine that only
/// dictates is a working install rather than a failed one.
pub fn run(speech_only: bool) -> Result<()> {
    let root = models_root();
    eprintln!("installing into {}", root.display());

    eprintln!(
        "\nspeech recognition ({}) - required",
        size(total_bytes(SPEECH))
    );
    fetch_all(SPEECH, &root)?;

    if speech_only {
        eprintln!("\nskipping cleanup model (--speech-only)");
    } else {
        eprintln!(
            "\ncleanup model ({}) - optional, skip with --speech-only",
            size(total_bytes(CLEANUP))
        );
        fetch_all(CLEANUP, &root)?;
    }

    let config = super::config::path();
    if seed(&config, include_str!("../packaging/config.template.toml"))? {
        eprintln!("\nwrote {}", config.display());
    } else {
        eprintln!("\nkept your {}", config.display());
    }

    let vocabulary = super::config::config_home().join("flow/vocabulary.txt");
    if seed(&vocabulary, include_str!("../packaging/vocabulary.template.txt"))? {
        eprintln!("wrote {}", vocabulary.display());
    } else {
        eprintln!("kept your {}", vocabulary.display());
    }

    eprintln!("\ndone. `flow daemon` to run it, or install packaging/flow.service");
    Ok(())
}
