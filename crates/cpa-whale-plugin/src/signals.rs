use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD;
use base64::Engine;
use chrono::{DateTime, Timelike, Utc};
use crossbeam_channel::{bounded, Receiver, Sender};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use serde_json::json;
use whale_protocol::{ExternalSignal, SignalKind};

use crate::config::{RiskWindowConfig, SignalSourceConfig, SignalsConfig};

#[derive(Debug, Clone, Default, Serialize)]
pub struct SignalSourceHealth {
    pub last_attempt_at: Option<DateTime<Utc>>,
    pub last_success_at: Option<DateTime<Utc>>,
    pub last_error: Option<String>,
}

pub struct SignalManager {
    config: Arc<RwLock<SignalsConfig>>,
    signals: Arc<RwLock<Vec<ExternalSignal>>>,
    health: Arc<RwLock<BTreeMap<String, SignalSourceHealth>>>,
    stop: Sender<()>,
    join: parking_lot::Mutex<Option<JoinHandle<()>>>,
}

impl SignalManager {
    pub fn new(config: SignalsConfig) -> Result<Self, String> {
        let config = Arc::new(RwLock::new(config));
        let signals = Arc::new(RwLock::new(Vec::new()));
        let health = Arc::new(RwLock::new(BTreeMap::new()));
        let (stop, receiver) = bounded(1);
        let worker_config = Arc::clone(&config);
        let worker_signals = Arc::clone(&signals);
        let worker_health = Arc::clone(&health);
        let join = thread::Builder::new()
            .name("cpa-whale-signals".into())
            .spawn(move || worker_loop(worker_config, worker_signals, worker_health, receiver))
            .map_err(|error| format!("start external signal worker: {error}"))?;
        Ok(Self {
            config,
            signals,
            health,
            stop,
            join: parking_lot::Mutex::new(Some(join)),
        })
    }

    pub fn reconfigure(&self, config: SignalsConfig) {
        let enabled_ids = config
            .sources
            .iter()
            .filter(|source| config.enabled && source.enabled)
            .map(|source| source.id.clone())
            .collect::<Vec<_>>();
        self.signals.write().retain(|signal| {
            enabled_ids
                .iter()
                .any(|id| signal.id.starts_with(&format!("{id}:")))
        });
        self.health
            .write()
            .retain(|id, _| enabled_ids.iter().any(|enabled| enabled == id));
        *self.config.write() = config;
    }

    pub fn current(&self) -> Vec<ExternalSignal> {
        let now = Utc::now();
        self.signals
            .read()
            .iter()
            .cloned()
            .map(|mut signal| {
                signal.stale = now > signal.expires_at;
                signal
            })
            .collect()
    }

    pub fn diagnostics(&self) -> serde_json::Value {
        serde_json::to_value(self.health.read().clone()).unwrap_or_else(|_| json!({}))
    }

    pub fn shutdown(&self) {
        let _ = self.stop.try_send(());
        if let Some(join) = self.join.lock().take() {
            let _ = join.join();
        }
    }
}

fn worker_loop(
    config: Arc<RwLock<SignalsConfig>>,
    signals: Arc<RwLock<Vec<ExternalSignal>>>,
    health: Arc<RwLock<BTreeMap<String, SignalSourceHealth>>>,
    stop: Receiver<()>,
) {
    let mut last_attempt = HashMap::<String, Instant>::new();
    loop {
        let cfg = config.read().clone();
        if cfg.enabled {
            for source in cfg.sources.iter().filter(|source| source.enabled) {
                if !due(
                    last_attempt.get(&source.id).copied(),
                    source.interval_seconds,
                ) {
                    continue;
                }
                last_attempt.insert(source.id.clone(), Instant::now());
                let attempted_at = Utc::now();
                let result = refresh_source(source, attempted_at);
                let mut source_health = health.write();
                let entry = source_health.entry(source.id.clone()).or_default();
                entry.last_attempt_at = Some(attempted_at);
                match result {
                    Ok(next) => {
                        replace_source(&signals, &source.id, next);
                        entry.last_success_at = Some(attempted_at);
                        entry.last_error = None;
                    }
                    Err(error) => entry.last_error = Some(error),
                }
            }
        } else {
            signals.write().clear();
        }
        if stop.recv_timeout(Duration::from_secs(30)).is_ok() {
            break;
        }
    }
}

