//! The chord, hold-to-talk, and the rest of how dictation is triggered.

use crate::*;
use iced::widget::{row, text, Space};
use iced::{Element, Font};

impl Console {
    pub(super) fn dictation_section(&self) -> Element<'_, Message> {
        let mut rows: Vec<Element<Message>> = vec![
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
                    // Reset earns its place only when the chord is not
                    // already the default - offered next to a chord that is
                    // the default, it is a button that does nothing.
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
            setting(
                "Terminal paste chord",
                "Send Ctrl+Shift+V when a terminal has focus.",
                toggle(
                    self.settings.terminal,
                    self.travel("terminal"),
                    Message::Terminal,
                ),
            ),
            setting(
                "Vocabulary",
                "Names and jargon the recogniser should get right.",
                row![
                    text(format!("{} terms", self.terms.len()))
                        .size(12)
                        .font(Font::MONOSPACE)
                        .color(MUTED),
                    Space::new().width(12),
                    action_msg("Edit", false, Message::Select(Section::Vocabulary)),
                ]
                .align_y(iced::Center)
                .into(),
            ),
        ];

        // Only offered when systemd actually answered. A switch we cannot read
        // the true state of is worse than no switch.
        if let Some(enabled) = self.autostart {
            rows.push(setting(
                "Start with session",
                "Enables the flow.service user unit so the daemon launches when you log in.",
                toggle(enabled, self.travel("autostart"), Message::Autostart),
            ));
        }

        section_shell(
            "Dictation",
            "How the chord behaves and what happens to your words.",
            rows,
            Some(self.save_note()),
        )
    }
}
