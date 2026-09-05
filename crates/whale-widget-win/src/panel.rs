use std::mem::{size_of, zeroed};
use std::ptr;

use whale_protocol::{CapabilitiesResponse, GlobalSnapshot, UsageDelta};
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MonitorFromWindow, ValidateRect, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture, VK_ESCAPE};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::graphics::{DataSettingsPanelData, DetailsPanelData, MenuPanelData, PanelRenderer};
use crate::model::{
    account_primary_remaining_percent, card_enabled, estimated_today_usd_micros, format_tokens,
    format_usd, model_detail_row, model_display_name, ClientSettings,
};

pub const WM_MENU_ACTION: u32 = WM_APP + 30;
pub const WM_MENU_SCALE: u32 = WM_APP + 31;
pub const WM_MENU_VOLUME: u32 = WM_APP + 32;
pub const WM_MENU_CLOSED: u32 = WM_APP + 33;
pub const WM_DATA_SETTINGS_SAVED: u32 = WM_APP + 34;

pub const ACTION_REFRESH: usize = 1;
pub const ACTION_SOUND_SET: usize = 2;
pub const ACTION_BUBBLE: usize = 3;
pub const ACTION_TOPMOST: usize = 4;
pub const ACTION_AUTOSTART: usize = 6;
pub const ACTION_DETAILS: usize = 7;
pub const ACTION_SETUP: usize = 8;
pub const ACTION_EXIT: usize = 9;
pub const ACTION_DATA_SETTINGS: usize = 10;

const MENU_CLASS: &str = "CPAWhaleMenuPanelV2";
const DETAILS_CLASS: &str = "CPAWhaleDetailsPanelV2";
const DATA_SETTINGS_CLASS: &str = "CPAWhaleDataSettingsPanelV1";
const MENU_WIDTH: f32 = 320.0;
const MENU_HEIGHT: f32 = 546.0;
const DETAILS_WIDTH: f32 = 620.0;
const DETAILS_HEIGHT: f32 = 520.0;
const DATA_SETTINGS_WIDTH: f32 = 320.0;
const DATA_SETTINGS_HEIGHT: f32 = 500.0;

