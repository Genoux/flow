use anyhow::{bail, Result};
use std::path::Path;

/// Read a 16 kHz mono WAV as normalised f32 samples, the format Parakeet expects.
pub fn read_16k_mono(path: impl AsRef<Path>) -> Result<Vec<f32>> {
    let mut reader = hound::WavReader::open(path.as_ref())?;
    let spec = reader.spec();

    if spec.sample_rate != super::audio::SAMPLE_RATE || spec.channels != 1 {
        bail!(
            "expected 16 kHz mono, got {} Hz {}ch",
            spec.sample_rate,
            spec.channels
        );
    }

    Ok(match spec.sample_format {
        hound::SampleFormat::Float => reader.samples::<f32>().filter_map(Result::ok).collect(),
        hound::SampleFormat::Int => reader
            .samples::<i16>()
            .filter_map(Result::ok)
            .map(|s| s as f32 / i16::MAX as f32)
            .collect(),
    })
}
