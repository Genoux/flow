//! Whether a newer Flow exists than the one running, and installing it.
//!
//! Asks GitHub for the published releases and compares the highest tag this
//! build may be offered against its own version. Installing is the release tarball unpacked into a
//! temporary directory and its own `packaging/install.sh` run from there - the
//! same script a person would run by hand, rather than a second install path
//! that can drift from it.
//!
//! curl rather than an HTTP crate, the same call `install.rs` already makes for
//! the models. An update check does not justify pulling reqwest, rustls and an
//! async runtime into a window whose dependency tree is already iced and wgpu.

use std::process::{Command, Stdio};

/// The list, not `releases/latest`. That endpoint is defined as the newest
/// release which is neither a draft nor a prerelease, so with every release
/// since v0.1.2 cut as a prerelease it kept answering v0.1.2 and the window
/// reported "up to date" on top of every alpha. The list is the only endpoint
/// that admits a prerelease exists.
const RELEASES: &str = "https://api.github.com/repos/Genoux/flow/releases?per_page=20";

/// Where the release workflow's tarball lands, named after the tag it was cut
/// from. Kept in step with the `Package` step in `.github/workflows/release.yml`.
const DOWNLOAD: &str = "https://github.com/Genoux/flow/releases/download";

/// How long to wait on GitHub before giving up. A settings window that hangs on
/// a dead network is worse than one that says it could not check.
const TIMEOUT_SECONDS: &str = "10";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Status {
    /// Nothing asked yet. Only ever seen for a moment now that the window
    /// checks on open.
    #[default]
    Unknown,
    Checking,
    Current,
    Available(String),
    /// Installed, but not running yet: this window and the daemon are still the
    /// old binaries until they are restarted, and saying "up to date" while the
    /// old one is on screen would be a lie.
    Installed(String),
    /// Kept as text because every reason a user can act on is different: no
    /// releases yet, no network, no curl.
    Failed(String),
}

pub fn running() -> &'static str {
    env!("CARGO_PKG_VERSION")
}

/// How a build that is not a release says which build it is.
///
/// `cargo run` and an installed release both report the same version, which is
/// no help at all when the thing being tested is a working tree: bumping the
/// version by hand only distinguishes the bumps, not the rebuilds between
/// them, and a commit cannot identify a build whose whole point is that it is
/// not committed yet. The binary's own modification time can, it changes on
/// every rebuild, and it costs one stat.
///
/// None for a release build, where the version is the whole truth.
pub fn dev_note() -> Option<String> {
    if !cfg!(debug_assertions) {
        return None;
    }

    let built = std::env::current_exe()
        .and_then(std::fs::metadata)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|at| at.duration_since(std::time::UNIX_EPOCH).ok());

    Some(match built {
        Some(built) => {
            format!(
                "dev, built {}",
                crate::history::ago(built.as_secs(), crate::history::now())
            )
        }
        None => "dev".to_string(),
    })
}

/// Blocking. Call it off the UI thread.
pub fn latest() -> Status {
    // The body and the status code together, because a 404 here is not a
    // failure worth a scary message - it is what an unreleased or private repo
    // returns, and the answer to it is "cut a release", not "check your wifi".
    let output = Command::new("curl")
        .args([
            "-sSL",
            "--max-time",
            TIMEOUT_SECONDS,
            "-H",
            "Accept: application/vnd.github+json",
            "-w",
            "\n%{http_code}",
            RELEASES,
        ])
        .output();

    let output = match output {
        Ok(output) => output,
        Err(err) => return Status::Failed(format!("curl: {err}")),
    };

    let body = String::from_utf8_lossy(&output.stdout);
    let Some((json, code)) = body.rsplit_once('\n') else {
        return Status::Failed("no answer from GitHub".into());
    };

    match code.trim() {
        "200" => match pick_channel(json, crate::system::channel()) {
            Err(reason) => Status::Failed(reason),
            Ok(Some(tag)) if newer(&tag, running()) => Status::Available(tag),
            Ok(_) => Status::Current,
        },
        "404" => Status::Failed("no releases published yet".into()),
        "403" => Status::Failed("GitHub rate limit reached, try later".into()),
        other => Status::Failed(format!("GitHub returned {other}")),
    }
}

/// Download the release tarball for `tag` and run its installer.
///
/// Blocking, and long: this is a download of a few tens of megabytes followed
/// by a script. Call it off the UI thread.
///
pub fn install(tag: &str) -> Result<(), String> {
    let channel = crate::system::channel();
    install_channel(tag, channel)?;
    crate::system::set_channel(channel)
}

