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

/// Append one dictation. Best effort in every direction: a full disk or a
/// read-only home must never cost the user the text that is already on its way
/// to their cursor.
///
/// `raw` is the transcript before cleanup, and is what makes undoing an edit
/// possible after the fact. It is omitted when cleanup changed nothing, so the
/// key's presence means exactly "the model rewrote this" - a reader wanting the
/// original reads `raw` and falls back to `text`.
pub fn append(text: &str, raw: &str, spoken: f32, paste_ms: u128, at: u64) {
    append_to(&path(), text, raw, spoken, paste_ms, at);
}

fn append_to(path: &std::path::Path, text: &str, raw: &str, spoken: f32, paste_ms: u128, at: u64) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    let mut line = json!({
        "at": at,
        "text": text,
        "spoken": spoken,
        "paste_ms": paste_ms,
    });
    if raw != text {
        line["raw"] = json!(raw);
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

    #[test]
    fn history_stays_private_after_append_and_trim() {
        let path =
            std::env::temp_dir().join(format!("flow-history-permissions-{}", std::process::id()));
        append_to(&path, "private words", "private raw", 1.0, 0, 0);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        std::fs::write(&path, "{}\n".repeat(LIMIT)).unwrap();
        let temporary = path.with_extension("jsonl.trimming");
        std::fs::write(&temporary, "old temporary file").unwrap();
        std::fs::set_permissions(&temporary, std::fs::Permissions::from_mode(0o644)).unwrap();
        append_to(&path, "latest", "latest", 1.0, 0, 0);
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
