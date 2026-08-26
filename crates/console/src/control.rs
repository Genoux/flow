//! The controls a row can hold, and the rules between rows.
//!
//! Hand-built rather than iced's own where the difference is motion: a
//! toggle that redraws instantly from its boolean has nowhere to put a
//! position, and the travel is the part that acknowledges the click.

use crate::theme::{
    dissolve, mix, ACCENT, BG, CONTROL_PAD, CONTROL_TEXT, EDGE, FAINT, FG, HAIRLINE, LINE, MUTED,
    ON_ACCENT, RADIUS, RAIL_ON,
};
use crate::Message;
use iced::widget::{button, canvas, column, container, row, slider, text, Canvas, Space};
use iced::{Background, Border, Color, Element, Fill, Font, Length, Point, Size, Theme};

/// The border every secondary control wears.
///
/// This is the whole of such a control's chrome - there is no fill to change -
/// which is why it is also the whole of its hover state. Shared between the
/// buttons and the dropdown so the two cannot drift apart, which they did the
/// first time the dropdown was styled on its own and arrived with a grey box
/// behind it that nothing else in the window has.
fn control_border(colour: Color) -> Border {
    Border {
        color: colour,
        width: HAIRLINE,
        radius: RADIUS.into(),
    }
}

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
/// The switch, in sizes rather than in corners.
///
/// Both radii here are written as half a height because both shapes are round
/// by construction - a circle in a pill - and not because a corner was chosen.
/// They are deliberately *not* `RADIUS`: a later pass unifying the window's
/// corner radii would land on these two `6.0`s and `9.0`s, and squaring off the
/// switch is not what that pass would have meant to do.
const KNOB_SIZE: f32 = 12.0;
const TRACK_WIDTH: f32 = 34.0;
const TRACK_HEIGHT: f32 = 18.0;

pub(crate) fn toggle(
    value: bool,
    travel: f32,
    on_change: fn(bool) -> Message,
) -> Element<'static, Message> {
    let at = if value { travel } else { 1.0 - travel };
    let left = (at * 1000.0) as u16;

    let knob = container(Space::new())
        .width(Length::Fixed(KNOB_SIZE))
        .height(Length::Fixed(KNOB_SIZE))
        .style(move |_| container::Style {
            background: Some(Background::Color(mix(MUTED, ON_ACCENT, at))),
            border: Border {
                radius: (KNOB_SIZE / 2.0).into(),
                ..Default::default()
            },
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
    .width(Length::Fixed(TRACK_WIDTH))
    .height(Length::Fixed(TRACK_HEIGHT))
    .padding([3, 3])
    .style(move |_| container::Style {
        background: Some(Background::Color(mix(LINE, ACCENT, at))),
        border: Border {
            radius: (TRACK_HEIGHT / 2.0).into(),
            ..Default::default()
        },
        ..Default::default()
    });

    button(track)
        .padding(0)
        .style(ghost)
        .on_press(on_change(!value))
        .into()
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
        container(
            slider(range, value, on_change)
                .height(14)
                .style(|_theme, _status| {
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
                })
        )
        .width(Length::Fixed(140.0)),
        Space::new().width(12),
        container(
            text(label.to_string())
                .size(12)
                .font(Font::MONOSPACE)
                .color(MUTED)
        )
        .width(Length::Fixed(56.0))
        .align_x(iced::alignment::Horizontal::Right),
    ]
    .align_y(iced::Center)
    .into()
}

