//! The controls a row can hold, and the rules between rows.
//!
//! Hand-built rather than iced's own where the difference is motion: a
//! toggle that redraws instantly from its boolean has nowhere to put a
//! position, and the travel is the part that acknowledges the click.

use crate::theme::{mix, ACCENT, EDGE, ERR, FAINT, FG, LINE, MUTED, ON_ACCENT};
use crate::Message;
use iced::widget::{button, canvas, container, row, slider, text, Canvas, Space};
use iced::{Background, Border, Color, Element, Fill, Font, Length, Point, Size, Theme};

/// A toggle whose knob travels rather than teleports.
///
/// Built by hand because iced's toggler redraws instantly from its boolean and
/// has nowhere to put a position. The knob is placed by two flexible spaces
/// either side of it, so its travel is just the ratio between them, and the
/// track colour crosses over on the same curve.
///
/// `travel` is 0 at the moment of the click and 1 when it has arrived; the
/// knob moves toward `value` over that, so a toggle flipped back mid-flight
/// simply reverses.
pub(crate) fn toggle(value: bool, travel: f32, on_change: fn(bool) -> Message) -> Element<'static, Message> {
    let at = if value { travel } else { 1.0 - travel };
    let left = (at * 1000.0) as u16;

    let knob = container(Space::new())
        .width(Length::Fixed(12.0))
        .height(Length::Fixed(12.0))
        .style(move |_| container::Style {
            background: Some(Background::Color(mix(MUTED, ON_ACCENT, at))),
            border: Border { radius: 6.0.into(), ..Default::default() },
            ..Default::default()
        });

    let track = container(
        row![
            Space::new().width(Length::FillPortion(left.max(1))),
            knob,
            Space::new().width(Length::FillPortion((1000 - left).max(1))),
        ]
        .align_y(iced::Center),
    )
    .width(Length::Fixed(34.0))
    .height(Length::Fixed(18.0))
    .padding([3, 3])
    .style(move |_| container::Style {
        background: Some(Background::Color(mix(LINE, ACCENT, at))),
        border: Border { radius: 9.0.into(), ..Default::default() },
        ..Default::default()
    });

    button(track).padding(0).style(ghost).on_press(on_change(!value)).into()
}

/// A slider with its value beside it, so the number is always readable rather
/// than something you infer from a handle position.
pub(crate) fn value_slider<'a>(
    range: std::ops::RangeInclusive<u32>,
    value: u32,
    on_change: fn(u32) -> Message,
    label: &str,
) -> Element<'a, Message> {
    row![
        container(slider(range, value, on_change).height(14).style(|_theme, _status| {
            slider::Style {
                rail: slider::Rail {
                    backgrounds: (Background::Color(ACCENT), Background::Color(LINE)),
                    width: 2.0,
                    border: Border::default(),
                },
                handle: slider::Handle {
                    shape: slider::HandleShape::Circle { radius: 5.0 },
                    background: Background::Color(FG),
                    border_width: 0.0,
                    border_color: Color::TRANSPARENT,
                },
            }
        }))
        .width(Length::Fixed(140.0)),
        Space::new().width(12),
        container(text(label.to_string()).size(12).font(Font::MONOSPACE).color(MUTED))
            .width(Length::Fixed(56.0))
            .align_x(iced::alignment::Horizontal::Right),
    ]
    .align_y(iced::Center)
    .into()
}

