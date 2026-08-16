//! The parts of the machine the window reports on or changes: whether Flow
//! starts with the session, and which microphone PipeWire is actually handing
//! it.
//!
//! Everything here shells out to the tools that own this state - `systemctl`
//! and `pactl` - rather than keeping a copy of it. A settings window that
//! remembers what it set is a window that disagrees with the system the moment
//! anything else touches it.

use std::process::Command;
use std::time::Duration;

/// Nothing here is on the dictation path, but a hung `systemctl` would freeze
/// the UI thread just as effectively.
const BUDGET: Duration = Duration::from_secs(3);

fn run(program: &str, args: &[&str]) -> Option<std::process::Output> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .ok()?;

    // wait_timeout is not in std, so poll: these commands return in
    // milliseconds, and the alternative is a window that can hang.
    let deadline = std::time::Instant::now() + BUDGET;
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if std::time::Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(20));
            }
            Ok(None) => {
                let _ = child.kill();
                return None;
            }
            Err(_) => return None,
        }
    }
}

/// Whether the user unit is enabled, or `None` when systemd cannot answer -
/// the unit is not installed, or this is not a systemd session. `None` means
/// "do not offer this control", not "off".
pub fn autostart_enabled() -> Option<bool> {
    let output = run("systemctl", &["--user", "is-enabled", "flow.service"])?;
    let state = String::from_utf8_lossy(&output.stdout).trim().to_owned();
    match state.as_str() {
        // `linked` and `static` are enabled-ish; anything else we do not
        // understand well enough to claim a state for.
        "enabled" | "enabled-runtime" | "linked" | "linked-runtime" | "static" => Some(true),
        "disabled" => Some(false),
        _ => None,
    }
}

/// Enable or disable the user unit. Returns the error text on failure so the
/// window can show why rather than silently springing the switch back.
pub fn set_autostart(enable: bool) -> Result<(), String> {
    let verb = if enable { "enable" } else { "disable" };
    let output = run("systemctl", &["--user", verb, "flow.service"])
        .ok_or_else(|| "systemctl did not respond".to_string())?;
    if output.status.success() {
        return Ok(());
    }
    let reason = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    Err(if reason.is_empty() {
        format!("systemctl {verb} failed")
    } else {
        reason
    })
}

/// The description of the default PipeWire source, which is the microphone
/// Flow records from. Read-only on purpose: the daemon deliberately follows
/// the system default so that changing your microphone in your desktop's own
/// settings just works, and a second picker here could only ever disagree
/// with it.
pub fn default_input() -> Option<String> {
    let default = run("pactl", &["get-default-source"])?;
    let name = String::from_utf8_lossy(&default.stdout).trim().to_owned();
    if name.is_empty() {
        return None;
    }

    // Prefer the human description over the alsa_input.usb-... device id.
    let listed = run("pactl", &["list", "sources"])?;
    let text = String::from_utf8_lossy(&listed.stdout);
    Some(description_of(&text, &name).unwrap_or(name))
}

/// Pull the `Description:` belonging to the source called `name` out of
/// `pactl list sources` output. Split out so the parsing is testable without
/// PipeWire running.
fn description_of(listing: &str, name: &str) -> Option<String> {
    let mut current_is_ours = false;
    for line in listing.lines() {
        let trimmed = line.trim();
        if let Some(value) = trimmed.strip_prefix("Name: ") {
            current_is_ours = value.trim() == name;
        } else if current_is_ours {
            if let Some(value) = trimmed.strip_prefix("Description: ") {
                return Some(value.trim().to_owned());
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    const LISTING: &str = "\
Source #184594
        State: RUNNING
        Name: alsa_input.usb-webcam.iec958-stereo
        Description: Full HD webcam Digital Stereo
Source #184596
        State: SUSPENDED
        Name: alsa_input.platform-snd_aloop.0.analog-stereo
        Description: Loopback Analog Stereo
";

    #[test]
    fn the_description_matches_the_named_source() {
        assert_eq!(
            description_of(LISTING, "alsa_input.usb-webcam.iec958-stereo").as_deref(),
            Some("Full HD webcam Digital Stereo")
        );
        assert_eq!(
            description_of(LISTING, "alsa_input.platform-snd_aloop.0.analog-stereo").as_deref(),
            Some("Loopback Analog Stereo")
        );
    }

    /// A name that is not in the listing must not borrow the next source's
    /// description - that would label the microphone as something it is not.
    #[test]
    fn an_unknown_source_has_no_description() {
        assert_eq!(description_of(LISTING, "alsa_input.nonexistent"), None);
        assert_eq!(description_of("", "anything"), None);
    }
}
