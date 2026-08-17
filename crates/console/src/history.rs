//! Reading the daemon's history file.
//!
//! Read from disk rather than from the daemon on purpose: history is the one
//! thing worth opening this window for when you are not dictating, so it has
//! to be there whether or not anything is running.

use std::path::PathBuf;

/// How many entries the window shows. The file keeps far more; this is what
/// fits on a screen without becoming a log viewer.
const SHOWN: usize = 50;

pub fn path() -> PathBuf {
    super::system::data_home().join("flow/history.jsonl")
}

#[derive(Debug, Clone)]
pub struct Entry {
    pub text: String,
    pub spoken: f32,
    pub at: u64,
}

/// The most recent entries, newest first. A missing file is an empty history,
/// not an error: it just means nothing has been dictated yet.
pub fn recent() -> Vec<Entry> {
    let Ok(text) = std::fs::read_to_string(path()) else {
        return Vec::new();
    };

    let mut entries: Vec<Entry> = text
        .lines()
        .rev()
        .take(SHOWN)
        .filter_map(|line| {
            let value: serde_json::Value = serde_json::from_str(line).ok()?;
            Some(Entry {
                text: value.get("text")?.as_str()?.to_owned(),
                spoken: value.get("spoken").and_then(|s| s.as_f64()).unwrap_or(0.0) as f32,
                at: value.get("at").and_then(|a| a.as_u64()).unwrap_or(0),
            })
        })
        .collect();
    entries.truncate(SHOWN);
    entries
}

/// How long ago, in the roughest terms that are still true. Deliberately not
/// a clock time: the useful question about a dictation is "was that the one I
/// just did", not what o'clock it happened.
pub fn ago(at: u64, now: u64) -> String {
    if at == 0 || at > now {
        return String::new();
    }
    let seconds = now - at;
    match seconds {
        0..=59 => "just now".to_string(),
        60..=3599 => format!("{}m ago", seconds / 60),
        3600..=86_399 => format!("{}h ago", seconds / 3600),
        _ => format!("{}d ago", seconds / 86_400),
    }
}

/// Words dictated per calendar day (UTC) for the last `days` days, oldest
/// first - the source for the Overview activity calendar. Reads the whole
/// file rather than the `SHOWN`-capped `recent()` list: the calendar looks
/// back further than the window's visible history ever does.
///
/// ponytail: UTC day boundaries, not the user's local midnight - correct
/// enough for "was this day active", not worth a timezone dependency for.
pub fn daily_words(days: usize) -> Vec<u32> {
    let mut buckets = vec![0u32; days];
    let today = now() / 86_400;

    let Ok(text) = std::fs::read_to_string(path()) else {
        return buckets;
    };

    for line in text.lines() {
        let Some(value) = serde_json::from_str::<serde_json::Value>(line).ok() else {
            continue;
        };
        let Some(at) = value.get("at").and_then(|a| a.as_u64()) else {
            continue;
        };
        let Some(entry_text) = value.get("text").and_then(|t| t.as_str()) else {
            continue;
        };

        let day = at / 86_400;
        if day > today {
            continue;
        }
        let ago = (today - day) as usize;
        if ago < days {
            buckets[days - 1 - ago] += entry_text.split_whitespace().count() as u32;
        }
    }

    buckets
}

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
    fn elapsed_time_reads_in_the_roughest_true_terms() {
        let now = 1_000_000;
        assert_eq!(ago(now, now), "just now");
        assert_eq!(ago(now - 59, now), "just now");
        assert_eq!(ago(now - 60, now), "1m ago");
        assert_eq!(ago(now - 3600, now), "1h ago");
        assert_eq!(ago(now - 86_400 * 3, now), "3d ago");
    }

    /// An unstamped or future-stamped entry says nothing rather than "in -4s".
    #[test]
    fn an_impossible_stamp_says_nothing() {
        assert_eq!(ago(0, 1_000_000), "");
        assert_eq!(ago(2_000_000, 1_000_000), "");
    }
}
