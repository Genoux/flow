use crate::Message;
use iced::advanced::widget::{operation, tree, Operation, Tree};
use iced::advanced::{layout, mouse, overlay, renderer, Clipboard, Layout, Shell, Widget};
use iced::{Element, Event, Length, Rectangle, Size, Vector};
use std::time::Instant;

const DURATION: f32 = 0.12;

pub(crate) fn vertical<'a>(content: impl Into<Element<'a, Message>>) -> Element<'a, Message> {
    Element::new(SmoothScroll {
        content: content.into(),
    })
}

struct SmoothScroll<'a> {
    content: Element<'a, Message>,
}

#[derive(Default)]
struct State {
    modifiers: iced::keyboard::Modifiers,
    motion: Option<Motion>,
}

#[derive(Clone, Copy)]
struct Motion {
    from: f32,
    target: f32,
    elapsed: f32,
    last_frame: Instant,
}

impl Motion {
    fn wheel(
        from: f32,
        pending: Option<Self>,
        delta: f32,
        maximum: f32,
        now: Instant,
    ) -> Option<Self> {
        let target = pending
            .filter(|motion| (motion.target - from).signum() == delta.signum())
            .map_or(from, |motion| motion.target);
        Self::toward(from, target + delta, maximum, now)
    }

    fn toward(from: f32, target: f32, maximum: f32, now: Instant) -> Option<Self> {
        let target = target.clamp(0.0, maximum);
        (from != target).then_some(Self {
            from,
            target,
            elapsed: 0.0,
            last_frame: now,
        })
    }

    fn advance(&mut self, now: Instant) -> f32 {
        let delta = now.saturating_duration_since(self.last_frame).as_secs_f32();
        self.last_frame = now;
        self.elapsed = (self.elapsed + delta.min(crate::FRAME_CAP)).min(DURATION);
        let amount = 1.0 - (1.0 - self.elapsed / DURATION).powi(3);
        self.from + (self.target - self.from) * amount
    }

    fn finished(self) -> bool {
        self.elapsed >= DURATION
    }
}

fn cancels_motion(event: &Event) -> bool {
    matches!(
        event,
        Event::Keyboard(_)
            | Event::Touch(_)
            | Event::Mouse(
                mouse::Event::ButtonPressed(_)
                    | mouse::Event::WheelScrolled {
                        delta: mouse::ScrollDelta::Pixels { .. }
                    }
            )
            | Event::Window(iced::window::Event::Unfocused | iced::window::Event::Resized(_))
    )
}

#[derive(Default)]
struct ScrollPosition {
    position: f32,
    maximum: f32,
    set: Option<f32>,
}

impl ScrollPosition {
    fn at(position: f32) -> Self {
        Self {
            set: Some(position),
            ..Self::default()
        }
    }
}

impl Operation for ScrollPosition {
    fn traverse(&mut self, _: &mut dyn FnMut(&mut dyn Operation)) {}

    fn scrollable(
        &mut self,
        _: Option<&iced::widget::Id>,
        bounds: Rectangle,
        content: Rectangle,
        translation: Vector,
        scroll: &mut dyn operation::Scrollable,
    ) {
        self.position = translation.y;
        self.maximum = (content.height - bounds.height).max(0.0);
        if let Some(position) = self.set {
            scroll.scroll_to(operation::scrollable::AbsoluteOffset {
                x: None,
                y: Some(position.clamp(0.0, self.maximum)),
            });
        }
    }
}

