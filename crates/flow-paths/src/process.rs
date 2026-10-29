//! Running a program with a deadline.
//!
//! Shared because a hung `launchctl` or `systemctl` is as bad in the menu bar
//! as it is in the window: it freezes whoever is waiting on it. Both ask with
//! the same budget.

use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

/// Nothing here is on the dictation path, but a hung `systemctl` would freeze
/// the UI thread just as effectively.
pub const BUDGET: Duration = Duration::from_secs(3);

pub fn run_for(program: &str, args: &[&str], budget: Duration) -> Option<Output> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;

    let stdout = drain(child.stdout.take()?);
    let stderr = drain(child.stderr.take()?);
    let deadline = Instant::now() + budget;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Some(Output {
                    status,
                    stdout: stdout
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                        .ok()??,
                    stderr: stderr
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                        .ok()??,
                });
            }
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(5));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn drain(
    mut pipe: impl std::io::Read + Send + 'static,
) -> std::sync::mpsc::Receiver<Option<Vec<u8>>> {
    let (sender, receiver) = std::sync::mpsc::channel();
    // PipeWire listings can exceed a pipe buffer; drain while the child runs.
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let output = pipe.read_to_end(&mut bytes).ok().map(|_| bytes);
        let _ = sender.send(output);
    });
    receiver
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A process that never answers is the failure behind the three-second
    /// tray opening. The launch path must kill and reap it on its smaller
    /// deadline instead of inheriting the budget meant for button presses.
    #[test]
    fn a_stalled_launch_probe_is_cut_short() {
        let started = std::time::Instant::now();
        let answer = run_for(
            "sh",
            &["-c", "while :; do :; done"],
            Duration::from_millis(25),
        );

        assert!(answer.is_none());
        assert!(
            started.elapsed() < Duration::from_millis(500),
            "a launch probe outlived its deadline by too much"
        );
    }

    #[test]
    fn command_output_can_exceed_pipe_capacity() {
        let output = run_for(
            "sh",
            &[
                "-c",
                "head -c 131072 /dev/zero; head -c 131072 /dev/zero >&2",
            ],
            Duration::from_secs(2),
        )
        .expect("both output pipes must drain before the deadline");
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 131072);
        assert_eq!(output.stderr.len(), 131072);
    }
}