/// A filling bar, 0 to 1.
///
/// Built from two flexible spaces like the toggle's knob rather than from a
/// measured width, so it fills whatever column it is given without anyone
/// having to tell it how wide that is.
///
/// The fill keeps a minimum portion *while it is working*: at a genuine zero
/// the rounded cap would collapse to nothing and the bar would read as a track
/// that had not started, when in fact it has - the first bytes of a 3 GB file
/// just do not show up as width yet. A stalled bar gets no such floor, because
/// there the sliver would be claiming progress that is not happening.
pub(crate) fn meter(fraction: f32, stalled: bool) -> Element<'static, Message> {
    let filled = (fraction.clamp(0.0, 1.0) * 1000.0) as u16;
    let filled = if stalled { filled } else { filled.max(6) };
    let colour = if stalled { ERR } else { ACCENT };

    container(
        row![
            container(Space::new().height(Fill))
                .width(Length::FillPortion(filled))
                .height(Fill)
                .style(move |_| container::Style {
                    background: Some(Background::Color(colour)),
                    border: Border { radius: 2.0.into(), ..Default::default() },
                    ..Default::default()
                }),
            Space::new().width(Length::FillPortion((1000 - filled).max(1))),
        ],
    )
    .width(Fill)
    .height(Length::Fixed(4.0))
    .style(|_| container::Style {
        background: Some(Background::Color(LINE)),
        border: Border { radius: 2.0.into(), ..Default::default() },
        ..Default::default()
    })
    .into()
}

/// A 7px dot. The only place the accent appears besides a primary button.
pub(crate) fn pip(colour: Color) -> Element<'static, Message> {
    container(Space::new().width(0))
        .width(Length::Fixed(7.0))
        .height(Length::Fixed(7.0))
        .style(move |_| container::Style {
            background: Some(Background::Color(colour)),
            border: Border { radius: 3.5.into(), ..Default::default() },
            ..Default::default()
        })
        .into()
}

pub(crate) fn vertical_hairline() -> Element<'static, Message> {
    container(Space::new().height(Fill))
        .width(Length::Fixed(1.0))
        .height(Fill)
        .style(|_| container::Style {
            background: Some(Background::Color(LINE)),
            ..Default::default()
        })
        .into()
}

pub(crate) fn hairline() -> Element<'static, Message> {
    rule(LINE)
}

/// The same rule, in the only colour that is visible on a card.
pub(crate) fn card_rule() -> Element<'static, Message> {
    rule(EDGE)
}

fn rule(colour: Color) -> Element<'static, Message> {
    container(Space::new().width(Fill))
        .height(Length::Fixed(1.0))
        .style(move |_| container::Style {
            background: Some(Background::Color(colour)),
            ..Default::default()
        })
        .into()
}

pub(crate) fn action_msg(label: &str, primary: bool, on_press: Message) -> Element<'static, Message> {
    button(
        text(label.to_string())
            .size(13)
            .color(if primary { ON_ACCENT } else { FG })
            // A button is as wide as its label, full stop. Left to wrap, an
            // "Install models" beside a long path folded onto two lines and
            // then clipped, because the row had already given the path every
            // pixel it asked for.
            .wrapping(text::Wrapping::None),
    )
    .padding([7, 14])
    .style(move |_theme, status| {
        let hovered = matches!(status, button::Status::Hovered);
        button::Style {
            background: primary.then_some(Background::Color(ACCENT)),
            text_color: if primary { ON_ACCENT } else { FG },
            border: Border {
                color: if primary {
                    ACCENT
                } else if hovered {
                    FAINT
                } else {
                    LINE
                },
                width: 1.0,
                radius: 6.0.into(),
            },
            ..Default::default()
        }
    })
    .on_press(on_press)
    .into()
}

/// An action with no chrome at all - a label that brightens under the pointer.
///
/// For the one place a control has to be available without being offered:
/// setup's skip. A bordered button there competes with the primary action and
/// reads as a fork in the road, when the honest shape is "carry on, unless you
/// would rather not".
pub(crate) fn quiet_action(label: &str, on_press: Message) -> Element<'static, Message> {
    button(text(label.to_string()).size(12).wrapping(text::Wrapping::None))
        .padding([4, 6])
        .style(|_theme, status| button::Style {
            background: None,
            text_color: match status {
                button::Status::Hovered | button::Status::Pressed => FG,
                _ => MUTED,
            },
            border: Border::default(),
            ..Default::default()
        })
        .on_press(on_press)
        .into()
}

/// Text that behaves like a link: no chrome at all, just the label.
fn ghost(_theme: &Theme, _status: button::Status) -> button::Style {
    button::Style {
        background: None,
        text_color: FG,
        border: Border::default(),
        ..Default::default()
    }
}

