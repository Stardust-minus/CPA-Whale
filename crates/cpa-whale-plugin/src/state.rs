use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, OnceLock};

use chrono::Utc;
use parking_lot::RwLock;
use serde_json::json;
use sha2::{Digest, Sha256};
use subtle::ConstantTimeEq;
use uuid::Uuid;
use whale_core::{parse_codex_quota, reporting_day, PriceCatalog};
use whale_protocol::{
    CapabilitiesResponse, FeatureCapabilities, GlobalSnapshot, InstanceCapabilities,
    ModelDescriptor, PluginHealth, PresentationDefaults, SignalKind, CAPABILITIES_SCHEMA_VERSION,
    GLOBAL_SCOPE,
};

use crate::aggregate::AggregateState;
use crate::auth::{sanitize_accounts, HostAuthListResponse};
use crate::config::{PluginConfig, QuotaAdapterConfig};
use crate::signals::SignalManager;
use crate::storage::{PersistedUsage, StorageHandle};
use crate::usage::SanitizedUsage;

static STATE: OnceLock<Arc<AppState>> = OnceLock::new();

pub struct AppState {
    config: RwLock<PluginConfig>,
    pricing: RwLock<PriceCatalog>,
    aggregate: RwLock<AggregateState>,
    storage: StorageHandle,
    signals: SignalManager,
    accepting: AtomicBool,
    last_auth_refresh_unix: AtomicI64,
}

impl AppState {
    pub fn initialize(config: PluginConfig) -> Result<Arc<Self>, String> {
        if let Some(state) = STATE.get() {
            state.reconfigure(config)?;
            return Ok(Arc::clone(state));
        }
        let pricing = config.price_catalog();
        let (storage, restored) = StorageHandle::open(&config)?;
        let aggregate = AggregateState::new(
            Uuid::new_v4().to_string(),
            config.timezone.clone(),
            &pricing,
            restored,
        );
        let signals = SignalManager::new(config.signals.clone())?;
        let state = Arc::new(Self {
            config: RwLock::new(config),
            pricing: RwLock::new(pricing),
            aggregate: RwLock::new(aggregate),
            storage,
            signals,
            accepting: AtomicBool::new(true),
            last_auth_refresh_unix: AtomicI64::new(0),
        });
        STATE
            .set(Arc::clone(&state))
            .map_err(|_| "plugin state was initialized concurrently".to_string())?;
        Ok(state)
    }

    pub fn reconfigure(&self, config: PluginConfig) -> Result<(), String> {
        let current = self.config.read();
        if current.database != config.database {
            return Err("changing database requires a plugin reload".into());
        }
        if current.timezone != config.timezone {
            return Err("changing timezone requires a plugin reload".into());
        }
        if current.queue_capacity != config.queue_capacity {
            return Err("changing queue-capacity requires a plugin reload".into());
        }
        if current.raw_events_retention_days != config.raw_events_retention_days
            || current.daily_retention_days != config.daily_retention_days
        {
            return Err("changing retention requires a plugin reload".into());
        }
        drop(current);
        *self.pricing.write() = config.price_catalog();
        self.signals.reconfigure(config.signals.clone());
        *self.config.write() = config;
        self.accepting.store(true, Ordering::Release);
        Ok(())
    }

    pub fn ingest(&self, mut usage: SanitizedUsage) -> bool {
        if !self.accepting.load(Ordering::Acquire) {
            return false;
        }
        let config = self.config.read().clone();
        let pricing = self.pricing.read().clone();
        usage.estimated_usd_micros = pricing.estimate(
            &usage.provider,
            &usage.model,
            &usage.alias,
            nonempty(&usage.reasoning_effort),
            &usage.tokens,
        );
        usage.pricing_version = usage
            .estimated_usd_micros
            .and_then(|_| nonempty(&pricing.version).map(ToOwned::to_owned));
        usage.quota = config
            .quota
            .adapter_for_provider(&usage.provider)
            .and_then(|adapter| normalize_quota(adapter, &usage));

        let mut aggregate = self.aggregate.write();
        let sequence = aggregate.next_sequence();
        let persisted = PersistedUsage {
            sequence,
            reporting_day: reporting_day(usage.requested_at, &config.timezone),
            usage: usage.clone(),
        };
        if !self.storage.try_enqueue(persisted) {
            return false;
        }
        aggregate.apply_usage(sequence, &mut usage, &pricing);
        true
    }

