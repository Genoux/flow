//! Turning values into the strings the window shows.
//!
//! Nothing here draws anything - these are the pure functions, which is also
//! why this is where their tests live.

use crate::theme::{ACCENT, MUTED};
use iced::Color;

/// This week against last week, as the second line of a KPI tile.
///
/// Counted in words, not percent. A percentage needs a base worth dividing by,
/// and a week of dictation rarely is one: a quiet week after a busy one reads
/// "-100% vs last week", which sounds like a fault rather than a week off, and
/// a busy week after a quiet one reads "+900%" off eleven words. The honest
/// answer to "how did this week go" is how many words up or down it was.
///
/// Only a gain takes the accent. A quieter week is not an error, so it is not
/// coloured like one.
pub(crate) fn trend(now: u32, before: u32) -> (String, Color) {
    match (now, before) {
        (0, 0) => ("nothing yet".to_string(), MUTED),
        (0, _) => ("nothing this week".to_string(), MUTED),
        (_, 0) => ("first week".to_string(), ACCENT),
        _ if now > before => (
            format!("{} more than last week", commas(now - before)),
            ACCENT,
        ),
        _ if now < before => (
            format!("{} fewer than last week", commas(before - now)),
            MUTED,
        ),
        _ => ("same as last week".to_string(), MUTED),
    }
}

/// `18402` -> `"18,402"`. Four figures of words dictated is a real number to
/// reach, and it should not have to be counted digit by digit.
pub(crate) fn commas(count: u32) -> String {
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// Cut to `chars` with an ellipsis, so a long value says it continues rather
/// than looking like it simply stopped. Used with `Wrapping::None`, which
/// stops the line becoming two but says nothing about where it ends.
pub(crate) fn clip(text: &str, chars: usize) -> String {
    if text.chars().count() <= chars {
        return text.to_string();
    }
    let cut: String = text.chars().take(chars).collect();
    format!("{}…", cut.trim_end())
}

/// Like `clip`, but keeps the end. A path that does not fit should still name
/// the file; cutting from the start would leave a directory prefix and lose
/// the only part that distinguishes Config from History.
pub(crate) fn clip_tail(text: &str, chars: usize) -> String {
    let count = text.chars().count();
    if count <= chars {
        return text.to_string();
    }
    let keep = chars.saturating_sub(1);
    let tail: String = text.chars().skip(count.saturating_sub(keep)).collect();
    format!("…{tail}")
}

/// `$HOME/…` as `~/…`, which is how the rest of Flow writes these paths.
/// Anything outside home is left alone - a custom XDG directory is the
/// actual location, not a tilde we invented.
pub(crate) fn display_path(path: &std::path::Path) -> String {
    collapse_home(
        path,
        std::env::var_os("HOME")
            .as_deref()
            .map(std::path::Path::new),
    )
}

fn collapse_home(path: &std::path::Path, home: Option<&std::path::Path>) -> String {
    let shown = path.display().to_string();
    let Some(home) = home else {
        return shown;
    };
    match path.strip_prefix(home) {
        Ok(rest) if rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => shown,
    }
}

/// "1 dictation" / "4 dictations". Spelled out rather than abbreviated, so a
/// caption reads as a sentence and not as something to decode.
pub(crate) fn plural(count: u32, noun: &str) -> String {
    if count == 1 {
        format!("1 {noun}")
    } else {
        format!("{count} {noun}s")
    }
}

#[cfg(test)]
mod tests {
    use super::{clip_tail, collapse_home, commas, trend};
    use crate::theme::{ACCENT, MUTED};
    use std::path::Path;

    #[test]
    fn a_week_is_reported_against_the_one_before_it() {
        assert_eq!(trend(1_320, 100).0, "1,220 more than last week");
        assert_eq!(trend(80, 100).0, "20 fewer than last week");
        assert_eq!(trend(100, 100), ("same as last week".to_string(), MUTED));
        assert_eq!(trend(120, 100).1, ACCENT);
        assert_eq!(trend(50, 0).1, ACCENT);
        assert_eq!(trend(0, 0), ("nothing yet".to_string(), MUTED));
        // The week off that used to read "-100% vs last week", in red.
        assert_eq!(trend(0, 900), ("nothing this week".to_string(), MUTED));
    }

    #[test]
    fn long_numbers_stay_readable() {
        assert_eq!(commas(7), "7");
        assert_eq!(commas(999), "999");
        assert_eq!(commas(1_000), "1,000");
        assert_eq!(commas(18_402), "18,402");
    }

    #[test]
    fn home_is_written_as_a_tilde() {
        let home = Path::new("/home/j");
        assert_eq!(
            collapse_home(Path::new("/home/j/.config/flow/config.toml"), Some(home)),
            "~/.config/flow/config.toml"
        );
        assert_eq!(
            collapse_home(
                Path::new("/home/j/.local/share/flow/history.jsonl"),
                Some(home)
            ),
            "~/.local/share/flow/history.jsonl"
        );
        assert_eq!(collapse_home(home, Some(home)), "~");
    }

    #[test]
    fn a_path_outside_home_is_left_alone() {
        assert_eq!(
            collapse_home(
                Path::new("/custom/config/flow/config.toml"),
                Some(Path::new("/home/j"))
            ),
            "/custom/config/flow/config.toml"
        );
        assert_eq!(collapse_home(Path::new("/tmp/x"), None), "/tmp/x");
    }

    #[test]
    fn a_long_path_keeps_the_filename() {
        assert_eq!(clip_tail("abcdef", 6), "abcdef");
        assert_eq!(
            clip_tail("/private/tmp/claude/flow/config.toml", 17),
            "…flow/config.toml"
        );
    }
}
