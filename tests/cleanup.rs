//! Behaviour tests for the cleanup prompt.
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
    /// Cleanup shortens; a big expansion means it started writing prose.
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
    // The failure every dictation cleanup hits at least once: the model helpfully
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
    // Guards against the opposite failure: a cleanup pass that rewrites healthy
    // sentences puts words in the speaker's mouth.
    Case {
        name: "already clean text is left alone",
        raw: "The deployment finished at nine this morning.",
        forbidden: &["I ", "cleaned", "here is"],
        required: &["deployment", "nine"],
        max_words: 10,
    },
    // The recogniser handles 25 languages, so cleanup must not quietly turn
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
    // correct cleanup still quotes "um" and repeated words as content. Banning
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

fn load() -> Option<flow::cleanup::Cleaner> {
    let path = flow::cleanup::model_path();
    if !path.is_file() {
        eprintln!("skipping: no cleanup model at {}", path.display());
        return None;
    }
    Some(flow::cleanup::Cleaner::load(&path, vec!["Flow".into(), "Hyprland".into()], None).expect("load"))
}

/// One test, not several: cargo runs tests as parallel threads, and two
/// concurrent model loads on the same Vulkan device segfault. Loading once is
/// also how the daemon behaves.
#[test]
fn cleanup_behaves() {
    let Some(cleaner) = load() else { return };
    let mut failures = Vec::new();

    for case in CASES {
        let started = std::time::Instant::now();
        let cleaned = cleaner.clean(case.raw).expect("clean");
        let elapsed = started.elapsed();
        let lowered = cleaned.to_lowercase();

        eprintln!("\n[{}] {:?}\n  -> {:?}  ({elapsed:?})", case.name, case.raw, cleaned);

        for bad in case.forbidden {
            if lowered.contains(&bad.to_lowercase()) {
                failures.push(format!("[{}] still contains {bad:?}: {cleaned:?}", case.name));
            }
        }
        for good in case.required {
            if !lowered.contains(&good.to_lowercase()) {
                failures.push(format!("[{}] lost {good:?}: {cleaned:?}", case.name));
            }
        }
        let words = cleaned.split_whitespace().count();
        if words > case.max_words {
            failures.push(format!(
                "[{}] grew to {words} words (max {}): {cleaned:?}",
                case.name, case.max_words
            ));
        }
        if cleaned.trim().is_empty() {
            failures.push(format!("[{}] produced nothing", case.name));
        }
    }

    // Greedy sampling is chosen so the same input always cleans the same way.
    // Without this the assertions above would be measuring noise.
    let repeated = "um so the the build is uh broken again";
    let first = cleaner.clean(repeated).expect("clean");
    let second = cleaner.clean(repeated).expect("clean");
    if first != second {
        failures.push(format!("not deterministic: {first:?} vs {second:?}"));
    }

    // Trivial input must not reach the model at all. Asserted through the real
    // Cleaner because the gate lives inside `clean`, and a timing bound is the
    // only thing that can tell "returned unchanged" apart from "skipped".
    let already_clean = "Yeah.";
    let started = std::time::Instant::now();
    let skipped = cleaner.clean(already_clean).expect("clean");
    let elapsed = started.elapsed();
    eprintln!("\n[trivial] {already_clean:?} -> {skipped:?}  ({elapsed:?})");
    if skipped != already_clean {
        failures.push(format!("trivial input was altered: {skipped:?}"));
    }
    if elapsed > std::time::Duration::from_millis(5) {
        failures.push(format!("trivial input took {elapsed:?} - reaching the model?"));
    }

    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
