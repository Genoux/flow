use crate::theme::LINE;
use crate::*;
use iced::widget::{button, column, container, responsive, row, text, text_input, tooltip, Space};
use iced::{Background, Border, Element, Fill};

impl Console {
    /// Why nothing on this screen is being applied, if it is not.
    ///
    /// `refine::system_prompt` is the only place a term is ever used, so a
    /// missing key and `cleanup = none` both leave the list on disk and out of
    /// the pipeline. Saying which one is the difference between a screen that
    /// looks broken and one that tells you where to go.
    fn vocabulary_block(&self) -> Option<String> {
        if self.settings.openrouter_key.is_none() {
            return Some("Not applied - these are spelled by the refining model, which needs an OpenRouter key.".to_string());
        }
        if !self.settings.cleanup.needs_key() {
            return Some("Not applied while cleanup is Off - change it in Style.".to_string());
        }
        None
    }

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
                    .size(MICRO)
                    .padding([8, 12])
                    .width(if wide { 230 } else { 160 })
                    .style(move |theme, status| {
                        super::editorial::input_style(theme, status, amount.get())
                    })
                    .into()
            });
            // The count matters less than whether any of these are reaching
            // anything: vocabulary is only ever spelled by the refining model,
            // so there are two states in which this whole screen is inert.
            let (subtitle, tone) = match self.vocabulary_block() {
                Some(reason) => (reason, WARN),
                None => (count, MUTED),
            };
            let header = row![
                column![
                    text("Your vocabulary").size(HEAD).color(FG),
                    text(subtitle).size(MICRO).color(tone)
                ]
                .spacing(4),
                Space::new().width(Fill),
                search,
            ]
            .align_y(iced::Center);

            // The empty state takes whatever the pane has left rather than
            // sitting in a short box with a hole under it. It cannot ask for
            // Fill: this whole page is inside a scrollable, which lays its
            // content out in unbounded height, and Fill against infinity
            // resolves to shrink. The pane height from `responsive` is the only
            // real number available here, so the rest of the stack is
            // subtracted from it.
            // ponytail: the two block heights are measured, not derived - they
            // drift if the banner or the editor is restyled. The floor keeps a
            // drifted number from collapsing the box; derive them from layout
            // if this ever needs to be exact.
            const BANNER_HEIGHT: f32 = 180.0;
            const EDITOR_HEIGHT: f32 = 114.0;
            let above =
                PAGE_TOP * 2.0 + 47.0 + BANNER_HEIGHT + 24.0 + EDITOR_HEIGHT + 12.0 + 40.0 + 14.0;
            let empty_height = (size.height - above).max(150.0);

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
                            text(title).size(BODY).color(FG),
                            text(detail).size(MICRO).color(MUTED),
                        ]
                        .spacing(6)
                        .align_x(iced::Center),
                    )
                    .padding([32, 24])
                    .width(Fill)
                    .height(empty_height)
                    .align_x(iced::Center)
                    .align_y(iced::Center)
                    .style(|_: &iced::Theme| container::Style {
                        border: Border {
                            color: LINE,
                            width: HAIRLINE,
                            radius: CARD_RADIUS.into(),
                        },
                        ..Default::default()
                    }),
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
                text("Add a word or phrase").size(BODY).color(FG),
                Space::new().height(10),
                row![
                    crate::interaction::field(|amount| text_input(
                        "e.g. a name, brand, or technical term",
                        &self.typing
                    )
                    .on_input(Message::TypingTerm)
                    .on_submit(Message::AddTerm)
                    .id("vocabulary-entry")
                    .size(BODY)
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
                        .size(MICRO)
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
                text(&self.terms[index]).size(BODY).color(FG).width(Fill),
                tooltip(
                    remove,
                    container(text("Remove word").size(MICRO))
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
