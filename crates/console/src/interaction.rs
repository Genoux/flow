use crate::motion::Transition;
use crate::Message;
use iced::advanced::widget::{operation, tree, Operation, Tree};
use iced::advanced::{layout, mouse, overlay, renderer, Clipboard, Layout, Shell, Widget};
use iced::{Element, Event, Length, Rectangle, Size, Vector};
use std::cell::Cell;
use std::rc::Rc;
use std::time::Instant;

pub(crate) fn hover<'a>(
    build: impl FnOnce(Rc<Cell<f32>>) -> Element<'a, Message>,
) -> Element<'a, Message> {
    interactive(false, build)
}

pub(crate) fn field<'a>(
    build: impl FnOnce(Rc<Cell<f32>>) -> Element<'a, Message>,
) -> Element<'a, Message> {
    interactive(true, build)
}

fn interactive<'a>(
    focus: bool,
    build: impl FnOnce(Rc<Cell<f32>>) -> Element<'a, Message>,
) -> Element<'a, Message> {
    let value = Rc::new(Cell::new(0.0));
    let content = build(value.clone());
    Element::new(Interactive {
        content,
        value,
        focus,
    })
}

struct Interactive<'a> {
    content: Element<'a, Message>,
    value: Rc<Cell<f32>>,
    focus: bool,
}

impl Widget<Message, iced::Theme, iced::Renderer> for Interactive<'_> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<Transition>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(Transition::new(0.0))
    }
    fn children(&self) -> Vec<Tree> {
        vec![Tree::new(&self.content)]
    }
    fn diff(&self, tree: &mut Tree) {
        tree.diff_children(std::slice::from_ref(&self.content));
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
        self.content
            .as_widget_mut()
            .layout(&mut tree.children[0], renderer, limits)
    }
    fn draw(
        &self,
        tree: &Tree,
        renderer: &mut iced::Renderer,
        theme: &iced::Theme,
        style: &renderer::Style,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
    ) {
        self.value.set(
            tree.state
                .downcast_ref::<Transition>()
                .value(Instant::now()),
        );
        self.content.as_widget().draw(
            &tree.children[0],
            renderer,
            theme,
            style,
            layout,
            cursor,
            viewport,
        );
    }
    fn update(
        &mut self,
        tree: &mut Tree,
        event: &Event,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        renderer: &iced::Renderer,
        clipboard: &mut dyn Clipboard,
        shell: &mut Shell<'_, Message>,
        viewport: &Rectangle,
    ) {
        self.content.as_widget_mut().update(
            &mut tree.children[0],
            event,
            layout,
            cursor,
            renderer,
            clipboard,
            shell,
            viewport,
        );
        let mut focused = Focus(false);
        if self.focus {
            self.content.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                renderer,
                &mut focused,
            );
        }
        let hovered = cursor.is_over(layout.bounds()) && cursor.is_over(*viewport);
        let target = if focused.0 {
            1.0
        } else if hovered {
            if self.focus {
                0.5
            } else {
                1.0
            }
        } else {
            0.0
        };
        let now = Instant::now();
        let transition = tree.state.downcast_mut::<Transition>();
        transition.set(target, now);
        self.value.set(transition.value(now));
        if transition.moving(now) {
            shell.request_redraw();
        }
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
            .operate(&mut tree.children[0], layout, renderer, operation);
    }
    fn mouse_interaction(
        &self,
        tree: &Tree,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        viewport: &Rectangle,
        renderer: &iced::Renderer,
    ) -> mouse::Interaction {
        self.content.as_widget().mouse_interaction(
            &tree.children[0],
            layout,
            cursor,
            viewport,
            renderer,
        )
    }
    fn overlay<'b>(
        &'b mut self,
        tree: &'b mut Tree,
        layout: Layout<'b>,
        renderer: &iced::Renderer,
        viewport: &Rectangle,
        translation: Vector,
    ) -> Option<overlay::Element<'b, Message, iced::Theme, iced::Renderer>> {
        self.content.as_widget_mut().overlay(
            &mut tree.children[0],
            layout,
            renderer,
            viewport,
            translation,
        )
    }
}

struct Focus(bool);
impl Operation for Focus {
    fn traverse(&mut self, operate: &mut dyn FnMut(&mut dyn Operation)) {
        operate(self);
    }
    fn focusable(
        &mut self,
        _: Option<&iced::widget::Id>,
        _: Rectangle,
        state: &mut dyn operation::Focusable,
    ) {
        self.0 |= state.is_focused();
    }
}