pub fn join_channel(channel: crate::system::Channel) -> Result<(), String> {
    let output = Command::new("curl")
        .args(["-fsSL", "--max-time", TIMEOUT_SECONDS, RELEASES])
        .output()
        .map_err(|e| e.to_string())?;
    if !output.status.success() {
        return Err("Could not fetch releases. Your current build is unchanged.".into());
    }
    let tag = pick_channel(&String::from_utf8_lossy(&output.stdout), channel)?
        .ok_or("No published release is available for that channel.")?;
    install_channel(&tag, channel)?;
    crate::system::set_channel(channel)
}

fn pick_channel(json: &str, channel: crate::system::Channel) -> Result<Option<String>, String> {
    let releases: Vec<serde_json::Value> = serde_json::from_str(json).map_err(|e| e.to_string())?;
    Ok(releases
        .iter()
        .filter(|r| r["draft"].as_bool() == Some(false))
        .filter(|r| match channel {
            crate::system::Channel::Stable => r["prerelease"].as_bool() == Some(false),
            crate::system::Channel::Experimental => r["prerelease"].as_bool() == Some(true),
        })
        .filter_map(|r| r["tag_name"].as_str())
        .filter(|t| semver::Version::parse(t.trim_start_matches('v')).is_ok())
        .filter(|t| channel != crate::system::Channel::Experimental || t.contains("-experimental."))
        .max_by(|a, b| {
            semver::Version::parse(a.trim_start_matches('v'))
                .unwrap()
                .cmp(&semver::Version::parse(b.trim_start_matches('v')).unwrap())
        })
        .map(str::to_owned))
}

fn install_channel(tag: &str, channel: crate::system::Channel) -> Result<(), String> {
    semver::Version::parse(tag.strip_prefix('v').ok_or("Invalid release tag")?)
        .map_err(|_| "Invalid release tag")?;
    let name = format!("flow-{tag}-x86_64-linux");
    let unique = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("flow-update-{}-{unique}", std::process::id()));
    use std::os::unix::fs::DirBuilderExt;
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(&dir)
        .map_err(|err| err.to_string())?;

    let result = (|| {
        let tarball = dir.join(format!("{name}.tar.gz"));
        run(Command::new("curl")
            .args(["-sSL", "--fail", "--max-time", "600", "-o"])
            .arg(&tarball)
            .arg(format!("{DOWNLOAD}/{tag}/{name}.tar.gz")))?;
        let checksum = dir.join(format!("{name}.tar.gz.sha256"));
        run(Command::new("curl")
            .args(["-fsSL", "--max-time", "30", "-o"])
            .arg(&checksum)
            .arg(format!("{DOWNLOAD}/{tag}/{name}.tar.gz.sha256")))?;
        let expected = std::fs::read_to_string(&checksum).map_err(|e| e.to_string())?;
        let hash = expected
            .split_whitespace()
            .next()
            .ok_or("Missing checksum")?;
        if hash.len() != 64 || !hash.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err("Invalid release checksum".into());
        }
        let actual = Command::new("sha256sum")
            .arg(&tarball)
            .output()
            .map_err(|e| e.to_string())?;
        if !actual.status.success()
            || String::from_utf8_lossy(&actual.stdout)
                .split_whitespace()
                .next()
                != Some(hash)
        {
            return Err("Release checksum mismatch. Installation cancelled.".into());
        }
        run(Command::new("tar")
            .arg("xzf")
            .arg(&tarball)
            .arg("-C")
            .arg(&dir))?;
        verify_binaries(&dir.join(&name).join("bin"), "", tag)?;
        let mut installer = Command::new("bash");
        installer
            .arg(dir.join(&name).join("packaging/install.sh"))
            .args(["--channel", channel.suffix()]);
        installer.args(["--no-activate", "--no-restart"]);
        run(&mut installer)?;
        verify_install(&crate::system::bin_dir(), channel, tag)
    })();
    let _ = std::fs::remove_dir_all(&dir);
    result
}

fn verify_install(
    dir: &std::path::Path,
    channel: crate::system::Channel,
    tag: &str,
) -> Result<(), String> {
    verify_binaries(dir, &format!("-{}", channel.suffix()), tag)
}

fn verify_binaries(dir: &std::path::Path, suffix: &str, tag: &str) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let expected = tag.trim_start_matches('v');
    let version = semver::Version::parse(expected).map_err(|e| e.to_string())?;
    for name in ["flow", "flow-console"] {
        let path = dir.join(format!("{name}{suffix}"));
        if !path
            .metadata()
            .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        {
            return Err(format!("{name} is missing or not executable."));
        }
        // Releases before 0.3.1 open a window for --version; retain channel rollback.
        if name == "flow-console" && version < semver::Version::new(0, 3, 1) {
            continue;
        }
        let output = crate::system::run_for(
            path.to_str().ok_or("Invalid binary path")?,
            &["--version"],
            std::time::Duration::from_secs(5),
        )
        .ok_or_else(|| format!("{name} did not answer the version check."))?;
        if !output.status.success()
            || String::from_utf8_lossy(&output.stdout).trim() != format!("{name} {expected}")
        {
            return Err(format!(
                "{name} did not install version {expected}. Reinstall the release."
            ));
        }
    }
    Ok(())
}

