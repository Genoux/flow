//! Dictate in French, get French back.
//!
//! The prompt has asked for this since commit 03085c6 and the model still does not
//! reliably obey - the sentence in `translated_the_speakers_french` is verbatim
//! from a real dictation that came back in English. So the language is detected
//! rather than requested, and a refining that changes it is treated as a failure.

use flow::refine::{changed_language, language};

/// Real dictation, Québécois French with English loanwords ("chills", "live",
/// "ok") mixed in. The code-switching is the hard part: it is what tipped the
/// model into treating the whole thing as English.
const FRENCH_IN: &str = "On copie-tu dimanche, je vais régler des chills le live, \
                         ok j'ai pas vraiment d'entête à ça, est-ce que dimanche c'est chill?";

/// What the model actually replied with.
const ENGLISH_OUT: &str = "Do you copy on Sunday? I'll handle the chill live, okay, \
                           I don't really have much to say about it. Is Sunday going to be chill?";

#[test]
fn a_language_is_recognised_when_there_is_enough_to_go_on() {
    assert_eq!(language(FRENCH_IN).map(|l| l.eng_name()), Some("French"));
    assert_eq!(language(ENGLISH_OUT).map(|l| l.eng_name()), Some("English"));
}

/// The regression this exists for.
#[test]
fn translated_the_speakers_french() {
    assert!(
        changed_language(ENGLISH_OUT, FRENCH_IN),
        "the reported failure must be caught"
    );
}

#[test]
fn a_faithful_refining_is_left_alone() {
    let raw = "euh alors je pense qu'on peut peut livrer la la fonctionnalité vendredi";
    let refined = "Alors, je pense qu'on peut livrer la fonctionnalité vendredi.";
    assert!(!changed_language(refined, raw), "both French, nothing to complain about");

    let english_raw = "um so i pushed the change and then uh the build broke again";
    let english_clean = "So I pushed the change and then the build broke again.";
    assert!(!changed_language(english_clean, english_raw), "both English");
}

/// Short or ambiguous text must not be guessed at: a false positive here would
/// silently switch refining off, which is worse than the translation it prevents.
#[test]
fn an_unclear_language_is_never_a_complaint() {
    for pair in [("Yeah.", "Yeah."), ("Mm.", "Mm."), ("OK", "Okay."), ("", "")] {
        assert!(
            !changed_language(pair.1, pair.0),
            "{pair:?} is too little to judge"
        );
    }
}

/// Refining routinely deletes fillers and repetitions, which shortens the text.
/// That must not tip the detector into a different answer.
#[test]
fn heavy_editing_of_the_same_language_is_not_a_translation() {
    let raw = "um so uh i mean the the deployment is is at nine i think uh yeah";
    let refined = "The deployment is at nine, I think.";
    assert!(!changed_language(refined, raw));
}
