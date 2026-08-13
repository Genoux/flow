use anyhow::{anyhow, Result};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Device, SampleFormat, StreamConfig};
use std::collections::VecDeque;
use std::process::Command;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

pub const SAMPLE_RATE: u32 = 16_000;

/// Audio from just before the bind. A phone-over-network source (WO Mic) is
/// already late; without this the first word is gone by the time the hold starts.
const PRE_ROLL: usize = SAMPLE_RATE as usize / 5;

/// Whatever the system calls its default input, and nothing else. Flow used to
/// hunt for a device named "pipewire" to get free rate conversion, but on a
/// PipeWire machine `default` already IS PipeWire and converts identically
/// (measured: both accept 16kHz mono f32 from 48kHz stereo hardware). Preferring
/// one by name only meant ignoring the source the user had actually selected.
pub fn open_device() -> Result<Device> {
    cpal::default_host()
        .default_input_device()
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

pub fn default_source_name() -> Option<String> {
    let output = Command::new("pactl")
        .args(["get-default-source"])
        .output()
        .ok()?;
    let name = String::from_utf8(output.stdout).ok()?;
    let name = name.trim();
    (!name.is_empty()).then(|| name.to_string())
}

/// A read-only view of what the microphone is hearing right now, for whoever is
/// drawing it. Reads the pre-roll ring, which is already kept for every chunk
/// whether or not a recording is live, so the audio callback gains no work.
#[derive(Clone)]
pub struct Monitor {
    recent: Arc<Mutex<VecDeque<f32>>>,
}

impl Monitor {
    /// Fill `window` with the newest samples, oldest first. Shorter than
    /// requested until the ring has filled, so callers must handle a partial
    /// window rather than assume a full one.
    pub fn window(&self, window: &mut Vec<f32>, samples: usize) {
        window.clear();
        let recent = self.recent.lock().unwrap();
        let from = recent.len().saturating_sub(samples);
        window.extend(recent.iter().skip(from).copied());
    }
}

/// Width of the window silence is measured over, 50ms: long enough to average out
/// a single quiet sample, short enough to sit inside a real pause between words.
const GAP_WINDOW: usize = SAMPLE_RATE as usize / 20;

/// Audio worth transcribing on its own after a split. Below this the tail costs
/// more in encoder overhead than the split saves.
const MIN_TAIL: usize = SAMPLE_RATE as usize / 2;

/// Where a recording can be cut in two while it is still being spoken, so the
/// first half can be transcribed before the speaker has finished.
///
/// Only inside a genuine silence. Transcription is ~23x realtime on the CPU, so
/// the start of a long dictation can be done by the time the key comes up, and
/// that is 45% of the measured wait on anything over ten seconds. But the saving
/// is worth nothing if the cut costs a word, and the pieces only agree with the
/// whole when the cut lands in a pause - see tests/chunking.rs.
///
/// `None` means leave it alone: no pause yet, nothing worth splitting, or a
/// speaker who has not drawn breath. Unbroken speech pays the full transcription
/// at the end, which is the same behaviour as before this existed.
pub fn split_at_silence(samples: &[f32], at_least: usize) -> Option<usize> {
    if samples.len() < at_least + GAP_WINDOW + MIN_TAIL {
        return None;
    }

    // The quietest window, not the first quiet one: a pause has a middle, and the
    // middle is furthest from the words on either side of it.
    let (index, level) = samples[at_least..]
        .chunks(GAP_WINDOW)
        .enumerate()
        .map(|(index, window)| (index, rms(window)))
        .min_by(|a, b| a.1.partial_cmp(&b.1).expect("audio is never NaN"))?;

    if level >= SILENCE_RMS {
        return None;
    }

    let at = at_least + index * GAP_WINDOW + GAP_WINDOW / 2;
    (samples.len() - at >= MIN_TAIL).then_some(at)
}

/// Process clock. `Instant` cannot live in an atomic, so callback freshness is
/// tracked as milliseconds since the first call.
fn now_millis() -> u64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// How long without a single callback before the stream is presumed dead. Well
/// clear of the few-millisecond callback interval, and of the gap PipeWire
/// introduces when it suspends an idle source, so a healthy capture is never
/// torn down and rebuilt for nothing.
const STREAM_TIMEOUT: Duration = Duration::from_secs(3);

/// Always-on capture so a network mic stays RUNNING between holds. Opening a
/// new stream per press is what let PipeWire suspend PhoneMic and feed silence.
pub struct Capture {
    /// Swappable, because the stream has to be rebuilt when its source vanishes.
    /// The buffers below are deliberately not: they are `Arc`s shared with the
    /// callback, so a replacement stream keeps feeding the same monitor and the
    /// same in-flight recording.
    stream: Mutex<Option<cpal::Stream>>,
    live: Arc<AtomicBool>,
    samples: Arc<Mutex<Vec<f32>>>,
    pre_roll: Arc<Mutex<VecDeque<f32>>>,
    last_write: Arc<AtomicU64>,
}

impl Capture {
    pub fn open(device: &Device) -> Result<Self> {
        wake_default_source();

        let format = device.default_input_config()?.sample_format();
        if format != SampleFormat::F32 {
            eprintln!("note: device native format is {format:?}, requesting f32 anyway");
        }

        let capture = Self {
            stream: Mutex::new(None),
            live: Arc::new(AtomicBool::new(false)),
            samples: Arc::new(Mutex::new(Vec::<f32>::new())),
            pre_roll: Arc::new(Mutex::new(VecDeque::with_capacity(PRE_ROLL))),
            last_write: Arc::new(AtomicU64::new(now_millis())),
        };
        capture.attach(device)?;
        Ok(capture)
    }

    /// Builds a stream on `device` and installs it, replacing any existing one.
    fn attach(&self, device: &Device) -> Result<()> {
        let live = self.live.clone();
        let sink = self.samples.clone();
        let roll = self.pre_roll.clone();
        let last_write = self.last_write.clone();

        let config = StreamConfig {
            channels: 1,
            sample_rate: SAMPLE_RATE,
            buffer_size: cpal::BufferSize::Default,
        };

        let stream = device.build_input_stream(
            config,
            move |data: &[f32], _: &_| {
                last_write.store(now_millis(), Ordering::Relaxed);
                write_input(&live, &sink, &roll, data)
            },
            |err| eprintln!("stream error: {err}"),
            None,
        )?;
        stream.play()?;

        // The ring survives the swap by design - the Monitor holds the same Arc -
        // but its contents came from the device that just went away, and `begin`
        // prepends them to the next recording. Emptying it here is what makes the
        // rebuilt stream actually start clean.
        self.pre_roll.lock().unwrap().clear();

        // Dropped only once the replacement is playing, so a failed rebuild
        // leaves the old stream in place rather than nothing at all.
        *self.stream.lock().unwrap() = Some(stream);
        self.last_write.store(now_millis(), Ordering::Relaxed);
        Ok(())
    }

    /// Time since the last callback delivered anything.
    pub fn silent_for(&self) -> Duration {
        Duration::from_millis(now_millis().saturating_sub(self.last_write.load(Ordering::Relaxed)))
    }

    /// Reopen the capture if callbacks have stopped arriving.
    ///
    /// When the source a stream is attached to disappears - a USB mic unplugged,
    /// a Bluetooth headset dropping - PipeWire tears the stream down and cpal
    /// reports no error at all, so the callback simply never fires again. The
    /// symptom is indistinguishable from a quiet room: every recording returns
    /// the stale pre-roll and gets skipped as silence, forever.
    ///
    /// A *change* of default source needs nothing from us: PipeWire moves a live
    /// stream to the new source by itself (verified by watching the graph relink
    /// mid-recording). This only handles the stream being destroyed.
    pub fn ensure_live(&self) -> bool {
        if self.silent_for() < STREAM_TIMEOUT {
            return true;
        }
        eprintln!(
            "capture stopped delivering {:?} ago - reopening",
            self.silent_for()
        );
        match open_device().and_then(|device| self.attach(&device)) {
            Ok(()) => {
                if let Some(name) = default_source_name() {
                    eprintln!("mic source: {name}");
                }
                true
            }
            Err(err) => {
                eprintln!("could not reopen the microphone: {err}");
                false
            }
        }
    }

    /// Drops the stream so callbacks stop with nothing reported - the same shape
    /// as a source disappearing, without unplugging hardware. Only for tests.
    #[doc(hidden)]
    pub fn kill_stream_for_test(&self) {
        *self.stream.lock().unwrap() = None;
    }

    pub fn monitor(&self) -> Monitor {
        Monitor {
            recent: self.pre_roll.clone(),
        }
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
        // Before the pre-roll is copied: a rebuilt stream starts with an empty
        // ring, and taking the stale one would prepend audio from another device.
        self.ensure_live();
        let pre = self.pre_roll.lock().unwrap();
        let mut samples = self.samples.lock().unwrap();
        samples.clear();
        samples.extend(pre.iter().copied());
        self.live.store(true, Ordering::Relaxed);
    }

    /// Hand over the start of a live recording so it can be transcribed while the
    /// rest is still being spoken, leaving the remainder to be finished on
    /// release. `None` whenever there is no safe place to cut - see
    /// [`split_at_silence`] - so a speaker who never pauses simply gets the old
    /// behaviour.
    pub fn take_prefix(&self, at_least: usize) -> Option<Vec<f32>> {
        if !self.live.load(Ordering::Relaxed) {
            return None;
        }
        let mut samples = self.samples.lock().unwrap();
        let at = split_at_silence(&samples, at_least)?;
        Some(samples.drain(..at).collect())
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
