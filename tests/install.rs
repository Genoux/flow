//! The manifest is the installer's only source of truth, so it has to be
//! self-consistent and pinned to an immutable revision.

use flow::install;

#[test]
fn the_manifest_is_well_formed() {
    for asset in install::SPEECH {
        assert_eq!(
            asset.sha256.len(),
            64,
            "{}: sha256 is not 64 hex",
            asset.dest
        );
        assert!(
            asset
                .sha256
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
            "{}: sha256 must be lowercase hex",
            asset.dest
        );
        assert!(asset.bytes > 0, "{}: zero bytes", asset.dest);
        assert!(
            !asset.dest.is_empty() && !asset.dest.starts_with('/'),
            "{}",
            asset.dest
        );
    }
}

/// A tag or branch can be moved under us; a commit cannot. This is the
/// property that makes the recorded hashes meaningful.
#[test]
fn every_source_is_pinned_to_a_commit() {
    for asset in install::SPEECH {
        assert_eq!(
            asset.revision.len(),
            40,
            "{}: revision {:?} is not a commit sha",
            asset.dest,
            asset.revision
        );
        assert!(
            asset.url().starts_with("https://huggingface.co/"),
            "{}",
            asset.url()
        );
        assert!(asset.url().contains(asset.revision), "{}", asset.url());
    }
}

#[test]
fn destinations_are_unique() {
    let mut seen: Vec<&str> = install::SPEECH.iter().map(|a| a.dest).collect();
    let before = seen.len();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), before, "duplicate destination in the manifest");
}

#[test]
fn the_download_size_is_reported_in_gigabytes() {
    let total = install::total_bytes(install::SPEECH);
    assert!(total > 2_000_000_000, "expected ~2.6GB, got {total}");
}

/// Hashing 2.45GB (`encoder.onnx.data`) is too slow for every run, and the
/// model is already installed on the machines this suite runs on:
///   cargo test --release --test install -- --ignored --nocapture
#[test]
#[ignore]
fn the_pins_match_the_speech_model_on_disk() {
    let root = flow_paths::models_dir();
    for asset in install::SPEECH {
        let path = root.join(asset.dest);
        if !path.is_file() {
            eprintln!("skipping: {} not installed", asset.dest);
            return;
        }
        assert_eq!(
            std::fs::metadata(&path).expect("metadata").len(),
            asset.bytes,
            "{} size differs from the manifest",
            asset.dest
        );
        assert_eq!(
            install::sha256(&path).expect("hash"),
            asset.sha256,
            "{} content differs from the manifest",
            asset.dest
        );
    }
}

/// The launch check: what the console spawns to find out whether an install
/// is whole, without hashing 2.6GB to do it.
mod damage_report {
    use flow::install;

    fn scratch(name: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("flow-damage-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        root
    }

    /// A file of `bytes` length at `dest`, the way a real install leaves it.
    fn place(root: &std::path::Path, asset: &install::Asset, bytes: u64) {
        let path = root.join(asset.dest);
        std::fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
        std::fs::write(&path, vec![0u8; bytes as usize]).expect("write");
    }

    fn whole(root: &std::path::Path) {
        for asset in install::SPEECH {
            place(root, asset, asset.bytes);
        }
    }

    #[test]
    fn a_whole_install_reports_nothing() {
        let root = scratch("whole");
        whole(&root);
        assert!(install::damaged_in(&root).is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_missing_file_is_named() {
        let root = scratch("missing");
        whole(&root);
        let gone = install::SPEECH.first().expect("an asset");
        std::fs::remove_file(root.join(gone.dest)).expect("remove");

        let damaged = install::damaged_in(&root);
        assert_eq!(damaged.len(), 1, "{damaged:?}");
        assert_eq!(damaged[0].dest, gone.dest);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_file_of_the_wrong_length_is_named() {
        // The blind spot this check exists for: the directory is there, the
        // file is there, and a directory-presence check used to call that
        // installed.
        let root = scratch("truncated");
        whole(&root);
        let short = install::SPEECH.first().expect("an asset");
        place(&root, short, short.bytes / 2);

        let damaged = install::damaged_in(&root);
        assert_eq!(damaged.len(), 1, "{damaged:?}");
        assert_eq!(damaged[0].dest, short.dest);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn nothing_installed_names_everything() {
        let root = scratch("empty");
        let damaged = install::damaged_in(&root);
        assert_eq!(damaged.len(), install::SPEECH.len());
    }
}
