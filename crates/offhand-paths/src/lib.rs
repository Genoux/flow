//! Every path Offhand touches, derived in one place.
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

pub mod launchd;
pub mod process;

use std::path::PathBuf;

/// `$XDG_RUNTIME_DIR`, falling back to a private per-user temporary directory.
pub fn runtime_dir() -> PathBuf {
    usable_runtime(std::env::var_os("XDG_RUNTIME_DIR")).unwrap_or_else(|| {
        unsafe extern "C" {
            fn geteuid() -> u32;
        }
        // POSIX geteuid has no preconditions and cannot fail.
        let uid = unsafe { geteuid() };
        let path = std::env::temp_dir().join(format!("offhand-{uid}"));
        private_runtime(&path, uid)
            .expect("offhand runtime directory must be private and owned by this user");
        path
    })
}

/// The XDG spec has applications fall back when the variable is set but names
/// no directory, rather than create it: it is the session manager's to make.
fn usable_runtime(variable: Option<std::ffi::OsString>) -> Option<PathBuf> {
    variable.map(PathBuf::from).filter(|path| path.is_dir())
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
        .expect("HOME is not set - offhand needs it to find its config and models")
}

/// Offhand was called Flow through 0.6.1, and an install from then keeps its
/// config, vocabulary, history and 2.6GB speech model under `flow/`. They are
/// renamed rather than copied, and only into a place that is still empty: when
/// both exist there is no telling which one the user means, so neither is
/// touched.
pub fn adopt_flow_directories(
    config_home: &std::path::Path,
    data_home: &std::path::Path,
) -> std::io::Result<()> {
    // Both are attempted before either failure is reported: a config that
    // could not move must not leave the speech model behind as well.
    [config_home, data_home]
        .map(|base| {
            let (former, current) = (base.join("flow"), base.join("offhand"));
            if current.exists() || !former.exists() {
                return Ok(());
            }
            // The daemon and the tray start together and both get here. The
            // one that loses the race finds the old folder already gone, which
            // is the outcome it wanted.
            match std::fs::rename(&former, &current) {
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && current.exists() => {
                    Ok(())
                }
                outcome => outcome,
            }
        })
        .into_iter()
        .collect()
}

/// `~/.config/offhand/config.toml`
pub fn config_file() -> PathBuf {
    config_home().join("offhand/config.toml")
}

/// `~/.config/offhand/vocabulary.txt`
pub fn vocabulary_file() -> PathBuf {
    config_home().join("offhand/vocabulary.txt")
}

/// `~/.local/share/offhand`
pub fn data_dir() -> PathBuf {
    data_home().join("offhand")
}

/// `~/.local/share/offhand/history.jsonl`
pub fn history_file() -> PathBuf {
    data_dir().join("history.jsonl")
}

/// Root for everything `offhand install` downloads.
pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

/// Where `offhand install` puts the speech model.
pub fn speech_model_dir() -> PathBuf {
    models_dir().join("nemotron")
}

/// Debug WAVs, written only when `record_debug` is on.
pub fn recordings_dir() -> PathBuf {
    data_dir().join("recordings")
}

/// The daemon's status socket, which the console connects to.
pub fn socket() -> PathBuf {
    runtime_dir().join("offhand.sock")
}

/// Written by the daemon so `offhand start` / `offhand stop` can signal it.
pub fn pid_file() -> PathBuf {
    runtime_dir().join("offhand.pid")
}

/// Volumes saved before ducking, so a crash mid-recording can still restore
/// them on the next run.
pub fn duck_state_file() -> PathBuf {
    runtime_dir().join("offhand-duck.json")
}

/// The LaunchAgent's stdout and stderr, which is where `offhand logs` reads on
/// macOS. `~/Library/Logs` is where Console.app already looks.
#[cfg(target_os = "macos")]
pub fn log_file() -> PathBuf {
    home().join("Library/Logs/Offhand/offhand.log")
}

/// Where `offhand` (daemon, tray and CLI) sits inside Offhand.app. It has its own
/// bundle so that a running daemon is not mistaken for Offhand.app itself.
pub const AGENT_PROGRAM: &str = "Contents/Helpers/Offhand Agent.app/Contents/MacOS/offhand";

/// The Offhand.app that `exe` runs from: the outermost `.app`, since the helper
/// bundle is itself an `.app` nested inside it.
pub fn app_bundle(exe: &std::path::Path) -> Option<PathBuf> {
    exe.ancestors()
        .filter(|path| path.extension().is_some_and(|ext| ext == "app"))
        .last()
        .map(std::path::Path::to_path_buf)
}

