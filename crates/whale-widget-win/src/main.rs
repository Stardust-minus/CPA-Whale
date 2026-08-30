#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
fn main() {
    if let Err(error) = whale_widget_win::win32::run() {
        let message: Vec<u16> = error.encode_utf16().chain(std::iter::once(0)).collect();
        let title: Vec<u16> = "CPA Whale 启动失败"
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            windows_sys::Win32::UI::WindowsAndMessaging::MessageBoxW(
                std::ptr::null_mut(),
                message.as_ptr(),
                title.as_ptr(),
                windows_sys::Win32::UI::WindowsAndMessaging::MB_OK
                    | windows_sys::Win32::UI::WindowsAndMessaging::MB_ICONERROR,
            );
        }
    }
}

#[cfg(not(windows))]
fn main() {
    println!("CPA Whale Windows client must be built for a Windows target");
}
