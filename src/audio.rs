use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, StreamConfig};
use std::collections::VecDeque;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const SAMPLE_RATE: u32 = 16_000;

/// PipeWire's ALSA device accepts any rate/channel count and converts internally,
/// so we ask it for exactly what Parakeet wants and skip resampling entirely.
const PREFERRED_DEVICE: &str = "pipewire";

/// Audio from just before the bind. A phone-over-network source (WO Mic) is
/// already late; without this the first word is gone by the time the hold starts.
const PRE_ROLL: usize = SAMPLE_RATE as usize / 5;

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

/// PipeWire suspends idle sources. A phone mic then wakes with a burst of
/// zeros, which is what "hold and nothing" looked like. Unsuspend before we
/// open a stream so the first callback is real audio.
pub fn wake_default_source() {
    let _ = Command::new("pactl")
        .args(["suspend-source", "@DEFAULT_SOURCE@", "0"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}

/// Prefer a real USB/webcam mic over PhoneMic. PipeWire often leaves a
/// Bluetooth speaker *monitor* as the default, which is playback, not a mic.
pub fn pin_capture_source() {
    let Some(sources) = list_sources() else { return };
    let target = sources
        .iter()
        .find(|name| name.to_ascii_lowercase().contains("webcam"))
        .or_else(|| sources.iter().find(|name| *name == "phone_mic"));
    if let Some(name) = target {
        let _ = Command::new("pactl")
            .args(["set-default-source", name])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        return;
    }
    if let Some(name) = default_source_name()
        && name.contains(".monitor")
    {
        eprintln!("default source is a monitor ({name}), not a microphone");
    }
}

fn list_sources() -> Option<Vec<String>> {
    let output = Command::new("pactl").args(["list", "short", "sources"]).output().ok()?;
    let text = String::from_utf8(output.stdout).ok()?;
    Some(
        text.lines()
            .filter_map(|line| line.split('\t').nth(1).map(str::to_string))
            .collect(),
    )
}

pub fn default_source_name() -> Option<String> {
    let output = Command::new("pactl")
        .args(["get-default-source"])
        .output()
        .ok()?;
    let name = String::from_utf8(output.stdout).ok()?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// Always-on capture so a network mic stays RUNNING between holds. Opening a
/// new stream per press is what let PipeWire suspend PhoneMic and feed silence.
pub struct Capture {
    _stream: cpal::Stream,
    live: Arc<AtomicBool>,
    samples: Arc<Mutex<Vec<f32>>>,
    pre_roll: Arc<Mutex<VecDeque<f32>>>,
}

impl Capture {
    pub fn open(device: &Device) -> Result<Self> {
        wake_default_source();

        let format = device.default_input_config()?.sample_format();
        if format != SampleFormat::F32 {
            eprintln!("note: device native format is {format:?}, requesting f32 anyway");
        }

        let live = Arc::new(AtomicBool::new(false));
        let samples = Arc::new(Mutex::new(Vec::<f32>::new()));
        let pre_roll = Arc::new(Mutex::new(VecDeque::with_capacity(PRE_ROLL)));

        let live_cb = live.clone();
        let sink = samples.clone();
        let roll = pre_roll.clone();

        let config = StreamConfig {
            channels: 1,
            sample_rate: SAMPLE_RATE,
            buffer_size: cpal::BufferSize::Default,
        };

        let stream = device.build_input_stream(
            config,
            move |data: &[f32], _: &_| write_input(&live_cb, &sink, &roll, data),
            |err| eprintln!("stream error: {err}"),
            None,
        )?;
        stream.play()?;

        Ok(Self {
            _stream: stream,
            live,
            samples,
            pre_roll,
        })
    }

    /// Wait until the source is delivering energy, not just empty buffers.
    /// A suspended phone mic callbacks with zeros; non-empty is not enough.
    pub fn warmup(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            wake_default_source();
            let pre = self.pre_roll.lock().unwrap();
            if pre.iter().any(|s| s.abs() > SILENCE_RMS) {
                return true;
            }
            drop(pre);
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    pub fn begin(&self) {
        wake_default_source();
        let pre = self.pre_roll.lock().unwrap();
        let mut samples = self.samples.lock().unwrap();
        samples.clear();
        samples.extend(pre.iter().copied());
        self.live.store(true, Ordering::Relaxed);
    }

    pub fn end(&self) -> Vec<f32> {
        self.live.store(false, Ordering::Relaxed);
        std::mem::take(&mut *self.samples.lock().unwrap())
    }
}

fn write_input(
    live: &AtomicBool,
    sink: &Mutex<Vec<f32>>,
    roll: &Mutex<VecDeque<f32>>,
    data: &[f32],
) {
    let mut pre = roll.lock().unwrap();
    pre.extend(data.iter().copied());
    while pre.len() > PRE_ROLL {
        pre.pop_front();
    }
    if live.load(Ordering::Relaxed) {
        sink.lock().unwrap().extend_from_slice(data);
    }
}

/// An open capture stream. Held for the duration of a push-to-talk press and
/// consumed by [`Recorder::stop`].
pub struct Recorder {
    capture: Capture,
}

impl Recorder {
    pub fn start(device: &Device) -> Result<Self> {
        let capture = Capture::open(device)?;
        capture.begin();
        Ok(Self { capture })
    }

    pub fn stop(self) -> Vec<f32> {
        self.capture.end()
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
/// quietest one-second window of tests/fixtures/jfk.wav is 0.0155. Noise sits
/// three times above quiet speech, so any threshold that passes a quiet talker
/// also passes silence-transcribed-as-nonsense. Separating those needs Silero
/// VAD, not a level check.
pub const SILENCE_RMS: f32 = 0.005;
