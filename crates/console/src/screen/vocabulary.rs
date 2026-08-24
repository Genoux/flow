//! Words the speech model would otherwise get wrong.

use crate::*;
use iced::widget::{column, container, row, text, Space};
use iced::{Background, Border, Element, Fill};

impl Console {
    /// The vocabulary, edited here rather than in a text editor. The file is
    /// the daemon's interface; it should not have to be the user's.
    pub(super) fn vocabulary_section(&self) -> Element<'_, Message> {
        let mut list = column![];
        if self.terms.is_empty() {
            list = list.push(
                // Sits where the first term would, so the line reads as the
                // list's own state rather than as a third paragraph of help.
                container(text("No words yet.").size(13).color(FAINT)).padding([ROW_PAD, 0.0]),
            );
        } else {
            for (index, term) in self.terms.iter().enumerate() {
                list = list.push(
                    container(
                        row![
                            text(term.clone()).size(13).color(FG),
                            Space::new().width(Fill),
                            action_msg("Remove", false, Message::RemoveTerm(index)),
                        ]
                        .align_y(iced::Center),
                    )
                    // The same air as any other row in this console, on both
                    // sides of every hairline. At 6 the terms read as a
                    // cramped table dropped into a page whose every other
                    // list breathes.
                    .padding([ROW_PAD, 0.0]),
                );
                if index + 1 < self.terms.len() {
                    list = list.push(hairline());
                }
            }
        }

        let entry = row![
            iced::widget::text_input("Hyprland", &self.typing)
                .on_input(Message::TypingTerm)
                .on_submit(Message::AddTerm)
                .size(13)
                .padding([8, 10])
                .style(|_theme, _status| iced::widget::text_input::Style {
                    background: Background::Color(BG),
                    border: Border {
                        color: LINE,
                        width: 1.0,
                        radius: 6.0.into()
                    },
                    icon: FAINT,
                    placeholder: FAINT,
                    value: FG,
                    selection: ACCENT,
                }),
            Space::new().width(8),
            action_msg("Add", true, Message::AddTerm),
        ]
        .align_y(iced::Center);

        let note: Element<Message> = match &self.term_error {
            Some(why) => text(why.clone()).size(12).color(ERR).into(),
            None => text("Works when the word sounds close: \"hyper land\" becomes Hyprland.")
                .size(12)
                .color(FAINT)
                .into(),
        };

        scroll(column![
            // Not "one per line": that is the rule for the file behind this
            // screen, and this screen has an add field.
            heading(
                "Vocabulary",
                "Words Flow mishears, spelled the way you want them.",
            ),
            entry,
            // Tight to the field it explains, then a real gap before the
            // list - the two spaces have to differ or the field, its note
            // and the terms read as three unrelated things equally spaced.
            Space::new().height(8),
            note,
            Space::new().height(GAP),
            list,
        ])
    }
}
