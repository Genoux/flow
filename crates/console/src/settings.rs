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
    flow_paths::config_file()
}

/// How much the refining model may change what you said.
///
/// A mirror of `refine::Cleanup` in the daemon, spelled out again because this
/// window is its own workspace on purpose - depending on the daemon crate would
/// drag llama.cpp and Vulkan into a settings window.
///
/// Only [`Cleanup::as_str`] and [`Cleanup::parse`] are a contract: they are the
/// spellings written to and read from the config file, so they must match the
/// daemon's. The card wording below is this window's alone. The daemon used to
/// carry a copy of it captioned "so the console never invents its own wording",
/// which no code on this side could reach across the crate boundary - it drifted
/// the first time these cards were rewritten, and has been deleted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Cleanup {
    None,
    #[default]
    Light,
    Medium,
}

impl Cleanup {
    pub const ALL: [Self; 3] = [Self::None, Self::Light, Self::Medium];

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "none" => Some(Self::None),
            "light" => Some(Self::Light),
            "medium" => Some(Self::Medium),
            // See `flow::refine::Cleanup::parse`: an existing `cleanup = hard`
            // file must still round-trip through the window as Medium rather
            // than reset the setting on open.
            "hard" => Some(Self::Medium),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Light => "light",
            Self::Medium => "medium",
        }
    }

    /// Card title, and the one line under it. Written for someone choosing,
    /// not for someone who already knows what the levels do.
    pub fn describe(self) -> (&'static str, &'static str) {
        match self {
            // "including mistakes" is the only reason anyone would not pick
            // None, so it earns its two words - Wispr's own None card keeps
            // the same clause. What went was "mistakes and all", which said it
            // in a folksier voice than the two levels beside it.
            Self::None => ("None", "Types exactly what you said, including mistakes."),
            // "nothing else" is the half that makes this the default. Fillers
            // out and grammar right is what every level above None does; not
            // touching the rest is what tells this level from that one.
            Self::Light => ("Light", "Removes stumbles and fixes grammar, nothing else."),
            Self::Medium => ("Medium", "Rewrites it to read well, in fewer words."),
        }
    }

    /// The same sentence at each level, so the cards show the difference rather
    /// than describing it. Wispr's own screen does this and it is the reason
    /// their levels are legible at a glance.
    ///
    /// These are measured, not written. The set they replaced was invented, and
    /// it was inventing the wrong thing: it showed None as lowercase and
    /// unpunctuated, which the recogniser never produces - Parakeet punctuates
    /// and capitalises, so what reaches the refiner is already sentences. That
    /// made Light look like it pastes lowercase rubbish when what it actually
    /// pastes is the middle line below, and it is the reason this screen read as
    /// a worse product than it is.
    ///
    /// Kept in step with `ADVERTISED_INPUT` in tests/refine.rs, which feeds the
    /// None line to the real model and checks the split these lines claim.
    ///
    /// Chosen because it is the shortest sentence found that shows both steps:
    /// "what we built don't work good" becomes "we built doesn't work well"
    /// between None and Light, which is the grammar fix Light is sold on, and
    /// "I think" and "you know" both go between Light and Medium, which is the
    /// rewrite Medium is sold on. A hedge is what Medium cuts most readily - it
    /// will not cut a real subject, so an example built on "me and him were
    /// thinking" showed almost no difference at the top of the dial.
    ///
    /// The middle line keeps "you know" on purpose, and it is the clearest
    /// thing on this screen: Light no longer decides that one of the speaker's
    /// own words was worthless. That decision cost a dictation its closing
    /// "what do you think", and it now belongs to the level below.
    ///
    /// Measured on both of this machine's GPUs, which do not always agree - see
    /// `FLOW_TEST_GPU` in tests/refine.rs. Re-measure rather than hand-edit.
    pub fn example(self) -> &'static str {
        match self {
            Self::None => {
                "Um, I think the thing what we built don't work good on mobile, you know."
            }
            Self::Light => "I think the thing we built doesn't work well on mobile, you know.",
            Self::Medium => "The thing we built doesn't work well on mobile.",
        }
    }
}

