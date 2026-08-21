//! A small island that floats above everything while flow is recording. The
//! bars are a mountain of the live voice: low at the ends, high in the middle.
//! Frequency still colours them, but it is not allowed to raise the ends above
//! the crest - that is how the island used to draw a W. Nothing here runs on a
//! timer except the sweep shown while the transcript is being produced.
//!
//! It is a wlr-layer-shell surface painted into a shared-memory buffer by hand.
//! A toolkit would mean GTK or Qt in a daemon that otherwise has no UI, and the
//! whole drawing here is one rounded rectangle repeated - the island and every
//! bar are the same primitive.

use anyhow::{Context, Result, anyhow};
use std::fs::File;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::fs::FileExt;
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::time::Duration;
use wayland_client::protocol::{
    wl_buffer, wl_compositor, wl_region, wl_registry, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{Connection, Dispatch, QueueHandle, delegate_noop};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use crate::audio::Monitor;

const WIDTH: u32 = 116;
const HEIGHT: u32 = 40;
/// Clear of the usual bottom bar without sitting in the middle of the screen.
const MARGIN_BOTTOM: i32 = 96;

const BAR_COUNT: usize = 7;
const BAR_WIDTH: f32 = 5.0;
const BAR_GAP: f32 = 5.0;
/// Equal to the bar width, so silence rests as a row of dots rather than slivers.
const BAR_MIN: f32 = 5.0;
const BAR_MAX: f32 = 26.0;

/// How fast the bars fall away when the sweep is about to take over, and how flat
/// counts as flat.
///
/// The handover used to be a cut: the bars were wherever the last syllable left
/// them and the sweep started from its own shape, so the island jumped. Letting
/// them settle first makes it read as one motion - the voice stops, the island
/// comes to rest, then the sweep begins from nothing.
const SETTLE: f32 = 0.80;
const SETTLED: f32 = 0.02;

/// How long refining has to be still running before the sweep replaces the bars.
///
/// Only ever started once recognition has found words - see [`Overlay::working`] -
/// because time cannot answer the question the sweep implies. A cough on seven
/// seconds of silence takes 280ms to recognise and comes back empty, so a delay
/// alone showed a spinner for something that was never going to produce text. This
/// only stops the other flash: a dictation short enough that refining returns
/// before a spinner is worth drawing.
const SWEEP_DELAY: Duration = Duration::from_millis(200);

/// The bars animate in place rather than scrolling, so this is a real frame rate
/// and not a sampling interval. 7ms is ~143Hz, matching a 144Hz panel: the easing
/// below is what makes the motion smooth, but it can only be as smooth as the
/// frames it is drawn on, and 60Hz visibly steps on a display this fast.
const FRAME: Duration = Duration::from_millis(7);

/// Muted black over the blur, rather than the blur alone.
///
/// The blur is what makes the island visible, not this: blurred wallpaper already
/// looks nothing like sharp wallpaper, so the shape reads without being painted
/// in. A pale veil was tried and came out a solid light pill with the wallpaper
/// lost behind it - opaque frosted plastic rather than glass. Darkening slightly
/// keeps the bars legible over a bright photo while leaving its colour showing.
///
/// Over a desktop as dark as this one the fill is close to invisible on its own,
/// which is fine - there the rim and the blur draw the shape.
const ISLAND: (f32, f32, f32) = (0.04, 0.045, 0.055);

/// Barely there, and deliberately flat. The island is meant to show the desktop
/// behind it out of focus - whatever is back there, wallpaper or window - so the
/// fill is a veil over the compositor's blur rather than a surface of its own. A
/// gradient here competes with what it is supposed to be revealing.
///
/// Clear of the `ignore_alpha = 0.1` in Hyprland's layer rule, below which it
/// stops blurring behind the surface and the island becomes a hole in the screen.
/// Hyprland also needs the rule itself to frost anything: without
/// `layerrule = blur, flow` this is a faint tint over a sharp desktop.
const ISLAND_ALPHA: f32 = 0.32;

/// A hairline ring around the edge, one pixel wide and one flat alpha.
///
/// This used to be drawn as a filled rounded rectangle with the tint painted over
/// it, on the theory that only the outermost pixel would show. With the tint at
/// full strength that was true. Once the tint dropped to a translucent veil, 80%
/// of the fill underneath showed through - and since that fill ramped from 0.48
/// at the top to 0.16 at the bottom, the island turned white and then less white
/// down its height. It is an actual ring now, so the fill is the only thing
/// filling anything.
const EDGE: (f32, f32, f32) = (1.0, 1.0, 1.0);
const EDGE_ALPHA: f32 = 0.13;
const BAR: (f32, f32, f32) = (1.0, 1.0, 1.0);
const BAR_ALPHA: f32 = 0.92;
/// A dark rim drawn under each bar. The island is glass, so whatever is behind
/// it can be any brightness - over a white window the white bars disappear
/// without this. It is invisible against a dark backdrop, which is the point:
/// it only shows where it is needed.
const RIM: (f32, f32, f32) = (0.0, 0.0, 0.0);
const RIM_ALPHA: f32 = 0.42;
const RIM_WIDTH: f32 = 0.9;
/// Muted while transcribing, so working reads as quieter than listening rather
/// than as another voice.
const BAR_WORKING_ALPHA: f32 = 0.45;

/// Brightness at the head of the spinning arc.
const ARM_FILL_ALPHA: f32 = 0.85;

/// Brightness of the rest of the ring, so the full circle is always faintly
/// visible and only the moving arc stands out against it.
const ARM_TRACK_ALPHA: f32 = 0.10;

/// Radius of the ring, as a fraction of the circle's own radius.
const ARM_RING: f32 = 0.52;

/// Thickness of the ring stroke, in unscaled pixels.
const ARM_STROKE: f32 = 1.8;

/// Seconds for the arc to travel one full turn. Quick: this is a wait, not a
/// feature, and it should read as "any moment now" rather than draw the eye.
const ARM_PERIOD: f32 = 0.6;

/// How much of the circle the lit arc covers, as a fraction of a full turn.
/// Short, so it reads as a moving highlight rather than a second track.
const ARM_ARC: f32 = 0.32;

// ponytail: shelved, not deleted. The arm window is now the settle wait
// (src/duck.rs FADE_OUT) plus change, short enough that the spinner reads as
// a flicker rather than a loading state. Flip back on if arming ever gets
// slow again.
const SHOW_ARM_SPINNER: bool = false;

/// How far the voice glow reaches, as a fraction of the island. Large and
/// faint - it should be felt rather than looked at.
const GLOW_REACH: f32 = 1.45;

/// How far up the glow sits, as a fraction of the island's half-height. It
/// crowns the shape rather than filling it: light coming off the top edge
/// reads as a halo, where light centred behind the bars just looks like the
/// island got brighter.
const GLOW_RISE: f32 = 0.55;

/// Peak brightness of the glow at full voice.
const GLOW_ALPHA: f32 = 0.16;

/// Soft layers the glow is built from. More is smoother and costs more; five
/// is past the point where another one is visible.
const GLOW_LAYERS: usize = 5;

/// How long the island takes to grow from its loading circle into the full
/// pill, with the bars rising as it goes. This is the cue that says speaking
/// will now be heard, so it wants to be quick and definite rather than a
/// gentle fade anyone could miss - but slow enough that the shape change
/// registers as a shape change.
const BLOOM: f32 = 0.05;

/// Below this the island stays flat. Measured on the webcam mic: an idle room
/// captures rms 0.000 to 0.004, so this clears the noise without clipping a
/// voice from across the desk.
///
/// Do not take the 0.046 room tone quoted on [`crate::audio::SILENCE_RMS`] as
/// the floor here - that figure is from the phone mic. Set this from real
/// `peak x rms y` lines in the log, never from another device's numbers.
///
/// This is a gate, not a scale. The decibel curve below is sensitive enough to
/// show a quiet consonant, which makes it sensitive enough to show room tone
/// too; this decides whether there is a voice at all. It reads the broadband
/// level rather than any one band, because that is what separates a room from a
/// word: an idle room measures rms 0.000 to 0.004 here, a spoken word 0.02 up.
///
/// How far above the room a window has to be before it moves the bars. The room
/// itself is tracked rather than assumed - see [`Analyzer::room`].
const NOISE_MARGIN: f32 = 3.0;

/// Where the tracked floor starts, and the lowest it may go. A dead capture
/// reports zero, and a floor of zero would let the first stray sample through.
const NOISE_START: f32 = 0.01;
const NOISE_MIN: f32 = 0.0015;

/// The bar scale, in decibels. A band's amplitude spans some 60dB between a
/// quiet consonant and a loud vowel, and neither a linear nor a square-root
/// scale can show both ends at once: tuned for the loud end, everything quiet
/// collapses onto the floor. That is exactly what pinned the outer bars until
/// the voice was raised.
///
/// Narrower than the range speech actually covers, on purpose: the window is what
/// converts loudness into movement, and a window wide enough to keep every band
/// inside its thresholds leaves the bars sitting still. Chosen by searching this
/// pair and BAND_GAIN together for the most movement that still satisfies
/// tests/spectrum.rs - see tests/calibrate.rs.
const FLOOR_DB: f32 = -72.0;
const CEILING_DB: f32 = -30.0;

/// Height of one bar, 0.0 to 1.0, from one band's amplitude.
pub fn band_fraction(amplitude: f32) -> f32 {
    if amplitude <= 0.0 {
        return 0.0;
    }
    let decibels = 20.0 * amplitude.log10();
    ((decibels - FLOOR_DB) / (CEILING_DB - FLOOR_DB)).clamp(0.0, 1.0)
}

/// Samples per analysis window: 32ms at 16kHz. Long enough to resolve a voice
/// into bands, short enough that the bars track syllables rather than smear
/// them together.
pub const WINDOW: usize = 512;

/// True once the capture has delivered a full analysis window after `since`.
///
/// Capture never stops, so the ring already holds the room when the island
/// maps. Drawing that window makes the bars spawn mid-syllable. Waiting for
/// this many new samples means the first motion is the voice from this hold.
pub fn fresh_window(heard: u64, since: u64) -> bool {
    heard.saturating_sub(since) >= WINDOW as u64
}

/// How many frequency bands the voice is split into. Fewer than there are
/// bars, because the island mirrors them about its centre.
pub const BAND_COUNT: usize = 4;

/// What each bar listens to, in Hz. Band 0 is the whole voice rather than a slice
/// of it, and deliberately overlaps the other three.
///
/// It draws the centre bar, and the centre has to stay up while someone is
/// speaking. A narrow low band cannot do that: the fundamental is simply absent
/// during an unvoiced consonant, so an "s" emptied the middle of the island and
/// left a hole between the tall bars either side. No gain repairs that, because
/// there is no energy there to amplify. The broadband level is the one measure
/// that is present for every sound a voice makes.
///
/// The rest split the range that carries the vowels and the sibilance, so the
/// bars still move with what is being said rather than all together.
const BANDS: [(f32, f32); BAND_COUNT] = [
    (80.0, 6500.0),
    (300.0, 900.0),
    (900.0, 2500.0),
    (2500.0, 6500.0),
];

/// How tall a bar may be relative to the centre. A parabola, not a gain table:
/// the bands already have their own gains, and those are what made the W.
///
/// The floor used to be 0.20, which compounded with a 75% voice mix into 15% of
/// the voice on the ends - they only moved for a raised voice. Half the centre
/// is still a mountain; it is also still a bar.
pub fn mountain(bar: usize) -> f32 {
    let centre = (BAR_COUNT - 1) as f32 / 2.0;
    let t = (bar as f32 - centre).abs() / centre;
    0.50 + 0.50 * (1.0 - t * t)
}

/// What this bar listens to, besides the shared voice. Ends mix presence with
/// sibilance so they move on a vowel and still flick on an "s" - pure sibilance
/// is silent for most of a sentence, which is why they used to sit still.
fn tint(bar: usize, bands: &[f32; BAND_COUNT]) -> f32 {
    match bar {
        0 | 6 => bands[2] * 0.6 + bands[3] * 0.4,
        1 | 5 => bands[2],
        2 | 4 => bands[1],
        _ => bands[0],
    }
}

/// How far a given voice level moves the bars. Separate from the mountain and
/// from the lerp: those are shape and speed. This is how readily a normal
/// speaking voice fills them - set for a few feet from the mic, not a raised
/// voice into it. Slowing the lerp without this made short syllables die
/// before they arrived, which read as the island going deaf.
const SENSE: f32 = 1.75;

/// Height of one bar, 0.0 to 1.0. Enough shared voice that a quiet sentence
/// still moves the ends; enough of each bar's own band that a vowel and an "s"
/// do not draw the same silhouette.
pub fn bar_height(bar: usize, bands: &[f32; BAND_COUNT]) -> f32 {
    let voice = bands[0];
    let local = tint(bar, bands);
    let edge = (bar as f32 - 3.0).abs() / 3.0;
    let mix = 0.28 + 0.18 * edge;
    let level = ((voice * (1.0 - mix) + local * mix) * SENSE).min(1.0);
    (level * mountain(bar)).clamp(0.0, 1.0)
}

/// Per-band gain, so the island crests in the middle and every bar carries part
/// of the picture. Not a loudness correction: measured on real speech the first
/// formant is the loudest band of the four, and left alone it would put the tall
/// bars either side of centre and leave a valley where the crest belongs.
///
/// Calibrated against tests/fixtures/jfk.wav - see tests/spectrum.rs and
/// tests/calibrate.rs. That recording is from 1961 and thin at both extremes, so
/// re-measure against this microphone before trusting bands 0 and 3.
const BAND_GAIN: [f32; BAND_COUNT] = [0.7, 0.25, 0.7, 4.5];

/// Sample rate the band edges are expressed against.
const SAMPLE_RATE: f32 = 16_000.0;

/// Splits a window of audio into the five band heights the bars draw. Holds the
/// FFT plan and its buffers, because planning is the expensive part and this
/// runs on every frame.
pub struct Analyzer {
    fft: std::sync::Arc<dyn realfft::RealToComplex<f32>>,
    input: Vec<f32>,
    output: Vec<realfft::num_complex::Complex<f32>>,
    /// The room, as loud as it has been recently. Falls quickly towards a quiet
    /// window and leaks upwards otherwise, so a voice can never raise it - only
    /// the silences between words set it, and a persistently louder room lifts it
    /// once those silences stop arriving.
    ///
    /// A fixed floor was the old approach and it could not work: measured here, a
    /// quiet room reads rms 0.008 at the median against a constant of 0.005, so
    /// almost every silent window reached the bars. The right number differs per
    /// microphone, and this one changes microphones.
    noise: f32,
    /// Precomputed Hann window. Without it a syllable's hard edges leak across
    /// every band and all five bars rise together on transients.
    taper: Vec<f32>,
}

impl Analyzer {
    pub fn new() -> Self {
        let fft = realfft::RealFftPlanner::<f32>::new().plan_fft_forward(WINDOW);
        let taper = (0..WINDOW)
            .map(|index| {
                let phase = std::f32::consts::TAU * index as f32 / WINDOW as f32;
                0.5 - 0.5 * phase.cos()
            })
            .collect();
        Self {
            input: fft.make_input_vec(),
            output: fft.make_output_vec(),
            fft,
            taper,
            noise: NOISE_START,
        }
    }

    /// The tracked level of the room, for tests and for anyone wondering why the
    /// bars are resting.
    pub fn room(&self) -> f32 {
        self.noise
    }

    /// Bar heights, 0.0 to 1.0, low band first. All zero until the window has
    /// filled, or while the window is only as loud as the room.
    pub fn bands(&mut self, window: &[f32]) -> [f32; BAND_COUNT] {
        if window.len() < WINDOW {
            return [0.0; BAND_COUNT];
        }
        let level = crate::audio::rms(&window[window.len() - WINDOW..]);

        // Quick down, slow up. Tracking the minimum is what keeps a voice out of
        // the estimate; the leak is what lets a louder room eventually raise it.
        self.noise = if level < self.noise {
            (self.noise * 0.7 + level * 0.3).max(NOISE_MIN)
        } else {
            (self.noise * 1.0004).min(level)
        };

        if level < self.noise * NOISE_MARGIN {
            return [0.0; BAND_COUNT];
        }
        let mut bands = self.amplitudes(window);
        for (band, height) in bands.iter_mut().enumerate() {
            *height = band_fraction(*height * BAND_GAIN[band]);
        }
        bands
    }

    /// Raw RMS amplitude within each band, before any gain or curve. Separate
    /// from [`Analyzer::bands`] so the gains can be calibrated against measured
    /// numbers rather than guessed.
    pub fn amplitudes(&mut self, window: &[f32]) -> [f32; BAND_COUNT] {
        let mut bands = [0.0; BAND_COUNT];
        if window.len() < WINDOW {
            return bands;
        }

        let samples = &window[window.len() - WINDOW..];
        for ((slot, sample), taper) in self.input.iter_mut().zip(samples).zip(&self.taper) {
            *slot = *sample * taper;
        }
        if self.fft.process(&mut self.input, &mut self.output).is_err() {
            return bands;
        }

        let per_bin = SAMPLE_RATE / WINDOW as f32;
        for (band, (low, high)) in BANDS.iter().enumerate() {
            let first = (low / per_bin).round() as usize;
            let last = ((high / per_bin).round() as usize).min(self.output.len() - 1);
            if first >= last {
                continue;
            }
            // Mean power over the band, so a wide band is not louder for being wide.
            let power: f32 = self.output[first..last]
                .iter()
                .map(|bin| bin.norm_sqr())
                .sum();
            bands[band] = (power / (last - first) as f32).sqrt() / WINDOW as f32 * 2.0;
        }
        bands
    }
}

impl Default for Analyzer {
    fn default() -> Self {
        Self::new()
    }
}

/// Bars per second the transcribing crest travels.
const SWEEP_SPEED: f32 = 4.5;
/// How far the crest reaches either side of its centre, in bars.
const SWEEP_REACH: f32 = 1.5;
/// Kept below a full bar so working never looks louder than speaking.
const SWEEP_HEIGHT: f32 = 0.72;

/// Height of one bar while the transcript is being produced. A single crest
/// sweeps the island, which reads as busy rather than as listening - the two
/// states have to be tellable apart at a glance.
pub fn sweep(bar: usize, seconds: f32) -> f32 {
    // A pause at each end, so the crest re-enters instead of bouncing.
    let travelled = (seconds * SWEEP_SPEED) % (BAR_COUNT as f32 + 2.0) - 1.0;
    let reach = (bar as f32 - travelled).abs() / SWEEP_REACH;
    (1.0 - reach).max(0.0) * SWEEP_HEIGHT
}

/// How much of the previous level survives one frame, rising and falling.
///
/// Held high on purpose: the island is a swell, not a meter. 0.80/0.93 still
/// covered most of the distance in a couple of frames and read as the bars
/// snapping to the voice. A syllable is a few hundred milliseconds; this can
/// take its time inside that and still arrive before the next one.
const ATTACK: f32 = 0.91;
const RELEASE: f32 = 0.96;

pub fn smooth(previous: f32, current: f32) -> f32 {
    let keep = if current > previous { ATTACK } else { RELEASE };
    previous * keep + current * (1.0 - keep)
}

/// Same easing as [`smooth`], but the ends answer faster than the centre.
///
/// One shared attack/release made the mountain sink as a single slab: every bar
/// was a scaled copy of the same voice, decaying on the same clock. Outer bars
/// catching a consonant and letting go of it first is what reads as a voice
/// instead of a fader.
pub fn smooth_bar(bar: usize, previous: f32, current: f32) -> f32 {
    let edge = (bar as f32 - 3.0).abs() / 3.0;
    let keep = if current > previous {
        ATTACK - 0.08 * edge
    } else {
        RELEASE - 0.06 * edge
    };
    previous * keep + current * (1.0 - keep)
}

/// Signed distance from a point to a rounded rectangle, negative inside it.
/// Both the island and its bars are drawn from this, which is what keeps the
/// edges smooth without a rasterising library.
pub fn rounded_rect_distance(
    point: (f32, f32),
    centre: (f32, f32),
    half: (f32, f32),
    radius: f32,
) -> f32 {
    let dx = (point.0 - centre.0).abs() - (half.0 - radius);
    let dy = (point.1 - centre.1).abs() - (half.1 - radius);
    let outside = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
    outside + dx.max(dy).min(0.0) - radius
}

/// What the island is showing. Also the message the daemon sends to change it.
#[derive(Clone, Copy)]
enum Command {
    /// Shown, but the microphone is not open yet - other apps are still being
    /// turned down. See [`Overlay::arm`].
    Arm,
    Record,
    /// The audio went to the worker. Counted so a finish cannot outrun it, but it
    /// says nothing about whether there is anything to transcribe yet.
    Queued,
    /// Recognition found words, so there is real work to wait for.
    Working,
    /// The recording was thrown away - a cancel, or a tap too short to count.
    Cancel,
    /// The transcript landed. Ignored once a new dictation has started, so a
    /// slow transcription cannot pull the island out from under the next one.
    Finish,
}

/// Handle to the drawing thread. Every method is best-effort: on a compositor
/// without layer-shell, or with no display at all, the island simply never
/// appears and dictation is untouched.
pub struct Overlay {
    commands: Sender<Command>,
}

impl Overlay {
    pub fn spawn(monitor: Monitor) -> Self {
        let (commands, incoming) = mpsc::channel();
        std::thread::spawn(move || {
            if let Err(err) = run(monitor, incoming) {
                eprintln!("overlay disabled: {err}");
            }
        });
        Self { commands }
    }

    /// Show the island before the microphone is open.
    ///
    /// Ducking takes a moment, and capture deliberately waits for it. Without
    /// this the island appeared at full strength with bars that could not move,
    /// which reads as a dead microphone - the user talks, nothing responds, and
    /// the first thing they said is gone. The armed island breathes instead of
    /// listening, and the moment it starts answering the voice is the moment
    /// there is something to answer.
    pub fn arm(&self) {
        let _ = self.commands.send(Command::Arm);
    }

    pub fn record(&self) {
        let _ = self.commands.send(Command::Record);
    }

    /// Audio handed over. Does not draw anything: until recognition has run, a
    /// cough and a sentence are indistinguishable, and a cough must not get a
    /// spinner.
    pub fn queued(&self) {
        let _ = self.commands.send(Command::Queued);
    }

    /// There are words. Refining takes long enough to be worth reporting.
    pub fn working(&self) {
        let _ = self.commands.send(Command::Working);
    }

    pub fn cancel(&self) {
        let _ = self.commands.send(Command::Cancel);
    }

    pub fn finish(&self) {
        let _ = self.commands.send(Command::Finish);
    }
}

/// Which dictation the island currently belongs to.
///
/// The island must stay up until the text has actually landed, and "landed" is
/// the end of a job that includes transcription, refining and the paste. Two ways
/// that guarantee used to break: a finish arriving for a dictation the user had
/// already replaced with a new recording, and - because dictations queue - a
/// finish for the first of two jobs hiding the island while the second was still
/// being transcribed.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Lifecycle {
    transcribing: bool,
    /// Jobs handed to the worker and not yet finished.
    pending: usize,
}

