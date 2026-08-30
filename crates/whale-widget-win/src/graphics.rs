use std::io::Cursor;
use std::mem::ManuallyDrop;

use image::AnimationDecoder;
use windows::core::Interface;
use windows::Win32::Foundation::{HMODULE, HWND, POINT};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_FIGURE_BEGIN_FILLED, D2D1_FIGURE_END_CLOSED,
    D2D1_PIXEL_FORMAT, D2D_RECT_F, D2D_SIZE_F, D2D_SIZE_U,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1CreateFactory, ID2D1Bitmap1, ID2D1Brush, ID2D1Device, ID2D1DeviceContext, ID2D1Factory1,
    ID2D1PathGeometry1, ID2D1RenderTarget, ID2D1SolidColorBrush, D2D1_ARC_SEGMENT,
    D2D1_ARC_SIZE_LARGE, D2D1_ARC_SIZE_SMALL, D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_OPTIONS_TARGET,
    D2D1_BITMAP_PROPERTIES1, D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_ELLIPSE, D2D1_FACTORY_OPTIONS,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE,
    D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
};
use windows::Win32::Graphics::Direct3D::{
    D3D_DRIVER_TYPE_HARDWARE, D3D_DRIVER_TYPE_WARP, D3D_FEATURE_LEVEL, D3D_FEATURE_LEVEL_10_0,
    D3D_FEATURE_LEVEL_10_1, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
};
use windows::Win32::Graphics::Direct3D11::{
    D3D11CreateDevice, ID3D11Device, ID3D11DeviceContext, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
    D3D11_SDK_VERSION,
};
use windows::Win32::Graphics::DirectComposition::{
    DCompositionCreateDevice, IDCompositionDevice, IDCompositionSurface, IDCompositionTarget,
    IDCompositionVisual,
};
use windows::Win32::Graphics::DirectWrite::{
    DWriteCreateFactory, IDWriteFactory, IDWriteFontCollection, IDWriteTextFormat,
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_EXTRA_BOLD, DWRITE_FONT_WEIGHT_NORMAL,
    DWRITE_FONT_WEIGHT_SEMI_BOLD, DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_WORD_WRAPPING_NO_WRAP,
    DWRITE_WORD_WRAPPING_WRAP,
};
use windows::Win32::Graphics::Dwm::DwmFlush;
use windows::Win32::Graphics::Dxgi::Common::{
    DXGI_ALPHA_MODE_PREMULTIPLIED, DXGI_FORMAT_B8G8R8A8_UNORM,
};
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGISurface};
use windows_numerics::{Matrix3x2, Vector2};

use crate::assets;
use crate::layout::{
    menu_button_rect, whale_rect, DESIGN_SIZE, TEXT_CENTER_X, TEXT_CENTER_Y, TEXT_MAX_WIDTH,
};
use crate::render::{
    bubble_piece_transform, whale_transform, Color, SceneContent, TextSpec, WidgetScene,
    DANGER_TEXT, GOOD_TEXT, MUTED_TEXT, NAVY, PRIMARY_TEXT, WHITE,
};

const GIF_FRAME_DURATION_MS: u64 = 20;

pub struct GpuRenderer {
    device: DeviceResources,
    _target: IDCompositionTarget,
    visual: IDCompositionVisual,
    surface: IDCompositionSurface,
    size: u32,
    assets: GpuAssets,
    geometry: BubbleGeometry,
    brushes: Brushes,
    formats: TextFormats,
    hardware_accelerated: bool,
}

impl GpuRenderer {
    /// Creates a DirectComposition-backed transparent renderer for `hwnd`.
    ///
    /// # Safety
    /// `hwnd` must be a live top-level window owned by this process and the caller must have
    /// initialized COM for the current thread.
    pub unsafe fn new(
        hwnd: windows_sys::Win32::Foundation::HWND,
        size: u32,
    ) -> Result<Self, String> {
        let (device, hardware_accelerated) = DeviceResources::new()?;
        let hwnd = HWND(hwnd);
        let target = device
            .composition
            .CreateTargetForHwnd(hwnd, true)
            .map_err(win_error("CreateTargetForHwnd"))?;
        let visual = device
            .composition
            .CreateVisual()
            .map_err(win_error("CreateVisual"))?;
        target
            .SetRoot(&visual)
            .map_err(win_error("IDCompositionTarget::SetRoot"))?;
        let surface = create_surface(&device.composition, size)?;
        visual
            .SetContent(&surface)
            .map_err(win_error("IDCompositionVisual::SetContent"))?;
        let assets = GpuAssets::new(&device.context)?;
        let geometry = BubbleGeometry::new(&device.factory)?;
        let brushes = Brushes::new(&device.context)?;
        let formats = TextFormats::new(&device.write_factory)?;
        device
            .composition
            .Commit()
            .map_err(win_error("IDCompositionDevice::Commit"))?;
        Ok(Self {
            device,
            _target: target,
            visual,
            surface,
            size,
            assets,
            geometry,
            brushes,
            formats,
            hardware_accelerated,
        })
    }

    pub fn is_hardware_accelerated(&self) -> bool {
        self.hardware_accelerated
    }

    pub fn gif_frame_count(&self) -> usize {
        self.assets.gif_frames.len()
    }

    pub fn gif_frame_for_elapsed(&self, elapsed_ms: u64) -> usize {
        let count = self.gif_frame_count().max(1);
        ((elapsed_ms / GIF_FRAME_DURATION_MS) as usize) % count
    }

    pub fn resize(&mut self, size: u32) -> Result<(), String> {
        let size = size.max(1);
        if self.size == size {
            return Ok(());
        }
        self.surface = unsafe { create_surface(&self.device.composition, size)? };
        unsafe {
            self.visual
                .SetContent(&self.surface)
                .map_err(win_error("IDCompositionVisual::SetContent"))?;
            self.device
                .composition
                .Commit()
                .map_err(win_error("IDCompositionDevice::Commit"))?;
        }
        self.size = size;
        Ok(())
    }

    pub fn whale_hit(&self, local_x: i32, local_y: i32, flipped: bool) -> bool {
        if local_x < 0 || local_y < 0 || local_x >= self.size as i32 || local_y >= self.size as i32
        {
            return false;
        }
        let design_x = local_x as f32 * DESIGN_SIZE / self.size as f32;
        let design_y = local_y as f32 * DESIGN_SIZE / self.size as f32;
        let rect = whale_rect(flipped);
        if !rect.contains(design_x, design_y) {
            return false;
        }
        let mut normalized_x = (design_x - rect.left) / rect.width();
        if flipped {
            normalized_x = 1.0 - normalized_x;
        }
        let normalized_y = (design_y - rect.top) / rect.height();
        let x = (normalized_x * self.assets.whale_mask_width as f32)
            .floor()
            .clamp(0.0, self.assets.whale_mask_width.saturating_sub(1) as f32)
            as usize;
        let y = (normalized_y * self.assets.whale_mask_height as f32)
            .floor()
            .clamp(0.0, self.assets.whale_mask_height.saturating_sub(1) as f32)
            as usize;
        self.assets.whale_alpha[y * self.assets.whale_mask_width + x] > 10
    }

