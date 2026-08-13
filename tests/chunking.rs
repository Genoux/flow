//! Would transcribing during the hold cost accuracy?
//!
//! Parakeet TDT has no streaming API in parakeet-rs, so getting the transcript
//! before the key comes up means splitting the audio and transcribing the pieces.
//! That is only worth doing if the pieces say the same thing as the whole, so
//! this measures it rather than assuming either way.
//!
//!   cargo test --release --test chunking -- --ignored --nocapture

use std::time::Instant;

/// Same minimum the daemon uses, so this measures the real thing.
const AT_LEAST: usize = 8 * 16_000;

/// Exactly what the daemon does: hand over prefixes until no safe cut remains.
fn split_on_silence(samples: &[f32]) -> Vec<&[f32]> {
    let mut pieces = Vec::new();
    let mut rest = samples;
    while let Some(at) = flow::audio::split_at_silence(rest, AT_LEAST) {
        let (piece, tail) = rest.split_at(at);
        pieces.push(piece);
        rest = tail;
    }
    if !rest.is_empty() {
        pieces.push(rest);
    }
    pieces
}

/// How long the quiet runs actually are in real speech, which is what decides
/// whether the optimisation ever fires.
#[test]
#[ignore]
fn report_the_pauses_in_real_speech() {
    let samples = flow::wav::read_16k_mono("tests/fixtures/jfk.wav").expect("fixture");
    let window = 16_000 / 20;
    let mut runs = Vec::new();
    let mut run = 0;
    for chunk in samples.chunks(window) {
        if flow::audio::rms(chunk) < flow::audio::SILENCE_RMS {
            run += 1;
        } else {
            if run > 0 {
                runs.push(run * 50);
            }
            run = 0;
        }
    }
    if run > 0 {
        runs.push(run * 50);
    }
    runs.sort_unstable();
    eprintln!("quiet runs in jfk.wav, ms: {runs:?}");
    eprintln!("longest: {}ms - the splitter needs 250ms", runs.last().copied().unwrap_or(0));
}

/// Real speech on both sides of a real cut. The fixture alone has no pause long
/// enough to split on, so two of them with a pause between is the closest thing
/// to a long dictation with a breath in the middle.
#[test]
#[ignore]
fn a_cut_in_a_real_pause_keeps_every_word() {
    let dir = flow::stt::model_dir();
    if !dir.is_dir() {
        eprintln!("skipping: no speech model");
        return;
    }
    let mut engine = flow::stt::Stt::load(&dir).expect("load");
    let one = flow::wav::read_16k_mono("tests/fixtures/jfk.wav").expect("fixture");

    let mut samples = one.clone();
    samples.extend(vec![0.0; 16_000 * 3 / 4]);
    samples.extend(one.clone());
    eprintln!("{:.1}s with a 750ms pause in the middle", samples.len() as f32 / 16_000.0);

    let whole = engine.transcribe(samples.clone()).expect("whole");
    let pieces = split_on_silence(&samples);
    eprintln!("split into {} pieces", pieces.len());
    assert!(pieces.len() >= 2, "the pause should have been found");

    let joined = pieces
        .iter()
        .map(|piece| engine.transcribe(piece.to_vec()).expect("piece"))
        .collect::<Vec<_>>()
        .join(" ");

    let words = |text: &str| {
        text.split_whitespace()
            .map(|w| w.trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase())
            .filter(|w| !w.is_empty())
            .collect::<Vec<_>>()
    };
    eprintln!("whole:  {whole:?}");
    eprintln!("joined: {joined:?}");
    assert_eq!(words(&whole), words(&joined), "a cut in a real pause changed the words");
    eprintln!("VERDICT: {} words preserved across a real cut", words(&whole).len());
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
