//! The shapes a page is assembled from: the scroll it lives in, its heading,
//! the rail beside it, and the rows that go in a list.
//!
//! Every settings screen is the same handful of these, which is the point -
//! a row that knows its own padding is a row that cannot be indented by
//! half a step on one screen and not the next.

use crate::control::{copy_btn, hairline};
use crate::format::{clip_tail, display_path};
use crate::theme::{
    mix, BG, CONTENT_RIGHT, ENTRY_INSET, FAINT, FG, GROUP_GAP, GROUP_PAD, LABEL_GAP, MUTED,
    PAGE_TOP, RAIL_ON, ROW_PAD, SCROLL_PAD,
};
use crate::{history, Message, Section};
use iced::widget::{
    button, column, container, responsive, rich_text, row, scrollable, span, text, Space,
};
use iced::{Background, Border, Color, Element, Fill, Font, Length};

/// One transcript and what it cost: the line itself, then how long it took to
/// say and how long ago that was. Shared so the Overview's excerpt and the
/// History log are the same row rather than two rows that look alike.
///
/// Copy is an icon on the row, not a label and not a hidden editor. iced draws
/// a caret in the text colour on any focused `text_editor`, and there is no
/// way to style it off - so a click that was meant to select a sentence parked
/// a blinking cursor in the middle of a settings window.
///
/// The icon's slot is always in the layout, at a fixed size, and only its
/// opacity moves with the hover - it never appears or disappears as an
/// element. Swapping a `Space` in for it while hidden (the first version of
/// this) changed the row's height between the two states, which under a
/// stationary pointer flips which row it is over and made the highlight
/// flicker between rows rather than hold still on one.
///
/// The transparent gap lives inside the hover target so moving between rows
/// cannot drop the highlight.
///
/// Rows only *set* the hovered index. Clearing it is `entry_list`'s job: a
/// nested copy control used to capture the event iced's `mouse_area` needs
/// to see, so `on_exit` fired while the pointer was still on the row, and
/// crossing from one row onto the next went through `None` and dropped the
/// highlight.
pub(crate) fn entry_row<'a>(
    entry: &'a history::Entry,
    index: usize,
    now: u64,
    copied: bool,
    warmth: f32,
    separated: bool,
) -> Element<'a, Message> {
    let when = history::ago(entry.at, now);
    let body = container(
        row![
            column![
                text(&entry.text).size(13).color(FG),
                Space::new().height(4),
                text(if when.is_empty() {
                    format!("{:.1}s", entry.spoken)
                } else {
                    format!("{:.1}s  ·  {when}", entry.spoken)
                })
                .size(11)
                .font(Font::MONOSPACE)
                .color(FAINT),
            ]
            .width(Fill),
            copy_btn(index, copied, warmth),
        ]
        .align_y(iced::Center),
    )
    .padding([10.0, ENTRY_INSET])
    .width(Fill)
    .style(move |_theme| container::Style {
        background: Some(Background::Color(mix(
            Color::TRANSPARENT,
            RAIL_ON,
            warmth * 0.7,
        ))),
        border: Border {
            radius: 6.0.into(),
            ..Default::default()
        },
        ..Default::default()
    });

    let mut stack = column![body].width(Fill);
    if separated {
        stack = stack.push(Space::new().height(2));
    }

    iced::widget::mouse_area(stack)
        .on_enter(Message::HoverEntry(Some(index)))
        .into()
}

/// The list, not the row, owns "pointer left". See `entry_row`.
pub(crate) fn entry_list<'a>(rows: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    iced::widget::mouse_area(rows)
        .on_exit(Message::HoverEntry(None))
        .into()
}

/// A scrollable with a thin, browser-style bar: invisible until the pointer
/// is over it, a faint hairline while hovered. Iced's default is a wide rail
/// that sits there permanently, which reads as chrome in a window this small.
///
/// Top and bottom padding is on the content, not the pane: it is the air
/// above the heading and below the last row, and it scrolls with them. The
/// right pad is for the bar itself, which iced overlays on top of the
/// content rather than beside it.
pub(crate) fn scroll<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    scroll_pad(content, PAGE_TOP)
}

fn scroll_pad<'a>(content: impl Into<Element<'a, Message>>, bottom: f32) -> Element<'a, Message> {
    scroll_inset(content, bottom, CONTENT_RIGHT)
}

