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

/// `$XDG_RUNTIME_DIR`, falling back to the temp dir.
///
/// The fallback is real rather than defensive: `XDG_RUNTIME_DIR` is genuinely
/// absent over plain `ssh` without a login session, and everything under here
/// (socket, pid, duck state) is transient enough that the temp dir is a correct
/// answer rather than a damage-limiting one.
pub fn runtime_dir() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
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

    /// The regression this crate exists to prevent: the console used to answer
    /// this question differently from the daemon, so it wrote settings the
    /// daemon never read.
    #[test]
    fn xdg_variables_win_over_home() {
        // SAFETY: single-threaded test, and the value is restored below.
        unsafe { std::env::set_var("XDG_CONFIG_HOME", "/custom/config") };
        assert_eq!(config_file(), PathBuf::from("/custom/config/flow/config.toml"));
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
