//! `flow install` - seed the config templates.
//!
//! This used to fetch about 3 GB of model weights and verify them by hash.
//! Transcription and refining are OpenRouter requests now, so there is nothing
//! to download and nothing to repair: the only thing a fresh machine still
//! needs on disk is a config it can read, and the key that goes in it is typed
//! into the console rather than fetched.

use anyhow::{Context, Result};
use std::path::Path;

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

/// The three files a fresh install wants, and whether each one was new.
pub fn run() -> Result<()> {
    for (path, contents) in [
        (
            super::config::path(),
            include_str!("../packaging/config.template.toml"),
        ),
        (
            flow_paths::vocabulary_file(),
            include_str!("../packaging/vocabulary.template.txt"),
        ),
        (
            flow_paths::instructions_file(),
            include_str!("../packaging/instructions.template.txt"),
        ),
    ] {
        let written = seed(&path, contents)?;
        eprintln!(
            "{} {}",
            if written { "wrote" } else { "kept" },
            path.display()
        );
    }
    eprintln!("\ndone. add an OpenRouter key in the console's Settings, then `flow daemon`.");
    Ok(())
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
}
