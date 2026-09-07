//! Every path Flow touches, derived in one place.
//!
//! This exists because the daemon and the console are separate crates that have
//! to agree. The console writes the config file the daemon reads, watches the
//! socket the daemon serves, and lists the history the daemon appends to - so a
//! disagreement about where any of those live is not untidiness, it is the
//! console editing a file nobody opens.
//!
//! They had already drifted before this crate: asked for the config directory
//! with `HOME` unset, the daemon panicked and the console silently answered
//! `/.config`. Same question, two answers, neither of them written down.
//!
//! Deliberately dependency-free. The console is its own workspace so that
//! iced's tree and llama.cpp's tree never meet, and this crate sits under both.

use std::path::PathBuf;

/// `$XDG_RUNTIME_DIR`, falling back to a private per-user temporary directory.
pub fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            unsafe extern "C" {
                fn geteuid() -> u32;
            }
            // POSIX geteuid has no preconditions and cannot fail.
            let uid = unsafe { geteuid() };
            let path = std::env::temp_dir().join(format!("flow-{uid}"));
            private_runtime(&path, uid)
                .expect("flow runtime directory must be private and owned by this user");
            path
        })
}

fn private_runtime(path: &std::path::Path, uid: u32) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, MetadataExt};

    match std::fs::DirBuilder::new().mode(0o700).create(path) {
        Ok(()) => {}
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
        Err(err) => return Err(err),
    }
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_dir() || metadata.uid() != uid || metadata.mode() & 0o077 != 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "insecure runtime directory",
        ));
    }
    Ok(())
}

/// `$XDG_CONFIG_HOME`, falling back to `~/.config`.
pub fn config_home() -> PathBuf {
    xdg_home("XDG_CONFIG_HOME", ".config")
}

/// `$XDG_DATA_HOME`, falling back to `~/.local/share`.
pub fn data_home() -> PathBuf {
    xdg_home("XDG_DATA_HOME", ".local/share")
}

/// Panics when `HOME` is unset, and that is the intended behaviour.
///
/// There is no correct path to return: the two prior guesses were a panic and
/// `/`, and `/` is worse - it turns a broken environment into a config file
/// written somewhere nobody will look. A user-session daemon and a desktop app
/// both always have `HOME`; if they do not, the environment is wrong and
/// saying so beats carrying on.
fn xdg_home(variable: &str, fallback: &str) -> PathBuf {
    std::env::var_os(variable)
        .map(PathBuf::from)
        .unwrap_or_else(|| home().join(fallback))
}

fn home() -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .expect("HOME is not set - flow needs it to find its config and models")
}

/// `~/.config/flow/config.toml`
pub fn config_file() -> PathBuf {
    config_home().join("flow/config.toml")
}

/// `~/.config/flow/vocabulary.txt`
pub fn vocabulary_file() -> PathBuf {
    config_home().join("flow/vocabulary.txt")
}

/// `~/.config/flow/instructions.txt`
pub fn instructions_file() -> PathBuf {
    config_home().join("flow/instructions.txt")
}

/// `~/.local/share/flow`
pub fn data_dir() -> PathBuf {
    data_home().join("flow")
}

/// `~/.local/share/flow/history.jsonl`
pub fn history_file() -> PathBuf {
    data_dir().join("history.jsonl")
}

/// Root for everything `flow install` downloads.
pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

/// Where `flow install` puts the speech model.
pub fn speech_model_dir() -> PathBuf {
    models_dir().join("tdt")
}

/// The refining model, as a single gguf file rather than a directory.
///
/// The filename names the quantisation on purpose: swapping the model means
/// changing this, which is the point at which someone has to notice that
/// `install.rs` pins a matching sha256.
pub fn refine_model_file() -> PathBuf {
    models_dir().join("qwen3-4b-instruct-q4km.gguf")
}

/// Debug WAVs, written only when `record_debug` is on.
pub fn recordings_dir() -> PathBuf {
    data_dir().join("recordings")
}

/// The daemon's status socket, which the console connects to.
pub fn socket() -> PathBuf {
    runtime_dir().join("flow.sock")
}

/// Written by the daemon so `flow start` / `flow stop` can signal it.
pub fn pid_file() -> PathBuf {
    runtime_dir().join("flow.pid")
}

/// Volumes saved before ducking, so a crash mid-recording can still restore
/// them on the next run.
pub fn duck_state_file() -> PathBuf {
    runtime_dir().join("flow-duck.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fallback_rejects_shared_foreign_and_symlink_directories() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

        let root = std::env::temp_dir().join(format!("flow-runtime-test-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        let uid = std::fs::metadata(&root).unwrap().uid();
        let private = root.join("private");
        private_runtime(&private, uid).unwrap();
        private_runtime(&private, uid).unwrap();
        assert!(private_runtime(&private, uid.wrapping_add(1)).is_err());
        let link = root.join("link");
        symlink(&private, &link).unwrap();
        assert!(private_runtime(&link, uid).is_err());
        std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(private_runtime(&private, uid).is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    /// The regression this crate exists to prevent: the console used to answer
    /// this question differently from the daemon, so it wrote settings the
    /// daemon never read.
    #[test]
    fn xdg_variables_win_over_home() {
        // SAFETY: single-threaded test, and the value is restored below.
        unsafe { std::env::set_var("XDG_CONFIG_HOME", "/custom/config") };
        assert_eq!(
            config_file(),
            PathBuf::from("/custom/config/flow/config.toml")
        );
        unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
    }

    #[test]
    fn runtime_files_share_one_directory() {
        let runtime = runtime_dir();
        for path in [socket(), pid_file(), duck_state_file()] {
            assert_eq!(path.parent(), Some(runtime.as_path()));
        }
    }

    #[test]
    fn config_and_data_are_not_the_same_tree() {
        // Catches a copy-paste that points the models at the config dir, which
        // would put 2.4GB of weights into a directory people sync between
        // machines.
        assert_ne!(config_home(), data_home());
        assert!(speech_model_dir().starts_with(data_dir()));
        assert!(vocabulary_file().starts_with(config_home()));
    }
}
