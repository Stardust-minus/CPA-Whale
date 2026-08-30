use std::mem::{size_of, zeroed};
use std::ptr;
use std::thread;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::Deserialize;

use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    CreateFontW, CreateSolidBrush, DeleteObject, SetBkColor, SetTextColor, ValidateRect, FW_NORMAL,
    HBRUSH, HGDIOBJ,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{SetFocus, VK_ESCAPE, VK_RETURN};
use windows_sys::Win32::UI::WindowsAndMessaging::*;

use crate::graphics::{PanelRenderer, SetupPanelData};

pub const WM_CONFIG_SAVED: u32 = WM_APP + 20;
const WM_CONNECTION_TESTED: u32 = WM_APP + 21;
const CLASS_NAME: &str = "CPAWhaleSetupWindowV2";
const ID_ENDPOINT: usize = 2001;
const ID_TOKEN: usize = 2002;
const DESIGN_WIDTH: f32 = 560.0;
const DESIGN_HEIGHT: f32 = 340.0;

pub struct SetupResult {
    pub endpoint: String,
    pub token: String,
}

struct SetupState {
    parent: HWND,
    endpoint: HWND,
    token: HWND,
    renderer: Option<PanelRenderer>,
    edit_background: HBRUSH,
    edit_font: *mut std::ffi::c_void,
    status: Option<String>,
    saving: bool,
    insecure_http_confirmed: bool,
    dpi: u32,
}

