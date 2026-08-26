use anyhow::{Result, anyhow};
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

/// Always the cpal default device, whatever a pinned microphone says.
///
/// Flow used to hunt for a device named "pipewire" to get free rate conversion,
/// but on a PipeWire machine `default` already IS PipeWire and converts
/// identically (measured: both accept 16kHz mono f32 from 48kHz stereo
/// hardware). That conversion is the reason this must not become a device
/// lookup again: [`Capture::attach`] hardcodes 16kHz mono f32, which only works
/// because it is talking to PipeWire rather than to the hardware, and opening a
/// raw `hw:` PCM through cpal fails outright.
///
/// Choosing a microphone therefore happens one layer up, by moving the stream
/// this opens onto another source - see [`Capture::set_source`].
pub fn open_device() -> Result<Device> {
    cpal::default_host()
        .default_input_device()
        .ok_or_else(|| anyhow!("no input device available"))
}

/// PipeWire suspends idle sources. A phone mic then wakes with a burst of
/// zeros, which is what "hold and nothing" looked like. Unsuspend before we
/// open a stream so the first callback is real audio.
pub fn wake_default_source() {
    wake_source("@DEFAULT_SOURCE@");
}

/// Unsuspend one source by name. Separate from [`wake_default_source`] because
/// a pinned microphone is by definition not the default one, and waking
/// `@DEFAULT_SOURCE@` while recording from another mic wakes the wrong device -
/// leaving the one actually being recorded suspended, which is the burst of
/// zeros above.
pub fn wake_source(name: &str) {
    let _ = Command::new("pactl")
        .args(["suspend-source", name, "0"])
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

/// The name of the running executable, which is what the PipeWire ALSA plugin
/// names our capture stream after.
///
/// Read at runtime rather than hardcoded to "flow". The plugin publishes
/// `alsa_capture.<binary>`, so anything running under another name - a test
/// binary, a dev build, a renamed release - would look for a stream that does
/// not exist and silently go on recording from the default microphone. That is
/// not hypothetical: it is what tests/routing.rs did before this was derived.
fn binary_name() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "flow".to_string())
}

fn unquote(value: &str) -> Option<&str> {
    value.strip_prefix('"')?.strip_suffix('"')
}

/// Whether this property line names the capture stream belonging to `binary`.
///
/// Two spellings because the ALSA plugin writes both, and matching either means
/// this survives a PipeWire that stops publishing one of them. Matching on the
/// pid instead would be exact and is not available: the plugin does not publish
/// `application.process.id` for its own streams, though pipewire-pulse clients
/// beside us in the same listing do.
fn names_stream(line: &str, binary: &str) -> bool {
    match line.split_once(" = ") {
        Some(("node.name", value)) => {
            unquote(value).and_then(|name| name.strip_prefix("alsa_capture.")) == Some(binary)
        }
        Some(("application.name", value)) => {
            unquote(value)
                .and_then(|name| name.strip_prefix("PipeWire ALSA ["))
                .and_then(|name| name.strip_suffix(']'))
                == Some(binary)
        }
        _ => false,
    }
}

/// The id of our own capture stream in `pactl list source-outputs`.
///
/// Split from the command that produces the listing so the parsing is testable
/// with no PipeWire running.
fn our_source_output(listing: &str, binary: &str) -> Option<String> {
    let mut id = None;
    for line in listing.lines() {
        let line = line.trim();
        if let Some(value) = line.strip_prefix("Source Output #") {
            id = Some(value.trim().to_owned());
        } else if names_stream(line, binary) {
            return id;
        }
    }
    None
}

/// Point Flow's live capture at `source`, leaving every other stream and the
/// system default exactly as they were.
///
/// This is the whole per-app routing mechanism. Rebuilding the cpal stream
/// against a chosen device was the obvious alternative and does not work - see
/// [`open_device`] - whereas PipeWire will happily move a running stream
/// between sources and remembers the choice for itself.
fn move_stream_to(source: &str) -> Result<()> {
    let binary = binary_name();
    let listing = Command::new("pactl")
        .args(["list", "source-outputs"])
        .output()?;
    let id = our_source_output(&String::from_utf8_lossy(&listing.stdout), &binary)
        .ok_or_else(|| anyhow!("no capture stream named after {binary} in the graph"))?;

    let moved = Command::new("pactl")
        .args(["move-source-output", &id, source])
        .output()?;
    if !moved.status.success() {
        return Err(anyhow!(
            "pactl move-source-output: {}",
            String::from_utf8_lossy(&moved.stderr).trim()
        ));
    }
    Ok(())
}

