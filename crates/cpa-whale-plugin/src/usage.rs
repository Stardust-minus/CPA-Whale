use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::Deserialize;
use whale_protocol::{QuotaSnapshot, TokenUsage};

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CpaUsageRecord {
    #[serde(default)]
    pub provider: String,
    #[serde(default)]
    pub executor_type: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub alias: String,
    #[serde(default, rename = "APIKey")]
    pub _api_key: String,
    #[serde(default, rename = "AuthID")]
    pub _auth_id: String,
    #[serde(default)]
    pub auth_index: String,
    #[serde(default)]
    pub auth_type: String,
    #[serde(default, rename = "Source")]
    pub _source: String,
    #[serde(default)]
    pub reasoning_effort: String,
    #[serde(default)]
    pub service_tier: String,
    #[serde(default = "default_generate")]
    pub generate: bool,
    #[serde(default = "Utc::now")]
    pub requested_at: DateTime<Utc>,
    #[serde(default)]
    pub latency: i64,
    #[serde(default)]
    pub ttft: i64,
    #[serde(default)]
    pub failed: bool,
    #[serde(default)]
    pub failure: CpaFailure,
    #[serde(default)]
    pub detail: CpaUsageDetail,
    #[serde(default)]
    pub response_headers: HashMap<String, Vec<String>>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CpaFailure {
    #[serde(default)]
    pub status_code: i32,
    #[serde(default, rename = "Body")]
    pub _body: String,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct CpaUsageDetail {
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub reasoning_tokens: i64,
    #[serde(default)]
    pub cached_tokens: i64,
    #[serde(default)]
    pub cache_read_tokens: i64,
    #[serde(default)]
    pub cache_creation_tokens: i64,
    #[serde(default)]
    pub total_tokens: i64,
}

#[derive(Debug, Clone)]
pub struct SanitizedUsage {
    pub requested_at: DateTime<Utc>,
    pub provider: String,
    pub executor_type: String,
    pub model: String,
    pub alias: String,
    pub reasoning_effort: String,
    pub service_tier: String,
    pub auth_index: String,
    pub auth_type: String,
    pub failed: bool,
    pub status_code: i32,
    pub latency_ms: i64,
    pub ttft_ms: i64,
    pub tokens: TokenUsage,
    pub response_headers: HashMap<String, Vec<String>>,
    pub quota: Option<QuotaSnapshot>,
    pub estimated_usd_micros: Option<i64>,
    pub pricing_version: Option<String>,
}

impl CpaUsageRecord {
    pub fn sanitize(self) -> Option<SanitizedUsage> {
        if !self.generate {
            return None;
        }
        let model = normalized_or(self.model, "unknown");
        let provider = normalized_or(self.provider, "unknown");
        let alias = normalized_or(self.alias, &model);
        let tokens = TokenUsage {
            input_tokens: self.detail.input_tokens.max(0),
            output_tokens: self.detail.output_tokens.max(0),
            reasoning_tokens: self.detail.reasoning_tokens.max(0),
            cached_tokens: self.detail.cached_tokens.max(0),
            cache_read_tokens: self.detail.cache_read_tokens.max(0),
            cache_write_tokens: self.detail.cache_creation_tokens.max(0),
            total_tokens: self.detail.total_tokens.max(0),
        };
        Some(SanitizedUsage {
            requested_at: self.requested_at,
            provider,
            executor_type: normalized_or(self.executor_type, "unknown"),
            model,
            alias,
            reasoning_effort: self.reasoning_effort.trim().to_string(),
            service_tier: self.service_tier.trim().to_string(),
            auth_index: self.auth_index.trim().to_string(),
            auth_type: self.auth_type.trim().to_string(),
            failed: self.failed,
            status_code: if self.failure.status_code > 0 {
                self.failure.status_code
            } else if self.failed {
                500
            } else {
                200
            },
            latency_ms: nanos_to_millis(self.latency),
            ttft_ms: nanos_to_millis(self.ttft),
            tokens,
            response_headers: self.response_headers,
            quota: None,
            estimated_usd_micros: None,
            pricing_version: None,
        })
    }
}

fn default_generate() -> bool {
    true
}

fn normalized_or(value: String, fallback: &str) -> String {
    let value = value.trim();
    if value.is_empty() {
        fallback.to_string()
    } else {
        value.to_string()
    }
}

fn nanos_to_millis(value: i64) -> i64 {
    value.max(0) / 1_000_000
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_pascal_case_and_drops_sensitive_fields() {
        let record: CpaUsageRecord = serde_json::from_str(
            r#"{
              "Provider":"codex","Model":"gpt-5.6-sol","Alias":"sol",
              "APIKey":"shared-secret","AuthID":"private","AuthIndex":"abc",
              "ReasoningEffort":"xhigh","RequestedAt":"2026-08-30T00:00:00Z",
              "Latency":1500000000,"TTFT":250000000,"Failed":false,
              "Detail":{"InputTokens":10,"OutputTokens":5,"ReasoningTokens":2,"TotalTokens":15},
              "ResponseHeaders":{"X-Codex-Primary-Used-Percent":["42"]}
            }"#,
        )
        .unwrap();
        let safe = record.sanitize().unwrap();
        assert_eq!(safe.model, "gpt-5.6-sol");
        assert_eq!(safe.latency_ms, 1500);
        assert_eq!(safe.tokens.reasoning_tokens, 2);
        assert!(!format!("{safe:?}").contains("shared-secret"));
    }
}
