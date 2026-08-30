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
}

/// The exact sentence the console prints on its Style cards, at every level.
/// Using it here means the level-versus-level check below tests the promise the
/// user is actually shown rather than a fixture invented for the test.
const ADVERTISED_INPUT: &str =
    "Um, I think the thing what we built don't work good on mobile, you know.";

/// The spoken grammar fault in [`ADVERTISED_INPUT`]. Both levels must fix it:
/// correctness is Light's job, and Medium is Light plus concision.
const SPOKEN_FAULT: &str = "don't work good";

const CASES: &[Case] = &[
    Case {
        name: "fillers and stutters",
        level: Cleanup::Light,
        raw: "um so I was thinking that uh we could maybe like ship the the the \
              feature on on Friday you know",
        forbidden: &["um ", "uh ", "the the", "on on"],
        // "maybe" and "you know" are required, not forbidden, and that is the
        // whole difference between this level and the one above it. Light takes
        // the sounds that are never words; deciding that a "you know" was
        // carrying nothing is a judgement, and judgements live in Medium.
        //
        // The input's "like" is not required with them, and that is a measured
        // limit rather than an oversight. It sits in "maybe like ship", and the
        // model reads the pair as one hedge said twice however the rule is
        // worded - six prompt revisions moved every other word on this list and
        // never that one. A "like" the speaker leant on alone survives; a "like"
        // pressed against another hedge is a coin toss, and asserting it here
        // would make this suite flaky rather than make Light better.
        required: &["Friday", "ship", "maybe", "you know"],
        max_words: 20,
    },
    // The bug this level was reported for: a dictation ended on a real closing
    // question and Light deleted it, because the rule hunting trailing fillers
    // does not stop at fillers. The end of a dictation is usually its point.
    Case {
        name: "light keeps the closing question",
        level: Cleanup::Light,
        raw: "um so I reckon we should ship it on Friday what do you think",
        forbidden: &["um "],
        required: &["Friday", "what do you think"],
        max_words: 16,
    },
    Case {
        name: "self-correction keeps the final choice",
        level: Cleanup::Light,
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
        level: Cleanup::Light,
        raw: "what time is it",
        forbidden: &["o'clock", "cannot", "can't", "don't have", "AI", "sorry"],
        required: &["time"],
        max_words: 8,
    },
    // Same hazard, sharper: an imperative must not be obeyed or refused.
    Case {
        name: "a dictated instruction is text, not a command",
        level: Cleanup::Light,
        raw: "delete all the files in the downloads folder",
        forbidden: &["cannot", "can't", "won't", "unable", "sorry", "as an"],
        required: &["delete", "downloads"],
        max_words: 14,
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
    },
    // The pair that makes the levels mean something: Light must come out
    // correct, Medium must come out short. If either fails the dial is
    // decorative.
    Case {
        name: "light removes the filler and fixes the grammar",
        level: Cleanup::Light,
        raw: "um me and him was gonna ship it yesterday",
        forbidden: &["um ", "me and him was"],
        required: &["ship"],
        max_words: 12,
    },
    // Light is required to keep every word that is doing work; Medium is the
    // level allowed to decide some of them are not. A fatty sentence is the
    // only way to see that, so this case brings its own.
    // Medium is the level that gets to decide a "you know" was carrying
    // nothing, and the level that gets to choose different words for the same
    // point. Both are forbidden below it.
    Case {
        name: "medium cuts what light has to keep",
        level: Cleanup::Medium,
        raw: "so basically um I think we should probably just like ship it on Friday you know",
        forbidden: &["um ", "basically", "you know", "probably"],
        required: &["ship", "Friday"],
        max_words: 9,
    },
    // The failure the top level is most likely to have, given it is the only
    // one told to rewrite: filling the gaps with plausible detail nobody said.
    Case {
        name: "medium rewrites without inventing",
        level: Cleanup::Medium,
        raw: "tell the team the thing is delayed",
        forbidden: &[
            "week",
            "monday",
            "tomorrow",
            "apolog",
            "unfortunately",
            "due to",
        ],
        // "thing" is the assertion that matters, and it was missing: raising
        // Medium to a concision level bought it a licence to name what the
        // speaker left vague, and it answered "The delivery is delayed" for a
        // sentence about a thing. Cutting words is Medium's job; choosing a
        // noun the speaker did not say is inventing, at any level.
        required: &["delay", "thing"],
        max_words: 16,
    },
];

