use flow::overlay::{
    BLOOM, DWELL, SILENT, SURFACE_WIDTH, TOAST_HOLD, TOAST_LIFE, TOAST_RISE, WINDOW, arrived,
    band_fraction, bar_height, bloom, fresh_window, giving_nothing, mountain,
    rounded_rect_distance, smooth, smooth_bar, sweep, toast_grown, toast_width,
};

/// Regression, and the whole reason the scale is in decibels.
///
/// These are real per-band amplitudes measured from speech: the tenth
/// percentile, the median and the ninetieth of the highest band over
/// tests/fixtures/jfk.wav. Under the old square-root scale the floor was
/// subtracted before the curve, and since a single band carries a fraction of
/// the broadband level, everything below the ninetieth percentile clamped to
/// zero - the outer bars only moved for a raised voice.
#[test]
fn a_quiet_band_still_shows_something() {
    let quiet = band_fraction(0.00008 * 4.0);
    let ordinary = band_fraction(0.00056 * 4.0);
    let loud = band_fraction(0.00547 * 4.0);

    assert!(
        quiet > 0.01,
        "the quietest tenth of speech is invisible: {quiet}"
    );
    assert!(
        ordinary > 0.25,
        "ordinary speech barely moves the bar: {ordinary}"
    );
    assert!(
        ordinary < loud && loud < 1.0,
        "ordinary {ordinary}, loud {loud}"
    );
}

#[test]
fn the_scale_rests_at_silence_and_never_overflows() {
    assert_eq!(band_fraction(0.0), 0.0);
    assert_eq!(
        band_fraction(-1.0),
        0.0,
        "a negative amplitude is not a tall bar"
    );
    assert_eq!(band_fraction(1e-9), 0.0, "digital silence must rest");
    assert_eq!(
        band_fraction(10.0),
        1.0,
        "a clipping mic must not overflow the island"
    );
}

#[test]
fn the_scale_is_monotonic() {
    let mut previous = 0.0;
    for step in 0..60 {
        let height = band_fraction(1e-5 * 1.2f32.powi(step));
        assert!(
            height >= previous,
            "step {step} fell back: {height} after {previous}"
        );
        previous = height;
    }
    assert_eq!(previous, 1.0, "the scale never reaches the top");
}

/// The ring is always full, so a window taken on spawn is the pre-roll. The
/// bars must wait for a new window or they open already mid-syllable.
#[test]
fn the_island_ignores_audio_from_before_it_appeared() {
    assert!(!fresh_window(100, 100), "nothing new yet");
    assert!(!fresh_window(100 + WINDOW as u64 - 1, 100));
    assert!(fresh_window(100 + WINDOW as u64, 100));
}

/// The island is a mountain, not a W. The raw bands climb outward and the
/// sibilance band is gained 4.5x, which is exactly first-and-last-tall. The
/// silhouette has to win even when an "s" lights the ends.
#[test]
fn the_silhouette_is_a_mountain() {
    for bar in 0..3 {
        assert!(
            mountain(bar) < mountain(bar + 1),
            "bar {bar} is not climbing toward the crest: {} then {}",
            mountain(bar),
            mountain(bar + 1)
        );
        assert_eq!(
            mountain(bar),
            mountain(6 - bar),
            "the mountain is not mirrored"
        );
    }
    assert!(
        mountain(0) > 0.45,
        "the ends vanish on a quiet voice: {}",
        mountain(0)
    );
    assert!(
        mountain(0) < 0.6,
        "the ends are no longer below the crest: {}",
        mountain(0)
    );
    assert_eq!(mountain(3), 1.0);
}

#[test]
fn a_flat_voice_still_draws_a_mountain() {
    let bands = [0.8; 4];
    let heights: Vec<f32> = (0..7).map(|bar| bar_height(bar, &bands)).collect();
    assert!(
        heights[0] < heights[1] && heights[1] < heights[2] && heights[2] < heights[3],
        "flat speech did not rise to a crest: {heights:?}"
    );
    assert!(
        heights[3] > heights[0] * 1.5,
        "the crest is not above the ends: {heights:?}"
    );
}