impl Lifecycle {
    /// A new recording started: the island is showing bars again, so a finish for
    /// anything earlier must no longer take it down.
    pub fn record(&mut self) {
        self.transcribing = false;
    }

    pub fn transcribe(&mut self) {
        self.transcribing = true;
        self.pending += 1;
    }

    /// Whether this finish should take the island down.
    pub fn finish(&mut self) -> bool {
        self.pending = self.pending.saturating_sub(1);
        self.transcribing && self.pending == 0
    }

    /// Nothing was queued, so there is nothing to wait for.
    pub fn cancel(&mut self) {
        self.transcribing = false;
    }
}

/// ARGB8888 pixels, little-endian and alpha-premultiplied as wl_shm wants them.
struct Canvas {
    pixels: Vec<u8>,
    width: usize,
    height: usize,
}

impl Canvas {
    fn new(width: usize, height: usize) -> Self {
        Self {
            pixels: vec![0; width * height * 4],
            width,
            height,
        }
    }

    fn clear(&mut self) {
        self.pixels.fill(0);
    }

    fn blend(&mut self, x: usize, y: usize, (r, g, b): (f32, f32, f32), alpha: f32) {
        let at = (y * self.width + x) * 4;
        let pixel = &mut self.pixels[at..at + 4];
        for (channel, value) in [b, g, r, 1.0].into_iter().enumerate() {
            let source = value * alpha * 255.0;
            pixel[channel] = (source + pixel[channel] as f32 * (1.0 - alpha)) as u8;
        }
    }

