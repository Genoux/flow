//! The OpenRouter key: whether a typed value could be one, and what to show
//! of a saved one.
//!
//! Kept apart from `settings.rs`, which only reads and writes the line in
//! `config.toml` and has no business judging what belongs on it.

/// The floor `router::send` already enforces on the daemon side (only
/// letters, digits, `-` and `_`), matched here so a bad key is refused before
/// it is ever written to disk rather than after the first failed dictation.
/// Duplicated rather than shared: the console does not link the daemon crate
/// (see `main.rs`'s own note on why), so a one-line predicate is smaller than
/// a dependency pulled in for it.
fn is_key_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_'
}

/// ponytail: OpenRouter keys are `sk-or-v1-` plus 64 hex characters today.
/// Both this and `MIN_KEY_LENGTH` assume that shape holds; if OpenRouter ever
/// changes the prefix or the length, loosen these to match rather than
/// stretching this pair to cover a format that has already moved.
const PREFIX: &str = "sk-or-";
const MIN_KEY_LENGTH: usize = 32;

/// How much of a key to show back - the enforced prefix plus the `v1-`
/// every current key has after it. Read off the key itself rather than
/// hardcoded, so a future `sk-or-v2-` still shows correctly without a change
/// here.
const VISIBLE_PREFIX: usize = PREFIX.len() + 3;

/// Reject what cannot be a real key before it reaches the file: this is the
/// check that must refuse a three-character placeholder that would otherwise
/// sit in `config.toml` looking saved while every dictation fails.
pub fn validate(candidate: &str) -> Result<String, String> {
    let key = candidate.trim();
    if key.is_empty() {
        return Err("Paste your OpenRouter key first.".into());
    }
    if !key.bytes().all(is_key_char) {
        return Err("An OpenRouter key is only letters, numbers, - and _.".into());
    }
    if !key.starts_with(PREFIX) {
        return Err(format!("An OpenRouter key starts with {PREFIX}."));
    }
    if key.len() < MIN_KEY_LENGTH {
        return Err("That's too short to be a real OpenRouter key.".into());
    }
    Ok(key.to_owned())
}

/// Whether the Save action has anything to save. Its own function because the
/// button's enabled state and `validate`'s refusal are different questions -
/// this one is answered on every keystroke, so it stays cheaper than a full
/// validation the row will run anyway on the actual save.
pub fn can_save(typing_key: &str) -> bool {
    !typing_key.is_empty()
}

/// `sk-or-v1-…4f2a`: enough to tell which key is saved, never enough to use
/// it. Takes whatever is actually there rather than assuming `validate`'s
/// floor, so it cannot panic on a key `Settings::load` read from a
/// hand-edited file.
pub fn fingerprint(key: &str) -> String {
    let head: String = key.chars().take(VISIBLE_PREFIX).collect();
    let tail: String = {
        let mut chars: Vec<char> = key.chars().rev().take(4).collect();
        chars.reverse();
        chars.into_iter().collect()
    };
    format!("{head}…{tail}")
}

/// The outcome of testing the saved key against the real API, read back from
/// `flow probe --porcelain`. Kept apart because each tells the user to do
/// something different: paste a key, paste a different one, or try again
/// later.
#[derive(Debug, Clone)]
pub enum Outcome {
    NoKey,
    /// Carries nothing on purpose: which model refines is a fact about the
    /// build, and naming it here put it on screen and pinned it in a test.
    Accepted,
    Rejected(String),
    Unreachable(String),
}

/// Parses `flow probe --porcelain`'s two lines. `None` is the same handshake
/// `system::damage_for` uses on `check --porcelain`: an older `flow`, or one
/// that did not answer, looks like a verdict this cannot read rather than a
/// clean bill of health.
pub fn parse_probe(text: &str) -> Option<Outcome> {
    let mut router = None;
    let mut detail = String::new();
    for line in text.lines() {
        if let Some(value) = line.strip_prefix("router\t") {
            router = Some(value);
        } else if let Some(value) = line.strip_prefix("detail\t") {
            detail = value.to_owned();
        }
    }
    match router? {
        "no-key" => Some(Outcome::NoKey),
        "accepted" => Some(Outcome::Accepted),
        "rejected" => Some(Outcome::Rejected(detail)),
        "unreachable" => Some(Outcome::Unreachable(detail)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_short_placeholder_is_never_accepted() {
        // The regression: `ddw` sat in a real config as a saved key for days
        // because nothing ever refused it.
        assert!(validate("ddw").is_err());
    }

    #[test]
    fn an_empty_key_is_refused() {
        assert!(validate("").is_err());
        assert!(validate("   ").is_err());
    }

    #[test]
    fn a_key_with_a_space_is_refused() {
        let err = validate("sk-or-v1- has a space in it").unwrap_err();
        assert!(err.contains("letters, numbers"), "{err}");
    }

    #[test]
    fn a_plausible_key_is_accepted() {
        let key = format!("sk-or-v1-{}", "a".repeat(48));
        assert_eq!(validate(&key).unwrap(), key);
    }

    #[test]
    fn a_key_missing_the_prefix_is_refused() {
        let err = validate(&"a".repeat(40)).unwrap_err();
        assert!(err.contains("sk-or-"), "{err}");
    }

    #[test]
    fn the_save_action_needs_something_typed() {
        assert!(!can_save(""));
        assert!(can_save("s"));
    }

    #[test]
    fn a_fingerprint_never_returns_the_whole_key() {
        let key = format!("sk-or-v1-{}", "b".repeat(64));
        let masked = fingerprint(&key);
        assert_ne!(masked, key);
        assert!(masked.starts_with("sk-or-v1-"));
        assert!(masked.ends_with("bbbb"));
        // Nothing from the middle of the key survives.
        assert!(!masked.contains(&"b".repeat(5)));
    }

    #[test]
    fn probe_output_reads_into_the_matching_outcome() {
        assert!(matches!(
            parse_probe("router\tno-key\ndetail\tAdd an OpenRouter key in Settings\n"),
            Some(Outcome::NoKey)
        ));
        assert!(matches!(
            parse_probe("router\taccepted\ndetail\tgoogle/gemini-3.1-flash-lite\n"),
            Some(Outcome::Accepted)
        ));
        // Whatever the refining model is called, acceptance reads the same.
        assert!(matches!(
            parse_probe("router\taccepted\ndetail\tsome/other-model-v9\n"),
            Some(Outcome::Accepted)
        ));
        assert!(matches!(
            parse_probe("router\trejected\ndetail\tMissing Authentication header\n"),
            Some(Outcome::Rejected(reason)) if reason == "Missing Authentication header"
        ));
        assert!(matches!(
            parse_probe("router\tunreachable\ndetail\ttimed out\n"),
            Some(Outcome::Unreachable(reason)) if reason == "timed out"
        ));
    }

    #[test]
    fn unparseable_probe_output_is_no_verdict_at_all() {
        assert!(parse_probe("").is_none());
        assert!(parse_probe("garbage\n").is_none());
    }
}
