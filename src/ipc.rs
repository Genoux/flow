use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;

/// SIGUSR1 toggles recording. A signal rather than a socket because the only
/// message we need is "toggle", and this keeps `flow toggle` dependency-free.
pub const TOGGLE: libc::c_int = signal_hook::consts::SIGUSR1;

pub fn pid_file() -> PathBuf {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    runtime.join("flow.pid")
}

pub fn write_pid() -> Result<()> {
    std::fs::write(pid_file(), std::process::id().to_string()).context("writing pid file")
}

pub fn remove_pid() {
    let _ = std::fs::remove_file(pid_file());
}

/// Signal the running daemon to toggle recording.
pub fn send_toggle() -> Result<()> {
    let path = pid_file();
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("no daemon running? {} missing", path.display()))?;
    let pid: i32 = raw.trim().parse().context("malformed pid file")?;

    // SAFETY: kill(2) with a parsed pid; failure is reported, not ignored.
    if unsafe { libc::kill(pid, TOGGLE) } != 0 {
        remove_pid();
        return Err(anyhow!(
            "daemon {pid} not reachable ({}); stale pid file removed",
            std::io::Error::last_os_error()
        ));
    }
    Ok(())
}