/// Run a command to completion, failing with whatever it said on stderr.
fn run(command: &mut Command) -> Result<(), String> {
    let program = command.get_program().to_string_lossy().into_owned();
    let output = command
        .stdin(Stdio::null())
        .output()
        .map_err(|err| format!("{program}: {err}"))?;
    if output.status.success() {
        return Ok(());
    }
    // The last line: a failing install.sh ends on the step that broke, and the
    // hundred lines of progress above it are not what went wrong.
    let stderr = String::from_utf8_lossy(&output.stderr);
    let reason = stderr.trim().lines().last().unwrap_or_default().trim();
    Err(if reason.is_empty() {
        format!("{program} failed")
    } else {
        reason.to_string()
    })
}

/// Whether `candidate` is a later version than `running`.
///
/// Compared field by field as numbers, so 0.10.0 beats 0.9.0 - which string
/// comparison gets backwards, and which is the whole reason this is not a
/// one-line `>`.
///
/// A pre-release loses to the version it precedes: 1.2.3-rc1 comes before
/// 1.2.3, so the suffix has to lower the version rather than extend it. Reading
/// `-rc1` as a fourth field made it *higher*, which would have offered an
/// update to the release candidate of a version already installed.
fn newer(candidate: &str, running: &str) -> bool {
    if let (Ok(candidate), Ok(running)) = (
        semver::Version::parse(candidate.trim_start_matches('v')),
        semver::Version::parse(running.trim_start_matches('v')),
    ) {
        return candidate > running;
    }
    let (candidate_release, candidate_pre) = parse(candidate);
    let (running_release, running_pre) = parse(running);
    match candidate_release.cmp(&running_release) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => running_pre && !candidate_pre,
    }
}

/// The numeric fields, and whether anything followed them.
fn parse(version: &str) -> (Vec<u64>, bool) {
    let version = version.trim().trim_start_matches('v');
    let (release, prerelease) = match version.find(['-', '+']) {
        Some(at) => (&version[..at], true),
        None => (version, false),
    };
    let fields = release
        .split('.')
        .map(|field| field.parse().unwrap_or(0))
        .collect();
    (fields, prerelease)
}

#[cfg(test)]
mod tests {
    use super::{newer, parse, pick_channel, Status};
    use crate::system::Channel;