    fn rounded_rect(
        &mut self,
        centre: (f32, f32),
        half: (f32, f32),
        radius: f32,
        colour: (f32, f32, f32),
        alpha: f32,
    ) {
        self.fill(centre, half, radius, None, None, colour, alpha);
    }

    /// A hairline ring: the shape minus a smaller copy of itself.
    fn rounded_ring(
        &mut self,
        centre: (f32, f32),
        half: (f32, f32),
        radius: f32,
        thickness: f32,
        colour: (f32, f32, f32),
        alpha: f32,
    ) {
        self.fill(centre, half, radius, Some(thickness), None, colour, alpha);
    }

    /// A rounded rect that never paints outside another one - the glow's
    /// layers reach past the island's own edges, and without this they showed
    /// up as a separate soft shape floating above the pill rather than a
    /// light contained inside it.
    #[allow(clippy::too_many_arguments)]
    fn clipped_rect(
        &mut self,
        centre: (f32, f32),
        half: (f32, f32),
        radius: f32,
        clip: ((f32, f32), (f32, f32), f32),
        colour: (f32, f32, f32),
        alpha: f32,
    ) {
        self.fill(centre, half, radius, None, Some(clip), colour, alpha);
    }

    /// A ring whose brightness varies around its own circumference, from a
    /// faint track up to a bright head and back down. This is the arming
    /// spinner: one continuous stroke, rather than a string of dots pretending
    /// to be one - forty tiny circles at this size reads as a string of beads,
    /// not a ring, because there is no shared edge for the eye to follow
    /// between them.
    #[allow(clippy::too_many_arguments)]
    fn spinner_ring(
        &mut self,
        centre: (f32, f32),
        radius: f32,
        thickness: f32,
        head: f32,
        arc: f32,
        colour: (f32, f32, f32),
        track_alpha: f32,
        lit_alpha: f32,
    ) {
        let outer = radius + thickness / 2.0 + 1.0;
        let left = (centre.0 - outer).floor().max(0.0) as usize;
        let right = ((centre.0 + outer).ceil() as usize).min(self.width);
        let first_row = (centre.1 - outer).floor().max(0.0) as usize;
        let last_row = ((centre.1 + outer).ceil() as usize).min(self.height);

        for y in first_row..last_row {
            for x in left..right {
                let point = (x as f32 + 0.5, y as f32 + 0.5);
                let dx = point.0 - centre.0;
                let dy = point.1 - centre.1;
                let r = (dx * dx + dy * dy).sqrt();
                // Coverage across the ring's thickness, same one-pixel feather
                // as the rest of the canvas.
                let band = (0.5 - ((r - radius).abs() - thickness / 2.0)).clamp(0.0, 1.0);
                if band <= 0.0 {
                    continue;
                }
                // Clockwise from twelve o'clock, matching every other angle in
                // this file.
                let along = (dy.atan2(dx) / std::f32::consts::TAU + 0.25).rem_euclid(1.0);
                let behind = (head - along).rem_euclid(1.0);
                let alpha = if behind < arc {
                    track_alpha + (lit_alpha - track_alpha) * (1.0 - behind / arc)
                } else {
                    track_alpha
                };
                self.blend(x, y, colour, band * alpha);
            }
        }
    }

