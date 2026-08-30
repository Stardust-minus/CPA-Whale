use std::fs;
use std::path::{Path, PathBuf};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;

use crate::model::ClientSettings;

pub struct LoadedSettings {
    pub settings: ClientSettings,
    pub token: String,
    pub path: PathBuf,
}

pub fn load() -> Result<LoadedSettings, String> {
    let path = settings_path()?;
    let mut settings = if path.exists() {
        let raw = fs::read(&path).map_err(|error| format!("read {}: {error}", path.display()))?;
        serde_json::from_slice::<ClientSettings>(&raw)
            .map_err(|error| format!("decode {}: {error}", path.display()))?
    } else {
        ClientSettings::default()
    };
    let token = if settings.protected_token.is_empty() {
        String::new()
    } else {
        unprotect(&settings.protected_token)?
    };
    settings.startup_baseline = None;
    settings.normalize();
    save(&path, &settings)?;
    Ok(LoadedSettings {
        settings,
        token,
        path,
    })
}

pub fn save(path: &Path, settings: &ClientSettings) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("create {}: {error}", parent.display()))?;
    }
    let raw =
        serde_json::to_vec_pretty(settings).map_err(|error| format!("encode settings: {error}"))?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, raw)
        .map_err(|error| format!("write {}: {error}", temporary.display()))?;
    fs::rename(&temporary, path).map_err(|error| format!("replace {}: {error}", path.display()))
}

pub fn protect_token(settings: &mut ClientSettings, token: &str) -> Result<(), String> {
    settings.protected_token = protect(token)?;
    Ok(())
}

pub fn settings_path() -> Result<PathBuf, String> {
    let root = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .ok_or_else(|| "LOCALAPPDATA is not set".to_string())?;
    Ok(root.join("CPAWhale").join("config.json"))
}

#[cfg(windows)]
fn protect(value: &str) -> Result<String, String> {
    use std::ptr;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let input = CRYPT_INTEGER_BLOB {
        cbData: value.len() as u32,
        pbData: value.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };
    let ok = unsafe {
        CryptProtectData(
            &input,
            ptr::null(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err("CryptProtectData failed".into());
    }
    let protected = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) };
    let encoded = STANDARD.encode(protected);
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(encoded)
}

#[cfg(windows)]
fn unprotect(encoded: &str) -> Result<String, String> {
    use std::ptr;
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    let mut protected = STANDARD
        .decode(encoded)
        .map_err(|error| format!("decode protected token: {error}"))?;
    let input = CRYPT_INTEGER_BLOB {
        cbData: protected.len() as u32,
        pbData: protected.as_mut_ptr(),
    };
    let mut output = CRYPT_INTEGER_BLOB {
        cbData: 0,
        pbData: ptr::null_mut(),
    };
    let ok = unsafe {
        CryptUnprotectData(
            &input,
            ptr::null_mut(),
            ptr::null(),
            ptr::null_mut(),
            ptr::null(),
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
    };
    if ok == 0 {
        return Err("CryptUnprotectData failed".into());
    }
    let bytes = unsafe { std::slice::from_raw_parts(output.pbData, output.cbData as usize) };
    let value = String::from_utf8(bytes.to_vec())
        .map_err(|error| format!("protected token is not UTF-8: {error}"))?;
    unsafe {
        LocalFree(output.pbData.cast());
    }
    Ok(value)
}

#[cfg(not(windows))]
fn protect(value: &str) -> Result<String, String> {
    Ok(STANDARD.encode(value))
}

#[cfg(not(windows))]
fn unprotect(encoded: &str) -> Result<String, String> {
    let bytes = STANDARD
        .decode(encoded)
        .map_err(|error| error.to_string())?;
    String::from_utf8(bytes).map_err(|error| error.to_string())
}

#[cfg(windows)]
pub fn install_autostart() -> Result<(), String> {
    use windows_sys::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};

    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let command = format!("\"{}\"", executable.display());
    let subkey = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    let name = wide("CPAWhale");
    let value = wide(&command);
    let status = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            name.as_ptr(),
            REG_SZ,
            value.as_ptr().cast(),
            (value.len() * 2) as u32,
        )
    };
    if status == 0 {
        Ok(())
    } else {
        Err(format!("enable autostart failed with status {status}"))
    }
}

#[cfg(windows)]
pub fn remove_autostart() -> Result<(), String> {
    use windows_sys::Win32::System::Registry::{RegDeleteKeyValueW, HKEY_CURRENT_USER};
    let subkey = wide("Software\\Microsoft\\Windows\\CurrentVersion\\Run");
    let name = wide("CPAWhale");
    let status = unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, subkey.as_ptr(), name.as_ptr()) };
    if status == 0 || status == 2 {
        Ok(())
    } else {
        Err(format!("disable autostart failed with status {status}"))
    }
}

#[cfg(not(windows))]
pub fn install_autostart() -> Result<(), String> {
    Ok(())
}

#[cfg(not(windows))]
pub fn remove_autostart() -> Result<(), String> {
    Ok(())
}

#[cfg(windows)]
fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}
