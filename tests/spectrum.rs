use flow::overlay::{Analyzer, BAND_COUNT, WINDOW};

fn tone(hz: f32, amplitude: f32) -> Vec<f32> {
    (0..WINDOW * 2)
        .map(|n| amplitude * (std::f32::consts::TAU * hz * n as f32 / 16_000.0).sin())
        .collect()
}

/// Bands 1 to 3 are frequency slices, so a pure tone must light its own and leave
/// its neighbours alone. If the bin arithmetic drifts the island stops meaning
/// anything - it would still wobble, just not with the voice.
#[test]
fn a_tone_lights_only_its_own_slice() {
    let mut analyzer = Analyzer::new();
    for (slice, hz) in [500.0, 1500.0, 4000.0].iter().enumerate() {
        let expected = slice + 1;
        let bands = analyzer.bands(&tone(*hz, 0.2));
        let loudest = (1..BAND_COUNT)
            .max_by(|a, b| bands[*a].total_cmp(&bands[*b]))
            .unwrap();
        assert_eq!(loudest, expected, "{hz}Hz lit band {loudest}: {bands:?}");

        let leaked = (1..BAND_COUNT)
            .filter(|band| *band != expected)
            .map(|band| bands[band])
            .fold(0.0f32, f32::max);
        assert!(
            leaked < 0.05,
            "{hz}Hz leaked {leaked} into a neighbour: {bands:?}"
        );
    }
}

/// Band 0 is the broadband level and draws the centre bar, so unlike the slices it
/// has to answer every sound - that is the whole reason it is not a slice.
#[test]
fn the_centre_answers_every_frequency() {
    let mut analyzer = Analyzer::new();
    for hz in [120.0, 500.0, 1500.0, 4000.0, 6000.0] {
        let bands = analyzer.bands(&tone(hz, 0.2));
        assert!(bands[0] > 0.2, "{hz}Hz left the centre bar at {}", bands[0]);
    }
}

#[test]
fn a_silent_window_rests_and_a_short_one_is_ignored() {
    let mut analyzer = Analyzer::new();
    assert_eq!(analyzer.bands(&vec![0.0; WINDOW * 2]), [0.0; BAND_COUNT]);
    assert_eq!(
        analyzer.bands(&[0.5; 8]),
        [0.0; BAND_COUNT],
        "a partial window must not draw"
    );
}

/// BAND_GAIN calibration. Speech energy falls steeply with frequency, so
/// without per-band gain the right-hand bars never move. This holds the balance
/// against real speech: every band has to carry some of the picture, and none
/// may sit pinned at full height through the sentence.
///
/// jfk.wav is a 1961 recording and is thin at both extremes, so the outer bands
/// read low here and the thresholds allow for it. Re-run against a recording
/// from the actual microphone before retuning BAND_GAIN.
#[test]
fn every_band_carries_part_of_real_speech() {
    let samples = flow::wav::read_16k_mono("tests/fixtures/jfk.wav").expect("fixture");
    let mut analyzer = Analyzer::new();

    let (mut totals, mut pinned) = ([0.0f32; BAND_COUNT], [0u32; BAND_COUNT]);
    let mut frames: Vec<[f32; BAND_COUNT]> = Vec::new();
    let mut voiced = 0;
    let mut at = WINDOW;
    while at <= samples.len() {
        let bands = analyzer.bands(&samples[..at]);
        if bands.iter().cloned().fold(0.0f32, f32::max) > 0.05 {
            for band in 0..BAND_COUNT {
                totals[band] += bands[band];
                if bands[band] >= 0.999 {
                    pinned[band] += 1;
                }
            }
            voiced += 1;
            frames.push(bands);
        }
        at += 256;
    }

    assert!(
        voiced > 200,
        "the fixture yielded only {voiced} voiced frames"
    );

    // The bars mirror these bands about the island's centre, so band 0 draws
    // the middle and band 2 the two ends. Speech has to fall away outward or
    // the island shows a valley where its crest belongs.
    let means: Vec<f32> = (0..BAND_COUNT).map(|b| totals[b] / voiced as f32).collect();
    assert!(
        means.windows(2).all(|pair| pair[0] > pair[1]),
        "the island does not crest in the middle: {means:?}"
    );

    // The complaint this guards: the outer bars sat at their rest for most of a
    // sentence and only moved for a raised voice. Every bar has to be doing
    // something through ordinary speech, even if only a little.
    for band in 0..BAND_COUNT {
        let flat =
            frames.iter().filter(|heights| heights[band] < 0.02).count() * 100 / frames.len();
        assert!(flat < 10, "band {band} sat flat for {flat}% of the speech");
    }

    for band in 0..BAND_COUNT {
        let mean = totals[band] / voiced as f32;
        eprintln!(
            "band {band}: mean {mean:.2}, pinned {}%",
            pinned[band] * 100 / voiced
        );
        assert!(
            mean > 0.08,
            "band {band} is dead on real speech: mean {mean}"
        );
        assert!(mean < 0.6, "band {band} dominates: mean {mean}");

        let stuck = pinned[band] * 100 / voiced;
        assert!(
            stuck < 15,
            "band {band} sat at full height for {stuck}% of the speech"
        );
    }
}
