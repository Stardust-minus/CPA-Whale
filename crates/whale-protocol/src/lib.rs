use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const API_SCHEMA_VERSION: u32 = 1;
pub const CAPABILITIES_SCHEMA_VERSION: u32 = 1;
pub const GLOBAL_SCOPE: &str = "global";
pub const GLOBAL_SCOPE_LABEL: &str = "CLIProxyAPI";

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct InstanceCapabilities {
    pub display_name: String,
    pub scope: String,
    pub scope_label: String,
    pub supports_user_attribution: bool,
    pub timezone: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FeatureCapabilities {
    pub pricing: bool,
    pub quota: bool,
    pub external_signals: bool,
    pub intelligence: bool,
    pub reset_events: bool,
    pub service_status: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelDescriptor {
    pub provider: String,
    pub model: String,
    pub display_name: String,
    pub reasoning_efforts: Vec<String>,
    pub priced: bool,
    pub has_intelligence: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PresentationDefaults {
    pub focus_model: Option<String>,
    pub focus_reasoning_effort: Option<String>,
    pub poll_interval_seconds: u64,
    pub cards: Vec<String>,
}

impl Default for PresentationDefaults {
    fn default() -> Self {
        Self {
            focus_model: None,
            focus_reasoning_effort: None,
            poll_interval_seconds: 60,
            cards: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct CapabilitiesResponse {
    pub schema_version: u32,
    pub plugin_version: String,
    pub minimum_client_version: String,
    pub instance: InstanceCapabilities,
    pub features: FeatureCapabilities,
    pub models: Vec<ModelDescriptor>,
    pub quota_providers: Vec<String>,
    pub defaults: PresentationDefaults,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TokenUsage {
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub reasoning_tokens: i64,
    pub cached_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub total_tokens: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageTotals {
    pub requests: i64,
    pub successful_requests: i64,
    pub failed_requests: i64,
    pub tokens: TokenUsage,
    pub estimated_usd_micros: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ModelUsage {
    pub model: String,
    pub reasoning_effort: Option<String>,
    pub provider: String,
    pub totals: UsageTotals,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct QuotaWindow {
    pub name: String,
    pub limit_name: Option<String>,
    pub used_percent: Option<f64>,
    pub remaining_percent: Option<f64>,
    pub window_minutes: Option<i64>,
    pub reset_after_seconds: Option<i64>,
    pub reset_at: Option<DateTime<Utc>>,
    pub allowed: Option<bool>,
    pub limit_reached: Option<bool>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct CreditSnapshot {
    pub has_credits: Option<bool>,
    pub unlimited: Option<bool>,
    pub balance: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct QuotaSnapshot {
    pub plan_type: Option<String>,
    pub active_limit: Option<String>,
    pub observed_at: Option<DateTime<Utc>>,
    pub windows: Vec<QuotaWindow>,
    pub credits: CreditSnapshot,
    pub source: String,
    pub available: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AccountSnapshot {
    pub auth_index: String,
    pub label: String,
    pub provider: String,
    pub status: String,
    pub unavailable: bool,
    pub totals: UsageTotals,
    pub quota: QuotaSnapshot,
    pub updated_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    ServiceStatus,
    Intelligence,
    ResetRisk,
    Changelog,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExternalSignal {
    pub id: String,
    pub kind: SignalKind,
    pub source: String,
    pub title: String,
    pub summary: String,
    pub model: Option<String>,
    pub reasoning_effort: Option<String>,
    pub value: Option<f64>,
    pub unit: Option<String>,
    pub confidence: String,
    pub source_timestamp: Option<DateTime<Utc>>,
    pub fetched_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub stale: bool,
    pub url: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PluginHealth {
    pub plugin_version: String,
    pub started_at: Option<DateTime<Utc>>,
    pub database_ok: bool,
    pub writer_ok: bool,
    pub queue_depth: usize,
    pub dropped_events: u64,
    pub last_event_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct GlobalSnapshot {
    pub schema_version: u32,
    pub scope: String,
    pub scope_label: String,
    pub supports_user_attribution: bool,
    pub epoch: String,
    pub sequence: u64,
    pub generated_at: DateTime<Utc>,
    pub reporting_day: String,
    pub timezone: String,
    pub pricing_version: Option<String>,
    pub all_time: UsageTotals,
    pub today: UsageTotals,
    pub models: Vec<ModelUsage>,
    pub accounts: Vec<AccountSnapshot>,
    pub signals: Vec<ExternalSignal>,
    pub health: PluginHealth,
}

impl GlobalSnapshot {
    pub fn empty(epoch: impl Into<String>, timezone: impl Into<String>) -> Self {
        Self {
            schema_version: API_SCHEMA_VERSION,
            scope: GLOBAL_SCOPE.to_string(),
            scope_label: GLOBAL_SCOPE_LABEL.to_string(),
            supports_user_attribution: false,
            epoch: epoch.into(),
            sequence: 0,
            generated_at: Utc::now(),
            reporting_day: String::new(),
            timezone: timezone.into(),
            pricing_version: None,
            all_time: UsageTotals::default(),
            today: UsageTotals::default(),
            models: Vec::new(),
            accounts: Vec::new(),
            signals: Vec::new(),
            health: PluginHealth::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ClientBaseline {
    pub epoch: String,
    pub sequence: u64,
    pub captured_at: DateTime<Utc>,
    pub totals: UsageTotals,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UsageDelta {
    pub compatible: bool,
    pub reason: Option<String>,
    pub elapsed_seconds: i64,
    pub from_sequence: u64,
    pub to_sequence: u64,
    pub totals: UsageTotals,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiError {
    pub error: String,
    pub message: String,
}