impl Widget<Message, iced::Theme, iced::Renderer> for SmoothScroll<'_> {
    fn tag(&self) -> tree::Tag {
        tree::Tag::of::<State>()
    }
    fn state(&self) -> tree::State {
        tree::State::new(State::default())
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
        let hovered = cursor.is_over(layout.bounds()) && cursor.is_over(*viewport);
        let state = tree.state.downcast_mut::<State>();
        if !hovered || cancels_motion(event) {
            state.motion = None;
        }
        if let Event::Keyboard(iced::keyboard::Event::ModifiersChanged(modifiers)) = event {
            state.modifiers = *modifiers;
        }
        if let Event::Window(iced::window::Event::RedrawRequested(now)) = event {
            if let Some(motion) = state.motion.as_mut() {
                let position = motion.advance(*now);
                let mut scroll = ScrollPosition::at(position);
                self.content.as_widget_mut().operate(
                    &mut tree.children[0],
                    layout,
                    renderer,
                    &mut scroll,
                );
                if motion.finished() {
                    state.motion = None;
                }
            }
        }
        let wheel = match event {
            Event::Mouse(mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines { y, .. },
            }) if hovered && state.modifiers == iced::keyboard::Modifiers::default() => Some(*y),
            _ => None,
        };
        let mut before = ScrollPosition::default();
        if wheel.is_some() {
            self.content.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                renderer,
                &mut before,
            );
        }
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
        if let Some(lines) = wheel {
            let mut after = ScrollPosition::default();
            self.content.as_widget_mut().operate(
                &mut tree.children[0],
                layout,
                renderer,
                &mut after,
            );
            if after.position != before.position
                || (state.motion.is_some() && shell.is_event_captured())
            {
                // iced 0.14's scrollable maps an unmodified wheel line to 60 pixels.
                state.motion = Motion::wheel(
                    before.position,
                    state.motion,
                    -lines * 60.0,
                    before.maximum,
                    Instant::now(),
                );
                let mut restore = ScrollPosition::at(before.position);
                self.content.as_widget_mut().operate(
                    &mut tree.children[0],
                    layout,
                    renderer,
                    &mut restore,
                );
            }
        }
        if state.motion.is_some() {
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
        tree.state.downcast_mut::<State>().motion = None;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn a_wheel_step_reaches_its_exact_distance_without_overshooting() {
        let start = Instant::now();
        let mut motion = Motion::toward(20.0, 80.0, 1000.0, start).unwrap();
        let mut previous = 20.0;
        for frame in 1..=8 {
            let position = motion.advance(start + Duration::from_millis(frame * 16));
            assert!(position >= previous && position <= 80.0);
            previous = position;
        }
        assert_eq!(previous, 80.0);
        assert!(motion.finished());
    }

    #[test]
    fn repeated_wheel_steps_accumulate_the_full_distance() {
        let start = Instant::now();
        let mut first = Motion::toward(0.0, 60.0, 1000.0, start).unwrap();
        let now = start + Duration::from_millis(16);
        let visible = first.advance(now);
        let mut second = Motion::toward(visible, first.target + 60.0, 1000.0, now).unwrap();
        let mut position = visible;
        for frame in 1..=8 {
            position = second.advance(now + Duration::from_millis(frame * 16));
        }
        assert_eq!(position, 120.0);
        assert!(second.finished());
    }

    #[test]
    fn wheel_targets_clamp_at_both_edges_and_reverse_without_a_queue() {
        let now = Instant::now();
        assert!(Motion::toward(0.0, -60.0, 1000.0, now).is_none());
        assert!(Motion::toward(1000.0, 1060.0, 1000.0, now).is_none());
        let mut down = Motion::toward(0.0, 60.0, 1000.0, now).unwrap();
        let next = now + Duration::from_millis(16);
        let visible = down.advance(next);
        let mut up = Motion::toward(visible, down.target - 60.0, 1000.0, next).unwrap();
        assert!(up.advance(next + Duration::from_millis(16)) < visible);
        assert_eq!(up.target, 0.0);
    }

    #[test]
    fn direct_input_cancels_wheel_motion() {
        assert!(cancels_motion(&Event::Mouse(mouse::Event::WheelScrolled {
            delta: mouse::ScrollDelta::Pixels { x: 0.0, y: -2.5 },
        })));
        assert!(cancels_motion(&Event::Keyboard(
            iced::keyboard::Event::ModifiersChanged(iced::keyboard::Modifiers::SHIFT,)
        )));
        assert!(cancels_motion(&Event::Mouse(mouse::Event::ButtonPressed(
            mouse::Button::Left
        ))));
        assert!(!cancels_motion(&Event::Mouse(
            mouse::Event::WheelScrolled {
                delta: mouse::ScrollDelta::Lines { x: 0.0, y: -1.0 },
            }
        )));
    }

    #[test]
    fn reversing_a_burst_moves_immediately_in_the_new_direction() {
        let start = Instant::now();
        let mut down = Motion::toward(0.0, 120.0, 1000.0, start).unwrap();
        let now = start + Duration::from_millis(16);
        let visible = down.advance(now);
        let mut up = Motion::wheel(visible, Some(down), -60.0, 1000.0, now).unwrap();
        assert!(up.advance(now + Duration::from_millis(16)) < visible);
    }
}