/// Ordinary speech, not a raised voice. The ends used to need a shout because
/// they were 15% of the voice after the old floor and mix.
#[test]
fn a_quiet_voice_still_moves_the_ends() {
    let bands = [0.28, 0.2, 0.15, 0.05];
    let end = bar_height(0, &bands);
    assert!(end > 0.10, "a quiet voice left the ends at rest: {end}");
    assert!(
        end < bar_height(3, &bands),
        "the quiet voice is not a mountain"
    );
}

/// Same overall level, different spectrum. If every bar is just the voice
/// scaled by the mountain, a vowel and an "s" draw the same shape.
#[test]
fn the_spectrum_changes_the_silhouette() {
    let vowel = [0.5, 0.75, 0.25, 0.08];
    let hiss = [0.5, 0.2, 0.35, 0.95];
    let vowel_ratio = bar_height(0, &vowel) / bar_height(3, &vowel);
    let hiss_ratio = bar_height(0, &hiss) / bar_height(3, &hiss);
    assert!(
        (vowel_ratio - hiss_ratio).abs() > 0.06,
        "vowel {vowel_ratio:.3} vs s {hiss_ratio:.3} - the bars are still one fader"
    );
}

#[test]
fn the_ends_fall_faster_than_the_centre() {
    let end = smooth_bar(0, 0.8, 0.0);
    let mid = smooth_bar(3, 0.8, 0.0);
    assert!(
        end < mid,
        "ends {end} did not drop ahead of the centre {mid}"
    );
}

#[test]
fn sibilance_cannot_raise_the_ends_above_the_crest() {
    // Loud "s", quieter vowel - the mapping that used to draw a W.
    let bands = [0.5, 0.3, 0.3, 1.0];
    assert!(
        bar_height(0, &bands) < bar_height(3, &bands),
        "an s made the left end taller than the centre"
    );
    assert!(
        bar_height(6, &bands) < bar_height(3, &bands),
        "an s made the right end taller than the centre"
    );
}

/// Recording and transcribing have to be tellable apart at a glance, so the
/// sweep is a single travelling crest rather than the whole island moving.
#[test]
fn the_transcribing_sweep_lights_one_end_at_a_time() {
    let lit = |seconds: f32| (0..5).filter(|bar| sweep(*bar, seconds) > 0.05).count();
    for step in 0..40 {
        let seconds = step as f32 * 0.05;
        assert!(
            lit(seconds) <= 3,
            "the sweep lit {} bars at {seconds}",
            lit(seconds)
        );
        for bar in 0..5 {
            let height = sweep(bar, seconds);
            assert!(
                (0.0..=0.8).contains(&height),
                "sweep out of range: {height}"
            );
        }
    }
}

#[test]
fn the_sweep_travels_and_repeats() {
    let crest = |seconds: f32| {
        (0..5)
            .max_by(|a, b| sweep(*a, seconds).total_cmp(&sweep(*b, seconds)))
            .unwrap()
    };
    let seen: std::collections::HashSet<usize> =
        (0..60).map(|step| crest(step as f32 * 0.03)).collect();
    assert!(
        seen.len() >= 4,
        "the crest never crossed the island: {seen:?}"
    );
}

/// Both directions ease. Attack used to be instant, which is what made the bars
/// snap rather than swell - a bar that arrives in one frame reads as a flicker
/// however high the frame rate is.
#[test]
fn level_eases_in_both_directions() {
    let jumped = smooth(0.2, 0.9);
    assert!(
        jumped > 0.2 && jumped < 0.5,
        "a rise must ease, not snap: {jumped}"
    );

    let mut rising = 0.2;
    for _ in 0..40 {
        rising = smooth(rising, 0.9);
    }
    assert!(rising > 0.85, "a held level must still arrive: {rising}");

    let dropped = smooth(0.9, 0.0);
    assert!(
        dropped < 0.9 && dropped > 0.6,
        "the fall is not gradual: {dropped}"
    );
    let mut falling = dropped;
    for _ in 0..120 {
        falling = smooth(falling, 0.0);
    }
    assert!(falling < 0.01, "the island never settles: {falling}");
}

