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

    let existing = std::fs::read_to_string(&path).unwrap_or_default();
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
    std::fs::write(&path, out)
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

/// Reject what cannot help before it reaches the file: a blank, a duplicate,
/// or something with a newline in it that would silently become two entries.
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
}
