pub const DESIGN_SIZE: f32 = 1026.0;
pub const BUBBLE_CANVAS_HEIGHT: f32 = 700.0;
pub const WHALE_RATIO: f32 = 0.5945;
pub const TEXT_CENTER_X: f32 = 454.0;
pub const TEXT_CENTER_Y: f32 = BUBBLE_CANVAS_HEIGHT * 0.38;
pub const TEXT_MAX_WIDTH: f32 = 560.0;
pub const MIN_SCALE: f32 = 0.6;
pub const MAX_SCALE: f32 = 2.5;
pub const MIN_BASE_DIP: f32 = 122.0;
pub const MAX_BASE_DIP: f32 = 625.0;
pub const NATURAL_BASE_DIP: f32 = 250.0;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RectF {
    pub left: f32,
    pub top: f32,
    pub right: f32,
    pub bottom: f32,
}

impl RectF {
    pub fn width(self) -> f32 {
        self.right - self.left
    }

    pub fn height(self) -> f32 {
        self.bottom - self.top
    }

    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.left && x <= self.right && y >= self.top && y <= self.bottom
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HorizontalAnchor {
    Left,
    Free,
    Right,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerticalAnchor {
    Top,
    Free,
    Bottom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapAnchors {
    pub horizontal: HorizontalAnchor,
    pub vertical: VerticalAnchor,
}

pub fn clamp_scale(scale: f32) -> f32 {
    if scale.is_finite() {
        scale.clamp(MIN_SCALE, MAX_SCALE)
    } else {
        1.5
    }
}

pub fn widget_base_dip(scale: f32, monitor_width_dip: f32, monitor_height_dip: f32) -> f32 {
    let shortest = monitor_width_dip.min(monitor_height_dip).max(0.0);
    let natural = NATURAL_BASE_DIP.min(shortest * 0.28) * clamp_scale(scale);
    natural.clamp(MIN_BASE_DIP, MAX_BASE_DIP)
}

pub fn widget_base_px(scale: f32, monitor_width_px: i32, monitor_height_px: i32, dpi: u32) -> i32 {
    let dpi = dpi.max(1) as f32;
    let width_dip = monitor_width_px.max(0) as f32 * 96.0 / dpi;
    let height_dip = monitor_height_px.max(0) as f32 * 96.0 / dpi;
    (widget_base_dip(scale, width_dip, height_dip) * dpi / 96.0).round() as i32
}

pub fn whale_rect(flipped: bool) -> RectF {
    let size = DESIGN_SIZE * WHALE_RATIO;
    let left = if flipped { 0.0 } else { DESIGN_SIZE - size };
    RectF {
        left,
        top: DESIGN_SIZE - size,
        right: left + size,
        bottom: DESIGN_SIZE,
    }
}

pub fn menu_button_rect(flipped: bool, base_dip: f32) -> RectF {
    let button_design = 26.0 / base_dip.max(1.0) * DESIGN_SIZE;
    let inset_design = 4.0 / base_dip.max(1.0) * DESIGN_SIZE;
    let top = DESIGN_SIZE * 0.4055 + inset_design;
    let left = if flipped {
        inset_design
    } else {
        DESIGN_SIZE - button_design - inset_design
    };
    RectF {
        left,
        top,
        right: left + button_design,
        bottom: top + button_design,
    }
}

pub fn snap_anchors(
    left: f32,
    top: f32,
    width: f32,
    height: f32,
    work_width: f32,
    work_height: f32,
) -> SnapAnchors {
    let center_x = left + width / 2.0;
    let center_y = top + height / 2.0;
    let horizontal = if center_x < work_width / 4.0 {
        HorizontalAnchor::Left
    } else if center_x > work_width * 3.0 / 4.0 {
        HorizontalAnchor::Right
    } else {
        HorizontalAnchor::Free
    };
    let vertical = if center_y < work_height / 4.0 {
        VerticalAnchor::Top
    } else if center_y > work_height * 3.0 / 4.0 {
        VerticalAnchor::Bottom
    } else {
        VerticalAnchor::Free
    };
    SnapAnchors {
        horizontal,
        vertical,
    }
}

pub fn scaled_fixed_corner(
    old_rect: RectF,
    new_width: f32,
    new_height: f32,
    flipped: bool,
) -> RectF {
    let bottom = old_rect.bottom;
    if flipped {
        RectF {
            left: old_rect.left,
            top: bottom - new_height,
            right: old_rect.left + new_width,
            bottom,
        }
    } else {
        RectF {
            left: old_rect.right - new_width,
            top: bottom - new_height,
            right: old_rect.right,
            bottom,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_original_text_center() {
        assert!((TEXT_CENTER_Y - 266.0).abs() < 0.001);
    }

    #[test]
    fn matches_original_default_size_at_full_hd() {
        assert_eq!(widget_base_dip(1.5, 1920.0, 1080.0), 375.0);
        assert_eq!(widget_base_dip(0.6, 1920.0, 1080.0), 150.0);
        assert_eq!(widget_base_dip(2.5, 1920.0, 1080.0), 625.0);
    }

    #[test]
    fn converts_dips_for_high_dpi() {
        assert_eq!(widget_base_px(1.5, 2880, 1620, 144), 563);
    }

    #[test]
    fn whale_occupies_original_fraction() {
        let rect = whale_rect(false);
        assert!((rect.width() / DESIGN_SIZE - WHALE_RATIO).abs() < f32::EPSILON);
        assert_eq!(rect.right, DESIGN_SIZE);
        assert_eq!(rect.bottom, DESIGN_SIZE);
    }

    #[test]
    fn scaling_keeps_the_whale_corner_fixed() {
        let old = RectF {
            left: 100.0,
            top: 100.0,
            right: 475.0,
            bottom: 475.0,
        };
        let right = scaled_fixed_corner(old, 500.0, 500.0, false);
        assert_eq!(right.right, old.right);
        assert_eq!(right.bottom, old.bottom);
        let left = scaled_fixed_corner(old, 500.0, 500.0, true);
        assert_eq!(left.left, old.left);
        assert_eq!(left.bottom, old.bottom);
    }

    #[test]
    fn snaps_each_axis_independently() {
        let anchors = snap_anchors(0.0, 0.0, 200.0, 200.0, 1000.0, 1000.0);
        assert_eq!(anchors.horizontal, HorizontalAnchor::Left);
        assert_eq!(anchors.vertical, VerticalAnchor::Top);

        let anchors = snap_anchors(400.0, 800.0, 200.0, 200.0, 1000.0, 1000.0);
        assert_eq!(anchors.horizontal, HorizontalAnchor::Free);
        assert_eq!(anchors.vertical, VerticalAnchor::Bottom);
    }
}
