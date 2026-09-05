use anyhow::{Context, Result, bail};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
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

/// Dump normalised f32 samples as 16 kHz mono i16 WAV. Used for the
/// `record_debug` A/B pair (raw + denoised for the same utterance).
pub fn write_16k_mono(path: impl AsRef<Path>, samples: &[f32]) -> Result<()> {
    let path = path.as_ref();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: super::audio::SAMPLE_RATE,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let file = std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("opening {} for write", path.display()))?;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
    let mut writer = hound::WavWriter::new(std::io::BufWriter::new(file), spec)?;
    for sample in samples {
        let clamped = sample.clamp(-1.0, 1.0);
        writer.write_sample((clamped * i16::MAX as f32) as i16)?;
    }
    writer.finalize().context("finalising wav")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recordings_are_private_and_readable_after_rewrite() {
        let path =
            std::env::temp_dir().join(format!("flow-wav-permissions-{}.wav", std::process::id()));
        for existing_mode in [None, Some(0o644)] {
            if let Some(mode) = existing_mode {
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(mode)).unwrap();
            }
            write_16k_mono(&path, &[0.25, -0.25]).unwrap();
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
            assert_eq!(read_16k_mono(&path).unwrap().len(), 2);
        }
        std::fs::remove_file(path).unwrap();
    }
}