    /// Shared rasteriser. `hollow` leaves everything further inside than that many
    /// pixels alone, which is what turns the shape into a ring. `clip` is another
    /// rounded rect the drawing is masked to, for shapes that must never show
    /// past the island's own edge.
    // Eight parameters and a nested tuple, kept as they are on purpose. Folding
    // centre/half/radius into a RoundedRect would read better and satisfy both
    // lints, but nothing tests this function - tests/overlay.rs covers the bar
    // maths and the corner distance, never the rasteriser - so the refactor
    // would be an unverifiable change to the only code that decides what the
    // island actually looks like. Worth doing the day a pixel test exists.
    #[allow(clippy::too_many_arguments, clippy::type_complexity)]
    fn fill(
        &mut self,
        centre: (f32, f32),
        half: (f32, f32),
        radius: f32,
        hollow: Option<f32>,
        clip: Option<((f32, f32), (f32, f32), f32)>,
        colour: (f32, f32, f32),
        alpha: f32,
    ) {
        let left = (centre.0 - half.0 - 1.0).floor().max(0.0) as usize;
        let right = ((centre.0 + half.0 + 1.0).ceil() as usize).min(self.width);
        let first_row = (centre.1 - half.1 - 1.0).floor().max(0.0) as usize;
        let last_row = ((centre.1 + half.1 + 1.0).ceil() as usize).min(self.height);

        for y in first_row..last_row {
            for x in left..right {
                let point = (x as f32 + 0.5, y as f32 + 0.5);
                // Distance to coverage across one pixel is the whole anti-alias.
                let mut coverage =
                    (0.5 - rounded_rect_distance(point, centre, half, radius)).clamp(0.0, 1.0);
                if let Some((clip_centre, clip_half, clip_radius)) = clip {
                    let inside = (0.5
                        - rounded_rect_distance(point, clip_centre, clip_half, clip_radius))
                    .clamp(0.0, 1.0);
                    coverage *= inside;
                }
                if let Some(thickness) = hollow {
                    let inner = (0.5
                        - rounded_rect_distance(
                            point,
                            centre,
                            (half.0 - thickness, half.1 - thickness),
                            radius - thickness,
                        ))
                    .clamp(0.0, 1.0);
                    coverage = (coverage - inner).clamp(0.0, 1.0);
                }
                if coverage > 0.0 {
                    self.blend(x, y, colour, coverage * alpha);
                }
            }
        }
    }
}