/// Opens the owner-bound CPA Whale connection setup window.
///
/// # Safety
/// `parent` must be a valid live window handle owned by the current process.
pub unsafe fn show(parent: HWND, endpoint: &str) -> Result<HWND, String> {
    let instance = GetModuleHandleW(ptr::null());
    if instance.is_null() {
        return Err("GetModuleHandleW failed".into());
    }
    let class = wide(CLASS_NAME);
    let window_class = WNDCLASSEXW {
        cbSize: size_of::<WNDCLASSEXW>() as u32,
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(window_proc),
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
    RegisterClassExW(&window_class);
    let dpi = GetDpiForWindow(windows::Win32::Foundation::HWND(parent)).max(96);
    let width = dip(DESIGN_WIDTH, dpi);
    let height = dip(DESIGN_HEIGHT, dpi);
    let parent_rect = window_rect(parent).unwrap_or(RECT {
        left: 0,
        top: 0,
        right: GetSystemMetrics(SM_CXSCREEN),
        bottom: GetSystemMetrics(SM_CYSCREEN),
    });
    let x = (parent_rect.left + parent_rect.right - width) / 2;
    let y = (parent_rect.top + parent_rect.bottom - height) / 2;
    let state = Box::new(SetupState {
        parent,
        endpoint: ptr::null_mut(),
        token: ptr::null_mut(),
        renderer: None,
        edit_background: CreateSolidBrush(0x00ff_ffff),
        edit_font: ptr::null_mut(),
        status: None,
        saving: false,
        insecure_http_confirmed: false,
        dpi,
    });
    let state_ptr = Box::into_raw(state);
    let hwnd = CreateWindowExW(
        WS_EX_TOOLWINDOW | WS_EX_TOPMOST | WS_EX_NOREDIRECTIONBITMAP,
        class.as_ptr(),
        wide("CPA Whale 连接设置").as_ptr(),
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
        return Err("创建连接设置窗口失败".into());
    }
    let state = &mut *state_ptr;
    SetWindowTextW(state.endpoint, wide(endpoint).as_ptr());
    SetFocus(state.endpoint);
    SetForegroundWindow(hwnd);
    Ok(hwnd)
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
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut SetupState;
    if state_ptr.is_null() {
        return DefWindowProcW(hwnd, message, wparam, lparam);
    }
    let state = &mut *state_ptr;
    match message {
        WM_CREATE => {
            let instance = GetModuleHandleW(ptr::null());
            state.endpoint = CreateWindowExW(
                0,
                wide("EDIT").as_ptr(),
                ptr::null(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32,
                dip(42.0, state.dpi),
                dip(120.0, state.dpi),
                dip(476.0, state.dpi),
                dip(28.0, state.dpi),
                hwnd,
                ID_ENDPOINT as HMENU,
                instance,
                ptr::null_mut(),
            );
            state.token = CreateWindowExW(
                0,
                wide("EDIT").as_ptr(),
                ptr::null(),
                WS_CHILD | WS_VISIBLE | WS_TABSTOP | ES_AUTOHSCROLL as u32 | ES_PASSWORD as u32,
                dip(42.0, state.dpi),
                dip(198.0, state.dpi),
                dip(476.0, state.dpi),
                dip(28.0, state.dpi),
                hwnd,
                ID_TOKEN as HMENU,
                instance,
                ptr::null_mut(),
            );
            state.edit_font = CreateFontW(
                -dip(14.0, state.dpi),
                0,
                0,
                0,
                FW_NORMAL as i32,
                0,
                0,
                0,
                1,
                0,
                0,
                5,
                0,
                wide("Segoe UI").as_ptr(),
            );
            SendMessageW(state.endpoint, WM_SETFONT, state.edit_font as usize, 1);
            SendMessageW(state.token, WM_SETFONT, state.edit_font as usize, 1);
            let mut rect: RECT = zeroed();
            GetClientRect(hwnd, &mut rect);
            match PanelRenderer::new(
                hwnd,
                (rect.right - rect.left).max(1) as u32,
                (rect.bottom - rect.top).max(1) as u32,
                state.dpi,
            ) {
                Ok(renderer) => state.renderer = Some(renderer),
                Err(error) => state.status = Some(format!("GPU 面板初始化失败: {error}")),
            }
            render(state);
            0
        }
        WM_NCHITTEST => {
            let screen_x = low_i16(lparam) as i32;
            let screen_y = high_i16(lparam) as i32;
            let rect = window_rect(hwnd).unwrap_or_else(|| zeroed());
            let local_x = (screen_x - rect.left) as f32 * 96.0 / state.dpi.max(1) as f32;
            let local_y = (screen_y - rect.top) as f32 * 96.0 / state.dpi.max(1) as f32;
            if local_x >= 500.0 && local_y <= 62.0 {
                HTCLIENT as LRESULT
            } else if local_y < 82.0 {
                HTCAPTION as LRESULT
            } else {
                HTCLIENT as LRESULT
            }
        }
        WM_LBUTTONUP => {
            let x = low_i16(lparam) as i32;
            let y = high_i16(lparam) as i32;
            if point_in_design(x, y, state.dpi, 500.0, 12.0, 48.0, 48.0) {
                DestroyWindow(hwnd);
            } else if point_in_design(x, y, state.dpi, 378.0, 252.0, 152.0, 48.0) {
                save_setup(hwnd, state);
            }
            0
        }
        WM_KEYDOWN if wparam == VK_ESCAPE as usize => {
            DestroyWindow(hwnd);
            0
        }
        WM_KEYDOWN if wparam == VK_RETURN as usize => {
            save_setup(hwnd, state);
            0
        }
        WM_CONNECTION_TESTED => {
            let result = Box::from_raw(lparam as *mut Result<SetupResult, String>);
            finish_connection_test(hwnd, state, *result);
            0
        }
        WM_CTLCOLORSTATIC | WM_CTLCOLOREDIT => {
            let dc = wparam as *mut std::ffi::c_void;
            SetTextColor(dc, 0x0070_3120);
            SetBkColor(dc, 0x00ff_ffff);
            state.edit_background as LRESULT
        }
        WM_PAINT => {
            ValidateRect(hwnd, ptr::null());
            render(state);
            0
        }
        WM_CLOSE => {
            DestroyWindow(hwnd);
            0
        }
        WM_NCDESTROY => {
            if !state.edit_background.is_null() {
                DeleteObject(state.edit_background as HGDIOBJ);
            }
            if !state.edit_font.is_null() {
                DeleteObject(state.edit_font as HGDIOBJ);
            }
            SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
            drop(Box::from_raw(state_ptr));
            DefWindowProcW(hwnd, message, wparam, lparam)
        }
        _ => DefWindowProcW(hwnd, message, wparam, lparam),
    }
}

unsafe fn save_setup(hwnd: HWND, state: &mut SetupState) {
    if state.saving {
        return;
    }
    let endpoint_input = window_text(state.endpoint);
    let token_input = window_text(state.token);
    let (endpoint, token) = match decode_connection_input(&endpoint_input, &token_input) {
        Ok(connection) => connection,
        Err(error) => {
            state.status = Some(error);
            render(state);
            return;
        }
    };
    let endpoint = match crate::network::normalize_endpoint(&endpoint) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            state.status = Some(error);
            render(state);
            return;
        }
    };
    if token.trim().is_empty() {
        state.status = Some("请输入 Whale 只读令牌".into());
        render(state);
        return;
    }
    if is_public_http(&endpoint) && !state.insecure_http_confirmed {
        state.insecure_http_confirmed = true;
        state.status = Some("公网 HTTP 会泄露只读令牌；再次点击才继续".into());
        render(state);
        return;
    }

    state.saving = true;
    state.status = Some("正在验证连接…".into());
    render(state);
    let hwnd_value = hwnd as usize;
    if let Err(error) = thread::Builder::new()
        .name("cpa-whale-connection-test".into())
        .spawn(move || {
            let result = crate::network::probe(&endpoint, &token).map(|probe| SetupResult {
                endpoint: probe.endpoint,
                token,
            });
            let pointer = Box::into_raw(Box::new(result));
            if unsafe {
                PostMessageW(
                    hwnd_value as HWND,
                    WM_CONNECTION_TESTED,
                    0,
                    pointer as isize,
                )
            } == 0
            {
                unsafe { drop(Box::from_raw(pointer)) };
            }
        })
    {
        state.saving = false;
        state.status = Some(format!("无法启动连接测试: {error}"));
        render(state);
    }
}

