//! Whether a newer Flow exists than the one running.
//!
//! Asks GitHub for the newest published release and compares its tag against
//! this build's version. Checking only: telling you an update exists is a
//! different job from replacing a daemon while it runs, and only the first one
//! is safe to do from a settings window.
//!
//! curl rather than an HTTP crate, the same call `install.rs` already makes for
//! the models. An update check does not justify pulling reqwest, rustls and an
//! async runtime into a window whose dependency tree is already iced and wgpu.

use std::process::Command;

const LATEST_RELEASE: &str = "https://api.github.com/repos/Genoux/flow/releases/latest";

/// How long to wait on GitHub before giving up. A settings window that hangs on
/// a dead network is worse than one that says it could not check.
const TIMEOUT_SECONDS: &str = "10";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Status {
    /// Nothing asked yet. Checking on open would put a network call in the path
    /// of a window whose whole job is usually to flip one switch.
    #[default]
    Unknown,
    Checking,
    Current,
    Available(String),
    /// Kept as text because every reason a user can act on is different: no
    /// releases yet, no network, no curl.
    Failed(String),
}

pub fn running() -> &'static str {
    env!("CARGO_PKG_VERSION")
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
    use super::{newer, parse, tag_of};

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
}
