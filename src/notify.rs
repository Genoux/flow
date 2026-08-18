//! Desktop notifications for the failures the user would otherwise never see.
//!
//! Flow runs as a systemd user unit, so every `eprintln!` here goes to the
//! journal and nowhere else. That is fine for the running commentary - a line
//! per dictation, timings, what was skipped and why - because nobody is meant
//! to read it. It is not fine for the handful of failures where the user is
//! left holding nothing: the chord did something, no text arrived, and the only
//! explanation is in a log they have no reason to open.
//!
//! So this is deliberately not a logging layer. It is reserved for the cases
//! where the answer to "why did nothing happen" is actionable, and every call
//! site is one the user has to do something about.

use std::process::{Command, Stdio};

/// Always `critical`: without exception these are states where dictation is
/// broken or a dictation was lost.
///
/// Whether that actually pins the notification until it is dismissed is the
/// bar's decision, not ours - a server only honours it if it advertises the
/// `persistence` capability, and quickshell (the one on this machine) does
/// not. So urgency is a hint here, not a guarantee, and the body text has to
/// carry the whole message on its own.
const URGENCY: &str = "critical";

/// Tell the user something went wrong, and always say the same thing to the
/// journal.
///
/// Best effort by design: `notify-send` may be absent, and there may be no
/// notification daemon running at all. Neither is worth an error path - the
/// journal line has already been written by the time either could fail, and a
/// failed notification must never take down the dictation that triggered it.
pub fn failure(summary: &str, body: &str) {
    eprintln!("{summary}: {body}");
    send(summary, body);
}

fn send(summary: &str, body: &str) {
    let summary = summary.to_string();
    let body = body.to_string();

    // On its own thread, and waited on there. Two reasons, and both are about
    // the daemon outliving the notification: `notify-send` with critical
    // urgency does not return until the notification is dismissed on some
    // bars, so waiting inline would hang the dictation behind a popup about
    // the dictation - and spawning without ever waiting would leave a zombie
    // per failure in a process that runs for weeks.
    std::thread::spawn(move || {
        let finished = Command::new("notify-send")
            .args([
                "--app-name=flow",
                &format!("--urgency={URGENCY}"),
                "--icon=audio-input-microphone",
                &summary,
                &body,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();

        if let Err(err) = finished {
            eprintln!("notification not shown ({err}) - is notify-send installed?");
        }
    });
}
