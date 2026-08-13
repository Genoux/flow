//! Searches BAND_GAIN and the dB window for values that satisfy every constraint
//! in tests/spectrum.rs at once, from measured per-frame amplitudes.
//!   cargo test --release --test calibrate -- --ignored --nocapture
use flow::overlay::{Analyzer, BAND_COUNT, WINDOW};

fn frames() -> Vec<[f32; BAND_COUNT]> {
    let samples = flow::wav::read_16k_mono("tests/fixtures/jfk.wav").expect("fixture");
    let mut analyzer = Analyzer::new();
    let mut out = Vec::new();
    let mut at = WINDOW;
    while at <= samples.len() {
        let amps = analyzer.amplitudes(&samples[..at]);
        if amps.iter().cloned().fold(0.0f32, f32::max) > 0.0005 {
            out.push(amps);
        }
        at += 256;
    }
    out
}

fn score(
    frames: &[[f32; BAND_COUNT]],
    gains: [f32; BAND_COUNT],
    floor: f32,
    ceiling: f32,
) -> Option<(Vec<f32>, f32)> {
    let mut sums = [0.0f32; BAND_COUNT];
    let mut flat = [0u32; BAND_COUNT];
    let mut pinned = [0u32; BAND_COUNT];

    for amps in frames {
        for band in 0..BAND_COUNT {
            let a = amps[band] * gains[band];
            let height = if a <= 0.0 {
                0.0
            } else {
                ((20.0 * a.log10() - floor) / (ceiling - floor)).clamp(0.0, 1.0)
            };
            sums[band] += height;
            if height < 0.02 {
                flat[band] += 1;
            }
            if height >= 0.999 {
                pinned[band] += 1;
            }
        }
    }

    let n = frames.len() as f32;
    let means: Vec<f32> = (0..BAND_COUNT).map(|b| sums[b] / n).collect();
    if !means.windows(2).all(|p| p[0] > p[1]) {
        return None;
    }
    for band in 0..BAND_COUNT {
        if means[band] <= 0.10 || means[band] >= 0.55 {
            return None;
        }
        if flat[band] * 100 / frames.len() as u32 >= 8 {
            return None;
        }
        if pinned[band] * 100 / frames.len() as u32 >= 12 {
            return None;
        }
    }
    // The complaint being fixed is bars that do not move, so movement is what
    // this maximises: the average per-band spread of height across the sentence.
    // A window wide enough to satisfy every threshold can still compress the
    // bars into a stationary row, which satisfies the tests and looks dead.
    let mut motion = 0.0;
    for band in 0..BAND_COUNT {
        let mean = means[band];
        let variance = frames
            .iter()
            .map(|amps| {
                let a = amps[band] * gains[band];
                let height = if a <= 0.0 {
                    0.0
                } else {
                    ((20.0 * a.log10() - floor) / (ceiling - floor)).clamp(0.0, 1.0)
                };
                (height - mean).powi(2)
            })
            .sum::<f32>()
            / n;
        motion += variance.sqrt();
    }
    Some((means, motion / BAND_COUNT as f32))
}

#[test]
#[ignore]
fn search() {
    let frames = frames();
    eprintln!("{} voiced frames\n", frames.len());

    let mut best: Option<([f32; BAND_COUNT], f32, f32, Vec<f32>, f32)> = None;
    for floor in [-96.0, -90.0, -84.0, -78.0, -72.0] {
        for ceiling in [-30.0, -24.0, -18.0, -12.0, -6.0] {
            if ceiling - floor < 40.0 {
                continue;
            }
            for g0 in [0.5, 0.7, 0.9, 1.1, 1.4] {
                for g1 in [0.15, 0.25, 0.35, 0.5, 0.7] {
                    for g2 in [0.4, 0.7, 1.0, 1.4, 2.0] {
                        for g3 in [2.0, 3.0, 4.5, 6.0, 9.0] {
                            let gains = [g0, g1, g2, g3];
                            if let Some((means, fitness)) = score(&frames, gains, floor, ceiling)
                                && best.as_ref().is_none_or(|(_, _, _, _, b)| fitness > *b)
                            {
                                best = Some((gains, floor, ceiling, means, fitness));
                            }
                        }
                    }
                }
            }
        }
    }

    match best {
        Some((gains, floor, ceiling, means, _)) => {
            eprintln!("BAND_GAIN  = {gains:?}");
            eprintln!("FLOOR_DB   = {floor}");
            eprintln!("CEILING_DB = {ceiling}");
            eprintln!("means      = {means:?}");
        }
        None => eprintln!("nothing satisfies the constraints - widen the search"),
    }
}
