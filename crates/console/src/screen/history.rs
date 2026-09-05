use crate::*;
use iced::widget::{column, container, row, text, Space};
use iced::{Element, Fill};

impl Console {
    pub(super) fn history_section(&self) -> Element<'_, Message> {
        let now = crate::history::now();
        let header = container(column![
            row![
                text("History").size(22).color(FG),
                Space::new().width(Fill),
                text(format!("{} recent dictations", self.entries.len()))
                    .size(12)
                    .color(MUTED),
            ]
            .align_y(iced::Center),
            Space::new().height(10),
            text("A record of your words.").size(13).color(MUTED),
            Space::new().height(32),
        ])
        .padding([0.0, ENTRY_INSET])
        .width(Fill);

        let list: Element<'_, Message> = if self.entries.is_empty() {
            container(column![
                text("Your first thought starts here.").size(18).color(FG),
                Space::new().height(10),
                text("Dictate anywhere. Your words will be here when you need them.")
                    .size(13)
                    .line_height(1.5)
                    .color(MUTED),
            ])
            .padding(
                iced::Padding::default()
                    .top(24)
                    .left(ENTRY_INSET)
                    .right(ENTRY_INSET),
            )
            .width(Fill)
            .into()
        } else {
            let mut rows = column![];
            let mut previous = "";
            for (index, entry) in self.entries.iter().enumerate() {
                let period = crate::history::period(entry.at, now);
                if period != previous {
                    if index > 0 {
                        rows = rows.push(Space::new().height(28));
                    }
                    rows = rows.push(
                        container(text(period).size(12).color(FG))
                            .padding(iced::Padding::default().left(ENTRY_INSET).bottom(8)),
                    );
                    previous = period;
                }
                rows = rows.push(entry_row(
                    entry,
                    index,
                    now,
                    self.just_copied(index),
                    self.entry_warmth(index),
                ));
            }
            entry_list(rows)
        };

        scroll_inset(column![header, list], PAGE_TOP, CONTENT_RIGHT - ENTRY_INSET)
    }
}
