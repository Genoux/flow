//! Speech swings between vowels and gaps; a room is steady. Measured per
//! recording against its own quiet baseline, so it needs no fixed level and works
//! on any microphone.
//!   cargo test --release --test ambient -- --ignored --nocapture
use std::time::Duration;

fn swing(samples: &[f32]) -> f32 {
    let mut levels: Vec<f32> = samples.chunks(512).map(flow::audio::rms).collect();
    if levels.len() < 8 {
        return 0.0;
    }
    levels.sort_by(f32::total_cmp);
    let at = |q: f32| levels[((levels.len() - 1) as f32 * q) as usize];
    at(0.95) / at(0.20).max(1e-6)
}

#[test]
#[ignore]
fn how_far_speech_swings_against_a_room() {
    let speech = flow::wav::read_16k_mono("tests/fixtures/jfk.wav").expect("fixture");
    eprintln!("jfk.wav (speech):   swing {:.1}x", swing(&speech));

    for round in 1..=3 {
        let device = flow::audio::open_device().expect("device");
        let capture = flow::audio::Capture::open(&device).expect("open");
        capture.begin();
        std::thread::sleep(Duration::from_millis(2500));
        let room = capture.end();
        eprintln!("room round {round}:      swing {:.1}x", swing(&room));
    }
}
