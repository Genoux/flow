//! Is "no callbacks" a safe signal for "stream is dead"?
//!
//! The reopen logic in `Capture::ensure_live` hangs entirely on that assumption.
//! If a healthy but idle capture stops delivering, the daemon would rebuild the
//! stream on every press after a pause and throw away the pre-roll each time.
//!
//! Needs a real microphone:
//!   cargo test --release --test capture_health -- --ignored --nocapture

use std::time::Duration;

#[test]
#[ignore]
fn an_idle_capture_keeps_delivering() {
    let device = flow::audio::open_device().expect("device");
    let capture = flow::audio::Capture::open(&device).expect("open");

    let mut worst = Duration::ZERO;
    for second in 1..=8 {
        std::thread::sleep(Duration::from_secs(1));
        let gap = capture.silent_for();
        worst = worst.max(gap);
        eprintln!("  t+{second}s idle: last callback {gap:?} ago");
    }

    eprintln!("worst idle gap: {worst:?}");
    assert!(
        worst < Duration::from_secs(3),
        "an idle healthy capture went {worst:?} without a callback, so the \
         3s reopen threshold would fire on a working microphone"
    );
}

/// Dropping the stream is the closest thing to a source disappearing that can be
/// staged without unplugging hardware: callbacks stop, and nothing errors.
#[test]
#[ignore]
fn a_dead_stream_is_detected_and_reopened() {
    let device = flow::audio::open_device().expect("device");
    let capture = flow::audio::Capture::open(&device).expect("open");

    std::thread::sleep(Duration::from_millis(300));
    assert!(
        capture.silent_for() < Duration::from_secs(1),
        "should be live"
    );

    capture.kill_stream_for_test();
    std::thread::sleep(Duration::from_millis(3_200));
    let stale = capture.silent_for();
    assert!(
        stale >= Duration::from_secs(3),
        "stream still feeding? {stale:?}"
    );

    assert!(capture.ensure_live(), "reopen failed");
    std::thread::sleep(Duration::from_millis(300));
    let after = capture.silent_for();
    assert!(
        after < Duration::from_secs(1),
        "callbacks did not resume after reopen: {after:?}"
    );
    eprintln!("dead stream detected at {stale:?}, live again at {after:?}");
}
