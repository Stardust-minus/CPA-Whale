use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};
use whale_core::{add_totals, reporting_day, PriceCatalog};
use whale_protocol::{
    AccountSnapshot, ExternalSignal, GlobalSnapshot, ModelUsage, PluginHealth, QuotaSnapshot,
    UsageTotals, API_SCHEMA_VERSION, GLOBAL_SCOPE, GLOBAL_SCOPE_LABEL,
};

use crate::auth::{deterministic_labels, SanitizedAccount};
use crate::storage::{RestoredAccounts, RestoredState};
use crate::usage::SanitizedUsage;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct ModelKey {
    provider: String,
    model: String,
    reasoning_effort: String,
}

#[derive(Debug, Clone)]
struct AccountRuntime {
    provider: String,
    status: String,
    unavailable: bool,
    totals: UsageTotals,
    quota: QuotaSnapshot,
    updated_at: Option<DateTime<Utc>>,
}

pub struct AggregateState {
    epoch: String,
    sequence: u64,
    started_at: DateTime<Utc>,
    timezone: String,
    reporting_day: String,
    pricing_version: Option<String>,
    all_time: UsageTotals,
    today: UsageTotals,
    models: BTreeMap<ModelKey, UsageTotals>,
    accounts: BTreeMap<String, AccountRuntime>,
    labels: HashMap<String, String>,
    signals: Vec<ExternalSignal>,
    last_event_at: Option<DateTime<Utc>>,
}

impl AggregateState {
    pub fn new(
        epoch: String,
        timezone: String,
        pricing: &PriceCatalog,
        restored: RestoredState,
    ) -> Self {
        Self {
            epoch,
            sequence: restored.sequence,
            started_at: Utc::now(),
            timezone,
            reporting_day: restored.reporting_day,
            pricing_version: nonempty(&pricing.version),
            all_time: restored.all_time,
            today: restored.today,
            models: restored_models(restored.models),
            accounts: restored_accounts(restored.accounts),
            labels: HashMap::new(),
            signals: Vec::new(),
            last_event_at: restored.last_event_at,
        }
    }

    pub fn next_sequence(&self) -> u64 {
        self.sequence.saturating_add(1)
    }

    pub fn apply_usage(
        &mut self,
        sequence: u64,
        usage: &mut SanitizedUsage,
        pricing: &PriceCatalog,
    ) {
        let day = reporting_day(usage.requested_at, &self.timezone);
        if day != self.reporting_day {
            self.reporting_day = day;
            self.today = UsageTotals::default();
            self.models.clear();
            for account in self.accounts.values_mut() {
                account.totals = UsageTotals::default();
            }
        }

        usage.estimated_usd_micros = pricing.estimate(
            &usage.provider,
            &usage.model,
            &usage.alias,
            nonempty_ref(&usage.reasoning_effort),
            &usage.tokens,
        );
        usage.pricing_version = usage
            .estimated_usd_micros
            .and_then(|_| nonempty(&pricing.version));
        self.pricing_version = nonempty(&pricing.version);

        let totals = UsageTotals {
            requests: 1,
            successful_requests: i64::from(!usage.failed),
            failed_requests: i64::from(usage.failed),
            tokens: usage.tokens.clone(),
            estimated_usd_micros: usage.estimated_usd_micros,
        };
        add_totals(&mut self.all_time, &totals);
        add_totals(&mut self.today, &totals);

        let model_key = ModelKey {
            provider: usage.provider.clone(),
            model: usage.model.clone(),
            reasoning_effort: usage.reasoning_effort.clone(),
        };
        add_totals(self.models.entry(model_key).or_default(), &totals);

        if !usage.auth_index.is_empty() {
            let account = self
                .accounts
                .entry(usage.auth_index.clone())
                .or_insert_with(|| AccountRuntime {
                    provider: usage.provider.clone(),
                    status: "unknown".into(),
                    unavailable: false,
                    totals: UsageTotals::default(),
                    quota: QuotaSnapshot::default(),
                    updated_at: None,
                });
            if account.provider == "unknown" && usage.provider != "unknown" {
                account.provider = usage.provider.clone();
            }
            add_totals(&mut account.totals, &totals);
            if let Some(quota) = usage.quota.clone() {
                account.quota = quota;
            }
            account.updated_at = Some(usage.requested_at);
        }

        self.sequence = sequence;
        self.last_event_at = Some(usage.requested_at);
    }