pub(crate) fn scroll_inset<'a>(
    content: impl Into<Element<'a, Message>>,
    bottom: f32,
    right: f32,
) -> Element<'a, Message> {
    scrollable(
        container(content).padding(
            iced::Padding::default()
                .top(PAGE_TOP)
                .bottom(bottom)
                .right(right),
        ),
    )
    .direction(scrollable::Direction::Vertical(
        scrollable::Scrollbar::new()
            .width(4)
            .margin(2)
            .scroller_width(4),
    ))
    .style(|theme, status| {
        let base = scrollable::default(theme, status);
        let scroller_colour = match status {
            scrollable::Status::Active { .. } => Color::TRANSPARENT,
            _ => FAINT,
        };
        scrollable::Style {
            vertical_rail: scrollable::Rail {
                background: None,
                scroller: scrollable::Scroller {
                    background: scroller_colour.into(),
                    ..base.vertical_rail.scroller
                },
                ..base.vertical_rail
            },
            ..base
        }
    })
    .height(Fill)
    .into()
}

/// A page's heading. Lives in the scroll with the rest of the page, so a
/// short window can give the rows the room instead of keeping a title parked
/// over them. The top inset is on `scroll`'s content, so every page starts
/// on the same line and that air is still there when you scroll back up.
/// An empty subtitle is a page whose title says the whole thing, and it takes
/// no room at all - not a blank line under the title. Most pages here are in
/// that shape now, so the gap has to go with the sentence rather than being
/// held open for one that is not coming.
pub(crate) fn heading<'a>(title: &'a str, subtitle: &'a str) -> Element<'a, Message> {
    let mut block = column![text(title).size(22).color(FG)];
    if !subtitle.is_empty() {
        block = block.push(Space::new().height(10));
        block = block.push(text(subtitle).size(13).color(MUTED));
    }
    block.push(Space::new().height(SCROLL_PAD)).into()
}

/// Every settings screen is the same shape: a heading and a list that
/// scroll together, and a footer docked to the pane. The title yields its
/// room in a short window; the path and its action do not ride under the
/// last row.
pub(crate) fn section_shell<'a>(
    title: &'a str,
    subtitle: &'a str,
    rows: Vec<Element<'a, Message>>,
) -> Element<'a, Message> {
    page_shell(title, subtitle, hairlined(rows))
}

/// Rows with a rule between each pair and none at either end.
fn hairlined<'a>(rows: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    let mut list = column![];
    let count = rows.len();
    for (index, entry) in rows.into_iter().enumerate() {
        list = list.push(entry);
        if index + 1 < count {
            list = list.push(hairline());
        }
    }
    list.into()
}

/// A labelled block of rows, for a page that edits more than one thing.
///
/// The label is what makes a long settings page readable: without it the rows
/// are one undifferentiated list and you have to read every one to find the
/// section you wanted. Groups are told apart by air and a label rather than by
/// a rule, because a rule between groups reads as just another row boundary -
/// which is the one thing it must not look like.
pub(crate) fn group<'a>(label: &'a str, rows: Vec<Element<'a, Message>>) -> Element<'a, Message> {
    column![
        Space::new().height(GROUP_PAD),
        text(label).size(11.5).color(MUTED),
        Space::new().height(GROUP_GAP),
        hairlined(rows),
    ]
    .into()
}

/// The shape every settings screen shares: a heading and a body that scroll
/// together. The title yields its room in a short window.
///
/// There used to be a footer docked under the pane, and the last thing left in
/// it was "Saved. Applies to your next dictation." after every toggle - a bar
/// that appeared to congratulate the user for a switch they had just watched
/// move. A save that worked needs no announcement. The one thing worth saying,
/// a save that did not, is on Overview with the other faults.
pub(crate) fn page_shell<'a>(
    title: &'a str,
    subtitle: &'a str,
    content: Element<'a, Message>,
) -> Element<'a, Message> {
    scroll(column![heading(title, subtitle), content])
}

