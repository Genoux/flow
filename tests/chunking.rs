//! Would transcribing during the hold cost accuracy?
//!
//! Parakeet TDT has no streaming API in parakeet-rs, so getting the transcript
//! before the key comes up means splitting the audio and transcribing the pieces.
//! That is only worth doing if the pieces say the same thing as the whole, so
//! this measures it rather than assuming either way.
//!
//!   cargo test --release --test chunking -- --ignored --nocapture

use std::time::Instant;

/// Longest silence-free run to keep in one piece. Around the length of a spoken
/// sentence, so a split lands between sentences where one is available.
const TARGET: usize = 8 * 16_000;

/// Quietest window worth splitting on, and how much of it to require. Reuses the
/// measured silence floor rather than inventing a second one.
const WINDOW: usize = 16_000 / 20;

/// Split on the quietest gap after `TARGET`, which is where a word boundary is
/// most likely to be.
fn split_on_silence(samples: &[f32]) -> Vec<&[f32]> {
    let mut pieces = Vec::new();
    let mut rest = samples;

    while rest.len() > TARGET + WINDOW * 2 {
        let search = &rest[TARGET..];
        let quietest = search
            .chunks(WINDOW)
            .enumerate()
            .map(|(index, window)| (index, flow::audio::rms(window)))
            .min_by(|a, b| a.1.partial_cmp(&b.1).expect("no NaN"))
            .map(|(index, _)| index)
            .unwrap_or(0);
        let at = (TARGET + quietest * WINDOW + WINDOW / 2).min(rest.len());
        let (piece, tail) = rest.split_at(at);
        pieces.push(piece);
        rest = tail;
    }
    if !rest.is_empty() {
        pieces.push(rest);
    }
    pieces
}

#[test]
#[ignore]
fn does_splitting_the_audio_change_the_words() {
    let dir = flow::stt::model_dir();
    if !dir.is_dir() {
        eprintln!("skipping: no speech model");
        return;
    }
    let mut engine = flow::stt::Stt::load(&dir).expect("load");
    let samples = flow::wav::read_16k_mono("tests/fixtures/jfk.wav").expect("fixture");
    let spoken = samples.len() as f32 / 16_000.0;
    eprintln!("fixture: {spoken:.1}s\n");

    let started = Instant::now();
    let whole = engine.transcribe(samples.clone()).expect("whole");
    let whole_took = started.elapsed();
    eprintln!("whole   ({whole_took:?}): {whole:?}\n");

    let pieces = split_on_silence(&samples);
    eprintln!("split into {} pieces:", pieces.len());
    let mut joined = Vec::new();
    let started = Instant::now();
    for (index, piece) in pieces.iter().enumerate() {
        let text = engine.transcribe(piece.to_vec()).expect("piece");
        eprintln!("  {index} ({:.1}s): {text:?}", piece.len() as f32 / 16_000.0);
        joined.push(text);
    }
    let pieces_took = started.elapsed();
    let joined = joined.join(" ");
    eprintln!("\njoined  ({pieces_took:?}): {joined:?}");

    // Compare on words alone: punctuation and capitalisation are cleanup's job,
    // and a split legitimately changes where sentences appear to end.
    let words = |text: &str| {
        text.split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
    };
    let (before, after) = (words(&whole), words(&joined));
    eprintln!("\nwords: {} whole vs {} joined", before.len(), after.len());
    if before == after {
        eprintln!("VERDICT: identical words - splitting is free");
    } else {
        let differing: Vec<_> = before
            .iter()
            .zip(after.iter())
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(index, (a, b))| format!("{index}: {a:?} vs {b:?}"))
            .take(8)
            .collect();
        eprintln!("VERDICT: words differ - {differing:?}");
    }
}
