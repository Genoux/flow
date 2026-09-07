//! The speaker's own standing instructions, edited on the Style screen.
//!
//! Same file shape as `vocabulary.txt` and the same reader, so this module is
//! the path, the validation, and nothing else. See `vocabulary::beside_config`.
//!
//! It lives beside the Style cards rather than on a screen of its own because
//! it answers the question the cards leave open: they decide how much of what
//! you said is changed, and these decide how the result is written.

use std::path::PathBuf;

/// Long enough for a sentence, short enough that one line is one instruction.
/// Every line is in every system prompt from now on, so a paragraph here is a
/// paragraph the model reads on every dictation for as long as it stays.
const LONGEST: usize = 120;

/// How many the list will hold. Not a storage limit - it is the point at which
/// standing instructions stop being style and start being a prompt of their
/// own, competing with the rules that keep a dictation from being answered.
const MOST: usize = 12;

/// The shipped explanation of the file, so a list first written from this
/// window still has it for whoever opens the file by hand afterwards. Included
/// from the same file `flow install` seeds, because two copies of an
/// explanation are two things to keep true.
const TEMPLATE: &str = include_str!("../../../packaging/instructions.template.txt");

pub fn path() -> PathBuf {
    super::vocabulary::beside_config("instructions.txt")
}

pub fn load() -> Vec<String> {
    super::vocabulary::load_from(&path())
}

/// Writes the header first when the file is not there yet: an install that
/// predates this file has no template seeded, and `save_to` keeps whatever
/// comment block it finds rather than inventing one.
pub fn save(instructions: &[String]) -> std::io::Result<()> {
    let path = path();
    if !path.exists() {
        crate::storage::write(&path, TEMPLATE)?;
    }
    super::vocabulary::save_to(&path, instructions)
}

pub fn validate(candidate: &str, existing: &[String]) -> Result<String, String> {
    let instruction = candidate.trim();
    if instruction.is_empty() {
        return Err("Type an instruction first.".into());
    }
    if instruction.starts_with('#') {
        return Err("An instruction cannot start with #, which marks a comment.".into());
    }
    if instruction.contains('\n') || instruction.contains('\r') {
        return Err("One instruction per line.".into());
    }
    if instruction.chars().count() > LONGEST {
        return Err("Too long to keep. Say it in a sentence.".into());
    }
    if existing.len() >= MOST {
        return Err(format!(
            "{MOST} is as many as Flow will follow well. Remove one first."
        ));
    }
    if existing
        .iter()
        .any(|existing| existing.eq_ignore_ascii_case(instruction))
    {
        return Err("That one is already in the list.".into());
    }
    Ok(instruction.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_cannot_help_is_refused() {
        assert!(validate("   ", &[]).is_err());
        assert!(validate("# a comment", &[]).is_err());
        assert!(validate("two\nlines", &[]).is_err());
        assert!(validate(&"a".repeat(LONGEST + 1), &[]).is_err());
        assert_eq!(
            validate("  Use British spelling.  ", &[]).unwrap(),
            "Use British spelling."
        );
    }

    #[test]
    fn a_duplicate_is_refused_whatever_its_case() {
        let existing = vec!["Use British spelling.".to_string()];
        assert!(validate("use british spelling.", &existing).is_err());
    }

    /// Past this the list stops being a style and starts being a second prompt.
    #[test]
    fn the_list_has_a_ceiling() {
        let full: Vec<String> = (0..MOST).map(|n| format!("instruction {n}")).collect();
        assert!(validate("one more", &full).is_err());
    }
}
