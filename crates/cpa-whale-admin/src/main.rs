use std::env;
use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const DEFAULT_CONFIG: &str = "/etc/cliproxyapi/config.yaml";
const DEFAULT_PLUGIN_DIR: &str = "/var/lib/cliproxyapi/plugins/linux/amd64";
const DEFAULT_DATABASE: &str = "/var/lib/cliproxyapi/whale/metrics.db";
const DEFAULT_STATE_DIR: &str = "/var/lib/cliproxyapi/whale";
const RESOURCE_PATH: &str = "/v0/resource/plugins/cpa-whale";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConnectionCode {
    endpoint: String,
    token: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct InstallManifest {
    version: String,
    installed_at_unix: u64,
    installed_plugin: PathBuf,
    replaced_plugin_backup: Option<PathBuf>,
    config_path: Option<PathBuf>,
    config_backup: Option<PathBuf>,
    database_path: Option<PathBuf>,
    database_backup: Option<PathBuf>,
}

fn main() {
    if let Err(error) = run(env::args().skip(1).collect()) {
        eprintln!("cpa-whale-admin: {error}");
        std::process::exit(1);
    }
}

fn run(args: Vec<String>) -> Result<(), String> {
    let Some(command) = args.first().map(String::as_str) else {
        print_help();
        return Ok(());
    };
    let rest = &args[1..];
    match command {
        "check" => command_check(rest),
        "token" if rest.first().map(String::as_str) == Some("generate") => {
            command_token_generate(&rest[1..])
        }
        "config" if rest.first().map(String::as_str) == Some("render") => {
            command_config_render(&rest[1..])
        }
        "install" => command_install(rest),
        "doctor" => command_doctor(rest),
        "rollback" => command_rollback(rest),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        _ => Err(format!("unknown command: {}", args.join(" "))),
    }
}

fn print_help() {
    println!(
        "CPA Whale administration tool v{}\n\
         \n\
         Commands:\n\
           check [--config PATH]\n\
           token generate [--endpoint URL]\n\
           config render --token-sha256 HEX [--token-id NAME] [--database PATH] [--timezone TZ]\n\
           install --plugin PATH [--plugin-dir DIR] [--config PATH] [--database PATH] [--state-dir DIR]\n\
           doctor --endpoint URL [--token TOKEN]\n\
           rollback [--manifest PATH]",
        env!("CARGO_PKG_VERSION")
    );
}

fn command_check(args: &[String]) -> Result<(), String> {
    let config_path = option_path(args, "--config")
        .or_else(|| env::var_os("CPA_CONFIG").map(PathBuf::from))
        .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG));
    let plugin_dir = detect_plugin_dir(&config_path)
        .unwrap_or_else(|| PathBuf::from(default_plugin_dir_for_arch()));
    let deployment = if Path::new("/.dockerenv").exists() {
        "docker"
    } else {
        "native-or-host"
    };
    println!("architecture: {}", env::consts::ARCH);
    println!("deployment: {deployment}");
    println!(
        "config: {} ({})",
        config_path.display(),
        existence(&config_path)
    );
    println!(
        "plugin-dir: {} ({})",
        plugin_dir.display(),
        existence(&plugin_dir)
    );
    println!("database-default: {DEFAULT_DATABASE}");
    println!(
        "cli-proxy-api: {}",
        detect_cpa_version().unwrap_or_else(|| "not found".into())
    );
    println!(
        "plugin-support: verify CLIProxyAPI reports X-CPA-SUPPORT-PLUGIN: 1 before activation"
    );
    Ok(())
}

fn command_token_generate(args: &[String]) -> Result<(), String> {
    let raw = generate_token()?;
    let digest = sha256_hex(raw.as_bytes());
    println!("WHALE_READ_TOKEN={raw}");
    println!("WHALE_READ_TOKEN_SHA256={digest}");
    if let Some(endpoint) = option(args, "--endpoint") {
        let endpoint = normalize_endpoint(endpoint)?;
        println!(
            "WHALE_CONNECTION_CODE={}",
            connection_code(&endpoint, &raw)?
        );
    }
    Ok(())
}

