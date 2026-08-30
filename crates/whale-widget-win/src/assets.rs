pub const APP_ICON_RESOURCE_ID: u16 = 1;
pub const WHALE_PNG: &[u8] = include_bytes!("../../../assets/DSniang1.png");
pub const RUA_GIF: &[u8] = include_bytes!("../../../assets/rua.gif");
pub const DUCK_PRESS_WAV: &[u8] = include_bytes!("../../../assets/Ya1.wav");
pub const DUCK_RELEASE_WAV: &[u8] = include_bytes!("../../../assets/Ya2.wav");
pub const FX_PRESS_WAV: &[u8] = include_bytes!("../../../assets/D1.wav");
pub const FX_RELEASE_WAV: &[u8] = include_bytes!("../../../assets/D2.wav");

/// Loads the embedded CPA Whale icon, falling back to the system application icon.
///
/// # Safety
/// `instance` must be the valid module handle for the current executable, or null when the
/// caller intentionally wants only the system fallback.
#[cfg(windows)]
pub unsafe fn load_app_icon(
    instance: windows_sys::Win32::Foundation::HINSTANCE,
) -> windows_sys::Win32::UI::WindowsAndMessaging::HICON {
    use windows_sys::Win32::UI::WindowsAndMessaging::{LoadIconW, IDI_APPLICATION};

    let icon = unsafe { LoadIconW(instance, APP_ICON_RESOURCE_ID as usize as *const u16) };
    if icon.is_null() {
        unsafe { LoadIconW(std::ptr::null_mut(), IDI_APPLICATION) }
    } else {
        icon
    }
}
