use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, StreamConfig};
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub const SAMPLE_RATE: u32 = 16_000;

/// PipeWire's ALSA device accepts any rate/channel count and converts internally,
/// so we ask it for exactly what Parakeet wants and skip resampling entirely.
const PREFERRED_DEVICE: &str = "pipewire";

pub fn open_device() -> Result<Device> {
    let host = cpal::default_host();
    for device in host.input_devices()? {
        if device.id().map(|id| id.to_string()).unwrap_or_default().contains(PREFERRED_DEVICE) {
            return Ok(device);
        }
    }
    host.default_input_device()
        .ok_or_else(|| anyhow!("no input device available"))
}

/// Capture `duration` of 16 kHz mono f32 audio.
pub fn record(device: &Device, duration: Duration) -> Result<Vec<f32>> {
    let config = StreamConfig {
        channels: 1,
        sample_rate: SAMPLE_RATE,
        buffer_size: cpal::BufferSize::Default,
    };

    let format = device.default_input_config()?.sample_format();
    if format != SampleFormat::F32 {
        // ponytail: PipeWire converts formats too, so we always ask for f32 and
        // only log a mismatch rather than implementing per-format conversion.
        eprintln!("note: device native format is {format:?}, requesting f32 anyway");
    }

    let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
    let sink = samples.clone();

    let stream = device.build_input_stream(
        config,
        move |data: &[f32], _: &_| sink.lock().unwrap().extend_from_slice(data),
        |err| eprintln!("stream error: {err}"),
        None,
    )?;

    stream.play()?;
    std::thread::sleep(duration);
    drop(stream);

    let captured = std::mem::take(&mut *samples.lock().unwrap());
    if captured.is_empty() {
        return Err(anyhow!("captured no audio - check the input source"));
    }
    Ok(captured)
}

/// Peak amplitude, used to tell "silence" from "wrong source" when debugging.
pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |acc, s| acc.max(s.abs()))
}
