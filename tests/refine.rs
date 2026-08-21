//! Behaviour tests for the refining prompt.
//!
//! Assertions are properties, not exact strings: the prompt is edited often and
//! a model swap would break literal expectations, but "must not answer the
//! question" has to hold for every model we ever ship.

use flow::refine::Cleanup;

struct Case {
    name: &'static str,
    raw: &'static str,
    /// Which level this case describes. Most are disfluency removal, which is
    /// every level's job and so is tested at the default.
    level: Cleanup,
    /// Case-insensitive substrings that must be gone from the output.
    forbidden: &'static [&'static str],
    /// Case-insensitive substrings that must survive.
    required: &'static [&'static str],
    /// Refining shortens; a big expansion means it started writing prose.
    max_words: usize,
    /// Ceiling on sentences in the output, for the levels that claim to
    /// restructure. `None` means the level makes no such claim and the sentence
    /// shape of the input is expected to survive.
    max_sentences: Option<usize>,
}

/// Shared by the Hard case above and the level-versus-level check below, which
/// is the assertion that actually earns Hard its card in the console: the same
/// input, both levels, Hard strictly shorter.
const RESTRUCTURE_INPUT: &str = "the cache is cold on every deploy. and uh we also \
     see the p99 spike. it's about four seconds now. anyway what I'm saying is we \
     need to warm the cache in the build step. that's the fix.";

const CASES: &[Case] = &[
    Case {
        name: "fillers and stutters",
        level: Cleanup::Light,
        raw: "um so I was thinking that uh we could maybe like ship the the the \
              feature on on Friday you know",
        forbidden: &["um ", "uh ", " like ", "the the", "on on", "you know"],
        required: &["Friday", "ship"],
        max_words: 20,
        max_sentences: None,
    },
    Case {
        name: "self-correction keeps the final choice",
        level: Cleanup::Light,
        raw: "send the invoice to John no wait send it to Mary instead",
        forbidden: &["no wait", "John"],
        required: &["Mary", "invoice"],
        max_words: 15,
        max_sentences: None,
    },
    // The failure every dictation refining hits at least once: the model helpfully
    // answers instead of transcribing. Dictating a question must produce the
    // question as text.
    Case {
        name: "a dictated question is text, not a prompt",
        level: Cleanup::Light,
        raw: "what time is it",
        forbidden: &["o'clock", "cannot", "can't", "don't have", "AI", "sorry"],
        required: &["time"],
        max_words: 8,
        max_sentences: None,
    },
    // Same hazard, sharper: an imperative must not be obeyed or refused.
    Case {
        name: "a dictated instruction is text, not a command",
        level: Cleanup::Light,
        raw: "delete all the files in the downloads folder",
        forbidden: &["cannot", "can't", "won't", "unable", "sorry", "as an"],
        required: &["delete", "downloads"],
        max_words: 14,
        max_sentences: None,
    },
    // Guards against the opposite failure: a refining pass that rewrites healthy
    // sentences puts words in the speaker's mouth.
    Case {
        name: "already clean text is left alone",
        level: Cleanup::Light,
        raw: "The deployment finished at nine this morning.",
        forbidden: &["I ", "cleaned", "here is"],
        required: &["deployment", "nine"],
        max_words: 10,
        max_sentences: None,
    },
    // The recogniser handles 25 languages, so refining must not quietly turn
    // dictation into English. Also exercises multi-byte output: an accented
    // character can span two tokens and only survives if the decoder holds
    // state between them.
    Case {
        name: "keeps the speaker's language and accents",
        level: Cleanup::Light,
        raw: "euh alors je pense qu'on peut peut livrer la la fonctionnalité vendredi",
        forbidden: &["euh", "la la", "friday", "deliver"],
        required: &["fonctionnalité", "vendredi"],
        max_words: 15,
        max_sentences: None,
    },
    // Verbatim from the user asking for this feature - a real malformed
    // dictation, with a meaning a human can state but the words never do.
    //
    // Note what is NOT forbidden here. This sentence is *about* fillers, so a
    // correct refining still quotes "um" and repeated words as content. Banning
    // them outright would fail the model for understanding the sentence, which
    // is the opposite of what this suite is meant to protect. Only structural
    // damage - the speaker's own stumbles - is forbidden.
    Case {
        name: "real dictation: garbled with recoverable intent",
        level: Cleanup::Light,
        raw: "How can we improve the transcript like if the human says like uh \
              like malform phrase can the LM correct it and like write what's \
              the intention uh of the of the of the of the speech. I don't want \
              like one one text to speech. I want the LM to interpret interpret \
              the message and clean up the text so it makes sense and it's clear \
              without like um or uh or it it it it's oot o or you know",
        forbidden: &["of the of the", "interpret interpret", "one one"],
        required: &["transcript", "clean"],
        max_words: 90,
        max_sentences: None,
    },
    // The pair that makes the levels mean something. Same input, same model,
    // and the only difference is which rules block the prompt carries.
    //
    // If either of these fails the dial is decorative: an instruct model asked
    // to "clean up" a transcript fixes grammar unprompted because that reads as
    // helpful, so Light has to forbid what Medium permits or both levels emit
    // the same sentence.
    Case {
        name: "light removes the filler but leaves the grammar alone",
        level: Cleanup::Light,
        raw: "um me and him was gonna ship it yesterday",
        forbidden: &["um "],
        required: &["me and him was"],
        max_words: 12,
        max_sentences: None,
    },
    Case {
        name: "medium fixes the grammar light left alone",
        level: Cleanup::Medium,
        raw: "um me and him was gonna ship it yesterday",
        forbidden: &["um ", "me and him was"],
        required: &["ship"],
        max_words: 12,
        max_sentences: None,
    },
    // Hard's whole claim is restructuring, so the input needs something to
    // restructure: the speaker states three symptoms, then arrives at the point
    // last, wrapped in spoken scaffolding ("anyway what I'm saying is").
    //
    // An input already in reading order will not do. Asking Hard to merge
    // sentences that read fine either way measures nothing, and a fixture like
    // that failed this test for a whole release cycle while HARD_RULES was
    // working correctly. See RESTRUCTURE_INPUT for the level-versus-level check.
    Case {
        name: "hard restructures what medium may only tidy",
        level: Cleanup::Hard,
        raw: RESTRUCTURE_INPUT,
        forbidden: &[
            "uh ",
            "anyway",
            "what i'm saying",
            "sorry",
            "as an",
            "regards",
        ],
        required: &["cache", "build step"],
        max_words: 40,
        // Five sentences in. Two or fewer out is the machine-checkable proof
        // that Hard merged and reordered, which Medium is forbidden to do.
        max_sentences: Some(2),
    },
    // The failure Hard is most likely to have, given it is the only level told
    // to rewrite: filling the gaps with plausible detail nobody said.
    Case {
        name: "hard rewrites without inventing",
        level: Cleanup::Hard,
        raw: "tell the team the thing is delayed",
        forbidden: &[
            "week",
            "monday",
            "tomorrow",
            "apolog",
            "unfortunately",
            "due to",
        ],
        required: &["delay"],
        max_words: 16,
        max_sentences: None,
    },
];