/// Neither direction may take so long that the island stops tracking the voice.
/// At the frame rate this runs at, a syllable is a few dozen frames.
#[test]
fn easing_still_keeps_up_with_speech() {
    let frames_to = |from: f32, to: f32, done: f32| {
        let mut level = from;
        for frame in 1..500 {
            level = smooth(level, to);
            if (level - to).abs() <= (to - from).abs() * (1.0 - done) {
                return frame;
            }
        }
        500
    };
    let up = frames_to(0.0, 1.0, 0.9);
    let down = frames_to(1.0, 0.0, 0.9);
    eprintln!("90% rise in {up} frames, fall in {down}");
    assert!(
        up <= 40,
        "the rise takes {up} frames, too slow to follow a syllable"
    );
    assert!((20..=90).contains(&down), "the fall takes {down} frames");
}

// -- resting on the room ----------------------------------------------------

use flow::overlay::Analyzer;

/// Measured on this machine: a quiet room reads rms 0.008 at the median and
/// 0.021 at its loudest, against the old fixed gate of 0.005 - so 92% of silence
/// was moving the bars. The floor has to come from the room, not a constant,
/// because these mics range from a webcam to a Bluetooth headset to a phone.
#[test]
fn the_bars_rest_on_room_noise() {
    let mut analyzer = Analyzer::new();
    let room = noise(0.008, 4_000);

    // A few seconds of room for the floor to settle on.
    for _ in 0..200 {
        analyzer.bands(&room);
    }
    let resting = analyzer.bands(&room);
    assert!(
        resting.iter().all(|height| *height < 0.02),
        "the bars are moving with the room: {resting:?}"
    );
}

/// And it must not gate the voice out along with the room.
#[test]
fn speech_still_moves_the_bars_over_a_noisy_room() {
    let mut analyzer = Analyzer::new();
    let room = noise(0.008, 4_000);
    for _ in 0..200 {
        analyzer.bands(&room);
    }

    let mut speech = noise(0.008, 4_000);
    for (index, sample) in speech.iter_mut().enumerate() {
        *sample += 0.09 * (index as f32 * 0.35).sin();
    }
    let heard = analyzer.bands(&speech);
    assert!(
        heard.iter().cloned().fold(0.0f32, f32::max) > 0.3,
        "a voice over a noisy room did not register: {heard:?}"
    );
}

/// A louder room must not permanently deafen the island: the floor has to be able
/// to climb as well as fall, or moving to a noisy desk leaves the bars flat.
#[test]
fn the_floor_climbs_when_the_room_gets_louder() {
    let mut analyzer = Analyzer::new();
    for _ in 0..200 {
        analyzer.bands(&noise(0.002, 4_000));
    }
    let quiet_floor = analyzer.room();
    for _ in 0..4_000 {
        analyzer.bands(&noise(0.02, 4_000));
    }
    let loud_floor = analyzer.room();
    eprintln!("floor went {quiet_floor:.5} -> {loud_floor:.5}");
    assert!(
        loud_floor > quiet_floor * 2.0,
        "the floor did not adapt upward"
    );
}

/// Deterministic pseudo-noise, so the thresholds above mean the same thing on
/// every run.
fn noise(level: f32, samples: usize) -> Vec<f32> {
    let mut state = 0x2545_F491_4F6C_DD1Du64;
    (0..samples)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            let unit = (state >> 11) as f32 / (1u64 << 53) as f32 * 2.0 - 1.0;
            unit * level * 1.7
        })
        .collect()
}

