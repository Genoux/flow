//! The raised surfaces the Overview is built from.
//!
//! A card is a container with a hairline and a shadow; everything about how
//! one is drawn lives here so seven of them on a page cannot drift apart.

use crate::theme::{dissolve, mix, EDGE, FAINT, FG, HAIRLINE, MUTED, RAISED};
use crate::Message;
use iced::widget::{column, container, text, Space};
use iced::{Background, Border, Color, Element, Fill, Font};

/// A raised surface with a hairline edge - the unit every Overview card is
/// built from, so a handful of related facts read as one glance rather than
/// more rows in the same list.
pub(crate) fn panel<'a>(content: Element<'a, Message>) -> Element<'a, Message> {
    panel_at(1.0, content)
}

/// The same surface, arriving or leaving. Split out for the microphone dialog,
/// whose panel has to fade with the words on it: left at full weight it was a
/// solid plate popping onto a console that had not dimmed yet, and only then
/// did the veil catch up - the one beat of the whole thing that read as two
/// events instead of one.
///
/// [`dissolve`], not [`emerge`]. A panel is a filled rectangle, and a filled
/// rectangle walked toward `BG` is still filled - on the way out it sat over the
/// settings rows as an opaque plate with the page's own words behind it, which
/// is the "placeholder" it left while disappearing. Only alpha ends in nothing.
///
/// Shares this body rather than copying it, so the dialog cannot end up on a
/// surface a shade off the cards.
pub(crate) fn panel_at<'a>(fade: f32, content: Element<'a, Message>) -> Element<'a, Message> {
    container(content)
        .padding(14)
        .width(Fill)
        .style(move |_theme| container::Style {
            background: Some(Background::Color(dissolve(RAISED, fade))),
            border: Border {
                color: dissolve(EDGE, fade),
                width: HAIRLINE,
                radius: 10.0.into(),
            },
            shadow: iced::Shadow {
                color: Color {
                    a: 0.22 * fade,
                    ..Color::BLACK
                },
                offset: iced::Vector::new(0.0, 2.0),
                blur_radius: 14.0,
            },
            ..Default::default()
        })
        .into()
}

/// A panel with its subject named along the top.
pub(crate) fn card<'a>(title: &'a str, content: Element<'a, Message>) -> Element<'a, Message> {
    panel(
        column![
            text(title).size(12.5).color(MUTED),
            Space::new().height(12),
            content,
        ]
        .into(),
    )
}

/// A number with what it is above it and what it means below it.
///
/// The second line is the whole point. "42" says nothing; "42, five of seven
/// days active" says whether this was a busy week. The value is plain `FG`
/// rather than the accent - four accent numbers in a row is a row of alarms,
/// and the accent's one job in this product is "live".
pub(crate) fn stat_tile(
    label: &'static str,
    value: String,
    note: (String, Color),
) -> Element<'static, Message> {
    let (note, colour) = note;
    panel(
        column![
            text(label).size(12).color(MUTED),
            Space::new().height(9),
            text(value).size(25).color(FG),
            Space::new().height(5),
            text(note).size(11).color(colour),
        ]
        .into(),
    )
}

/// A small label with its value under it. Sized to its content: the row packs
/// these left with deliberate gaps rather than stretching each to an equal
/// share of the width.
pub(crate) fn fact(label: &'static str, value: String) -> Element<'static, Message> {
    column![
        text(label).size(11).color(FAINT),
        Space::new().height(4),
        text(value)
            .size(12.5)
            .font(Font::MONOSPACE)
            // Brighter than MUTED: the label above it is the quiet half of the
            // pair, and at the same weight neither reads as the answer.
            .color(mix(MUTED, FG, 0.4))
            .wrapping(text::Wrapping::None),
    ]
    .into()
}