/// The slot copy sits in, on the meta line of a transcript. Fixed size at
/// every warmth so hovering never changes the row's height (see
/// `entry_row`); only the icon's opacity moves, using the row's own fade so
/// it settles in lockstep with the highlight rather than snapping in on top
/// of it.
///
/// A paper, not a word: drawn rather than picked from a font so it matches
/// the rest of the chrome instead of arriving as someone else's glyph. Copied
/// swaps the glyph itself to a checkmark, full opacity and accent - a colour
/// change alone on the same paper reads as "this button is now green", not as
/// "this worked".
///
/// A `mouse_area`, not a `button` and not a tooltip: both of those captured
/// the pointer event the parent row needs to stay hovered, and the tooltip
/// invalidated layout as it opened, which is what made the highlight drop
/// whenever the pointer reached the icon.
const COPY_SLOT: f32 = 20.0;
const COPY_GLYPH: f32 = 13.0;

pub(crate) fn copy_btn(index: usize, copied: bool, warmth: f32) -> Element<'static, Message> {
    let opacity = if copied { 1.0 } else { warmth };
    let colour = if copied { ACCENT } else { MUTED };
    iced::widget::mouse_area(
        container(
            Canvas::new(CopyMark { colour: Color { a: opacity, ..colour }, checked: copied })
                .width(Length::Fixed(COPY_GLYPH))
                .height(Length::Fixed(COPY_GLYPH)),
        )
        .width(Length::Fixed(COPY_SLOT))
        .height(Length::Fixed(COPY_SLOT))
        .align_x(iced::alignment::Horizontal::Center)
        .align_y(iced::alignment::Vertical::Center),
    )
    .on_press(Message::Copy(index))
    .interaction(iced::mouse::Interaction::Pointer)
    .into()
}

/// Two sheets, one offset, for "copy"; a single tick for "copied". Stroke
/// only, so it thins and fades with whatever colour and alpha it is asked
/// for rather than filling a blob that fights the rest of the row.
struct CopyMark {
    colour: Color,
    checked: bool,
}

impl canvas::Program<Message> for CopyMark {
    type State = ();

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &Theme,
        bounds: iced::Rectangle,
        _cursor: iced::mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let stroke = canvas::Stroke::default()
            .with_width(0.85)
            .with_color(self.colour)
            .with_line_cap(canvas::LineCap::Round)
            .with_line_join(canvas::LineJoin::Round);

        // Built from the canvas's own size, as fractions, rather than points
        // tuned by eye for one fixed box: that's what let the two sheets sit
        // 1px low the first time round, and would do it again the next time
        // `COPY_GLYPH` changed. The sheets are placed by an equal-and-opposite
        // offset from the centre on each axis, which centres the combined
        // shape by construction - not by re-measuring its bounding box.
        let (w, h) = (bounds.width, bounds.height);
        let (cx, cy) = (w / 2.0, h / 2.0);

        if self.checked {
            let mut check = canvas::path::Builder::new();
            check.move_to(Point::new(0.127 * w, 0.527 * h));
            check.line_to(Point::new(0.382 * w, 0.800 * h));
            check.line_to(Point::new(0.873 * w, 0.200 * h));
            frame.stroke(&check.build(), stroke);
        } else {
            let (sw, sh) = (0.58 * w, 0.68 * h);
            let (dx, dy) = (0.12 * w, 0.12 * h);
            let radius = 0.12 * w.min(h);
            frame.stroke(
                &canvas::Path::rounded_rectangle(
                    Point::new(cx - sw / 2.0 + dx, cy - sh / 2.0 - dy),
                    Size::new(sw, sh),
                    radius.into(),
                ),
                stroke,
            );
            frame.stroke(
                &canvas::Path::rounded_rectangle(
                    Point::new(cx - sw / 2.0 - dx, cy - sh / 2.0 + dy),
                    Size::new(sw, sh),
                    radius.into(),
                ),
                stroke,
            );
        }
        vec![frame.into_geometry()]
    }
}