/// The helper's `offhand`, from any executable inside Offhand.app.
pub fn agent_program(exe: &std::path::Path) -> Option<PathBuf> {
    app_bundle(exe).map(|bundle| bundle.join(AGENT_PROGRAM))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_app_is_the_outermost_bundle_from_either_binary() {
        let app = PathBuf::from("/Users/me/Applications/Offhand.app");
        for exe in [
            app.join("Contents/MacOS/offhand-console"),
            app.join(AGENT_PROGRAM),
        ] {
            assert_eq!(app_bundle(&exe), Some(app.clone()));
            assert_eq!(agent_program(&exe), Some(app.join(AGENT_PROGRAM)));
        }
    }

    #[test]
    fn a_binary_outside_a_bundle_has_no_app() {
        let exe = std::path::Path::new("/usr/local/bin/offhand");
        assert_eq!(app_bundle(exe), None);
        assert_eq!(agent_program(exe), None);
    }

    #[test]
    fn fallback_rejects_shared_foreign_and_symlink_directories() {
        use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};

        let root =
            std::env::temp_dir().join(format!("offhand-runtime-test-{}", std::process::id()));
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

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("offhand-{name}-{}", std::process::id()));
        std::fs::create_dir(&root).unwrap();
        root
    }

    #[test]
    fn a_flow_install_is_carried_over_with_its_contents() {
        let root = scratch("carry-over");
        let (config, data) = (root.join("config"), root.join("data"));
        std::fs::create_dir_all(config.join("flow")).unwrap();
        std::fs::create_dir_all(data.join("flow/models/nemotron")).unwrap();
        std::fs::write(config.join("flow/config.toml"), "cleanup = \"light\"\n").unwrap();
        std::fs::write(data.join("flow/models/nemotron/encoder.onnx"), "weights").unwrap();

        adopt_flow_directories(&config, &data).unwrap();

        assert!(!config.join("flow").exists());
        assert!(!data.join("flow").exists());
        assert_eq!(
            std::fs::read_to_string(config.join("offhand/config.toml")).unwrap(),
            "cleanup = \"light\"\n"
        );
        assert_eq!(
            std::fs::read_to_string(data.join("offhand/models/nemotron/encoder.onnx")).unwrap(),
            "weights"
        );
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn an_existing_offhand_directory_is_never_replaced() {
        let root = scratch("both-present");
        let (config, data) = (root.join("config"), root.join("data"));
        for (base, name, text) in [
            (&config, "flow", "old config"),
            (&config, "offhand", "new config"),
            (&data, "flow", "old data"),
            (&data, "offhand", "new data"),
        ] {
            std::fs::create_dir_all(base.join(name)).unwrap();
            std::fs::write(base.join(name).join("file"), text).unwrap();
        }

        adopt_flow_directories(&config, &data).unwrap();

        for (base, name, text) in [
            (&config, "flow", "old config"),
            (&config, "offhand", "new config"),
            (&data, "flow", "old data"),
            (&data, "offhand", "new data"),
        ] {
            assert_eq!(
                std::fs::read_to_string(base.join(name).join("file")).unwrap(),
                text
            );
        }
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn nothing_to_carry_over_creates_nothing() {
        let root = scratch("neither-present");
        let (config, data) = (root.join("config"), root.join("data"));
        std::fs::create_dir(&config).unwrap();

        adopt_flow_directories(&config, &data).unwrap();

        assert_eq!(std::fs::read_dir(&config).unwrap().count(), 0);
        assert!(!data.exists());
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
            PathBuf::from("/custom/config/offhand/config.toml")
        );
        unsafe { std::env::remove_var("XDG_CONFIG_HOME") };
    }

    /// Dotfiles shared with Linux export `/run/user/<uid>` on macOS, where it
    /// never exists, and the daemon then failed to write its pid file.
    #[test]
    fn a_runtime_variable_naming_no_directory_is_ignored() {
        let existing = std::env::temp_dir();
        assert_eq!(
            usable_runtime(Some(existing.clone().into_os_string())),
            Some(existing)
        );
        assert_eq!(usable_runtime(Some("/run/user/nonexistent".into())), None);
        assert_eq!(usable_runtime(Some("".into())), None);
        assert_eq!(usable_runtime(None), None);
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
