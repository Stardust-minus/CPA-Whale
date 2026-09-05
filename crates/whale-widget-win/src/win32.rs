use std::mem::{size_of, zeroed};
use std::path::PathBuf;
use std::ptr;
use std::time::{Duration, Instant};

use whale_protocol::{CapabilitiesResponse, GlobalSnapshot};
use windows::Win32::Graphics::Dwm::{DwmGetCompositionTimingInfo, DWM_TIMING_INFO};
use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
use windows::Win32::UI::HiDpi::{
    GetDpiForWindow, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows_sys::Win32::Foundation::{GetLastError, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CombineRgn, CreateEllipticRgn, CreateRectRgn, DeleteObject, GetMonitorInfoW, MonitorFromWindow,
    SetWindowRgn, ValidateRect, MONITORINFO, MONITOR_DEFAULTTONEAREST, RGN_OR,
};
use windows_sys::Win32::Media::Audio::{PlaySoundW, SND_ASYNC, SND_MEMORY, SND_NODEFAULT};
use windows_sys::Win32::Media::{timeBeginPeriod, timeEndPeriod};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, TrackMouseEvent, TME_LEAVE, TRACKMOUSEEVENT,
};
use windows_sys::Win32::UI::Shell::{
    Shell_NotifyIconW, NIF_ICON, NIF_MESSAGE, NIF_TIP, NIM_ADD, NIM_DELETE, NOTIFYICONDATAW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::animation::{
    BubbleAnimation, ContentSwapAnimation, ContentSwapPhase, Easing, ScalarAnimation,
};
use crate::assets;
use crate::graphics::GpuRenderer;
use crate::layout::{scaled_fixed_corner, widget_base_px, RectF, DESIGN_SIZE};
use crate::model::{
    estimated_today_usd_micros, BubbleAction, BubbleContent, ClientSettings, RandomCard,
    RuntimeState,
};
use crate::network::{NetworkHandle, WM_CAPABILITIES, WM_NETWORK_ERROR, WM_SNAPSHOT};
use crate::panel::{
    self, DataSettingsResult, ACTION_AUTOSTART, ACTION_BUBBLE, ACTION_DATA_SETTINGS,
    ACTION_DETAILS, ACTION_EXIT, ACTION_REFRESH, ACTION_SETUP, ACTION_SOUND_SET, ACTION_TOPMOST,
    WM_DATA_SETTINGS_SAVED, WM_MENU_ACTION, WM_MENU_CLOSED, WM_MENU_SCALE, WM_MENU_VOLUME,
};
use crate::render::{VisualState, WidgetScene};
use crate::settings::{self, LoadedSettings};
use crate::setup::{self, SetupResult, WM_CONFIG_SAVED};

const CLASS_NAME: &str = "CPAWhaleWidgetWindowV2";
const WM_TRAY: u32 = WM_APP + 10;
const TIMER_BUBBLE: usize = 1;
const TIMER_ANIMATION: usize = 2;
const DEFAULT_ANIMATION_INTERVAL_MS: u32 = 4;
const WM_MOUSELEAVE_MESSAGE: u32 = 0x02a3;

struct DragState {
    start_cursor: POINT,
    start_rect: RECT,
    moved: bool,
}

struct SnapAnimation {
    from: POINT,
    to: POINT,
    started: Instant,
    duration: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RegionSignature {
    width: i32,
    height: i32,
    flip_mode: u8,
    bubble: bool,
    menu_button: bool,
}

struct NumberRoll {
    from_tokens: i64,
    to_tokens: i64,
    from_usd: Option<i64>,
    to_usd: Option<i64>,
    started: Instant,
    duration: Duration,
}

impl NumberRoll {
    fn sample(&self, now: Instant) -> (i64, Option<i64>, bool) {
        let progress = now.saturating_duration_since(self.started).as_secs_f64()
            / self.duration.as_secs_f64().max(f64::EPSILON);
        let done = progress >= 1.0;
        let progress = progress.clamp(0.0, 1.0);
        let eased = 1.0 - (1.0 - progress).powi(3);
        let tokens = lerp_i64(self.from_tokens, self.to_tokens, eased);
        let usd = match (self.from_usd, self.to_usd) {
            (Some(from), Some(to)) => Some(lerp_i64(from, to, eased)),
            (_, target) if done => target,
            (current, _) => current,
        };
        (tokens, usd, done)
    }
}

struct TimerResolution;

impl TimerResolution {
    fn one_millisecond() -> Self {
        unsafe {
            timeBeginPeriod(1);
        }
        Self
    }
}

impl Drop for TimerResolution {
    fn drop(&mut self) {
        unsafe {
            timeEndPeriod(1);
        }
    }
}

struct App {
    hwnd: HWND,
    graphics: Option<GpuRenderer>,
    settings: ClientSettings,
    runtime: RuntimeState,
    settings_path: PathBuf,
    token: String,
    visual: VisualState,
    flipped: bool,
    base_dip: f32,
    animation_interval_ms: u32,
    bubble_animation: Option<BubbleAnimation>,
    content_swap: Option<ContentSwapAnimation>,
    press_animation: Option<ScalarAnimation>,
    hover_animation: Option<ScalarAnimation>,
    mirror_animation: Option<ScalarAnimation>,
    number_roll: Option<NumberRoll>,
    drag: Option<DragState>,
    bubble_click: bool,
    menu_button_click: bool,
    snap_animation: Option<SnapAnimation>,
    network: Option<NetworkHandle>,
    menu_hwnd: HWND,
    tray: NOTIFYICONDATAW,
    gif_started: Option<Instant>,
    region_signature: Option<RegionSignature>,
}

impl App {
    fn new(loaded: LoadedSettings) -> Self {
        Self {
            hwnd: ptr::null_mut(),
            graphics: None,
            settings: loaded.settings,
            runtime: RuntimeState::default(),
            settings_path: loaded.path,
            token: loaded.token,
            visual: VisualState::default(),
            flipped: false,
            base_dip: 375.0,
            animation_interval_ms: DEFAULT_ANIMATION_INTERVAL_MS,
            bubble_animation: None,
            content_swap: None,
            press_animation: None,
            hover_animation: None,
            mirror_animation: None,
            number_roll: None,
            drag: None,
            bubble_click: false,
            menu_button_click: false,
            snap_animation: None,
            network: None,
            menu_hwnd: ptr::null_mut(),
            tray: unsafe { zeroed() },
            gif_started: None,
            region_signature: None,
        }
    }

    fn initialize_graphics(&mut self, size: u32) -> Result<(), String> {
        let renderer = unsafe { GpuRenderer::new(self.hwnd, size) }?;
        self.graphics = Some(renderer);
        self.region_signature = None;
        Ok(())
    }

    fn start_network(&mut self) {
        if self.token.is_empty() || self.settings.endpoint.is_empty() {
            self.runtime.apply_network_error("尚未配置 CPA 连接");
            self.open_data_bubble(false);
            return;
        }
        self.runtime.begin_refresh();
        self.network = Some(NetworkHandle::start(
            self.hwnd,
            self.settings.endpoint.clone(),
            self.token.clone(),
        ));
    }

    fn restart_network(&mut self) {
        if let Some(mut network) = self.network.take() {
            network.shutdown();
        }
        self.start_network();
    }

    fn refresh(&mut self) {
        self.runtime.begin_refresh();
        if let Some(network) = &self.network {
            network.refresh();
        }
        self.render();
    }

    fn render(&mut self) {
        self.update_window_region();
        let scene = WidgetScene::build(&self.settings, &self.runtime, self.flipped, self.visual);
        let result = self.graphics.as_mut().map_or_else(
            || Err("GPU 渲染器尚未初始化".to_string()),
            |graphics| graphics.render(&scene, self.base_dip),
        );
        if let Err(first_error) = result {
            let size = window_rect(self.hwnd)
                .map(|rect| (rect.right - rect.left).max(1) as u32)
                .unwrap_or(375);
            self.graphics = None;
            match self.initialize_graphics(size).and_then(|()| {
                self.graphics
                    .as_mut()
                    .expect("renderer was just initialized")
                    .render(&scene, self.base_dip)
            }) {
                Ok(()) => {}
                Err(recovery_error) => self.runtime.apply_network_error(format!(
                    "GPU 渲染失败: {first_error}; 设备重建失败: {recovery_error}"
                )),
            }
        }
    }

    fn save(&mut self) {
        if let Ok(rect) = window_rect(self.hwnd) {
            self.settings.x = Some(rect.left);
            self.settings.y = Some(rect.top);
        }
        if let Err(error) = settings::save(&self.settings_path, &self.settings) {
            self.runtime.apply_network_error(error);
        }
    }

    fn on_snapshot(&mut self, snapshot: GlobalSnapshot) {
        let target_tokens = snapshot.today.tokens.total_tokens;
        let target_usd = estimated_today_usd_micros(&snapshot);
        let previous_tokens = self.runtime.display_today_tokens.or_else(|| {
            self.runtime
                .snapshot
                .as_ref()
                .map(|value| value.today.tokens.total_tokens)
        });
        let previous_usd = self.runtime.display_today_usd_micros.or_else(|| {
            self.runtime
                .snapshot
                .as_ref()
                .and_then(estimated_today_usd_micros)
        });
        let animate = self.runtime.bubble_open
            && matches!(self.runtime.bubble_content, BubbleContent::Data)
            && previous_tokens.is_some_and(|value| value != target_tokens);
        self.runtime.apply_snapshot(&mut self.settings, snapshot);
        if animate {
            let from_tokens = previous_tokens.unwrap_or(target_tokens);
            self.runtime.display_today_tokens = Some(from_tokens);
            self.runtime.display_today_usd_micros = previous_usd;
            self.number_roll = Some(NumberRoll {
                from_tokens,
                to_tokens: target_tokens,
                from_usd: previous_usd,
                to_usd: target_usd,
                started: Instant::now(),
                duration: Duration::from_millis(700),
            });
            self.start_animation();
        } else {
            self.runtime.display_today_tokens = Some(target_tokens);
            self.runtime.display_today_usd_micros = target_usd;
            self.number_roll = None;
        }
        self.save();
        self.render();
    }

    fn open_data_bubble(&mut self, auto_close: bool) {
        self.runtime.open_data_bubble(self.settings.bubble_enabled);
        if !self.runtime.bubble_open {
            return;
        }
        self.gif_started = None;
        self.bubble_animation = Some(BubbleAnimation::opening(Instant::now()));
        self.visual.content_opacity = 1.0;
        unsafe {
            KillTimer(self.hwnd, TIMER_BUBBLE);
            if auto_close {
                SetTimer(self.hwnd, TIMER_BUBBLE, 5_000, None);
            }
        }
        self.start_animation();
    }

    fn close_bubble(&mut self) {
        if !self.runtime.bubble_open {
            return;
        }
        self.content_swap = None;
        self.runtime.pending_content = None;
        self.bubble_animation = Some(BubbleAnimation::closing(Instant::now()));
        unsafe { KillTimer(self.hwnd, TIMER_BUBBLE) };
        self.start_animation();
    }

    fn on_whale_click(&mut self) {
        self.open_data_bubble(true);
        self.refresh();
    }

    fn on_bubble_click(&mut self) {
        match self.runtime.begin_random_transition(&self.settings) {
            BubbleAction::SwitchContent => {
                self.content_swap = Some(ContentSwapAnimation::new(Instant::now()));
                unsafe {
                    KillTimer(self.hwnd, TIMER_BUBBLE);
                    SetTimer(self.hwnd, TIMER_BUBBLE, 5_000, None);
                }
                self.start_animation();
            }
            BubbleAction::Close => self.close_bubble(),
            BubbleAction::None => {}
        }
    }

    fn open_setup(&mut self) {
        if let Err(error) = unsafe { setup::show(self.hwnd, &self.settings.endpoint) } {
            self.runtime.apply_network_error(error);
            self.open_data_bubble(true);
        }
    }

    fn open_menu(&mut self) {
        unsafe {
            if !self.menu_hwnd.is_null() && IsWindow(self.menu_hwnd) != 0 {
                DestroyWindow(self.menu_hwnd);
                self.menu_hwnd = ptr::null_mut();
                return;
            }
        }
        let hardware = self
            .graphics
            .as_ref()
            .is_some_and(GpuRenderer::is_hardware_accelerated);
        match unsafe { panel::show_menu(self.hwnd, &self.settings, self.flipped, hardware) } {
            Ok(hwnd) => {
                self.menu_hwnd = hwnd;
                self.set_hovered(true);
            }
            Err(error) => {
                self.runtime.apply_network_error(error);
                self.open_data_bubble(true);
            }
        }
    }

    fn open_details(&mut self) {
        let hardware = self
            .graphics
            .as_ref()
            .is_some_and(GpuRenderer::is_hardware_accelerated);
        if let Err(error) = unsafe {
            panel::show_details(
                self.hwnd,
                self.runtime.capabilities.as_ref(),
                self.runtime.snapshot.as_ref(),
                self.runtime.startup_delta.as_ref(),
                hardware,
            )
        } {
            self.runtime.apply_network_error(error);
            self.open_data_bubble(true);
        }
    }

    fn open_data_settings(&mut self) {
        if let Err(error) = unsafe {
            panel::show_data_settings(
                self.hwnd,
                &self.settings,
                self.runtime.capabilities.as_ref(),
            )
        } {
            self.runtime.apply_network_error(error);
            self.open_data_bubble(true);
        }
    }

    fn handle_menu_action(&mut self, action: usize) {
        match action {
            ACTION_REFRESH => {
                self.open_data_bubble(true);
                self.refresh();
            }
            ACTION_SOUND_SET => {
                self.settings.sound_set = if self.settings.sound_set == "fx1" {
                    "duck".into()
                } else {
                    "fx1".into()
                };
                self.save();
            }
            ACTION_BUBBLE => {
                self.settings.bubble_enabled = !self.settings.bubble_enabled;
                if !self.settings.bubble_enabled {
                    self.close_bubble();
                }
                self.save();
            }
            ACTION_TOPMOST => {
                self.settings.always_on_top = !self.settings.always_on_top;
                self.apply_topmost();
                self.save();
            }
            ACTION_AUTOSTART => {
                let enabled = !self.settings.autostart;
                let result = if enabled {
                    settings::install_autostart()
                } else {
                    settings::remove_autostart()
                };
                match result {
                    Ok(()) => {
                        self.settings.autostart = enabled;
                        self.save();
                    }
                    Err(error) => {
                        self.runtime.apply_network_error(error);
                        self.open_data_bubble(true);
                    }
                }
            }
            ACTION_DETAILS => self.open_details(),
            ACTION_DATA_SETTINGS => self.open_data_settings(),
            ACTION_SETUP => self.open_setup(),
            ACTION_EXIT => unsafe {
                DestroyWindow(self.hwnd);
            },
            _ => {}
        }
    }

    fn handle_data_settings_result(&mut self, result: DataSettingsResult) {
        self.settings.model = result.model;
        self.settings.reasoning_effort = result.reasoning_effort;
        self.settings.card_overrides = result.card_overrides;
        self.save();
        self.open_data_bubble(true);
        self.render();
    }

    fn handle_setup_result(&mut self, result: SetupResult) {
        self.settings.endpoint = result.endpoint;
        match settings::protect_token(&mut self.settings, &result.token) {
            Ok(()) => {
                self.token = result.token;
                self.runtime.last_error = None;
                self.save();
                self.restart_network();
                self.open_data_bubble(true);
            }
            Err(error) => {
                self.runtime.apply_network_error(error);
                self.open_data_bubble(true);
            }
        }
    }

    fn update_window_region(&mut self) {
        let Some(graphics) = self.graphics.as_ref() else {
            return;
        };
        let mut client: RECT = unsafe { zeroed() };
        if unsafe { GetClientRect(self.hwnd, &mut client) } == 0 {
            return;
        }
        let width = (client.right - client.left).max(1);
        let height = (client.bottom - client.top).max(1);
        let flip_mode = if self.mirror_animation.is_some() {
            2
        } else {
            u8::from(self.flipped)
        };
        let signature = RegionSignature {
            width,
            height,
            flip_mode,
            bubble: self.runtime.bubble_open || self.bubble_animation.is_some(),
            menu_button: self.visual.menu_button_opacity > 0.01 || self.hover_animation.is_some(),
        };
        if self.region_signature == Some(signature) {
            return;
        }

        unsafe {
            let region = CreateRectRgn(0, 0, 0, 0);
            if region.is_null() {
                return;
            }
            let flips: &[bool] = match flip_mode {
                0 => &[false],
                1 => &[true],
                _ => &[false, true],
            };
            for flipped in flips {
                for (left, top, right, bottom) in graphics.whale_region_runs(*flipped) {
                    combine_rect_region(region, left, top, right, bottom);
                }
                if signature.bubble {
                    combine_design_ellipse(region, 68.0, 8.0, 840.0, 510.0, *flipped, width);
                    combine_design_ellipse(region, 304.0, 525.0, 400.0, 597.0, *flipped, width);
                    combine_design_ellipse(region, 405.0, 614.0, 479.0, 678.0, *flipped, width);
                }
                if signature.menu_button {
                    let button = crate::layout::menu_button_rect(*flipped, self.base_dip);
                    combine_design_rect(region, button, *flipped, width);
                }
            }
            if SetWindowRgn(self.hwnd, region, 1) == 0 {
                DeleteObject(region);
                return;
            }
        }
        self.region_signature = Some(signature);
    }

    fn apply_topmost(&self) {
        unsafe {
            SetWindowPos(
                self.hwnd,
                if self.settings.always_on_top {
                    HWND_TOPMOST
                } else {
                    HWND_NOTOPMOST
                },
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
    }

    fn set_hovered(&mut self, hovered: bool) {
        let target = if hovered { 1.0 } else { 0.0 };
        if (self.visual.menu_button_opacity - target).abs() < 0.001 {
            return;
        }
        self.hover_animation = Some(ScalarAnimation::new(
            self.visual.menu_button_opacity,
            target,
            Instant::now(),
            Duration::from_millis(150),
            Easing::Smooth,
        ));
        self.start_animation();
    }

    fn press(&mut self, down: bool) {
        let target = if down { 1.0 } else { 0.0 };
        self.press_animation = Some(ScalarAnimation::press(
            self.visual.squish_progress,
            target,
            Instant::now(),
        ));
        self.start_animation();
    }

    fn start_animation(&self) {
        unsafe { SetTimer(self.hwnd, TIMER_ANIMATION, self.animation_interval_ms, None) };
    }

    fn tick_animation(&mut self) {
        let now = Instant::now();
        let mut active = false;

        if let Some(animation) = self.bubble_animation {
            if animation.sample(now, &mut self.visual) {
                active = true;
            } else {
                self.bubble_animation = None;
                if self.visual.bubble_main <= 0.001 {
                    self.runtime.close_bubble();
                    self.gif_started = None;
                }
            }
        }

        if let Some(animation) = &mut self.content_swap {
            let (opacity, phase) = animation.sample(now);
            self.visual.content_opacity = opacity;
            match phase {
                ContentSwapPhase::Commit => {
                    self.runtime.commit_pending_content();
                    self.gif_started = matches!(
                        self.runtime.bubble_content,
                        BubbleContent::Random(RandomCard::RuaGif)
                    )
                    .then_some(now);
                    active = true;
                }
                ContentSwapPhase::Complete => {
                    self.content_swap = None;
                    self.visual.content_opacity = 1.0;
                }
                ContentSwapPhase::FadingOut | ContentSwapPhase::FadingIn => active = true,
            }
        }

        if let Some(animation) = self.press_animation {
            let (value, done) = animation.sample(now);
            self.visual.squish_progress = value;
            if done {
                self.press_animation = None;
            } else {
                active = true;
            }
        }

        if let Some(animation) = self.hover_animation {
            let (value, done) = animation.sample(now);
            self.visual.menu_button_opacity = value;
            if done {
                self.hover_animation = None;
            } else {
                active = true;
            }
        }

        if let Some(animation) = self.mirror_animation {
            let (value, done) = animation.sample(now);
            self.visual.mirror_progress = value;
            if done {
                self.mirror_animation = None;
            } else {
                active = true;
            }
        }

        if let Some(animation) = &self.number_roll {
            let (tokens, usd, done) = animation.sample(now);
            self.runtime.display_today_tokens = Some(tokens);
            self.runtime.display_today_usd_micros = usd;
            if done {
                self.number_roll = None;
            } else {
                active = true;
            }
        }

        if self.tick_snap(now) {
            active = true;
        }

        if let Some(started) = self.gif_started {
            if self.runtime.bubble_open
                && matches!(
                    self.runtime.bubble_content,
                    BubbleContent::Random(RandomCard::RuaGif)
                )
            {
                if let Some(graphics) = &self.graphics {
                    self.visual.gif_frame = graphics.gif_frame_for_elapsed(
                        now.saturating_duration_since(started).as_millis() as u64,
                    );
                }
                active = true;
            }
        }

        self.render();
        if !active {
            unsafe { KillTimer(self.hwnd, TIMER_ANIMATION) };
        }
    }

    fn tick_snap(&mut self, now: Instant) -> bool {
        let Some(animation) = &self.snap_animation else {
            return false;
        };
        let progress = now
            .saturating_duration_since(animation.started)
            .as_secs_f32()
            / animation.duration.as_secs_f32();
        let progress = progress.clamp(0.0, 1.0);
        let eased = 1.0 - (1.0 - progress).powi(3);
        let x = lerp(animation.from.x, animation.to.x, eased);
        let y = lerp(animation.from.y, animation.to.y, eased);
        unsafe {
            SetWindowPos(
                self.hwnd,
                if self.settings.always_on_top {
                    HWND_TOPMOST
                } else {
                    HWND_NOTOPMOST
                },
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        if progress >= 1.0 {
            self.snap_animation = None;
            self.save();
            false
        } else {
            true
        }
    }

    fn local_cursor(&self) -> Option<(i32, i32)> {
        let mut cursor = POINT { x: 0, y: 0 };
        unsafe { GetCursorPos(&mut cursor) };
        let rect = window_rect(self.hwnd).ok()?;
        Some((cursor.x - rect.left, cursor.y - rect.top))
    }

    fn whale_hit_local(&self, x: i32, y: i32) -> bool {
        self.graphics
            .as_ref()
            .is_none_or(|graphics| graphics.whale_hit(x, y, self.flipped))
    }

    fn bubble_hit_local(&self, x: i32, y: i32) -> bool {
        self.visual.bubble_main > 0.2
            && self
                .graphics
                .as_ref()
                .is_some_and(|graphics| graphics.bubble_hit(x, y, self.flipped))
    }

    fn menu_button_hit_local(&self, x: i32, y: i32) -> bool {
        self.visual.menu_button_opacity > 0.1
            && self
                .graphics
                .as_ref()
                .is_some_and(|graphics| graphics.menu_button_hit(x, y, self.flipped, self.base_dip))
    }

    fn resize_for_scale(&mut self, scale: f32) {
        self.settings.scale = crate::layout::clamp_scale(scale);
        let Ok(rect) = window_rect(self.hwnd) else {
            return;
        };
        let work = monitor_work_area(self.hwnd).unwrap_or(RECT {
            left: 0,
            top: 0,
            right: unsafe { GetSystemMetrics(SM_CXSCREEN) },
            bottom: unsafe { GetSystemMetrics(SM_CYSCREEN) },
        });
        let dpi = window_dpi(self.hwnd);
        let size = widget_base_px(
            self.settings.scale,
            work.right - work.left,
            work.bottom - work.top,
            dpi,
        )
        .max(1);
        let next = scaled_fixed_corner(
            RectF {
                left: rect.left as f32,
                top: rect.top as f32,
                right: rect.right as f32,
                bottom: rect.bottom as f32,
            },
            size as f32,
            size as f32,
            self.flipped,
        );
        unsafe {
            SetWindowPos(
                self.hwnd,
                if self.settings.always_on_top {
                    HWND_TOPMOST
                } else {
                    HWND_NOTOPMOST
                },
                next.left.round() as i32,
                next.top.round() as i32,
                size,
                size,
                SWP_NOACTIVATE,
            );
        }
        self.base_dip = size as f32 * 96.0 / dpi.max(1) as f32;
        if let Some(graphics) = &mut self.graphics {
            if let Err(error) = graphics.resize(size as u32) {
                self.runtime.apply_network_error(error);
            }
        }
        self.save();
        self.render();
    }

    fn snap(&mut self) {
        let Ok(rect) = window_rect(self.hwnd) else {
            return;
        };
        let work = monitor_work_area(self.hwnd).unwrap_or(RECT {
            left: 0,
            top: 0,
            right: unsafe { GetSystemMetrics(SM_CXSCREEN) },
            bottom: unsafe { GetSystemMetrics(SM_CYSCREEN) },
        });
        let width = rect.right - rect.left;
        let height = rect.bottom - rect.top;
        let local_left = (rect.left - work.left) as f32;
        let local_top = (rect.top - work.top) as f32;
        let anchors = crate::layout::snap_anchors(
            local_left,
            local_top,
            width as f32,
            height as f32,
            (work.right - work.left) as f32,
            (work.bottom - work.top) as f32,
        );
        let mut target = POINT {
            x: rect.left,
            y: rect.top,
        };
        match anchors.horizontal {
            crate::layout::HorizontalAnchor::Left => target.x = work.left,
            crate::layout::HorizontalAnchor::Right => target.x = work.right - width,
            crate::layout::HorizontalAnchor::Free => {}
        }
        match anchors.vertical {
            crate::layout::VerticalAnchor::Top => target.y = work.top,
            crate::layout::VerticalAnchor::Bottom => target.y = work.bottom - height,
            crate::layout::VerticalAnchor::Free => {}
        }
        let target_flipped = matches!(anchors.horizontal, crate::layout::HorizontalAnchor::Left);
        if target_flipped != self.flipped {
            self.mirror_animation = Some(ScalarAnimation::new(
                self.visual.mirror_progress,
                if target_flipped { 1.0 } else { 0.0 },
                Instant::now(),
                Duration::from_millis(300),
                Easing::Smooth,
            ));
            self.flipped = target_flipped;
        }
        self.snap_animation = Some(SnapAnimation {
            from: POINT {
                x: rect.left,
                y: rect.top,
            },
            to: target,
            started: Instant::now(),
            duration: Duration::from_millis(180),
        });
        self.start_animation();
    }

    fn play_press(&self) {
        if self.settings.volume <= 0.0 {
            return;
        }
        let sound = if self.settings.sound_set == "fx1" {
            assets::FX_PRESS_WAV
        } else {
            assets::DUCK_PRESS_WAV
        };
        unsafe {
            PlaySoundW(
                sound.as_ptr() as *const u16,
                ptr::null_mut(),
                SND_ASYNC | SND_MEMORY | SND_NODEFAULT,
            );
        }
    }

    fn play_release(&self) {
        if self.settings.volume <= 0.0 {
            return;
        }
        let sound = if self.settings.sound_set == "fx1" {
            assets::FX_RELEASE_WAV
        } else {
            assets::DUCK_RELEASE_WAV
        };
        unsafe {
            PlaySoundW(
                sound.as_ptr() as *const u16,
                ptr::null_mut(),
                SND_ASYNC | SND_MEMORY | SND_NODEFAULT,
            );
        }
    }

    fn add_tray(&mut self) {
        let instance = unsafe { GetModuleHandleW(ptr::null()) };
        let mut data: NOTIFYICONDATAW = unsafe { zeroed() };
        data.cbSize = size_of::<NOTIFYICONDATAW>() as u32;
        data.hWnd = self.hwnd;
        data.uID = 1;
        data.uFlags = NIF_MESSAGE | NIF_ICON | NIF_TIP;
        data.uCallbackMessage = WM_TRAY;
        data.hIcon = unsafe { assets::load_app_icon(instance) };
        copy_wide(&mut data.szTip, "CPA Whale · CLIProxyAPI 今日");
        unsafe { Shell_NotifyIconW(NIM_ADD, &data) };
        self.tray = data;
    }

    fn remove_tray(&self) {
        unsafe { Shell_NotifyIconW(NIM_DELETE, &self.tray) };
    }
}

pub fn run() -> Result<(), String> {
    let _timer_resolution = TimerResolution::one_millisecond();
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(None, COINIT_APARTMENTTHREADED)
            .ok()
            .map_err(|error| format!("CoInitializeEx: {error}"))?;
    }
    let loaded = settings::load()?;
    let app = Box::new(App::new(loaded));
    let app_ptr = Box::into_raw(app);
    unsafe {
        let instance = GetModuleHandleW(ptr::null());
        if instance.is_null() {
            drop(Box::from_raw(app_ptr));
            return Err(last_error("GetModuleHandleW"));
        }
        let class_name = wide(CLASS_NAME);
        let window_class = WNDCLASSEXW {
            cbSize: size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW | CS_DBLCLKS,
            lpfnWndProc: Some(window_proc),
            cbClsExtra: 0,
            cbWndExtra: 0,
            hInstance: instance,
            hIcon: assets::load_app_icon(instance),
            hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
            hbrBackground: ptr::null_mut(),
            lpszMenuName: ptr::null(),
            lpszClassName: class_name.as_ptr(),
            hIconSm: assets::load_app_icon(instance),
        };
        if RegisterClassExW(&window_class) == 0 {
            drop(Box::from_raw(app_ptr));
            return Err(last_error("RegisterClassExW"));
        }
        let initial_size = 375;
        let x = (*app_ptr)
            .settings
            .x
            .unwrap_or_else(|| GetSystemMetrics(SM_CXSCREEN) - initial_size);
        let y = (*app_ptr)
            .settings
            .y
            .unwrap_or_else(|| GetSystemMetrics(SM_CYSCREEN) - initial_size);
        let ex_style = WS_EX_TOOLWINDOW
            | WS_EX_NOACTIVATE
            | WS_EX_NOREDIRECTIONBITMAP
            | if (*app_ptr).settings.always_on_top {
                WS_EX_TOPMOST
            } else {
                0
            };
        let hwnd = CreateWindowExW(
            ex_style,
            class_name.as_ptr(),
            wide("CPA Whale").as_ptr(),
            WS_POPUP,
            x,
            y,
            initial_size,
            initial_size,
            ptr::null_mut(),
            ptr::null_mut(),
            instance,
            app_ptr.cast(),
        );
        if hwnd.is_null() {
            drop(Box::from_raw(app_ptr));
            return Err(last_error("CreateWindowExW"));
        }
        (*app_ptr).hwnd = hwnd;
        (*app_ptr).animation_interval_ms = query_animation_interval_ms(hwnd);
        (*app_ptr).initialize_graphics(initial_size as u32)?;
        (*app_ptr).add_tray();
        (*app_ptr).resize_for_scale((*app_ptr).settings.scale);
        (*app_ptr).start_network();
        (*app_ptr).render();
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        if (&*app_ptr).token.is_empty() || (&*app_ptr).settings.endpoint.is_empty() {
            (*app_ptr).open_setup();
        }
        let mut message: MSG = zeroed();
        while GetMessageW(&mut message, ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    Ok(())
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_NCCREATE {
        let create = &*(lparam as *const CREATESTRUCTW);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, create.lpCreateParams as isize);
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let app_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
    if app_ptr.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let app = &mut *app_ptr;
    match message {
        WM_NCHITTEST => {
            let screen_x = low_i16(lparam) as i32;
            let screen_y = high_i16(lparam) as i32;
            let rect = window_rect(hwnd).unwrap_or_else(|_| zeroed());
            let x = screen_x - rect.left;
            let y = screen_y - rect.top;
            if app.menu_button_hit_local(x, y)
                || app.bubble_hit_local(x, y)
                || app.whale_hit_local(x, y)
            {
                HTCLIENT as LRESULT
            } else {
                HTTRANSPARENT as LRESULT
            }
        }
        WM_LBUTTONDOWN => {
            if let Some((x, y)) = app.local_cursor() {
                if app.menu_button_hit_local(x, y) {
                    app.menu_button_click = true;
                    SetCapture(hwnd);
                } else if app.bubble_hit_local(x, y) {
                    app.bubble_click = true;
                    SetCapture(hwnd);
                } else if app.whale_hit_local(x, y) {
                    let mut cursor = POINT { x: 0, y: 0 };
                    GetCursorPos(&mut cursor);
                    let rect = window_rect(hwnd).unwrap_or_else(|_| zeroed());
                    app.drag = Some(DragState {
                        start_cursor: cursor,
                        start_rect: rect,
                        moved: false,
                    });
                    app.press(true);
                    app.play_press();
                    SetCapture(hwnd);
                }
            }
            0
        }
        WM_MOUSEMOVE => {
            let mut tracking = TRACKMOUSEEVENT {
                cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                dwFlags: TME_LEAVE,
                hwndTrack: hwnd,
                dwHoverTime: 0,
            };
            TrackMouseEvent(&mut tracking);
            if let Some(drag) = &mut app.drag {
                let mut cursor = POINT { x: 0, y: 0 };
                GetCursorPos(&mut cursor);
                let dx = cursor.x - drag.start_cursor.x;
                let dy = cursor.y - drag.start_cursor.y;
                if dx * dx + dy * dy >= 9 {
                    drag.moved = true;
                }
                SetWindowPos(
                    hwnd,
                    if app.settings.always_on_top {
                        HWND_TOPMOST
                    } else {
                        HWND_NOTOPMOST
                    },
                    drag.start_rect.left + dx,
                    drag.start_rect.top + dy,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOACTIVATE,
                );
            } else if let Some((x, y)) = app.local_cursor() {
                let hovered = app.whale_hit_local(x, y) || app.menu_button_hit_local(x, y);
                app.set_hovered(hovered);
                SetCursor(LoadCursorW(
                    ptr::null_mut(),
                    if app.whale_hit_local(x, y) {
                        IDC_HAND
                    } else {
                        IDC_ARROW
                    },
                ));
            }
            0
        }
        WM_MOUSELEAVE_MESSAGE => {
            if app.drag.is_none() {
                app.set_hovered(false);
            }
            0
        }
        WM_LBUTTONUP => {
            if app.menu_button_click {
                app.menu_button_click = false;
                ReleaseCapture();
                app.open_menu();
            } else if app.bubble_click {
                app.bubble_click = false;
                ReleaseCapture();
                app.on_bubble_click();
            } else if let Some(drag) = app.drag.take() {
                ReleaseCapture();
                app.press(false);
                app.play_release();
                if drag.moved {
                    app.snap();
                } else {
                    app.on_whale_click();
                }
            }
            0
        }
        WM_LBUTTONDBLCLK => 0,
        WM_RBUTTONUP => {
            app.open_menu();
            0
        }
        WM_MOUSEWHEEL => {
            let delta = high_i16(wparam as isize) as f32 / WHEEL_DELTA as f32;
            app.resize_for_scale(app.settings.scale + delta * 0.05);
            0
        }
        WM_TIMER if wparam == TIMER_BUBBLE => {
            app.close_bubble();
            0
        }
        WM_TIMER if wparam == TIMER_ANIMATION => {
            app.tick_animation();
            0
        }
        WM_DPICHANGED => {
            let suggested = &*(lparam as *const RECT);
            SetWindowPos(
                hwnd,
                if app.settings.always_on_top {
                    HWND_TOPMOST
                } else {
                    HWND_NOTOPMOST
                },
                suggested.left,
                suggested.top,
                suggested.right - suggested.left,
                suggested.bottom - suggested.top,
                SWP_NOACTIVATE,
            );
            app.resize_for_scale(app.settings.scale);
            0
        }
        WM_DISPLAYCHANGE | WM_SETTINGCHANGE => {
            app.animation_interval_ms = query_animation_interval_ms(hwnd);
            app.resize_for_scale(app.settings.scale);
            app.snap();
            0
        }
        WM_ERASEBKGND => 1,
        WM_PAINT => {
            ValidateRect(hwnd, ptr::null());
            0
        }
        WM_CAPABILITIES => {
            let capabilities = Box::from_raw(lparam as *mut CapabilitiesResponse);
            app.runtime
                .apply_capabilities(&mut app.settings, *capabilities);
            app.save();
            app.render();
            0
        }
        WM_SNAPSHOT => {
            let snapshot = Box::from_raw(lparam as *mut GlobalSnapshot);
            app.on_snapshot(*snapshot);
            0
        }
        WM_NETWORK_ERROR => {
            let error = Box::from_raw(lparam as *mut String);
            app.runtime.apply_network_error(*error);
            app.render();
            0
        }
        WM_CONFIG_SAVED => {
            let result = Box::from_raw(lparam as *mut SetupResult);
            app.handle_setup_result(*result);
            0
        }
        WM_DATA_SETTINGS_SAVED => {
            let result = Box::from_raw(lparam as *mut DataSettingsResult);
            app.handle_data_settings_result(*result);
            0
        }
        WM_MENU_ACTION => {
            app.handle_menu_action(wparam);
            0
        }
        WM_MENU_SCALE => {
            let scale = f32::from_bits(lparam as u32);
            app.resize_for_scale(scale);
            0
        }
        WM_MENU_VOLUME => {
            app.settings.volume = f32::from_bits(lparam as u32).clamp(0.0, 1.0);
            app.save();
            0
        }
        WM_MENU_CLOSED => {
            app.menu_hwnd = ptr::null_mut();
            app.set_hovered(false);
            0
        }
        WM_TRAY => {
            let event = lparam as u32;
            if event == WM_RBUTTONUP {
                app.open_menu();
            } else if event == WM_LBUTTONDBLCLK {
                app.open_details();
            } else if event == WM_LBUTTONUP {
                app.open_data_bubble(true);
            }
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_DESTROY => {
            app.remove_tray();
            if let Some(mut network) = app.network.take() {
                network.shutdown();
            }
            PostQuitMessage(0);
            0
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(app_ptr));
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

fn query_animation_interval_ms(hwnd: HWND) -> u32 {
    let mut timing = DWM_TIMING_INFO {
        cbSize: size_of::<DWM_TIMING_INFO>() as u32,
        ..DWM_TIMING_INFO::default()
    };
    let result =
        unsafe { DwmGetCompositionTimingInfo(windows::Win32::Foundation::HWND(hwnd), &mut timing) };
    if result.is_err() {
        return DEFAULT_ANIMATION_INTERVAL_MS;
    }
    let numerator = unsafe { std::ptr::addr_of!(timing.rateRefresh.uiNumerator).read_unaligned() };
    let denominator =
        unsafe { std::ptr::addr_of!(timing.rateRefresh.uiDenominator).read_unaligned() };
    if numerator == 0 || denominator == 0 {
        return DEFAULT_ANIMATION_INTERVAL_MS;
    }
    let refresh_hz = numerator as f64 / denominator as f64;
    (1000.0 / refresh_hz).floor().clamp(1.0, 16.0) as u32
}

fn window_dpi(hwnd: HWND) -> u32 {
    unsafe { GetDpiForWindow(windows::Win32::Foundation::HWND(hwnd)) }.max(96)
}

fn window_rect(hwnd: HWND) -> Result<RECT, String> {
    let mut rect: RECT = unsafe { zeroed() };
    if unsafe { GetWindowRect(hwnd, &mut rect) } == 0 {
        Err(last_error("GetWindowRect"))
    } else {
        Ok(rect)
    }
}

fn monitor_work_area(hwnd: HWND) -> Option<RECT> {
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if monitor.is_null() {
            return None;
        }
        let mut info: MONITORINFO = zeroed();
        info.cbSize = size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info) == 0 {
            None
        } else {
            Some(info.rcWork)
        }
    }
}

unsafe fn combine_rect_region(
    region: windows_sys::Win32::Graphics::Gdi::HRGN,
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
) {
    if right <= left || bottom <= top {
        return;
    }
    let part = CreateRectRgn(left, top, right, bottom);
    if !part.is_null() {
        CombineRgn(region, region, part, RGN_OR);
        DeleteObject(part);
    }
}

unsafe fn combine_design_rect(
    region: windows_sys::Win32::Graphics::Gdi::HRGN,
    rect: RectF,
    flipped: bool,
    size: i32,
) {
    let scale = size as f32 / DESIGN_SIZE;
    let (left, right) = if flipped {
        (DESIGN_SIZE - rect.right, DESIGN_SIZE - rect.left)
    } else {
        (rect.left, rect.right)
    };
    combine_rect_region(
        region,
        (left * scale).floor() as i32,
        (rect.top * scale).floor() as i32,
        (right * scale).ceil() as i32,
        (rect.bottom * scale).ceil() as i32,
    );
}

unsafe fn combine_design_ellipse(
    region: windows_sys::Win32::Graphics::Gdi::HRGN,
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
    flipped: bool,
    size: i32,
) {
    let scale = size as f32 / DESIGN_SIZE;
    let (left, right) = if flipped {
        (DESIGN_SIZE - right, DESIGN_SIZE - left)
    } else {
        (left, right)
    };
    let part = CreateEllipticRgn(
        (left * scale).floor() as i32,
        (top * scale).floor() as i32,
        (right * scale).ceil() as i32,
        (bottom * scale).ceil() as i32,
    );
    if !part.is_null() {
        CombineRgn(region, region, part, RGN_OR);
        DeleteObject(part);
    }
}

fn lerp(from: i32, to: i32, progress: f32) -> i32 {
    (from as f32 + (to - from) as f32 * progress).round() as i32
}

fn lerp_i64(from: i64, to: i64, progress: f64) -> i64 {
    (from as f64 + (to - from) as f64 * progress).round() as i64
}

fn copy_wide<const N: usize>(target: &mut [u16; N], value: &str) {
    let encoded = value.encode_utf16().take(N.saturating_sub(1));
    for (slot, character) in target.iter_mut().zip(encoded) {
        *slot = character;
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn low_i16(value: isize) -> i16 {
    (value & 0xffff) as u16 as i16
}

fn high_i16(value: isize) -> i16 {
    ((value >> 16) & 0xffff) as u16 as i16
}

fn last_error(operation: &str) -> String {
    format!("{operation} failed with Windows error {}", unsafe {
        GetLastError()
    })
}
