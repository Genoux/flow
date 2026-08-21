//! Everything dictated, most recent first.

use crate::*;
use iced::widget::{column, container, text};
use iced::{Element, Fill};

impl Console {
    pub(super) fn history_section(&self) -> Element<'_, Message> {
        let now = crate::history::now();

        let list: Element<'_, Message> = if self.entries.is_empty() {
            // Sits on the heading's left edge and at a row's own top pad, so
            // the line reads as the first entry's place rather than as loose
            // text floating outside the list.
            container(
                text("Nothing yet. Hold the chord and say something.")
                    .size(13)
                    .color(FAINT),
            )
            .padding([10.0, ENTRY_INSET])
            .width(Fill)
            .into()
        } else {
            let mut rows = column![];
            for (index, entry) in self.entries.iter().enumerate() {
                rows = rows.push(entry_row(
                    entry,
                    index,
                    now,
                    self.just_copied(index),
                    self.entry_warmth(index),
                    index + 1 < self.entries.len(),
                ));
            }
            entry_list(rows)
        };

        scroll_inset(
            column![
                container(heading(
                    "History",
                    "Everything Flow has typed for you, most recent first.",
                ))
                .padding([0.0, ENTRY_INSET])
                .width(Fill),
                list,
            ],
            PAGE_TOP,
            CONTENT_RIGHT - ENTRY_INSET,
        )
    }
}
