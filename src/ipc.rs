use anyhow::{anyhow, Context, Result};
use std::path::PathBuf;

/// Start and stop are separate signals rather than one toggle so they are
/// idempotent: a missed or doubled press cannot leave the daemon out of sync
/// with the key, which is how toggle strands a microphone open.
pub const START: libc::c_int = signal_hook::consts::SIGUSR1;
pub const STOP: libc::c_int = signal_hook::consts::SIGUSR2;

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