/// The rounded corners are the whole shape of the island, and coverage comes
/// straight from this distance - if it is wrong the island draws as a box.
#[test]
fn distance_is_negative_inside_and_positive_past_the_corner() {
    let centre = (50.0, 20.0);
    let half = (50.0, 20.0);
    let radius = 20.0;

    let inside = rounded_rect_distance(centre, centre, half, radius);
    assert!(inside < 0.0, "the centre must be inside, got {inside}");

    let edge = rounded_rect_distance((50.0, 0.0), centre, half, radius);
    assert!(
        edge.abs() < 0.001,
        "the top edge must sit on the boundary, got {edge}"
    );

    let corner = rounded_rect_distance((0.5, 0.5), centre, half, radius);
    assert!(
        corner > 0.0,
        "the square corner must be cut away, got {corner}"
    );
}

// -- how long the island stays up ------------------------------------------
//
// The island is the only feedback that anything is happening, so it has to
// outlive the paste. These cover the two ways it used to come down early.

use flow::overlay::Lifecycle;

#[test]
fn the_island_survives_until_the_text_has_landed() {
    let mut life = Lifecycle::default();
    life.record();
    life.transcribe();
    assert!(
        life.finish(),
        "the only job finished, so the island comes down"
    );
}

/// Dictations queue. A finish for the first must not take the island down while
/// the second is still being transcribed.
#[test]
fn a_finish_does_not_hide_the_island_while_another_job_is_pending() {
    let mut life = Lifecycle::default();
    life.record();
    life.transcribe();

    // Second dictation, started before the first came back.
    life.record();
    life.transcribe();

    assert!(!life.finish(), "first job finished, second still in flight");
    assert!(life.finish(), "now nothing is left");
}

/// A finish arriving after the user has started recording again belongs to a
/// dictation already replaced, and the island is showing bars for the new one.
#[test]
fn a_finish_for_a_replaced_dictation_is_ignored() {
    let mut life = Lifecycle::default();
    life.record();
    life.transcribe();
    life.record();

    assert!(
        !life.finish(),
        "island is showing bars for the new recording"
    );
}

#[test]
fn a_stray_finish_never_hides_anything() {
    let mut life = Lifecycle::default();
    assert!(!life.finish(), "nothing was ever queued");
    life.record();
    assert!(!life.finish(), "recording, not transcribing");
}

/// A tap let go of inside the bloom still gets a whole island. Unmapping on the
/// frame the growth lands can take the surface away before the compositor has
/// shown the finished shape at all - which is the flicker that finishing the
/// bloom was meant to remove, arriving by a different door.
#[test]
fn a_tap_released_mid_bloom_still_opens_fully() {
    assert!(!arrived(BLOOM / 3.0), "it closed part-grown");
    assert!(!arrived(BLOOM), "it closed on the frame the growth landed");
    assert!(
        arrived(BLOOM + DWELL),
        "it never opens long enough to be seen"
    );
    assert_eq!(
        bloom(BLOOM + DWELL, None),
        1.0,
        "it is allowed to go before it is fully open"
    );
}

/// The same gate every exit waits on, so it has to be free for the ones that
/// have been on screen for a while.
#[test]
fn a_real_dictation_is_never_held_back() {
    assert!(arrived(3.0), "a finished dictation waited on the dwell");
}

/// Out is in, run backwards at the same speed. The old exit ran over its own
/// slower constant, and the mismatch is what made a leaving island read as one
/// stalling as a dot.
#[test]
fn the_island_leaves_the_way_it_arrived() {
    let up = BLOOM + DWELL;
    for step in 0..=20 {
        let into = BLOOM * step as f32 / 20.0;
        let growing = bloom(into, None);
        let leaving = bloom(up, Some(BLOOM - into));
        assert!(
            (growing - leaving).abs() < 1e-5,
            "{into}s into the bloom is {growing}, the mirror of it leaving is {leaving}"
        );
    }
}