/// A rail item behaves like a button, because it is one: the whole row lights,
/// not just its label. Selection holds a permanent muted background so the
/// current section is legible at a glance, and hover raises any row toward the
/// same treatment - so the thing under the pointer looks like the thing that
/// would happen if you clicked.
///
/// `warmth` is how far into the hover this row is, 0 to 1.
pub(crate) fn nav(
    section: Section,
    selected: bool,
    warmth: f32,
    enabled: bool,
) -> Element<'static, Message> {
    // Disabled sits below rest, not above it: the point is that there is
    // nothing here yet, and a greyed item that lights up on hover is an item
    // still promising something.
    if !enabled {
        return button(text(section.label()).size(13).color(mix(BG, MUTED, 0.45)))
            .width(Fill)
            .padding([6, 9])
            .style(|_theme, _status| button::Style {
                background: None,
                text_color: mix(BG, MUTED, 0.45),
                border: Border {
                    radius: 6.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .into();
    }

    let colour = if selected { FG } else { mix(MUTED, FG, warmth) };
    // Selected sits at full weight; hover approaches it without arriving, so
    // the two never read as the same state. 0.7 rather than 0.55 because a
    // hover you have to look for is a hover that is not doing its job.
    let fill = if selected { 1.0 } else { warmth * 0.7 };

    iced::widget::mouse_area(
        button(text(section.label()).size(13).color(colour))
            .width(Fill)
            .padding([6, 9])
            .style(move |_theme, _status| button::Style {
                background: Some(Background::Color(mix(Color::TRANSPARENT, RAIL_ON, fill))),
                text_color: colour,
                border: Border {
                    radius: 6.0.into(),
                    ..Default::default()
                },
                ..Default::default()
            })
            .on_press(Message::Select(section)),
    )
    .on_enter(Message::Hover(Some(section)))
    .on_exit(Message::Hover(None))
    .into()
}

/// A label and its explanation on the left, the control on the right.
///
/// The description is optional, and passing "" means the row has none rather
/// than an empty one: a title is the setting, and a line under it is only
/// written where the title cannot carry the whole meaning. Held open, the gap
/// made a row with nothing to explain taller than the rows either side of it.
pub(crate) fn setting<'a>(
    label: &'a str,
    description: &'a str,
    control: Element<'a, Message>,
) -> Element<'a, Message> {
    let mut text_block = column![text(label).size(13.5).color(FG)];
    if !description.is_empty() {
        text_block = text_block.push(Space::new().height(LABEL_GAP));
        text_block = text_block.push(text(description).size(12).color(FAINT));
    }

    container(
        row![
            text_block.width(Length::FillPortion(3)),
            Space::new().width(20),
            container(control)
                .width(Length::FillPortion(2))
                .align_x(iced::alignment::Horizontal::Right),
        ]
        .align_y(iced::Center),
    )
    .padding([ROW_PAD, 0.0])
    .into()
}

