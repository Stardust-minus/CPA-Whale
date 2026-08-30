use crate::layout::{DESIGN_SIZE, TEXT_CENTER_X, TEXT_CENTER_Y, TEXT_MAX_WIDTH};
use crate::model::{
    displayed_card, CardLine, CardStyle, CardTone, ClientSettings, RandomCard, RuntimeState,
};

pub const NAVY: Color = Color::rgba(32, 49, 112, 255);
pub const PRIMARY_TEXT: Color = Color::rgba(83, 107, 169, 255);
pub const MUTED_TEXT: Color = Color::rgba(159, 176, 217, 255);
pub const GOOD_TEXT: Color = Color::rgba(47, 162, 76, 255);
pub const DANGER_TEXT: Color = Color::rgba(224, 67, 63, 255);
pub const WHITE: Color = Color::rgba(255, 255, 255, 255);

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Color {
    pub r: f32,
    pub g: f32,
    pub b: f32,
    pub a: f32,
}

impl Color {
    pub const fn rgba(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self {
            r: r as f32 / 255.0,
            g: g as f32 / 255.0,
            b: b as f32 / 255.0,
            a: a as f32 / 255.0,
        }
    }

    pub fn with_opacity(self, opacity: f32) -> Self {
        Self {
            a: self.a * opacity.clamp(0.0, 1.0),
            ..self
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VisualState {
    pub bubble_main: f32,
    pub bubble_tail_1: f32,
    pub bubble_tail_2: f32,
    pub text_opacity: f32,
    pub content_opacity: f32,
    pub squish_progress: f32,
    pub mirror_progress: f32,
    pub menu_button_opacity: f32,
    pub gif_frame: usize,
}

impl Default for VisualState {
    fn default() -> Self {
        Self {
            bubble_main: 0.0,
            bubble_tail_1: 0.0,
            bubble_tail_2: 0.0,
            text_opacity: 0.0,
            content_opacity: 1.0,
            squish_progress: 0.0,
            mirror_progress: 0.0,
            menu_button_opacity: 0.0,
            gif_frame: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextSpec {
    pub text: String,
    pub size: f32,
    pub weight: u16,
    pub color: Color,
    pub wrap: bool,
    pub line_count: usize,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SceneContent {
    Lines([Option<TextSpec>; 3]),
    RuaGif { frame: usize },
}

#[derive(Debug, Clone, PartialEq)]
pub struct WidgetScene {
    pub flipped: bool,
    pub visual: VisualState,
    pub content: SceneContent,
}

impl WidgetScene {
    pub fn build(
        _settings: &ClientSettings,
        runtime: &RuntimeState,
        flipped: bool,
        visual: VisualState,
    ) -> Self {
        let content = match displayed_card(runtime) {
            RandomCard::Lines(lines) => SceneContent::Lines(lines.map(|line| line.map(text_spec))),
            RandomCard::RuaGif => SceneContent::RuaGif {
                frame: visual.gif_frame,
            },
        };
        Self {
            flipped,
            visual,
            content,
        }
    }
}

pub fn text_spec(line: CardLine) -> TextSpec {
    let (size, weight) = match line.style {
        CardStyle::Label => (66.0, 600),
        CardStyle::Amount => (128.0, 800),
        CardStyle::Period => (104.0, 800),
        CardStyle::Hint => (48.0, 400),
    };
    let color = match line.tone {
        CardTone::Primary => PRIMARY_TEXT,
        CardTone::Muted => MUTED_TEXT,
        CardTone::Good => GOOD_TEXT,
        CardTone::Danger => DANGER_TEXT,
    };
    let (size, line_count) = fitted_text_metrics(&line.text, size, line.wrap);
    TextSpec {
        text: line.text,
        size,
        weight,
        color,
        wrap: line.wrap,
        line_count,
    }
}

fn fitted_text_metrics(text: &str, base_size: f32, wrap: bool) -> (f32, usize) {
    let units = text_width_units(text);
    if wrap {
        let size = base_size.min(56.0);
        let units_per_line = TEXT_MAX_WIDTH * 0.86 / size;
        let line_count = (units / units_per_line).ceil().clamp(1.0, 3.0) as usize;
        return (size, line_count);
    }
    let estimated = units * base_size;
    let size = if estimated <= TEXT_MAX_WIDTH * 0.90 {
        base_size
    } else {
        (base_size * TEXT_MAX_WIDTH * 0.90 / estimated).max(base_size * 0.52)
    };
    (size, 1)
}

fn text_width_units(text: &str) -> f32 {
    text.chars().fold(0.0_f32, |width, character| {
        width
            + if character.is_whitespace() {
                0.32
            } else if character.is_ascii_punctuation() {
                0.42
            } else if character.is_ascii() {
                0.60
            } else {
                1.0
            }
    })
}

pub fn text_layout_bounds() -> (f32, f32, f32, f32) {
    (
        TEXT_CENTER_X - TEXT_MAX_WIDTH / 2.0,
        80.0,
        TEXT_CENTER_X + TEXT_MAX_WIDTH / 2.0,
        452.0,
    )
}

pub fn whale_transform(squish_progress: f32) -> (f32, f32) {
    let progress = squish_progress.clamp(0.0, 1.0);
    (1.0 + 0.05 * progress, 1.0 - 0.12 * progress)
}

pub fn bubble_piece_transform(progress: f32, center_x: f32, center_y: f32) -> [f32; 6] {
    let progress = progress.clamp(0.0, 1.0);
    let eased = 1.0 - (1.0 - progress).powi(3);
    let scale = 0.7 + 0.3 * eased;
    [
        scale,
        0.0,
        0.0,
        scale,
        center_x * (1.0 - scale),
        center_y * (1.0 - scale),
    ]
}

pub fn design_to_surface(value: f32, surface_size: f32) -> f32 {
    value * surface_size / DESIGN_SIZE
}

pub fn text_center() -> (f32, f32) {
    (TEXT_CENTER_X, TEXT_CENTER_Y)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{BubbleContent, RandomCard};

    #[test]
    fn uses_original_text_center_in_the_bubble_canvas() {
        assert_eq!(text_center(), (454.0, 266.0));
        assert!(text_center().1 / DESIGN_SIZE < 0.27);
    }

    #[test]
    fn rua_frame_does_not_change_when_scene_is_rebuilt() {
        let settings = ClientSettings::default();
        let mut runtime = RuntimeState::default();
        runtime.bubble_open = true;
        runtime.bubble_content = BubbleContent::Random(RandomCard::RuaGif);
        let visual = VisualState {
            gif_frame: 7,
            ..VisualState::default()
        };
        for _ in 0..300 {
            let scene = WidgetScene::build(&settings, &runtime, false, visual);
            assert_eq!(scene.content, SceneContent::RuaGif { frame: 7 });
        }
    }

    #[test]
    fn long_period_text_is_reduced_to_stay_inside_the_bubble() {
        assert!(fitted_text_metrics("社区 IQ  100.33", 104.0, false).0 < 104.0);
    }

    #[test]
    fn wrapped_text_reserves_multiple_lines_of_vertical_space() {
        let spec = text_spec(
            CardLine::new(
                "这是一段需要在气泡里完整换行显示而不能裁掉上下部分的长文本",
                CardStyle::Label,
            )
            .wrapped(),
        );
        assert!(spec.line_count >= 2);
        assert_eq!(spec.size, 56.0);
    }

    #[test]
    fn q_squish_matches_original() {
        assert_eq!(whale_transform(1.0), (1.05, 0.88));
    }
}