fn render(
    canvas: &mut Canvas,
    heights: &[f32; BAR_COUNT],
    seconds: f32,
    transcribing: bool,
    arming: bool,
    // 0 at the instant the microphone opened, 1 once the bars have risen.
    wake: f32,
    scale: f32,
) {
    canvas.clear();

    let width = WIDTH as f32 * scale;
    let height = HEIGHT as f32 * scale;
    let centre = (width / 2.0, height / 2.0);
    let corner = height / 2.0;

    // While loading the island is a circle; once the microphone opens it grows
    // into the pill. Drawn from a width that changes rather than a constant
    // one, so the surface never has to be resized - everything outside the
    // shape is transparent anyway, and a Wayland resize mid-animation would
    // cost a reconfigure per frame.
    //
    // `wake` runs 0 to 1 across BLOOM, so the growth and the bars rising are
    // the same movement rather than two that have to be kept in step. `woke`
    // is set on the keypress now (Arm), not held back for the mic actually
    // opening (Record), so press, open and extend read as one motion instead
    // of a paused circle followed by a second growth. Forced back to a circle
    // only if the spinner is ever turned back on - that shape is what a
    // spinner needs, this one doesn't.
    let grown = if arming && SHOW_ARM_SPINNER {
        0.0
    } else {
        wake
    };
    let half_width = corner + (width / 2.0 - corner) * grown;
    let half = (half_width, height / 2.0);
    canvas.rounded_rect(centre, half, corner, ISLAND, ISLAND_ALPHA);
    canvas.rounded_ring(centre, half, corner, scale, EDGE, EDGE_ALPHA);

    let pitch = (BAR_WIDTH + BAR_GAP) * scale;
    let span = pitch * BAR_COUNT as f32 - BAR_GAP * scale;
    let first = (width - span) / 2.0 + BAR_WIDTH * scale / 2.0;
    // Bars spread from the centre as the pill grows, so they arrive with the
    // shape rather than appearing inside a shape that is already there.
    let spread = |x: f32| centre.0 + (x - centre.0) * grown;

    // ponytail: shelved, not deleted (SHOW_ARM_SPINNER). A spinner needs
    // something to be indeterminate about; there no longer is one - the pill
    // is already mid-bloom by the time this would draw. Flip the flag back on
    // if arming ever gets slow enough to need one again.
    if arming && SHOW_ARM_SPINNER {
        canvas.spinner_ring(
            centre,
            corner * ARM_RING,
            ARM_STROKE * scale,
            (seconds / ARM_PERIOD).fract(),
            ARM_ARC,
            BAR,
            ARM_TRACK_ALPHA,
            ARM_FILL_ALPHA,
        );
        return;
    }

    // A soft light behind the bars that rises with the voice. Built from a few
    // nested rounded rects rather than a real gradient - the canvas has no
    // gradient primitive, and overlapping translucent layers accumulate toward
    // the middle, which is a radial falloff by another name.
    //
    // Deliberately below the bars in both senses: it is drawn first, and it
    // never gets bright enough to compete with them. It should register as the
    // island warming to your voice, not as a second indicator.
    if !transcribing {
        let level = heights.iter().sum::<f32>() / BAR_COUNT as f32;
        let level = (level * wake).clamp(0.0, 1.0);
        if level > 0.01 {
            // Anchored above the middle so the light breaks over the top edge.
            // Each layer is both larger and higher than the last, so the
            // falloff runs upward as well as outward - a halo rather than a
            // blob sitting behind the bars.
            let crown = centre.1 - half.1 * GLOW_RISE;
            for layer in 0..GLOW_LAYERS {
                let out = layer as f32 / GLOW_LAYERS as f32;
                let reach = 0.45 + (GLOW_REACH - 0.45) * out;
                let size = (half.0 * reach, half.1 * reach);
                let at = (centre.0, crown - half.1 * GLOW_RISE * out * 0.5);
                // Clipped to the island's own shape: the light brightens the
                // pill from within rather than spilling past its edge.
                canvas.clipped_rect(
                    at,
                    size,
                    size.1,
                    (centre, half, corner),
                    BAR,
                    level * GLOW_ALPHA * (1.0 - out) / GLOW_LAYERS as f32,
                );
            }
        }
    }

    let alpha = if transcribing {
        BAR_WORKING_ALPHA
    } else {
        BAR_ALPHA
    };
    for (index, band) in heights.iter().enumerate() {
        let height = if transcribing {
            sweep(index, seconds)
        } else {
            // Rising from nothing on wake, so the microphone opening is a
            // visible event rather than a state the user has to infer.
            band * wake
        };
        let bar = (BAR_MIN + (BAR_MAX - BAR_MIN) * height) * scale;
        let at = (spread(first + pitch * index as f32), centre.1);
        let half = (BAR_WIDTH * scale / 2.0, bar / 2.0);
        let rim = RIM_WIDTH * scale;

        canvas.rounded_rect(
            at,
            (half.0 + rim, half.1 + rim),
            half.0 + rim,
            RIM,
            RIM_ALPHA * alpha,
        );
        canvas.rounded_rect(at, half, half.0, BAR, alpha);
    }
}