/// The subset of Flow's config the window can change. Anything else in the
/// file is passed through untouched.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub push_to_talk: bool,
    pub cleanup: Cleanup,
    pub denoise: bool,
    /// Play the island's arrive and leave chimes.
    pub sound: bool,
    pub duck: u32,
    /// Which GPU runs the refining model. `None` means the daemon picks, and is
    /// written as no line at all rather than a value - the daemon's default is
    /// "choose for me", and there is no number that spells that.
    pub gpu: Option<u32>,
    /// The chord held to dictate, in the daemon's spelling, e.g.
    /// "super+shift+d".
    pub hotkey: String,
    /// Which microphone to record from, as a pactl source name. `None` follows
    /// the system default and is written as no line at all, the same way `gpu`
    /// spells "choose for me".
    pub input_device: Option<String>,
}

/// The chord a fresh install dictates with, and what Reset puts back.
///
/// Must stay in step with `hotkey::Chord::default` in the daemon: the console
/// writes this string and the daemon is what parses it. `super` is the same
/// physical key as cmd, meta and win - the daemon accepts all four spellings,
/// and this is the one it writes back.
pub const DEFAULT_HOTKEY: &str = "super+shift+d";

impl Default for Settings {
    /// Matches the daemon's own defaults in src/config.rs. They have to agree:
    /// a file with no `duck` line means 50 to the daemon, so the window must
    /// show 50 rather than 0.
    fn default() -> Self {
        Self {
            push_to_talk: true,
            cleanup: Cleanup::default(),
            denoise: false,
            sound: true,
            duck: 50,
            gpu: None,
            hotkey: DEFAULT_HOTKEY.to_string(),
            input_device: None,
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
                "cleanup" => {
                    if let Some(level) = Cleanup::parse(&value) {
                        settings.cleanup = level;
                    }
                }
                // The key this replaced. Still read so a config written before
                // levels existed opens on the level it meant.
                "refine" => {
                    settings.cleanup = if value == "true" {
                        Cleanup::default()
                    } else {
                        Cleanup::None
                    }
                }
                "denoise" => settings.denoise = value == "true",
                "sound" => settings.sound = value == "true",
                "duck" => {
                    if let Ok(parsed) = value.parse() {
                        settings.duck = parsed;
                    }
                }
                "gpu" => settings.gpu = value.parse().ok(),
                "hotkey" => settings.hotkey = value.to_owned(),
                "input_device" => {
                    settings.input_device = (!value.is_empty()).then(|| value.to_owned())
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
    ///
    /// A `None` value means the key must not appear at all: the daemon reads an
    /// absent `gpu` as "choose for me", and there is no number that says that.
    fn render(&self, existing: &str) -> String {
        let wanted: [(&str, Option<String>); 9] = [
            ("push_to_talk", Some(self.push_to_talk.to_string())),
            ("cleanup", Some(self.cleanup.as_str().to_string())),
            // Deleted rather than left alone. The daemon still understands
            // `refine`, and applies keys in file order - so a stale `refine`
            // line sitting below `cleanup` would silently undo the level the
            // user just picked.
            ("refine", None),
            ("denoise", Some(self.denoise.to_string())),
            ("sound", Some(self.sound.to_string())),
            ("duck", Some(self.duck.to_string())),
            ("gpu", self.gpu.map(|index| index.to_string())),
            ("hotkey", Some(self.hotkey.clone())),
            ("input_device", self.input_device.clone()),
        ];

        let mut lines: Vec<String> = existing.lines().map(str::to_owned).collect();
        let mut written = Vec::new();

        // Edit in place, and drop the line entirely for a key that should now
        // be absent.
        lines.retain_mut(|line| {
            let Some(key) = setting_key(line) else {
                return true;
            };
            match wanted.iter().find(|(name, _)| *name == key) {
                Some((name, Some(value))) => {
                    *line = format!("{name} = {value}");
                    written.push(*name);
                    true
                }
                Some((_, None)) => false,
                None => true,
            }
        });

        let missing: Vec<_> = wanted
            .iter()
            .filter_map(|(name, value)| value.as_ref().map(|value| (name, value)))
            .filter(|(name, _)| !written.contains(*name))
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
        let template = "# duck = 50\n# refine = true\n";
        let parsed = Settings::parse(template);
        assert_eq!(parsed, Settings::default());

        // Writing must append rather than uncomment, so the explanation lives.
        let out = Settings::default().render(template);
        assert!(out.contains("# duck = 50"), "comment was lost:\n{out}");
        assert!(
            out.contains("\nduck = 50"),
            "no real setting written:\n{out}"
        );
    }

    #[test]
    fn existing_keys_are_edited_in_place_and_others_untouched() {
        // record_debug has no control in the window, so it stands in for any
        // key a future daemon might add that this version knows nothing about.
        let existing = "# keep me\nduck = 20\nrecord_debug = true\n";
        let settings = Settings {
            duck: 75,
            ..Settings::default()
        };
        let out = settings.render(existing);

        assert!(out.contains("# keep me"));
        assert!(out.contains("duck = 75"), "duck not updated:\n{out}");
        assert!(!out.contains("duck = 20"));
        // A key the window does not manage must survive a save.
        assert!(
            out.contains("record_debug = true"),
            "unknown key dropped:\n{out}"
        );
    }

    #[test]
    fn a_saved_file_reads_back_the_same() {
        let settings = Settings {
            push_to_talk: false,
            cleanup: Cleanup::Medium,
            denoise: true,
            sound: false,
            duck: 0,
            gpu: Some(0),
            hotkey: "ctrl+alt+space".to_string(),
            input_device: Some("alsa_input.usb-Generic_USB_Audio-00.HiFi_5_1__Mic__source".into()),
        };
        assert_eq!(Settings::parse(&settings.render("")), settings);
    }

    /// The window has to agree with the daemon on the default, or opening it
    /// once would write `sound = false` over a chime nobody turned off.
    #[test]
    fn the_sound_switch_starts_on() {
        assert!(Settings::default().sound);
        assert!(!Settings::parse("sound = false").sound);
        assert!(Settings::default().render("").contains("sound = true"));
    }

    /// The daemon applies keys in file order, so a `refine` line left below the
    /// `cleanup` line would undo the level the user just picked. Saving has to
    /// take the old key out, not just stop writing it.
    #[test]
    fn saving_removes_the_key_cleanup_replaced() {
        let existing = "cleanup = none\nrefine = true\n";
        let out = Settings {
            cleanup: Cleanup::None,
            ..Settings::default()
        }
        .render(existing);

        assert!(
            !out.contains("refine"),
            "stale refine line survived:\n{out}"
        );
        assert_eq!(Settings::parse(&out).cleanup, Cleanup::None);
    }

    /// A config written before levels existed must open on the level it meant.
    #[test]
    fn the_old_refine_key_still_reads() {
        assert_eq!(Settings::parse("refine = false\n").cleanup, Cleanup::None);
        assert_eq!(Settings::parse("refine = true\n").cleanup, Cleanup::Light);
    }

    #[test]
    fn trailing_comments_parse() {
        assert_eq!(Settings::parse("duck = 30 # quieter\n").duck, 30);
    }

    /// "Let the daemon choose" is the absence of the key, so switching back to
    /// automatic has to remove a line that is already there - writing `gpu = 0`
    /// would pin the first device instead.
    #[test]
    fn automatic_gpu_removes_the_key() {
        let pinned = Settings {
            gpu: Some(1),
            ..Settings::default()
        };
        let out = pinned.render("");
        assert!(out.contains("gpu = 1"), "gpu not written:\n{out}");
        assert_eq!(Settings::parse(&out).gpu, Some(1));

        let automatic = Settings::default();
        let back = automatic.render(&out);
        assert!(!back.contains("gpu ="), "gpu line survived:\n{back}");
        assert_eq!(Settings::parse(&back).gpu, None);
    }

    /// Same absence-means-automatic rule as `gpu`, and the same failure if it
    /// is got wrong: going back to Auto-detect has to take the line out, because
    /// the daemon reads a name that is still there as a mic that is still
    /// pinned.
    #[test]
    fn automatic_input_removes_the_key() {
        let pinned = Settings {
            input_device: Some("alsa_input.usb-webcam-02.iec958-stereo".into()),
            ..Settings::default()
        };
        let out = pinned.render("");
        assert_eq!(
            Settings::parse(&out).input_device.as_deref(),
            Some("alsa_input.usb-webcam-02.iec958-stereo")
        );

        let back = Settings::default().render(&out);
        assert!(
            !back.contains("input_device"),
            "input_device line survived:\n{back}"
        );
        assert_eq!(Settings::parse(&back).input_device, None);
    }
}