fn command_config_render(args: &[String]) -> Result<(), String> {
    let digest = required_option(args, "--token-sha256")?;
    validate_digest(digest)?;
    let token_id = option(args, "--token-id").unwrap_or("desktop");
    let database = option(args, "--database").unwrap_or(DEFAULT_DATABASE);
    let timezone = option(args, "--timezone").unwrap_or("UTC");
    println!("{}", render_config(token_id, digest, database, timezone)?);
    Ok(())
}

fn command_install(args: &[String]) -> Result<(), String> {
    let plugin_source = PathBuf::from(required_option(args, "--plugin")?);
    if !plugin_source.is_file() {
        return Err(format!(
            "plugin does not exist: {}",
            plugin_source.display()
        ));
    }
    validate_plugin_architecture(&plugin_source)?;

    let config_path = option_path(args, "--config")
        .or_else(|| env::var_os("CPA_CONFIG").map(PathBuf::from))
        .or_else(|| {
            Path::new(DEFAULT_CONFIG)
                .exists()
                .then(|| PathBuf::from(DEFAULT_CONFIG))
        });
    let plugin_dir = option_path(args, "--plugin-dir")
        .or_else(|| config_path.as_deref().and_then(detect_plugin_dir))
        .unwrap_or_else(|| PathBuf::from(default_plugin_dir_for_arch()));
    let database_path =
        option_path(args, "--database").unwrap_or_else(|| PathBuf::from(DEFAULT_DATABASE));
    let state_dir =
        option_path(args, "--state-dir").unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_DIR));
    fs::create_dir_all(&plugin_dir)
        .map_err(|error| format!("create plugin directory {}: {error}", plugin_dir.display()))?;
    fs::create_dir_all(&state_dir)
        .map_err(|error| format!("create state directory {}: {error}", state_dir.display()))?;

    let now = unix_time()?;
    let destination = plugin_dir.join(format!(
        "cpa-whale-v{}-{}.so",
        env!("CARGO_PKG_VERSION"),
        platform_name()
    ));
    let replaced_plugin_backup = atomic_install(&plugin_source, &destination, now)?;
    let config_backup = config_path
        .as_deref()
        .filter(|path| path.is_file())
        .map(|path| backup_file(path, now))
        .transpose()?;
    let database_backup = database_path
        .is_file()
        .then(|| backup_file(&database_path, now))
        .transpose()?;
    let manifest = InstallManifest {
        version: env!("CARGO_PKG_VERSION").into(),
        installed_at_unix: now,
        installed_plugin: destination.clone(),
        replaced_plugin_backup,
        config_path,
        config_backup,
        database_path: Some(database_path),
        database_backup,
    };
    let manifest_path = state_dir.join("install-manifest.json");
    write_json_atomic(&manifest_path, &manifest)?;

    println!("installed-plugin: {}", destination.display());
    println!("sha256: {}", sha256_file(&destination)?);
    println!("manifest: {}", manifest_path.display());
    println!(
        "next: generate a read token, merge config v2, then enable the plugin in Management UI"
    );
    println!("no CLIProxyAPI restart or Management API operation was performed");
    Ok(())
}

fn command_doctor(args: &[String]) -> Result<(), String> {
    let endpoint = normalize_endpoint(required_option(args, "--endpoint")?)?;
    let token = option(args, "--token")
        .map(ToOwned::to_owned)
        .or_else(|| env::var("WHALE_READ_TOKEN").ok())
        .ok_or_else(|| "--token or WHALE_READ_TOKEN is required".to_string())?;
    let health_url = format!("{endpoint}/v1/health");
    match ureq::get(&health_url).call() {
        Err(ureq::Error::Status(401, _)) => println!("unauthenticated-health: 401 OK"),
        Ok(response) => {
            return Err(format!(
                "unauthenticated health returned {}, expected 401",
                response.status()
            ))
        }
        Err(error) => return Err(format!("unauthenticated health request failed: {error}")),
    }

    let capabilities = authenticated_json(&format!("{endpoint}/v1/capabilities"), &token)?;
    let health = authenticated_json(&health_url, &token)?;
    let snapshot = authenticated_json(&format!("{endpoint}/v1/snapshot"), &token)?;
    require_json_field(&capabilities, "schema_version")?;
    require_json_field(&health, "plugin_version")?;
    require_json_field(&snapshot, "schema_version")?;
    require_json_field(&snapshot, "today")?;
    println!("capabilities: OK");
    println!("health: OK");
    println!("snapshot: OK");
    if health.get("database_ok") != Some(&Value::Bool(true))
        || health.get("writer_ok") != Some(&Value::Bool(true))
    {
        return Err("plugin health reports database or writer failure".into());
    }
    if health
        .get("dropped_events")
        .and_then(Value::as_u64)
        .unwrap_or_default()
        > 0
    {
        return Err("plugin health reports dropped usage events".into());
    }
    println!("doctor: PASS");
    Ok(())
}

