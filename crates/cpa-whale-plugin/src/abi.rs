use std::ffi::{c_char, c_void, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::ptr;
use std::sync::OnceLock;

use serde::Deserialize;
use serde_json::Value;

pub const ABI_VERSION: u32 = 1;

#[repr(C)]
pub struct CliproxyBuffer {
    pub ptr: *mut u8,
    pub len: usize,
}

pub type HostCall =
    unsafe extern "C" fn(*mut c_void, *const c_char, *const u8, usize, *mut CliproxyBuffer) -> i32;
pub type HostFree = unsafe extern "C" fn(*mut c_void, usize);
pub type PluginCall =
    unsafe extern "C" fn(*const c_char, *const u8, usize, *mut CliproxyBuffer) -> i32;
pub type PluginFree = unsafe extern "C" fn(*mut c_void, usize);
pub type PluginShutdown = unsafe extern "C" fn();

#[repr(C)]
pub struct CliproxyHostApi {
    pub abi_version: u32,
    pub host_ctx: *mut c_void,
    pub call: Option<HostCall>,
    pub free_buffer: Option<HostFree>,
}

#[repr(C)]
pub struct CliproxyPluginApi {
    pub abi_version: u32,
    pub call: Option<PluginCall>,
    pub free_buffer: Option<PluginFree>,
    pub shutdown: Option<PluginShutdown>,
}

#[derive(Clone, Copy)]
struct HostBridge {
    host_ctx: usize,
    call: HostCall,
    free_buffer: Option<HostFree>,
}

static HOST: OnceLock<HostBridge> = OnceLock::new();

#[derive(Debug, Deserialize)]
struct HostEnvelope {
    ok: bool,
    result: Option<Value>,
    error: Option<HostError>,
}

#[derive(Debug, Deserialize)]
struct HostError {
    code: String,
    message: String,
}

pub unsafe fn install_host(host: *const CliproxyHostApi) -> Result<(), String> {
    if host.is_null() {
        return Err("host API is null".into());
    }
    let host = &*host;
    if host.abi_version != ABI_VERSION {
        return Err(format!("unsupported host ABI {}", host.abi_version));
    }
    let call = host
        .call
        .ok_or_else(|| "host call function is missing".to_string())?;
    let bridge = HostBridge {
        host_ctx: host.host_ctx as usize,
        call,
        free_buffer: host.free_buffer,
    };
    if let Some(existing) = HOST.get() {
        if existing.host_ctx != bridge.host_ctx {
            return Err("host API was already installed with a different context".into());
        }
        return Ok(());
    }
    HOST.set(bridge)
        .map_err(|_| "host API is already installed".to_string())
}

pub fn call_host(method: &str, payload: &[u8]) -> Result<Vec<u8>, String> {
    let bridge = HOST
        .get()
        .ok_or_else(|| "host API is unavailable".to_string())?;
    let method = CString::new(method).map_err(|_| "host method contains NUL".to_string())?;
    let mut response = CliproxyBuffer {
        ptr: ptr::null_mut(),
        len: 0,
    };
    let code = unsafe {
        (bridge.call)(
            bridge.host_ctx as *mut c_void,
            method.as_ptr(),
            payload.as_ptr(),
            payload.len(),
            &mut response,
        )
    };
    let raw = if response.ptr.is_null() || response.len == 0 {
        Vec::new()
    } else {
        unsafe { std::slice::from_raw_parts(response.ptr, response.len).to_vec() }
    };
    if !response.ptr.is_null() {
        if let Some(free) = bridge.free_buffer {
            unsafe { free(response.ptr.cast::<c_void>(), response.len) };
        }
    }
    if raw.is_empty() {
        return Err(format!(
            "host call {method:?} returned no response (code {code})"
        ));
    }
    let envelope: HostEnvelope = serde_json::from_slice(&raw)
        .map_err(|error| format!("decode host response for {method:?}: {error}"))?;
    if !envelope.ok || code != 0 {
        let detail = envelope
            .error
            .map(|error| format!("{}: {}", error.code, error.message))
            .unwrap_or_else(|| format!("host call failed with code {code}"));
        return Err(detail);
    }
    envelope
        .result
        .map(|result| serde_json::to_vec(&result).map_err(|error| error.to_string()))
        .transpose()
        .map(|result| result.unwrap_or_else(|| b"{}".to_vec()))
}

pub unsafe extern "C" fn plugin_call(
    method: *const c_char,
    request: *const u8,
    request_len: usize,
    response: *mut CliproxyBuffer,
) -> i32 {
    clear_response(response);
    let result = catch_unwind(AssertUnwindSafe(|| {
        if method.is_null() {
            return Err("method is required".to_string());
        }
        let method = CStr::from_ptr(method)
            .to_str()
            .map_err(|_| "method is not valid UTF-8".to_string())?;
        let request = if request.is_null() || request_len == 0 {
            &[]
        } else {
            std::slice::from_raw_parts(request, request_len)
        };
        crate::dispatch(method, request)
    }));
    match result {
        Ok(Ok(raw)) => {
            write_response(response, raw);
            0
        }
        Ok(Err(message)) => {
            write_response(response, crate::error_envelope("plugin_error", &message));
            0
        }
        Err(_) => {
            write_response(
                response,
                crate::error_envelope("plugin_panic", "plugin call panicked"),
            );
            1
        }
    }
}

pub unsafe extern "C" fn plugin_free(ptr: *mut c_void, len: usize) {
    if ptr.is_null() || len == 0 {
        return;
    }
    let slice = ptr::slice_from_raw_parts_mut(ptr.cast::<u8>(), len);
    drop(Box::from_raw(slice));
}

pub unsafe extern "C" fn plugin_shutdown() {
    crate::shutdown();
}

unsafe fn clear_response(response: *mut CliproxyBuffer) {
    if !response.is_null() {
        (*response).ptr = ptr::null_mut();
        (*response).len = 0;
    }
}

fn write_response(response: *mut CliproxyBuffer, raw: Vec<u8>) {
    if response.is_null() || raw.is_empty() {
        return;
    }
    let boxed = raw.into_boxed_slice();
    let len = boxed.len();
    let ptr = Box::into_raw(boxed).cast::<u8>();
    unsafe {
        (*response).ptr = ptr;
        (*response).len = len;
    }
}