/// A read-only pair, for About. Same rhythm as `setting` without a control.
pub(crate) fn fact_row(label: &'static str, value: impl Into<String>) -> Element<'static, Message> {
    container(
        row![
            text(label).size(13.5).color(FG),
            Space::new().width(Fill),
            text(value.into())
                .size(12)
                .font(Font::MONOSPACE)
                .color(MUTED),
        ]
        .align_y(iced::Center),
    )
    .padding([ROW_PAD, 0.0])
    .into()
}

/// Like `fact_row`, but the value is a path you can click to open in the
/// file manager. The type is the affordance; a second "Open" button would
/// repeat what the path already is.
///
/// The path is the shrinking half of the row: a long directory must not sit
/// on the label the way a short Session value can sit on a Fill. `~/…` is
/// what we draw; the real file is what a click reveals.
pub(crate) fn fact_path(label: &'static str, path: &std::path::Path) -> Element<'static, Message> {
    let real = path.to_path_buf();
    let shown = display_path(path);
    container(
        row![
            text(label).size(13.5).color(FG),
            Space::new().width(20),
            responsive(move |size| {
                // Default monospace at 12px is a little under 8px wide; the
                // extra room is the ellipsis, so a custom XDG path keeps the
                // filename instead of running off the pane.
                let chars = (size.width / 8.0).floor().max(8.0) as usize;
                container(path_link(real.clone(), clip_tail(&shown, chars)))
                    .width(Fill)
                    .align_x(iced::alignment::Horizontal::Right)
                    .into()
            })
            .height(Length::Shrink),
        ]
        .align_y(iced::Center),
    )
    .padding([ROW_PAD, 0.0])
    .into()
}

fn path_link(path: std::path::PathBuf, shown: String) -> Element<'static, Message> {
    // A rich-text link, not a button: iced already turns those into a
    // pointer and an underline on hover, which is the affordance a path
    // sitting where Session's value sits would otherwise lack.
    rich_text![span(shown)
        .size(12)
        .font(Font::MONOSPACE)
        .color(MUTED)
        .link(path)]
    .on_link_click(Message::OpenPath)
    .wrapping(text::Wrapping::None)
    .into()
}

/// A layer the pointer cannot reach.
///
/// `stack!` paints the setup overlay above the console, but iced keeps routing
/// the pointer to everything underneath it. Two things showed through: the rail
/// lit its rows under the veil, and the calendar's tooltip - an overlay, so
/// raised above the veil rather than hidden behind it - appeared over a screen
/// the console was not even showing.
///
/// iced's own `opaque` is the wrong tool for it. That captures button presses
/// and forwards everything else, including `overlay`, so it stops the clicks
/// and leaves the hover - and hover is the half that was visible.
///
/// So this swallows mouse events instead of forwarding them, draws and measures
/// its content as though the pointer were off the window entirely, and raises no
/// overlay of its own. Keyboard and window events still pass, because a layer
/// that cannot be clicked is not the same as one that has stopped existing.
pub(crate) fn inert<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    use iced::advanced::widget::{tree, Operation, Tree};
    use iced::advanced::{layout, mouse, overlay, renderer, Clipboard, Layout, Shell, Widget};
    use iced::{Event, Rectangle, Size, Vector};

    struct Inert<'a> {
        content: Element<'a, Message>,
    }

    impl Widget<Message, iced::Theme, iced::Renderer> for Inert<'_> {
        fn tag(&self) -> tree::Tag {
            self.content.as_widget().tag()
        }

        fn state(&self) -> tree::State {
            self.content.as_widget().state()
        }

        fn children(&self) -> Vec<Tree> {
            self.content.as_widget().children()
        }

        fn diff(&self, tree: &mut Tree) {
            self.content.as_widget().diff(tree);
        }

        fn size(&self) -> Size<Length> {
            self.content.as_widget().size()
        }

        fn size_hint(&self) -> Size<Length> {
            self.content.as_widget().size_hint()
        }

        fn layout(
            &mut self,
            tree: &mut Tree,
            renderer: &iced::Renderer,
            limits: &layout::Limits,
        ) -> layout::Node {
            self.content.as_widget_mut().layout(tree, renderer, limits)
        }

        fn draw(
            &self,
            tree: &Tree,
            renderer: &mut iced::Renderer,
            theme: &iced::Theme,
            style: &renderer::Style,
            layout: Layout<'_>,
            _cursor: mouse::Cursor,
            viewport: &Rectangle,
        ) {
            self.content.as_widget().draw(
                tree,
                renderer,
                theme,
                style,
                layout,
                mouse::Cursor::Unavailable,
                viewport,
            );
        }

        fn operate(
            &mut self,
            tree: &mut Tree,
            layout: Layout<'_>,
            renderer: &iced::Renderer,
            operation: &mut dyn Operation,
        ) {
            self.content
                .as_widget_mut()
                .operate(tree, layout, renderer, operation);
        }

        fn update(
            &mut self,
            tree: &mut Tree,
            event: &Event,
            layout: Layout<'_>,
            _cursor: mouse::Cursor,
            renderer: &iced::Renderer,
            clipboard: &mut dyn Clipboard,
            shell: &mut Shell<'_, Message>,
            viewport: &Rectangle,
        ) {
            if matches!(event, Event::Mouse(_) | Event::Touch(_)) {
                return;
            }
            self.content.as_widget_mut().update(
                tree,
                event,
                layout,
                mouse::Cursor::Unavailable,
                renderer,
                clipboard,
                shell,
                viewport,
            );
        }

        fn mouse_interaction(
            &self,
            _tree: &Tree,
            _layout: Layout<'_>,
            _cursor: mouse::Cursor,
            _viewport: &Rectangle,
            _renderer: &iced::Renderer,
        ) -> mouse::Interaction {
            mouse::Interaction::None
        }

        fn overlay<'b>(
            &'b mut self,
            _tree: &'b mut Tree,
            _layout: Layout<'b>,
            _renderer: &iced::Renderer,
            _viewport: &Rectangle,
            _translation: Vector,
        ) -> Option<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
            None
        }
    }

    Element::new(Inert {
        content: content.into(),
    })
}
