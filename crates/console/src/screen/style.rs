use crate::*;
use iced::widget::{button, column, container, responsive, row, text, text_input, tooltip, Space};
use iced::{Background, Border, Element, Fill};

impl Console {
    pub(super) fn style_section(&self) -> Element<'_, Message> {
        responsive(move |size| {
            let width = size.width - CONTENT_RIGHT;
            let wide = width >= 650.0;
            let cards: Element<'_, Message> = if wide {
                settings::Cleanup::ALL
                    .into_iter()
                    .fold(row![], |cards, level| {
                        cards.push(
                            self.cleanup_card(level, if width < 740.0 { 290.0 } else { 260.0 }),
                        )
                    })
                    .spacing(GAP)
                    .into()
            } else {
                settings::Cleanup::ALL
                    .into_iter()
                    .fold(column![], |cards, level| {
                        cards.push(self.cleanup_card(level, 236.0))
                    })
                    .spacing(GAP)
                    .into()
            };

            let banner = super::editorial::banner(
                "A little polish.\nStill your voice.",
                "Keep every word, tidy up the stumbles,\nor make your thoughts more concise.",
                wide,
                false,
                super::editorial::Photo::Woodland,
            );

            scroll(column![
                heading("Style", ""),
                banner,
                Space::new().height(24),
                cards,
                Space::new().height(20),
                text("The same thought, with a different amount of cleanup.")
                    .size(12)
                    .color(MUTED),
                Space::new().height(36),
                self.instructions_editor(wide),
            ])
        })
        .into()
    }

    /// The cards decide how much of what you said is changed. This decides how
    /// the result is written, which is the part no level can know.
    fn instructions_editor(&self, wide: bool) -> Element<'_, Message> {
        let error = self.note_error.as_deref();
        let note = error.unwrap_or(
            "For example, “Use British spelling.” or “Keep code names exactly as I say them.”",
        );

        let list = self
            .notes
            .iter()
            .enumerate()
            .fold(column![].spacing(8), |list, (index, _)| {
                list.push(self.instruction_row(index))
            });

        column![
            text("Your instructions").size(15).color(FG),
            Space::new().height(4),
            text("Followed on every dictation, at every level above Off.")
                .size(12)
                .color(MUTED),
            Space::new().height(14),
            list,
            Space::new().height(if self.notes.is_empty() { 0 } else { 12 }),
            row![
                crate::interaction::field(|amount| text_input(
                    "e.g. Use British spelling.",
                    &self.note_typing
                )
                .on_input(Message::TypingInstruction)
                .on_submit(Message::AddInstruction)
                .id("instruction-entry")
                .size(13)
                .padding([10, 12])
                .style(move |theme, status| super::editorial::input_style(
                    theme,
                    status,
                    amount.get()
                ))
                .into()),
                crate::control::action_padded(
                    if wide { "Add instruction" } else { "Add" },
                    true,
                    1.0,
                    [10.0, 14.0],
                    (!self.note_typing.trim().is_empty()).then_some(Message::AddInstruction)
                ),
            ]
            .spacing(10)
            .align_y(iced::Center),
            Space::new().height(8),
            // Reserved whether or not anything is being said, so adding an
            // instruction cannot move the page under the pointer.
            container(
                text(note)
                    .size(12)
                    .line_height(1.5)
                    .color(if error.is_some() { ERR } else { MUTED })
            )
            .height(40),
        ]
        .into()
    }

    fn instruction_row(&self, index: usize) -> Element<'_, Message> {
        let remove = crate::interaction::hover(move |amount| {
            button(text("×").size(20))
                .padding([2, 8])
                .on_press(Message::RemoveInstruction(index))
                .style(move |_, _| button::Style {
                    text_color: mix(MUTED, FG, amount.get()),
                    background: None,
                    ..Default::default()
                })
                .into()
        });

        container(
            row![
                text(&self.notes[index]).size(14).color(FG).width(Fill),
                tooltip(
                    remove,
                    container(text("Remove instruction").size(12))
                        .padding(8)
                        .style(container::dark),
                    tooltip::Position::Top
                ),
            ]
            .spacing(12)
            .align_y(iced::Center),
        )
        .padding([12, 14])
        .width(Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(mix(BG, theme::RAISED, 0.65))),
            border: Border {
                radius: CARD_RADIUS.into(),
                ..Default::default()
            },
            ..Default::default()
        })
        .into()
    }

    fn cleanup_card(&self, level: settings::Cleanup, height: f32) -> Element<'_, Message> {
        let (title, blurb) = level.describe();
        let chosen = self.settings.cleanup == level;
        let selected = self.cleanup_selection[level as usize].value(self.now);
        let warmth = if chosen {
            0.0
        } else {
            self.cleanup_hover[level as usize].value(self.now)
        };
        let surface = mix(
            mix(mix(BG, theme::RAISED, 0.6), ACCENT, 0.035 * selected),
            FG,
            warmth * 0.025,
        );
        let tone = mix(MUTED, ACCENT, selected);
        let indicator = container(container(Space::new().width(6).height(6)).style(move |_| {
            container::Style {
                background: Some(Background::Color(iced::Color {
                    a: selected,
                    ..tone
                })),
                border: Border {
                    radius: 3.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        }))
        .center(16)
        .style(move |_| container::Style {
            border: Border {
                radius: 8.0.into(),
                width: HAIRLINE,
                color: tone,
            },
            ..Default::default()
        });

        let body = column![
            row![
                text(title).size(19).color(FG),
                Space::new().width(Fill),
                indicator,
            ]
            .align_y(iced::Center),
            Space::new().height(10),
            container(text(blurb).size(12).line_height(1.5).color(MUTED)).height(54),
            Space::new().height(16),
            text("“")
                .size(32)
                .line_height(0.8)
                .color(mix(MUTED, ACCENT, selected * 0.5)),
            Space::new().height(6),
            text(level.example()).size(14).line_height(1.5).color(FG),
        ];

        iced::widget::mouse_area(
            button(container(body).padding(20).width(Fill).height(height))
                .padding(0)
                .width(Fill)
                .on_press_maybe((!chosen).then_some(Message::SetCleanup(level)))
                .style(move |_, status| button::Style {
                    background: Some(Background::Color(
                        if chosen || status == button::Status::Pressed {
                            mix(mix(BG, theme::RAISED, 0.6), ACCENT, 0.035)
                        } else {
                            surface
                        },
                    )),
                    border: Border {
                        radius: CARD_RADIUS.into(),
                        ..Default::default()
                    },
                    ..Default::default()
                }),
        )
        .on_enter(Message::HoverCleanup(Some(level)))
        .on_exit(Message::HoverCleanup(None))
        .into()
    }
}
