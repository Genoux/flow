//! Does the vocabulary list fix the names it lists, and only those?
//!
//! Every case is a real transcript from this machine's history, refined while
//! this exact list was in `vocabulary.txt`. The list both recovered "Vercel"
//! and forced it onto words that were never it - TanStack, Claude, a creator
//! credit - so both directions are graded.
//!
//! Run explicitly:
//!   cargo test --release --test vocabulary -- --nocapture --ignored

use flow::refine::{Cleanup, Style};

const VOCABULARY: [&str; 9] = [
    "Satty",
    "DuckDuckGo",
    "Wayland",
    "Vercel",
    "Gemini",
    "LLM",
    "wd",
    "wdq",
    "qwd",
];

const RECOVERED: &[(&str, &str)] = &[
    (
        "Okay, and on Slack I have a versal that post uh when there's a deploy done so uh it's a channel for dev activity but uh it works on another project but not on ad collection how how to add it to Slack on ad collection is it on Versale",
        "Vercel",
    ),
    (
        "Why is the main deploy production is not linked to the official ad collection co domain?  Can you check on Versa",
        "Vercel",
    ),
    (
        "Okay, so I have this project on Brasil and it the deployment doesn't work.  Can you make sure that the GitHub uh report on Vursale for this project uh deployed",
        "Vercel",
    ),
];

const LEFT_ALONE: &[&str] = &[
    "Okay, well I used the ten stack tool and like there's no error view, there's no loading state like what happen is uh the loading fail like what's the view like one of the I want to see that I want to test that",
    "Hi Drager credit if there is no creator credit to show",
    "When we load the app can we make sure that clot is up to date so we are we have the most uh up to date model and also I would like to be able to edit the thinking",
    "How can I strategize to contact Flexy support to get a free E Seven Pro table",
    "So I want you to tell me if I can go and spell on cane or not",
];

#[test]
#[ignore = "calls OpenRouter with the configured key"]
fn vocabulary_fixes_listed_names_and_nothing_else() {
    let Some(key) = flow::config::Config::load().openrouter_key else {
        eprintln!("skipping: no OpenRouter key configured");
        return;
    };
    let refiner = flow::refine::Refiner::new(key);
    let style = Style::new(Cleanup::Light).with_vocabulary(VOCABULARY.map(str::to_string).to_vec());
    let refine = |raw: &str| {
        refiner
            .refine_within(raw, std::time::Duration::from_secs(120), &style)
            .expect("refine")
    };

    let missed: Vec<String> = RECOVERED
        .iter()
        .map(|(raw, name)| (refine(raw), *name))
        .inspect(|(out, _)| eprintln!("recover: {out}"))
        .filter(|(out, name)| !out.contains(name))
        .map(|(out, _)| out)
        .collect();
    let forced: Vec<String> = LEFT_ALONE
        .iter()
        .map(|raw| refine(raw))
        .inspect(|out| eprintln!("leave:   {out}"))
        .filter(|out| out.contains("Vercel"))
        .collect();

    assert!(
        missed.is_empty() && forced.is_empty(),
        "missed a listed name: {missed:#?}\nforced a listed name: {forced:#?}"
    );
}
