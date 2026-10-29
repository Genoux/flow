//! macOS opens the microphone at whatever rate and channel count the hardware
//! has, and everything downstream assumes 16kHz mono. `MonoResampler` is that
//! seam, so it is held to what the rest of the pipeline relies on: the length
//! is right, how the callback happened to be chunked changes nothing, and the
//! voice band survives.

use flow::audio::{MonoResampler, SAMPLE_RATE};

const TONE_HZ: f32 = 440.0;
const AMPLITUDE: f32 = 0.5;

/// Interleaved, with the same tone on every channel.
fn tone(rate: u32, channels: usize, seconds: f32) -> Vec<f32> {
    let frames = (rate as f32 * seconds) as usize;
    (0..frames)
        .flat_map(|frame| {
            let sample = AMPLITUDE
                * (2.0 * std::f32::consts::PI * TONE_HZ * frame as f32 / rate as f32).sin();
            std::iter::repeat_n(sample, channels)
        })
        .collect()
}

fn resample(rate: u32, channels: u16, input: &[f32], sizes: &[usize]) -> Vec<f32> {
    let mut resampler = MonoResampler::new(rate, channels).expect("resampler");
    let mut output = Vec::new();
    let mut at = 0;
    for size in sizes.iter().cycle() {
        if at >= input.len() {
            break;
        }
        let end = (at + size * channels as usize).min(input.len());
        resampler.push(&input[at..end], |chunk| output.extend_from_slice(chunk));
        at = end;
    }
    output
}

fn rms(samples: &[f32]) -> f32 {
    (samples.iter().map(|sample| sample * sample).sum::<f32>() / samples.len() as f32).sqrt()
}

fn crossings(samples: &[f32]) -> usize {
    samples
        .windows(2)
        .filter(|pair| (pair[0] < 0.0) != (pair[1] < 0.0))
        .count()
}

/// Skips the start, where the resampler is still filling its own delay.
const SETTLE: usize = 2048;

fn check_length(rate: u32, channels: u16) {
    let input = tone(rate, channels as usize, 3.0);
    let frames = input.len() / channels as usize;
    let output = resample(rate, channels, &input, &[7, 512, 1000]);
    let expected = frames * SAMPLE_RATE as usize / rate as usize;
    assert!(output.len() <= expected, "{} > {expected}", output.len());
    assert!(
        output.len() + 400 >= expected,
        "{} is more than a chunk short of {expected}",
        output.len()
    );
}

fn check_tone(rate: u32, channels: u16) {
    let input = tone(rate, channels as usize, 3.0);
    let output = resample(rate, channels, &input, &[7, 512, 1000]);
    let settled = &output[SETTLE..];

    let expected_rms = AMPLITUDE / 2f32.sqrt();
    let measured = rms(settled);
    assert!(
        (measured - expected_rms).abs() < expected_rms * 0.03,
        "rms {measured}, wanted {expected_rms}"
    );

    let seconds = settled.len() as f32 / SAMPLE_RATE as f32;
    let rate_of_crossings = crossings(settled) as f32 / seconds;
    assert!(
        (rate_of_crossings - 2.0 * TONE_HZ).abs() < 2.0 * TONE_HZ * 0.02,
        "{rate_of_crossings} crossings/s, wanted {}",
        2.0 * TONE_HZ
    );
}

fn check_chunking_is_irrelevant(rate: u32, channels: u16) {
    let input = tone(rate, channels as usize, 2.0);
    let frames = input.len() / channels as usize;
    let whole = resample(rate, channels, &input, &[frames]);
    for sizes in [&[7usize][..], &[512], &[1000], &[7, 512, 1000], &[1]] {
        assert_eq!(whole, resample(rate, channels, &input, sizes), "{sizes:?}");
    }
}

#[test]
fn stereo_48k_comes_out_a_third_as_long() {
    check_length(48_000, 2);
}

#[test]
fn stereo_48k_keeps_the_tone() {
    check_tone(48_000, 2);
}

#[test]
fn stereo_48k_does_not_care_how_it_was_chunked() {
    check_chunking_is_irrelevant(48_000, 2);
}

#[test]
fn mono_44k_comes_out_at_the_model_rate() {
    check_length(44_100, 1);
}

#[test]
fn mono_44k_keeps_the_tone() {
    check_tone(44_100, 1);
}

#[test]
fn mono_44k_does_not_care_how_it_was_chunked() {
    check_chunking_is_irrelevant(44_100, 1);
}

#[test]
fn channels_are_averaged_not_summed() {
    let rate = 48_000;
    let mut stereo = tone(rate, 2, 1.0);
    for frame in stereo.chunks_exact_mut(2) {
        frame[1] = 0.0;
    }
    let output = resample(rate, 2, &stereo, &[1000]);
    let expected = AMPLITUDE / 2.0 / 2f32.sqrt();
    let measured = rms(&output[SETTLE..]);
    assert!(
        (measured - expected).abs() < expected * 0.03,
        "rms {measured}, wanted {expected}"
    );
}

#[test]
fn nothing_arrives_until_a_chunk_is_full() {
    let mut resampler = MonoResampler::new(48_000, 2).expect("resampler");
    let mut emitted = 0;
    resampler.push(&[0.1; 20], |chunk| emitted += chunk.len());
    assert_eq!(emitted, 0);
}