const DATA_CARDS: [(&str, &str); 7] = [
    ("startup", "挂件启动后"),
    ("models", "模型分布"),
    ("quota", "账户额度"),
    ("intelligence", "模型智力"),
    ("reset", "重置参考"),
    ("service-status", "服务状态"),
    ("entertainment", "娱乐内容"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SliderDrag {
    Scale,
    Volume,
}

struct MenuState {
    parent: HWND,
    settings: ClientSettings,
    hardware_accelerated: bool,
    renderer: Option<PanelRenderer>,
    dpi: u32,
    slider_drag: Option<SliderDrag>,
}

pub struct DataSettingsResult {
    pub model: String,
    pub reasoning_effort: String,
    pub card_overrides: std::collections::BTreeMap<String, bool>,
}

struct DataSettingsState {
    parent: HWND,
    settings: ClientSettings,
    capabilities: Option<CapabilitiesResponse>,
    renderer: Option<PanelRenderer>,
    dpi: u32,
}

struct DetailsState {
    renderer: Option<PanelRenderer>,
    dpi: u32,
    today_tokens: String,
    today_usd: String,
    startup_tokens: String,
    capabilities: Option<CapabilitiesResponse>,
    snapshot: Option<GlobalSnapshot>,
    page: usize,
    hardware_accelerated: bool,
}

/// Opens the self-drawn whale menu beside the widget.
///
/// # Safety
/// `parent` must be a live widget window owned by this process.
pub unsafe fn show_menu(
    parent: HWND,
    settings: &ClientSettings,
    flipped: bool,
    hardware_accelerated: bool,
) -> Result<HWND, String> {
    let instance = GetModuleHandleW(ptr::null());
    if instance.is_null() {
        return Err("GetModuleHandleW failed".into());
    }
    register_class(MENU_CLASS, Some(menu_proc), instance);
    let dpi = GetDpiForWindow(windows::Win32::Foundation::HWND(parent)).max(96);
    let width = dip(MENU_WIDTH, dpi);
    let height = dip(MENU_HEIGHT, dpi);
    let parent_rect = window_rect(parent).ok_or_else(|| "读取鲸鱼位置失败".to_string())?;
    let work = monitor_work_area(parent).unwrap_or(RECT {
        left: 0,
        top: 0,
        right: GetSystemMetrics(SM_CXSCREEN),
        bottom: GetSystemMetrics(SM_CYSCREEN),
    });
    let mut x = if flipped {
        parent_rect.left
    } else {
        parent_rect.right - width
    };
    let mut y = parent_rect.top - height + dip(150.0, dpi);
    x = x.clamp(
        work.left + dip(4.0, dpi),
        work.right - width - dip(4.0, dpi),
    );
    y = y.clamp(
        work.top + dip(4.0, dpi),
        work.bottom - height - dip(4.0, dpi),
    );
    let state = Box::new(MenuState {
        parent,
        settings: settings.clone(),
        hardware_accelerated,
        renderer: None,
        dpi,
        slider_drag: None,
    });
    let state_ptr = Box::into_raw(state);
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP,
        wide(MENU_CLASS).as_ptr(),
        wide("CPA Whale 菜单").as_ptr(),
        WS_POPUP | WS_VISIBLE,
        x,
        y,
        width,
        height,
        parent,
        ptr::null_mut(),
        instance,
        state_ptr.cast(),
    );
    if hwnd.is_null() {
        drop(Box::from_raw(state_ptr));
        return Err("创建小鲸鱼菜单失败".into());
    }
    SetForegroundWindow(hwnd);
    Ok(hwnd)
}

/// Opens the self-drawn CPA details panel.
///
/// # Safety
/// `parent` must be a live widget window owned by this process.
pub unsafe fn show_details(
    parent: HWND,
    capabilities: Option<&CapabilitiesResponse>,
    snapshot: Option<&GlobalSnapshot>,
    startup: Option<&UsageDelta>,
    hardware_accelerated: bool,
) -> Result<HWND, String> {
    let instance = GetModuleHandleW(ptr::null());
    if instance.is_null() {
        return Err("GetModuleHandleW failed".into());
    }
    register_class(DETAILS_CLASS, Some(details_proc), instance);
    let dpi = GetDpiForWindow(windows::Win32::Foundation::HWND(parent)).max(96);
    let width = dip(DETAILS_WIDTH, dpi);
    let height = dip(DETAILS_HEIGHT, dpi);
    let parent_rect = window_rect(parent).unwrap_or(RECT {
        left: 0,
        top: 0,
        right: GetSystemMetrics(SM_CXSCREEN),
        bottom: GetSystemMetrics(SM_CYSCREEN),
    });
    let work = monitor_work_area(parent).unwrap_or(RECT {
        left: 0,
        top: 0,
        right: GetSystemMetrics(SM_CXSCREEN),
        bottom: GetSystemMetrics(SM_CYSCREEN),
    });
    let x = ((parent_rect.left + parent_rect.right - width) / 2).clamp(
        work.left + dip(8.0, dpi),
        work.right - width - dip(8.0, dpi),
    );
    let y = ((parent_rect.top + parent_rect.bottom - height) / 2).clamp(
        work.top + dip(8.0, dpi),
        work.bottom - height - dip(8.0, dpi),
    );
    let today_tokens = snapshot
        .map(|value| format_tokens(value.today.tokens.total_tokens))
        .unwrap_or_else(|| "--".into());
    let today_usd = snapshot
        .map(|value| format_usd(estimated_today_usd_micros(value)))
        .unwrap_or_else(|| "--".into());
    let startup_tokens = startup
        .filter(|delta| delta.compatible)
        .map(|delta| format!("+{}", format_tokens(delta.totals.tokens.total_tokens)))
        .unwrap_or_else(|| "--".into());
    let state = Box::new(DetailsState {
        renderer: None,
        dpi,
        today_tokens,
        today_usd,
        startup_tokens,
        capabilities: capabilities.cloned(),
        snapshot: snapshot.cloned(),
        page: 0,
        hardware_accelerated,
    });
    let state_ptr = Box::into_raw(state);
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP,
        wide(DETAILS_CLASS).as_ptr(),
        wide("CPA Whale 详细信息").as_ptr(),
        WS_POPUP | WS_VISIBLE,
        x,
        y,
        width,
        height,
        parent,
        ptr::null_mut(),
        instance,
        state_ptr.cast(),
    );
    if hwnd.is_null() {
        drop(Box::from_raw(state_ptr));
        return Err("创建详细信息面板失败".into());
    }
    SetForegroundWindow(hwnd);
    Ok(hwnd)
}