    pub fn whale_region_runs(&self, flipped: bool) -> Vec<(i32, i32, i32, i32)> {
        let mask_width = self.assets.whale_mask_width;
        let mask_height = self.assets.whale_mask_height;
        if mask_width == 0 || mask_height == 0 {
            return Vec::new();
        }
        let whale = whale_rect(flipped);
        let design_scale = self.size as f32 / DESIGN_SIZE;
        let mut runs = Vec::new();
        for source_y in 0..mask_height {
            let row = &self.assets.whale_alpha[source_y * mask_width..(source_y + 1) * mask_width];
            let mut source_x = 0;
            while source_x < mask_width {
                while source_x < mask_width && row[source_x] <= 10 {
                    source_x += 1;
                }
                if source_x >= mask_width {
                    break;
                }
                let run_start = source_x;
                while source_x < mask_width && row[source_x] > 10 {
                    source_x += 1;
                }
                let run_end = source_x;
                let (mapped_start, mapped_end) = if flipped {
                    (mask_width - run_end, mask_width - run_start)
                } else {
                    (run_start, run_end)
                };
                let left = ((whale.left + mapped_start as f32 * whale.width() / mask_width as f32)
                    * design_scale)
                    .floor() as i32
                    - 1;
                let right = ((whale.left + mapped_end as f32 * whale.width() / mask_width as f32)
                    * design_scale)
                    .ceil() as i32
                    + 1;
                let top = ((whale.top + source_y as f32 * whale.height() / mask_height as f32)
                    * design_scale)
                    .floor() as i32
                    - 1;
                let bottom = ((whale.top
                    + (source_y + 1) as f32 * whale.height() / mask_height as f32)
                    * design_scale)
                    .ceil() as i32
                    + 1;
                runs.push((
                    left.clamp(0, self.size as i32),
                    top.clamp(0, self.size as i32),
                    right.clamp(0, self.size as i32),
                    bottom.clamp(0, self.size as i32),
                ));
            }
        }
        runs
    }

    pub fn menu_button_hit(
        &self,
        local_x: i32,
        local_y: i32,
        flipped: bool,
        base_dip: f32,
    ) -> bool {
        let design_x = local_x as f32 * DESIGN_SIZE / self.size as f32;
        let design_y = local_y as f32 * DESIGN_SIZE / self.size as f32;
        menu_button_rect(flipped, base_dip).contains(design_x, design_y)
    }

    pub fn bubble_hit(&self, local_x: i32, local_y: i32, flipped: bool) -> bool {
        let mut x = local_x as f32 * DESIGN_SIZE / self.size as f32;
        let y = local_y as f32 * DESIGN_SIZE / self.size as f32;
        if flipped {
            x = DESIGN_SIZE - x;
        }
        let dx = (x - 454.0) / 382.0;
        let dy = (y - 248.0) / 241.0;
        dx * dx + dy * dy <= 1.0
    }

    pub fn render(&mut self, scene: &WidgetScene, base_dip: f32) -> Result<(), String> {
        unsafe { self.render_inner(scene, base_dip) }
    }

    unsafe fn render_inner(&mut self, scene: &WidgetScene, base_dip: f32) -> Result<(), String> {
        let mut offset = POINT::default();
        let dxgi_surface: IDXGISurface = self
            .surface
            .BeginDraw(None, &mut offset)
            .map_err(win_error("IDCompositionSurface::BeginDraw"))?;
        let target_properties =
            bitmap_properties(D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW);
        let target_bitmap = self
            .device
            .context
            .CreateBitmapFromDxgiSurface(&dxgi_surface, Some(&target_properties))
            .map_err(win_error("CreateBitmapFromDxgiSurface"))?;
        self.device.context.SetTarget(&target_bitmap);
        self.device
            .context
            .SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        self.device.context.BeginDraw();
        self.device.context.Clear(Some(&D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }));