#[derive(Default)]
struct Wayland {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    layer_shell: Option<zwlr_layer_shell_v1::ZwlrLayerShellV1>,
    scale: i32,
    configured: bool,
    closed: bool,
}

/// The mapped surface. Dropped the moment recording stops, so nothing of flow
/// is on screen - or composited - between dictations.
struct Island {
    surface: wl_surface::WlSurface,
    layer: zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
}

impl Island {
    fn map(
        compositor: &wl_compositor::WlCompositor,
        shell: &zwlr_layer_shell_v1::ZwlrLayerShellV1,
        queue: &QueueHandle<Wayland>,
    ) -> Self {
        let surface = compositor.create_surface(queue, ());
        let layer = shell.get_layer_surface(
            &surface,
            None,
            zwlr_layer_shell_v1::Layer::Overlay,
            "flow".into(),
            queue,
            (),
        );
        layer.set_size(WIDTH, HEIGHT);
        layer.set_anchor(zwlr_layer_surface_v1::Anchor::Bottom);
        layer.set_margin(0, 0, MARGIN_BOTTOM, 0);
        layer.set_keyboard_interactivity(zwlr_layer_surface_v1::KeyboardInteractivity::None);

        // An empty input region makes the island click-through; an indicator
        // that swallows a click on whatever is underneath is worse than none.
        let region = compositor.create_region(queue, ());
        surface.set_input_region(Some(&region));
        region.destroy();

        surface.commit();
        Self { surface, layer }
    }
}

