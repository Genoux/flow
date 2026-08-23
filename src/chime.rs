//! The two-note answer to the island: one when it arrives, one when it goes.
//!
//! The sound is handed to `paplay` on stdin rather than decoded here. `pactl`
//! is already a hard dependency of ducking and `paplay` ships beside it, so a
//! daemon that has no audio *output* stack gains one for the price of a pipe -
//! no decoder crate, no second device to open against the one being recorded.

use std::io::Write;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

/// The name the chime's playback stream carries, and what ducking looks for
/// when it skips Flow's own audio - see [`crate::duck`]. Without it the sound
/// announcing a recording is faded out by the recording it announces: ducking
/// starts a few milliseconds later and sweeps for new streams every 50ms,
/// which lands inside these 230.
pub const CLIENT: &str = "flow";

const SHOW: &[u8] = include_bytes!("../assets/island-show.wav");
const HIDE: &[u8] = include_bytes!("../assets/island-hide.wav");

/// How loud, against PulseAudio's 65536 for "unchanged". A confirmation is
/// meant to sit under whatever you are listening to, not over it, and these
/// cues were mastered for a web page rather than for a daemon that fires them
/// at every dictation. The one number to turn if it still lands wrong.
const VOLUME: u32 = 22_936; // 35%

/// ponytail: process-wide because there is one daemon, one setting, and the
/// thread that plays these is the one drawing the island - which has no other
/// reason to know a config exists. A field would have to be plumbed through
/// `Overlay::spawn` and every command in between to say one bit.
static ENABLED: AtomicBool = AtomicBool::new(true);

/// Follow `config.sound`. Called as each dictation begins, so the switch in the
/// console takes effect on the next chord rather than the next restart.
pub fn set_enabled(on: bool) {
    ENABLED.store(on, Ordering::Relaxed);
}

pub fn show() {
    play(SHOW);
}

pub fn hide() {
    play(HIDE);
}

fn play(wav: &'static [u8]) {
    if !ENABLED.load(Ordering::Relaxed) {
        return;
    }

    // On its own thread because the caller is the one drawing the island, and a
    // frame owed to the compositor must never wait on an audio server.
    std::thread::spawn(move || {
        let Ok(mut player) = Command::new("paplay")
            .arg(format!("--client-name={CLIENT}"))
            .arg(format!("--volume={VOLUME}"))
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        else {
            // No paplay, no sound. The island was always the real feedback.
            return;
        };

        if let Some(mut stdin) = player.stdin.take() {
            let _ = stdin.write_all(wav);
        }
        let _ = player.wait();
    });
}