        let result = self.draw_scene(scene, base_dip, offset);
        let end_draw = self.device.context.EndDraw(None, None);
        self.device
            .context
            .SetTarget(None::<&windows::Win32::Graphics::Direct2D::ID2D1Image>);
        let surface_end = self.surface.EndDraw();
        result?;
        end_draw.map_err(win_error("ID2D1DeviceContext::EndDraw"))?;
        surface_end.map_err(win_error("IDCompositionSurface::EndDraw"))?;
        self.device
            .composition
            .Commit()
            .map_err(win_error("IDCompositionDevice::Commit"))?;
        let _ = DwmFlush();
        Ok(())
    }

    unsafe fn draw_scene(
        &self,
        scene: &WidgetScene,
        base_dip: f32,
        offset: POINT,
    ) -> Result<(), String> {
        let surface = matrix_scale_translate(
            self.size as f32 / DESIGN_SIZE,
            self.size as f32 / DESIGN_SIZE,
            offset.x as f32,
            offset.y as f32,
        );
        let mirror_progress = scene.visual.mirror_progress.clamp(0.0, 1.0);
        let mirror = Matrix3x2 {
            M11: 1.0 - 2.0 * mirror_progress,
            M22: 1.0,
            M31: DESIGN_SIZE * mirror_progress,
            ..Matrix3x2::default()
        };
        let root = matrix_multiply(mirror, surface);
        self.draw_bubble_piece(
            &self.geometry.tail_2,
            (442.0, 646.0),
            scene.visual.bubble_tail_2,
            root,
        );
        self.draw_bubble_piece(
            &self.geometry.tail_1,
            (352.0, 561.0),
            scene.visual.bubble_tail_1,
            root,
        );
        self.draw_bubble_piece(
            &self.geometry.main,
            (454.0, 248.0),
            scene.visual.bubble_main,
            root,
        );

        let whale_rect = whale_rect(false);
        let (sx, sy) = whale_transform(scene.visual.squish_progress);
        let squish = matrix_scale_at(sx, sy, DESIGN_SIZE / 2.0, DESIGN_SIZE);
        self.device
            .context
            .SetTransform(&matrix_multiply(matrix_multiply(squish, mirror), surface));
        let destination = D2D_RECT_F {
            left: whale_rect.left,
            top: whale_rect.top,
            right: whale_rect.right,
            bottom: whale_rect.bottom,
        };
        let render_target: &ID2D1RenderTarget = &self.device.context;
        render_target.DrawBitmap(
            &self.assets.whale,
            Some(&destination),
            1.0,
            D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
            None,
        );

        let content_opacity = scene.visual.text_opacity.clamp(0.0, 1.0)
            * scene.visual.content_opacity.clamp(0.0, 1.0);
        if content_opacity > 0.001 {
            self.device.context.SetTransform(&surface);
            match &scene.content {
                SceneContent::Lines(lines) => {
                    self.draw_lines(lines, mirror_progress, content_opacity)?;
                }
                SceneContent::RuaGif { frame } => {
                    self.draw_gif(*frame, mirror_progress, content_opacity);
                }
            }
        }
        self.draw_menu_button(scene, base_dip, surface);
        self.device.context.SetTransform(&matrix_identity());
        Ok(())
    }

    unsafe fn draw_bubble_piece(
        &self,
        geometry: &ID2D1PathGeometry1,
        center: (f32, f32),
        progress: f32,
        root: Matrix3x2,
    ) {
        if progress <= 0.001 {
            return;
        }
        let piece = bubble_piece_transform(progress, center.0, center.1);
        let piece = Matrix3x2 {
            M11: piece[0],
            M12: piece[1],
            M21: piece[2],
            M22: piece[3],
            M31: piece[4],
            M32: piece[5],
        };
        self.device
            .context
            .SetTransform(&matrix_multiply(piece, root));
        self.brushes.white.SetOpacity(progress.clamp(0.0, 1.0));
        self.brushes.navy.SetOpacity(progress.clamp(0.0, 1.0));
        self.device
            .context
            .FillGeometry(geometry, &self.brushes.white, None::<&ID2D1Brush>);
        self.device.context.DrawGeometry(
            geometry,
            &self.brushes.navy,
            18.0,
            None::<&windows::Win32::Graphics::Direct2D::ID2D1StrokeStyle>,
        );
    }

    unsafe fn draw_lines(
        &self,
        lines: &[Option<TextSpec>; 3],
        mirror_progress: f32,
        opacity: f32,
    ) -> Result<(), String> {
        let active = lines.iter().flatten().collect::<Vec<_>>();
        if active.is_empty() {
            return Ok(());
        }
        let heights = active
            .iter()
            .map(|line| text_height(line))
            .collect::<Vec<_>>();
        let total_height = heights.iter().sum::<f32>();
        let mut top = TEXT_CENTER_Y - total_height / 2.0;
        let center_x =
            TEXT_CENTER_X + (DESIGN_SIZE - 2.0 * TEXT_CENTER_X) * mirror_progress.clamp(0.0, 1.0);
        for (line, height) in active.into_iter().zip(heights) {
            let rect = D2D_RECT_F {
                left: center_x - TEXT_MAX_WIDTH / 2.0,
                top,
                right: center_x + TEXT_MAX_WIDTH / 2.0,
                bottom: top + height,
            };
            let format = self.formats.for_spec(line);
            format
                .SetWordWrapping(if line.wrap {
                    DWRITE_WORD_WRAPPING_WRAP
                } else {
                    DWRITE_WORD_WRAPPING_NO_WRAP
                })
                .map_err(win_error("IDWriteTextFormat::SetWordWrapping"))?;
            let brush = self.brushes.for_color(line.color);
            brush.SetOpacity(opacity);
            let utf16 = line.text.encode_utf16().collect::<Vec<_>>();
            let render_target: &ID2D1RenderTarget = &self.device.context;
            render_target.DrawText(
                &utf16,
                format,
                &rect,
                brush,
                windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
            top += height;
        }
        Ok(())
    }

    unsafe fn draw_gif(&self, frame: usize, mirror_progress: f32, opacity: f32) {
        if self.assets.gif_frames.is_empty() {
            return;
        }
        let bitmap = &self.assets.gif_frames[frame % self.assets.gif_frames.len()];
        let center_x =
            TEXT_CENTER_X + (DESIGN_SIZE - 2.0 * TEXT_CENTER_X) * mirror_progress.clamp(0.0, 1.0);
        let size = 400.0;
        let destination = D2D_RECT_F {
            left: center_x - size / 2.0,
            top: TEXT_CENTER_Y - size / 2.0,
            right: center_x + size / 2.0,
            bottom: TEXT_CENTER_Y + size / 2.0,
        };
        let render_target: &ID2D1RenderTarget = &self.device.context;
        render_target.DrawBitmap(
            bitmap,
            Some(&destination),
            opacity,
            D2D1_BITMAP_INTERPOLATION_MODE_LINEAR,
            None,
        );
    }

    unsafe fn draw_menu_button(&self, scene: &WidgetScene, base_dip: f32, surface: Matrix3x2) {
        let opacity = scene.visual.menu_button_opacity.clamp(0.0, 1.0);
        if opacity <= 0.001 {
            return;
        }
        self.device.context.SetTransform(&surface);
        let rect = menu_button_rect(scene.flipped, base_dip);
        let rounded = windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: rect.left,
                top: rect.top,
                right: rect.right,
                bottom: rect.bottom,
            },
            radiusX: 6.0 / base_dip.max(1.0) * DESIGN_SIZE,
            radiusY: 6.0 / base_dip.max(1.0) * DESIGN_SIZE,
        };
        self.brushes.navy.SetOpacity(opacity * 0.85);
        self.device
            .context
            .FillRoundedRectangle(&rounded, &self.brushes.navy);
        self.brushes.white.SetOpacity(opacity);
        let inset = rect.width() * 0.24;
        let stroke = (2.0 / base_dip.max(1.0) * DESIGN_SIZE).max(2.0);
        for fraction in [0.32, 0.5, 0.68] {
            let y = rect.top + rect.height() * fraction;
            self.device.context.DrawLine(
                Vector2 {
                    X: rect.left + inset,
                    Y: y,
                },
                Vector2 {
                    X: rect.right - inset,
                    Y: y,
                },
                &self.brushes.white,
                stroke,
                None::<&windows::Win32::Graphics::Direct2D::ID2D1StrokeStyle>,
            );
        }
    }
}

pub struct MenuPanelData<'a> {
    pub scale: f32,
    pub volume: f32,
    pub sound_set: &'a str,
    pub bubble_enabled: bool,
    pub always_on_top: bool,
    pub autostart: bool,
    pub hardware_accelerated: bool,
}

pub struct SetupPanelData<'a> {
    pub status: Option<&'a str>,
    pub saving: bool,
}

pub struct DataSettingsPanelData<'a> {
    pub model: &'a str,
    pub reasoning_effort: &'a str,
    pub cards: &'a [(&'a str, bool)],
}

pub struct DetailsPanelData<'a> {
    pub today_tokens: &'a str,
    pub today_usd: &'a str,
    pub startup_tokens: &'a str,
    pub accounts: usize,
    pub signals: usize,
    pub hardware_accelerated: bool,
    pub page: usize,
    pub rows: &'a [String],
}

pub struct PanelRenderer {
    device: DeviceResources,
    _target: IDCompositionTarget,
    visual: IDCompositionVisual,
    surface: IDCompositionSurface,
    width: u32,
    height: u32,
    dpi_scale: f32,
    brushes: Brushes,
}

impl PanelRenderer {
    /// Creates a GPU-rendered DirectComposition panel.
    ///
    /// # Safety
    /// `hwnd` must be a live top-level window owned by the current process.
    pub unsafe fn new(
        hwnd: windows_sys::Win32::Foundation::HWND,
        width: u32,
        height: u32,
        dpi: u32,
    ) -> Result<Self, String> {
        let (device, _) = DeviceResources::new()?;
        let target = device
            .composition
            .CreateTargetForHwnd(HWND(hwnd), true)
            .map_err(win_error("CreateTargetForHwnd(panel)"))?;
        let visual = device
            .composition
            .CreateVisual()
            .map_err(win_error("CreateVisual(panel)"))?;
        target
            .SetRoot(&visual)
            .map_err(win_error("SetRoot(panel)"))?;
        let surface = create_rect_surface(&device.composition, width, height)?;
        visual
            .SetContent(&surface)
            .map_err(win_error("SetContent(panel)"))?;
        let brushes = Brushes::new(&device.context)?;
        device
            .composition
            .Commit()
            .map_err(win_error("Commit(panel)"))?;
        Ok(Self {
            device,
            _target: target,
            visual,
            surface,
            width,
            height,
            dpi_scale: dpi.max(1) as f32 / 96.0,
            brushes,
        })
    }

    pub fn resize(&mut self, width: u32, height: u32, dpi: u32) -> Result<(), String> {
        if self.width == width
            && self.height == height
            && (self.dpi_scale - dpi as f32 / 96.0).abs() < 0.001
        {
            return Ok(());
        }
        unsafe {
            self.surface = create_rect_surface(&self.device.composition, width, height)?;
            self.visual
                .SetContent(&self.surface)
                .map_err(win_error("SetContent(panel resize)"))?;
            self.device
                .composition
                .Commit()
                .map_err(win_error("Commit(panel resize)"))?;
        }
        self.width = width;
        self.height = height;
        self.dpi_scale = dpi.max(1) as f32 / 96.0;
        Ok(())
    }

