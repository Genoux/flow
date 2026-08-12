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

/// An open capture stream. Held for the duration of a push-to-talk press and
/// consumed by [`Recorder::stop`].
pub struct Recorder {
    stream: cpal::Stream,
    samples: Arc<Mutex<Vec<f32>>>,
}

impl Recorder {
    pub fn start(device: &Device) -> Result<Self> {
        let config = StreamConfig {
            channels: 1,
            sample_rate: SAMPLE_RATE,
            buffer_size: cpal::BufferSize::Default,
        };

        let format = device.default_input_config()?.sample_format();
        if format != SampleFormat::F32 {
            // ponytail: PipeWire converts formats too, so we always ask for f32
            // rather than implementing per-format conversion.
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

        Ok(Self { stream, samples })
    }

    pub fn stop(self) -> Vec<f32> {
        drop(self.stream);
        std::mem::take(&mut *self.samples.lock().unwrap())
    }
}

/// Capture a fixed `duration` of 16 kHz mono f32 audio.
pub fn record(device: &Device, duration: Duration) -> Result<Vec<f32>> {
    let recorder = Recorder::start(device)?;
    std::thread::sleep(duration);
    let captured = recorder.stop();

    if captured.is_empty() {
        return Err(anyhow!("captured no audio - check the input source"));
    }
    Ok(captured)
}

/// Peak amplitude, used to tell "silence" from "wrong source" when debugging.
pub fn peak(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |acc, s| acc.max(s.abs()))
}

/// Root-mean-square level. Preferred over peak for deciding whether anything
/// was said: peak reacts to a single transient (a key click, a door), while RMS
/// reflects sustained energy. Measured room tone peaked at 0.140 yet still
/// transcribed to nonsense, so peak alone cannot gate.
pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let sum: f64 = samples.iter().map(|s| (*s as f64) * (*s as f64)).sum();
    (sum / samples.len() as f64).sqrt() as f32
}

/// Catches a dead capture only - a muted, disconnected or wrong-source mic.
///
/// This is deliberately NOT a noise gate, and no amplitude threshold can be
/// one here: measured room tone on this machine is rms 0.046, while the
/// quietest one-second window of real speech (tests/fixtures/jfk.wav) is
/// 0.0155. Noise sits three times above quiet speech, so any threshold that
/// passes a quiet talker also passes silence-transcribed-as-nonsense.
/// Separating those needs Silero VAD, not a level check.
pub const SILENCE_RMS: f32 = 0.005;
