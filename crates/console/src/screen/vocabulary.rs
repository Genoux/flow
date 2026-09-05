use crate::*;
use iced::widget::{button, column, container, responsive, row, text, text_input, tooltip, Space};
use iced::{Background, Border, Element, Fill};

impl Console {
    pub(super) fn vocabulary_section(&self) -> Element<'_, Message> {
        responsive(move |size| {
            let wide = size.width - CONTENT_RIGHT >= 650.0;
            let entries = vocabulary::matching(&self.terms, &self.term_query);
            let count = if self.term_query.trim().is_empty() {
                plural(self.terms.len() as u32, "word")
            } else {
                format!("{} of {}", entries.len(), self.terms.len())
            };
            let search = crate::interaction::field(|amount| {
                text_input("Find a word…", &self.term_query)
                    .on_input(Message::FilterTerms)
                    .size(12)
                    .padding([8, 12])
                    .width(if wide { 230 } else { 160 })
                    .style(move |theme, status| {
                        super::editorial::input_style(theme, status, amount.get())
                    })
                    .into()
            });
            let header = row![
                column![
                    text("Your vocabulary").size(15).color(FG),
                    text(count).size(12).color(MUTED)
                ]
                .spacing(4),
                Space::new().width(Fill),
                search,
            ]
            .align_y(iced::Center);

            let mut list = column![].spacing(8);
            if entries.is_empty() {
                let (title, detail) = if self.terms.is_empty() {
                    (
                        "Make room for your words.",
                        "Add names, brands, or specialist terms you use often.",
                    )
                } else {
                    (
                        "No matching words.",
                        "Try a different spelling or a shorter search.",
                    )
                };
                list = list.push(
                    container(
                        column![
                            text(title)
                                .size(19)
                                .font(iced::Font::with_name("Noto Serif Display"))
                                .color(FG),
                            text(detail).size(12).color(MUTED),
                        ]
                        .spacing(8),
                    )
                    .padding([24, 0]),
                );
            } else {
                for group in entries.chunks(if wide { 2 } else { 1 }) {
                    let mut line = row![].spacing(8);
                    for &index in group {
                        line = line.push(self.vocabulary_word(index));
                    }
                    if wide && group.len() == 1 {
                        line = line.push(Space::new().width(Fill));
                    }
                    list = list.push(line);
                }
            }

            let error = self.term_error.as_deref();
            let note = error.unwrap_or(
                "For example, “hyper land” can become “Hyprland” when the sounds are close.",
            );
            let editor = column![
                text("Add a word or phrase").size(14).color(FG),
                Space::new().height(10),
                row![
                    crate::interaction::field(|amount| text_input(
                        "e.g. a name, brand, or technical term",
                        &self.typing
                    )
                    .on_input(Message::TypingTerm)
                    .on_submit(Message::AddTerm)
                    .id("vocabulary-entry")
                    .size(13)
                    .padding([10, 12])
                    .style(move |theme, status| super::editorial::input_style(
                        theme,
                        status,
                        amount.get()
                    ))
                    .into()),
                    crate::control::action_padded(
                        "Add word",
                        true,
                        1.0,
                        [10.0, 14.0],
                        (!self.typing.trim().is_empty()).then_some(Message::AddTerm)
                    ),
                ]
                .spacing(10)
                .align_y(iced::Center),
                Space::new().height(8),
                container(
                    text(note)
                        .size(12)
                        .line_height(1.5)
                        .color(if error.is_some() { ERR } else { MUTED })
                )
                .height(40),
            ];

            scroll(column![
                heading("Vocabulary", ""),
                super::editorial::banner(
                    "Words that are yours.",
                    "Names, places, and specialist terms.\nHelp Flow get the spelling right.",
                    wide,
                    true,
                    super::editorial::Photo::Portrait,
                ),
                Space::new().height(24),
                editor,
                Space::new().height(12),
                header,
                Space::new().height(14),
                list,
            ])
        })
        .into()
    }

    fn vocabulary_word(&self, index: usize) -> Element<'_, Message> {
        let remove = crate::interaction::hover(move |amount| {
            button(text("×").size(20))
                .padding([2, 8])
                .on_press(Message::RemoveTerm(index))
                .style(move |_, _| button::Style {
                    text_color: mix(MUTED, FG, amount.get()),
                    background: None,
                    ..Default::default()
                })
                .into()
        });
        container(
            row![
                text(&self.terms[index]).size(14).color(FG).width(Fill),
                tooltip(
                    remove,
                    container(text("Remove word").size(12))
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
}