    pub fn render_menu(&mut self, data: &MenuPanelData<'_>) -> Result<(), String> {
        unsafe {
            let target = self.begin_panel_frame()?;
            self.draw_card(8.0, 8.0, 312.0, 538.0, 14.0);
            self.draw_panel_text(
                "小鲸鱼菜单",
                D2D_RECT_F {
                    left: 28.0,
                    top: 20.0,
                    right: 292.0,
                    bottom: 50.0,
                },
                18.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                PRIMARY_TEXT,
                DWRITE_TEXT_ALIGNMENT_LEADING,
            )?;
            self.draw_panel_text(
                "显示",
                D2D_RECT_F {
                    left: 28.0,
                    top: 56.0,
                    right: 292.0,
                    bottom: 78.0,
                },
                10.5,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                MUTED_TEXT,
                DWRITE_TEXT_ALIGNMENT_LEADING,
            )?;

            self.draw_row_label("大小", 82.0)?;
            self.draw_slider(
                104.0,
                96.0,
                136.0,
                ((data.scale - 0.6) / 1.9).clamp(0.0, 1.0),
            );
            self.draw_panel_text(
                &format!("{:.2}x", data.scale),
                D2D_RECT_F {
                    left: 248.0,
                    top: 82.0,
                    right: 294.0,
                    bottom: 110.0,
                },
                11.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                PRIMARY_TEXT,
                DWRITE_TEXT_ALIGNMENT_CENTER,
            )?;

            self.draw_row_label("音效", 122.0)?;
            self.draw_value(
                if data.sound_set == "fx1" {
                    "音效1"
                } else {
                    "小黄鸭"
                },
                122.0,
            )?;
            self.draw_row_label("音量", 160.0)?;
            self.draw_slider(104.0, 174.0, 136.0, data.volume.clamp(0.0, 1.0));
            self.draw_panel_text(
                &format!("{}%", (data.volume * 100.0).round() as i32),
                D2D_RECT_F {
                    left: 248.0,
                    top: 160.0,
                    right: 294.0,
                    bottom: 188.0,
                },
                11.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                PRIMARY_TEXT,
                DWRITE_TEXT_ALIGNMENT_CENTER,
            )?;
            self.draw_separator(196.0);
            self.draw_panel_text(
                "行为",
                D2D_RECT_F {
                    left: 28.0,
                    top: 204.0,
                    right: 292.0,
                    bottom: 224.0,
                },
                10.5,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                MUTED_TEXT,
                DWRITE_TEXT_ALIGNMENT_LEADING,
            )?;
            self.draw_toggle_row("思考气泡", 228.0, data.bubble_enabled)?;
            self.draw_toggle_row("始终置顶", 264.0, data.always_on_top)?;
            self.draw_toggle_row("开机启动", 300.0, data.autostart)?;
            self.draw_separator(338.0);
            self.draw_action("刷新数据", 346.0, PRIMARY_TEXT)?;
            self.draw_action("查看详细信息", 380.0, PRIMARY_TEXT)?;
            self.draw_action("数据设置", 414.0, PRIMARY_TEXT)?;
            self.draw_action("连接设置", 448.0, PRIMARY_TEXT)?;
            self.draw_action("退出", 482.0, DANGER_TEXT)?;
            self.end_panel_frame(target)
        }
    }

    pub fn render_setup(&mut self, data: &SetupPanelData<'_>) -> Result<(), String> {
        unsafe {
            let target = self.begin_panel_frame()?;
            self.draw_card(8.0, 8.0, 552.0, 332.0, 16.0);
            self.draw_panel_text(
                "连接 CPA Whale",
                D2D_RECT_F {
                    left: 30.0,
                    top: 22.0,
                    right: 500.0,
                    bottom: 54.0,
                },
                21.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                PRIMARY_TEXT,
                DWRITE_TEXT_ALIGNMENT_LEADING,
            )?;
            self.draw_panel_text(
                "×",
                D2D_RECT_F {
                    left: 510.0,
                    top: 18.0,
                    right: 540.0,
                    bottom: 50.0,
                },
                20.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                MUTED_TEXT,
                DWRITE_TEXT_ALIGNMENT_CENTER,
            )?;
            self.draw_panel_text(
                "只读取 CLIProxyAPI 统计；令牌由 Windows DPAPI 加密保存。",
                D2D_RECT_F {
                    left: 30.0,
                    top: 52.0,
                    right: 530.0,
                    bottom: 76.0,
                },
                11.5,
                DWRITE_FONT_WEIGHT_NORMAL,
                MUTED_TEXT,
                DWRITE_TEXT_ALIGNMENT_LEADING,
            )?;
            self.draw_panel_text(
                "Whale API 地址",
                D2D_RECT_F {
                    left: 34.0,
                    top: 88.0,
                    right: 520.0,
                    bottom: 110.0,
                },
                11.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                PRIMARY_TEXT,
                DWRITE_TEXT_ALIGNMENT_LEADING,
            )?;
            self.draw_input_backing(30.0, 112.0, 500.0, 40.0);
            self.draw_panel_text(
                "Whale 只读令牌",
                D2D_RECT_F {
                    left: 34.0,
                    top: 166.0,
                    right: 520.0,
                    bottom: 188.0,
                },
                11.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                PRIMARY_TEXT,
                DWRITE_TEXT_ALIGNMENT_LEADING,
            )?;
            self.draw_input_backing(30.0, 190.0, 500.0, 40.0);
            if let Some(status) = data.status {
                self.draw_panel_text(
                    status,
                    D2D_RECT_F {
                        left: 32.0,
                        top: 242.0,
                        right: 350.0,
                        bottom: 270.0,
                    },
                    11.0,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    if data.saving {
                        PRIMARY_TEXT
                    } else if status.contains("成功") {
                        GOOD_TEXT
                    } else {
                        DANGER_TEXT
                    },
                    DWRITE_TEXT_ALIGNMENT_LEADING,
                )?;
            }
            self.draw_button(
                378.0,
                252.0,
                152.0,
                48.0,
                if data.saving {
                    "连接中…"
                } else {
                    "保存并连接"
                },
            );
            self.end_panel_frame(target)
        }
    }

    pub fn render_data_settings(&mut self, data: &DataSettingsPanelData<'_>) -> Result<(), String> {
        unsafe {
            let target = self.begin_panel_frame()?;
            self.draw_card(8.0, 8.0, 312.0, 492.0, 14.0);
            self.draw_panel_text(
                "数据设置",
                D2D_RECT_F {
                    left: 28.0,
                    top: 20.0,
                    right: 270.0,
                    bottom: 50.0,
                },
                18.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                PRIMARY_TEXT,
                DWRITE_TEXT_ALIGNMENT_LEADING,
            )?;
            self.draw_panel_text(
                "×",
                D2D_RECT_F {
                    left: 274.0,
                    top: 16.0,
                    right: 304.0,
                    bottom: 48.0,
                },
                18.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                MUTED_TEXT,
                DWRITE_TEXT_ALIGNMENT_CENTER,
            )?;
            self.draw_row_label("关注模型", 66.0)?;
            self.draw_value(data.model, 66.0)?;
            self.draw_row_label("推理档位", 108.0)?;
            self.draw_value(data.reasoning_effort, 108.0)?;
            self.draw_separator(148.0);
            for (index, (label, enabled)) in data.cards.iter().enumerate() {
                self.draw_toggle_row(label, 162.0 + index as f32 * 38.0, *enabled)?;
            }
            self.draw_action("保存", 446.0, PRIMARY_TEXT)?;
            self.end_panel_frame(target)
        }
    }