#[test]
fn an_island_nobody_sent_away_grows_and_stays() {
    assert_eq!(bloom(0.0, None), 0.0, "the island starts as a dot");
    assert_eq!(bloom(BLOOM, None), 1.0);
    assert_eq!(bloom(600.0, None), 1.0, "a long dictation must not shrink");
}

/// It has to reach nothing, or the surface would never be dropped.
#[test]
fn a_departed_island_reaches_its_dot() {
    assert_eq!(bloom(BLOOM + DWELL, Some(BLOOM)), 0.0);
    assert_eq!(bloom(BLOOM + DWELL, Some(BLOOM + 10.0)), 0.0, "and stays");
    let midway = bloom(BLOOM + DWELL, Some(BLOOM / 2.0));
    assert!(
        (0.0..1.0).contains(&midway),
        "half way out and not moving: {midway}"
    );
}

/// The message is the only thing that tells a user a dictation is not coming,
/// so it has to be readable: out, held long enough to read it, then back.
#[test]
fn the_message_widens_holds_and_narrows_back() {
    assert_eq!(toast_grown(0.0), 0.0, "it starts as the island's own pill");
    assert_eq!(toast_grown(TOAST_RISE), 1.0, "it never reaches full width");

    let held = toast_grown(TOAST_RISE + TOAST_HOLD);
    assert!(
        held > 0.999,
        "it started narrowing before the hold was over: {held}"
    );

    assert_eq!(
        toast_grown(TOAST_LIFE),
        0.0,
        "it has to end as the pill it grew out of, or the island cannot leave"
    );
    assert_eq!(toast_grown(TOAST_LIFE + 10.0), 0.0, "and stay there");
}

/// Out is in, run backwards - the same rule the island itself follows.
#[test]
fn the_message_narrows_the_way_it_widened() {
    for step in 0..=20 {
        let into = TOAST_RISE * step as f32 / 20.0;
        let widening = toast_grown(into);
        let narrowing = toast_grown(TOAST_LIFE - into);
        assert!(
            (widening - narrowing).abs() < 1e-5,
            "{into}s in is {widening}, its mirror on the way out is {narrowing}"
        );
    }
}

/// The surface is a fixed size and the toast is drawn inside it, so copy that
/// outgrows it does not overflow - it silently clips. Caught here instead.
#[test]
fn the_message_fits_the_surface() {
    for (message, scale) in [SILENT]
        .into_iter()
        .flat_map(|message| [1.0, 2.0, 3.0].map(|scale| (message, scale)))
    {
        let box_width = toast_width(message, scale);
        let surface = SURFACE_WIDTH as f32 * scale;
        assert!(
            box_width <= surface,
            "{message:?} is {box_width}px wide at scale {scale}, the surface only {surface}px"
        );
        assert!(
            box_width > surface * 0.4,
            "the surface is {surface}px for a {box_width}px toast - all that is wasted pixels"
        );
    }
}

/// The two ways a microphone gives nothing back, and the one way a quiet room
/// must not be mistaken for either.
///
/// The room-tone case is the whole reason this lives on the island rather than
/// on the finished recording: it fires mid-hold, and firing on someone pausing
/// to think would cut a real dictation short.
#[test]
fn a_dead_microphone_is_not_a_quiet_room() {
    let opened = 100;
    let delivering = opened + WINDOW as u64;

    // The device went away: nothing has arrived since the hold opened, so
    // there is no window to measure at all.
    assert!(giving_nothing(opened, opened, None), "stream stopped");
    assert!(
        giving_nothing(delivering - 1, opened, None),
        "not a full window yet"
    );

    // Muted: buffers keep coming, every sample flat.
    assert!(
        giving_nothing(delivering, opened, Some(0.0)),
        "muted source"
    );

    // Room tone with nobody speaking. An order of magnitude above the floor -
    // this is the case that must survive.
    assert!(
        !giving_nothing(delivering, opened, Some(0.046)),
        "a pause to think is not a dead microphone"
    );
    assert!(
        !giving_nothing(delivering, opened, Some(0.2)),
        "speech is obviously alive"
    );
}
