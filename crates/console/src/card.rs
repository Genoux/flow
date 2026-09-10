//! Shared surfaces and editorial data blocks.

use crate::theme::{dissolve, mix, EDGE, FG, HAIRLINE, LABEL, MICRO, MUTED, RAISED};
use crate::Message;
use iced::widget::{column, container, text, Space};
use iced::{Background, Border, Color, Element, Fill};

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
                color: dissolve(mix(RAISED, EDGE, 0.5), fade),
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

pub(crate) fn card<'a>(title: &'a str, content: Element<'a, Message>) -> Element<'a, Message> {
    container(column![
        text(title).size(LABEL).color(MUTED),
        Space::new().height(12),
        content,
    ])
    .width(Fill)
    .into()
}

pub(crate) fn stat_tile(
    label: &'static str,
    value: String,
    note: (String, Color),
) -> Element<'static, Message> {
    let (note, colour) = note;
    container(column![
        text(label).size(MICRO).color(MUTED),
        Space::new().height(9),
        text(value).size(32).color(FG),
        Space::new().height(5),
        text(note).size(MICRO).color(colour),
    ])
    .width(Fill)
    .into()
}