    pub fn snapshot(&self) -> GlobalSnapshot {
        let health_state = self.storage.health();
        let health = PluginHealth {
            plugin_version: crate::PLUGIN_VERSION.into(),
            database_ok: health_state.database_ok.load(Ordering::Relaxed),
            writer_ok: health_state.writer_ok.load(Ordering::Relaxed),
            queue_depth: self.storage.queue_depth(),
            dropped_events: health_state.dropped_events.load(Ordering::Relaxed),
            last_error: health_state.last_error.lock().clone(),
            ..PluginHealth::default()
        };
        let config = self.config.read().clone();
        let mut snapshot = self.aggregate.read().snapshot(health);
        snapshot.scope = GLOBAL_SCOPE.into();
        snapshot.scope_label = config.instance.scope_label.clone();
        snapshot.supports_user_attribution = config.instance.supports_user_attribution;
        snapshot
            .accounts
            .retain(|account| config.quota.account_visible(account));
        snapshot.signals = self.signals.current();
        snapshot
    }

    pub fn capabilities(&self) -> CapabilitiesResponse {
        let config = self.config.read().clone();
        let pricing = self.pricing.read().clone();
        let snapshot = self.snapshot();
        let mut models = BTreeMap::<(String, String), ModelDescriptor>::new();

        for model in &snapshot.models {
            let key = (model.provider.clone(), model.model.clone());
            let descriptor = models.entry(key).or_insert_with(|| ModelDescriptor {
                provider: model.provider.clone(),
                model: model.model.clone(),
                display_name: config.model_display_name(&model.model),
                reasoning_efforts: Vec::new(),
                priced: pricing.has_rate(&model.provider, &model.model),
                has_intelligence: false,
            });
            if let Some(effort) = model.reasoning_effort.as_deref() {
                push_unique(&mut descriptor.reasoning_efforts, effort);
            }
        }

        for rate in &config.pricing.rates {
            for model in rate.all_models() {
                let provider = rate.provider.clone().unwrap_or_default();
                let matching_key = models
                    .keys()
                    .find(|(candidate_provider, candidate_model)| {
                        candidate_model.eq_ignore_ascii_case(&model)
                            && (provider.is_empty()
                                || candidate_provider.eq_ignore_ascii_case(&provider))
                    })
                    .cloned();
                let key = matching_key.unwrap_or_else(|| (provider.clone(), model.clone()));
                let descriptor = models.entry(key).or_insert_with(|| ModelDescriptor {
                    provider,
                    model: model.clone(),
                    display_name: config.model_display_name(&model),
                    reasoning_efforts: Vec::new(),
                    priced: true,
                    has_intelligence: false,
                });
                descriptor.priced = true;
                if let Some(effort) = rate.reasoning_effort.as_deref() {
                    push_unique(&mut descriptor.reasoning_efforts, effort);
                }
            }
        }

        for signal in &snapshot.signals {
            if signal.kind != SignalKind::Intelligence {
                continue;
            }
            let Some(model) = signal.model.as_deref() else {
                continue;
            };
            let matching_key = models
                .keys()
                .find(|(_, candidate)| candidate.eq_ignore_ascii_case(model));
            let key = matching_key
                .cloned()
                .unwrap_or_else(|| (String::new(), model.into()));
            let descriptor = models.entry(key).or_insert_with(|| ModelDescriptor {
                provider: String::new(),
                model: model.into(),
                display_name: config.model_display_name(model),
                reasoning_efforts: Vec::new(),
                priced: false,
                has_intelligence: true,
            });
            descriptor.has_intelligence = true;
            if let Some(effort) = signal.reasoning_effort.as_deref() {
                push_unique(&mut descriptor.reasoning_efforts, effort);
            }
        }

        let quota_providers = config
            .quota
            .adapters
            .iter()
            .filter(|adapter| adapter.enabled)
            .flat_map(|adapter| adapter.providers.iter().cloned())
            .filter(|provider| provider != "*")
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        let source_adapters = config
            .signals
            .sources
            .iter()
            .filter(|source| config.signals.enabled && source.enabled)
            .map(|source| source.adapter.as_str())
            .collect::<BTreeSet<_>>();

        CapabilitiesResponse {
            schema_version: CAPABILITIES_SCHEMA_VERSION,
            plugin_version: crate::PLUGIN_VERSION.into(),
            minimum_client_version: "0.3.0".into(),
            instance: InstanceCapabilities {
                display_name: config.instance.display_name.clone(),
                scope: GLOBAL_SCOPE.into(),
                scope_label: config.instance.scope_label.clone(),
                supports_user_attribution: config.instance.supports_user_attribution,
                timezone: config.timezone.clone(),
            },
            features: FeatureCapabilities {
                pricing: !pricing.rates.is_empty(),
                quota: !config.quota.adapters.is_empty(),
                external_signals: config.signals.enabled && !source_adapters.is_empty(),
                intelligence: source_adapters.contains("codex-radar-intelligence"),
                reset_events: source_adapters.contains("divin-reset-events")
                    || source_adapters.contains("historical-risk-window"),
                service_status: source_adapters.contains("statuspage-v2"),
            },
            models: models.into_values().collect(),
            quota_providers,
            defaults: PresentationDefaults {
                focus_model: config.instance.focus_model.clone(),
                focus_reasoning_effort: config.instance.focus_reasoning_effort.clone(),
                poll_interval_seconds: config.instance.poll_interval_seconds,
                cards: config.instance.cards.clone(),
            },
        }
    }