fn command_rollback(args: &[String]) -> Result<(), String> {
    let manifest_path = option_path(args, "--manifest")
        .unwrap_or_else(|| PathBuf::from(DEFAULT_STATE_DIR).join("install-manifest.json"));
    let manifest: InstallManifest = serde_json::from_slice(
        &fs::read(&manifest_path)
            .map_err(|error| format!("read {}: {error}", manifest_path.display()))?,
    )
    .map_err(|error| format!("decode {}: {error}", manifest_path.display()))?;
    if let Some(backup) = manifest.replaced_plugin_backup.as_deref() {
        fs::copy(backup, &manifest.installed_plugin).map_err(|error| {
            format!(
                "restore {} from {}: {error}",
                manifest.installed_plugin.display(),
                backup.display()
            )
        })?;
        println!("restored-plugin: {}", manifest.installed_plugin.display());
    } else if manifest.installed_plugin.exists() {
        fs::remove_file(&manifest.installed_plugin)
            .map_err(|error| format!("remove {}: {error}", manifest.installed_plugin.display()))?;
        println!("removed-plugin: {}", manifest.installed_plugin.display());
    }
    if let (Some(config), Some(backup)) = (
        manifest.config_path.as_deref(),
        manifest.config_backup.as_deref(),
    ) {
        fs::copy(backup, config)
            .map_err(|error| format!("restore config {}: {error}", config.display()))?;
        println!("restored-config: {}", config.display());
    }
    println!("disable or reload CPA Whale through the Management UI before deleting mapped files");
    println!("no CLIProxyAPI restart was performed");
    Ok(())
}

fn generate_token() -> Result<String, String> {
    let mut bytes = [0_u8; 32];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|error| format!("read operating-system randomness: {error}"))?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn connection_code(endpoint: &str, token: &str) -> Result<String, String> {
    let raw = serde_json::to_vec(&ConnectionCode {
        endpoint: endpoint.into(),
        token: token.into(),
    })
    .map_err(|error| format!("encode connection code: {error}"))?;
    Ok(format!("CPAW1-{}", URL_SAFE_NO_PAD.encode(raw)))
}

fn render_config(
    token_id: &str,
    digest: &str,
    database: &str,
    timezone: &str,
) -> Result<String, String> {
    let value = json!({
        "plugins": {
            "enabled": true,
            "configs": {
                "cpa-whale": {
                    "config-version": 2,
                    "enabled": true,
                    "priority": 10,
                    "storage": {
                        "database": database,
                        "timezone": timezone,
                        "raw-events-retention-days": 7,
                        "daily-retention-days": 90,
                        "queue-capacity": 4096
                    },
                    "api": {
                        "read-tokens": [{"id": token_id, "sha256": digest}]
                    },
                    "instance": {
                        "display-name": "CLIProxyAPI",
                        "scope-label": "CLIProxyAPI",
                        "poll-interval-seconds": 60
                    },
                    "quota": {
                        "adapters": [{
                            "adapter": "codex-response-headers",
                            "enabled": true,
                            "providers": ["codex"]
                        }],
                        "account-visibility": {
                            "require-available": true,
                            "exclude-unavailable-accounts": true
                        }
                    },
                    "signals": {"enabled": false, "sources": []},
                    "pricing": {"version": "", "rates": []}
                }
            }
        }
    });
    serde_yaml::to_string(&value).map_err(|error| format!("encode config fragment: {error}"))
}

fn detect_plugin_dir(config_path: &Path) -> Option<PathBuf> {
    let raw = fs::read(config_path).ok()?;
    let yaml = serde_yaml::from_slice::<serde_yaml::Value>(&raw).ok()?;
    yaml.get("plugins")?.get("dir")?.as_str().map(|value| {
        let base = PathBuf::from(value);
        base.join("linux").join(arch_directory())
    })
}

