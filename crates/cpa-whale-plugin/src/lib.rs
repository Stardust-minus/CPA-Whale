mod abi;
mod aggregate;
mod api;
mod auth;
mod config;
mod signals;
mod state;
mod storage;
mod usage;

use std::ffi::c_void;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::Deserialize;
use serde_json::json;

use abi::{CliproxyHostApi, CliproxyPluginApi, ABI_VERSION};
use config::PluginConfig;
use state::{shutdown_global, state, AppState};
use usage::CpaUsageRecord;

pub const PLUGIN_VERSION: &str = env!("CARGO_PKG_VERSION");
const SCHEMA_VERSION: u32 = 4;

#[derive(Debug, Deserialize)]
struct LifecycleRequest {
    #[serde(default)]
    config_yaml: String,
    #[allow(dead_code)]
    #[serde(default)]
    schema_version: u32,
}

/// Initializes the trusted CPA dynamic plugin ABI.
///
/// # Safety
/// `host` and `plugin` must be valid pointers supplied by CLIProxyAPI for ABI version 1,
/// and `plugin` must remain writable for the duration of this call.
#[no_mangle]
pub unsafe extern "C" fn cliproxy_plugin_init(
    host: *const CliproxyHostApi,
    plugin: *mut CliproxyPluginApi,
) -> i32 {
    if plugin.is_null() {
        return 1;
    }
    if abi::install_host(host).is_err() {
        return 1;
    }
    (*plugin).abi_version = ABI_VERSION;
    (*plugin).call = Some(abi::plugin_call);
    (*plugin).free_buffer = Some(abi::plugin_free);
    (*plugin).shutdown = Some(abi::plugin_shutdown);
    0
}

pub(crate) fn dispatch(method: &str, request: &[u8]) -> Result<Vec<u8>, String> {
    let result = match method {
        "plugin.register" | "plugin.reconfigure" => register(request)?,
        "plugin.quiesce" => {
            if let Ok(state) = state() {
                state.quiesce();
            }
            json!({})
        }
        "plugin.shutdown" => {
            shutdown_global();
            json!({})
        }
        "usage.handle" => {
            let record = serde_json::from_slice::<CpaUsageRecord>(request)
                .map_err(|error| format!("decode usage record: {error}"))?;
            let accepted = record
                .sanitize()
                .map(|usage| state().map(|state| state.ingest(usage)))
                .transpose()?
                .unwrap_or(false);
            json!({"accepted":accepted})
        }
        "management.register" => api::registration_result(),
        "management.handle" => api::handle(request)?,
        _ => return Ok(error_envelope("unknown_method", "unknown method")),
    };
    ok_envelope(result)
}

fn register(request: &[u8]) -> Result<serde_json::Value, String> {
    let request = serde_json::from_slice::<LifecycleRequest>(request)
        .map_err(|error| format!("decode lifecycle request: {error}"))?;
    let yaml = if request.config_yaml.is_empty() {
        Vec::new()
    } else {
        STANDARD
            .decode(request.config_yaml)
            .map_err(|error| format!("decode config_yaml: {error}"))?
    };
    let config = PluginConfig::parse(&yaml)?;
    AppState::initialize(config)?;
    Ok(json!({
        "schema_version": SCHEMA_VERSION,
        "metadata": {
            "Name": "CPA Whale",
            "Version": PLUGIN_VERSION,
            "Author": "CPA Whale contributors",
            "GitHubRepository": "https://github.com/Stardust-minus/CPA-Whale",
            "Logo": "",
            "ConfigFields": [
                {"Name":"config-version","Type":"integer","Description":"CPA Whale configuration schema"},
                {"Name":"storage.database","Type":"string","Description":"SQLite database path"},
                {"Name":"storage.timezone","Type":"string","Description":"Reporting timezone"},
                {"Name":"api.read-tokens","Type":"array","Description":"Named SHA-256 read-token digests"},
                {"Name":"storage.raw-events-retention-days","Type":"integer","Description":"Raw usage retention"},
                {"Name":"storage.daily-retention-days","Type":"integer","Description":"Daily rollup retention"}
            ]
        },
        "capabilities": {
            "usage_plugin": true,
            "management_api": true
        }
    }))
}

