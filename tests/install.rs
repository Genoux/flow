//! The manifest is the installer's only source of truth, so it has to be
//! self-consistent, pinned to immutable revisions, and match what a working
//! machine actually has on disk.

use flow::install;

fn every_asset() -> Vec<&'static install::Asset> {
    install::SPEECH.iter().chain(install::REFINE).collect()
}

#[test]
fn the_manifest_is_well_formed() {
    for asset in every_asset() {
        assert_eq!(asset.sha256.len(), 64, "{}: sha256 is not 64 hex", asset.dest);
        assert!(
            asset.sha256.chars().all(|c| c.is_ascii_hexdigit() && !c.is_uppercase()),
            "{}: sha256 must be lowercase hex",
            asset.dest
        );
        assert!(asset.bytes > 0, "{}: zero bytes", asset.dest);
        assert!(!asset.dest.is_empty() && !asset.dest.starts_with('/'), "{}", asset.dest);
    }
}

/// A tag or branch can be moved under us; a commit cannot. This is the property
/// that makes the recorded hashes meaningful.
#[test]
fn every_source_is_pinned_to_a_commit() {
    for asset in every_asset() {
        assert_eq!(
            asset.revision.len(),
            40,
            "{}: revision {:?} is not a commit sha",
            asset.dest,
            asset.revision
        );
        assert!(asset.url().starts_with("https://huggingface.co/"), "{}", asset.url());
        assert!(asset.url().contains(asset.revision), "{}", asset.url());
    }
}

#[test]
fn destinations_are_unique() {
    let mut seen: Vec<&str> = every_asset().iter().map(|a| a.dest).collect();
    let before = seen.len();
    seen.sort_unstable();
    seen.dedup();
    assert_eq!(seen.len(), before, "duplicate destination in the manifest");
}

/// Speech is mandatory and refining is optional, which is the whole reason they
/// are separate lists - a machine that skips refining must still dictate.
#[test]
fn speech_and_refining_are_separate() {
    assert!(!install::SPEECH.is_empty());
    assert!(!install::REFINE.is_empty());
    assert!(install::SPEECH.iter().all(|a| a.dest.starts_with("tdt/")));
}

#[test]
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

/// Hashing 2.4GB is too slow for every run:
///   cargo test --release --test install -- --ignored --nocapture
#[test]
#[ignore]
fn the_pins_match_the_refining_model_on_disk() {
    let root = flow_paths::models_dir();
    for asset in install::REFINE {
        let path = root.join(asset.dest);
        if !path.is_file() {
            eprintln!("skipping: {} not installed", asset.dest);
            return;
        }
        assert_eq!(install::sha256(&path).expect("hash"), asset.sha256, "{}", asset.dest);
    }
}

/// The user's config may be a symlink into a dotfiles repo. Overwriting it would
/// destroy their settings, so seeding is create-if-absent and nothing else.
#[test]
fn seeding_never_overwrites() {
    let dir = std::env::temp_dir().join(format!("flow-seed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("mkdir");
    let path = dir.join("config.toml");

    assert!(install::seed(&path, "fresh").expect("first seed"), "should create");
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "fresh");

    std::fs::write(&path, "mine, hand-edited").expect("write");
    assert!(!install::seed(&path, "fresh").expect("second seed"), "should leave alone");
    assert_eq!(std::fs::read_to_string(&path).expect("read"), "mine, hand-edited");

    std::fs::remove_dir_all(&dir).ok();
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("flow-{name}-{}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("mkdir");
    dir
}

/// Real download against Hugging Face, using the three smallest pinned assets
/// (~230KB) so it exercises fetch, verify and rename without pulling gigabytes.
///   cargo test --release --test install -- --ignored --nocapture
#[test]
#[ignore]
fn downloading_verifies_and_lands_the_files() {
    let root = scratch("install");
    let small = &install::SPEECH[2..];
    assert!(install::total_bytes(small) < 1_000_000, "meant to be the small ones");

    install::fetch_all(small, &root).expect("fetch");

    for asset in small {
        let path = root.join(asset.dest);
        assert!(path.is_file(), "{} missing", asset.dest);
        assert_eq!(install::sha256(&path).expect("hash"), asset.sha256);
        assert!(!path.with_extension("part").exists(), "{} left a .part", asset.dest);
    }

    // Second run must be a no-op, which is what makes a failed install resumable.
    install::fetch_all(small, &root).expect("rerun");
    std::fs::remove_dir_all(&root).ok();
}

/// The invariant the whole design turns on: content that fails verification must
/// never appear at the path the daemon loads from.
#[test]
#[ignore]
fn a_bad_hash_never_lands() {
    let root = scratch("badhash");
    let tampered = [install::Asset {
        sha256: "0000000000000000000000000000000000000000000000000000000000000000",
        ..install::SPEECH[4]
    }];

    let err = install::fetch_all(&tampered, &root).expect_err("should reject");
    assert!(err.to_string().contains("sha256 mismatch"), "{err}");
    assert!(
        !root.join(tampered[0].dest).exists(),
        "unverified content landed at the real path"
    );
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn the_download_size_is_reported_in_gigabytes() {
    let speech = install::total_bytes(install::SPEECH);
    let both = install::total_bytes(install::SPEECH) + install::total_bytes(install::REFINE);
    assert!(speech > 600_000_000, "speech should be ~670MB, got {speech}");
    assert!(both > speech, "refining should add to the total");
}
