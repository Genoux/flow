//! RNNoise via [`nnnoiseless`], adapted to Flow's 16 kHz mono pipeline.
//!
//! RNNoise is trained at 48 kHz and processes 480-sample (10 ms) frames, so
//! Flow's 16 kHz audio is upsampled 3× before denoise and downsampled 3× after.
//! Both stages use a polynomial resampler (`rubato`) sized for these fixed
//! ratios; anything simpler (linear interpolation) would alias enough to hurt
//! the very frequency features RNNoise keys off.

use anyhow::{Context, Result};
use nnnoiseless::DenoiseState;
use rubato::{FftFixedIn, Resampler};

/// Flow's capture sample rate.
const CAPTURE_HZ: usize = 16_000;
/// The rate RNNoise was trained on.
const RNNOISE_HZ: usize = 48_000;
/// RNNoise's fixed frame size: 480 samples = 10 ms at 48 kHz.
const RNNOISE_FRAME: usize = DenoiseState::FRAME_SIZE;

/// Denoise `samples` (normalised f32, mono, 16 kHz) and return the result at
/// the same rate. Silently returns the input unchanged if the resampler or
/// denoiser can't be constructed - a bad denoiser must never cost the user
/// their audio.
pub fn denoise_16k_mono(samples: &[f32]) -> Vec<f32> {
    match run(samples) {
        Ok(out) => out,
        Err(err) => {
            eprintln!("denoise failed ({err:#}), passing audio through untouched");
            samples.to_vec()
        }
    }
}

fn run(samples: &[f32]) -> Result<Vec<f32>> {
    if samples.is_empty() {
        return Ok(Vec::new());
    }

    // 16 k -> 48 k. FftFixedIn is the "small fixed input chunk" variant, so we
    // feed it one 160-sample slice at a time and it returns 480 output samples
    // - exactly one RNNoise frame per iteration.
    let mut up = FftFixedIn::<f32>::new(CAPTURE_HZ, RNNOISE_HZ, 160, 1, 1)
        .context("building 16k->48k resampler")?;
    let mut down = FftFixedIn::<f32>::new(RNNOISE_HZ, CAPTURE_HZ, RNNOISE_FRAME, 1, 1)
        .context("building 48k->16k resampler")?;
    let mut denoiser = DenoiseState::new();

    // nnnoiseless expects samples scaled to i16 range, not [-1, 1].
    // See DenoiseState::process_frame docs.
    let mut scaled = vec![0f32; RNNOISE_FRAME];
    let mut out_frame = vec![0f32; RNNOISE_FRAME];
    let mut result = Vec::with_capacity(samples.len());

    let mut cursor = 0;
    while cursor < samples.len() {
        // Zero-pad the final short chunk so every stage sees its fixed size.
        let end = (cursor + 160).min(samples.len());
        let mut chunk = [0f32; 160];
        chunk[..end - cursor].copy_from_slice(&samples[cursor..end]);
        cursor = end;

        let up_out = up
            .process(&[chunk.as_slice()], None)
            .context("upsample to 48k")?;
        let up48 = &up_out[0];

        for (dst, src) in scaled.iter_mut().zip(up48.iter()) {
            *dst = src * i16::MAX as f32;
        }
        denoiser.process_frame(&mut out_frame, &scaled);
        for sample in out_frame.iter_mut() {
            *sample /= i16::MAX as f32;
        }

        let down_out = down
            .process(&[out_frame.as_slice()], None)
            .context("downsample to 16k")?;
        result.extend_from_slice(&down_out[0]);
    }

    // The resampler's internal buffering means the output may be a few samples
    // shorter or longer than the input; trim any trailing padding so the caller
    // gets what it asked for and the wav dumps line up frame-for-frame.
    result.truncate(samples.len());
    Ok(result)
}
