use anyhow::{Context, Result};
use parakeet_rs::{ParakeetTDT, Transcriber};
use std::path::{Path, PathBuf};
use std::time::Instant;

pub fn model_dir() -> PathBuf {
    flow_paths::speech_model_dir()
}

pub struct Stt {
    model: ParakeetTDT,
}

impl Stt {
    /// Runs on CPU. Measured at ~23x realtime with the int8 TDT model on 16 cores,
    /// which leaves the whole GPU free for the cleanup model. The `cuda` feature is
    /// deliberately not enabled: ort falls back to CPU silently when the CUDA runtime
    /// is absent, so enabling it buys a misleading log line and a 3-5GB build dep.
    pub fn load(dir: &Path) -> Result<Self> {
        let started = Instant::now();
        let model = ParakeetTDT::from_pretrained(dir, None)
            .with_context(|| format!("loading model from {}", dir.display()))?;
        eprintln!("model loaded in {:?}", started.elapsed());
        Ok(Self { model })
    }

    pub fn transcribe(&mut self, audio: Vec<f32>) -> Result<String> {
        let result = self
            .model
            .transcribe_samples(audio, super::audio::SAMPLE_RATE, 1, None)?;
        Ok(result.text.trim().to_string())
    }
}
