use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;

/// `flow start` / `flow stop` signal the daemon. Start alone is enough for
/// hold-to-talk: the daemon watches the physical chord and stops on release.
/// An explicit stop remains safe if nothing is recording.
pub const START: libc::c_int = signal_hook::consts::SIGUSR1;
pub const STOP: libc::c_int = signal_hook::consts::SIGUSR2;

pub fn pid_file() -> PathBuf {
    flow_paths::pid_file()
}

pub fn write_pid() -> Result<()> {
    std::fs::write(pid_file(), std::process::id().to_string()).context("writing pid file")
}

pub fn remove_pid() {
    let _ = std::fs::remove_file(pid_file());
}

pub fn send(signal: libc::c_int) -> Result<()> {
    let path = pid_file();
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("no daemon running? {} missing", path.display()))?;
    let pid: i32 = raw.trim().parse().context("malformed pid file")?;

    // SAFETY: kill(2) with a parsed pid; failure is reported, not ignored.
    if unsafe { libc::kill(pid, signal) } != 0 {
        remove_pid();
        return Err(anyhow!(
            "daemon {pid} not reachable ({}); stale pid file removed",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}
