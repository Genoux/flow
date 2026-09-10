use anyhow::{Context, Result};
use parakeet_rs::Nemotron;
use std::path::Path;
use std::time::Instant;

pub struct Stt {
    model: Nemotron,
}

impl Stt {
    pub fn load(dir: &Path) -> Result<Self> {
        let started = Instant::now();
        let model = Nemotron::from_pretrained(dir, None)
            .with_context(|| format!("loading speech model from {}", dir.display()))?;
        eprintln!(
            "speech model loaded in {:?} (nemotron, cpu, {}ms streaming chunks)",
            started.elapsed(),
            model.chunk_samples() * 1000 / crate::audio::SAMPLE_RATE as usize
        );
        Ok(Self { model })
    }

    pub fn chunk_samples(&self) -> usize {
        self.model.chunk_samples()
    }

    pub fn start_stream(&mut self) {
        self.model.reset();
    }

    pub fn stream_chunk(&mut self, audio: &[f32]) -> Result<()> {
        anyhow::ensure!(
            audio.len() == self.chunk_samples(),
            "incorrect streaming chunk size"
        );
        self.model.transcribe_chunk(audio)?;
        Ok(())
    }

    pub fn finish_stream(&mut self, audio: &[f32]) -> Result<String> {
        let size = self.chunk_samples();
        let mut chunks = audio.chunks_exact(size);
        for chunk in &mut chunks {
            self.stream_chunk(chunk)?;
        }
        let remainder = chunks.remainder();
        if !remainder.is_empty() {
            // parakeet-rs 0.3.7 has no flush API and decodes only full chunks.
            let mut padded = vec![0.0; size];
            padded[..remainder.len()].copy_from_slice(remainder);
            self.stream_chunk(&padded)?;
        }
        Ok(self.model.get_transcript().trim().to_string())
    }

    pub fn transcribe(&mut self, audio: Vec<f32>) -> Result<String> {
        self.model.reset();
        Ok(self.model.transcribe_audio(&audio)?.trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_missing_model_directory_is_a_clear_error_not_a_panic() {
        let dir = std::env::temp_dir().join(format!("flow-no-such-model-{}", std::process::id()));
        let Err(err) = Stt::load(&dir) else {
            panic!("a missing model directory must not load successfully");
        };
        assert!(err.to_string().contains("loading speech model"), "{err}");
    }
}