    pub fn render_details(&mut self, data: &DetailsPanelData<'_>) -> Result<(), String> {
        unsafe {
            let target = self.begin_panel_frame()?;
            self.draw_card(8.0, 8.0, 612.0, 512.0, 16.0);
            self.draw_panel_text(
                "CLIProxyAPI 统计",
                D2D_RECT_F {
                    left: 30.0,
                    top: 22.0,
                    right: 590.0,
                    bottom: 54.0,
                },
                21.0,
                DWRITE_FONT_WEIGHT_SEMI_BOLD,
                PRIMARY_TEXT,
                DWRITE_TEXT_ALIGNMENT_LEADING,
            )?;
            self.draw_panel_text(
                "×",
                D2D_RECT_F {
                    left: 568.0,
                    top: 18.0,
                    right: 598.0,
                    bottom: 50.0,
                },
                20.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                MUTED_TEXT,
                DWRITE_TEXT_ALIGNMENT_CENTER,
            )?;
            self.draw_panel_text(
                "今日、启动后、模型、账户与信号",
                D2D_RECT_F {
                    left: 30.0,
                    top: 52.0,
                    right: 590.0,
                    bottom: 76.0,
                },
                11.0,
                DWRITE_FONT_WEIGHT_NORMAL,
                MUTED_TEXT,
                DWRITE_TEXT_ALIGNMENT_LEADING,
            )?;
            for (index, label) in ["概览", "模型", "账户", "信号"].iter().enumerate() {
                let left = 30.0 + index as f32 * 142.0;
                let rounded = windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT {
                    rect: D2D_RECT_F {
                        left,
                        top: 82.0,
                        right: left + 130.0,
                        bottom: 116.0,
                    },
                    radiusX: 9.0,
                    radiusY: 9.0,
                };
                if data.page == index {
                    self.brushes.navy.SetOpacity(0.9);
                    self.device
                        .context
                        .FillRoundedRectangle(&rounded, &self.brushes.navy);
                } else {
                    self.brushes.muted.SetOpacity(0.14);
                    self.device
                        .context
                        .FillRoundedRectangle(&rounded, &self.brushes.muted);
                }
                self.draw_panel_text(
                    label,
                    rounded.rect,
                    11.5,
                    DWRITE_FONT_WEIGHT_SEMI_BOLD,
                    if data.page == index {
                        WHITE
                    } else {
                        PRIMARY_TEXT
                    },
                    DWRITE_TEXT_ALIGNMENT_CENTER,
                )?;
            }
            if data.page == 0 {
                self.draw_metric_card("今日 Tokens", data.today_tokens, 30.0, 132.0, 265.0)?;
                self.draw_metric_card("USD 等价", data.today_usd, 315.0, 132.0, 265.0)?;
                self.draw_metric_card("挂件启动后", data.startup_tokens, 30.0, 230.0, 265.0)?;
                self.draw_metric_card(
                    "上游账户",
                    &format!("{} 个", data.accounts),
                    315.0,
                    230.0,
                    265.0,
                )?;
                self.draw_metric_card(
                    "外部信号",
                    &format!("{} 条", data.signals),
                    30.0,
                    328.0,
                    265.0,
                )?;
                self.draw_metric_card("数据来源", "CLIProxyAPI", 315.0, 328.0, 265.0)?;
            } else if data.rows.is_empty() {
                self.draw_panel_text(
                    "暂无可显示的数据",
                    D2D_RECT_F {
                        left: 30.0,
                        top: 160.0,
                        right: 590.0,
                        bottom: 210.0,
                    },
                    13.0,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    MUTED_TEXT,
                    DWRITE_TEXT_ALIGNMENT_CENTER,
                )?;
            } else {
                for (index, row) in data.rows.iter().take(7).enumerate() {
                    let top = 132.0 + index as f32 * 49.0;
                    let rounded = windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT {
                        rect: D2D_RECT_F {
                            left: 30.0,
                            top,
                            right: 590.0,
                            bottom: top + 40.0,
                        },
                        radiusX: 9.0,
                        radiusY: 9.0,
                    };
                    self.brushes.muted.SetOpacity(0.11);
                    self.device
                        .context
                        .FillRoundedRectangle(&rounded, &self.brushes.muted);
                    self.draw_panel_text(
                        row,
                        D2D_RECT_F {
                            left: 44.0,
                            top: top + 4.0,
                            right: 576.0,
                            bottom: top + 36.0,
                        },
                        11.5,
                        DWRITE_FONT_WEIGHT_NORMAL,
                        PRIMARY_TEXT,
                        DWRITE_TEXT_ALIGNMENT_LEADING,
                    )?;
                }
            }
            self.end_panel_frame(target)
        }
    }

    unsafe fn begin_panel_frame(&self) -> Result<ID2D1Bitmap1, String> {
        let mut offset = POINT::default();
        let dxgi_surface: IDXGISurface = self
            .surface
            .BeginDraw(None, &mut offset)
            .map_err(win_error("BeginDraw(panel)"))?;
        let properties =
            bitmap_properties(D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW);
        let target = self
            .device
            .context
            .CreateBitmapFromDxgiSurface(&dxgi_surface, Some(&properties))
            .map_err(win_error("CreateBitmapFromDxgiSurface(panel)"))?;
        self.device.context.SetTarget(&target);
        self.device
            .context
            .SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
        self.device.context.BeginDraw();
        self.device.context.Clear(Some(&D2D1_COLOR_F {
            r: 0.0,
            g: 0.0,
            b: 0.0,
            a: 0.0,
        }));
        self.device.context.SetTransform(&matrix_scale_translate(
            self.dpi_scale,
            self.dpi_scale,
            offset.x as f32,
            offset.y as f32,
        ));
        Ok(target)
    }

    unsafe fn end_panel_frame(&self, target: ID2D1Bitmap1) -> Result<(), String> {
        let draw = self.device.context.EndDraw(None, None);
        self.device
            .context
            .SetTarget(None::<&windows::Win32::Graphics::Direct2D::ID2D1Image>);
        drop(target);
        let surface = self.surface.EndDraw();
        draw.map_err(win_error("EndDraw(panel)"))?;
        surface.map_err(win_error("Surface::EndDraw(panel)"))?;
        self.device
            .composition
            .Commit()
            .map_err(win_error("Commit(panel)"))
    }

