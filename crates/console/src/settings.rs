//! Reading and writing `~/.config/flow/config.toml`.
//!
//! The daemon's parser is flat `key = value` with `#` comments, and this writes
//! the same shape. Two rules it follows that a naive serialiser would not:
//!
//! * **Never rewrite the whole file.** A key we manage is edited in place; a
//!   key we do not recognise, every comment, and the blank lines between them
//!   are all left exactly as they were. The template Flow ships is mostly
//!   commented explanation, and losing it because a toggle moved would be a
//!   poor trade.
//! * **A commented-out default counts as absent.** `# duck = 50` is the
//!   template showing you what the default is, not a setting. Writing `duck`
//!   appends a real line rather than uncommenting that one, so the explanation
//!   survives.

use std::path::PathBuf;

pub fn config_path() -> PathBuf {
    let home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/".into())).join(".config")
        });
    home.join("flow/config.toml")
}

/// The subset of Flow's config the window can change. Anything else in the
/// file is passed through untouched.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub push_to_talk: bool,
    pub cleanup: bool,
    pub terminal: bool,
    pub denoise: bool,
    pub duck: u32,
    pub duck_settle_ms: u64,
}

impl Default for Settings {
    /// Matches the daemon's own defaults in src/config.rs. They have to agree:
    /// a file with no `duck` line means 50 to the daemon, so the window must
    /// show 50 rather than 0.
    fn default() -> Self {
        Self {
            push_to_talk: true,
            cleanup: true,
            terminal: false,
            denoise: false,
            duck: 50,
            duck_settle_ms: 150,
        }
    }
}

impl Settings {
    pub fn load() -> Self {
        std::fs::read_to_string(config_path())
            .map(|text| Self::parse(&text))
            .unwrap_or_default()
    }

    /// A missing or malformed value keeps the default rather than failing: the
    /// window is not the right place to refuse to open over a typo, and the
    /// daemon already reports config errors properly on startup.
    pub fn parse(text: &str) -> Self {
        let mut settings = Self::default();
        for (key, value) in pairs(text) {
            match key.as_str() {
                "push_to_talk" => settings.push_to_talk = value == "true",
                "cleanup" => settings.cleanup = value == "true",
                "terminal" => settings.terminal = value == "true",
                "denoise" => settings.denoise = value == "true",
                "duck" => {
                    if let Ok(parsed) = value.parse() {
                        settings.duck = parsed;
                    }
                }
                "duck_settle_ms" => {
                    if let Ok(parsed) = value.parse() {
                        settings.duck_settle_ms = parsed;
                    }
                }
                _ => {}
            }
        }
        settings
    }

    pub fn save(&self) -> std::io::Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let existing = std::fs::read_to_string(&path).unwrap_or_default();
        std::fs::write(&path, self.render(&existing))
    }

    /// Apply our values onto `existing`, editing the lines that set a key we
    /// manage and appending the ones that were never there.
    fn render(&self, existing: &str) -> String {
        let wanted = [
            ("push_to_talk", self.push_to_talk.to_string()),
            ("cleanup", self.cleanup.to_string()),
            ("terminal", self.terminal.to_string()),
            ("denoise", self.denoise.to_string()),
            ("duck", self.duck.to_string()),
            ("duck_settle_ms", self.duck_settle_ms.to_string()),
        ];

        let mut lines: Vec<String> = existing.lines().map(str::to_owned).collect();
        let mut written = Vec::new();

        for line in lines.iter_mut() {
            let Some(key) = setting_key(line) else { continue };
            if let Some((name, value)) = wanted.iter().find(|(name, _)| *name == key) {
                *line = format!("{name} = {value}");
                written.push(*name);
            }
        }

        let missing: Vec<_> = wanted
            .iter()
            .filter(|(name, _)| !written.contains(name))
            .collect();

        if !missing.is_empty() {
            if !lines.is_empty() && !lines.last().is_some_and(|l| l.trim().is_empty()) {
                lines.push(String::new());
            }
            for (name, value) in missing {
                lines.push(format!("{name} = {value}"));
            }
        }

        let mut out = lines.join("\n");
        out.push('\n');
        out
    }
}

/// Key/value pairs from live (uncommented) lines only.
fn pairs(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|line| {
            let key = setting_key(line)?;
            let value = line.split_once('=')?.1;
            // Trailing comments are legal in the daemon's parser.
            let value = value.split_once('#').map_or(value, |(before, _)| before);
            Some((key, value.trim().to_owned()))
        })
        .collect()
}

/// The key a line sets, or `None` for blanks, comments and anything without a
/// `=`. A commented line is deliberately not a setting - see the module note.
fn setting_key(line: &str) -> Option<String> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let (key, _) = trimmed.split_once('=')?;
    Some(key.trim().to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commented_defaults_are_not_settings() {
        // The shipped template is almost entirely commented explanation.
        let template = "# duck = 50\n# cleanup = true\n";
        let parsed = Settings::parse(template);
        assert_eq!(parsed, Settings::default());

        // Writing must append rather than uncomment, so the explanation lives.
        let out = Settings::default().render(template);
        assert!(out.contains("# duck = 50"), "comment was lost:\n{out}");
        assert!(out.contains("\nduck = 50"), "no real setting written:\n{out}");
    }

    #[test]
    fn existing_keys_are_edited_in_place_and_others_untouched() {
        let existing = "# keep me\nduck = 20\ngpu = 1\n";
        let settings = Settings {
            duck: 75,
            ..Settings::default()
        };
        let out = settings.render(existing);

        assert!(out.contains("# keep me"));
        assert!(out.contains("duck = 75"), "duck not updated:\n{out}");
        assert!(!out.contains("duck = 20"));
        // A key the window does not manage must survive a save.
        assert!(out.contains("gpu = 1"), "unknown key dropped:\n{out}");
    }

    #[test]
    fn a_saved_file_reads_back_the_same() {
        let settings = Settings {
            push_to_talk: false,
            cleanup: false,
            terminal: true,
            denoise: true,
            duck: 0,
            duck_settle_ms: 400,
        };
        assert_eq!(Settings::parse(&settings.render("")), settings);
    }

    #[test]
    fn trailing_comments_parse() {
        assert_eq!(Settings::parse("duck = 30 # quieter\n").duck, 30);
    }
}
