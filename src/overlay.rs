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
use wayland_client::{Connection, Dispatch, EventQueue, QueueHandle, delegate_noop};
use wayland_protocols_wlr::layer_shell::v1::client::{zwlr_layer_shell_v1, zwlr_layer_surface_v1};

use crate::audio::Monitor;

/// Inter Medium, subset to the characters the messages use. Vendored rather
/// than found on the system: this daemon has no UI toolkit and no fontconfig,
/// and a toast that silently loses its text on a machine missing some font is
/// worse than no toast. See `assets/README.md`.
static FONT: std::sync::LazyLock<fontdue::Font> = std::sync::LazyLock::new(|| {
    fontdue::Font::from_bytes(
        include_bytes!("../assets/Inter-Medium.ttf").as_slice(),
        fontdue::FontSettings::default(),
    )
    .expect("the vendored font is built into the binary")
});

/// The pill. Not the surface: the toast that replaces it is wider, and a
/// layer surface resized mid-animation costs a reconfigure per frame, so both
/// are drawn into one buffer sized for the larger of them. Everything outside
/// the shape being drawn is transparent, and the input region is empty, so the
/// extra width costs nothing but the pixels it never touches.
const WIDTH: u32 = 92;
const PILL_HEIGHT: u32 = 30;
const HEIGHT: u32 = PILL_HEIGHT;
/// Room for the widest message, since the buffer is sized once and the toast
/// widens inside it. Sized for a sentence rather than for the two fixed strings
/// the island reaches on its own: anything can be sent through [`Overlay::say`],
/// and a message that has to be cut to a few words is not worth showing. What
/// still does not fit is elided by [`fit`] rather than clipped.
pub const SURFACE_WIDTH: u32 = 420;
/// Anchored to the bottom edge, clear of it by just enough that the pill does
/// not read as cut off by the screen.
const MARGIN_BOTTOM: i32 = 8;

pub const BAR_COUNT: usize = 7;
const BAR_WIDTH: f32 = 4.0;
const BAR_GAP: f32 = 4.0;
/// Equal to the bar width, so silence rests as a row of dots rather than slivers.
const BAR_MIN: f32 = 4.0;
const BAR_MAX: f32 = 19.0;

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
const ISLAND_ALPHA: f32 = 0.55;

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
const EDGE_ALPHA: f32 = 0.09;
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

/// How long the island takes to widen into the full pill, with the bars rising
/// as it goes. This is the cue that says speaking will now be heard, so it
/// wants to be definite rather than a gentle fade anyone could miss - but slow
/// enough that the shape change registers as a shape change, and slow enough
/// that the chime riding it has something to ride.
pub const BLOOM: f32 = 0.16;

/// The narrowest the island is ever drawn, as a multiple of its corner radius.
///
/// At 1.0 the pill is exactly a circle, which reads as a loading spinner by
/// accident and is the one frame in this whole animation that says nothing
/// about what is happening. The island fades in already wider than that and
/// widens from there, so the movement reads as a pill opening and closing
/// rather than as something collapsing to a point.
///
/// It also has to be wide enough to hold the wave, which is not clipped to it:
/// at 1.7 the outermost bar hung a pixel past the rounded end while the pill
/// was at its narrowest. See `no_bar_ever_hangs_outside_the_island`, which is
/// what will say so if this or the bar geometry moves again.
const NARROWEST: f32 = 1.9;

/// How long the island sits fully open before an exit may take it away.
///
/// The island is deliberately not 1:1 with the chord. A toggle flicked on and
/// off inside a tenth of a second would otherwise produce an island that
/// flickers with it, and two chimes on top of each other - the shape and the
/// sound both need room to be one event rather than a stutter. Long enough to
/// register as "in, held, out", short enough that nobody waits on it.
pub const DWELL: f32 = 0.4;

/// Whether the island has been up long enough to be taken away. `shown` is
/// seconds since it appeared.
///
/// Every exit waits on this, not just the fast ones. A dictation that ran for
/// seconds cleared it long ago and pays nothing; a tap that came and went
/// inside the bloom is what it exists for.
pub fn arrived(shown: f32) -> bool {
    shown >= BLOOM + DWELL
}

/// How grown the island is: 0 a dot, 1 the full pill.
///
/// `shown` is seconds since the island appeared, `leaving` seconds since it was
/// told to go. Out is in, run backwards at the same speed: one movement the eye
/// learns once and then recognises. The earlier version left over a separate,
/// slower constant, and the mismatch was the whole reason the exit read as the
/// island stalling as a dot rather than as the island leaving.
pub fn bloom(shown: f32, leaving: Option<f32>) -> f32 {
    let grown = (shown / BLOOM).min(1.0);
    let Some(since) = leaving else { return grown };
    (grown - since / BLOOM).max(0.0)
}

/// When the island says anything at all.
///
/// It says nothing about an ordinary fumbled chord - arriving and leaving is
/// the whole answer to that, and a sentence every time a tap comes up short is
/// nagging. A message is for the case the user cannot fix by trying again:
/// they did everything right, holding longer would not have helped, and
/// something outside the gesture has to change before the next one works.
///
/// Two of them the island reaches on its own, from the live monitor it draws
/// the bars from - see [`Silence`]. Anything else is sent by whoever found it,
/// through [`Overlay::say`].
pub const MUTED: &str = "No sound - is your mic muted?";

