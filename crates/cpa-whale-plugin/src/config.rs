use std::collections::{BTreeMap, BTreeSet};

use serde::Deserialize;
use whale_core::{PriceCatalog, PriceRate};
use whale_protocol::AccountSnapshot;

pub const CONFIG_VERSION: u32 = 2;

fn default_database() -> String {
    "/var/lib/cliproxyapi/whale/metrics.db".to_string()
}

fn legacy_timezone() -> String {
    "Asia/Shanghai".to_string()
}

fn default_timezone() -> String {
    "UTC".to_string()
}

fn default_raw_retention_days() -> i64 {
    7
}

fn default_daily_retention_days() -> i64 {
    90
}

fn default_queue_capacity() -> usize {
    4096
}

fn default_poll_interval_seconds() -> u64 {
    60
}

fn default_signal_interval_seconds() -> u64 {
    300
}

fn default_cards() -> Vec<String> {
    [
        "today",
        "startup",
        "models",
        "quota",
        "intelligence",
        "reset",
        "service-status",
        "entertainment",
    ]
    .into_iter()
    .map(str::to_string)
    .collect()
}

#[derive(Debug, Clone)]
pub struct PluginConfig {
    pub config_version: u32,
    pub enabled: bool,
    pub priority: i64,
    pub database: String,
    pub timezone: String,
    pub read_tokens: Vec<ReadTokenConfig>,
    pub raw_events_retention_days: i64,
    pub daily_retention_days: i64,
    pub queue_capacity: usize,
    pub instance: InstanceConfig,
    pub pricing: PricingConfig,
    pub quota: QuotaConfig,
    pub signals: SignalsConfig,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            config_version: CONFIG_VERSION,
            enabled: false,
            priority: 0,
            database: default_database(),
            timezone: default_timezone(),
            read_tokens: Vec::new(),
            raw_events_retention_days: default_raw_retention_days(),
            daily_retention_days: default_daily_retention_days(),
            queue_capacity: default_queue_capacity(),
            instance: InstanceConfig::default(),
            pricing: PricingConfig::default(),
            quota: QuotaConfig::default(),
            signals: SignalsConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
struct PluginConfigDocument {
    config_version: Option<u32>,
    enabled: bool,
    priority: i64,
    database: Option<String>,
    timezone: Option<String>,
    read_token_sha256: Option<String>,
    raw_events_retention_days: Option<i64>,
    daily_retention_days: Option<i64>,
    queue_capacity: Option<usize>,
    storage: Option<StorageConfigDocument>,
    api: Option<ApiConfigDocument>,
    instance: Option<InstanceConfig>,
    pricing: PricingConfig,
    quota: Option<QuotaConfig>,
    signals: Option<SignalsConfig>,
    external_signals: Option<LegacyExternalSignalsConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
struct StorageConfigDocument {
    database: Option<String>,
    timezone: Option<String>,
    raw_events_retention_days: Option<i64>,
    daily_retention_days: Option<i64>,
    queue_capacity: Option<usize>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
struct ApiConfigDocument {
    read_tokens: Vec<ReadTokenConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct ReadTokenConfig {
    pub id: String,
    pub sha256: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct InstanceConfig {
    pub display_name: String,
    pub scope_label: String,
    pub supports_user_attribution: bool,
    pub focus_model: Option<String>,
    pub focus_reasoning_effort: Option<String>,
    pub poll_interval_seconds: u64,
    pub cards: Vec<String>,
    pub model_display_names: BTreeMap<String, String>,
}

impl Default for InstanceConfig {
    fn default() -> Self {
        Self {
            display_name: "CLIProxyAPI".into(),
            scope_label: "CLIProxyAPI".into(),
            supports_user_attribution: false,
            focus_model: None,
            focus_reasoning_effort: None,
            poll_interval_seconds: default_poll_interval_seconds(),
            cards: default_cards(),
            model_display_names: BTreeMap::new(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct QuotaConfig {
    pub adapters: Vec<QuotaAdapterConfig>,
    pub account_visibility: AccountVisibilityConfig,
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            adapters: vec![QuotaAdapterConfig::codex()],
            account_visibility: AccountVisibilityConfig::default(),
        }
    }
}

impl QuotaConfig {
    fn legacy() -> Self {
        Self {
            adapters: vec![QuotaAdapterConfig::codex()],
            account_visibility: AccountVisibilityConfig {
                require_available: false,
                ..AccountVisibilityConfig::default()
            },
        }
    }

    pub fn adapter_for_provider(&self, provider: &str) -> Option<&QuotaAdapterConfig> {
        self.adapters.iter().find(|adapter| {
            adapter.enabled
                && adapter
                    .providers
                    .iter()
                    .any(|candidate| candidate == "*" || candidate.eq_ignore_ascii_case(provider))
        })
    }

    pub fn account_visible(&self, account: &AccountSnapshot) -> bool {
        let policy = &self.account_visibility;
        if policy.require_available && !account.quota.available {
            return false;
        }
        if policy.exclude_unavailable_accounts && account.unavailable {
            return false;
        }
        matches_filter(&policy.include_providers, &account.provider)
            && matches_optional_filter(
                &policy.include_plan_types,
                account.quota.plan_type.as_deref(),
            )
            && matches_optional_filter(
                &policy.include_active_limits,
                account.quota.active_limit.as_deref(),
            )
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct QuotaAdapterConfig {
    pub adapter: String,
    pub enabled: bool,
    pub providers: Vec<String>,
}

impl Default for QuotaAdapterConfig {
    fn default() -> Self {
        Self::codex()
    }
}

impl QuotaAdapterConfig {
    fn codex() -> Self {
        Self {
            adapter: "codex-response-headers".into(),
            enabled: true,
            providers: vec!["codex".into()],
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct AccountVisibilityConfig {
    pub require_available: bool,
    pub exclude_unavailable_accounts: bool,
    pub include_providers: Vec<String>,
    pub include_plan_types: Vec<String>,
    pub include_active_limits: Vec<String>,
}

impl Default for AccountVisibilityConfig {
    fn default() -> Self {
        Self {
            require_available: true,
            exclude_unavailable_accounts: true,
            include_providers: Vec::new(),
            include_plan_types: Vec::new(),
            include_active_limits: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct SignalsConfig {
    pub enabled: bool,
    pub sources: Vec<SignalSourceConfig>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct SignalSourceConfig {
    pub id: String,
    pub enabled: bool,
    pub adapter: String,
    pub display_name: String,
    pub url: Option<String>,
    pub interval_seconds: u64,
    pub include_models: Vec<String>,
    pub tool_filter: Option<String>,
    pub timezone: Option<String>,
    pub risk_windows: Vec<RiskWindowConfig>,
    pub fallback_risk: Option<f64>,
    pub fallback_label: Option<String>,
}

impl Default for SignalSourceConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            enabled: true,
            adapter: String::new(),
            display_name: String::new(),
            url: None,
            interval_seconds: default_signal_interval_seconds(),
            include_models: Vec::new(),
            tool_filter: None,
            timezone: None,
            risk_windows: Vec::new(),
            fallback_risk: None,
            fallback_label: None,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct RiskWindowConfig {
    pub start_hour: u32,
    pub end_hour: u32,
    pub risk: f64,
    pub label: String,
}

impl Default for RiskWindowConfig {
    fn default() -> Self {
        Self {
            start_hour: 0,
            end_hour: 23,
            risk: 0.5,
            label: "一般".into(),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct PricingConfig {
    pub version: String,
    pub rates: Vec<PriceRateConfig>,
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
pub struct PriceRateConfig {
    pub provider: Option<String>,
    pub model: String,
    pub models: Vec<String>,
    pub aliases: Vec<String>,
    pub display_name: Option<String>,
    pub reasoning_effort: Option<String>,
    pub priority: i64,
    pub input_usd_per_million: Option<f64>,
    pub cache_read_usd_per_million: Option<f64>,
    pub cache_write_usd_per_million: Option<f64>,
    pub output_usd_per_million: Option<f64>,
    pub reasoning_usd_per_million: Option<f64>,
}

impl PriceRateConfig {
    pub fn all_models(&self) -> Vec<String> {
        let mut values = BTreeSet::new();
        for value in std::iter::once(&self.model)
            .chain(self.models.iter())
            .chain(self.aliases.iter())
        {
            let value = value.trim();
            if !value.is_empty() {
                values.insert(value.to_string());
            }
        }
        values.into_iter().collect()
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, rename_all = "kebab-case")]
struct LegacyExternalSignalsConfig {
    enabled: bool,
    openai_status_url: String,
    anthropic_status_url: String,
    intelligence_url: String,
    reset_events_url: String,
    status_interval_seconds: u64,
    intelligence_interval_seconds: u64,
    reset_interval_seconds: u64,
}

impl Default for LegacyExternalSignalsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            openai_status_url: "https://status.openai.com/api/v2/summary.json".into(),
            anthropic_status_url: "https://status.claude.com/api/v2/summary.json".into(),
            intelligence_url: "https://codex-reset-radar.pages.dev/api/radar-insights".into(),
            reset_events_url: "https://divin.cc/api/reset-events".into(),
            status_interval_seconds: 300,
            intelligence_interval_seconds: 600,
            reset_interval_seconds: 1800,
        }
    }
}

impl LegacyExternalSignalsConfig {
    fn into_signals(self) -> SignalsConfig {
        SignalsConfig {
            enabled: self.enabled,
            sources: vec![
                signal_source(
                    "openai-status",
                    "statuspage-v2",
                    "OpenAI",
                    Some(self.openai_status_url),
                    self.status_interval_seconds,
                ),
                signal_source(
                    "anthropic-status",
                    "statuspage-v2",
                    "Anthropic",
                    Some(self.anthropic_status_url),
                    self.status_interval_seconds,
                ),
                signal_source(
                    "community-intelligence",
                    "codex-radar-intelligence",
                    "AI 雷达 / 分布式众测",
                    Some(self.intelligence_url),
                    self.intelligence_interval_seconds,
                ),
                SignalSourceConfig {
                    tool_filter: Some("codex".into()),
                    ..signal_source(
                        "reset-events",
                        "divin-reset-events",
                        "divin.cc 第三方聚合",
                        Some(self.reset_events_url),
                        self.reset_interval_seconds,
                    )
                },
                SignalSourceConfig {
                    timezone: Some("Asia/Shanghai".into()),
                    risk_windows: vec![
                        risk_window(8, 8, 0.9, "高峰"),
                        risk_window(0, 7, 0.7, "偏高"),
                        risk_window(16, 22, 0.25, "相对较低"),
                    ],
                    fallback_risk: Some(0.45),
                    fallback_label: Some("一般".into()),
                    ..signal_source(
                        "historical-reset-risk",
                        "historical-risk-window",
                        "AI 雷达社区历史统计",
                        None,
                        300,
                    )
                },
            ],
        }
    }
}

impl PluginConfig {
    pub fn parse(yaml: &[u8]) -> Result<Self, String> {
        let document = serde_yaml::from_slice::<PluginConfigDocument>(yaml)
            .map_err(|error| format!("invalid plugin config: {error}"))?;
        let config_version = document.config_version.unwrap_or(1);
        if !(1..=CONFIG_VERSION).contains(&config_version) {
            return Err(format!("unsupported config-version: {config_version}"));
        }
        let legacy = config_version < CONFIG_VERSION;
        let storage = document.storage.unwrap_or_default();
        let api = document.api.unwrap_or_default();

        let mut read_tokens = api.read_tokens;
        if let Some(digest) = document
            .read_token_sha256
            .filter(|digest| !digest.trim().is_empty())
        {
            read_tokens.push(ReadTokenConfig {
                id: "legacy".into(),
                sha256: digest,
            });
        }

        let mut instance = document.instance.unwrap_or_default();
        if instance.cards.is_empty() {
            instance.cards = default_cards();
        }

        let config = Self {
            config_version,
            enabled: document.enabled,
            priority: document.priority,
            database: storage
                .database
                .or(document.database)
                .unwrap_or_else(default_database),
            timezone: storage
                .timezone
                .or(document.timezone)
                .unwrap_or_else(if legacy {
                    legacy_timezone
                } else {
                    default_timezone
                }),
            read_tokens,
            raw_events_retention_days: storage
                .raw_events_retention_days
                .or(document.raw_events_retention_days)
                .unwrap_or_else(default_raw_retention_days),
            daily_retention_days: storage
                .daily_retention_days
                .or(document.daily_retention_days)
                .unwrap_or_else(default_daily_retention_days),
            queue_capacity: storage
                .queue_capacity
                .or(document.queue_capacity)
                .unwrap_or_else(default_queue_capacity),
            instance,
            pricing: document.pricing,
            quota: document.quota.unwrap_or_else(|| {
                if legacy {
                    QuotaConfig::legacy()
                } else {
                    QuotaConfig::default()
                }
            }),
            signals: document.signals.unwrap_or_else(|| {
                if legacy {
                    document.external_signals.unwrap_or_default().into_signals()
                } else {
                    SignalsConfig::default()
                }
            }),
        };
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.database.trim().is_empty() {
            return Err("database path must not be empty".into());
        }
        if self.timezone.parse::<chrono_tz::Tz>().is_err() {
            return Err(format!("invalid timezone: {}", self.timezone));
        }
        if !(15..=3600).contains(&self.instance.poll_interval_seconds) {
            return Err("poll-interval-seconds must be between 15 and 3600".into());
        }
        validate_read_tokens(&self.read_tokens)?;
        if !(1..=1_000_000).contains(&self.queue_capacity) {
            return Err("queue-capacity must be between 1 and 1000000".into());
        }
        if !(1..=3650).contains(&self.raw_events_retention_days) {
            return Err("raw-events-retention-days must be between 1 and 3650".into());
        }
        if !(1..=3650).contains(&self.daily_retention_days) {
            return Err("daily-retention-days must be between 1 and 3650".into());
        }
        for adapter in &self.quota.adapters {
            if adapter.enabled && adapter.adapter != "codex-response-headers" {
                return Err(format!("unsupported quota adapter: {}", adapter.adapter));
            }
            if adapter.enabled && adapter.providers.is_empty() {
                return Err("enabled quota adapter must include at least one provider".into());
            }
        }
        validate_signals(&self.signals)?;
        for rate in &self.pricing.rates {
            if rate.all_models().is_empty() {
                return Err("pricing rate must include at least one model".into());
            }
            for value in [
                rate.input_usd_per_million,
                rate.cache_read_usd_per_million,
                rate.cache_write_usd_per_million,
                rate.output_usd_per_million,
                rate.reasoning_usd_per_million,
            ]
            .into_iter()
            .flatten()
            {
                if !value.is_finite() || !(0.0..=1_000_000.0).contains(&value) {
                    return Err("pricing values must be finite and between 0 and 1000000".into());
                }
            }
        }
        Ok(())
    }

    pub fn price_catalog(&self) -> PriceCatalog {
        PriceCatalog {
            version: self.pricing.version.clone(),
            rates: self
                .pricing
                .rates
                .iter()
                .map(|rate| PriceRate {
                    provider: rate
                        .provider
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned),
                    models: rate.all_models(),
                    reasoning_effort: rate
                        .reasoning_effort
                        .as_deref()
                        .map(str::trim)
                        .filter(|value| !value.is_empty())
                        .map(ToOwned::to_owned),
                    priority: rate.priority,
                    input_usd_micros_per_million: usd_to_micros(rate.input_usd_per_million),
                    cache_read_usd_micros_per_million: usd_to_micros(
                        rate.cache_read_usd_per_million,
                    ),
                    cache_write_usd_micros_per_million: usd_to_micros(
                        rate.cache_write_usd_per_million,
                    ),
                    output_usd_micros_per_million: usd_to_micros(rate.output_usd_per_million),
                    reasoning_usd_micros_per_million: usd_to_micros(rate.reasoning_usd_per_million),
                })
                .collect(),
        }
    }

    pub fn model_display_name(&self, model: &str) -> String {
        self.instance
            .model_display_names
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(model))
            .map(|(_, value)| value.clone())
            .or_else(|| {
                self.pricing.rates.iter().find_map(|rate| {
                    rate.all_models()
                        .iter()
                        .any(|candidate| candidate.eq_ignore_ascii_case(model))
                        .then(|| rate.display_name.clone())
                        .flatten()
                })
            })
            .unwrap_or_else(|| model.to_string())
    }
}

fn validate_read_tokens(tokens: &[ReadTokenConfig]) -> Result<(), String> {
    let mut ids = BTreeSet::new();
    for token in tokens {
        let id = token.id.trim();
        if id.is_empty() {
            return Err("read token id must not be empty".into());
        }
        if !ids.insert(id.to_ascii_lowercase()) {
            return Err(format!("duplicate read token id: {id}"));
        }
        let digest = token.sha256.trim();
        if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
            return Err(format!(
                "read token {id} sha256 must be 64 hexadecimal characters"
            ));
        }
    }
    Ok(())
}

fn validate_signals(signals: &SignalsConfig) -> Result<(), String> {
    let mut ids = BTreeSet::new();
    for source in &signals.sources {
        if !source.enabled {
            continue;
        }
        if source.id.trim().is_empty() || !ids.insert(source.id.to_ascii_lowercase()) {
            return Err("enabled signal source ids must be non-empty and unique".into());
        }
        if !(60..=86_400).contains(&source.interval_seconds) {
            return Err("signal intervals must be between 60 and 86400 seconds".into());
        }
        match source.adapter.as_str() {
            "statuspage-v2" | "codex-radar-intelligence" | "divin-reset-events" => {
                let url = source
                    .url
                    .as_deref()
                    .ok_or_else(|| format!("signal source {} requires a URL", source.id))?;
                if !(url.starts_with("https://") || url.starts_with("http://")) {
                    return Err(format!("signal source {} URL is invalid", source.id));
                }
            }
            "historical-risk-window" => {
                let timezone = source.timezone.as_deref().unwrap_or("UTC");
                if timezone.parse::<chrono_tz::Tz>().is_err() {
                    return Err(format!("signal source {} has invalid timezone", source.id));
                }
                for window in &source.risk_windows {
                    if window.start_hour > 23
                        || window.end_hour > 23
                        || !window.risk.is_finite()
                        || !(0.0..=1.0).contains(&window.risk)
                    {
                        return Err(format!(
                            "signal source {} has invalid risk window",
                            source.id
                        ));
                    }
                }
            }
            adapter => return Err(format!("unsupported signal adapter: {adapter}")),
        }
    }
    Ok(())
}

fn signal_source(
    id: &str,
    adapter: &str,
    display_name: &str,
    url: Option<String>,
    interval_seconds: u64,
) -> SignalSourceConfig {
    SignalSourceConfig {
        id: id.into(),
        adapter: adapter.into(),
        display_name: display_name.into(),
        url,
        interval_seconds,
        ..SignalSourceConfig::default()
    }
}

fn risk_window(start_hour: u32, end_hour: u32, risk: f64, label: &str) -> RiskWindowConfig {
    RiskWindowConfig {
        start_hour,
        end_hour,
        risk,
        label: label.into(),
    }
}

fn matches_filter(filter: &[String], value: &str) -> bool {
    filter.is_empty()
        || filter
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(value))
}

fn matches_optional_filter(filter: &[String], value: Option<&str>) -> bool {
    filter.is_empty() || value.is_some_and(|value| matches_filter(filter, value))
}

fn usd_to_micros(value: Option<f64>) -> i64 {
    value.unwrap_or(0.0).mul_add(1_000_000.0, 0.0).round() as i64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_legacy_hyphenated_config() {
        let config = PluginConfig::parse(
            br#"
enabled: true
database: /tmp/whale.db
timezone: Asia/Shanghai
read-token-sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
pricing:
  version: test
  rates:
    - model: gpt-example
      reasoning-effort: xhigh
      input-usd-per-million: 1.25
"#,
        )
        .unwrap();
        assert_eq!(config.config_version, 1);
        assert!(config.signals.enabled);
        assert_eq!(config.read_tokens[0].id, "legacy");
        assert!(!config.quota.account_visibility.require_available);
        assert_eq!(
            config.price_catalog().rates[0].input_usd_micros_per_million,
            1_250_000
        );
    }

    #[test]
    fn parses_v2_with_safe_defaults_and_multiple_tokens() {
        let config = PluginConfig::parse(
            br#"
config-version: 2
enabled: true
storage:
  database: /tmp/whale.db
api:
  read-tokens:
    - id: desktop
      sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
    - id: laptop
      sha256: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
instance:
  display-name: Home CPA
  focus-model: model-a
pricing:
  version: test
  rates:
    - provider: codex
      models: [model-a, model-a-latest]
      display-name: Model A
      input-usd-per-million: 1.0
"#,
        )
        .unwrap();
        assert_eq!(config.config_version, 2);
        assert_eq!(config.timezone, "UTC");
        assert!(!config.signals.enabled);
        assert_eq!(config.read_tokens.len(), 2);
        assert_eq!(config.model_display_name("model-a-latest"), "Model A");
        assert!(config.quota.account_visibility.require_available);
    }

    #[test]
    fn astra_flex_example_prices_cached_input_and_reasoning() {
        let yaml = format!(
            "config-version: 2\n{}",
            include_str!("../../../deploy/pricing-gpt-6-astra.example.yaml")
        );
        let config = PluginConfig::parse(yaml.as_bytes()).unwrap();
        assert!(!config.signals.enabled);
        assert_eq!(config.model_display_name("gpt-6-astra"), "Astra");
        let catalog = config.price_catalog();
        assert_eq!(catalog.rates.len(), 1);
        let rate = &catalog.rates[0];
        assert_eq!(rate.input_usd_micros_per_million, 5_000_000);
        assert_eq!(rate.cache_read_usd_micros_per_million, 500_000);
        assert_eq!(rate.cache_write_usd_micros_per_million, 5_000_000);
        assert_eq!(rate.output_usd_micros_per_million, 25_000_000);
        assert_eq!(rate.reasoning_usd_micros_per_million, 25_000_000);

        let tokens = whale_protocol::TokenUsage {
            input_tokens: 1_000_000,
            cache_read_tokens: 400_000,
            cache_write_tokens: 100_000,
            output_tokens: 500_000,
            reasoning_tokens: 200_000,
            total_tokens: 1_500_000,
            ..whale_protocol::TokenUsage::default()
        };
        for provider in ["codex", "openai"] {
            for effort in [None, Some("xhigh")] {
                assert!(catalog.has_rate(provider, "gpt-6-astra"));
                assert_eq!(
                    catalog.estimate(provider, "gpt-6-astra", "", effort, &tokens),
                    Some(15_700_000)
                );
            }
        }
        assert_eq!(
            catalog.estimate("codex", "upstream-id", "gpt-6-astra", None, &tokens),
            Some(15_700_000)
        );
        assert_eq!(
            catalog.estimate("codex", "gpt-5.6-sol", "", None, &tokens),
            None
        );
        assert_eq!(
            catalog.estimate("codex", "gpt-6-astra-other", "", None, &tokens),
            None
        );
    }

    #[test]
    fn rejects_duplicate_token_ids() {
        let error = PluginConfig::parse(
            br#"
config-version: 2
api:
  read-tokens:
    - id: desktop
      sha256: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
    - id: DESKTOP
      sha256: bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
"#,
        )
        .unwrap_err();
        assert!(error.contains("duplicate read token id"));
    }
}
