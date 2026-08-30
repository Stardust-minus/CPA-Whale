use std::time::{Duration, Instant};

use crate::render::VisualState;

const PIECE_DURATION: f32 = 0.200;
const OPEN_TAIL_2_DELAY: f32 = 0.000;
const OPEN_TAIL_1_DELAY: f32 = 0.130;
const OPEN_MAIN_DELAY: f32 = 0.260;
const OPEN_TEXT_DELAY: f32 = 0.360;
const TEXT_DURATION: f32 = 0.160;
const CLOSE_MAIN_DELAY: f32 = 0.100;
const CLOSE_TAIL_1_DELAY: f32 = 0.200;
const CLOSE_TAIL_2_DELAY: f32 = 0.300;
const PRESS_DURATION: f32 = 0.220;
const CONTENT_FADE_OUT: f32 = 0.190;
const CONTENT_FADE_IN: f32 = 0.220;
const NUMBER_ROLL_DURATION: f32 = 0.700;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BubbleDirection {
    Opening,
    Closing,
}

#[derive(Debug, Clone, Copy)]
pub struct BubbleAnimation {
    direction: BubbleDirection,
    started: Instant,
}

impl BubbleAnimation {
    pub fn opening(now: Instant) -> Self {
        Self {
            direction: BubbleDirection::Opening,
            started: now,
        }
    }

    pub fn closing(now: Instant) -> Self {
        Self {
            direction: BubbleDirection::Closing,
            started: now,
        }
    }

