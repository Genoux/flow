//! Version, models, paths, and the setup re-entry point.

use crate::*;
use iced::widget::{column, container, row, text, Space};
use iced::{Element, Fill, Font};

impl Console {
    pub(super) fn about_section(&self) -> Element<'_, Message> {
        // Bound so the borrows outlive the rows built from them.
        // Which engines these are is a fact about the build, not a choice -
        // the same class of thing as the version. They stopped being a screen
        // of their own when the install began fetching both.
        let rows: Vec<Element<Message>> = vec![
            self.version_row(),
            fact_row("Speech", self.model_fact(0)),
            fact_row("Cleanup", self.model_fact(1)),
            fact_row("Session", self.session.clone()),
            fact_path("Config", &settings::config_path()),
            fact_path("History", &crate::history::path()),
            // The way back to a clean install. Here rather than on Overview
            // because it belongs with the two model rows above it - it is the
            // thing you do when one of them is wrong.
            setting(
                "Run setup again",
                "Deletes both models and fetches them from scratch. About 3 GB.",
                action_msg("Run setup", false, Message::RerunSetup),
            ),
        ];

        section_shell(
            "Flow",
            "Push-to-talk dictation that runs entirely on your own machine.",
            rows,
            None,
        )
    }

    fn model_fact(&self, index: usize) -> String {
        self.models
            .get(index)
            .map(system::Model::fact)
            .unwrap_or_else(|| "unknown".into())
    }

    fn version_row(&self) -> Element<'_, Message> {
        let (dot, note) = update_state(&self.update);

        let action = if self.updating {
            action_msg("Updating…", true, Message::InstallUpdate)
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