    unsafe fn draw_card(&self, left: f32, top: f32, right: f32, bottom: f32, radius: f32) {
        let rounded = windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left,
                top,
                right,
                bottom,
            },
            radiusX: radius,
            radiusY: radius,
        };
        self.brushes.white.SetOpacity(0.96);
        self.device
            .context
            .FillRoundedRectangle(&rounded, &self.brushes.white);
        self.brushes.navy.SetOpacity(0.32);
        self.device.context.DrawRoundedRectangle(
            &rounded,
            &self.brushes.navy,
            1.0,
            None::<&windows::Win32::Graphics::Direct2D::ID2D1StrokeStyle>,
        );
    }

    unsafe fn draw_input_backing(&self, left: f32, top: f32, width: f32, height: f32) {
        let rounded = windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left,
                top,
                right: left + width,
                bottom: top + height,
            },
            radiusX: 9.0,
            radiusY: 9.0,
        };
        self.brushes.white.SetOpacity(1.0);
        self.device
            .context
            .FillRoundedRectangle(&rounded, &self.brushes.white);
        self.brushes.navy.SetOpacity(0.28);
        self.device.context.DrawRoundedRectangle(
            &rounded,
            &self.brushes.navy,
            1.0,
            None::<&windows::Win32::Graphics::Direct2D::ID2D1StrokeStyle>,
        );
    }

    unsafe fn draw_button(&self, left: f32, top: f32, width: f32, height: f32, label: &str) {
        let rounded = windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left,
                top,
                right: left + width,
                bottom: top + height,
            },
            radiusX: 10.0,
            radiusY: 10.0,
        };
        self.brushes.navy.SetOpacity(0.94);
        self.device
            .context
            .FillRoundedRectangle(&rounded, &self.brushes.navy);
        let _ = self.draw_panel_text(
            label,
            rounded.rect,
            12.5,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            WHITE,
            DWRITE_TEXT_ALIGNMENT_CENTER,
        );
    }

    unsafe fn draw_row_label(&self, label: &str, top: f32) -> Result<(), String> {
        self.draw_panel_text(
            label,
            D2D_RECT_F {
                left: 28.0,
                top,
                right: 100.0,
                bottom: top + 28.0,
            },
            11.5,
            DWRITE_FONT_WEIGHT_NORMAL,
            PRIMARY_TEXT,
            DWRITE_TEXT_ALIGNMENT_LEADING,
        )
    }

    unsafe fn draw_value(&self, value: &str, top: f32) -> Result<(), String> {
        self.draw_panel_text(
            value,
            D2D_RECT_F {
                left: 104.0,
                top,
                right: 294.0,
                bottom: top + 28.0,
            },
            11.5,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            PRIMARY_TEXT,
            DWRITE_TEXT_ALIGNMENT_LEADING,
        )
    }

    unsafe fn draw_slider(&self, left: f32, center_y: f32, width: f32, progress: f32) {
        let progress = progress.clamp(0.0, 1.0);
        let background = D2D_RECT_F {
            left,
            top: center_y - 2.0,
            right: left + width,
            bottom: center_y + 2.0,
        };
        self.brushes.muted.SetOpacity(0.42);
        self.device
            .context
            .FillRectangle(&background, &self.brushes.muted);
        let foreground = D2D_RECT_F {
            right: left + width * progress,
            ..background
        };
        self.brushes.navy.SetOpacity(0.9);
        self.device
            .context
            .FillRectangle(&foreground, &self.brushes.navy);
        let knob = D2D1_ELLIPSE {
            point: Vector2 {
                X: left + width * progress,
                Y: center_y,
            },
            radiusX: 6.0,
            radiusY: 6.0,
        };
        self.device.context.FillEllipse(&knob, &self.brushes.navy);
    }

    unsafe fn draw_toggle_row(&self, label: &str, top: f32, enabled: bool) -> Result<(), String> {
        self.draw_row_label(label, top)?;
        let left = 254.0;
        let switch = windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left,
                top: top + 4.0,
                right: left + 40.0,
                bottom: top + 24.0,
            },
            radiusX: 10.0,
            radiusY: 10.0,
        };
        if enabled {
            self.brushes.navy.SetOpacity(0.9);
            self.device
                .context
                .FillRoundedRectangle(&switch, &self.brushes.navy);
        } else {
            self.brushes.muted.SetOpacity(0.45);
            self.device
                .context
                .FillRoundedRectangle(&switch, &self.brushes.muted);
        }
        let center_x = if enabled { left + 30.0 } else { left + 10.0 };
        let knob = D2D1_ELLIPSE {
            point: Vector2 {
                X: center_x,
                Y: top + 14.0,
            },
            radiusX: 7.0,
            radiusY: 7.0,
        };
        self.brushes.white.SetOpacity(1.0);
        self.device.context.FillEllipse(&knob, &self.brushes.white);
        Ok(())
    }

    unsafe fn draw_separator(&self, y: f32) {
        self.brushes.navy.SetOpacity(0.18);
        self.device.context.DrawLine(
            Vector2 { X: 28.0, Y: y },
            Vector2 { X: 292.0, Y: y },
            &self.brushes.navy,
            1.0,
            None::<&windows::Win32::Graphics::Direct2D::ID2D1StrokeStyle>,
        );
    }

    unsafe fn draw_action(&self, label: &str, top: f32, color: Color) -> Result<(), String> {
        let rounded = windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left: 28.0,
                top,
                right: 292.0,
                bottom: top + 30.0,
            },
            radiusX: 8.0,
            radiusY: 8.0,
        };
        self.brushes.muted.SetOpacity(0.09);
        self.device
            .context
            .FillRoundedRectangle(&rounded, &self.brushes.muted);
        self.draw_panel_text(
            label,
            D2D_RECT_F {
                left: 40.0,
                top,
                right: 282.0,
                bottom: top + 30.0,
            },
            11.5,
            DWRITE_FONT_WEIGHT_SEMI_BOLD,
            color,
            DWRITE_TEXT_ALIGNMENT_LEADING,
        )
    }

    unsafe fn draw_metric_card(
        &self,
        label: &str,
        value: &str,
        left: f32,
        top: f32,
        width: f32,
    ) -> Result<(), String> {
        let rounded = windows::Win32::Graphics::Direct2D::D2D1_ROUNDED_RECT {
            rect: D2D_RECT_F {
                left,
                top,
                right: left + width,
                bottom: top + 78.0,
            },
            radiusX: 12.0,
            radiusY: 12.0,
        };
        self.brushes.muted.SetOpacity(0.12);
        self.device
            .context
            .FillRoundedRectangle(&rounded, &self.brushes.muted);
        self.draw_panel_text(
            label,
            D2D_RECT_F {
                left: left + 14.0,
                top: top + 10.0,
                right: left + width - 14.0,
                bottom: top + 32.0,
            },
            10.5,
            DWRITE_FONT_WEIGHT_NORMAL,
            MUTED_TEXT,
            DWRITE_TEXT_ALIGNMENT_LEADING,
        )?;
        self.draw_panel_text(
            value,
            D2D_RECT_F {
                left: left + 14.0,
                top: top + 32.0,
                right: left + width - 14.0,
                bottom: top + 68.0,
            },
            18.0,
            DWRITE_FONT_WEIGHT_EXTRA_BOLD,
            PRIMARY_TEXT,
            DWRITE_TEXT_ALIGNMENT_LEADING,
        )
    }

    unsafe fn draw_panel_text(
        &self,
        text: &str,
        rect: D2D_RECT_F,
        size: f32,
        weight: DWRITE_FONT_WEIGHT,
        color: Color,
        alignment: windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_ALIGNMENT,
    ) -> Result<(), String> {
        let format = create_text_format(&self.device.write_factory, size, weight)?;
        format
            .SetTextAlignment(alignment)
            .map_err(win_error("SetTextAlignment(panel)"))?;
        let brush = self.brushes.for_color(color);
        brush.SetOpacity(color.a);
        let utf16 = text.encode_utf16().collect::<Vec<_>>();
        let render_target: &ID2D1RenderTarget = &self.device.context;
        render_target.DrawText(
            &utf16,
            &format,
            &rect,
            brush,
            windows::Win32::Graphics::Direct2D::D2D1_DRAW_TEXT_OPTIONS_CLIP,
            DWRITE_MEASURING_MODE_NATURAL,
        );
        Ok(())
    }
}

struct DeviceResources {
    #[allow(dead_code)]
    d3d: ID3D11Device,
    #[allow(dead_code)]
    immediate: ID3D11DeviceContext,
    factory: ID2D1Factory1,
    #[allow(dead_code)]
    d2d: ID2D1Device,
    context: ID2D1DeviceContext,
    write_factory: IDWriteFactory,
    composition: IDCompositionDevice,
}

