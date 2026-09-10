//! When refining should not run at all.
//!
//! Pure logic, testable without a network or a key. The GPU-placement tests
//! that used to sit above these went with the local model.
use flow::refine::needs_refining;

/// Already capitalised, already punctuated, no fillers - the model can only
/// return it unchanged, so paying ~200ms to hear that is waste.
///
/// "Mm-hmm." used to be here. It is pure filler now and never reaches refining at
/// all, so what this function would say about it no longer matters.
#[test]
fn short_and_already_clean_skips_the_model() {
    for already_clean in [
        "Yeah.",
        "Please.",
        "Thank you.",
        "This test.",
        "Oh my god.",
        "Okay, I'm testing.",
        "No!",
        "Why?",
    ] {
        assert!(
            !needs_refining(already_clean),
            "{already_clean:?} should skip"
        );
    }
}

#[test]
fn anything_the_model_could_fix_still_goes_through() {
    for needs_work in [
        "Hello", // no terminal punctuation
        "Uh",    // filler, should be deleted entirely
        "Um",
        "Laugh at",          // fragment
        "Okay. Should I be", // trails off
        "See no lay no",     // room tone misheard
        "yeah.",             // not capitalised
        "Um, yeah.",         // punctuated but carries a filler
        "I mean, yes.",      // multi-word filler
        "so i pushed the change to the config and then restarted it",
    ] {
        assert!(
            needs_refining(needs_work),
            "{needs_work:?} should be cleaned"
        );
    }
}

/// The gate is a latency optimisation for trivial input, so it must never fire on
/// anything long enough to plausibly need a comma.
#[test]
fn longer_utterances_are_never_skipped() {
    let clean_but_long = "One two three four five six seven.";
    assert!(
        needs_refining(clean_but_long),
        "too long to assume it is finished"
    );
}

#[test]
fn empty_input_needs_nothing() {
    assert!(!needs_refining(""));
    assert!(!needs_refining("   "));
}

// -- hesitation is not text -------------------------------------------------

use flow::refine::is_only_filler;

/// Holding the key and saying "uh" is a pause, not a dictation. It used to reach
/// the model, which deleted the filler, found nothing left, and answered the
/// question it thought it had been asked - pasting the literal word "None."
#[test]
fn a_transcript_of_pure_hesitation_has_nothing_to_write() {
    for hesitation in [
        "Um", "Uh", "uh", "Um.", "Uh, um", "er", "Ah!", "you know", "I mean",
    ] {
        assert!(is_only_filler(hesitation), "{hesitation:?} is not text");
    }
}

#[test]
fn real_words_are_never_mistaken_for_hesitation() {
    for real in [
        "Yes.",
        "I like it.",
        "Uh, ship it.",
        "Sort of works now.",
        "You know what to do.",
        "Um so the build broke",
    ] {
        assert!(!is_only_filler(real), "{real:?} carries words");
    }
}

/// Left to the caller's existing empty check, so the two paths cannot disagree
/// about which one owns an empty transcript.
#[test]
fn nothing_at_all_is_not_filler() {
    assert!(!is_only_filler(""));
    assert!(!is_only_filler("   "));
}
