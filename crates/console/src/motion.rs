use std::time::Instant;

const DURATION: f32 = crate::theme::FADE as f32 / 1000.0;

pub(crate) fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[derive(Clone, Copy)]
pub(crate) struct Transition {
    from: f32,
    to: f32,
    since: Instant,
}

impl Transition {
    pub(crate) fn new(value: f32) -> Self {
        Self {
            from: value,
            to: value,
            since: Instant::now(),
        }
    }

    pub(crate) fn value(self, now: Instant) -> f32 {
        let t =
            (now.saturating_duration_since(self.since).as_secs_f32() / DURATION).clamp(0.0, 1.0);
        self.from + (self.to - self.from) * ease(t)
    }

    pub(crate) fn set(&mut self, target: f32, now: Instant) {
        if self.to != target {
            self.from = self.value(now);
            self.to = target;
            self.since = now;
        }
    }

    pub(crate) fn moving(self, now: Instant) -> bool {
        self.from != self.to && now.saturating_duration_since(self.since).as_secs_f32() < DURATION
    }
}

#[derive(Clone, Copy, Default)]
pub(crate) struct PageTransition {
    from: f32,
    elapsed: f32,
    running: bool,
}

impl PageTransition {
    pub(crate) fn value(self) -> f32 {
        self.from + (1.0 - self.from) * ease(self.elapsed / DURATION)
    }

    pub(crate) fn reveal(&mut self) {
        self.from = if self.running { self.value() } else { 0.0 };
        self.elapsed = 0.0;
        self.running = true;
    }

    pub(crate) fn advance(&mut self, elapsed: f32) {
        if self.running {
            // History's initial text layout can consume the entire wall-clock fade.
            self.elapsed = (self.elapsed + elapsed.clamp(0.0, crate::FRAME_CAP)).min(DURATION);
            self.running = self.elapsed < DURATION;
        }
    }

    pub(crate) fn moving(self) -> bool {
        self.running
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn reversing_a_hover_keeps_its_current_brightness() {
        let now = Instant::now();
        let mut transition = Transition::new(0.0);
        transition.set(1.0, now);
        let halfway = now + Duration::from_millis(crate::theme::FADE / 2);
        let before = transition.value(halfway);
        transition.set(0.0, halfway);
        assert!((before - transition.value(halfway)).abs() < f32::EPSILON);
        assert!(before > 0.4 && before < 0.6);
        let finished = halfway + Duration::from_millis(300);
        assert_eq!(transition.value(finished), 0.0);
        assert!(!transition.moving(finished));
    }

    #[test]
    fn a_slow_first_page_layout_does_not_skip_the_fade() {
        let mut page = PageTransition::default();
        assert_eq!(page.value(), 0.0);
        assert!(!page.moving());
        page.reveal();
        page.advance(0.5);
        assert!(page.value() > 0.0 && page.value() < 0.1);
        assert!(page.moving());
        for _ in 0..6 {
            page.advance(crate::FRAME_CAP);
        }
        assert_eq!(page.value(), 1.0);
        assert!(!page.moving());
    }

    #[test]
    fn switching_pages_mid_fade_keeps_the_current_brightness() {
        let mut page = PageTransition::default();
        page.reveal();
        page.advance(crate::FRAME_CAP);
        let visible = page.value();
        page.reveal();
        assert_eq!(page.value(), visible);
        for _ in 0..7 {
            page.advance(crate::FRAME_CAP);
        }
        page.reveal();
        assert_eq!(page.value(), 0.0);
        assert!(page.moving());
    }
}
