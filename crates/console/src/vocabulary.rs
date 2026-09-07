//! The words the recogniser keeps getting wrong, edited in the window.
//!
//! The file is the daemon's interface, but it is not the user's: adding a
//! colleague's name should not mean finding a text editor. So this reads and
//! writes `~/.config/flow/vocabulary.txt` and the window presents it as a list
//! you add to and remove from.
//!
//! The leading comment block is preserved on write. It is the only explanation
//! of what the file is for, and someone who opens it by hand deserves to still
//! find it there.

use std::path::PathBuf;

pub fn path() -> PathBuf {
    super::settings::config_path()
        .parent()
        .map(|dir| dir.join("vocabulary.txt"))
        .unwrap_or_default()
}

/// Terms in file order, read exactly the way the daemon reads them so the list
/// shown is the list that reaches the model.
pub fn load() -> Vec<String> {
    std::fs::read_to_string(path())
        .unwrap_or_default()
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

/// Write `terms` back, keeping whatever comment block the file opened with.
pub fn save(terms: &[String]) -> std::io::Result<()> {
    let path = path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let existing = crate::storage::read(&path)?;
    let mut out = String::new();
    for line in leading_comment(&existing) {
        out.push_str(line);
        out.push('\n');
    }
    if !out.is_empty() {
        out.push('\n');
    }
    for term in terms {
        out.push_str(term);
        out.push('\n');
    }
    crate::storage::write(&path, &out)
}

/// The comment block at the top of the file, up to the first term. Comments
/// further down belong to lines being removed and are not worth guessing at.
fn leading_comment(text: &str) -> Vec<&str> {
    text.lines()
        .take_while(|line| {
            let trimmed = line.trim();
            trimmed.is_empty() || trimmed.starts_with('#')
        })
        .collect::<Vec<_>>()
        .into_iter()
        // Drop trailing blanks so the spacing is ours, not whatever was there.
        .rev()
        .skip_while(|line| line.trim().is_empty())
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect()
}

/// Longest a term can be. Past this it is a sentence, and a sentence in this
/// file is a sentence in every system prompt from now on.
const LONGEST: usize = 40;

/// A term the model can only be misled by.
///
/// The list works by sounding close to what was said - "hyper land" recovers
/// Hyprland - so a term with no sound to match recovers nothing. It is not free
/// either: every term is pasted into every system prompt as a name this speaker
/// uses, so a few stray keystrokes spend the model's attention on nonsense on
/// every dictation from then on. A real `vocabulary.txt` had `wd`, `wdq` and
/// `qwd` sitting in it, reaching the model on every dictation for weeks.
///
/// Only ASCII is judged, and an initialism is kept: `LLM` is spelled out when
/// spoken, and a script that does not write its vowels must be left alone
/// rather than guessed at.
fn is_unpronounceable(term: &str) -> bool {
    let letters = || term.chars().filter(|c| c.is_ascii_alphabetic());

    term.is_ascii()
        && letters().next().is_some()
        && !letters().all(|c| c.is_ascii_uppercase())
        && !term.to_lowercase().contains(['a', 'e', 'i', 'o', 'u', 'y'])
}

/// Reject what cannot help before it reaches the file: a blank, a duplicate,
/// something with a newline in it that would silently become two entries, or a
/// term with no sound for the recogniser to have mangled.
pub fn validate(candidate: &str, existing: &[String]) -> Result<String, String> {
    let term = candidate.trim();
    if term.is_empty() {
        return Err("Type a word first.".into());
    }
    if term.starts_with('#') {
        return Err("A term cannot start with #, which marks a comment.".into());
    }
    if term.contains('\n') || term.contains('\r') {
        return Err("One term per entry.".into());
    }
    if term.chars().count() > LONGEST {
        return Err("That is long for a term. Add the name on its own.".into());
    }
    if is_unpronounceable(term) {
        return Err(format!(
            "{term} has no sound to match. Flow fixes words that are heard wrong, \
             so a term needs to be sayable."
        ));
    }
    if existing.iter().any(|e| e.eq_ignore_ascii_case(term)) {
        return Err(format!("{term} is already in the list."));
    }
    Ok(term.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_explanation_at_the_top_survives_a_write() {
        let existing =
            "# Words the recogniser gets wrong.\n# One per line.\n\nHyprland\nPipeWire\n";
        let kept = leading_comment(existing);
        assert_eq!(
            kept,
            vec!["# Words the recogniser gets wrong.", "# One per line."]
        );
    }

    /// A file that is only terms has no comment to keep, and must not gain a
    /// stray blank line every time it is saved.
    #[test]
    fn a_file_without_comments_keeps_nothing() {
        assert!(leading_comment("Hyprland\nPipeWire\n").is_empty());
        assert!(leading_comment("").is_empty());
    }

    #[test]
    fn what_cannot_help_is_refused() {
        let existing = vec!["Hyprland".to_string()];
        assert!(validate("   ", &existing).is_err());
        assert!(validate("# not a term", &existing).is_err());
        assert!(validate("two\nlines", &existing).is_err());
        // Case-insensitive, because the recogniser does not care either.
        assert!(validate("hyprland", &existing).is_err());
        assert_eq!(validate("  PipeWire  ", &existing).unwrap(), "PipeWire");
    }

    /// The three that were found in a real config, reaching the model on every
    /// dictation.
    #[test]
    fn a_term_with_no_sound_is_refused() {
        for junk in ["wd", "wdq", "qwd"] {
            assert!(validate(junk, &[]).is_err(), "{junk:?} was accepted");
        }
    }

    /// An initialism is said out loud, and a script without written vowels is
    /// not ours to judge.
    #[test]
    fn a_sayable_term_is_kept() {
        for term in ["LLM", "AI", "Flow", "Hyprland", "Zürich", "北京", "GNOME"] {
            assert!(validate(term, &[]).is_ok(), "{term:?} was refused");
        }
    }

    #[test]
    fn a_sentence_is_not_a_term() {
        assert!(validate("the thing we built does not work on mobile", &[]).is_err());
    }
}

pub fn matching(terms: &[String], query: &str) -> Vec<usize> {
    let query = query.trim().to_lowercase();
    let mut matches: Vec<_> = terms
        .iter()
        .enumerate()
        .filter(|(_, term)| term.to_lowercase().contains(&query))
        .map(|(index, _)| index)
        .collect();
    matches.sort_by_cached_key(|&index| terms[index].to_lowercase());
    matches
}

#[cfg(test)]
mod search_tests {
    use super::*;

    #[test]
    fn sorting_and_filtering_keep_the_original_removal_index() {
        let terms = ["Zürich", "Ada", "Hyprland", "Flow"].map(str::to_owned);
        assert_eq!(matching(&terms, ""), vec![1, 3, 2, 0]);
        assert_eq!(matching(&terms, " HYPR "), vec![2]);
        assert_eq!(matching(&terms, "ZÜR"), vec![0]);
        assert!(matching(&terms, "missing").is_empty());
    }
}
