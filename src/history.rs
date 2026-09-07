//! Every finished dictation, appended to a file so it outlives the daemon.
//!
//! The console reads this rather than asking the daemon, which means history
//! is there before the daemon starts, survives it restarting, and does not
//! need anything running to look at. One JSON object per line: appending is a
//! single write with no read-modify-write, so a crash mid-dictation can at
//! worst lose the last line rather than the file.

use serde_json::json;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::PathBuf;

/// Trim back to this many entries once the file grows past [`LIMIT`]. Keeping
/// a bounded history is the point - this is a record of what you dictated, not
/// an archive, and an unbounded one on a machine used daily is a slow leak.
const KEEP: usize = 500;

/// Rewrite the file once it passes this. Checked by line count rather than
/// bytes so one enormous dictation cannot trigger a rewrite on its own.
const LIMIT: usize = 1_000;

pub fn path() -> PathBuf {
    flow_paths::history_file()
}

/// One finished dictation, as the file records it.
pub struct Record<'a> {
    /// What was pasted.
    pub text: &'a str,
    /// The transcript before cleanup, which is what makes undoing an edit
    /// possible after the fact. Written only when it differs from `text`, so
    /// the key's presence means exactly "the model rewrote this" - a reader
    /// wanting the original reads `raw` and falls back to `text`.
    pub raw: &'a str,
    pub spoken: f32,
    pub paste_ms: u128,
    pub at: u64,
    /// Why `text` looks the way it does. Recorded because `raw`'s absence
    /// cannot say: see [`crate::refine::Outcome`].
    pub outcome: crate::refine::Outcome,
}

/// Append one dictation. Best effort in every direction: a full disk or a
/// read-only home must never cost the user the text that is already on its way
/// to their cursor.
pub fn append(record: Record<'_>) {
    append_to(&path(), record);
}

fn append_to(path: &std::path::Path, record: Record<'_>) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let Record {
        text,
        raw,
        spoken,
        paste_ms,
        at,
        outcome,
    } = record;

    let mut line = json!({
        "at": at,
        "text": text,
        "spoken": spoken,
        "paste_ms": paste_ms,
        "cleanup": outcome.as_str(),
    });
    if raw != text {
        line["raw"] = json!(raw);
    }
    if let Some(reason) = outcome.reason() {
        line["reason"] = json!(reason);
    }

    let appended = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .mode(0o600)
        .open(path)
        .and_then(|mut file| {
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            writeln!(file, "{line}")
        });

    if let Err(err) = appended {
        eprintln!("history not written: {err}");
        return;
    }

    trim(path);
}

/// Keep the newest [`KEEP`] lines once the file passes [`LIMIT`].
///
/// Writes a sibling file and renames over the original, so a crash halfway
/// through leaves the old history intact rather than a truncated one.
fn trim(path: &std::path::Path) {
    let Ok(text) = std::fs::read_to_string(path) else {
        return;
    };
    let lines: Vec<&str> = text.lines().collect();
    if lines.len() <= LIMIT {
        return;
    }

    let kept = lines[lines.len() - KEEP..].join("\n");
    let temporary = path.with_extension("jsonl.trimming");
    let written = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(&temporary)
        .and_then(|mut file| {
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
            writeln!(file, "{kept}")
        });
    if written.is_ok() {
        let _ = std::fs::rename(&temporary, path);
    }
}

/// Seconds since the epoch, for stamping an entry. Zero if the clock is
/// unreadable, which only costs the entry its timestamp.
pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|since| since.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record<'a>(text: &'a str, raw: &'a str) -> Record<'a> {
        Record {
            text,
            raw,
            spoken: 1.0,
            paste_ms: 0,
            at: 0,
            outcome: crate::refine::Outcome::Applied,
        }
    }

    /// A fallback and a dictation that needed no cleanup both leave `raw` out,
    /// which is why the outcome is written down separately.
    #[test]
    fn the_record_says_why_the_text_looks_like_that() {
        let path =
            std::env::temp_dir().join(format!("flow-history-outcome-{}", std::process::id()));
        let _ = std::fs::remove_file(&path);
        append_to(
            &path,
            Record {
                outcome: crate::refine::Outcome::FellBack("refining exceeded 2.5s".into()),
                ..record("um the build broke", "um the build broke")
            },
        );
        append_to(&path, record("Yeah.", "Yeah."));

        let written = std::fs::read_to_string(&path).unwrap();
        let mut lines = written.lines();
        let first: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(first["cleanup"], "fell_back");
        assert_eq!(first["reason"], "refining exceeded 2.5s");
        let second: serde_json::Value = serde_json::from_str(lines.next().unwrap()).unwrap();
        assert_eq!(second["cleanup"], "applied");
        assert!(second.get("reason").is_none());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn history_stays_private_after_append_and_trim() {
        let path =
            std::env::temp_dir().join(format!("flow-history-permissions-{}", std::process::id()));
        append_to(&path, record("private words", "private raw"));
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::write(&path, "{}\n".repeat(LIMIT)).unwrap();
        let temporary = path.with_extension("jsonl.trimming");
        std::fs::write(&temporary, "old temporary file").unwrap();
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o644)).unwrap();
        append_to(&path, record("latest", "latest"));
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap().lines().count(),
            KEEP
        );
        std::fs::remove_file(path).unwrap();
    }
}