impl DeviceResources {
    unsafe fn new() -> Result<(Self, bool), String> {
        let (d3d, immediate, hardware) = create_d3d_device()?;
        let dxgi: IDXGIDevice = d3d
            .cast()
            .map_err(win_error("ID3D11Device::cast IDXGIDevice"))?;
        let factory: ID2D1Factory1 = D2D1CreateFactory(
            D2D1_FACTORY_TYPE_SINGLE_THREADED,
            None::<*const D2D1_FACTORY_OPTIONS>,
        )
        .map_err(win_error("D2D1CreateFactory"))?;
        let d2d = factory
            .CreateDevice(&dxgi)
            .map_err(win_error("ID2D1Factory1::CreateDevice"))?;
        let context = d2d
            .CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)
            .map_err(win_error("ID2D1Device::CreateDeviceContext"))?;
        let write_factory: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)
            .map_err(win_error("DWriteCreateFactory"))?;
        let composition: IDCompositionDevice =
            DCompositionCreateDevice(&dxgi).map_err(win_error("DCompositionCreateDevice"))?;
        Ok((
            Self {
                d3d,
                immediate,
                factory,
                d2d,
                context,
                write_factory,
                composition,
            },
            hardware,
        ))
    }
}

unsafe fn create_d3d_device() -> Result<(ID3D11Device, ID3D11DeviceContext, bool), String> {
    let levels: [D3D_FEATURE_LEVEL; 4] = [
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
    ];
    match create_d3d(D3D_DRIVER_TYPE_HARDWARE, &levels) {
        Ok((device, context)) => Ok((device, context, true)),
        Err(hardware_error) => create_d3d(D3D_DRIVER_TYPE_WARP, &levels)
            .map(|(device, context)| (device, context, false))
            .map_err(|warp_error| {
                format!(
                    "D3D11 hardware initialization failed: {hardware_error}; WARP fallback failed: {warp_error}"
                )
            }),
    }
}

unsafe fn create_d3d(
    driver: windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE,
    levels: &[D3D_FEATURE_LEVEL],
) -> Result<(ID3D11Device, ID3D11DeviceContext), String> {
    let mut device = None;
    let mut context = None;
    let mut selected = D3D_FEATURE_LEVEL_10_0;
    D3D11CreateDevice(
        None::<&windows::Win32::Graphics::Dxgi::IDXGIAdapter>,
        driver,
        HMODULE::default(),
        D3D11_CREATE_DEVICE_BGRA_SUPPORT,
        Some(levels),
        D3D11_SDK_VERSION,
        Some(&mut device),
        Some(&mut selected),
        Some(&mut context),
    )
    .map_err(win_error("D3D11CreateDevice"))?;
    let device = device.ok_or_else(|| "D3D11CreateDevice returned no device".to_string())?;
    let context = context.ok_or_else(|| "D3D11CreateDevice returned no context".to_string())?;
    Ok((device, context))
}

struct GpuAssets {
    whale: ID2D1Bitmap1,
    gif_frames: Vec<ID2D1Bitmap1>,
    whale_alpha: Vec<u8>,
    whale_mask_width: usize,
    whale_mask_height: usize,
}

impl GpuAssets {
    unsafe fn new(context: &ID2D1DeviceContext) -> Result<Self, String> {
        let whale_image = image::load_from_memory(assets::WHALE_PNG)
            .map_err(|error| format!("decode whale PNG: {error}"))?
            .to_rgba8();
        let whale_mask_width = whale_image.width() as usize;
        let whale_mask_height = whale_image.height() as usize;
        let whale_alpha = whale_image
            .pixels()
            .map(|pixel| pixel[3])
            .collect::<Vec<_>>();
        let whale = create_bitmap(
            context,
            whale_image.width(),
            whale_image.height(),
            whale_image.as_raw(),
        )?;

        let decoder = image::codecs::gif::GifDecoder::new(Cursor::new(assets::RUA_GIF))
            .map_err(|error| format!("decode Rua GIF: {error}"))?;
        let gif_frames = decoder
            .into_frames()
            .collect_frames()
            .map_err(|error| format!("decode Rua GIF frames: {error}"))?
            .into_iter()
            .map(|frame| {
                let image = frame.into_buffer();
                create_bitmap(context, image.width(), image.height(), image.as_raw())
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            whale,
            gif_frames,
            whale_alpha,
            whale_mask_width,
            whale_mask_height,
        })
    }
}

unsafe fn create_bitmap(
    context: &ID2D1DeviceContext,
    width: u32,
    height: u32,
    rgba: &[u8],
) -> Result<ID2D1Bitmap1, String> {
    let mut bgra_premultiplied = Vec::with_capacity(rgba.len());
    for pixel in rgba.chunks_exact(4) {
        let alpha = pixel[3] as u16;
        bgra_premultiplied.push((pixel[2] as u16 * alpha / 255) as u8);
        bgra_premultiplied.push((pixel[1] as u16 * alpha / 255) as u8);
        bgra_premultiplied.push((pixel[0] as u16 * alpha / 255) as u8);
        bgra_premultiplied.push(pixel[3]);
    }
    let properties = bitmap_properties(D2D1_BITMAP_OPTIONS_NONE);
    context
        .CreateBitmap(
            D2D_SIZE_U { width, height },
            Some(bgra_premultiplied.as_ptr().cast()),
            width * 4,
            &properties,
        )
        .map_err(win_error("ID2D1DeviceContext::CreateBitmap"))
}

struct BubbleGeometry {
    main: ID2D1PathGeometry1,
    tail_1: ID2D1PathGeometry1,
    tail_2: ID2D1PathGeometry1,
}

impl BubbleGeometry {
    unsafe fn new(factory: &ID2D1Factory1) -> Result<Self, String> {
        Ok(Self {
            main: create_main_bubble(factory)?,
            tail_1: create_ellipse_path(factory, 352.0, 561.0, 37.5, 26.0)?,
            tail_2: create_ellipse_path(factory, 442.0, 646.0, 24.5, 18.0)?,
        })
    }
}

unsafe fn create_main_bubble(factory: &ID2D1Factory1) -> Result<ID2D1PathGeometry1, String> {
    let geometry = factory
        .CreatePathGeometry()
        .map_err(win_error("ID2D1Factory1::CreatePathGeometry"))?;
    let sink = geometry
        .Open()
        .map_err(win_error("ID2D1PathGeometry::Open"))?;
    sink.BeginFigure(Vector2 { X: 827.0, Y: 248.0 }, D2D1_FIGURE_BEGIN_FILLED);
    for arc in [
        arc(81.0, 246.0, 373.0, 232.0, 0.0, true),
        arc(301.0, 465.0, 373.0, 232.0, 0.0, false),
        arc(413.0, 484.0, 57.0, 32.0, 10.0, false),
        arc(827.0, 248.0, 373.0, 232.0, 0.0, false),
    ] {
        sink.AddArc(&arc);
    }
    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
    sink.Close()
        .map_err(win_error("ID2D1GeometrySink::Close"))?;
    Ok(geometry)
}

unsafe fn create_ellipse_path(
    factory: &ID2D1Factory1,
    cx: f32,
    cy: f32,
    rx: f32,
    ry: f32,
) -> Result<ID2D1PathGeometry1, String> {
    let geometry = factory
        .CreatePathGeometry()
        .map_err(win_error("ID2D1Factory1::CreatePathGeometry"))?;
    let sink = geometry
        .Open()
        .map_err(win_error("ID2D1PathGeometry::Open"))?;
    sink.BeginFigure(Vector2 { X: cx + rx, Y: cy }, D2D1_FIGURE_BEGIN_FILLED);
    sink.AddArc(&arc(cx - rx, cy, rx, ry, 0.0, false));
    sink.AddArc(&arc(cx + rx, cy, rx, ry, 0.0, false));
    sink.EndFigure(D2D1_FIGURE_END_CLOSED);
    sink.Close()
        .map_err(win_error("ID2D1GeometrySink::Close"))?;
    Ok(geometry)
}