/// One choosable thing on its own line, for the microphone dialog.
///
/// The rail's language, reused: `RAIL_ON` behind the current one, and hover
/// approaching that fill without arriving at it, so "this is the one you are
/// using" and "this is the one you are about to click" never read as the same
/// state. Which is the whole reason this exists instead of a `pick_list` menu -
/// iced's menu has one selection fill and gives it to whatever the pointer is
/// over, so it can only ever show the second of those two things.
///
/// `note` is the second line, and "" means the row has none rather than an
/// empty one - the same rule `setting` follows, for the same reason: a device
/// with nothing to explain should not be taller than its neighbours.
pub(crate) fn option_row(
    title: &str,
    note: String,
    current: bool,
    fade: f32,
    on_press: Message,
) -> Element<'static, Message> {
    let ink = if current { FG } else { MUTED };
    let mut block = column![text(title.to_string())
        .size(13)
        .color(dissolve(ink, fade))
        .wrapping(text::Wrapping::None)];
    if !note.is_empty() {
        block = block.push(Space::new().height(3));
        // `MUTED`, not the `FAINT` a second line usually gets. `FAINT` is sized
        // for 11px meta - a timestamp, a month - and it sits at 3.07:1 on the
        // `RAIL_ON` the current row is painted with. A note here says why a row
        // is not what it looks like - that the pinned microphone is unplugged -
        // so it is text rather than decoration and gets a colour that clears
        // the bar: 4.60:1 on that fill, 5.59:1 on the panel.
        block = block.push(text(note).size(11.5).color(dissolve(MUTED, fade)));
    }

    // The mark, not the fill, is what says which microphone is on. `RAIL_ON`
    // over the panel is 1.23:1, and it is also what hover paints, so state and
    // cursor said the same thing in the same material and every row swept past
    // impersonated the current one. A dot in the green the toggles and sliders
    // already mean "on" with says it once - and says it in chroma rather than a
    // near-black luminance step, which is the first thing a video encoder drops
    // on a streamed desktop. Trailing, so the labels keep the left edge they
    // share with the dialog title, and width-matched when absent so no row
    // shifts as the choice moves.
    let mark: Element<'static, Message> = if current {
        container(Space::new().width(6).height(6))
            .style(move |_theme| container::Style {
                background: Some(Background::Color(dissolve(ACCENT, fade))),
                border: Border {
                    radius: 3.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into()
    } else {
        Space::new().width(6).into()
    };

    // `Fill` on the block rather than a spacer between it and the mark. A
    // spacer is only what is left over, so a note wide enough to use the whole
    // row took it to zero and pushed the mark off the edge - which is how the
    // current row became the one row with no mark on it. Bounded, a long note
    // wraps instead, and the mark keeps its 6px whatever the device is called.
    button(
        row![container(block).width(Fill), mark]
            .spacing(8)
            .align_y(iced::Center),
    )
    .width(Fill)
        .padding([9, 11])
        .style(move |_theme, status| {
            let fill = if current {
                1.0
            } else if matches!(status, button::Status::Hovered) {
                0.55
            } else {
                0.0
            };
            button::Style {
                // `RAIL_ON` at `fill` alpha over the panel, which is the same
                // colour `mix` gave at rest and nothing at all once the dialog
                // is gone - a row walked to `RAISED` would have stayed a solid
                // bar inside a panel that had already left.
                background: Some(Background::Color(dissolve(RAIL_ON, fill * fade))),
                text_color: dissolve(ink, fade),
                border: Border {
                    radius: RADIUS.into(),
                    ..Default::default()
                },
                ..Default::default()
            }
        })
        .on_press(on_press)
        .into()
}

/// The × that shuts a dialog. `ghost`, because a close button is the one
/// control in a dialog that should not compete with what the dialog is asking.
pub(crate) fn close_btn(fade: f32) -> Element<'static, Message> {
    button(text("\u{00d7}").size(17).color(dissolve(MUTED, fade)))
        .padding([0, 4])
        .style(move |_theme, status| button::Style {
            text_color: dissolve(
                if matches!(status, button::Status::Hovered) {
                    FG
                } else {
                    MUTED
                },
                fade,
            ),
            ..ghost(&Theme::Dark, status)
        })
        .on_press(Message::ClosePicker)
        .into()
}

/// A 7px dot. The only place the accent appears besides a primary button.
pub(crate) fn pip(colour: Color) -> Element<'static, Message> {
    container(Space::new().width(0))
        .width(Length::Fixed(7.0))
        .height(Length::Fixed(7.0))
        .style(move |_| container::Style {
            background: Some(Background::Color(colour)),
            border: Border {
                radius: 3.5.into(),
                ..Default::default()
            },
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

pub(crate) fn action_msg(
    label: &str,
    primary: bool,
    on_press: impl Into<Option<Message>>,
) -> Element<'static, Message> {
    action_faded(label, primary, 1.0, on_press)
}

pub(crate) fn action_faded(
    label: &str,
    primary: bool,
    fade: f32,
    on_press: impl Into<Option<Message>>,
) -> Element<'static, Message> {
    let on_press = on_press.into();
    let ink = if on_press.is_none() {
        FAINT
    } else if primary {
        ON_ACCENT
    } else {
        FG
    };
    button(
        text(label.to_string())
            .size(CONTROL_TEXT)
            .color(crate::theme::emerge(ink, fade))
            // A button is as wide as its label, full stop. Left to wrap, a
            // "Download" beside a long path folded onto two lines and then
            // clipped, because the row had already given the path every pixel
            // it asked for.
            .wrapping(text::Wrapping::None),
    )
    .padding(CONTROL_PAD)
    .style(move |_theme, status| {
        let paint = |colour: Color| crate::theme::emerge(colour, fade);
        if matches!(status, button::Status::Disabled) {
            let fill = paint(mix(ACCENT, BG, 0.62));
            return button::Style {
                background: primary.then_some(Background::Color(fill)),
                text_color: paint(FAINT),
                border: control_border(if primary { fill } else { paint(LINE) }),
                ..Default::default()
            };
        }
        let hovered = matches!(status, button::Status::Hovered);
        let primary_fill = paint(match status {
            button::Status::Hovered => mix(ACCENT, FG, 0.12),
            button::Status::Pressed => mix(ACCENT, ON_ACCENT, 0.16),
            _ => ACCENT,
        });
        button::Style {
            background: primary.then_some(Background::Color(primary_fill)),
            text_color: paint(if primary { ON_ACCENT } else { FG }),
            border: control_border(if primary {
                primary_fill
            } else if hovered {
                paint(FAINT)
            } else {
                paint(LINE)
            }),
            ..Default::default()
        }
    })
    .on_press_maybe(if fade > 0.5 { on_press } else { None })
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
            Canvas::new(CopyMark {
                colour: Color {
                    a: opacity,
                    ..colour
                },
                checked: copied,
            })
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