    #[test]
    fn installation_requires_matching_binaries_and_preserves_legacy_rollback() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("flow-version-test-{}", std::process::id()));
        std::fs::create_dir(&dir).unwrap();
        let write = |name: &str, body: &str| {
            let path = dir.join(format!("{name}-stable"));
            std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
        };
        write("flow", "echo flow 0.3.1");
        assert!(super::verify_install(&dir, Channel::Stable, "v0.3.1").is_err());
        write("flow-console", "echo flow-console 0.3.0");
        assert!(super::verify_install(&dir, Channel::Stable, "v0.3.1").is_err());
        write("flow-console", "echo flow-console 0.3.1; exit 1");
        assert!(super::verify_install(&dir, Channel::Stable, "v0.3.1").is_err());
        write("flow-console", "echo flow-console 0.3.1");
        assert!(super::verify_install(&dir, Channel::Stable, "v0.3.1").is_ok());
        write("flow", "echo flow 0.3.0");
        write(
            "flow-console",
            &format!("touch '{}'", dir.join("launched").display()),
        );
        assert!(super::verify_install(&dir, Channel::Stable, "v0.3.0").is_ok());
        assert!(!dir.join("launched").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn channel_selection_survives_mixed_releases_and_numeric_prereleases() {
        let releases = r#"[
            {"tag_name":"v0.3.0","draft":false,"prerelease":false},
            {"tag_name":"v0.3.0-experimental.9","draft":false,"prerelease":true},
            {"tag_name":"v0.3.0-experimental.10","draft":false,"prerelease":true},
            {"tag_name":"v9.0.0-alpha.1","draft":false,"prerelease":true},
            {"tag_name":"v9.0.0-experimental.1","draft":true,"prerelease":true}
        ]"#;
        assert_eq!(
            pick_channel(releases, Channel::Stable).unwrap().as_deref(),
            Some("v0.3.0")
        );
        assert_eq!(
            pick_channel(releases, Channel::Experimental)
                .unwrap()
                .as_deref(),
            Some("v0.3.0-experimental.10")
        );
        assert!(newer("v0.3.0-experimental.10", "0.3.0-experimental.9"));
        assert!(!newer("v0.3.0-experimental.1", "0.3.0"));
    }

    #[test]
    fn later_versions_are_newer() {
        assert!(newer("v0.2.0", "0.1.0"));
        assert!(newer("0.1.1", "0.1.0"));
        assert!(newer("1.0.0", "0.9.9"));
        assert!(!newer("v0.1.0", "0.1.0"));
        assert!(!newer("0.1.0", "0.2.0"));
    }

    // The bug a string compare would ship: "0.10.0" < "0.9.0" alphabetically.
    #[test]
    fn ten_beats_nine() {
        assert!(newer("0.10.0", "0.9.0"));
        assert!(!newer("0.9.0", "0.10.0"));
    }

    #[test]
    fn a_v_prefix_and_a_prerelease_do_not_confuse_it() {
        assert_eq!(parse("v1.2.3"), (vec![1, 2, 3], false));
        assert_eq!(parse("1.2.3-rc1"), (vec![1, 2, 3], true));
        // A release candidate is not newer than the release it precedes, and
        // the release IS newer than its candidate.
        assert!(!newer("1.2.3-rc1", "1.2.3"));
        assert!(newer("1.2.3", "1.2.3-rc1"));
        // But a candidate for a later version still counts.
        assert!(newer("1.3.0-rc1", "1.2.3"));
    }

    /// The shape GitHub actually returns, in the order it returns it: newest
    /// first, drafts included for whoever can see them.
    const LIST: &str = r#"[
        {"tag_name":"v0.3.0","draft":true,"prerelease":false},
        {"tag_name":"v0.3.0-experimental.1","draft":false,"prerelease":true},
        {"tag_name":"v0.1.3-alpha.2","draft":false,"prerelease":true},
        {"tag_name":"v0.1.2","draft":false,"prerelease":false}
    ]"#;

    /// The regression that shipped: every release after v0.1.2 was a
    /// prerelease, `releases/latest` skips those by definition, and so the
    /// window sat on an alpha reporting "up to date" forever.
    #[test]
    fn a_prerelease_build_is_offered_the_newest_prerelease() {
        assert_eq!(
            pick_channel(LIST, Channel::Experimental),
            Ok(Some("v0.3.0-experimental.1".into()))
        );
    }

    #[test]
    fn a_stable_build_is_never_walked_onto_a_prerelease() {
        assert_eq!(
            pick_channel(LIST, Channel::Stable),
            Ok(Some("v0.1.2".into()))
        );
        // And that is not an update, so nothing gets offered.
        assert!(!newer("v0.1.2", "0.1.2"));
    }

    #[test]
    fn a_draft_is_invisible_even_to_a_prerelease_build() {
        // v0.3.0 is the highest tag in the list and must still lose.
        assert_eq!(
            pick_channel(LIST, Channel::Experimental),
            Ok(Some("v0.3.0-experimental.1".into()))
        );
    }

    // The list is ordered by when a release was cut, which is not the same as
    // by version: a patch backported after a minor is returned above it.
    #[test]
    fn the_highest_version_wins_not_the_first_listed() {
        let out_of_order = r#"[
            {"tag_name":"v0.1.4","draft":false,"prerelease":false},
            {"tag_name":"v0.9.0","draft":false,"prerelease":false}
        ]"#;
        assert_eq!(
            pick_channel(out_of_order, Channel::Stable),
            Ok(Some("v0.9.0".into()))
        );
    }

    #[test]
    fn a_repo_with_no_releases_offers_nothing_and_is_not_an_error() {
        assert_eq!(pick_channel("[]", Channel::Stable), Ok(None));
        assert!(pick_channel("not json", Channel::Stable).is_err());
    }

    /// The unit tests above all feed `newer` and `tag_of` strings this file
    /// wrote itself. Only GitHub can say whether the URL, the header and the
    /// shape of the answer are still right, and a wrong answer here is silent:
    /// the window would simply never offer an update.
    ///
    /// Network, no side effects:
    ///   cargo test --manifest-path crates/console/Cargo.toml -- --ignored --nocapture
    #[test]
    #[ignore]
    fn github_answers_the_check() {
        let status = super::latest();
        eprintln!("running {}, GitHub says {status:?}", super::running());
        assert!(
            matches!(status, Status::Current | Status::Available(_)),
            "the check did not resolve against the real repo: {status:?}"
        );
    }
}
