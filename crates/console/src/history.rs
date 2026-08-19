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
    flow_paths::history_file()
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

/// What one calendar day of dictation amounted to - the unit the Overview's
/// activity calendar and its week-over-week numbers are both built from.
///
/// Words *and* dictations *and* seconds because the three answer different
/// questions and only one pass over the file is needed to get all three: the
/// calendar colours by words, the KPI row wants counts and speaking time, and
/// re-reading the file per metric would be three reads of the same lines.
#[derive(Debug, Clone, Copy, Default)]
pub struct Day {
    pub words: u32,
    pub dictations: u32,
    pub spoken: f32,
}

/// A rollup per calendar day (UTC) for the last `days` days, oldest first.
/// Reads the whole file rather than the `SHOWN`-capped `recent()` list: the
/// calendar looks back further than the window's visible history ever does.
///
/// ponytail: UTC day boundaries, not the user's local midnight - correct
/// enough for "was this day active", not worth a timezone dependency for.
pub fn daily(days: usize) -> Vec<Day> {
    let mut buckets = vec![Day::default(); days];
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
            let bucket = &mut buckets[days - 1 - ago];
            bucket.words += entry_text.split_whitespace().count() as u32;
            bucket.dictations += 1;
            bucket.spoken += value.get("spoken").and_then(|s| s.as_f64()).unwrap_or(0.0) as f32;
        }
    }

    buckets
}

/// Total words over a run of days.
pub fn words(days: &[Day]) -> u32 {
    days.iter().map(|day| day.words).sum()
}

/// The calendar date a UTC day number falls on, as `(year, month, day)` with
/// month 1-12.
///
/// Hinnant's `civil_from_days`, inlined rather than taken as a dependency:
/// the only date question this window asks is which month a calendar column
/// belongs to, and a date crate is a lot of tree to carry for one label row.
/// Shifts the epoch to 1 March so the leap day lands at the end of the year
/// and every month before it has a fixed length.
pub fn civil(day: u64) -> (i64, u32, u32) {
    let z = day as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097); // day of era, 0..=146_096
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // 0..=399
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // 0..=365, from 1 March
    let mp = (5 * doy + 2) / 153; // month shifted so 0 = March
    let date = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (year + i64::from(month <= 2), month, date)
}

/// `"12 Aug"` - short enough for a calendar tooltip, unambiguous in a way
/// that a numeric month never is across locales.
pub fn short_date(day: u64) -> String {
    const MONTHS: [&str; 12] =
        ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    let (_, month, date) = civil(day);
    format!("{date} {}", MONTHS[(month - 1) as usize])
}

/// `"2m 40s"`, or just `"40s"` under a minute - speaking time is measured in
/// seconds and minutes, and an hours field that reads `0h` on every real
/// week is noise.
pub fn duration(seconds: f32) -> String {
    let seconds = seconds.max(0.0).round() as u64;
    if seconds < 60 {
        return format!("{seconds}s");
    }
    let minutes = seconds / 60;
    if minutes < 60 {
        return format!("{minutes}m {}s", seconds % 60);
    }
    format!("{}h {}m", minutes / 60, minutes % 60)
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

    /// The month labels on the activity calendar are only as trustworthy as
    /// this, and it is the one piece of date arithmetic in the console.
    #[test]
    fn a_day_number_names_its_calendar_date() {
        assert_eq!(civil(0), (1970, 1, 1)); // the epoch itself
        assert_eq!(civil(59), (1970, 3, 1)); // 1970 was not a leap year
        assert_eq!(civil(11_016), (2000, 2, 29)); // 2000 was, despite the century
        assert_eq!(civil(20_684), (2026, 8, 19));
        assert_eq!(short_date(20_684), "19 Aug");
    }

    #[test]
    fn speaking_time_drops_the_fields_it_does_not_need() {
        assert_eq!(duration(0.0), "0s");
        assert_eq!(duration(59.4), "59s");
        assert_eq!(duration(60.0), "1m 0s");
        assert_eq!(duration(160.0), "2m 40s");
        assert_eq!(duration(3_720.0), "1h 2m");
    }
}