impl Drop for Island {
    fn drop(&mut self) {
        self.layer.destroy();
        self.surface.destroy();
    }
}

/// Two buffers so the compositor is never reading the frame we are painting.
struct Buffers {
    file: File,
    pool: wl_shm_pool::WlShmPool,
    buffers: [wl_buffer::WlBuffer; 2],
    canvas: Canvas,
    next: usize,
    scale: i32,
}

impl Buffers {
    fn create(shm: &wl_shm::WlShm, queue: &QueueHandle<Wayland>, scale: i32) -> Result<Self> {
        let width = WIDTH as i32 * scale;
        let height = HEIGHT as i32 * scale;
        let stride = width * 4;
        let frame = stride * height;

        let file = File::from(shared_memory(frame as usize * 2)?);
        let pool = shm.create_pool(file.as_fd(), frame * 2, queue, ());
        let buffers = [0, 1].map(|slot| {
            pool.create_buffer(
                slot * frame,
                width,
                height,
                stride,
                wl_shm::Format::Argb8888,
                queue,
                (),
            )
        });

        Ok(Self {
            file,
            pool,
            buffers,
            canvas: Canvas::new(width as usize, height as usize),
            next: 0,
            scale,
        })
    }

    fn present(
        &mut self,
        surface: &wl_surface::WlSurface,
        heights: &[f32; BAR_COUNT],
        seconds: f32,
        transcribing: bool,
        arming: bool,
        wake: f32,
    ) -> Result<()> {
        render(
            &mut self.canvas,
            heights,
            seconds,
            transcribing,
            arming,
            wake,
            self.scale as f32,
        );

        let slot = self.next;
        self.next = 1 - self.next;
        self.file
            .write_all_at(
                &self.canvas.pixels,
                (slot * self.canvas.pixels.len()) as u64,
            )
            .context("writing the overlay frame")?;

        surface.attach(Some(&self.buffers[slot]), 0, 0);
        surface.damage_buffer(0, 0, self.canvas.width as i32, self.canvas.height as i32);
        surface.commit();
        Ok(())
    }
}

impl Drop for Buffers {
    fn drop(&mut self) {
        for buffer in &self.buffers {
            buffer.destroy();
        }
        self.pool.destroy();
    }
}

fn shared_memory(size: usize) -> Result<OwnedFd> {
    // SAFETY: a NUL-terminated literal name, and the raw fd is taken into an
    // OwnedFd immediately so it is closed exactly once.
    let raw = unsafe { libc::memfd_create(c"flow-overlay".as_ptr(), libc::MFD_CLOEXEC) };
    if raw < 0 {
        return Err(std::io::Error::last_os_error()).context("memfd_create");
    }
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };

    // SAFETY: sizing a fresh memfd owned by `fd`.
    if unsafe { libc::ftruncate(fd.as_raw_fd(), size as libc::off_t) } != 0 {
        return Err(std::io::Error::last_os_error()).context("sizing the overlay buffer");
    }
    Ok(fd)
}