fn ok_envelope(value: serde_json::Value) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&json!({"ok":true,"result":value}))
        .map_err(|error| format!("encode plugin response: {error}"))
}

pub(crate) fn error_envelope(code: &str, message: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "ok": false,
        "error": {"code":code,"message":message}
    }))
    .unwrap_or_else(|_| b"{\"ok\":false}".to_vec())
}

pub(crate) fn shutdown() {
    shutdown_global();
}

#[allow(dead_code)]
fn _assert_ffi_types(_: *mut c_void) {}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;
    use serde_json::Value;
    use sha2::{Digest, Sha256};
    use std::time::{SystemTime, UNIX_EPOCH};

    #[test]
    fn dispatches_usage_and_serves_an_authenticated_snapshot() {
        let suffix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let database = std::env::temp_dir().join(format!("cpa-whale-dispatch-{suffix}.db"));
        let token = "test-whale-token";
        let digest = Sha256::digest(token.as_bytes());
        let digest_hex = digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let yaml = format!(
            "config-version: 2\nenabled: true\nstorage:\n  database: {}\n  timezone: Asia/Shanghai\n  queue-capacity: 16\napi:\n  read-tokens:\n    - id: test-client\n      sha256: {}\npricing:\n  version: test\n  rates:\n    - models: [gpt-5.6-sol]\n      input-usd-per-million: 1.0\nsignals:\n  enabled: false\n",
            database.display(),
            digest_hex
        );
        let lifecycle = serde_json::to_vec(&json!({
            "config_yaml": STANDARD.encode(yaml),
            "schema_version": 4
        }))
        .unwrap();
        let registration = dispatch("plugin.register", &lifecycle).unwrap();
        let registration: Value = serde_json::from_slice(&registration).unwrap();
        assert_eq!(registration["ok"], true);

        let usage = include_bytes!("../../../tests/fixtures/usage-codex.json");
        let handled: Value =
            serde_json::from_slice(&dispatch("usage.handle", usage).unwrap()).unwrap();
        assert_eq!(handled["result"]["accepted"], true);

        let request = serde_json::to_vec(&json!({
            "Method": "GET",
            "Path": "/v0/resource/plugins/cpa-whale/v1/snapshot",
            "Headers": {"Authorization":[format!("Bearer {token}")]},
            "Query": {},
            "Body": "",
            "HostCallbackID": ""
        }))
        .unwrap();
        let response: Value =
            serde_json::from_slice(&dispatch("management.handle", &request).unwrap()).unwrap();
        let body_b64 = response["result"]["Body"].as_str().unwrap();
        let body = STANDARD.decode(body_b64).unwrap();
        let snapshot: whale_protocol::GlobalSnapshot = serde_json::from_slice(&body).unwrap();
        assert_eq!(snapshot.scope, whale_protocol::GLOBAL_SCOPE);
        assert_eq!(snapshot.today.tokens.total_tokens, 1500);
        assert_eq!(snapshot.models[0].model, "gpt-5.6-sol");
        assert!(snapshot.accounts[0].quota.available);
        assert!(!String::from_utf8_lossy(&body).contains("must-not-be-persisted"));

        let capabilities_request = serde_json::to_vec(&json!({
            "Method": "GET",
            "Path": "/v0/resource/plugins/cpa-whale/v1/capabilities",
            "Headers": {"Authorization":[format!("Bearer {token}")]},
            "Query": {},
            "Body": "",
            "HostCallbackID": ""
        }))
        .unwrap();
        let response: Value =
            serde_json::from_slice(&dispatch("management.handle", &capabilities_request).unwrap())
                .unwrap();
        let capabilities = STANDARD
            .decode(response["result"]["Body"].as_str().unwrap())
            .unwrap();
        let capabilities: whale_protocol::CapabilitiesResponse =
            serde_json::from_slice(&capabilities).unwrap();
        assert_eq!(
            capabilities.schema_version,
            whale_protocol::CAPABILITIES_SCHEMA_VERSION
        );
        assert_eq!(
            capabilities
                .models
                .iter()
                .filter(|model| model.model == "gpt-5.6-sol")
                .count(),
            1
        );

        shutdown();
        let _ = std::fs::remove_file(database);
    }
}