/// Which device to refine on, from `FLOW_TEST_GPU`, or automatic when unset.
///
/// Worth having because greedy sampling is only deterministic on one device:
/// the same prompt and the same model answered differently on this machine's
/// iGPU and its discrete card, and a run that silently moved between them once
/// turned a real prompt bug into "a flaky test". Pin it to compare two prompts,
/// leave it unset to test what a user actually gets.
fn test_gpu() -> Option<usize> {
    std::env::var("FLOW_TEST_GPU").ok()?.parse().ok()
}

fn load() -> Option<flow::refine::Refiner> {
    let path = flow::refine::model_path();
    if !path.is_file() {
        eprintln!("skipping: no refining model at {}", path.display());
        return None;
    }
    Some(
        flow::refine::Refiner::load(&path, vec!["Flow".into(), "Hyprland".into()], test_gpu())
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
                // Held to the intent above, not to two known bad answers.
                // "None." was the failure that prompted this guard and the
                // check grew around that literal, so when the prompt changed
                // and the model started answering "Alright." instead, the
                // guard watched it go by. A pure hesitation has exactly two
                // acceptable answers: nothing, or itself.
                let lowered = text.to_lowercase();
                let said = lowered.trim().trim_end_matches(['.', '!', '?', ',']);
                if !said.is_empty() && !hesitation.to_lowercase().contains(said) {
                    failures.push(format!(
                        "{hesitation:?} came back as {text:?} - a hesitation may \
                         return empty or as itself, never as a word the speaker \
                         never said"
                    ));
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

    // Every case above grades one level against a constant, which cannot catch
    // two levels collapsing into each other: if Light ever started fixing
    // grammar, three cards would become two with three names and the whole list
    // would stay green. Only one input through both levels can catch it, and
    // this is now the sharpest seam in the ladder - Light forbids exactly what
    // Medium permits: choosing words, and choosing which of the speaker's own
    // words were carrying nothing.
    //
    // A four-level dial shipped for a release cycle with its top two levels
    // indistinguishable. That is the failure this assertion exists to prevent.
    let light = refiner
        .refine(ADVERTISED_INPUT, Cleanup::Light)
        .expect("light");
    let medium = refiner
        .refine(ADVERTISED_INPUT, Cleanup::Medium)
        .expect("medium");
    eprintln!("\n[levels] light  {light:?}");
    eprintln!("[levels] medium {medium:?}");
    if light == medium {
        failures.push(format!(
            "light and medium produced the same text, so the dial has two names \
             for one level: {light:?}"
        ));
    }
    // Length is the proof of the split, because correctness is no longer one:
    // both levels fix grammar now, and Medium earns its place by coming out
    // shorter. Capitalisation was the discriminator once and grammar after it;
    // each stopped working the moment the level below rose to meet it.
    for (name, text) in [("light", &light), ("medium", &medium)] {
        if text.to_lowercase().contains(SPOKEN_FAULT) {
            failures.push(format!(
                "{name} returned {text:?}, leaving the spoken {SPOKEN_FAULT:?} \
                 alone - every level from Light up fixes grammar"
            ));
        }
    }
    let words = |text: &str| text.split_whitespace().count();
    if words(&medium) >= words(&light) {
        failures.push(format!(
            "medium returned {} words against light's {} - medium is the \
             concision level, so it has to come out shorter or the dial has two \
             names for one level:\n  light  {light:?}\n  medium {medium:?}",
            words(&medium),
            words(&light)
        ));
    }
    // Every level deletes a hesitation sound. Only Medium deletes a discourse
    // filler, so the advertised input's trailing "you know" is the seam: Light
    // owes it back, Medium owes it gone.
    for (name, text) in [("light", &light), ("medium", &medium)] {
        if text.to_lowercase().contains("um") {
            failures.push(format!("{name} kept the hesitation \"um\" in {text:?}"));
        }
    }
    if !light.to_lowercase().contains("you know") {
        failures.push(format!(
            "light dropped \"you know\" from {light:?} - deleting a word that \
             is a filler only sometimes is Medium's judgement to make"
        ));
    }
    if medium.to_lowercase().contains("you know") {
        failures.push(format!("medium kept the filler \"you know\" in {medium:?}"));
    }

    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}