/// The other silence, and deliberately not about muting. A stream that stopped
/// delivering is a device that went away or a server that dropped the client,
/// and sending someone to their mute button for that is a wrong turn they take
/// before they find the real cause.
pub const NO_INPUT: &str = "Lost the mic - is it still plugged in?";

/// Which way the microphone is giving nothing back. The two need different
/// advice, which is the whole reason they are told apart.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Silence {
    /// Buffers still arriving, every sample flat: a muted source, or one whose
    /// input is some device nobody is speaking into.
    Muted,
    /// Nothing arriving at all since the hold opened. The stream is gone, and
    /// no level can be measured because the samples it would need never came.
    Gone,
}

impl Silence {
    pub fn message(self) -> &'static str {
        match self {
            Self::Muted => MUTED,
            Self::Gone => NO_INPUT,
        }
    }
}

/// How long the microphone must give nothing before [`Silence`] is said.
///
/// Spotted mid-hold, said at the release. Only the island can reach this
/// verdict early - it has the live monitor it draws the bars from, where the
/// daemon has nothing until the recording is finished. But early is when the
/// user is still holding the key, and a message that widens out of the island
/// mid-gesture interrupts a hold they have not finished making. So the verdict
/// waits for the release, which is the same moment the island would have gone
/// quietly - and it goes with an explanation instead.
///
/// Not "you were quiet": [`crate::audio::SILENCE_RMS`] sits an order of
/// magnitude below what a real room reads, so someone gathering their thought
/// mid-hold clears it comfortably. Only a source delivering nothing at all
/// stays under it, and the delay is here for a stream that takes a beat to
/// produce its first real chunk.
const DEAD_MIC: Duration = Duration::from_millis(1500);

/// Whether a dictation should be ended because nothing is reaching the
/// microphone.
///
/// Only a hold. A tap session is bounded by the toggle that started it - the
/// user may sit quiet for as long as they like before they start speaking, and
/// calling that a dead microphone says the app stopped listening when it had
/// not. A hold is different: the key is down, so the user is talking into it,
/// and a line that has delivered nothing since it opened is not stage fright.
pub fn dead_line(holding: bool, flat_for: Option<Duration>) -> bool {
    holding && flat_for.is_some_and(|flat| flat >= DEAD_MIC)
}

/// Type size and the room left around it inside the toast.
const TOAST_TEXT: f32 = 11.0;
const TOAST_PAD: f32 = 13.0;
const TOAST_HEIGHT: f32 = 24.0;
const TOAST_RADIUS: f32 = 9.0;
const TOAST_TEXT_ALPHA: f32 = 0.88;

/// How far into the widening the text starts appearing, as a fraction of it.
const TOAST_INK: f32 = 0.55;

/// How long the island takes to widen into the message, and to narrow back out
/// of it.
///
/// Gentler than the island's own bloom. The island answers a keypress and wants
/// to be instant; the message is telling the user something after the fact, and
/// arriving with the same snap reads as an alarm. One constant for both
/// directions, for the same reason [`bloom`] has one.
pub const TOAST_RISE: f32 = 0.16;
/// Long enough to read the message twice at a glance, short enough that it is
/// gone before the next dictation. Nothing dismisses it - the surface is
/// click-through by design - so this is the only way it ever leaves.
pub const TOAST_HOLD: f32 = 1.7;

/// How wide the box for this message is at this scale: the text plus the room
/// around it. Also what decides whether a message still fits the surface - see
/// the test of the same name.
pub fn toast_width(text: &str, scale: f32) -> f32 {
    let span: f32 = text
        .chars()
        .map(|glyph| FONT.metrics(glyph, TOAST_TEXT * scale).advance_width)
        .sum();
    span + TOAST_PAD * 2.0 * scale
}

/// The message trimmed to what the surface can show, with an ellipsis where it
/// was cut.
///
/// The two fixed strings are checked against the surface by test. This is for
/// everything else: an error carries whatever the failure had to say, and the
/// old surface clipped anything too long without a mark, so a sentence could
/// lose its last word and read as complete.
///
/// Measured at scale 1 and applied at every scale. Advance widths are
/// proportional to the type size and the surface is scaled by the same factor,
/// so the ratio of text to room does not move between them.
pub fn fit(text: &str) -> String {
    let room = SURFACE_WIDTH as f32;
    if toast_width(text, 1.0) <= room {
        return text.to_string();
    }
    let mut width = toast_width("\u{2026}", 1.0);
    let mut kept = String::new();
    for glyph in text.chars() {
        width += FONT.metrics(glyph, TOAST_TEXT).advance_width;
        if width > room {
            break;
        }
        kept.push(glyph);
    }
    // The space before the cut would otherwise sit between the last word and
    // the ellipsis, which reads as a gap rather than as a trim.
    format!("{}\u{2026}", kept.trim_end())
}

/// The whole life of a message, out of the pill and back into it.
pub const TOAST_LIFE: f32 = TOAST_RISE + TOAST_HOLD + TOAST_RISE;

/// How far the message has widened out of the island: 0 still the pill, 1 the
/// full box. `shown` is seconds since it started.
///
/// Symmetrical, and it never fades. The message is the island wearing a wider
/// shape, so there is nothing to fade in or out of - it widens out of the pill,
/// holds, and narrows back into it. What is left at the end is the island
/// itself, which then leaves the way it arrived. One shape, start to finish.
pub fn toast_grown(shown: f32) -> f32 {
    let widened = shown / TOAST_RISE;
    let left = (TOAST_LIFE - shown) / TOAST_RISE;
    widened.min(left).clamp(0.0, 1.0)
}

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

