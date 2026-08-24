//! Everything you can change about how Flow behaves, on one page.
//!
//! Dictation and Audio used to be two rail sections holding four rows each,
//! which meant a click to find out whether the thing you wanted was on the
//! other one. They are groups now: the same rows, told apart by a label and
//! air, all reachable without navigating.
//!
//! Grouped by what a setting is about, which is the only thing that survives
//! being read by somebody who does not already know the app. "Dictation" was
//! the first attempt at a group name here and it carried nothing: this app only
//! does dictation, so the label was the product name rather than a topic, and
//! every row on the page would have qualified for it. GNOME's guidance names
//! "General" and "Options" as uninformative for exactly that reason. The fix is
//! to say what the rows have in common, not what the program is called.
//!
//! General leads, as it does on every platform with a settings window, and holds
//! the app-level rows that belong to no topic - starting with the login item,
//! which is where macOS files its own.

use crate::*;
use iced::widget::{column, row, text, Space};
use iced::{Element, Font};

impl Console {
    pub(super) fn preferences_section(&self) -> Element<'_, Message> {
        let body = column![
            group("General", self.general_rows()),
            group("Trigger", self.trigger_rows()),
            group("Microphone", self.microphone_rows()),
        ];

        page_shell(
            "Settings",
            "How Flow starts, what starts a dictation, and what it listens to.",
            body.into(),
            self.save_note(),
        )
    }

    /// The app-level rows, which belong to no topic of their own. The login item
    /// is first: it is the row about the program rather than about dictating, so
    /// it is what somebody looks for before they have learned anything else.
    fn general_rows(&self) -> Vec<Element<'_, Message>> {
        let mut rows: Vec<Element<Message>> = Vec::new();
        // Only offered when systemd actually answered. A switch we cannot read
        // the true state of is worse than no switch.
        if let Some(enabled) = self.autostart {
            rows.push(setting(
                "Start with session",
                "Enables the flow.service user unit so the daemon launches when you log in.",
                toggle(enabled, self.travel("autostart"), Message::Autostart),
            ));
        }
        rows.push(setting(
            "Dictation sound",
            "A short chime when the island appears, and another when it goes.",
            toggle(self.settings.sound, self.travel("sound"), Message::Sound),
        ));
        rows
    }

    /// What starts and ends a dictation. Named for what it does rather than for
    /// the key, so the row called "Chord" is not sitting under a heading of the
    /// same word - a group label that repeats a row label tells you nothing
    /// about the rows beside it.
    fn trigger_rows(&self) -> Vec<Element<'_, Message>> {
        vec![
            setting(
                "Hold to talk",
                "On, hold the chord while you speak. Off, tap to start and tap to stop.",
                toggle(
                    self.settings.push_to_talk,
                    self.travel("push_to_talk"),
                    Message::PushToTalk,
                ),
            ),
            setting(
                "Chord",
                // This did once need a restart, and the note saying so outlived
                // the reason: the chord is shared with the config watcher now
                // and `hotkey::spawn` compares it on every key, so a rebinding
                // is live by the next press. The stale line was sending people
                // off to restart for nothing.
                "The keys that start a dictation.",
                row![
                    text(if self.capturing {
                        "press the chord…".to_string()
                    } else {
                        self.settings.hotkey.replace('+', " ")
                    })
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(if self.capturing { ACCENT } else { MUTED }),
                    Space::new().width(12),
                    // Reset earns its place only when the chord is not already
                    // the default - offered next to a chord that is the default,
                    // it is a button that does nothing.
                    if !self.capturing && self.settings.hotkey != settings::DEFAULT_HOTKEY {
                        row![
                            action_msg("Reset", false, Message::ResetChord),
                            Space::new().width(8)
                        ]
                        .into()
                    } else {
                        Element::from(Space::new().width(0))
                    },
                    if self.capturing {
                        action_msg("Cancel", false, Message::CancelCapture)
                    } else if self.can_capture {
                        action_msg("Change", false, Message::CaptureChord)
                    } else {
                        // No readable keyboard, so offer the file instead of a
                        // button that could only fail.
                        action_msg("Open config", false, Message::OpenConfig)
                    },
                ]
                .align_y(iced::Center)
                .into(),
            ),
        ]
    }

    /// What Flow listens to, and what is done to the room before it does.
    /// Ducking belongs here rather than with the sounds Flow makes: it exists to
    /// keep your speakers out of the microphone, which is a fact about the
    /// input, not about the output.
    fn microphone_rows(&self) -> Vec<Element<'_, Message>> {
        vec![
            // Read-only, and deliberately so: the daemon records from the
            // system default source, which means changing your microphone in
            // your desktop's own settings already works. A picker here could
            // only ever be a second answer to the same question.
            setting(
                "Microphone",
                "Follows your system's default input. Change it in your sound settings.",
                text(
                    self.input
                        .clone()
                        .unwrap_or_else(|| "not detected".to_string()),
                )
                .size(12)
                .font(Font::MONOSPACE)
                .color(MUTED)
                .into(),
            ),
            setting(
                "Turn other apps down",
                "Keeps your speakers out of the microphone while you dictate.",
                value_slider(
                    0..=100,
                    self.settings.duck,
                    Message::Duck,
                    &format!("{}%", self.settings.duck),
                ),
            ),
            setting(
                "Noise suppression",
                "Runs RNNoise over the audio. Can blunt consonants on a weak mic.",
                toggle(
                    self.settings.denoise,
                    self.travel("denoise"),
                    Message::Denoise,
                ),
            ),
        ]
    }
}