fn detect_cpa_version() -> Option<String> {
    for binary in ["cli-proxy-api", "cliproxyapi"] {
        if let Ok(output) = Command::new(binary).arg("--version").output() {
            let text = if output.stdout.is_empty() {
                String::from_utf8_lossy(&output.stderr)
            } else {
                String::from_utf8_lossy(&output.stdout)
            };
            let text = text.trim();
            if !text.is_empty() {
                return Some(text.lines().next().unwrap_or(text).to_string());
            }
        }
    }
    None
}

fn validate_plugin_architecture(path: &Path) -> Result<(), String> {
    let mut header = [0_u8; 20];
    File::open(path)
        .and_then(|mut file| file.read_exact(&mut header))
        .map_err(|error| format!("read plugin ELF header: {error}"))?;
    if &header[..4] != b"\x7fELF" || header[4] != 2 || header[5] != 1 {
        return Err("plugin must be a little-endian 64-bit ELF shared object".into());
    }
    let machine = u16::from_le_bytes([header[18], header[19]]);
    let expected = match env::consts::ARCH {
        "x86_64" => 62,
        "aarch64" => 183,
        architecture => return Err(format!("unsupported host architecture: {architecture}")),
    };
    if machine != expected {
        return Err(format!(
            "plugin ELF machine {machine} does not match host {}",
            env::consts::ARCH
        ));
    }
    Ok(())
}

fn atomic_install(source: &Path, destination: &Path, now: u64) -> Result<Option<PathBuf>, String> {
    let backup = if destination.exists() {
        let backup = destination.with_extension(format!("so.backup-{now}"));
        fs::copy(destination, &backup)
            .map_err(|error| format!("backup {}: {error}", destination.display()))?;
        Some(backup)
    } else {
        None
    };
    let temporary = destination.with_extension("so.tmp");
    fs::copy(source, &temporary)
        .map_err(|error| format!("copy plugin to {}: {error}", temporary.display()))?;
    let file = File::open(&temporary)
        .map_err(|error| format!("open {} for sync: {error}", temporary.display()))?;
    file.sync_all()
        .map_err(|error| format!("sync {}: {error}", temporary.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&temporary, fs::Permissions::from_mode(0o755))
            .map_err(|error| format!("set plugin permissions: {error}"))?;
    }
    fs::rename(&temporary, destination)
        .map_err(|error| format!("activate {}: {error}", destination.display()))?;
    Ok(backup)
}

fn backup_file(path: &Path, now: u64) -> Result<PathBuf, String> {
    let backup = path.with_extension(format!("backup-{now}"));
    fs::copy(path, &backup)
        .map_err(|error| format!("backup {} to {}: {error}", path.display(), backup.display()))?;
    Ok(backup)
}

fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<(), String> {
    let raw = serde_json::to_vec_pretty(value)
        .map_err(|error| format!("encode {}: {error}", path.display()))?;
    let temporary = path.with_extension("json.tmp");
    let mut file = File::create(&temporary)
        .map_err(|error| format!("create {}: {error}", temporary.display()))?;
    file.write_all(&raw)
        .and_then(|_| file.sync_all())
        .map_err(|error| format!("write {}: {error}", temporary.display()))?;
    fs::rename(&temporary, path).map_err(|error| format!("replace {}: {error}", path.display()))
}

fn authenticated_json(url: &str, token: &str) -> Result<Value, String> {
    let response = ureq::get(url)
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/json")
        .call()
        .map_err(|error| format!("GET {url}: {error}"))?;
    let body = response
        .into_string()
        .map_err(|error| format!("read {url}: {error}"))?;
    serde_json::from_str(&body).map_err(|error| format!("decode {url}: {error}"))
}

fn require_json_field(value: &Value, field: &str) -> Result<(), String> {
    value
        .get(field)
        .is_some()
        .then_some(())
        .ok_or_else(|| format!("response is missing {field}"))
}

fn normalize_endpoint(value: &str) -> Result<String, String> {
    let value = value.trim().trim_end_matches('/');
    if !(value.starts_with("https://") || value.starts_with("http://")) {
        return Err("endpoint must begin with http:// or https://".into());
    }
    if value.contains(RESOURCE_PATH) {
        Ok(value.into())
    } else {
        Ok(format!("{value}{RESOURCE_PATH}"))
    }
}

fn validate_digest(value: &str) -> Result<(), String> {
    if value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        Ok(())
    } else {
        Err("token SHA-256 must be 64 hexadecimal characters".into())
    }
}

