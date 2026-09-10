use anyhow::{Result, bail};
use std::time::Duration;

/// How much audio goes in one request.
///
/// OpenRouter's upstreams time out at 60 seconds of *processing* per request,
/// and their own guidance is to split anything longer. Recognition is far
/// faster than realtime, so a minute of audio is nowhere near a minute of
/// processing - but the ceiling is theirs, not ours, and a long explanation is
/// exactly the dictation worth not losing. Forty-five seconds leaves room
/// under it without cutting more often than it has to.
const CHUNK_SECONDS: usize = 45;

/// Never cut before this much audio: a cut needs somewhere quiet to land, and
/// the shorter the prefix the more likely the only quiet spot is mid-sentence.
/// Same floor `tests/chunking.rs` measured the local splitter against.
const AT_LEAST_SECONDS: usize = 8;

/// Per request, not per dictation. A dictation long enough to be split gets
/// this much for each of its pieces.
const TIMEOUT: Duration = Duration::from_secs(90);

pub struct Stt {
    key: String,
}

impl Stt {
    /// The key is copied in rather than read per dictation: it is settled when
    /// the daemon starts, and re-reading the config on the dictation path would
    /// put a file read in front of every hotkey press.
    pub fn new(key: String) -> Self {
        Self { key }
    }

    /// Transcribe one utterance.
    ///
    /// Audio arrives as the daemon's normalised 16 kHz mono samples and leaves
    /// as text, the same shape the local recogniser had - every caller was
    /// already funnelled through this one call.
    pub fn transcribe(&mut self, audio: Vec<f32>) -> Result<String> {
        if self.key.is_empty() {
            bail!("no OpenRouter key - add one in Settings");
        }

        let pieces = split(&audio);
        let mut parts = Vec::with_capacity(pieces.len());
        for piece in pieces {
            let wav = super::wav::encode_16k_mono(piece)?;
            let text = super::router::transcribe(&self.key, &wav, TIMEOUT)?;
            if !text.is_empty() {
                parts.push(text);
            }
        }
        Ok(parts.join(" "))
    }
}

/// Cut a long dictation into request-sized pieces at pauses.
///
/// Cutting at a pause rather than at a stopwatch is what keeps the seam from
/// landing inside a word: `split_at_silence` is the same function the local
/// path used to get an early transcript during a long hold, and
/// `tests/chunking.rs` is where the choice of a real gap over a fixed offset
/// was measured.
///
/// A dictation with no pause in it cannot be cut safely, so it is sent whole
/// and the provider decides. Better a request that may time out than a
/// transcript with a word sliced in half.
fn split(samples: &[f32]) -> Vec<&[f32]> {
    let rate = super::audio::SAMPLE_RATE as usize;
    let limit = CHUNK_SECONDS * rate;
    if samples.len() <= limit {
        return vec![samples];
    }

    let mut pieces = Vec::new();
    let mut rest = samples;
    while rest.len() > limit {
        // Look for a pause inside the piece we are allowed to send, not in
        // whatever is left: a cut past the limit is a piece too long to send.
        let head = &rest[..limit];
        let Some(at) = super::audio::split_at_silence(head, AT_LEAST_SECONDS * rate) else {
            break;
        };
        let (piece, tail) = rest.split_at(at);
        pieces.push(piece);
        rest = tail;
    }
    if !rest.is_empty() {
        pieces.push(rest);
    }
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loud(seconds: f32) -> Vec<f32> {
        // Alternating sign so this is not mistaken for silence by anything
        // measuring rms.
        (0..(seconds * super::super::audio::SAMPLE_RATE as f32) as usize)
            .map(|i| if i % 2 == 0 { 0.4 } else { -0.4 })
            .collect()
    }

    fn quiet(seconds: f32) -> Vec<f32> {
        vec![0.0; (seconds * super::super::audio::SAMPLE_RATE as f32) as usize]
    }

    #[test]
    fn a_normal_dictation_is_one_request() {
        let short = loud(10.0);
        assert_eq!(split(&short).len(), 1);
        // Exactly at the limit is still one: the ceiling is inclusive.
        let exact = loud(CHUNK_SECONDS as f32);
        assert_eq!(split(&exact).len(), 1);
    }

    #[test]
    fn a_long_dictation_is_cut_at_a_pause() {
        let mut samples = loud(40.0);
        samples.extend(quiet(1.0));
        samples.extend(loud(40.0));

        let pieces = split(&samples);
        assert!(pieces.len() > 1, "81 seconds went out as one request");
        let rate = super::super::audio::SAMPLE_RATE as usize;
        for piece in &pieces {
            assert!(
                piece.len() <= CHUNK_SECONDS * rate,
                "piece of {}s exceeds the request ceiling",
                piece.len() / rate
            );
        }
        // Nothing may be dropped on the floor between pieces.
        assert_eq!(
            pieces.iter().map(|p| p.len()).sum::<usize>(),
            samples.len(),
            "audio went missing at a seam"
        );
    }

    #[test]
    fn unbroken_speech_is_sent_whole_rather_than_sliced_mid_word() {
        // No pause anywhere, so there is no safe cut. One oversized request
        // that may fail beats a transcript with a word cut in half.
        let unbroken = loud(90.0);
        assert_eq!(split(&unbroken).len(), 1);
    }

    #[test]
    fn a_missing_key_is_named_not_a_silent_failure() {
        let err = Stt::new(String::new())
            .transcribe(vec![0.0; 16])
            .unwrap_err()
            .to_string();
        assert!(err.contains("Settings"), "{err}");
    }
}
