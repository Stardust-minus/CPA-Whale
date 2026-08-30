use std::collections::HashMap;

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::state::state;

const RESOURCE_BASE: &str = "/v0/resource/plugins/cpa-whale";
const DIAGNOSTICS_PATH: &str = "/v0/management/plugins/cpa-whale/diagnostics";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ManagementRequest {
    #[serde(default, rename = "Method")]
    pub _method: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub headers: Option<HashMap<String, Vec<String>>>,
    #[serde(default, rename = "Query")]
    pub _query: Option<HashMap<String, Vec<String>>>,
    #[serde(default, rename = "Body")]
    pub _body: Option<String>,
    #[serde(default, rename = "HostCallbackID")]
    pub _host_callback_id: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct ManagementResponse {
    status_code: u16,
    headers: HashMap<String, Vec<String>>,
    body: String,
}

pub fn registration_result() -> serde_json::Value {
    json!({
        "routes": [{
            "Method": "GET",
            "Path": "/plugins/cpa-whale/diagnostics",
            "Description": "CPA Whale operator diagnostics"
        }],
        "resources": [
            {"Path":"/v1/capabilities","Menu":"CPA Whale","Description":"Instance and feature capabilities"},
            {"Path":"/v1/snapshot","Menu":"CPA Whale","Description":"Global CPA Whale snapshot"},
            {"Path":"/v1/models","Menu":"CPA Whale","Description":"Global model totals"},
            {"Path":"/v1/accounts","Menu":"CPA Whale","Description":"Sanitized account and quota state"},
            {"Path":"/v1/signals","Menu":"CPA Whale","Description":"Cached external signals"},
            {"Path":"/v1/health","Menu":"CPA Whale","Description":"Plugin health"}
        ]
    })
}

pub fn handle(raw: &[u8]) -> Result<serde_json::Value, String> {
    let request = serde_json::from_slice::<ManagementRequest>(raw)
        .map_err(|error| format!("decode management request: {error}"))?;
    let app = state()?;
    if request.path == DIAGNOSTICS_PATH {
        return response(200, app.diagnostics());
    }
    let authorization = request
        .headers
        .as_ref()
        .and_then(|headers| header(headers, "authorization"));
    if let Err(message) = app.authorize(authorization) {
        let status = if message == "read token is not configured" {
            503
        } else {
            401
        };
        return response(status, json!({"error":"unauthorized","message":message}));
    }
    app.refresh_accounts_if_stale();
    let snapshot = app.snapshot();
    let value = match request.path.as_str() {
        path if path == format!("{RESOURCE_BASE}/v1/capabilities") => {
            serde_json::to_value(app.capabilities())
        }
        path if path == format!("{RESOURCE_BASE}/v1/snapshot") => serde_json::to_value(snapshot),
        path if path == format!("{RESOURCE_BASE}/v1/models") => {
            serde_json::to_value(snapshot.models)
        }
        path if path == format!("{RESOURCE_BASE}/v1/accounts") => {
            serde_json::to_value(snapshot.accounts)
        }
        path if path == format!("{RESOURCE_BASE}/v1/signals") => {
            serde_json::to_value(snapshot.signals)
        }
        path if path == format!("{RESOURCE_BASE}/v1/health") => {
            serde_json::to_value(snapshot.health)
        }
        _ => {
            return response(
                404,
                json!({"error":"not_found","message":"route not found"}),
            )
        }
    }
    .map_err(|error| format!("encode API response: {error}"))?;
    response(200, value)
}

fn response(status_code: u16, value: serde_json::Value) -> Result<serde_json::Value, String> {
    let raw =
        serde_json::to_vec(&value).map_err(|error| format!("encode response body: {error}"))?;
    let mut headers = HashMap::new();
    headers.insert(
        "content-type".into(),
        vec!["application/json; charset=utf-8".into()],
    );
    headers.insert("cache-control".into(), vec!["no-store".into()]);
    serde_json::to_value(ManagementResponse {
        status_code,
        headers,
        body: STANDARD.encode(raw),
    })
    .map_err(|error| format!("encode management response: {error}"))
}

fn header<'a>(headers: &'a HashMap<String, Vec<String>>, wanted: &str) -> Option<&'a str> {
    headers
        .iter()
        .find(|(name, _)| name.eq_ignore_ascii_case(wanted))
        .and_then(|(_, values)| values.first())
        .map(String::as_str)
}