/// A read-only view of what the microphone is hearing right now, for whoever is
/// drawing it. Reads the pre-roll ring, which is already kept for every chunk
/// whether or not a recording is live, so the audio callback gains no work.
#[derive(Clone)]
pub struct Monitor {
    recent: Arc<Mutex<VecDeque<f32>>>,
    heard: Arc<AtomicU64>,
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

    /// Samples delivered since the stream opened. The island uses this to ignore
    /// the pre-roll: the ring is always full, so the newest window on spawn is
    /// whatever the room was doing before the key went down.
    pub fn heard(&self) -> u64 {
        self.heard.load(Ordering::Relaxed)
    }
}

/// How far the loud parts of a recording must exceed its quiet parts before it
/// counts as containing a voice.
///
/// Measured: real speech swings 30x between its vowels and the gaps between its
/// words, and this room swings 2.1 to 2.8x, because a room has nothing to swing
/// between. Level cannot make this call at all - across 169 labelled recordings
/// junk spans peak 0.000-1.276 and rms 0.0000-0.0291 while real dictations span
/// peak 0.129-1.125 and rms 0.0114-0.0634, overlapping on both. Holding the key
/// without speaking produced "Oh" and "Yeah." from room tone, and no threshold on
/// how loud it was could have told those from a real "Yeah."
///
/// Set low in the gap rather than halfway: passing a stray word through costs a
/// keystroke to delete, and rejecting a real one costs the words themselves.
///
/// Lowered from 5.0 after a real, calm, continuously-spoken dictation (no
/// pauses between words, explaining something in one steady breath) measured
/// 3.6x and was silently thrown away - "30x between vowels and gaps" assumed
/// speech with pauses in it, and not everyone talks that way. 3.2 sits between
/// that real recording and the documented room-tone ceiling of 2.8x, same
/// margin-in-the-gap reasoning as the original number.
const SPEECH_SWING: f32 = 3.2;

/// Windows shorter than this cannot be judged - there is nothing to compare.
const SWING_MIN_WINDOWS: usize = 8;

/// Ratio of the loud parts of a recording to its quiet parts, measured against its
/// own baseline so it needs no fixed level and works on any microphone.
pub fn swing(samples: &[f32]) -> f32 {
    let mut levels: Vec<f32> = samples.chunks(GAP_WINDOW).map(rms).collect();
    if levels.len() < SWING_MIN_WINDOWS {
        return f32::INFINITY;
    }
    levels.sort_by(f32::total_cmp);
    let at = |quantile: f32| levels[((levels.len() - 1) as f32 * quantile) as usize];
    at(0.95) / at(0.20).max(1e-6)
}

/// Was anybody talking?
pub fn sounds_like_speech(samples: &[f32]) -> bool {
    swing(samples) >= SPEECH_SWING
}

/// Width of the window silence is measured over, 50ms: long enough to average out
/// a single quiet sample, short enough to sit inside a real pause between words.
const GAP_WINDOW: usize = SAMPLE_RATE as usize / 20;

/// Audio worth transcribing on its own after a split. Below this the tail costs
/// more in encoder overhead than the split saves.
const MIN_TAIL: usize = SAMPLE_RATE as usize / 2;

