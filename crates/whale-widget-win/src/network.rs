use std::ffi::c_void;
use std::ptr;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use serde::de::DeserializeOwned;
use whale_protocol::{CapabilitiesResponse, GlobalSnapshot, CAPABILITIES_SCHEMA_VERSION};
use windows_sys::Win32::Foundation::{GetLastError, HWND};
use windows_sys::Win32::Networking::WinHttp::{
    WinHttpAddRequestHeaders, WinHttpCloseHandle, WinHttpConnect, WinHttpOpen, WinHttpOpenRequest,
    WinHttpQueryDataAvailable, WinHttpReadData, WinHttpReceiveResponse, WinHttpSendRequest,
    WinHttpSetTimeouts, WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, WINHTTP_ADDREQ_FLAG_ADD,
    WINHTTP_ADDREQ_FLAG_REPLACE, WINHTTP_FLAG_SECURE,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

pub const WM_SNAPSHOT: u32 = WM_APP + 1;
pub const WM_NETWORK_ERROR: u32 = WM_APP + 2;
pub const WM_CAPABILITIES: u32 = WM_APP + 3;

const DEFAULT_POLL_INTERVAL_SECONDS: u64 = 60;
const MIN_POLL_INTERVAL_SECONDS: u64 = 15;
const MAX_POLL_INTERVAL_SECONDS: u64 = 3600;
const RESOURCE_PATH: &str = "/v0/resource/plugins/cpa-whale";

pub enum PollCommand {
    Refresh,
    Stop,
}

pub struct NetworkHandle {
    sender: Sender<PollCommand>,
    join: Option<JoinHandle<()>>,
}

#[derive(Debug, Clone)]
pub struct ConnectionProbe {
    pub endpoint: String,
    pub capabilities: Option<CapabilitiesResponse>,
    pub snapshot: GlobalSnapshot,
}

impl NetworkHandle {
    pub fn start(hwnd: HWND, endpoint: String, token: String) -> Self {
        let (sender, receiver) = mpsc::channel();
        let hwnd_value = hwnd as usize;
        let join = thread::Builder::new()
            .name("cpa-whale-network".into())
            .spawn(move || poll_loop(hwnd_value as HWND, endpoint, token, receiver))
            .ok();
        Self { sender, join }
    }

    pub fn refresh(&self) {
        let _ = self.sender.send(PollCommand::Refresh);
    }

    pub fn shutdown(&mut self) {
        let _ = self.sender.send(PollCommand::Stop);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

impl Drop for NetworkHandle {
    fn drop(&mut self) {
        self.shutdown();
    }
}

pub fn normalize_endpoint(value: &str) -> Result<String, String> {
    let value = value.trim().trim_end_matches('/');
    if value.is_empty() {
        return Err("请输入 CLIProxyAPI 地址".into());
    }
    if !(value.starts_with("https://") || value.starts_with("http://")) {
        return Err("地址必须以 https:// 或 http:// 开头".into());
    }
    if value.contains(RESOURCE_PATH) {
        Ok(value.to_string())
    } else {
        Ok(format!("{value}{RESOURCE_PATH}"))
    }
}

pub fn probe(endpoint: &str, token: &str) -> Result<ConnectionProbe, String> {
    let endpoint = normalize_endpoint(endpoint)?;
    let capabilities = fetch_capabilities(&endpoint, token).ok();
    if let Some(capabilities) = capabilities.as_ref() {
        if capabilities.schema_version > CAPABILITIES_SCHEMA_VERSION {
            return Err(format!(
                "服务端能力协议 v{} 高于客户端支持的 v{}",
                capabilities.schema_version, CAPABILITIES_SCHEMA_VERSION
            ));
        }
    }
    let snapshot = fetch_snapshot(&endpoint, token)?;
    Ok(ConnectionProbe {
        endpoint,
        capabilities,
        snapshot,
    })
}

fn poll_loop(hwnd: HWND, endpoint: String, token: String, receiver: Receiver<PollCommand>) {
    let endpoint = match normalize_endpoint(&endpoint) {
        Ok(endpoint) => endpoint,
        Err(error) => {
            post_error(hwnd, error);
            return;
        }
    };
    let mut interval_seconds = DEFAULT_POLL_INTERVAL_SECONDS;
    let mut polls_since_capabilities = u64::MAX;
    loop {
        if polls_since_capabilities >= 10 {
            if let Ok(capabilities) = fetch_capabilities(&endpoint, &token) {
                interval_seconds = capabilities
                    .defaults
                    .poll_interval_seconds
                    .clamp(MIN_POLL_INTERVAL_SECONDS, MAX_POLL_INTERVAL_SECONDS);
                post_boxed(hwnd, WM_CAPABILITIES, capabilities);
            }
            polls_since_capabilities = 0;
        }
        post_result(hwnd, fetch_snapshot(&endpoint, &token));
        polls_since_capabilities = polls_since_capabilities.saturating_add(1);
        match receiver.recv_timeout(Duration::from_secs(interval_seconds)) {
            Ok(PollCommand::Stop) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Ok(PollCommand::Refresh) => polls_since_capabilities = u64::MAX,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn post_result(hwnd: HWND, result: Result<GlobalSnapshot, String>) {
    match result {
        Ok(snapshot) => post_boxed(hwnd, WM_SNAPSHOT, snapshot),
        Err(error) => post_error(hwnd, error),
    }
}

fn post_error(hwnd: HWND, error: String) {
    post_boxed(hwnd, WM_NETWORK_ERROR, error);
}

fn post_boxed<T>(hwnd: HWND, message: u32, value: T) {
    let pointer = Box::into_raw(Box::new(value));
    let posted = unsafe { PostMessageW(hwnd, message, 0, pointer as isize) };
    if posted == 0 {
        unsafe { drop(Box::from_raw(pointer)) };
    }
}

fn fetch_capabilities(endpoint: &str, token: &str) -> Result<CapabilitiesResponse, String> {
    fetch_json(endpoint, token, "/v1/capabilities")
}

fn fetch_snapshot(endpoint: &str, token: &str) -> Result<GlobalSnapshot, String> {
    fetch_json(endpoint, token, "/v1/snapshot")
}

fn fetch_json<T: DeserializeOwned>(
    endpoint: &str,
    token: &str,
    resource: &str,
) -> Result<T, String> {
    if token.trim().is_empty() {
        return Err("尚未配置 Whale 只读令牌，请从小鲸鱼菜单打开连接设置".into());
    }
    let url = ParsedUrl::parse(&format!("{}{resource}", endpoint.trim_end_matches('/')))?;
    let agent = wide(&format!("CPA Whale/{}", env!("CARGO_PKG_VERSION")));
    let session = unsafe {
        WinHttpOpen(
            agent.as_ptr(),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            ptr::null(),
            ptr::null(),
            0,
        )
    };
    let session = InternetHandle::new(session, "WinHttpOpen")?;
    unsafe {
        WinHttpSetTimeouts(session.0, 5_000, 5_000, 10_000, 10_000);
    }
    let host = wide(&url.host);
    let connection = unsafe { WinHttpConnect(session.0, host.as_ptr(), url.port, 0) };
    let connection = InternetHandle::new(connection, "WinHttpConnect")?;
    let method = wide("GET");
    let path = wide(&url.path);
    let flags = if url.secure { WINHTTP_FLAG_SECURE } else { 0 };
    let request = unsafe {
        WinHttpOpenRequest(
            connection.0,
            method.as_ptr(),
            path.as_ptr(),
            ptr::null(),
            ptr::null(),
            ptr::null(),
            flags,
        )
    };
    let request = InternetHandle::new(request, "WinHttpOpenRequest")?;
    let headers = wide(&format!(
        "Authorization: Bearer {token}\r\nAccept: application/json\r\n"
    ));
    let added = unsafe {
        WinHttpAddRequestHeaders(
            request.0,
            headers.as_ptr(),
            u32::MAX,
            WINHTTP_ADDREQ_FLAG_ADD | WINHTTP_ADDREQ_FLAG_REPLACE,
        )
    };
    if added == 0 {
        return Err(last_error("WinHttpAddRequestHeaders"));
    }
    let sent = unsafe { WinHttpSendRequest(request.0, ptr::null(), 0, ptr::null_mut(), 0, 0, 0) };
    if sent == 0 {
        return Err(last_error("WinHttpSendRequest"));
    }
    if unsafe { WinHttpReceiveResponse(request.0, ptr::null_mut()) } == 0 {
        return Err(last_error("WinHttpReceiveResponse"));
    }
    let mut body = Vec::new();
    loop {
        let mut available = 0_u32;
        if unsafe { WinHttpQueryDataAvailable(request.0, &mut available) } == 0 {
            return Err(last_error("WinHttpQueryDataAvailable"));
        }
        if available == 0 {
            break;
        }
        if body.len().saturating_add(available as usize) > 4 * 1024 * 1024 {
            return Err("Whale API response exceeds 4 MiB".into());
        }
        let start = body.len();
        body.resize(start + available as usize, 0);
        let mut read = 0_u32;
        if unsafe {
            WinHttpReadData(
                request.0,
                body[start..].as_mut_ptr().cast::<c_void>(),
                available,
                &mut read,
            )
        } == 0
        {
            return Err(last_error("WinHttpReadData"));
        }
        body.truncate(start + read as usize);
    }
    serde_json::from_slice::<T>(&body).map_err(|error| {
        let preview = String::from_utf8_lossy(&body);
        format!(
            "decode Whale API response: {error}; response={}",
            preview.chars().take(200).collect::<String>()
        )
    })
}

struct InternetHandle(*mut c_void);

impl InternetHandle {
    fn new(handle: *mut c_void, operation: &str) -> Result<Self, String> {
        if handle.is_null() {
            Err(last_error(operation))
        } else {
            Ok(Self(handle))
        }
    }
}

impl Drop for InternetHandle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { WinHttpCloseHandle(self.0) };
        }
    }
}

struct ParsedUrl {
    secure: bool,
    host: String,
    port: u16,
    path: String,
}

impl ParsedUrl {
    fn parse(value: &str) -> Result<Self, String> {
        let (secure, remainder) = if let Some(value) = value.strip_prefix("https://") {
            (true, value)
        } else if let Some(value) = value.strip_prefix("http://") {
            (false, value)
        } else {
            return Err("endpoint must begin with http:// or https://".into());
        };
        let (authority, path) = remainder
            .split_once('/')
            .map(|(authority, path)| (authority, format!("/{path}")))
            .unwrap_or((remainder, "/".into()));
        let (host, port) = authority
            .rsplit_once(':')
            .and_then(|(host, port)| port.parse::<u16>().ok().map(|port| (host, port)))
            .unwrap_or((authority, if secure { 443 } else { 80 }));
        if host.is_empty() {
            return Err("endpoint host is empty".into());
        }
        Ok(Self {
            secure,
            host: host.to_string(),
            port,
            path,
        })
    }
}

fn last_error(operation: &str) -> String {
    format!("{operation} failed with Windows error {}", unsafe {
        GetLastError()
    })
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_cpa_root_and_preserves_full_plugin_path() {
        assert_eq!(
            normalize_endpoint("https://example.test").unwrap(),
            "https://example.test/v0/resource/plugins/cpa-whale"
        );
        assert_eq!(
            normalize_endpoint("https://example.test/v0/resource/plugins/cpa-whale/").unwrap(),
            "https://example.test/v0/resource/plugins/cpa-whale"
        );
    }
}