    pub fn update_accounts(&mut self, inventory: &[SanitizedAccount]) {
        self.labels = deterministic_labels(inventory);
        for item in inventory {
            let account = self
                .accounts
                .entry(item.auth_index.clone())
                .or_insert_with(|| AccountRuntime {
                    provider: item.provider.clone(),
                    status: item.status.clone(),
                    unavailable: item.unavailable,
                    totals: UsageTotals::default(),
                    quota: QuotaSnapshot::default(),
                    updated_at: None,
                });
            account.provider = item.provider.clone();
            account.status = item.status.clone();
            account.unavailable = item.unavailable;
        }
    }

    pub fn snapshot(&self, mut health: PluginHealth) -> GlobalSnapshot {
        health.started_at = Some(self.started_at);
        health.last_event_at = self.last_event_at;
        let models = self
            .models
            .iter()
            .map(|(key, totals)| ModelUsage {
                model: key.model.clone(),
                reasoning_effort: nonempty(&key.reasoning_effort),
                provider: key.provider.clone(),
                totals: totals.clone(),
            })
            .collect();
        let accounts = self
            .accounts
            .iter()
            .map(|(auth_index, account)| AccountSnapshot {
                auth_index: auth_index.clone(),
                label: self
                    .labels
                    .get(auth_index)
                    .cloned()
                    .unwrap_or_else(|| fallback_label(&account.provider, auth_index)),
                provider: account.provider.clone(),
                status: account.status.clone(),
                unavailable: account.unavailable,
                totals: account.totals.clone(),
                quota: account.quota.clone(),
                updated_at: account.updated_at,
            })
            .collect();
        GlobalSnapshot {
            schema_version: API_SCHEMA_VERSION,
            scope: GLOBAL_SCOPE.into(),
            scope_label: GLOBAL_SCOPE_LABEL.into(),
            supports_user_attribution: false,
            epoch: self.epoch.clone(),
            sequence: self.sequence,
            generated_at: Utc::now(),
            reporting_day: self.reporting_day.clone(),
            timezone: self.timezone.clone(),
            pricing_version: self.pricing_version.clone(),
            all_time: self.all_time.clone(),
            today: self.today.clone(),
            models,
            accounts,
            signals: self.signals.clone(),
            health,
        }
    }
}

fn fallback_label(provider: &str, auth_index: &str) -> String {
    let short = auth_index.chars().take(6).collect::<String>();
    format!("{} {}", title_case(provider), short)
}

fn title_case(value: &str) -> String {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Unknown".into(),
    }
}

fn nonempty(value: &str) -> Option<String> {
    nonempty_ref(value).map(ToOwned::to_owned)
}

fn nonempty_ref(value: &str) -> Option<&str> {
    let value = value.trim();
    (!value.is_empty()).then_some(value)
}

fn restored_models(
    values: BTreeMap<(String, String, String), UsageTotals>,
) -> BTreeMap<ModelKey, UsageTotals> {
    values
        .into_iter()
        .map(|((provider, model, reasoning_effort), totals)| {
            (
                ModelKey {
                    provider,
                    model,
                    reasoning_effort,
                },
                totals,
            )
        })
        .collect()
}

fn restored_accounts(values: RestoredAccounts) -> BTreeMap<String, AccountRuntime> {
    values
        .into_iter()
        .map(|(auth_index, (provider, totals, quota, updated_at))| {
            (
                auth_index,
                AccountRuntime {
                    provider,
                    status: "unknown".into(),
                    unavailable: false,
                    totals,
                    quota,
                    updated_at,
                },
            )
        })
        .collect()
}
