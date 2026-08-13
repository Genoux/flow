//! Which GPU the cleanup model lands on, and when it should not run at all.
//!
//! Both are pure logic so they can be tested without a GPU or a model. The
//! topologies below are real ones, including the machine this was written on.

use flow::cleanup::{Candidate, choose_device};

fn candidate(index: usize, description: &str, discrete: bool, free_gb: f64) -> Candidate {
    Candidate {
        index,
        description: description.to_string(),
        discrete,
        free_bytes: (free_gb * 1e9) as u64,
    }
}

/// 2.4GB model plus ~410MB of KV cache and compute buffers, measured.
const NEEDED: u64 = 2_900_000_000;

/// The regression this whole function exists for. Ranking by free memory alone
/// picks the iGPU here, because it reports shared system RAM.
#[test]
fn a_discrete_card_beats_an_igpu_reporting_more_free_memory() {
    let devices = [
        candidate(0, "NVIDIA GeForce RTX 3060 Ti", true, 4.41),
        candidate(1, "AMD Radeon Graphics (RADV RAPHAEL)", false, 16.88),
    ];
    let chosen = choose_device(&devices, NEEDED).expect("should pick the discrete card");
    assert_eq!(chosen.index, 0, "picked {}", chosen.description);
}

/// Enumeration order is not preference order, which is the other half of the bug:
/// the same two devices with the iGPU first must still pick the discrete card.
#[test]
fn enumeration_order_does_not_decide() {
    let devices = [
        candidate(0, "AMD Radeon Graphics (RADV RAPHAEL)", false, 16.88),
        candidate(1, "NVIDIA GeForce RTX 3060 Ti", true, 4.41),
    ];
    assert_eq!(choose_device(&devices, NEEDED).expect("discrete").index, 1);
}

#[test]
fn the_roomiest_discrete_card_wins_among_equals() {
    let devices = [
        candidate(0, "NVIDIA GeForce RTX 3060 Ti", true, 4.41),
        candidate(1, "NVIDIA GeForce RTX 4090", true, 22.0),
    ];
    assert_eq!(choose_device(&devices, NEEDED).expect("bigger").index, 1);
}

/// A laptop with only an iGPU should still use it - it is the only accelerator
/// there is, and it beats the CPU.
#[test]
fn an_igpu_is_used_when_it_is_all_there_is() {
    let devices = [candidate(0, "Intel Iris Xe", false, 6.0)];
    assert_eq!(choose_device(&devices, NEEDED).expect("igpu").index, 0);
}

/// The failure this prevents: offloading 2.4GB onto a card that cannot hold it.
#[test]
fn a_device_without_room_is_not_chosen() {
    let devices = [candidate(0, "NVIDIA GeForce GT 1030", true, 1.8)];
    assert!(choose_device(&devices, NEEDED).is_none(), "should fall back to CPU");
}

#[test]
fn a_small_discrete_card_loses_to_an_igpu_with_room() {
    let devices = [
        candidate(0, "NVIDIA GeForce GT 1030", true, 1.8),
        candidate(1, "AMD Radeon Graphics", false, 12.0),
    ];
    assert_eq!(
        choose_device(&devices, NEEDED).expect("igpu has room").index,
        1,
        "a discrete card that cannot fit the model is not a candidate"
    );
}

#[test]
fn no_accelerator_means_cpu() {
    assert!(choose_device(&[], NEEDED).is_none());
}

// -- trivial utterances -----------------------------------------------------
//
// Every string below was really dictated on this machine and pulled from the
// journal, so the split is what the model actually has to deal with.

use flow::cleanup::needs_cleanup;

/// Already capitalised, already punctuated, no fillers - the model can only
/// return it unchanged, so paying ~200ms to hear that is waste.
///
/// "Mm-hmm." used to be here. It is pure filler now and never reaches cleanup at
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
        assert!(!needs_cleanup(already_clean), "{already_clean:?} should skip");
    }
}

#[test]
fn anything_the_model_could_fix_still_goes_through() {
    for needs_work in [
        "Hello",             // no terminal punctuation
        "Uh",                // filler, should be deleted entirely
        "Um",
        "Laugh at",          // fragment
        "Okay. Should I be", // trails off
        "See no lay no",     // room tone misheard
        "yeah.",             // not capitalised
        "Um, yeah.",         // punctuated but carries a filler
        "I mean, yes.",      // multi-word filler
        "so i pushed the change to the config and then restarted it",
    ] {
        assert!(needs_cleanup(needs_work), "{needs_work:?} should be cleaned");
    }
}

/// The gate is a latency optimisation for trivial input, so it must never fire on
/// anything long enough to plausibly need a comma.
#[test]
fn longer_utterances_are_never_skipped() {
    let clean_but_long = "One two three four five six seven.";
    assert!(needs_cleanup(clean_but_long), "too long to assume it is finished");
}

#[test]
fn empty_input_needs_nothing() {
    assert!(!needs_cleanup(""));
    assert!(!needs_cleanup("   "));
}

// -- hesitation is not text -------------------------------------------------

use flow::cleanup::is_only_filler;

/// Holding the key and saying "uh" is a pause, not a dictation. It used to reach
/// the model, which deleted the filler, found nothing left, and answered the
/// question it thought it had been asked - pasting the literal word "None."
#[test]
fn a_transcript_of_pure_hesitation_has_nothing_to_write() {
    for hesitation in ["Um", "Uh", "uh", "Um.", "Uh, um", "er", "Ah!", "you know", "I mean"] {
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