/// Shortest run of quiet that is a pause between words rather than a silence
/// inside one. A stop consonant's closure - the gap in the middle of "stop" or
/// "back" - runs 50 to 120ms, so a single quiet window is no evidence of a word
/// boundary at all, and cutting on one would slice a word in half.
const MIN_GAP: usize = SAMPLE_RATE as usize / 4;

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
    if samples.len() < at_least + MIN_GAP + MIN_TAIL {
        return None;
    }

    let quiet: Vec<bool> = samples[at_least..]
        .chunks(GAP_WINDOW)
        .map(|window| rms(window) < SILENCE_RMS)
        .collect();

    // The longest sustained run of quiet, not the quietest single window: only a
    // run long enough to outlast a consonant says anything about where a word
    // ends. Cut through its middle, furthest from the words on either side.
    let mut longest: Option<(usize, usize)> = None;
    let mut run = 0;
    for (index, quiet) in quiet.iter().enumerate() {
        run = if *quiet { run + 1 } else { 0 };
        if run >= MIN_GAP / GAP_WINDOW && longest.is_none_or(|(_, best)| run > best) {
            longest = Some((index + 1 - run, run));
        }
    }

    let (start, length) = longest?;
    let at = at_least + (start + length / 2) * GAP_WINDOW;
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
    heard: Arc<AtomicU64>,
    /// The source the config asks for, and the source actually applied to the
    /// stream as it stands. Two fields rather than one because a rebuilt stream
    /// is a new source-output id, so what was wanted outlives what was applied.
    wanted_source: Mutex<Option<String>>,
    routed_to: Mutex<Option<String>>,
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
            heard: Arc::new(AtomicU64::new(0)),
            wanted_source: Mutex::new(None),
            routed_to: Mutex::new(None),
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
        let heard = self.heard.clone();

        let config = StreamConfig {
            channels: 1,
            sample_rate: SAMPLE_RATE,
            buffer_size: cpal::BufferSize::Default,
        };

        let stream = device.build_input_stream(
            config,
            move |data: &[f32], _: &_| {
                last_write.store(now_millis(), Ordering::Relaxed);
                heard.fetch_add(data.len() as u64, Ordering::Relaxed);
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

        // A new stream is a new source-output, so whatever was routed applies
        // to something that no longer exists. `begin_inner` re-applies from
        // `wanted_source` on the next press.
        *self.routed_to.lock().unwrap() = None;
        Ok(())
    }

    /// Record from `wanted`, or from the system default when it is `None`.
    ///
    /// Called per dictation from the daemon, which is how a microphone picked
    /// in the console lands on the next press rather than the next restart -
    /// the same read-at-the-point-of-use rule as ducking and the chord.
    pub fn set_source(&self, wanted: Option<&str>) {
        *self.wanted_source.lock().unwrap() = wanted.map(str::to_owned);
        self.apply_source();
    }

    /// Move the stream onto the wanted source unless it is already there.
    ///
    /// Never fails a dictation. A microphone that has been unplugged leaves
    /// `routed_to` unset so the next press tries again, and PipeWire keeps
    /// feeding us the default in the meantime - degrading to the wrong mic is
    /// recoverable, and degrading to silence is not.
    fn apply_source(&self) {
        let wanted = self.wanted_source.lock().unwrap().clone();
        let mut routed = self.routed_to.lock().unwrap();

        // Never pinned and still not pinned, which is nearly every machine:
        // PipeWire follows the default source on its own, so there is nothing
        // to do and - more to the point - nothing to ask it, keeping the
        // common path free of a shell-out entirely.
        if wanted.is_none() && routed.is_none() {
            return;
        }

        // Going back to Automatic has to name the default explicitly. The
        // stream is sitting on a mic somebody pinned and will stay there.
        let Some(target) = wanted.or_else(default_source_name) else {
            return;
        };
        if routed.as_deref() == Some(target.as_str()) {
            return;
        }

        match move_stream_to(&target) {
            Ok(()) => {
                wake_source(&target);
                eprintln!("mic source: {target}");
                *routed = Some(target);
            }
            Err(err) => {
                *routed = None;
                eprintln!("could not record from {target}, using the default instead: {err:#}");
            }
        }
    }

    /// The source the stream was last successfully moved onto, or `None` while
    /// it is simply following the system default.
    pub fn current_source(&self) -> Option<String> {
        self.routed_to.lock().unwrap().clone()
    }

    /// Unsuspend whichever source we are actually on, which is not always the
    /// default one. See [`wake_source`].
    fn wake_current(&self) {
        match self.routed_to.lock().unwrap().as_deref() {
            Some(name) => wake_source(name),
            None => wake_default_source(),
        }
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
                // Every dictation from here on returns the stale pre-roll and
                // is skipped as silence, so without this the tool looks like it
                // stopped responding rather than like the mic went away.
                crate::notify::failure(
                    "Flow lost the microphone",
                    "Check your sound settings, then restart Flow.",
                );
                eprintln!("reopen failed: {err}");
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
            heard: self.heard.clone(),
        }
    }

    /// Wait until the source is delivering energy, not just empty buffers.
    /// A suspended phone mic callbacks with zeros; non-empty is not enough.
    pub fn warmup(&self, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            self.wake_current();
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
        self.begin_inner(true);
    }

    /// Start recording from this instant, discarding the pre-roll ring.
    ///
    /// For the ducked path: the ring holds the 200ms *before* the key went
    /// down, which is by definition before other apps were turned down, so it
    /// is the one slice of a ducked recording guaranteed to carry the video
    /// the user was trying to mute.
    pub fn begin_without_pre_roll(&self) {
        self.begin_inner(false);
    }

    fn begin_inner(&self, keep_pre_roll: bool) {
        // Before the pre-roll is copied: a rebuilt stream starts with an empty
        // ring, and taking the stale one would prepend audio from another device.
        self.ensure_live();
        // After it, not before: a reopen builds a new source-output, so the
        // routing has to be re-applied to the stream that now exists.
        self.apply_source();
        self.wake_current();
        let pre = self.pre_roll.lock().unwrap();
        let mut samples = self.samples.lock().unwrap();
        samples.clear();
        if keep_pre_roll {
            samples.extend(pre.iter().copied());
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Two streams, ours second, because the bug this guards is returning the
    /// id of whichever block happened to come first and moving another app's
    /// microphone instead of our own.
    const OUTPUTS: &str = "\
Source Output #6733079
\tDriver: PipeWire
\tSource: 6733069
\tProperties:
\t\tapplication.name = \"sunshine\"
\t\tnode.name = \"sunshine\"
Source Output #6192836
\tDriver: PipeWire
\tSource: 6133567
\tProperties:
\t\tapplication.name = \"PipeWire ALSA [flow]\"
\t\tnode.name = \"alsa_capture.flow\"
";

    #[test]
    fn our_own_stream_is_the_one_that_moves() {
        assert_eq!(
            our_source_output(OUTPUTS, "flow").as_deref(),
            Some("6192836")
        );
    }

    /// No stream of ours in the graph is the normal state before the capture
    /// opens. It must read as "nothing to move", never as somebody else's id.
    #[test]
    fn another_app_is_never_mistaken_for_ours() {
        let others = OUTPUTS.split("Source Output #6192836").next().unwrap();
        assert_eq!(our_source_output(others, "flow"), None);
        assert_eq!(our_source_output("", "flow"), None);
    }

    /// The name is the binary's, not the string "flow". Looking for the wrong
    /// one finds nothing and silently leaves the capture on the default
    /// microphone, which is how this was caught: the test binary is not called
    /// flow, so the first version of this feature did nothing under test while
    /// appearing to work.
    #[test]
    fn the_stream_is_found_under_whatever_the_binary_is_called() {
        let renamed = OUTPUTS.replace("flow", "flow-dev");
        assert_eq!(
            our_source_output(&renamed, "flow-dev").as_deref(),
            Some("6192836")
        );
        assert_eq!(our_source_output(&renamed, "flow"), None);
    }

    /// The ALSA plugin has published either spelling depending on the PipeWire
    /// version, and finding neither would silently disable the whole feature.
    #[test]
    fn either_spelling_identifies_the_stream() {
        for line in [
            r#"node.name = "alsa_capture.flow""#,
            r#"application.name = "PipeWire ALSA [flow]""#,
        ] {
            assert!(names_stream(line, "flow"), "{line} was not recognised");
        }
    }

    /// A prefix match would claim `flow-dev`'s stream for `flow`, and move a
    /// microphone out from under another running copy.
    #[test]
    fn a_similar_name_is_not_our_stream() {
        for line in [
            r#"node.name = "alsa_capture.flow-dev""#,
            r#"node.name = "alsa_capture.flowers""#,
            r#"node.name = "sunshine""#,
            r#"application.name = "PipeWire ALSA [flow-dev]""#,
            r#"application.name = "flow""#,
        ] {
            assert!(!names_stream(line, "flow"), "{line} was claimed as ours");
        }
    }

    /// Whatever it resolves to, it has to be a name the listing could actually
    /// contain - an empty one would match the wrong stream or none at all.
    #[test]
    fn the_binary_always_has_a_name() {
        let name = binary_name();
        assert!(!name.is_empty());
        assert!(!name.contains('/'), "{name} is a path, not a name");
    }
}