    pub fn refresh_accounts_if_stale(&self) {
        let now = Utc::now().timestamp();
        let last = self.last_auth_refresh_unix.load(Ordering::Relaxed);
        if now.saturating_sub(last) < 60 {
            return;
        }
        if self
            .last_auth_refresh_unix
            .compare_exchange(last, now, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            return;
        }
        let result = crate::abi::call_host("host.auth.list", b"{}");
        match result.and_then(|raw| {
            serde_json::from_slice::<HostAuthListResponse>(&raw).map_err(|e| e.to_string())
        }) {
            Ok(response) => self
                .aggregate
                .write()
                .update_accounts(&sanitize_accounts(response.files)),
            Err(error) => {
                let health = self.storage.health();
                *health.last_error.lock() = Some(format!("refresh account inventory: {error}"));
            }
        }
    }

    pub fn authorize(&self, header: Option<&str>) -> Result<(), &'static str> {
        let config = self.config.read();
        if config.read_tokens.is_empty() {
            return Err("read token is not configured");
        }
        let token = header
            .and_then(|value| value.trim().strip_prefix("Bearer "))
            .or_else(|| header.and_then(|value| value.trim().strip_prefix("bearer ")))
            .ok_or("missing bearer token")?;
        let digest = Sha256::digest(token.as_bytes());
        let actual = hex_lower(&digest);
        let mut matched = 0_u8;
        for configured in &config.read_tokens {
            let expected = configured.sha256.trim().to_ascii_lowercase();
            matched |= actual.as_bytes().ct_eq(expected.as_bytes()).unwrap_u8();
        }
        if matched != 0 {
            Ok(())
        } else {
            Err("invalid bearer token")
        }
    }

    pub fn diagnostics(&self) -> serde_json::Value {
        let snapshot = self.snapshot();
        let config = self.config.read();
        json!({
            "plugin_version": crate::PLUGIN_VERSION,
            "schema_version": whale_protocol::API_SCHEMA_VERSION,
            "capabilities_schema_version": CAPABILITIES_SCHEMA_VERSION,
            "config_version": config.config_version,
            "enabled": config.enabled,
            "priority": config.priority,
            "epoch": snapshot.epoch,
            "sequence": snapshot.sequence,
            "reporting_day": snapshot.reporting_day,
            "timezone": snapshot.timezone,
            "database": config.database,
            "token_configured": !config.read_tokens.is_empty(),
            "read_token_ids": config.read_tokens.iter().map(|token| token.id.clone()).collect::<Vec<_>>(),
            "raw_events_retention_days": config.raw_events_retention_days,
            "daily_retention_days": config.daily_retention_days,
            "signal_sources": self.signals.diagnostics(),
            "health": snapshot.health,
        })
    }

    pub fn quiesce(&self) {
        self.accepting.store(false, Ordering::Release);
    }

    pub fn shutdown(&self) {
        self.quiesce();
        self.signals.shutdown();
        self.storage.shutdown();
    }
}

pub fn state() -> Result<Arc<AppState>, String> {
    STATE
        .get()
        .cloned()
        .ok_or_else(|| "plugin is not initialized".to_string())
}

pub fn shutdown_global() {
    if let Some(state) = STATE.get() {
        state.shutdown();
    }
}

fn normalize_quota(
    adapter: &QuotaAdapterConfig,
    usage: &SanitizedUsage,
) -> Option<whale_protocol::QuotaSnapshot> {
    match adapter.adapter.as_str() {
        "codex-response-headers" => {
            let quota = parse_codex_quota(&usage.response_headers, usage.requested_at);
            quota.available.then_some(quota)
        }
        _ => None,
    }
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(value))
    {
        values.push(value.to_string());
        values.sort_by_key(|item| item.to_ascii_lowercase());
    }
}

fn nonempty(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0f) as usize] as char);
    }
    out
}