fn run(monitor: Monitor, commands: mpsc::Receiver<Command>) -> Result<()> {
    let connection = Connection::connect_to_env().context("no wayland display")?;
    let mut queue = connection.new_event_queue();
    let handle = queue.handle();
    connection.display().get_registry(&handle, ());

    let mut state = Wayland {
        scale: 1,
        ..Wayland::default()
    };
    queue.roundtrip(&mut state)?;

    let compositor = state
        .compositor
        .clone()
        .ok_or_else(|| anyhow!("compositor does not advertise wl_compositor"))?;
    let shm = state
        .shm
        .clone()
        .ok_or_else(|| anyhow!("compositor does not advertise wl_shm"))?;
    let shell = state
        .layer_shell
        .clone()
        .ok_or_else(|| anyhow!("compositor does not support wlr-layer-shell"))?;

    let mut island: Option<Island> = None;
    // Set when the audio is handed over, cleared when the sweep starts or the
    // transcript comes back empty before it ever did.
    let mut waiting_to_sweep: Option<std::time::Instant> = None;
    let mut buffers: Option<Buffers> = None;
    let mut analyzer = Analyzer::new();
    let mut window: Vec<f32> = Vec::with_capacity(WINDOW);
    let mut heights = [0.0f32; BAR_COUNT];
    // Samples already in the ring when this hold began. None once a fresh
    // window has arrived and the bars may follow the voice.
    let mut born: Option<u64> = None;
    let mut started = std::time::Instant::now();
    let mut transcribing = false;
    let mut arming = false;
    // True only while the mic is actually open. The bars must stop the
    // instant this goes false - `transcribing` is not that signal, it only
    // flips on `SWEEP_DELAY` after, which is what let the bars keep answering
    // real room sound for a couple hundred ms after the key was released.
    let mut listening = false;
    let mut woke: Option<std::time::Instant> = None;
    // The bars are still falling to rest; the sweep waits for them.
    let mut settling = false;
    let mut lifecycle = Lifecycle::default();

    loop {
        let waiting = if island.is_some() {
            commands.recv_timeout(FRAME)
        } else {
            commands.recv().map_err(|_| RecvTimeoutError::Disconnected)
        };

        match waiting {
            // Same mapping as Record, but the bars stay asleep. Kept separate
            // rather than folded into Record so a caller that never arms - the
            // unducked path, where capture starts at once - is unchanged.
            Ok(Command::Arm) => {
                heights = [0.0; BAR_COUNT];
                window.clear();
                born = Some(monitor.heard());
                transcribing = false;
                arming = true;
                // The bloom starts on the keypress rather than waiting for the
                // mic to actually open - press, open, and extend are meant to
                // read as one motion, not a hold followed by a second growth.
                woke = Some(std::time::Instant::now());
                settling = false;
                waiting_to_sweep = None;
                lifecycle.record();
                started = std::time::Instant::now();
                state.configured = false;
                state.closed = false;
                island = Some(Island::map(&compositor, &shell, &handle));
            }
            Ok(Command::Record) => {
                heights = [0.0; BAR_COUNT];
                window.clear();
                born = Some(monitor.heard());
                transcribing = false;
                settling = false;
                // Waking from armed: the island is already up, so this is the
                // moment the bars come alive and the user can speak.
                let was_armed = arming;
                arming = false;
                listening = true;
                // Only start a fresh bloom here for the unducked path, which
                // never arms - it jumps straight to Record. An armed hold
                // already started its bloom on the keypress; restarting it
                // now would snap a finished pill back to a circle and regrow
                // it right as recording begins.
                if !was_armed {
                    woke = Some(std::time::Instant::now());
                }
                // A sweep that had not started yet belongs to the dictation this
                // one replaces, and must not appear over the new bars.
                waiting_to_sweep = None;
                if !was_armed {
                    lifecycle.record();
                }
                if island.is_none() {
                    started = std::time::Instant::now();
                    // Mapped at once. The duck happens on the same keypress, so
                    // any delay here reads as the island lagging the response.
                    state.configured = false;
                    state.closed = false;
                    island = Some(Island::map(&compositor, &shell, &handle));
                }
            }
            // The island stays mapped through the handover, so the bars turn
            // into the sweep rather than blinking out and back.
            // Counted at once so a finish cannot outrun it. Nothing is drawn:
            // recognition has not run, so there may be nothing here at all.
            Ok(Command::Queued) => {
                listening = false;
                lifecycle.transcribe();
            }
            // Words exist. Still held back by SWEEP_DELAY, because a short
            // dictation can finish refining faster than a spinner is worth showing.
            Ok(Command::Working) => waiting_to_sweep = Some(std::time::Instant::now()),
            // Nothing came of the recording and the island never appeared. Leaving
            // it unshown is the whole point of the delay.
            // Only the finish that leaves nothing in flight takes the island
            // down, so the feedback outlives the paste rather than the other way
            // round. See [`Lifecycle`].
            Ok(Command::Finish) => {
                if lifecycle.finish() {
                    waiting_to_sweep = None;
                    island = None;
                    transcribing = false;
                    queue.roundtrip(&mut state)?;
                    continue;
                }
            }
            Ok(Command::Cancel) => {
                waiting_to_sweep = None;
                listening = false;
                lifecycle.cancel();
                island = None;
                transcribing = false;
                queue.roundtrip(&mut state)?;
                continue;
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return Ok(()),
        }

        // Still working after all this time, so it is worth saying so.
        if let Some(since) = waiting_to_sweep
            && since.elapsed() >= SWEEP_DELAY
        {
            waiting_to_sweep = None;
            transcribing = true;
            settling = true;
        }

        queue.roundtrip(&mut state)?;

        // The compositor can take the surface away at any point - a monitor
        // unplugged mid-sentence. Recording carries on regardless.
        if state.closed {
            island = None;
            continue;
        }
        let Some(mapped) = island.as_ref() else {
            continue;
        };
        if !state.configured {
            continue;
        }

        if buffers
            .as_ref()
            .is_none_or(|held| held.scale != state.scale)
        {
            buffers = Some(Buffers::create(&shm, &handle, state.scale)?);
        }
        let Some(buffers) = buffers.as_mut() else {
            continue;
        };

        mapped.surface.set_buffer_scale(state.scale);
        if settling {
            // Down to rest before the sweep, so the two do not collide.
            for bar in heights.iter_mut() {
                *bar *= SETTLE;
            }
            if heights.iter().all(|bar| *bar < SETTLED) {
                heights = [0.0; BAR_COUNT];
                settling = false;
                // The sweep times from the moment the island is flat, so it
                // always begins at its start rather than partway through.
                started = std::time::Instant::now();
            }
        } else if listening {
            let ready = match born {
                Some(since) if !fresh_window(monitor.heard(), since) => false,
                Some(_) => {
                    born = None;
                    false
                }
                None => true,
            };
            if ready {
                monitor.window(&mut window, WINDOW);
                let measured = analyzer.bands(&window);
                for (index, held) in heights.iter_mut().enumerate() {
                    *held = smooth_bar(index, *held, bar_height(index, &measured));
                }
            }
        }
        buffers.present(
            &mapped.surface,
            &heights,
            started.elapsed().as_secs_f32(),
            transcribing && !settling,
            arming,
            // Full immediately when nothing armed - the unducked path opens the
            // microphone on the keypress and has nothing to announce.
            woke.map_or(1.0, |at| (at.elapsed().as_secs_f32() / BLOOM).min(1.0)),
        )?;
        connection.flush()?;
    }
}

impl Dispatch<wl_registry::WlRegistry, ()> for Wayland {
    fn event(
        state: &mut Self,
        registry: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        queue: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global {
            name,
            interface,
            version,
        } = event
        else {
            return;
        };
        match &interface[..] {
            // Version 6 carries preferred_buffer_scale, which is what keeps the
            // island sharp on a scaled output instead of upscaled and soft.
            "wl_compositor" => {
                state.compositor = Some(registry.bind(name, version.min(6), queue, ()));
            }
            "wl_shm" => state.shm = Some(registry.bind(name, 1, queue, ())),
            "zwlr_layer_shell_v1" => {
                state.layer_shell = Some(registry.bind(name, version.min(4), queue, ()));
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_surface::WlSurface, ()> for Wayland {
    fn event(
        state: &mut Self,
        _: &wl_surface::WlSurface,
        event: wl_surface::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_surface::Event::PreferredBufferScale { factor } = event {
            state.scale = factor.max(1);
        }
    }
}

impl Dispatch<zwlr_layer_surface_v1::ZwlrLayerSurfaceV1, ()> for Wayland {
    fn event(
        state: &mut Self,
        layer: &zwlr_layer_surface_v1::ZwlrLayerSurfaceV1,
        event: zwlr_layer_surface_v1::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            zwlr_layer_surface_v1::Event::Configure { serial, .. } => {
                layer.ack_configure(serial);
                state.configured = true;
            }
            zwlr_layer_surface_v1::Event::Closed => state.closed = true,
            _ => {}
        }
    }
}

delegate_noop!(Wayland: ignore wl_compositor::WlCompositor);
delegate_noop!(Wayland: ignore wl_shm::WlShm);
delegate_noop!(Wayland: ignore wl_shm_pool::WlShmPool);
delegate_noop!(Wayland: ignore wl_buffer::WlBuffer);
delegate_noop!(Wayland: ignore wl_region::WlRegion);
delegate_noop!(Wayland: ignore zwlr_layer_shell_v1::ZwlrLayerShellV1);
