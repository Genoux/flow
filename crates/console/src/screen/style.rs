use crate::*;
use iced::widget::{button, column, container, responsive, row, text, Space};
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
                text("The same thought, with a different amount of polish.")
                    .size(12)
                    .color(MUTED),
            ])
        })
        .into()
    }

    fn cleanup_card(&self, level: settings::Cleanup, height: f32) -> Element<'_, Message> {
        let (title, blurb) = level.describe();
        let chosen = self.settings.cleanup == level;
        let locked = level.needs_key() && self.settings.openrouter_key.is_none();
        let selected = self.cleanup_selection[level as usize].value(self.now);
        // A card that cannot be picked must not light up under the pointer:
        // the glow is the only thing on it that claims it is pressable.
        let warmth = if chosen || locked {
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
                text(title).size(19).color(if locked { MUTED } else { FG }),
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
            // The example is what this level would write. A locked one has
            // written nothing, so the slot says why instead of promising it.
            text(if locked {
                "Needs an OpenRouter key."
            } else {
                level.example()
            })
            .size(14)
            .line_height(1.5)
            .color(if locked { MUTED } else { FG }),
        ];

        iced::widget::mouse_area(
            button(container(body).padding(20).width(Fill).height(height))
                .padding(0)
                .width(Fill)
                .on_press_maybe((!chosen && !locked).then_some(Message::SetCleanup(level)))
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
