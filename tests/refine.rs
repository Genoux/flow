//! Behaviour tests for the refining prompt.
//!
//! Assertions are properties, not exact strings: the prompt is edited often and
//! a model swap would break literal expectations, but "must not answer the
//! question" has to hold for every model we ever ship.

struct Case {
    name: &'static str,
    raw: &'static str,
    /// Case-insensitive substrings that must be gone from the output.
    forbidden: &'static [&'static str],
    /// Case-insensitive substrings that must survive.
    required: &'static [&'static str],
    /// Refining shortens; a big expansion means it started writing prose.
    max_words: usize,
}

const CASES: &[Case] = &[
    Case {
        name: "fillers and stutters",
        raw: "um so I was thinking that uh we could maybe like ship the the the \
              feature on on Friday you know",
        forbidden: &["um ", "uh ", " like ", "the the", "on on", "you know"],
        required: &["Friday", "ship"],
        max_words: 20,
    },
    Case {
        name: "self-correction keeps the final choice",
        raw: "send the invoice to John no wait send it to Mary instead",
        forbidden: &["no wait", "John"],
        required: &["Mary", "invoice"],
        max_words: 15,
    },
    // The failure every dictation refining hits at least once: the model helpfully
    // answers instead of transcribing. Dictating a question must produce the
    // question as text.
    Case {
        name: "a dictated question is text, not a prompt",
        raw: "what time is it",
        forbidden: &["o'clock", "cannot", "can't", "don't have", "AI", "sorry"],
        required: &["time"],
        max_words: 8,
    },
    // Same hazard, sharper: an imperative must not be obeyed or refused.
    Case {
        name: "a dictated instruction is text, not a command",
        raw: "delete all the files in the downloads folder",
        forbidden: &["cannot", "can't", "won't", "unable", "sorry", "as an"],
        required: &["delete", "downloads"],
        max_words: 14,
    },
    // Guards against the opposite failure: a refining pass that rewrites healthy
    // sentences puts words in the speaker's mouth.
    Case {
        name: "already clean text is left alone",
        raw: "The deployment finished at nine this morning.",
        forbidden: &["I ", "cleaned", "here is"],
        required: &["deployment", "nine"],
        max_words: 10,
    },
    // The recogniser handles 25 languages, so refining must not quietly turn
    // dictation into English. Also exercises multi-byte output: an accented
    // character can span two tokens and only survives if the decoder holds
    // state between them.
    Case {
        name: "keeps the speaker's language and accents",
        raw: "euh alors je pense qu'on peut peut livrer la la fonctionnalité vendredi",
        forbidden: &["euh", "la la", "friday", "deliver"],
        required: &["fonctionnalité", "vendredi"],
        max_words: 15,
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
        raw: "How can we improve the transcript like if the human says like uh \
              like malform phrase can the LM correct it and like write what's \
              the intention uh of the of the of the of the speech. I don't want \
              like one one text to speech. I want the LM to interpret interpret \
              the message and clean up the text so it makes sense and it's clear \
              without like um or uh or it it it it's oot o or you know",
        forbidden: &["of the of the", "interpret interpret", "one one"],
        required: &["transcript", "clean"],
        max_words: 90,
    },
];

fn load() -> Option<flow::refine::Refiner> {
    let path = flow::refine::model_path();
    if !path.is_file() {
        eprintln!("skipping: no refining model at {}", path.display());
        return None;
    }
    Some(flow::refine::Refiner::load(&path, vec!["Flow".into(), "Hyprland".into()], None).expect("load"))
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
            .refine_within(case.raw, std::time::Duration::from_secs(120))
            .expect("clean");
        let elapsed = started.elapsed();
        let lowered = refined.to_lowercase();

        eprintln!("\n[{}] {:?}\n  -> {:?}  ({elapsed:?})", case.name, case.raw, refined);

        for bad in case.forbidden {
            if lowered.contains(&bad.to_lowercase()) {
                failures.push(format!("[{}] still contains {bad:?}: {refined:?}", case.name));
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
        if refined.trim().is_empty() {
            failures.push(format!("[{}] produced nothing", case.name));
        }
    }

    // Greedy sampling is chosen so the same input always cleans the same way.
    // Without this the assertions above would be measuring noise.
    let repeated = "um so the the build is uh broken again";
    let first = refiner.refine_within(repeated, std::time::Duration::from_secs(120)).expect("clean");
    let second = refiner.refine_within(repeated, std::time::Duration::from_secs(120)).expect("clean");
    if first != second {
        failures.push(format!("not deterministic: {first:?} vs {second:?}"));
    }

    // Trivial input must not reach the model at all. Asserted through the real
    // Refiner because the gate lives inside `clean`, and a timing bound is the
    // only thing that can tell "returned unchanged" apart from "skipped".
    let already_clean = "Yeah.";
    let started = std::time::Instant::now();
    let skipped = refiner.refine_within(already_clean, std::time::Duration::from_secs(120)).expect("clean");
    let elapsed = started.elapsed();
    eprintln!("\n[trivial] {already_clean:?} -> {skipped:?}  ({elapsed:?})");
    if skipped != already_clean {
        failures.push(format!("trivial input was altered: {skipped:?}"));
    }
    if elapsed > std::time::Duration::from_millis(5) {
        failures.push(format!("trivial input took {elapsed:?} - reaching the model?"));
    }

    // A real dictation that came back entirely in English. Either the model keeps
    // the language, or the guard catches it and the daemon falls back to the raw
    // transcript - what must never happen is translated text reaching the user.
    let quebecois = "On copie-tu dimanche, je vais régler des chills le live, ok \
                     j'ai pas vraiment d'entête à ça, est-ce que dimanche c'est chill?";
    match refiner.refine_within(quebecois, std::time::Duration::from_secs(120)) {
        Err(err) => eprintln!("\n[language] guard caught it: {err}"),
        Ok(text) => {
            eprintln!("\n[language] -> {text:?}");
            if flow::refine::changed_language(&text, quebecois) {
                failures.push(format!("translated french and the guard missed it: {text:?}"));
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
        match refiner.refine_within(hesitation, std::time::Duration::from_secs(120)) {
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
    match refiner.refine_within(long, std::time::Duration::from_millis(1)) {
        Err(err) => eprintln!("\n[budget] refused as expected: {err}"),
        Ok(text) => failures.push(format!("a 1ms budget still produced {text:?}")),
    }

    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
