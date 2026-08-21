//! Whether a newer Flow exists than the one running, and installing it.
//!
//! Asks GitHub for the newest published release and compares its tag against
//! this build's version. Installing is the release tarball unpacked into a
//! temporary directory and its own `packaging/install.sh` run from there - the
//! same script a person would run by hand, rather than a second install path
//! that can drift from it.
//!
//! curl rather than an HTTP crate, the same call `install.rs` already makes for
//! the models. An update check does not justify pulling reqwest, rustls and an
//! async runtime into a window whose dependency tree is already iced and wgpu.

use std::process::{Command, Stdio};

const LATEST_RELEASE: &str = "https://api.github.com/repos/Genoux/flow/releases/latest";

/// Where the release workflow's tarball lands, named after the tag it was cut
/// from. Kept in step with the `Package` step in `.github/workflows/release.yml`.
const DOWNLOAD: &str = "https://github.com/Genoux/flow/releases/download";

/// How long to wait on GitHub before giving up. A settings window that hangs on
/// a dead network is worse than one that says it could not check.
const TIMEOUT_SECONDS: &str = "10";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Status {
    /// Nothing asked yet. Only ever seen for a moment now that the window
    /// checks on open, or for good in demo mode, which never asks.
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
            LATEST_RELEASE,
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
        "200" => match tag_of(json) {
            Some(tag) if newer(&tag, running()) => Status::Available(tag),
            Some(_) => Status::Current,
            None => Status::Failed("GitHub sent a release with no tag".into()),
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
/// The console replaces its own binary here, which works because `install`
/// unlinks the destination before writing - the running process keeps the file
/// it was started from. It does mean the new version only appears on the next
/// launch, which is what `Status::Installed` exists to say.
pub fn install(tag: &str) -> Result<(), String> {
    let name = format!("flow-{tag}-x86_64-linux");
    let dir = std::env::temp_dir().join(format!("flow-update-{tag}"));
    // Left over from an interrupted attempt otherwise, and tar would unpack
    // over a half-written tree.
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|err| format!("{}: {err}", dir.display()))?;

    let tarball = dir.join(format!("{name}.tar.gz"));
    run(Command::new("curl")
        .args(["-sSL", "--fail", "--max-time", "600", "-o"])
        .arg(&tarball)
        .arg(format!("{DOWNLOAD}/{tag}/{name}.tar.gz")))?;
    run(Command::new("tar")
        .arg("xzf")
        .arg(&tarball)
        .arg("-C")
        .arg(&dir))?;
    run(Command::new("bash").arg(dir.join(&name).join("packaging/install.sh")))?;

    let _ = std::fs::remove_dir_all(&dir);
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

fn tag_of(json: &str) -> Option<String> {
    let value: serde_json::Value = serde_json::from_str(json).ok()?;
    Some(value.get("tag_name")?.as_str()?.to_string())
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
    use super::{newer, parse, tag_of, Status};

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

    #[test]
    fn the_tag_is_read_out_of_the_release() {
        assert_eq!(
            tag_of(r#"{"tag_name":"v0.2.0","name":"whatever"}"#),
            Some("v0.2.0".into())
        );
        assert_eq!(tag_of("not json"), None);
        assert_eq!(tag_of(r#"{"message":"Not Found"}"#), None);
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