fn sha256_file(path: &Path) -> Result<String, String> {
    let mut file = File::open(path).map_err(|error| format!("open {}: {error}", path.display()))?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|error| format!("read {}: {error}", path.display()))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hex_lower(&hasher.finalize()))
}

fn sha256_hex(value: &[u8]) -> String {
    hex_lower(&Sha256::digest(value))
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

fn option<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    args.iter()
        .position(|argument| argument == name)
        .and_then(|index| args.get(index + 1))
        .map(String::as_str)
}

fn option_path(args: &[String], name: &str) -> Option<PathBuf> {
    option(args, name).map(PathBuf::from)
}

fn required_option<'a>(args: &'a [String], name: &str) -> Result<&'a str, String> {
    option(args, name).ok_or_else(|| format!("{name} is required"))
}

fn unix_time() -> Result<u64, String> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|error| format!("system clock is before Unix epoch: {error}"))
}

fn default_plugin_dir_for_arch() -> &'static str {
    match env::consts::ARCH {
        "x86_64" => DEFAULT_PLUGIN_DIR,
        "aarch64" => "/var/lib/cliproxyapi/plugins/linux/arm64",
        _ => DEFAULT_PLUGIN_DIR,
    }
}

fn arch_directory() -> &'static str {
    match env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        _ => env::consts::ARCH,
    }
}

fn platform_name() -> &'static str {
    match env::consts::ARCH {
        "x86_64" => "linux-amd64",
        "aarch64" => "linux-arm64",
        _ => "linux-unknown",
    }
}

fn existence(path: &Path) -> &'static str {
    if path.exists() {
        "found"
    } else {
        "not found"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_code_contains_endpoint_and_token() {
        let code = connection_code(
            "https://example.test/v0/resource/plugins/cpa-whale",
            "secret",
        )
        .unwrap();
        let raw = URL_SAFE_NO_PAD
            .decode(code.strip_prefix("CPAW1-").unwrap())
            .unwrap();
        let decoded: ConnectionCode = serde_json::from_slice(&raw).unwrap();
        assert_eq!(decoded.token, "secret");
        assert_eq!(
            decoded.endpoint,
            "https://example.test/v0/resource/plugins/cpa-whale"
        );
    }

    #[test]
    fn rendered_config_is_v2_and_never_contains_raw_token() {
        let digest = "a".repeat(64);
        let rendered = render_config("desktop", &digest, "/tmp/metrics.db", "UTC").unwrap();
        let parsed: serde_yaml::Value = serde_yaml::from_str(&rendered).unwrap();
        assert_eq!(
            parsed["plugins"]["configs"]["cpa-whale"]["config-version"].as_i64(),
            Some(2)
        );
        assert!(rendered.contains(&digest));
        assert!(!rendered.contains("WHALE_READ_TOKEN="));
    }

    #[test]
    fn atomic_install_keeps_replaced_plugin_backup() {
        let directory = env::temp_dir().join(format!("cpa-whale-install-{}", unix_time().unwrap()));
        fs::create_dir_all(&directory).unwrap();
        let source = directory.join("source.so");
        let destination = directory.join("destination.so");
        fs::write(&source, b"new-plugin").unwrap();
        fs::write(&destination, b"old-plugin").unwrap();
        let backup = atomic_install(&source, &destination, unix_time().unwrap())
            .unwrap()
            .unwrap();
        assert_eq!(fs::read(&destination).unwrap(), b"new-plugin");
        assert_eq!(fs::read(&backup).unwrap(), b"old-plugin");
        let _ = fs::remove_dir_all(directory);
    }

    #[test]
    fn validates_elf_machine() {
        let path = env::temp_dir().join(format!("cpa-whale-elf-{}", unix_time().unwrap()));
        let mut header = [0_u8; 20];
        header[..4].copy_from_slice(b"\x7fELF");
        header[4] = 2;
        header[5] = 1;
        let machine = match env::consts::ARCH {
            "x86_64" => 62_u16,
            "aarch64" => 183_u16,
            _ => return,
        };
        header[18..20].copy_from_slice(&machine.to_le_bytes());
        fs::write(&path, header).unwrap();
        validate_plugin_architecture(&path).unwrap();
        let _ = fs::remove_file(path);
    }
}