/// How the microphone gave nothing back this frame, from the same live monitor
/// the bars are drawn from, or `None` if it gave something.
///
/// Two ways for a microphone to give nothing and both have to be caught. A
/// muted source keeps delivering buffers that are all zero, which is `level`
/// under [`crate::audio::SILENCE_RMS`]. A stream whose device went away stops
/// delivering at all, which is `heard` standing still since the hold opened -
/// and that one never reaches `level`, because the window waits on the very
/// samples that stopped coming.
///
/// The distinction used to be collapsed into a bool, and the one message behind
/// it told people to check their mute button for a microphone that had been
/// unplugged.
///
/// `level` is None until a window of fresh samples has arrived: before that the
/// ring still holds the room from before the keypress.
pub fn silence(heard: u64, opened: u64, level: Option<f32>) -> Option<Silence> {
    if !fresh_window(heard, opened) {
        return Some(Silence::Gone);
    }
    level
        .is_some_and(|rms| rms < crate::audio::SILENCE_RMS)
        .then_some(Silence::Muted)
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

/// Bars per second the resting swell travels. Close enough to [`SWEEP_SPEED`]
/// now that height, not pace, is what tells listening apart from working - a
/// third of the way up the bars against nearly three quarters. Slower than this
/// and a glance at the island caught the swell standing still.
const REST_SPEED: f32 = 4.2;
/// How far the swell reaches either side of its centre, in bars. Narrower than
/// it was, because a wide reach lifts most of the wave at once and reads as the
/// whole island breathing rather than as something moving across it.
const REST_REACH: f32 = 2.0;
/// Enough to be seen moving and not enough to be read as a level. Against a
/// resting bar it is about half again its height - at 0.35 the island looked
/// like it was hearing something, which is a lie while nobody is speaking.
const REST_HEIGHT: f32 = 0.22;

/// The lift under a bar while the island is listening and nobody is speaking.
///
/// The same travelling crest as [`sweep`], at a fraction of the height and a
/// fraction of the speed, so a resting island breathes instead of sitting as a
/// row of dead dots. Shaped by [`mountain`] like the voice is, or the swell
/// would cross a flat island and the voice would arrive on a curved one.
pub fn resting(bar: usize, seconds: f32) -> f32 {
    // A pause at each end, so the swell re-enters instead of bouncing.
    let travelled = (seconds * REST_SPEED) % (BAR_COUNT as f32 + 2.0) - 1.0;
    let reach = (bar as f32 - travelled).abs() / REST_REACH;
    (1.0 - reach).max(0.0) * REST_HEIGHT * mountain(bar)
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
#[derive(Clone)]
enum Command {
    /// Shown, but the microphone is not open yet - other apps are still being
    /// turned down. See [`Overlay::arm`].
    Arm,
    Record {
        /// Hold to talk. The island's life is the key's, so a line that has gone
        /// dead ends it; in tap mode the toggle is what ends it and nothing else
        /// may - see [`dead_line`].
        holding: bool,
    },
    /// The audio went to the worker. Counted so a finish cannot outrun it, but it
    /// says nothing about whether there is anything to transcribe yet.
    Queued,
    /// Recognition found words, so there is real work to wait for.
    Working,
    /// The recording was thrown away on purpose - another key turned the hold
    /// into a shortcut. The user meant it, so nothing is said about it.
    Cancel,
    /// Something failed in a way the user has to act on. The island widens into
    /// the sentence rather than leaving. Owned, not borrowed: most of what is
    /// worth saying is built from the failure that happened.
    Say(String),
    /// The transcript landed. Ignored once a new dictation has started, so a
    /// slow transcription cannot pull the island out from under the next one.
    Finish,
}

/// When the bloom should be timed from for an island that is starting again.
///
/// Normally now, from a dot. Caught on its way out it is back-dated to whatever
/// is still on screen, so a chord re-triggered inside the outro picks the growth
/// up instead of collapsing to a dot first and regrowing.
fn resume(grown: f32) -> std::time::Instant {
    std::time::Instant::now() - Duration::from_secs_f32(grown.clamp(0.0, 1.0) * BLOOM)
}

/// Drop the surface and push the destroy out to the compositor.
///
/// The roundtrip is the whole point. Dropping the [`Island`] only queues the
/// destroy requests, and the next thing this loop does with no island up is
/// block on `recv()` - so without a flush the surface stays on screen until the
/// next chord arrives to wake the loop and send them.
fn take_down(
    island: &mut Option<Island>,
    queue: &mut EventQueue<Wayland>,
    state: &mut Wayland,
) -> Result<()> {
    *island = None;
    queue.roundtrip(state)?;
    Ok(())
}

/// What the island does once it has finished arriving.
///
/// Both wait for the same gate. An exit that fires mid-bloom leaves half a
/// shape blinking out, which reads as a glitch rather than as a chord that came
/// to nothing - and a message that starts widening out of a pill still growing
/// reads as one twitch rather than two movements.
#[derive(Clone)]
enum Ending {
    /// Go. The island having appeared at all is the answer.
    Close,
    /// Widen into this first, and go when it has had its time.
    Say(String),
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

    pub fn record(&self, holding: bool) {
        let _ = self.commands.send(Command::Record { holding });
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

    /// Tell the user something. The island widens out of its pill into the
    /// sentence, holds it long enough to read twice, and narrows back.
    ///
    /// For what the user has to act on and cannot fix by dictating again: a
    /// failure that will happen the same way next time, a setting that has to
    /// change. Ordinary disappointments - a tap too short, a cough, nothing
    /// recognised - are not for this. The island arriving and leaving is
    /// already the whole answer to those, and a sentence about each one turns
    /// the island from an indicator into something that talks back.
    ///
    /// Anything too long for the surface is elided by [`fit`] rather than
    /// clipped, so a message built from an error is safe to pass straight in.
    pub fn say(&self, message: impl AsRef<str>) {
        let _ = self.commands.send(Command::Say(fit(message.as_ref())));
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

    /// Shared rasteriser. `hollow` leaves everything further inside than that many
    /// pixels alone, which is what turns the shape into a ring. `clip` is another
    /// rounded rect the drawing is masked to, for shapes that must never show
    /// past the island's own edge.
    /// One line of text, centred on `at`, its baseline placed so the line sits
    /// on the middle rather than hanging from it.
    ///
    /// Laid out by hand: fontdue's layout engine exists for paragraphs, and
    /// every string here is one short line with no wrapping, no shaping and no
    /// bidi. Kerning is skipped for the same reason - at this size and this
    /// length nobody can see it, and asking for it means a shaping pass.
    fn text(&mut self, line: &str, size: f32, at: (f32, f32), colour: (f32, f32, f32), alpha: f32) {
        let span: f32 = line
            .chars()
            .map(|glyph| FONT.metrics(glyph, size).advance_width)
            .sum();
        // Centring on the cap height rather than the full line box: the line
        // box is sized for descenders and accents most of these strings never
        // use, and centring on it leaves the text visibly high in the pill.
        let middle = FONT
            .horizontal_line_metrics(size)
            .map_or(size * 0.35, |line| line.ascent * 0.36);

        let mut pen = at.0 - span / 2.0;
        let baseline = at.1 + middle;
        for glyph in line.chars() {
            let (metrics, coverage) = FONT.rasterize(glyph, size);
            let left = pen + metrics.xmin as f32;
            let top = baseline - (metrics.height as f32 + metrics.ymin as f32);
            for row in 0..metrics.height {
                for column in 0..metrics.width {
                    let ink = coverage[row * metrics.width + column] as f32 / 255.0;
                    if ink > 0.0 {
                        let x = (left + column as f32).round();
                        let y = (top + row as f32).round();
                        if x >= 0.0 && y >= 0.0 {
                            self.blend(x as usize, y as usize, colour, ink * alpha);
                        }
                    }
                }
            }
            pen += metrics.advance_width;
        }
    }

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

/// The message a chord that recorded nothing turns into.
///
/// Not a second shape that replaces the island: the same rounded rect, drawn
/// from the island's own pill out to the width of the sentence. The chord did
/// do something, and one continuous expansion is what says so - the island
/// retracting to a dot first said the opposite, that the gesture had come to
/// nothing, a moment before the words arrived to say it had not.
fn render_toast(canvas: &mut Canvas, text: &str, grown: f32, scale: f32) {
    let height = PILL_HEIGHT as f32 * scale;
    let centre = (canvas.width as f32 / 2.0, canvas.height as f32 / 2.0);

    let widen = |from: f32, to: f32| from + (to - from) * grown;
    let half = (
        widen(WIDTH as f32 * scale / 2.0, toast_width(text, scale) / 2.0),
        widen(height / 2.0, TOAST_HEIGHT * scale / 2.0),
    );
    let corner = widen(height / 2.0, TOAST_RADIUS * scale);

    canvas.rounded_rect(centre, half, corner, ISLAND, ISLAND_ALPHA);
    canvas.rounded_ring(centre, half, corner, scale, EDGE, EDGE_ALPHA);

    // Held back until the box is nearly the right width. The sentence is wider
    // than the island it grows from, so ink any earlier is glyphs hanging off
    // both ends of the shape that is supposed to contain them.
    // Held back until the box is nearly the right width, and gone again before
    // it narrows. The sentence is wider than the island it grows from, so ink
    // outside that window is glyphs hanging off both ends of the shape that is
    // supposed to contain them.
    let ink = ((grown - TOAST_INK) / (1.0 - TOAST_INK)).clamp(0.0, 1.0);
    canvas.text(
        text,
        TOAST_TEXT * scale,
        centre,
        BAR,
        TOAST_TEXT_ALPHA * ink,
    );
}

/// What the surface is painting this frame.
///
/// The island and the message are never both up - one replaces the other - and
/// an enum is what says so. Passed as parameters instead, a toast would sit
/// beside the island's own fields it silently makes dead.
enum Face<'a> {
    Island {
        heights: &'a [f32; BAR_COUNT],
        seconds: f32,
        transcribing: bool,
        /// 0 at the instant the microphone opened, 1 once the bars have risen.
        wake: f32,
    },
    Toast {
        text: &'a str,
        /// 0 still the island's pill, 1 the full message box.
        grown: f32,
    },
}

fn render(canvas: &mut Canvas, face: Face, scale: f32) {
    canvas.clear();
    match face {
        Face::Island {
            heights,
            seconds,
            transcribing,
            wake,
        } => render_island(canvas, heights, seconds, transcribing, wake, scale),
        Face::Toast { text, grown } => render_toast(canvas, text, grown, scale),
    }
}

#[allow(clippy::too_many_arguments)]
fn render_island(
    canvas: &mut Canvas,
    heights: &[f32; BAR_COUNT],
    seconds: f32,
    transcribing: bool,
    wake: f32,
    scale: f32,
) {
    let centre = (canvas.width as f32 / 2.0, canvas.height as f32 / 2.0);
    let height = PILL_HEIGHT as f32 * scale;
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
    // of a paused circle followed by a second growth.
    let grown = wake;
    // Never narrower than [`NARROWEST`], and already fading by the time it is
    // that narrow: the island is a pill widening and narrowing, and it is never
    // caught being a circle with a dot in it at either end.
    let stub = corner * NARROWEST;
    let half_width = stub + (WIDTH as f32 * scale / 2.0 - stub) * grown;
    let pill = (half_width, height / 2.0);
    // Opacity is the growth, not a slice of it. Fading over a fraction meant the
    // whole fade was spent inside the first few frames of an already short
    // bloom - forty milliseconds, which is not a fade anybody sees. Widening and
    // lighting up now take exactly as long as each other.
    let visible = grown;

    // How loud you are, in one number. Hoisted because both halves of the glow
    // want it: the halo outside the pill and the light inside it are one
    // response to your voice, not two.
    let level = if transcribing {
        0.0
    } else {
        (heights.iter().sum::<f32>() / BAR_COUNT as f32).clamp(0.0, 1.0)
    };
    canvas.rounded_rect(centre, pill, corner, ISLAND, ISLAND_ALPHA * visible);
    canvas.rounded_ring(centre, pill, corner, scale, EDGE, EDGE_ALPHA * visible);

    let pitch = (BAR_WIDTH + BAR_GAP) * scale;
    let span = pitch * BAR_COUNT as f32 - BAR_GAP * scale;
    let first = centre.0 - span / 2.0 + BAR_WIDTH * scale / 2.0;

    // A soft light behind the bars that rises with the voice. Built from a few
    // nested rounded rects rather than a real gradient - the canvas has no
    // gradient primitive, and overlapping translucent layers accumulate toward
    // the middle, which is a radial falloff by another name.
    //
    // Deliberately below the bars in both senses: it is drawn first, and it
    // never gets bright enough to compete with them. It should register as the
    // island warming to your voice, not as a second indicator.
    {
        if level > 0.01 {
            // Anchored above the middle so the light breaks over the top edge.
            // Each layer is both larger and higher than the last, so the
            // falloff runs upward as well as outward - a halo rather than a
            // blob sitting behind the bars.
            let crown = centre.1 - pill.1 * GLOW_RISE;
            for layer in 0..GLOW_LAYERS {
                let out = layer as f32 / GLOW_LAYERS as f32;
                let reach = 0.45 + (GLOW_REACH - 0.45) * out;
                let size = (pill.0 * reach, pill.1 * reach);
                let at = (centre.0, crown - pill.1 * GLOW_RISE * out * 0.5);
                // Clipped to the island's own shape: the light brightens the
                // pill from within rather than spilling past its edge.
                canvas.clipped_rect(
                    at,
                    size,
                    size.1,
                    (centre, pill, corner),
                    BAR,
                    level * GLOW_ALPHA * (1.0 - out) / GLOW_LAYERS as f32 * visible,
                );
            }
        }
    }

    // The same fade the island itself uses, because they are one movement: the
    // pill opens and the wave lights up inside it over the same span. Clipping
    // the bars to the pill instead made the shape a hole they slid out from
    // behind, which is a different and busier idea than an island turning on.
    let alpha = visible
        * if transcribing {
            BAR_WORKING_ALPHA
        } else {
            BAR_ALPHA
        };
    // The wave never moves and never collapses. Every bar keeps its own place
    // and its own height from the first frame to the last, and only its opacity
    // changes. Scaling the bars into the centre instead made the shape and its
    // contents two animations fighting over one movement, and left the island
    // ending on a dot with everything piled inside it.
    for (index, band) in heights.iter().enumerate() {
        // The voice always wins: the swell is a floor under a resting island,
        // not something mixed into what is being said.
        let height = if transcribing {
            sweep(index, seconds)
        } else {
            band.max(resting(index, seconds))
        };
        let bar = (BAR_MIN + (BAR_MAX - BAR_MIN) * height) * scale;
        let at = (first + pitch * index as f32, centre.1);
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
    /// Also the moment the island is heard. Both callers reach the surface
    /// through here, so the chime lives with the mapping rather than at each
    /// one - and it goes first, ahead of the Wayland round trips and well
    /// ahead of the ducking that follows the arm.
    fn map(
        compositor: &wl_compositor::WlCompositor,
        shell: &zwlr_layer_shell_v1::ZwlrLayerShellV1,
        queue: &QueueHandle<Wayland>,
    ) -> Self {
        crate::chime::show();

        let surface = compositor.create_surface(queue, ());
        let layer = shell.get_layer_surface(
            &surface,
            None,
            zwlr_layer_shell_v1::Layer::Overlay,
            "flow".into(),
            queue,
            (),
        );
        layer.set_size(SURFACE_WIDTH, HEIGHT);
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
        let width = SURFACE_WIDTH as i32 * scale;
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

    fn present(&mut self, surface: &wl_surface::WlSurface, face: Face) -> Result<()> {
        render(&mut self.canvas, face, self.scale as f32);

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

/// A connected display with everything the island needs off the registry.
struct Display {
    connection: Connection,
    queue: EventQueue<Wayland>,
    state: Wayland,
    compositor: wl_compositor::WlCompositor,
    shm: wl_shm::WlShm,
    shell: zwlr_layer_shell_v1::ZwlrLayerShellV1,
}

fn connect() -> Result<Display> {
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

    Ok(Display {
        connection,
        queue,
        state,
        compositor,
        shm,
        shell,
    })
}

fn run(monitor: Monitor, commands: mpsc::Receiver<Command>) -> Result<()> {
    let Display {
        connection,
        mut queue,
        mut state,
        compositor,
        shm,
        shell,
    } = connect()?;
    let handle = queue.handle();

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
    // The same count, kept past the point `born` clears: a stream that has died
    // is one whose count never moves, and that has to stay checkable for the
    // whole hold rather than only until the bars wake up.
    let mut opened = 0u64;
    // When the microphone last gave anything, and which way it is giving
    // nothing. None while it is delivering.
    let mut flat_since: Option<(std::time::Instant, Silence)> = None;
    let mut started = std::time::Instant::now();
    let mut transcribing = false;
    let mut arming = false;
    // True only while the mic is actually open. The bars must stop the
    // instant this goes false - `transcribing` is not that signal, it only
    // flips on `SWEEP_DELAY` after, which is what let the bars keep answering
    // real room sound for a couple hundred ms after the key was released.
    let mut listening = false;
    // Hold to talk, carried from the command that opened the microphone. A tap
    // session is the user's to end - see [`dead_line`].
    let mut holding = true;
    // Set while the microphone has been giving nothing for long enough to be
    // worth saying so, and holding which silence it is. Said when the dictation
    // ends, never into a live hold.
    let mut dead: Option<Silence> = None;
    let mut woke = std::time::Instant::now();
    // Last drawn size. The message waits on it to reach full before widening,
    // and a chord re-triggered on the way out picks the growth back up from it.
    let mut grown = 0.0f32;
    // Set when the island has started leaving. It runs [`bloom`] backwards and
    // the surface goes when it reaches its dot.
    let mut leaving: Option<std::time::Instant> = None;
    // How this dictation ends, once the island has finished arriving.
    let mut ending: Option<Ending> = None;
    let mut toast: Option<(std::time::Instant, String)> = None;
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
                opened = monitor.heard();
                born = Some(opened);
                flat_since = None;
                transcribing = false;
                arming = true;
                settling = false;
                waiting_to_sweep = None;
                ending = None;
                toast = None;
                lifecycle.record();
                if island.is_none() {
                    grown = 0.0;
                    started = std::time::Instant::now();
                    state.configured = false;
                    state.closed = false;
                    island = Some(Island::map(&compositor, &shell, &handle));
                }
                // The bloom starts on the keypress rather than waiting for the
                // mic to actually open - press, open, and extend are meant to
                // read as one motion, not a hold followed by a second growth.
                //
                // Back-dated to whatever is still on screen, so a chord caught
                // during the outro carries on from the size it is rather than
                // collapsing to a dot and regrowing.
                woke = resume(grown);
                leaving = None;
            }
            Ok(Command::Record { holding: held }) => {
                heights = [0.0; BAR_COUNT];
                window.clear();
                opened = monitor.heard();
                born = Some(opened);
                flat_since = None;
                transcribing = false;
                settling = false;
                // Waking from armed: the island is already up, so this is the
                // moment the bars come alive and the user can speak.
                let was_armed = arming;
                arming = false;
                listening = true;
                holding = held;
                // Only start a fresh bloom here for the unducked path, which
                // never arms - it jumps straight to Record. An armed hold
                // already started its bloom on the keypress; restarting it
                // now would snap a finished pill back to a circle and regrow
                // it right as recording begins.
                if !was_armed {
                    woke = resume(grown);
                }
                leaving = None;
                // A sweep that had not started yet belongs to the dictation this
                // one replaces, and must not appear over the new bars.
                waiting_to_sweep = None;
                ending = None;
                toast = None;
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
            // `get_or_insert`, not an assignment: the silent-microphone path
            // says its piece and is then finished like any other dictation, and
            // a plain close arriving second must not talk over it.
            Ok(Command::Finish) => {
                if lifecycle.finish() {
                    waiting_to_sweep = None;
                    ending.get_or_insert(Ending::Close);
                }
            }
            // A tap too short to record ends here too. The island arriving and
            // going is the whole answer to it - the chord did something, the
            // user saw that it did, and nothing is owed beyond that.
            Ok(Command::Cancel) => {
                waiting_to_sweep = None;
                listening = false;
                lifecycle.cancel();
                ending.get_or_insert(Ending::Close);
            }
            // The one ending that is not a leaving: the island stays out and
            // widens into the message instead - see [`toast_grown`].
            Ok(Command::Say(message)) => {
                waiting_to_sweep = None;
                listening = false;
                lifecycle.cancel();
                ending = Some(Ending::Say(message));
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
            toast = None;
            ending = None;
            leaving = None;
            grown = 0.0;
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

            let level = born.is_none().then(|| crate::audio::rms(&window));
            match silence(monitor.heard(), opened, level) {
                // The clock keeps running across a change of kind - a stream
                // that goes away while muted is one unbroken silence - but the
                // reason follows the latest read, which is the one the message
                // has to be right about.
                Some(reason) => {
                    flat_since
                        .get_or_insert((std::time::Instant::now(), reason))
                        .1 = reason;
                }
                None => flat_since = None,
            }
        }

        // Nothing is reaching the microphone. Noted, not said: while the key is
        // down the user is still making the gesture, and a box of text widening
        // out of the island mid-hold answers a question they have not finished
        // asking. It is delivered when the dictation ends - see [`DEAD_MIC`].
        //
        // Recomputed every frame rather than latched, so a line that comes back
        // clears it: `flat_since` is reset the moment a real sample arrives.
        if listening {
            dead = flat_since
                .filter(|(since, _)| dead_line(holding, Some(since.elapsed())))
                .map(|(_, reason)| reason);
        }
        grown = bloom(
            woke.elapsed().as_secs_f32(),
            leaving.map(|at| at.elapsed().as_secs_f32()),
        );
        let face = match &toast {
            Some((at, text)) => Face::Toast {
                text,
                grown: toast_grown(at.elapsed().as_secs_f32()),
            },
            None => Face::Island {
                heights: &heights,
                seconds: started.elapsed().as_secs_f32(),
                transcribing: transcribing && !settling,
                wake: grown,
            },
        };
        buffers.present(&mapped.surface, face)?;
        connection.flush()?;

        // Only now, with the finished shape already on its way to the
        // compositor, may the island act on how this dictation ends. Nothing
        // leaves mid-bloom and nothing interrupts a message that is up.
        if toast.is_none()
            && leaving.is_none()
            && arrived(woke.elapsed().as_secs_f32())
            && let Some(end) = ending.take()
        {
            transcribing = false;
            settling = false;
            match end {
                Ending::Say(message) => toast = Some((std::time::Instant::now(), message)),
                // The dead line spotted mid-hold, delivered now that the key is
                // up. Whatever ended the dictation, this is the reason it had
                // nothing to show for itself.
                Ending::Close if dead.is_some() => {
                    let reason = dead.take().expect("checked above");
                    toast = Some((std::time::Instant::now(), reason.message().to_string()));
                }
                Ending::Close => {
                    crate::chime::hide();
                    leaving = Some(std::time::Instant::now());
                }
            }
        }

        // The message has had its time and has narrowed back into the pill it
        // came out of. Nothing dismisses it but this - the surface is
        // click-through, so there is nothing to dismiss it with. What is left is
        // the island, which now leaves the way it arrived.
        if toast
            .as_ref()
            .is_some_and(|(at, _)| at.elapsed().as_secs_f32() >= TOAST_LIFE)
        {
            toast = None;
            ending = None;
            crate::chime::hide();
            leaving = Some(std::time::Instant::now());
        }

        // Pulled all the way back into its dot, so there is nothing left to show.
        if leaving.is_some() && grown <= 0.0 {
            leaving = None;
            take_down(&mut island, &mut queue, &mut state)?;
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Where the toast actually put ink, as (left, right, top, bottom).
    fn ink(canvas: &Canvas, colour_floor: u8) -> (usize, usize, usize, usize) {
        let (mut left, mut right, mut top, mut bottom) = (canvas.width, 0, canvas.height, 0);
        for y in 0..canvas.height {
            for x in 0..canvas.width {
                // The text is the only near-white thing drawn; the pill behind
                // it is dark and its edge ring is barely there.
                if canvas.pixels[(y * canvas.width + x) * 4 + 1] > colour_floor {
                    left = left.min(x);
                    right = right.max(x);
                    top = top.min(y);
                    bottom = bottom.max(y);
                }
            }
        }
        (left, right, top, bottom)
    }

    /// The baseline and the pen advance are both hand-rolled, and both fail
    /// quietly: text drawn a few pixels low still renders, it just sits wrong
    /// in the pill. This is what notices.
    #[test]
    fn the_message_sits_centred_in_its_box() {
        let scale = 2.0;
        let mut canvas = Canvas::new(SURFACE_WIDTH as usize * 2, HEIGHT as usize * 2);
        render(
            &mut canvas,
            Face::Toast {
                text: MUTED,
                grown: 1.0,
            },
            scale,
        );

        let (left, right, top, bottom) = ink(&canvas, 160);
        assert!(left < right && top < bottom, "the message drew nothing");

        let slack = 2.0 * scale;
        let across = (left + right) as f32 / 2.0;
        assert!(
            (across - canvas.width as f32 / 2.0).abs() < slack,
            "the text runs off centre: {left}..{right} in {} wide",
            canvas.width
        );
        let down = (top + bottom) as f32 / 2.0;
        assert!(
            (down - canvas.height as f32 / 2.0).abs() < slack,
            "the text hangs off the middle: {top}..{bottom} in {} tall",
            canvas.height
        );

        let inside =
            (SURFACE_WIDTH as f32 * scale - toast_width(MUTED, scale)) / 2.0 + TOAST_PAD * scale;
        assert!(
            left as f32 >= inside - slack && (right as f32) <= canvas.width as f32 - inside + slack,
            "the text overruns the padding: {left}..{right}, box starts at {inside}"
        );
    }

    /// The wave does not animate. Bars keep their height while the island opens
    /// and closes around them - scaling them with the pill made the shape and
    /// its contents two movements competing for one gesture.
    #[test]
    fn the_bars_keep_their_height_while_the_island_opens() {
        let heights = [0.8; BAR_COUNT];
        // Relative to the brightest thing on the canvas, not a fixed number:
        // the bars are drawn at whatever opacity the fade is at, so an absolute
        // threshold measures the fade rather than the geometry.
        let extent = |wake: f32| {
            let mut canvas = Canvas::new(SURFACE_WIDTH as usize, HEIGHT as usize);
            render(
                &mut canvas,
                Face::Island {
                    heights: &heights,
                    seconds: 0.0,
                    transcribing: false,
                    wake,
                },
                1.0,
            );
            let peak = canvas
                .pixels
                .chunks(4)
                .map(|pixel| pixel[1])
                .max()
                .expect("a painted canvas");
            let (_, _, top, bottom) = ink(&canvas, peak / 2);
            (top, bottom)
        };

        // The bars are drawn at both, so the two frames differ only in how wide
        // the pill is - any change in the bars themselves is the wave animating.
        let part_open = extent(0.5);
        assert!(part_open.0 < part_open.1, "no bars drawn to compare");
        assert_eq!(
            part_open,
            extent(1.0),
            "the bars changed height as the island widened"
        );
    }

    /// The wave is not clipped to the pill, so nothing structural stops a bar
    /// hanging outside it. What keeps them in is arithmetic between five
    /// constants ([`NARROWEST`], `BAR_MAX`, `BAR_COUNT`, `BAR_WIDTH`, `HEIGHT`),
    /// and arithmetic nobody wrote down is arithmetic that breaks quietly. Tall
    /// bars at every width, checked against the shape they sit in.
    #[test]
    fn no_bar_ever_hangs_outside_the_island() {
        let scale = 1.0;
        for step in 0..=40 {
            let wake = step as f32 / 40.0;
            let mut canvas = Canvas::new(SURFACE_WIDTH as usize, HEIGHT as usize);
            render_island(&mut canvas, &[1.0; BAR_COUNT], 0.0, false, wake, scale);

            // The same pill render_island draws, so a disagreement here is the
            // two of them having drifted apart.
            let height = HEIGHT as f32 * scale;
            let corner = height / 2.0;
            let centre = (SURFACE_WIDTH as f32 * scale / 2.0, height / 2.0);
            let stub = corner * NARROWEST;
            let pill = (
                stub + (WIDTH as f32 * scale / 2.0 - stub) * wake,
                height / 2.0,
            );

            for y in 0..canvas.height {
                for x in 0..canvas.width {
                    let at = (y * canvas.width + x) * 4;
                    // Above the antialiasing fringe: an edge pixel sitting a
                    // hair outside the shape is the rasteriser, not a stray bar.
                    if canvas.pixels[at + 3] <= 8 {
                        continue;
                    }
                    let point = (x as f32 + 0.5, y as f32 + 0.5);
                    let outside = rounded_rect_distance(point, centre, pill, corner);
                    assert!(
                        outside <= 0.5,
                        "at wake {wake} a pixel at {point:?} is {outside}px outside the island"
                    );
                }
            }
        }
    }

    /// Never a circle with a dot in it. Whatever the island is drawn at, it is
    /// wider than it is tall - a pill - or it is not drawn at all. The shape it
    /// used to pass through on the way out was a ring with the bars collapsed to
    /// its centre, which reads as a spinner and says nothing.
    #[test]
    fn the_island_is_never_caught_being_a_circle() {
        for step in 0..=40 {
            let wake = step as f32 / 40.0;
            let mut canvas = Canvas::new(SURFACE_WIDTH as usize, HEIGHT as usize);
            render(
                &mut canvas,
                Face::Island {
                    heights: &[0.0; BAR_COUNT],
                    seconds: 0.0,
                    transcribing: false,
                    wake,
                },
                1.0,
            );

            let lit = |x: usize| {
                (0..canvas.height).any(|y| canvas.pixels[(y * canvas.width + x) * 4 + 3] > 0)
            };
            let drawn = (0..canvas.width).filter(|x| lit(*x)).count();
            if drawn == 0 {
                continue;
            }
            assert!(
                drawn > HEIGHT as usize,
                "at wake {wake} the island is {drawn}px wide and {HEIGHT}px tall - a circle"
            );
        }
    }

    /// A finished message is the island again, not a blank surface: it narrows
    /// all the way back into the pill it came out of, and the pill is what then
    /// leaves. The text has to be gone by then, or glyphs would be left hanging
    /// off both ends of a shape too small to hold them.
    #[test]
    fn a_finished_message_is_the_island_again() {
        let mut ended = Canvas::new(SURFACE_WIDTH as usize, HEIGHT as usize);
        render(
            &mut ended,
            Face::Toast {
                text: MUTED,
                grown: toast_grown(TOAST_LIFE),
            },
            1.0,
        );
        assert!(
            ended.pixels.iter().any(|channel| *channel != 0),
            "the message vanished instead of narrowing back into the island"
        );

        let (left, right, ..) = ink(&ended, 160);
        assert!(left > right, "the text is still painting at full width");

        let mut island = Canvas::new(SURFACE_WIDTH as usize, HEIGHT as usize);
        render(
            &mut island,
            Face::Island {
                heights: &[0.0; BAR_COUNT],
                seconds: 0.0,
                transcribing: false,
                wake: 1.0,
            },
            1.0,
        );
        let width = |canvas: &Canvas| {
            (0..canvas.width)
                .filter(|x| {
                    (0..canvas.height).any(|y| canvas.pixels[(y * canvas.width + x) * 4 + 3] > 0)
                })
                .count()
        };
        assert_eq!(
            width(&ended),
            width(&island),
            "the message did not end at the island's own width"
        );
    }
}