fn arc(x: f32, y: f32, rx: f32, ry: f32, rotation: f32, large: bool) -> D2D1_ARC_SEGMENT {
    D2D1_ARC_SEGMENT {
        point: Vector2 { X: x, Y: y },
        size: D2D_SIZE_F {
            width: rx,
            height: ry,
        },
        rotationAngle: rotation,
        sweepDirection: D2D1_SWEEP_DIRECTION_COUNTER_CLOCKWISE,
        arcSize: if large {
            D2D1_ARC_SIZE_LARGE
        } else {
            D2D1_ARC_SIZE_SMALL
        },
    }
}

struct Brushes {
    white: ID2D1SolidColorBrush,
    navy: ID2D1SolidColorBrush,
    primary: ID2D1SolidColorBrush,
    muted: ID2D1SolidColorBrush,
    good: ID2D1SolidColorBrush,
    danger: ID2D1SolidColorBrush,
}

impl Brushes {
    unsafe fn new(context: &ID2D1DeviceContext) -> Result<Self, String> {
        Ok(Self {
            white: create_brush(context, WHITE)?,
            navy: create_brush(context, NAVY)?,
            primary: create_brush(context, PRIMARY_TEXT)?,
            muted: create_brush(context, MUTED_TEXT)?,
            good: create_brush(context, GOOD_TEXT)?,
            danger: create_brush(context, DANGER_TEXT)?,
        })
    }

    fn for_color(&self, color: Color) -> &ID2D1SolidColorBrush {
        if color == MUTED_TEXT {
            &self.muted
        } else if color == GOOD_TEXT {
            &self.good
        } else if color == DANGER_TEXT {
            &self.danger
        } else if color == NAVY {
            &self.navy
        } else {
            &self.primary
        }
    }
}

unsafe fn create_brush(
    context: &ID2D1DeviceContext,
    color: Color,
) -> Result<ID2D1SolidColorBrush, String> {
    context
        .CreateSolidColorBrush(
            &D2D1_COLOR_F {
                r: color.r,
                g: color.g,
                b: color.b,
                a: color.a,
            },
            None,
        )
        .map_err(win_error("ID2D1RenderTarget::CreateSolidColorBrush"))
}

struct TextFormats {
    label: IDWriteTextFormat,
    amount: IDWriteTextFormat,
    period: IDWriteTextFormat,
    hint: IDWriteTextFormat,
    wrapped: IDWriteTextFormat,
}

impl TextFormats {
    unsafe fn new(factory: &IDWriteFactory) -> Result<Self, String> {
        Ok(Self {
            label: create_text_format(factory, 66.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
            amount: create_text_format(factory, 128.0, DWRITE_FONT_WEIGHT_EXTRA_BOLD)?,
            period: create_text_format(factory, 104.0, DWRITE_FONT_WEIGHT_EXTRA_BOLD)?,
            hint: create_text_format(factory, 48.0, DWRITE_FONT_WEIGHT_NORMAL)?,
            wrapped: create_text_format(factory, 56.0, DWRITE_FONT_WEIGHT_SEMI_BOLD)?,
        })
    }

    fn for_spec(&self, spec: &TextSpec) -> &IDWriteTextFormat {
        if spec.wrap {
            &self.wrapped
        } else if spec.size >= 120.0 {
            &self.amount
        } else if spec.size >= 100.0 {
            &self.period
        } else if spec.size <= 56.0 {
            &self.hint
        } else {
            &self.label
        }
    }
}

unsafe fn create_text_format(
    factory: &IDWriteFactory,
    size: f32,
    weight: DWRITE_FONT_WEIGHT,
) -> Result<IDWriteTextFormat, String> {
    let family = wide("Microsoft YaHei UI");
    let locale = wide("zh-CN");
    let format = factory
        .CreateTextFormat(
            windows::core::PCWSTR(family.as_ptr()),
            None::<&IDWriteFontCollection>,
            weight,
            DWRITE_FONT_STYLE_NORMAL,
            DWRITE_FONT_STRETCH_NORMAL,
            size,
            windows::core::PCWSTR(locale.as_ptr()),
        )
        .map_err(win_error("IDWriteFactory::CreateTextFormat"))?;
    format
        .SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)
        .map_err(win_error("IDWriteTextFormat::SetTextAlignment"))?;
    format
        .SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)
        .map_err(win_error("IDWriteTextFormat::SetParagraphAlignment"))?;
    Ok(format)
}

unsafe fn create_surface(
    composition: &IDCompositionDevice,
    size: u32,
) -> Result<IDCompositionSurface, String> {
    create_rect_surface(composition, size, size)
}

unsafe fn create_rect_surface(
    composition: &IDCompositionDevice,
    width: u32,
    height: u32,
) -> Result<IDCompositionSurface, String> {
    composition
        .CreateSurface(
            width.max(1),
            height.max(1),
            DXGI_FORMAT_B8G8R8A8_UNORM,
            DXGI_ALPHA_MODE_PREMULTIPLIED,
        )
        .map_err(win_error("IDCompositionDevice::CreateSurface"))
}

fn bitmap_properties(
    options: windows::Win32::Graphics::Direct2D::D2D1_BITMAP_OPTIONS,
) -> D2D1_BITMAP_PROPERTIES1 {
    D2D1_BITMAP_PROPERTIES1 {
        pixelFormat: D2D1_PIXEL_FORMAT {
            format: DXGI_FORMAT_B8G8R8A8_UNORM,
            alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
        },
        dpiX: 96.0,
        dpiY: 96.0,
        bitmapOptions: options,
        colorContext: ManuallyDrop::new(None),
    }
}

fn text_height(spec: &TextSpec) -> f32 {
    if spec.wrap {
        return spec.size * spec.line_count.max(1) as f32 * 1.34 + 28.0;
    }
    if spec.size >= 120.0 {
        spec.size * 1.16 + 10.0
    } else if spec.size <= 56.0 {
        spec.size * 1.30 + 12.0
    } else {
        spec.size * 1.24 + 10.0
    }
}

fn matrix_identity() -> Matrix3x2 {
    Matrix3x2 {
        M11: 1.0,
        M22: 1.0,
        ..Matrix3x2::default()
    }
}

fn matrix_scale_translate(sx: f32, sy: f32, tx: f32, ty: f32) -> Matrix3x2 {
    Matrix3x2 {
        M11: sx,
        M22: sy,
        M31: tx,
        M32: ty,
        ..Matrix3x2::default()
    }
}

fn matrix_scale_at(sx: f32, sy: f32, cx: f32, cy: f32) -> Matrix3x2 {
    Matrix3x2 {
        M11: sx,
        M22: sy,
        M31: cx * (1.0 - sx),
        M32: cy * (1.0 - sy),
        ..Matrix3x2::default()
    }
}

fn matrix_multiply(left: Matrix3x2, right: Matrix3x2) -> Matrix3x2 {
    Matrix3x2 {
        M11: left.M11 * right.M11 + left.M12 * right.M21,
        M12: left.M11 * right.M12 + left.M12 * right.M22,
        M21: left.M21 * right.M11 + left.M22 * right.M21,
        M22: left.M21 * right.M12 + left.M22 * right.M22,
        M31: left.M31 * right.M11 + left.M32 * right.M21 + right.M31,
        M32: left.M31 * right.M12 + left.M32 * right.M22 + right.M32,
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn win_error(operation: &'static str) -> impl FnOnce(windows::core::Error) -> String {
    move |error| format!("{operation}: {error}")
}