/// Opens the self-drawn data preferences panel.
///
/// # Safety
/// `parent` must be a live widget window owned by this process.
pub unsafe fn show_data_settings(
    parent: HWND,
    settings: &ClientSettings,
    capabilities: Option<&CapabilitiesResponse>,
) -> Result<HWND, String> {
    let instance = GetModuleHandleW(ptr::null());
    if instance.is_null() {
        return Err("GetModuleHandleW failed".into());
    }
    register_class(DATA_SETTINGS_CLASS, Some(data_settings_proc), instance);
    let dpi = GetDpiForWindow(windows::Win32::Foundation::HWND(parent)).max(96);
    let width = dip(DATA_SETTINGS_WIDTH, dpi);
    let height = dip(DATA_SETTINGS_HEIGHT, dpi);
    let parent_rect = window_rect(parent).unwrap_or(RECT {
        left: 0,
        top: 0,
        right: GetSystemMetrics(SM_CXSCREEN),
        bottom: GetSystemMetrics(SM_CYSCREEN),
    });
    let work = monitor_work_area(parent).unwrap_or(RECT {
        left: 0,
        top: 0,
        right: GetSystemMetrics(SM_CXSCREEN),
        bottom: GetSystemMetrics(SM_CYSCREEN),
    });
    let x = ((parent_rect.left + parent_rect.right - width) / 2).clamp(
        work.left + dip(8.0, dpi),
        work.right - width - dip(8.0, dpi),
    );
    let y = ((parent_rect.top + parent_rect.bottom - height) / 2).clamp(
        work.top + dip(8.0, dpi),
        work.bottom - height - dip(8.0, dpi),
    );
    let state = Box::new(DataSettingsState {
        parent,
        settings: settings.clone(),
        capabilities: capabilities.cloned(),
        renderer: None,
        dpi,
    });
    let state_ptr = Box::into_raw(state);
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP,
        wide(DATA_SETTINGS_CLASS).as_ptr(),
        wide("CPA Whale 数据设置").as_ptr(),
        WS_POPUP | WS_VISIBLE,
        x,
        y,
        width,
        height,
        parent,
        ptr::null_mut(),
        instance,
        state_ptr.cast(),
    );
    if hwnd.is_null() {
        drop(Box::from_raw(state_ptr));
        return Err("创建数据设置面板失败".into());
    }
    SetForegroundWindow(hwnd);
    Ok(hwnd)
}

unsafe fn register_class(name: &str, proc: WNDPROC, instance: *mut std::ffi::c_void) {
    let class = wide(name);
    let descriptor = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: proc,
        cbClsExtra: 0,
        cbWndExtra: 0,
        hInstance: instance,
        hIcon: crate::assets::load_app_icon(instance),
        hCursor: LoadCursorW(ptr::null_mut(), IDC_ARROW),
        hbrBackground: ptr::null_mut(),
        lpszMenuName: ptr::null(),
        lpszClassName: class.as_ptr(),
        hIconSm: crate::assets::load_app_icon(instance),
    };
    RegisterClassExW(&descriptor);
}