    pub fn sample(self, now: Instant, visual: &mut VisualState) -> bool {
        let elapsed = now.saturating_duration_since(self.started).as_secs_f32();
        match self.direction {
            BubbleDirection::Opening => {
                visual.bubble_tail_2 = piece_in(elapsed, OPEN_TAIL_2_DELAY);
                visual.bubble_tail_1 = piece_in(elapsed, OPEN_TAIL_1_DELAY);
                visual.bubble_main = piece_in(elapsed, OPEN_MAIN_DELAY);
                visual.text_opacity = linear(elapsed, OPEN_TEXT_DELAY, TEXT_DURATION);
                elapsed < OPEN_TEXT_DELAY + TEXT_DURATION
            }
            BubbleDirection::Closing => {
                visual.text_opacity = 1.0 - linear(elapsed, 0.0, TEXT_DURATION);
                visual.bubble_main = 1.0 - piece_in(elapsed, CLOSE_MAIN_DELAY);
                visual.bubble_tail_1 = 1.0 - piece_in(elapsed, CLOSE_TAIL_1_DELAY);
                visual.bubble_tail_2 = 1.0 - piece_in(elapsed, CLOSE_TAIL_2_DELAY);
                elapsed < CLOSE_TAIL_2_DELAY + PIECE_DURATION
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContentSwapPhase {
    FadingOut,
    Commit,
    FadingIn,
    Complete,
}

#[derive(Debug, Clone, Copy)]
pub struct ContentSwapAnimation {
    started: Instant,
    committed: bool,
}

impl ContentSwapAnimation {
    pub fn new(now: Instant) -> Self {
        Self {
            started: now,
            committed: false,
        }
    }

    pub fn sample(&mut self, now: Instant) -> (f32, ContentSwapPhase) {
        let elapsed = now.saturating_duration_since(self.started).as_secs_f32();
        if elapsed < CONTENT_FADE_OUT {
            return (
                1.0 - linear(elapsed, 0.0, CONTENT_FADE_OUT),
                ContentSwapPhase::FadingOut,
            );
        }
        if !self.committed {
            self.committed = true;
            return (0.0, ContentSwapPhase::Commit);
        }
        let fade_in_elapsed = elapsed - CONTENT_FADE_OUT;
        if fade_in_elapsed < CONTENT_FADE_IN {
            return (
                linear(fade_in_elapsed, 0.0, CONTENT_FADE_IN),
                ContentSwapPhase::FadingIn,
            );
        }
        (1.0, ContentSwapPhase::Complete)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ScalarAnimation {
    from: f32,
    to: f32,
    started: Instant,
    duration: Duration,
    easing: Easing,
}

#[derive(Debug, Clone, Copy)]
pub enum Easing {
    CubicOut,
    OriginalSpring,
    Smooth,
}

impl ScalarAnimation {
    pub fn new(from: f32, to: f32, now: Instant, duration: Duration, easing: Easing) -> Self {
        Self {
            from,
            to,
            started: now,
            duration,
            easing,
        }
    }

    pub fn press(from: f32, to: f32, now: Instant) -> Self {
        Self::new(
            from,
            to,
            now,
            Duration::from_secs_f32(PRESS_DURATION),
            Easing::OriginalSpring,
        )
    }

    pub fn number(from: f32, to: f32, now: Instant) -> Self {
        Self::new(
            from,
            to,
            now,
            Duration::from_secs_f32(NUMBER_ROLL_DURATION),
            Easing::CubicOut,
        )
    }

    pub fn sample(self, now: Instant) -> (f32, bool) {
        let duration = self.duration.as_secs_f32().max(f32::EPSILON);
        let progress = now.saturating_duration_since(self.started).as_secs_f32() / duration;
        let done = progress >= 1.0;
        let progress = progress.clamp(0.0, 1.0);
        let eased = match self.easing {
            Easing::CubicOut => 1.0 - (1.0 - progress).powi(3),
            Easing::OriginalSpring => cubic_bezier(progress, 0.34, 1.56, 0.64, 1.0),
            Easing::Smooth => progress * progress * (3.0 - 2.0 * progress),
        };
        (self.from + (self.to - self.from) * eased, done)
    }
}

fn piece_in(elapsed: f32, delay: f32) -> f32 {
    let progress = linear(elapsed, delay, PIECE_DURATION);
    1.0 - (1.0 - progress).powi(3)
}

fn linear(elapsed: f32, delay: f32, duration: f32) -> f32 {
    ((elapsed - delay) / duration).clamp(0.0, 1.0)
}

fn cubic_bezier(progress: f32, x1: f32, y1: f32, x2: f32, y2: f32) -> f32 {
    let mut t = progress;
    for _ in 0..5 {
        let x = bezier(t, x1, x2);
        let dx = bezier_derivative(t, x1, x2);
        if dx.abs() < 1e-5 {
            break;
        }
        t = (t - (x - progress) / dx).clamp(0.0, 1.0);
    }
    bezier(t, y1, y2)
}

fn bezier(t: f32, p1: f32, p2: f32) -> f32 {
    let inv = 1.0 - t;
    3.0 * inv * inv * t * p1 + 3.0 * inv * t * t * p2 + t * t * t
}

fn bezier_derivative(t: f32, p1: f32, p2: f32) -> f32 {
    let inv = 1.0 - t;
    3.0 * inv * inv * p1 + 6.0 * inv * t * (p2 - p1) + 3.0 * t * t * (1.0 - p2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bubble_opens_in_original_tail_to_main_order() {
        let start = Instant::now();
        let animation = BubbleAnimation::opening(start);
        let mut visual = VisualState::default();
        animation.sample(start + Duration::from_millis(150), &mut visual);
        assert!(visual.bubble_tail_2 > visual.bubble_tail_1);
        assert_eq!(visual.bubble_main, 0.0);
        assert_eq!(visual.text_opacity, 0.0);

        animation.sample(start + Duration::from_millis(420), &mut visual);
        assert!(visual.bubble_main > 0.9);
        assert!(visual.text_opacity > 0.0);
    }

    #[test]
    fn content_swap_commits_only_after_fade_out() {
        let start = Instant::now();
        let mut animation = ContentSwapAnimation::new(start);
        let (_, phase) = animation.sample(start + Duration::from_millis(100));
        assert_eq!(phase, ContentSwapPhase::FadingOut);
        let (_, phase) = animation.sample(start + Duration::from_millis(190));
        assert_eq!(phase, ContentSwapPhase::Commit);
        let (_, phase) = animation.sample(start + Duration::from_millis(300));
        assert_eq!(phase, ContentSwapPhase::FadingIn);
    }

    #[test]
    fn q_spring_overshoots_before_settling() {
        let start = Instant::now();
        let animation = ScalarAnimation::press(0.0, 1.0, start);
        let (middle, _) = animation.sample(start + Duration::from_millis(130));
        assert!(middle > 1.0);
        let (end, done) = animation.sample(start + Duration::from_millis(220));
        assert!(done);
        assert!((end - 1.0).abs() < 0.001);
    }
}
