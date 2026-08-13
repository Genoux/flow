use flow::overlay::{band_fraction, rounded_rect_distance, smooth, sweep};

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

    assert!(quiet > 0.01, "the quietest tenth of speech is invisible: {quiet}");
    assert!(ordinary > 0.25, "ordinary speech barely moves the bar: {ordinary}");
    assert!(ordinary < loud && loud < 1.0, "ordinary {ordinary}, loud {loud}");
}

#[test]
fn the_scale_rests_at_silence_and_never_overflows() {
    assert_eq!(band_fraction(0.0), 0.0);
    assert_eq!(band_fraction(-1.0), 0.0, "a negative amplitude is not a tall bar");
    assert_eq!(band_fraction(1e-9), 0.0, "digital silence must rest");
    assert_eq!(band_fraction(10.0), 1.0, "a clipping mic must not overflow the island");
}

#[test]
fn the_scale_is_monotonic() {
    let mut previous = 0.0;
    for step in 0..60 {
        let height = band_fraction(1e-5 * 1.2f32.powi(step));
        assert!(height >= previous, "step {step} fell back: {height} after {previous}");
        previous = height;
    }
    assert_eq!(previous, 1.0, "the scale never reaches the top");
}

/// Recording and transcribing have to be tellable apart at a glance, so the
/// sweep is a single travelling crest rather than the whole island moving.
#[test]
fn the_transcribing_sweep_lights_one_end_at_a_time() {
    let lit = |seconds: f32| (0..5).filter(|bar| sweep(*bar, seconds) > 0.05).count();
    for step in 0..40 {
        let seconds = step as f32 * 0.05;
        assert!(lit(seconds) <= 3, "the sweep lit {} bars at {seconds}", lit(seconds));
        for bar in 0..5 {
            let height = sweep(bar, seconds);
            assert!((0.0..=0.8).contains(&height), "sweep out of range: {height}");
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
    assert!(seen.len() >= 4, "the crest never crossed the island: {seen:?}");
}

/// Attack is instant so the island answers the voice; release is gradual so it
/// does not collapse between syllables.
#[test]
fn level_snaps_up_and_falls_away() {
    assert_eq!(smooth(0.2, 0.9), 0.9, "a rising level must not lag");

    let mut falling = smooth(0.9, 0.0);
    assert!(falling < 0.9 && falling > 0.5, "the fall is not gradual: {falling}");
    for _ in 0..30 {
        falling = smooth(falling, 0.0);
    }
    assert!(falling < 0.01, "the island never settles: {falling}");
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
    assert!(edge.abs() < 0.001, "the top edge must sit on the boundary, got {edge}");

    let corner = rounded_rect_distance((0.5, 0.5), centre, half, radius);
    assert!(corner > 0.0, "the square corner must be cut away, got {corner}");
}