fn due(last: Option<Instant>, seconds: u64) -> bool {
    last.map(|last| last.elapsed() >= Duration::from_secs(seconds))
        .unwrap_or(true)
}

fn refresh_source(
    source: &SignalSourceConfig,
    fetched_at: DateTime<Utc>,
) -> Result<Vec<ExternalSignal>, String> {
    match source.adapter.as_str() {
        "statuspage-v2" => {
            let url = required_url(source)?;
            let raw = host_get(url)?;
            parse_official_status(&raw, source, fetched_at).map(|signal| vec![signal])
        }
        "codex-radar-intelligence" => {
            let url = required_url(source)?;
            let raw = host_get(url)?;
            parse_intelligence(&raw, source, fetched_at)
        }
        "divin-reset-events" => {
            let url = required_url(source)?;
            let raw = host_get(url)?;
            parse_reset_events(&raw, source, fetched_at)
        }
        "historical-risk-window" => Ok(vec![historical_risk_signal(source, fetched_at)?]),
        adapter => Err(format!("unsupported signal adapter: {adapter}")),
    }
}

fn required_url(source: &SignalSourceConfig) -> Result<&str, String> {
    source
        .url
        .as_deref()
        .ok_or_else(|| format!("signal source {} has no URL", source.id))
}

fn replace_source(
    signals: &Arc<RwLock<Vec<ExternalSignal>>>,
    source_id: &str,
    mut next: Vec<ExternalSignal>,
) {
    let prefix = format!("{source_id}:");
    let mut current = signals.write();
    current.retain(|signal| !signal.id.starts_with(&prefix));
    current.append(&mut next);
    current.sort_by(|left, right| left.id.cmp(&right.id));
}

