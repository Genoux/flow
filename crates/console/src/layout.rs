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
    PAGE_TOP, RADIUS, RAIL_ON, ROW_PAD, SCROLL_PAD,
};
use crate::{history, Message, Section};
use iced::widget::{button, column, container, responsive, row, scrollable, text, Space};
use iced::{Background, Border, Color, Element, Fill, Length};

// The copy slot remains in layout while its emphasis changes. List-level exit
// tracking avoids flicker when the pointer crosses a nested copy control.
pub(crate) fn entry_row<'a>(
    entry: &'a history::Entry,
    index: usize,
    now: u64,
    copied: bool,
    warmth: f32,
) -> Element<'a, Message> {
    let when = history::ago(entry.at, now);
    let when = if when.is_empty() {
        "Undated".to_owned()
    } else {
        when
    };
    let duration = history::duration(entry.spoken);
    let words = crate::format::plural(entry.text.split_whitespace().count() as u32, "word");
    let copy = copy_btn(index, copied, 0.65 + warmth * 0.35);
    let transcript = text(&entry.text).size(13).line_height(1.65).color(FG);
    // Same small print as the time and the word count, and in the same colour:
    // this answers "why does this one still have my stumbles in it", which is a
    // fact about the dictation rather than a fault to alarm anybody with.
    let cleanup = entry.cleanup.as_ref().map(|cleanup| {
        let label = text(format!("· {}", cleanup.label())).size(11).color(FAINT);
        match cleanup.detail() {
            // The guard that refused the model's answer, which is the whole
            // reason this line exists: "cleanup skipped" on its own leaves the
            // same question it was added to answer.
            Some(why) => Element::from(iced::widget::tooltip(
                label,
                container(text(why).size(12))
                    .padding(8)
                    .style(container::dark),
                iced::widget::tooltip::Position::Top,
            )),
            None => Element::from(label),
        }
    });
    let content = column![
        row![
            text(when).size(11).color(MUTED),
            text(format!("· {duration} · {words}"))
                .size(11)
                .color(FAINT),
        ]
        .extend(cleanup)
        .push(Space::new().width(Fill))
        .push(copy)
        .spacing(6)
        .align_y(iced::Center),
        Space::new().height(7),
        transcript,
    ];
    let body = container(content)
        .padding([14.0, ENTRY_INSET])
        .width(Fill)
        .style(move |_theme| container::Style {
            background: Some(Background::Color(mix(
                Color::TRANSPARENT,
                RAIL_ON,
                warmth * 0.45,
            ))),
            border: Border {
                radius: RADIUS.into(),
                ..Default::default()
            },
            ..Default::default()
        });

    iced::widget::mouse_area(body)
        .on_enter(Message::HoverEntry(Some(index)))
        .into()
}

/// The list, not the row, owns "pointer left". See `entry_row`.
pub(crate) fn entry_list<'a>(rows: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    iced::widget::mouse_area(rows)
        .on_exit(Message::HoverEntry(None))
        .into()
}

/// A scrollable with no bar at all.
///
/// It went from iced's permanent wide rail, to a 4px hairline that appeared on
/// hover, to nothing - each step for the same reason, which is that this window
/// is small and a bar is the one piece of chrome that is never about the page it
/// is on. The wheel does not need it: iced drives scrolling from
/// `cursor_over_scrollable`, so the bar is decoration, and `draw_scrollbar`
/// guards on `bounds.width > 0.0` for both the rail and the scroller - a zero
/// width is genuinely nothing drawn, not a transparent quad still being painted.
///
/// Zero rather than a transparent 4px, which was the other way to do it: a
/// transparent bar is still draggable, so the far right edge would scroll the
/// page when grabbed with nothing there to say why.
///
/// Top and bottom padding is on the content, not the pane: it is the air above
/// the heading and below the last row, and it scrolls with them. The right pad
/// is the page's own margin - it used to be partly for the bar, which iced
/// overlays on top of the content rather than beside it, and it stays because
/// the text still needs a margin.
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
    crate::smooth_scroll::vertical(
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
                .width(0)
                .margin(0)
                .scroller_width(0),
        ))
        .height(Fill),
    )
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
                    radius: RADIUS.into(),
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
                    radius: RADIUS.into(),
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
    description: impl Into<String>,
    control: Element<'a, Message>,
) -> Element<'a, Message> {
    setting_toned(label, description, FAINT, control)
}

/// `setting`, with the description carrying a colour.
///
/// A refused save and a rejected key are read in the description slot, and a
/// warning drawn in the same grey as every other subtitle is a warning nobody
/// sees - which is the whole failure this screen was fixed for.
pub(crate) fn setting_toned<'a>(
    label: &'a str,
    description: impl Into<String>,
    tone: Color,
    control: Element<'a, Message>,
) -> Element<'a, Message> {
    let description = description.into();
    let mut text_block = column![text(label).size(13.5).color(FG)];
    if !description.is_empty() {
        text_block = text_block.push(Space::new().height(LABEL_GAP));
        text_block = text_block.push(text(description).size(12).color(tone));
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
            text(value.into()).size(12).color(MUTED),
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
                // 8px per character is wider than the proportional face
                // actually draws at 12px, deliberately: the estimate has to
                // clip early rather than late, so a custom XDG path keeps the
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
    crate::interaction::hover(move |amount| {
        button(text(shown).size(12).wrapping(text::Wrapping::None))
            .padding(0)
            .on_press(Message::OpenPath(path))
            .style(move |_, _| button::Style {
                text_color: mix(MUTED, FG, amount.get()),
                ..Default::default()
            })
            .into()
    })
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
