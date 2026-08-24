//! How much Flow may change what was said: the cleanup level cards.

use crate::*;
use iced::widget::{column, container, row, text, Space};
use iced::{Background, Border, Color, Element, Fill, Font};

impl Console {
    /// How much Flow is allowed to change what you said.
    ///
    /// Four rows rather than a slider or a dropdown: the levels differ by what
    /// they are permitted to touch, which is a difference you can only judge by
    /// reading an example of each. A dropdown shows one option at a time and
    /// makes you remember the rest.
    ///
    /// The example under each title is the same sentence at every level, so the
    /// page demonstrates the difference instead of asserting it.
    pub(super) fn style_section(&self) -> Element<'_, Message> {
        // Cards, not settings rows: each already carries its own border and
        // tint, so a hairline between them would double the line. A small
        // gap does the separating instead, same as History's entries.
        let mut list = column![];
        for (index, level) in settings::Cleanup::ALL.into_iter().enumerate() {
            if index > 0 {
                list = list.push(Space::new().height(4));
            }
            list = list.push(self.cleanup_row(level));
        }

        scroll_inset(
            column![
                // "It never leaves this machine" used to close this line and
                // said the same thing About's subtitle already says. A promise
                // repeated on every page reads as a product that is anxious
                // about it.
                container(heading("Style", "How much Flow edits what you said."))
                    .padding([0.0, ENTRY_INSET])
                    .width(Fill),
                list,
            ],
            PAGE_TOP,
            CONTENT_RIGHT - ENTRY_INSET,
        )
    }

    /// One selectable level. The whole row is the target, because a row with a
    /// radio at one end trains you to aim at the radio.
    fn cleanup_row(&self, level: settings::Cleanup) -> Element<'_, Message> {
        let (title, blurb) = level.describe();
        let chosen = self.settings.cleanup == level;

        // Struck through at None, because that row's example is the one thing
        // on the page that is not an improvement - it is what you actually said.
        let example = text(level.example())
            .size(12)
            .font(Font::MONOSPACE)
            .color(if chosen { MUTED } else { FAINT });

        let body = column![
            row![
                text(title)
                    .size(13.5)
                    .color(if chosen { FG } else { MUTED }),
                Space::new().width(Fill),
                pip(if chosen { OK } else { mix(BG, FG, 0.18) }),
            ]
            .align_y(iced::Center),
            Space::new().height(LABEL_GAP),
            text(blurb).size(12).color(MUTED),
            Space::new().height(6),
            example,
        ];

        let tint = if chosen {
            mix(BG, OK, 0.05)
        } else {
            Color::TRANSPARENT
        };
        iced::widget::button(container(body).padding([10, 12]).width(Fill))
            .padding(0)
            .on_press(Message::SetCleanup(level))
            .style(move |_, status| iced::widget::button::Style {
                background: Some(Background::Color(match status {
                    iced::widget::button::Status::Hovered if !chosen => mix(BG, FG, 0.04),
                    _ => tint,
                })),
                border: Border {
                    radius: 8.0.into(),
                    width: 1.0,
                    color: if chosen {
                        mix(BG, OK, 0.3)
                    } else {
                        Color::TRANSPARENT
                    },
                },
                ..Default::default()
            })
            .into()
    }
}