unsafe extern "system" fn menu_proc(
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
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut MenuState;
    if state_ptr.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let state = &mut *state_ptr;
    match message {
        WM_CREATE => {
            let mut rect: RECT = zeroed();
            GetClientRect(hwnd, &mut rect);
            if let Ok(renderer) = PanelRenderer::new(
                hwnd,
                (rect.right - rect.left).max(1) as u32,
                (rect.bottom - rect.top).max(1) as u32,
                state.dpi,
            ) {
                state.renderer = Some(renderer);
            }
            render_menu(state);
            0
        }
        WM_LBUTTONDOWN => {
            let (x, y) = design_point(lparam, state.dpi);
            if (78.0..=112.0).contains(&y) {
                state.slider_drag = Some(SliderDrag::Scale);
                SetCapture(hwnd);
                update_slider(state, x);
            } else if (152.0..=188.0).contains(&y) {
                state.slider_drag = Some(SliderDrag::Volume);
                SetCapture(hwnd);
                update_slider(state, x);
            }
            0
        }
        WM_MOUSEMOVE => {
            if state.slider_drag.is_some() {
                let (x, _) = design_point(lparam, state.dpi);
                update_slider(state, x);
            }
            0
        }
        WM_LBUTTONUP => {
            if state.slider_drag.take().is_some() {
                ReleaseCapture();
                return 0;
            }
            let (_, y) = design_point(lparam, state.dpi);
            let action = if (116.0..=152.0).contains(&y) {
                state.settings.sound_set = if state.settings.sound_set == "fx1" {
                    "duck".into()
                } else {
                    "fx1".into()
                };
                Some(ACTION_SOUND_SET)
            } else if (222.0..=258.0).contains(&y) {
                state.settings.bubble_enabled = !state.settings.bubble_enabled;
                Some(ACTION_BUBBLE)
            } else if (258.0..=294.0).contains(&y) {
                state.settings.always_on_top = !state.settings.always_on_top;
                Some(ACTION_TOPMOST)
            } else if (294.0..=330.0).contains(&y) {
                state.settings.autostart = !state.settings.autostart;
                Some(ACTION_AUTOSTART)
            } else if (342.0..=378.0).contains(&y) {
                Some(ACTION_REFRESH)
            } else if (378.0..=412.0).contains(&y) {
                Some(ACTION_DETAILS)
            } else if (412.0..=446.0).contains(&y) {
                Some(ACTION_DATA_SETTINGS)
            } else if (446.0..=480.0).contains(&y) {
                Some(ACTION_SETUP)
            } else if y >= 480.0 {
                Some(ACTION_EXIT)
            } else {
                None
            };
            if let Some(action) = action {
                PostMessageW(state.parent, WM_MENU_ACTION, action, 0);
                render_menu(state);
                if matches!(
                    action,
                    ACTION_REFRESH
                        | ACTION_DETAILS
                        | ACTION_DATA_SETTINGS
                        | ACTION_SETUP
                        | ACTION_EXIT
                ) {
                    DestroyWindow(hwnd);
                }
            }
            0
        }
        WM_RBUTTONUP => {
            PostMessageW(state.parent, WM_MENU_ACTION, ACTION_REFRESH, 0);
            0
        }
        WM_KEYDOWN if wparam == VK_ESCAPE as usize => {
            DestroyWindow(hwnd);
            0
        }
        WM_ACTIVATE if (wparam & 0xffff) == WA_INACTIVE as usize => {
            if state.slider_drag.is_none() {
                DestroyWindow(hwnd);
            }
            0
        }
        WM_PAINT => {
            ValidateRect(hwnd, ptr::null());
            render_menu(state);
            0
        }
        WM_NCDESTROY => {
            PostMessageW(state.parent, WM_MENU_CLOSED, 0, 0);
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(state_ptr));
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe extern "system" fn data_settings_proc(
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
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DataSettingsState;
    if state_ptr.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let state = &mut *state_ptr;
    match message {
        WM_CREATE => {
            let mut rect: RECT = zeroed();
            GetClientRect(hwnd, &mut rect);
            if let Ok(renderer) = PanelRenderer::new(
                hwnd,
                (rect.right - rect.left).max(1) as u32,
                (rect.bottom - rect.top).max(1) as u32,
                state.dpi,
            ) {
                state.renderer = Some(renderer);
            }
            render_data_settings(state);
            0
        }
        WM_NCHITTEST => {
            let screen_x = low_i16(lparam) as i32;
            let screen_y = high_i16(lparam) as i32;
            let rect = window_rect(hwnd).unwrap_or_else(|| zeroed());
            let local_x = (screen_x - rect.left) as f32 * 96.0 / state.dpi.max(1) as f32;
            let local_y = (screen_y - rect.top) as f32 * 96.0 / state.dpi.max(1) as f32;
            if local_x >= 270.0 && local_y <= 58.0 {
                HTCLIENT as LRESULT
            } else if local_y < 58.0 {
                HTCAPTION as LRESULT
            } else {
                HTCLIENT as LRESULT
            }
        }
        WM_LBUTTONUP => {
            let (x, y) = design_point(lparam, state.dpi);
            if x >= 274.0 && y <= 56.0 {
                DestroyWindow(hwnd);
            } else if (58.0..=100.0).contains(&y) {
                cycle_model(state);
                render_data_settings(state);
            } else if (100.0..=146.0).contains(&y) {
                cycle_reasoning_effort(state);
                render_data_settings(state);
            } else if let Some((card, _)) = DATA_CARDS
                .iter()
                .enumerate()
                .find(|(index, _)| {
                    let top = 158.0 + *index as f32 * 38.0;
                    (top..=top + 34.0).contains(&y)
                })
                .map(|(index, item)| (item.0, index))
            {
                let enabled = card_enabled(&state.settings, state.capabilities.as_ref(), card);
                state
                    .settings
                    .card_overrides
                    .insert(card.to_string(), !enabled);
                render_data_settings(state);
            } else if y >= 438.0 {
                save_data_settings(hwnd, state);
            }
            0
        }
        WM_KEYDOWN if wparam == VK_ESCAPE as usize => {
            DestroyWindow(hwnd);
            0
        }
        WM_PAINT => {
            ValidateRect(hwnd, ptr::null());
            render_data_settings(state);
            0
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(state_ptr));
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe extern "system" fn details_proc(
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
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut DetailsState;
    if state_ptr.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let state = &mut *state_ptr;
    match message {
        WM_CREATE => {
            let mut rect: RECT = zeroed();
            GetClientRect(hwnd, &mut rect);
            if let Ok(renderer) = PanelRenderer::new(
                hwnd,
                (rect.right - rect.left).max(1) as u32,
                (rect.bottom - rect.top).max(1) as u32,
                state.dpi,
            ) {
                state.renderer = Some(renderer);
            }
            render_details(state);
            0
        }
        WM_NCHITTEST => {
            let screen_x = low_i16(lparam) as i32;
            let screen_y = high_i16(lparam) as i32;
            let rect = window_rect(hwnd).unwrap_or_else(|| zeroed());
            let local_x = (screen_x - rect.left) as f32 * 96.0 / state.dpi.max(1) as f32;
            let local_y = (screen_y - rect.top) as f32 * 96.0 / state.dpi.max(1) as f32;
            if local_x >= 552.0 && local_y <= 62.0 {
                HTCLIENT as LRESULT
            } else if local_y < 78.0 {
                HTCAPTION as LRESULT
            } else {
                HTCLIENT as LRESULT
            }
        }
        WM_LBUTTONUP => {
            let (x, y) = design_point(lparam, state.dpi);
            if x >= 560.0 && y <= 58.0 {
                DestroyWindow(hwnd);
            } else if (82.0..=116.0).contains(&y) && (30.0..=598.0).contains(&x) {
                state.page = (((x - 30.0) / 142.0).floor() as usize).min(3);
                render_details(state);
            }
            0
        }
        WM_KEYDOWN if wparam == VK_ESCAPE as usize => {
            DestroyWindow(hwnd);
            0
        }
        WM_PAINT => {
            ValidateRect(hwnd, ptr::null());
            render_details(state);
            0
        }
        WM_NCDESTROY => {
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(state_ptr));
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe fn update_slider(state: &mut MenuState, x: f32) {
    let progress = ((x - 104.0) / 136.0).clamp(0.0, 1.0);
    match state.slider_drag {
        Some(SliderDrag::Scale) => {
            state.settings.scale = 0.6 + progress * 1.9;
            PostMessageW(
                state.parent,
                WM_MENU_SCALE,
                0,
                state.settings.scale.to_bits() as isize,
            );
        }
        Some(SliderDrag::Volume) => {
            state.settings.volume = progress;
            PostMessageW(
                state.parent,
                WM_MENU_VOLUME,
                0,
                state.settings.volume.to_bits() as isize,
            );
        }
        None => {}
    }
    render_menu(state);
}

fn cycle_model(state: &mut DataSettingsState) {
    let Some(capabilities) = state.capabilities.as_ref() else {
        return;
    };
    if capabilities.models.is_empty() {
        state.settings.model.clear();
        state.settings.reasoning_effort.clear();
        return;
    }
    let current = capabilities
        .models
        .iter()
        .position(|model| model.model.eq_ignore_ascii_case(&state.settings.model));
    let next = current.map_or(0, |index| (index + 1) % capabilities.models.len());
    state.settings.model = capabilities.models[next].model.clone();
    state.settings.reasoning_effort = capabilities.models[next]
        .reasoning_efforts
        .first()
        .cloned()
        .unwrap_or_default();
}

fn cycle_reasoning_effort(state: &mut DataSettingsState) {
    let Some(model) = state.capabilities.as_ref().and_then(|capabilities| {
        capabilities
            .models
            .iter()
            .find(|model| model.model.eq_ignore_ascii_case(&state.settings.model))
    }) else {
        state.settings.reasoning_effort.clear();
        return;
    };
    let mut efforts = vec![String::new()];
    efforts.extend(model.reasoning_efforts.iter().cloned());
    let current = efforts
        .iter()
        .position(|effort| effort.eq_ignore_ascii_case(&state.settings.reasoning_effort));
    state.settings.reasoning_effort =
        efforts[current.map_or(0, |index| (index + 1) % efforts.len())].clone();
}

unsafe fn save_data_settings(hwnd: HWND, state: &DataSettingsState) {
    let result = Box::new(DataSettingsResult {
        model: state.settings.model.clone(),
        reasoning_effort: state.settings.reasoning_effort.clone(),
        card_overrides: state.settings.card_overrides.clone(),
    });
    let pointer = Box::into_raw(result);
    if PostMessageW(state.parent, WM_DATA_SETTINGS_SAVED, 0, pointer as isize) == 0 {
        drop(Box::from_raw(pointer));
        return;
    }
    DestroyWindow(hwnd);
}

fn render_data_settings(state: &mut DataSettingsState) {
    let model = if state.settings.model.is_empty() {
        "自动".to_string()
    } else {
        model_display_name(state.capabilities.as_ref(), &state.settings.model)
    };
    let reasoning_effort = if state.settings.reasoning_effort.is_empty() {
        "自动".to_string()
    } else {
        state.settings.reasoning_effort.clone()
    };
    let cards = DATA_CARDS
        .iter()
        .map(|(id, label)| {
            (
                *label,
                card_enabled(&state.settings, state.capabilities.as_ref(), id),
            )
        })
        .collect::<Vec<_>>();
    if let Some(renderer) = &mut state.renderer {
        let _ = renderer.render_data_settings(&DataSettingsPanelData {
            model: &model,
            reasoning_effort: &reasoning_effort,
            cards: &cards,
        });
    }
}

fn render_menu(state: &mut MenuState) {
    if let Some(renderer) = &mut state.renderer {
        let _ = renderer.render_menu(&MenuPanelData {
            scale: state.settings.scale,
            volume: state.settings.volume,
            sound_set: &state.settings.sound_set,
            bubble_enabled: state.settings.bubble_enabled,
            always_on_top: state.settings.always_on_top,
            autostart: state.settings.autostart,
            hardware_accelerated: state.hardware_accelerated,
        });
    }
}

fn render_details(state: &mut DetailsState) {
    let rows = details_rows(state);
    let accounts = state.snapshot.as_ref().map_or(0, |value| {
        value
            .accounts
            .iter()
            .filter(|account| account.quota.available)
            .count()
    });
    let signals = state
        .snapshot
        .as_ref()
        .map_or(0, |value| value.signals.len());
    if let Some(renderer) = &mut state.renderer {
        let _ = renderer.render_details(&DetailsPanelData {
            today_tokens: &state.today_tokens,
            today_usd: &state.today_usd,
            startup_tokens: &state.startup_tokens,
            accounts,
            signals,
            hardware_accelerated: state.hardware_accelerated,
            page: state.page,
            rows: &rows,
        });
    }
}

fn details_rows(state: &DetailsState) -> Vec<String> {
    let Some(snapshot) = &state.snapshot else {
        return Vec::new();
    };
    match state.page {
        1 => snapshot
            .models
            .iter()
            .map(|model| model_detail_row(state.capabilities.as_ref(), model))
            .collect(),
        2 => snapshot
            .accounts
            .iter()
            .filter(|account| account.quota.available)
            .map(|account| {
                let remaining = account_primary_remaining_percent(account)
                    .map(|value| format!("剩余 {value:.0}%"))
                    .unwrap_or_else(|| "剩余 --".into());
                format!("{} · {}", account.label, remaining)
            })
            .collect(),
        3 => snapshot
            .signals
            .iter()
            .map(|signal| {
                let summary = abbreviate(&signal.summary, 38);
                if summary.is_empty() {
                    format!("{} · {}", signal.source, signal.title)
                } else {
                    format!("{} · {} · {}", signal.source, signal.title, summary)
                }
            })
            .collect(),
        _ => Vec::new(),
    }
}

fn abbreviate(value: &str, limit: usize) -> String {
    let mut text = value.chars().take(limit).collect::<String>();
    if value.chars().count() > limit {
        text.push('…');
    }
    text
}

fn design_point(lparam: LPARAM, dpi: u32) -> (f32, f32) {
    let scale = dpi.max(1) as f32 / 96.0;
    (
        low_i16(lparam) as f32 / scale,
        high_i16(lparam) as f32 / scale,
    )
}

fn dip(value: f32, dpi: u32) -> i32 {
    (value * dpi.max(1) as f32 / 96.0).round() as i32
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

fn window_rect(hwnd: HWND) -> Option<RECT> {
    let mut rect: RECT = unsafe { zeroed() };
    (unsafe { GetWindowRect(hwnd, &mut rect) } != 0).then_some(rect)
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