fn host_get(url: &str) -> Result<Vec<u8>, String> {
    let request = serde_json::to_vec(&json!({
        "method": "GET",
        "url": url,
        "headers": {
            "Accept": ["application/json"],
            "User-Agent": [format!("CPA-Whale/{}", crate::PLUGIN_VERSION)]
        }
    }))
    .map_err(|error| error.to_string())?;
    let raw = crate::abi::call_host("host.http.do", &request)?;
    let response = serde_json::from_slice::<HostHttpResponse>(&raw)
        .map_err(|error| format!("decode host HTTP response: {error}"))?;
    if !(200..300).contains(&response.status_code) {
        return Err(format!("GET {url} returned {}", response.status_code));
    }
    let body = STANDARD
        .decode(response.body)
        .map_err(|error| format!("decode host HTTP body: {error}"))?;
    if body.len() > 4 * 1024 * 1024 {
        return Err(format!("GET {url} exceeded 4 MiB"));
    }
    Ok(body)
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct HostHttpResponse {
    status_code: i32,
    #[serde(default)]
    body: String,
}

#[derive(Deserialize)]
struct StatusSummary {
    status: StatusValue,
    #[serde(default)]
    page: StatusPage,
}

#[derive(Deserialize)]
struct StatusValue {
    indicator: String,
    description: String,
}

#[derive(Default, Deserialize)]
struct StatusPage {
    #[serde(default)]
    updated_at: Option<DateTime<Utc>>,
}

fn parse_official_status(
    raw: &[u8],
    source: &SignalSourceConfig,
    fetched_at: DateTime<Utc>,
) -> Result<ExternalSignal, String> {
    let parsed = serde_json::from_slice::<StatusSummary>(raw)
        .map_err(|error| format!("decode {} status: {error}", source.display_name))?;
    let normal = parsed.status.indicator.eq_ignore_ascii_case("none");
    Ok(ExternalSignal {
        id: format!("{}:status", source.id),
        kind: SignalKind::ServiceStatus,
        source: source.display_name.clone(),
        title: if normal {
            "Operational".into()
        } else {
            parsed.status.indicator
        },
        summary: parsed.status.description,
        model: None,
        reasoning_effort: None,
        value: Some(if normal { 1.0 } else { 0.0 }),
        unit: Some("operational".into()),
        confidence: "official".into(),
        source_timestamp: parsed.page.updated_at,
        fetched_at,
        expires_at: expires_at(source, fetched_at),
        stale: false,
        url: source.url.clone(),
    })
}

#[derive(Deserialize)]
struct IntelligenceResponse {
    #[serde(default)]
    source_updated_at: Option<DateTime<Utc>>,
    #[serde(default)]
    comprehensive_points: Vec<IntelligencePoint>,
}

#[derive(Deserialize)]
struct IntelligencePoint {
    model: String,
    effort: String,
    iq: Option<f64>,
    software_iq: Option<f64>,
    visual_iq: Option<f64>,
    samples: Option<i64>,
}

fn parse_intelligence(
    raw: &[u8],
    source: &SignalSourceConfig,
    fetched_at: DateTime<Utc>,
) -> Result<Vec<ExternalSignal>, String> {
    let parsed = serde_json::from_slice::<IntelligenceResponse>(raw)
        .map_err(|error| format!("decode intelligence metrics: {error}"))?;
    Ok(parsed
        .comprehensive_points
        .into_iter()
        .filter(|point| model_included(&source.include_models, &point.model))
        .filter_map(|point| {
            let iq = point.iq?;
            Some(ExternalSignal {
                id: format!("{}:{}:{}", source.id, point.model, point.effort),
                kind: SignalKind::Intelligence,
                source: source.display_name.clone(),
                title: format!("{} {}", point.model, point.effort),
                summary: format!(
                    "software {} · visual {} · samples {}",
                    format_optional(point.software_iq),
                    format_optional(point.visual_iq),
                    point.samples.unwrap_or(0)
                ),
                model: Some(point.model),
                reasoning_effort: Some(point.effort),
                value: Some(iq),
                unit: Some("community_iq".into()),
                confidence: "community_benchmark".into(),
                source_timestamp: parsed.source_updated_at,
                fetched_at,
                expires_at: expires_at(source, fetched_at),
                stale: false,
                url: source.url.clone(),
            })
        })
        .collect())
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResetEvent {
    id: String,
    title: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    tool: String,
    #[serde(default)]
    credibility: String,
    #[serde(default)]
    sort_time: Option<DateTime<Utc>>,
    #[serde(default)]
    source_url: Option<String>,
}

fn parse_reset_events(
    raw: &[u8],
    source: &SignalSourceConfig,
    fetched_at: DateTime<Utc>,
) -> Result<Vec<ExternalSignal>, String> {
    let events = serde_json::from_slice::<Vec<ResetEvent>>(raw)
        .map_err(|error| format!("decode reset events: {error}"))?;
    Ok(events
        .into_iter()
        .filter(|event| {
            source
                .tool_filter
                .as_deref()
                .map(|tool| event.tool.eq_ignore_ascii_case(tool))
                .unwrap_or(true)
        })
        .take(3)
        .map(|event| ExternalSignal {
            id: format!("{}:{}", source.id, event.id),
            kind: SignalKind::ResetRisk,
            source: source.display_name.clone(),
            title: event.title,
            summary: event.summary,
            model: None,
            reasoning_effort: None,
            value: None,
            unit: None,
            confidence: if event.credibility.is_empty() {
                "third_party".into()
            } else {
                event.credibility
            },
            source_timestamp: event.sort_time,
            fetched_at,
            expires_at: expires_at(source, fetched_at),
            stale: false,
            url: event.source_url.or_else(|| source.url.clone()),
        })
        .collect())
}

fn historical_risk_signal(
    source: &SignalSourceConfig,
    fetched_at: DateTime<Utc>,
) -> Result<ExternalSignal, String> {
    let timezone = source.timezone.as_deref().unwrap_or("UTC");
    let timezone = timezone
        .parse::<chrono_tz::Tz>()
        .map_err(|error| format!("invalid historical risk timezone: {error}"))?;
    let hour = fetched_at.with_timezone(&timezone).hour();
    let selected = source
        .risk_windows
        .iter()
        .find(|window| hour_in_window(hour, window));
    let risk = selected
        .map(|window| window.risk)
        .or(source.fallback_risk)
        .unwrap_or(0.5);
    let label = selected
        .map(|window| window.label.clone())
        .or_else(|| source.fallback_label.clone())
        .unwrap_or_else(|| "reference".into());
    Ok(ExternalSignal {
        id: format!("{}:risk-window", source.id),
        kind: SignalKind::ResetRisk,
        source: source.display_name.clone(),
        title: label,
        summary: format!("Historical reference window in {timezone}; not an official prediction."),
        model: None,
        reasoning_effort: None,
        value: Some(risk),
        unit: Some("risk_index".into()),
        confidence: "historical_reference".into(),
        source_timestamp: None,
        fetched_at,
        expires_at: expires_at(source, fetched_at),
        stale: false,
        url: source.url.clone(),
    })
}

fn hour_in_window(hour: u32, window: &RiskWindowConfig) -> bool {
    if window.start_hour <= window.end_hour {
        (window.start_hour..=window.end_hour).contains(&hour)
    } else {
        hour >= window.start_hour || hour <= window.end_hour
    }
}

fn model_included(include: &[String], model: &str) -> bool {
    include.is_empty()
        || include
            .iter()
            .any(|candidate| candidate == "*" || candidate.eq_ignore_ascii_case(model))
}

fn expires_at(source: &SignalSourceConfig, fetched_at: DateTime<Utc>) -> DateTime<Utc> {
    fetched_at
        + chrono::Duration::seconds(
            source
                .interval_seconds
                .saturating_mul(2)
                .min(i64::MAX as u64) as i64,
        )
}

fn format_optional(value: Option<f64>) -> String {
    value
        .map(|value| format!("{value:.2}"))
        .unwrap_or_else(|| "--".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(adapter: &str) -> SignalSourceConfig {
        SignalSourceConfig {
            id: "test-source".into(),
            adapter: adapter.into(),
            display_name: "Test Source".into(),
            url: Some("https://example.invalid/data".into()),
            interval_seconds: 300,
            ..SignalSourceConfig::default()
        }
    }

    #[test]
    fn parses_all_models_unless_filtered() {
        let all = parse_intelligence(
            include_bytes!("../../../tests/fixtures/radar-intelligence.json"),
            &source("codex-radar-intelligence"),
            Utc::now(),
        )
        .unwrap();
        assert!(all.iter().any(|signal| {
            signal.model.as_deref() == Some("gpt-5.6-sol")
                && signal.reasoning_effort.as_deref() == Some("xhigh")
                && signal.value == Some(100.55)
        }));

        let mut filtered = source("codex-radar-intelligence");
        filtered.include_models = vec!["missing".into()];
        assert!(parse_intelligence(
            include_bytes!("../../../tests/fixtures/radar-intelligence.json"),
            &filtered,
            Utc::now(),
        )
        .unwrap()
        .is_empty());
    }

    #[test]
    fn reset_tool_filter_is_configurable() {
        let mut source = source("divin-reset-events");
        source.tool_filter = Some("codex".into());
        let signals = parse_reset_events(
            include_bytes!("../../../tests/fixtures/divin-reset-events.json"),
            &source,
            Utc::now(),
        )
        .unwrap();
        assert!(!signals.is_empty());
        assert!(signals.iter().all(|signal| signal.source == "Test Source"));
    }

    #[test]
    fn parses_standard_statuspage_payload() {
        let raw = br#"{"status":{"indicator":"none","description":"All Systems Operational"},"page":{"updated_at":"2026-08-30T00:00:00Z"}}"#;
        let signal = parse_official_status(raw, &source("statuspage-v2"), Utc::now()).unwrap();
        assert_eq!(signal.confidence, "official");
        assert_eq!(signal.value, Some(1.0));
    }

    #[test]
    fn historical_windows_support_midnight_wrap() {
        let mut source = source("historical-risk-window");
        source.url = None;
        source.timezone = Some("UTC".into());
        source.risk_windows = vec![RiskWindowConfig {
            start_hour: 22,
            end_hour: 2,
            risk: 0.9,
            label: "high".into(),
        }];
        let at = DateTime::parse_from_rfc3339("2026-08-30T23:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let signal = historical_risk_signal(&source, at).unwrap();
        assert_eq!(signal.value, Some(0.9));
    }
}
