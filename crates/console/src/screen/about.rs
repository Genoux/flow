//! Version, models, paths, and which build is running.

use crate::*;
use iced::widget::{column, container, row, text, Space};
use iced::{Element, Fill, Font};

impl Console {
    pub(super) fn about_section(&self) -> Element<'_, Message> {
        // Which engines these are is a fact about the build, not a choice -
        // the same class of thing as the version. They are constants now
        // rather than files on disk, so they cannot be missing or damaged.
        let rows: Vec<Element<Message>> = vec![
            self.version_row(),
            fact_row("Build", "Experimental · MAI + Flash-Lite".to_string()),
            fact_row("Speech", "microsoft/mai-transcribe-2".to_string()),
            fact_row("Polish", "google/gemini-3.1-flash-lite".to_string()),
            fact_row("Session", self.session.clone()),
            fact_path("Config", &settings::config_path()),
            fact_path("History", &crate::history::path()),
        ];

        // Not "push-to-talk": tap to start and tap to stop is the other half
        // of the Shortcut group, and naming only one of them here made the
        // product's one-line description describe a setting.
        section_shell("Flow", "Dictation through MAI and Flash-Lite.", rows)
    }

    fn version_row(&self) -> Element<'_, Message> {
        let (dot, note) = update_state(&self.update);

        let action = if self.updating {
            action_msg("Updating…", true, Message::InstallUpdate)
        } else if matches!(self.update, update::Status::Installed(_)) {
            action_msg("Restart Flow", true, Message::RestartApp)
        } else if let update::Status::Available(tag) = &self.update {
            action_msg(&format!("Update to {tag}"), true, Message::InstallUpdate)
        } else if self.update == update::Status::Checking {
            action_msg("Checking…", false, Message::CheckUpdate)
        } else {
            action_msg("Check for updates", false, Message::CheckUpdate)
        };

        container(
            row![
                column![
                    text("Version").size(13.5).color(FG),
                    Space::new().height(LABEL_GAP),
                    text(note).size(12).color(FAINT),
                ],
                Space::new().width(Fill),
                pip(dot),
                Space::new().width(7),
                text(update::running())
                    .size(12)
                    .font(Font::MONOSPACE)
                    .color(MUTED),
                Space::new().width(12),
                action,
            ]
            .align_y(iced::Center),
        )
        .padding([ROW_PAD, 0.0])
        .into()
    }
}