fn sentences(text: &str) -> usize {
    text.split(['.', '!', '?'])
        .filter(|part| !part.trim().is_empty())
        .count()
}

fn load() -> Option<flow::refine::Refiner> {
    let path = flow::refine::model_path();
    if !path.is_file() {
        eprintln!("skipping: no refining model at {}", path.display());
        return None;
    }
    Some(
        flow::refine::Refiner::load(&path, vec!["Flow".into(), "Hyprland".into()], None)
            .expect("load"),
    )
}

/// One test, not several: cargo runs tests as parallel threads, and two
/// concurrent model loads on the same Vulkan device segfault. Loading once is
/// also how the daemon behaves.
#[test]
fn refining_behaves() {
    let Some(refiner) = load() else { return };
    let mut failures = Vec::new();

    for case in CASES {
        let started = std::time::Instant::now();
        let refined = refiner
            .refine_within(case.raw, std::time::Duration::from_secs(120), case.level)
            .expect("clean");
        let elapsed = started.elapsed();
        let lowered = refined.to_lowercase();

        eprintln!(
            "\n[{}] ({}) {:?}\n  -> {:?}  ({elapsed:?})",
            case.name,
            case.level.as_str(),
            case.raw,
            refined
        );

        for bad in case.forbidden {
            if lowered.contains(&bad.to_lowercase()) {
                failures.push(format!(
                    "[{}] still contains {bad:?}: {refined:?}",
                    case.name
                ));
            }
        }
        for good in case.required {
            if !lowered.contains(&good.to_lowercase()) {
                failures.push(format!("[{}] lost {good:?}: {refined:?}", case.name));
            }
        }
        let words = refined.split_whitespace().count();
        if words > case.max_words {
            failures.push(format!(
                "[{}] grew to {words} words (max {}): {refined:?}",
                case.name, case.max_words
            ));
        }
        if let Some(ceiling) = case.max_sentences {
            let sentences = sentences(&refined);
            if sentences > ceiling {
                failures.push(format!(
                    "[{}] left {sentences} sentences (max {ceiling}), so it tidied \
                     rather than restructured: {refined:?}",
                    case.name
                ));
            }
        }
        if refined.trim().is_empty() {
            failures.push(format!("[{}] produced nothing", case.name));
        }
    }

    // Greedy sampling is chosen so the same input always cleans the same way.
    // Without this the assertions above would be measuring noise.
    let repeated = "um so the the build is uh broken again";
    let first = refiner
        .refine_within(
            repeated,
            std::time::Duration::from_secs(120),
            Cleanup::Light,
        )
        .expect("clean");
    let second = refiner
        .refine_within(
            repeated,
            std::time::Duration::from_secs(120),
            Cleanup::Light,
        )
        .expect("clean");
    if first != second {
        failures.push(format!("not deterministic: {first:?} vs {second:?}"));
    }

    // Trivial input must not reach the model at all. Asserted through the real
    // Refiner because the gate lives inside `clean`, and a timing bound is the
    // only thing that can tell "returned unchanged" apart from "skipped".
    let already_clean = "Yeah.";
    let started = std::time::Instant::now();
    let skipped = refiner
        .refine_within(
            already_clean,
            std::time::Duration::from_secs(120),
            Cleanup::Light,
        )
        .expect("clean");
    let elapsed = started.elapsed();
    eprintln!("\n[trivial] {already_clean:?} -> {skipped:?}  ({elapsed:?})");
    if skipped != already_clean {
        failures.push(format!("trivial input was altered: {skipped:?}"));
    }
    if elapsed > std::time::Duration::from_millis(5) {
        failures.push(format!(
            "trivial input took {elapsed:?} - reaching the model?"
        ));
    }

    // A real dictation that came back entirely in English. Either the model keeps
    // the language, or the guard catches it and the daemon falls back to the raw
    // transcript - what must never happen is translated text reaching the user.
    let quebecois = "On copie-tu dimanche, je vais régler des chills le live, ok \
                     j'ai pas vraiment d'entête à ça, est-ce que dimanche c'est chill?";
    match refiner.refine_within(
        quebecois,
        std::time::Duration::from_secs(120),
        Cleanup::Light,
    ) {
        Err(err) => eprintln!("\n[language] guard caught it: {err}"),
        Ok(text) => {
            eprintln!("\n[language] -> {text:?}");
            if flow::refine::changed_language(&text, quebecois) {
                failures.push(format!(
                    "translated french and the guard missed it: {text:?}"
                ));
            }
            if text.to_lowercase().contains("sunday") {
                failures.push(format!("translated 'dimanche' to Sunday: {text:?}"));
            }
        }
    }

    // Pure hesitation reached the model and came back as the literal word
    // "None." - it deleted the filler, found nothing left, and answered. The
    // daemon never sends these now, but the guard has to hold if one slips
    // through, because pasting an invented word is the worst outcome here.
    for hesitation in ["Um", "Uh", "Er"] {
        match refiner.refine_within(
            hesitation,
            std::time::Duration::from_secs(120),
            Cleanup::Light,
        ) {
            Err(err) => eprintln!("\n[filler] {hesitation:?} refused: {err}"),
            Ok(text) => {
                eprintln!("\n[filler] {hesitation:?} -> {text:?}");
                let lowered = text.to_lowercase();
                if lowered.starts_with("none") || lowered.starts_with("n/a") {
                    failures.push(format!("{hesitation:?} produced the non-answer {text:?}"));
                }
            }
        }
    }

    // An impossible budget must fail rather than return half a sentence: main.rs
    // treats the error as "use the raw transcript", and a truncated refining would
    // be worse than the transcript it replaced.
    let long = "um so i pushed the change and then uh the build broke again";
    match refiner.refine_within(long, std::time::Duration::from_millis(1), Cleanup::Light) {
        Err(err) => eprintln!("\n[budget] refused as expected: {err}"),
        Ok(text) => failures.push(format!("a 1ms budget still produced {text:?}")),
    }

    // The assertion that earns Hard its own card in the console. The case list
    // above can only check Hard against a fixed number; if Medium ever learned
    // to restructure, four cards would become three with two names and every
    // case above would still pass. Only running one input through both levels
    // and comparing them can catch that.
    let medium = refiner
        .refine(RESTRUCTURE_INPUT, Cleanup::Medium)
        .expect("medium");
    let hard = refiner
        .refine(RESTRUCTURE_INPUT, Cleanup::Hard)
        .expect("hard");
    eprintln!("\n[levels] medium ({} sent) {medium:?}", sentences(&medium));
    eprintln!("[levels] hard   ({} sent) {hard:?}", sentences(&hard));
    if sentences(&hard) >= sentences(&medium) {
        failures.push(format!(
            "hard left {} sentences and medium left {}, so hard restructured no \
             more than the level below it and the console should offer three \
             cards, not four:\n  medium: {medium:?}\n  hard:   {hard:?}",
            sentences(&hard),
            sentences(&medium)
        ));
    }

    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