unsafe fn finish_connection_test(
    hwnd: HWND,
    state: &mut SetupState,
    result: Result<SetupResult, String>,
) {
    match result {
        Ok(result) => {
            let pointer = Box::into_raw(Box::new(result));
            if PostMessageW(state.parent, WM_CONFIG_SAVED, 0, pointer as isize) == 0 {
                drop(Box::from_raw(pointer));
                state.saving = false;
                state.status = Some("无法把设置发送给小鲸鱼".into());
                render(state);
                return;
            }
            DestroyWindow(hwnd);
        }
        Err(error) => {
            state.saving = false;
            state.status = Some(error);
            render(state);
        }
    }
}

#[derive(Deserialize)]
struct ConnectionCode {
    endpoint: String,
    token: String,
}

fn decode_connection_input(endpoint: &str, token: &str) -> Result<(String, String), String> {
    let endpoint = endpoint.trim();
    let Some(encoded) = endpoint.strip_prefix("CPAW1-") else {
        return Ok((endpoint.to_string(), token.trim().to_string()));
    };
    let raw = URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|error| format!("连接码格式错误: {error}"))?;
    let connection = serde_json::from_slice::<ConnectionCode>(&raw)
        .map_err(|error| format!("连接码内容错误: {error}"))?;
    Ok((connection.endpoint, connection.token))
}

fn is_public_http(endpoint: &str) -> bool {
    let Some(authority) = endpoint.strip_prefix("http://") else {
        return false;
    };
    let authority = authority.split('/').next().unwrap_or_default();
    let host = if let Some(bracketed) = authority.strip_prefix('[') {
        bracketed.split(']').next().unwrap_or_default()
    } else {
        authority.split(':').next().unwrap_or_default()
    }
    .to_ascii_lowercase();
    if host == "localhost" || host == "::1" || host.starts_with("127.") || host.starts_with("10.") {
        return false;
    }
    if host.starts_with("192.168.") {
        return false;
    }
    if let Some(second) = host
        .strip_prefix("172.")
        .and_then(|value| value.split('.').next())
        .and_then(|value| value.parse::<u8>().ok())
    {
        return !(16..=31).contains(&second);
    }
    true
}

fn render(state: &mut SetupState) {
    if let Some(renderer) = &mut state.renderer {
        let _ = renderer.render_setup(&SetupPanelData {
            status: state.status.as_deref(),
            saving: state.saving,
        });
    }
}

unsafe fn window_text(hwnd: HWND) -> String {
    let length = GetWindowTextLengthW(hwnd);
    let mut buffer = vec![0_u16; length as usize + 1];
    let read = GetWindowTextW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32);
    String::from_utf16_lossy(&buffer[..read.max(0) as usize])
}

fn point_in_design(x: i32, y: i32, dpi: u32, left: f32, top: f32, width: f32, height: f32) -> bool {
    let scale = dpi.max(1) as f32 / 96.0;
    let x = x as f32 / scale;
    let y = y as f32 / scale;
    x >= left && x <= left + width && y >= top && y <= top + height
}

fn dip(value: f32, dpi: u32) -> i32 {
    (value * dpi.max(1) as f32 / 96.0).round() as i32
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
