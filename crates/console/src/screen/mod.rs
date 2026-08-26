//! The window's chrome and the one screen inside it.
//!
//! `view` is the shell, `rail` the left-hand nav, and `pane` the dispatcher
//! that turns the selected `Section` into its own module below. Sections live
//! in sibling files so that editing the Audio page means opening audio.rs, and
//! so this file stays the map rather than the territory.

use crate::*;
use iced::widget::{column, container, row, stack, text, Space};
use iced::{Element, Fill, Font, Length};

mod about;
mod history;
mod overview;
mod preferences;
mod style;
mod vocabulary;

impl Console {
    pub(crate) fn view(&self) -> Element<'_, Message> {
        // Setup takes the whole window, rail included. The rail is a way to
        // move between seven screens that have nothing on them yet, and
        // offering it here would be offering the user seven ways to watch the
        // same download from somewhere it cannot be seen.
        // The rail/pane divider is its own element: a container border applies
        // to all four sides, and only this edge should be drawn.
        let console = row![self.rail(), vertical_hairline(), self.pane()];

        let Some(state) = self.download.as_ref().filter(|_| self.showing_setup) else {
            // A dialog is the only other thing in this window allowed to sit on
            // top of the console, and it goes on top the same way setup does:
            // the page underneath stays visible and stops taking clicks.
            return match self.mic_dialog() {
                Some(dialog) => stack![inert(console), dialog].into(),
                None => console.into(),
            };
        };

        // Setup dissolves into the console rather than being replaced by it.
        // The console sits underneath so the veil has something to fade over -
        // to be looked at, and nothing else. `inert` is what stops the pointer
        // reaching a page that is behind a full-screen overlay.
        let fade = match self.fading {
            Some(elapsed) => 1.0 - (elapsed / setup::FADE).clamp(0.0, 1.0),
            None => 1.0,
        };
        stack![inert(console), setup::view(state, fade)].into()
    }

    // -- rail ---------------------------------------------------------------

    fn rail(&self) -> Element<'_, Message> {
        let mut items = column![
            container(text("Flow").size(14).color(FG)).padding([0, 9]),
            Space::new().height(14)
        ]
        .spacing(2);

        for section in Section::ALL {
            let selected = section == self.section;
            let warmth = if self.hovered == Some(section) {
                progress(self.hover_at, self.now, FADE)
            } else {
                0.0
            };
            let enabled = !self.incomplete() || section.works_without_models();
            items = items.push(nav(section, selected, warmth, enabled));
        }

        container(
            column![
                items,
                Space::new().height(Fill),
                // A debug build says so, and says when it was made. See
                // `update::dev_note`.
                container(column![
                    text(update::running())
                        .size(11)
                        .font(Font::MONOSPACE)
                        .color(FAINT),
                    text(update::dev_note().unwrap_or_default())
                        .size(10)
                        .font(Font::MONOSPACE)
                        .color(FAINT),
                ])
                .padding([0, 9]),
            ]
            .spacing(0),
        )
        .width(Length::Fixed(RAIL_WIDTH))
        .height(Fill)
        .padding([26, 11])
        .into()
    }

    // -- pane ---------------------------------------------------------------

    fn pane(&self) -> Element<'_, Message> {
        // A disabled nav item cannot be clicked, but `FLOW_SECTION` can open
        // one directly and a section can be disabled while it is already open -
        // stopping setup from Style would leave it on screen with a nav that
        // says it is unavailable. Overview is where the way out is.
        let section = if self.incomplete() && !self.section.works_without_models() {
            Section::Overview
        } else {
            self.section
        };

        let content = match section {
            Section::Overview => self.overview_section(),
            Section::History => self.history_section(),
            Section::Settings => self.preferences_section(),
            Section::Vocabulary => self.vocabulary_section(),
            Section::Style => self.style_section(),
            Section::About => self.about_section(),
        };

        // Switching sections is deliberately instant. Motion here read as the
        // page arriving late rather than as polish - navigation should feel
        // like the content was already there.
        //
        // Left only, not both sides: a right pad here would sit outside the
        // scrollable and push its whole viewport away from the window edge,
        // floating the scrollbar in a dead gutter instead of letting it run
        // where every other app puts one - flush against the edge it scrolls.
        // `scroll_pad` owns the right side, on the content, so the bar can
        // sit at the edge while the text keeps its own margin from it.
        let left = if matches!(self.section, Section::History | Section::Style) {
            PANE_INSET - ENTRY_INSET
        } else {
            PANE_INSET
        };

        container(content)
            .width(Fill)
            .height(Fill)
            .padding(iced::Padding::default().left(left))
            .into()
    }
}
