use std::collections::BTreeMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use whale_core::{baseline_from_snapshot, delta_from_baseline};
use whale_protocol::{
    CapabilitiesResponse, ClientBaseline, FeatureCapabilities, GlobalSnapshot,
    InstanceCapabilities, ModelDescriptor, PresentationDefaults, SignalKind, UsageDelta,
    CAPABILITIES_SCHEMA_VERSION, GLOBAL_SCOPE,
};

use crate::layout::clamp_scale;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ClientSettings {
    pub endpoint: String,
    pub protected_token: String,
    pub model: String,
    pub reasoning_effort: String,
    pub card_overrides: BTreeMap<String, bool>,
    pub scale: f32,
    pub volume: f32,
    pub sound_set: String,
    pub bubble_enabled: bool,
    pub always_on_top: bool,
    pub autostart: bool,
    pub x: Option<i32>,
    pub y: Option<i32>,
    pub startup_baseline: Option<ClientBaseline>,
}

impl Default for ClientSettings {
    fn default() -> Self {
        Self {
            endpoint: String::new(),
            protected_token: String::new(),
            model: String::new(),
            reasoning_effort: String::new(),
            card_overrides: BTreeMap::new(),
            scale: 1.5,
            volume: 0.9,
            sound_set: "duck".into(),
            bubble_enabled: true,
            always_on_top: true,
            autostart: false,
            x: None,
            y: None,
            startup_baseline: None,
        }
    }
}

impl ClientSettings {
    pub fn normalize(&mut self) {
        self.scale = clamp_scale(self.scale);
        self.volume = self.volume.clamp(0.0, 1.0);
        self.model = self.model.trim().to_string();
        self.reasoning_effort = self.reasoning_effort.trim().to_string();
        if self.sound_set != "fx1" {
            self.sound_set = "duck".into();
        }
        self.endpoint = self.endpoint.trim_end_matches('/').to_string();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardStyle {
    Label,
    Amount,
    Period,
    Hint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CardTone {
    Primary,
    Muted,
    Good,
    Danger,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CardLine {
    pub text: String,
    pub style: CardStyle,
    pub tone: CardTone,
    pub wrap: bool,
}

impl CardLine {
    pub fn new(text: impl Into<String>, style: CardStyle) -> Self {
        Self {
            text: text.into(),
            style,
            tone: CardTone::Primary,
            wrap: false,
        }
    }

    pub fn tone(mut self, tone: CardTone) -> Self {
        self.tone = tone;
        self
    }

    pub fn wrapped(mut self) -> Self {
        self.wrap = true;
        self
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RandomCard {
    Lines([Option<CardLine>; 3]),
    RuaGif,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BubbleContent {
    Data,
    Random(RandomCard),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BubbleAction {
    None,
    SwitchContent,
    Close,
}

#[derive(Debug, Clone)]
pub struct RuntimeState {
    pub capabilities: Option<CapabilitiesResponse>,
    pub snapshot: Option<GlobalSnapshot>,
    pub startup_delta: Option<UsageDelta>,
    pub last_error: Option<String>,
    pub last_fetch_at: Option<DateTime<Utc>>,
    pub fetching: bool,
    pub display_today_tokens: Option<i64>,
    pub display_today_usd_micros: Option<i64>,
    pub startup_priced_usd_micros: Option<i64>,
    pub bubble_open: bool,
    pub bubble_content: BubbleContent,
    pub pending_content: Option<BubbleContent>,
    entertainment_deck: Vec<RandomCard>,
    entertainment_index: usize,
    startup_cost_day: Option<String>,
    startup_cost_baseline: BTreeMap<String, i64>,
    startup_last_model_costs: BTreeMap<String, i64>,
    startup_completed_usd_micros: i64,
    random_counter: u64,
}

impl Default for RuntimeState {
    fn default() -> Self {
        Self {
            capabilities: None,
            snapshot: None,
            startup_delta: None,
            last_error: None,
            last_fetch_at: None,
            fetching: false,
            display_today_tokens: None,
            display_today_usd_micros: None,
            startup_priced_usd_micros: None,
            bubble_open: false,
            bubble_content: BubbleContent::Data,
            pending_content: None,
            entertainment_deck: Vec::new(),
            entertainment_index: 0,
            startup_cost_day: None,
            startup_cost_baseline: BTreeMap::new(),
            startup_last_model_costs: BTreeMap::new(),
            startup_completed_usd_micros: 0,
            random_counter: 0,
        }
    }
}

impl RuntimeState {
    pub fn apply_capabilities(
        &mut self,
        settings: &mut ClientSettings,
        capabilities: CapabilitiesResponse,
    ) {
        self.capabilities = Some(capabilities);
        self.resolve_focus(settings);
    }

    pub fn begin_refresh(&mut self) {
        self.fetching = true;
    }

    pub fn apply_snapshot(&mut self, settings: &mut ClientSettings, snapshot: GlobalSnapshot) {
        if self
            .capabilities
            .as_ref()
            .is_none_or(|capabilities| capabilities.minimum_client_version.is_empty())
        {
            self.capabilities = Some(legacy_capabilities(&snapshot));
        }
        if settings.startup_baseline.is_none() {
            settings.startup_baseline = Some(baseline_from_snapshot(&snapshot));
        }
        self.startup_delta = settings
            .startup_baseline
            .as_ref()
            .map(|baseline| delta_from_baseline(baseline, &snapshot));
        self.update_startup_priced_usd(&snapshot);
        self.last_fetch_at = Some(Utc::now());
        self.last_error = None;
        self.fetching = false;
        if self.display_today_tokens.is_none() {
            self.display_today_tokens = Some(snapshot.today.tokens.total_tokens);
        }
        if self.display_today_usd_micros.is_none() {
            self.display_today_usd_micros = estimated_today_usd_micros(&snapshot);
        }
        self.snapshot = Some(snapshot);
        self.resolve_focus(settings);
    }

    fn resolve_focus(&self, settings: &mut ClientSettings) {
        let Some(capabilities) = self.capabilities.as_ref() else {
            return;
        };
        let current_valid = !settings.model.is_empty()
            && capabilities
                .models
                .iter()
                .any(|model| model.model.eq_ignore_ascii_case(&settings.model));
        if !current_valid {
            settings.model = capabilities
                .defaults
                .focus_model
                .as_ref()
                .filter(|model| {
                    capabilities
                        .models
                        .iter()
                        .any(|candidate| candidate.model.eq_ignore_ascii_case(model))
                })
                .cloned()
                .or_else(|| best_focus_model(capabilities, self.snapshot.as_ref()))
                .unwrap_or_default();
        }
        let descriptor = capabilities
            .models
            .iter()
            .find(|model| model.model.eq_ignore_ascii_case(&settings.model));
        let effort_valid = !settings.reasoning_effort.is_empty()
            && descriptor.is_some_and(|model| {
                model
                    .reasoning_efforts
                    .iter()
                    .any(|effort| effort.eq_ignore_ascii_case(&settings.reasoning_effort))
            });
        if !effort_valid {
            settings.reasoning_effort = capabilities
                .defaults
                .focus_reasoning_effort
                .as_ref()
                .filter(|effort| {
                    descriptor.is_some_and(|model| {
                        model
                            .reasoning_efforts
                            .iter()
                            .any(|candidate| candidate.eq_ignore_ascii_case(effort))
                    })
                })
                .cloned()
                .or_else(|| descriptor.and_then(|model| model.reasoning_efforts.first().cloned()))
                .unwrap_or_default();
        }
    }

    fn update_startup_priced_usd(&mut self, snapshot: &GlobalSnapshot) {
        let current = snapshot
            .models
            .iter()
            .filter_map(|model| {
                model.totals.estimated_usd_micros.map(|cost| {
                    (
                        format!(
                            "{}|{}|{}",
                            model.provider,
                            model.model,
                            model.reasoning_effort.as_deref().unwrap_or_default()
                        ),
                        cost,
                    )
                })
            })
            .collect::<BTreeMap<_, _>>();
        match self.startup_cost_day.as_deref() {
            None => {
                self.startup_cost_day = Some(snapshot.reporting_day.clone());
                self.startup_cost_baseline = current.clone();
                self.startup_last_model_costs = current;
                self.startup_priced_usd_micros = Some(0);
            }
            Some(day) if day == snapshot.reporting_day => {
                self.startup_last_model_costs = current.clone();
                let today_delta = priced_cost_delta(&current, &self.startup_cost_baseline);
                self.startup_priced_usd_micros = Some(
                    self.startup_completed_usd_micros
                        .saturating_add(today_delta),
                );
            }
            Some(_) => {
                let completed_day =
                    priced_cost_delta(&self.startup_last_model_costs, &self.startup_cost_baseline);
                self.startup_completed_usd_micros = self
                    .startup_completed_usd_micros
                    .saturating_add(completed_day);
                self.startup_cost_day = Some(snapshot.reporting_day.clone());
                self.startup_cost_baseline.clear();
                self.startup_last_model_costs = current.clone();
                self.startup_priced_usd_micros = Some(
                    self.startup_completed_usd_micros
                        .saturating_add(current.values().copied().sum::<i64>()),
                );
            }
        }
    }

    pub fn apply_network_error(&mut self, error: impl Into<String>) {
        self.last_error = Some(error.into());
        self.fetching = false;
    }

    pub fn open_data_bubble(&mut self, enabled: bool) {
        self.bubble_content = BubbleContent::Data;
        self.pending_content = None;
        self.entertainment_deck.clear();
        self.entertainment_index = 0;
        self.bubble_open = enabled;
    }

    pub fn begin_random_transition(&mut self, settings: &ClientSettings) -> BubbleAction {
        if !self.bubble_open || self.pending_content.is_some() {
            return BubbleAction::None;
        }
        if matches!(self.bubble_content, BubbleContent::Data) {
            self.random_counter = self.random_counter.wrapping_add(1);
            let seed = random_seed(self.random_counter);
            self.entertainment_deck = build_entertainment_deck(
                seed,
                settings,
                self.capabilities.as_ref(),
                self.snapshot.as_ref(),
                self.startup_delta.as_ref(),
                self.startup_priced_usd_micros,
            );
            self.entertainment_index = 0;
        }
        if let Some(card) = self
            .entertainment_deck
            .get(self.entertainment_index)
            .cloned()
        {
            self.entertainment_index += 1;
            self.pending_content = Some(BubbleContent::Random(card));
            BubbleAction::SwitchContent
        } else {
            BubbleAction::Close
        }
    }

    pub fn commit_pending_content(&mut self) {
        if let Some(content) = self.pending_content.take() {
            self.bubble_content = content;
        }
    }

    pub fn close_bubble(&mut self) {
        self.pending_content = None;
        self.entertainment_deck.clear();
        self.entertainment_index = 0;
        self.bubble_open = false;
    }
}

fn priced_cost_delta(current: &BTreeMap<String, i64>, baseline: &BTreeMap<String, i64>) -> i64 {
    current.iter().fold(0_i64, |total, (key, value)| {
        total.saturating_add(
            value
                .saturating_sub(*baseline.get(key).unwrap_or(&0))
                .max(0),
        )
    })
}

pub fn data_card_lines(runtime: &RuntimeState) -> [Option<CardLine>; 3] {
    if let Some(snapshot) = &runtime.snapshot {
        let hint = if runtime.fetching {
            "加载中…".to_string()
        } else if let Some(error) = &runtime.last_error {
            abbreviate(error, 22)
        } else if runtime
            .capabilities
            .as_ref()
            .is_some_and(|capabilities| capabilities.features.pricing)
        {
            format_usd(runtime.display_today_usd_micros)
        } else {
            format!("{} 次请求", snapshot.today.requests)
        };
        [
            Some(CardLine::new("CLIProxyAPI 今日", CardStyle::Label)),
            Some(CardLine::new(
                format_tokens(
                    runtime
                        .display_today_tokens
                        .unwrap_or(snapshot.today.tokens.total_tokens),
                ),
                CardStyle::Amount,
            )),
            Some(
                CardLine::new(hint, CardStyle::Hint).tone(if runtime.last_error.is_some() {
                    CardTone::Danger
                } else {
                    CardTone::Muted
                }),
            ),
        ]
    } else if let Some(error) = &runtime.last_error {
        [
            Some(CardLine::new("CPA Whale", CardStyle::Label)),
            Some(
                CardLine::new(abbreviate(error, 20), CardStyle::Label)
                    .tone(CardTone::Danger)
                    .wrapped(),
            ),
            Some(CardLine::new("打开菜单检查连接", CardStyle::Hint).tone(CardTone::Muted)),
        ]
    } else {
        [
            Some(CardLine::new("CLIProxyAPI", CardStyle::Label)),
            Some(CardLine::new("连接中…", CardStyle::Amount)),
            None,
        ]
    }
}

pub fn displayed_card(runtime: &RuntimeState) -> RandomCard {
    match &runtime.bubble_content {
        BubbleContent::Data => RandomCard::Lines(data_card_lines(runtime)),
        BubbleContent::Random(card) => card.clone(),
    }
}

pub fn build_entertainment_deck(
    seed: u64,
    settings: &ClientSettings,
    capabilities: Option<&CapabilitiesResponse>,
    snapshot: Option<&GlobalSnapshot>,
    startup_delta: Option<&UsageDelta>,
    startup_usd_micros: Option<i64>,
) -> Vec<RandomCard> {
    let casual = [
        "不知道用户有什么用，先赶走吧~",
        "我...我...我也要挣钱吗？",
        "我去吃饭啦，跑完叫我",
        "压力一只蓝色大肥鱼？！",
        "DeepSleep...",
        "坏了...用户彻底怒了！",
    ];
    let token = [
        "你桌面上怎么又多了一只鲸鱼...?",
        "恭喜你实现 token 自由！token 全跑了！",
        "真当我是便宜货啊...",
    ];
    let mut cards = Vec::new();
    if card_enabled(settings, capabilities, "startup") {
        cards.push(startup_card(startup_delta, startup_usd_micros));
    }
    if card_enabled(settings, capabilities, "models") {
        cards.push(model_distribution_card(capabilities, snapshot));
    }
    if card_enabled(settings, capabilities, "intelligence")
        && capabilities.is_none_or(|value| value.features.intelligence)
    {
        cards.push(intelligence_card(settings, capabilities, snapshot));
    }
    if card_enabled(settings, capabilities, "quota")
        && capabilities.is_none_or(|value| value.features.quota)
    {
        cards.push(quota_card(snapshot));
    }
    if card_enabled(settings, capabilities, "reset")
        && capabilities.is_none_or(|value| value.features.reset_events)
    {
        cards.push(reset_card(snapshot));
    }
    if card_enabled(settings, capabilities, "service-status")
        && capabilities.is_none_or(|value| value.features.service_status)
    {
        cards.push(service_status_card(snapshot));
    }
    if card_enabled(settings, capabilities, "entertainment") {
        cards.extend([
            RandomCard::RuaGif,
            single_line(
                CardStyle::Label,
                casual[(splitmix64(seed ^ 0x22) as usize) % casual.len()],
                true,
            ),
            single_line(
                CardStyle::Label,
                token[(splitmix64(seed ^ 0x33) as usize) % token.len()],
                true,
            ),
            single_line(CardStyle::Amount, "哦鲸鲸... ", false),
        ]);
    }
    cards
}

fn startup_card(delta: Option<&UsageDelta>, priced_usd_micros: Option<i64>) -> RandomCard {
    match delta {
        Some(delta) if delta.compatible => RandomCard::Lines([
            Some(CardLine::new("挂件启动后", CardStyle::Label)),
            Some(CardLine::new(
                format!("+{}", format_tokens(delta.totals.tokens.total_tokens)),
                CardStyle::Period,
            )),
            Some(
                CardLine::new(
                    format_usd(priced_usd_micros.or(delta.totals.estimated_usd_micros)),
                    CardStyle::Hint,
                )
                .tone(CardTone::Muted),
            ),
        ]),
        _ => RandomCard::Lines([
            Some(CardLine::new("挂件启动后", CardStyle::Label)),
            Some(CardLine::new("等待基线", CardStyle::Period)),
            Some(CardLine::new("CLIProxyAPI", CardStyle::Hint).tone(CardTone::Muted)),
        ]),
    }
}

fn model_distribution_card(
    capabilities: Option<&CapabilitiesResponse>,
    snapshot: Option<&GlobalSnapshot>,
) -> RandomCard {
    let Some(snapshot) = snapshot else {
        return single_line(CardStyle::Label, "模型数据加载中…", false);
    };
    let mut models = snapshot.models.iter().collect::<Vec<_>>();
    models.sort_by_key(|model| std::cmp::Reverse(model.totals.tokens.total_tokens));
    let mut lines: [Option<CardLine>; 3] = [None, None, None];
    for (slot, model) in lines.iter_mut().zip(models.into_iter().take(3)) {
        *slot = Some(CardLine::new(
            format!(
                "{}  {}",
                model_display_name(capabilities, &model.model),
                format_tokens(model.totals.tokens.total_tokens)
            ),
            CardStyle::Label,
        ));
    }
    if lines.iter().all(Option::is_none) {
        single_line(CardStyle::Label, "暂无模型数据", false)
    } else {
        RandomCard::Lines(lines)
    }
}

fn quota_card(snapshot: Option<&GlobalSnapshot>) -> RandomCard {
    let Some(snapshot) = snapshot else {
        return single_line(CardStyle::Label, "账户额度加载中…", false);
    };
    let best = snapshot
        .accounts
        .iter()
        .filter_map(account_primary_remaining_percent)
        .min_by(f64::total_cmp);
    if let Some(remaining) = best {
        RandomCard::Lines([
            Some(CardLine::new("账户最低剩余", CardStyle::Label)),
            Some(
                CardLine::new(format!("{remaining:.0}%"), CardStyle::Period).tone(
                    if remaining < 15.0 {
                        CardTone::Danger
                    } else {
                        CardTone::Primary
                    },
                ),
            ),
            None,
        ])
    } else {
        RandomCard::Lines([
            Some(CardLine::new("上游账户额度", CardStyle::Label)),
            Some(CardLine::new("--", CardStyle::Period)),
            None,
        ])
    }
}

pub fn account_primary_remaining_percent(account: &whale_protocol::AccountSnapshot) -> Option<f64> {
    if !account.quota.available {
        return None;
    }
    account
        .quota
        .windows
        .iter()
        .filter(|window| window.name.eq_ignore_ascii_case("primary"))
        .filter(|window| window.remaining_percent.is_some())
        .max_by_key(|window| window.window_minutes.unwrap_or_default())
        .and_then(|window| window.remaining_percent)
        .or_else(|| {
            account
                .quota
                .windows
                .iter()
                .filter(|window| window.window_minutes.unwrap_or_default() > 0)
                .filter_map(|window| window.remaining_percent)
                .min_by(f64::total_cmp)
        })
}

fn reset_card(snapshot: Option<&GlobalSnapshot>) -> RandomCard {
    let signal = snapshot.and_then(|snapshot| {
        snapshot
            .signals
            .iter()
            .find(|signal| signal.kind == SignalKind::ResetRisk && !signal.stale)
            .or_else(|| {
                snapshot
                    .signals
                    .iter()
                    .find(|signal| signal.kind == SignalKind::ResetRisk)
            })
    });
    if let Some(signal) = signal {
        let main = signal
            .value
            .map(|value| match signal.unit.as_deref() {
                Some("risk_index") => format!("{:.0}%", value * 100.0),
                Some("percent") | Some("%") => format!("{value:.0}%"),
                _ => format!("{value:.2}"),
            })
            .unwrap_or_else(|| abbreviate(&signal.title, 10));
        RandomCard::Lines([
            Some(CardLine::new("重置预测参考", CardStyle::Label)),
            Some(CardLine::new(main, CardStyle::Period)),
            None,
        ])
    } else {
        RandomCard::Lines([
            Some(CardLine::new("重置预测参考", CardStyle::Label)),
            Some(CardLine::new("暂无信号", CardStyle::Period)),
            None,
        ])
    }
}

fn service_status_card(snapshot: Option<&GlobalSnapshot>) -> RandomCard {
    let statuses = snapshot
        .map(|snapshot| {
            snapshot
                .signals
                .iter()
                .filter(|signal| signal.kind == SignalKind::ServiceStatus)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    if statuses.is_empty() {
        return RandomCard::Lines([
            Some(CardLine::new("服务状态", CardStyle::Label)),
            Some(CardLine::new("等待更新", CardStyle::Period)),
            Some(CardLine::new("等待服务端配置", CardStyle::Hint).tone(CardTone::Muted)),
        ]);
    }
    let healthy = statuses.iter().all(|signal| signal.value == Some(1.0));
    let hint = abbreviate(
        &statuses
            .iter()
            .take(2)
            .map(|signal| compact_status_source(&signal.source))
            .collect::<Vec<_>>()
            .join(" · "),
        20,
    );
    RandomCard::Lines([
        Some(CardLine::new("官方服务状态", CardStyle::Label)),
        Some(
            CardLine::new(if healthy { "正常" } else { "有异常" }, CardStyle::Period).tone(
                if healthy {
                    CardTone::Good
                } else {
                    CardTone::Danger
                },
            ),
        ),
        Some(CardLine::new(hint, CardStyle::Hint).tone(CardTone::Muted)),
    ])
}

fn intelligence_card(
    settings: &ClientSettings,
    capabilities: Option<&CapabilitiesResponse>,
    snapshot: Option<&GlobalSnapshot>,
) -> RandomCard {
    let Some(snapshot) = snapshot else {
        return RandomCard::Lines([
            Some(CardLine::new("CLIProxyAPI", CardStyle::Label)),
            Some(CardLine::new("等待数据", CardStyle::Period)),
            Some(CardLine::new("稍后再摸摸我", CardStyle::Hint).tone(CardTone::Muted)),
        ]);
    };
    let intelligence = snapshot.signals.iter().find(|signal| {
        signal.kind == SignalKind::Intelligence
            && signal
                .model
                .as_deref()
                .is_some_and(|model| model.eq_ignore_ascii_case(&settings.model))
            && (settings.reasoning_effort.is_empty()
                || signal
                    .reasoning_effort
                    .as_deref()
                    .is_some_and(|effort| effort.eq_ignore_ascii_case(&settings.reasoning_effort)))
    });
    if let Some(signal) = intelligence {
        return RandomCard::Lines([
            Some(CardLine::new(
                format!(
                    "{} / {}",
                    model_display_name(capabilities, &settings.model),
                    if settings.reasoning_effort.is_empty() {
                        "--"
                    } else {
                        settings.reasoning_effort.as_str()
                    }
                ),
                CardStyle::Label,
            )),
            Some(CardLine::new(
                signal
                    .value
                    .map(|value| format!("IQ  {value:.2}"))
                    .unwrap_or_else(|| "IQ  --".into()),
                CardStyle::Period,
            )),
            Some(CardLine::new("社区参考 · 非官方", CardStyle::Hint).tone(CardTone::Muted)),
        ]);
    }
    RandomCard::Lines([
        Some(CardLine::new("CLIProxyAPI 今日", CardStyle::Label)),
        Some(CardLine::new(
            format_tokens(snapshot.today.tokens.total_tokens),
            CardStyle::Period,
        )),
        Some(
            CardLine::new(
                format_usd(estimated_today_usd_micros(snapshot)),
                CardStyle::Hint,
            )
            .tone(CardTone::Muted),
        ),
    ])
}

fn single_line(style: CardStyle, text: &str, wrap: bool) -> RandomCard {
    let line = CardLine::new(text, style);
    RandomCard::Lines([None, Some(if wrap { line.wrapped() } else { line }), None])
}

fn random_seed(counter: u64) -> u64 {
    let wall = Utc::now().timestamp_nanos_opt().unwrap_or_default() as u64;
    splitmix64(wall ^ counter.rotate_left(17))
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9e37_79b9_7f4a_7c15);
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

fn compact_status_source(value: &str) -> String {
    let value = value.trim();
    let lower = value.to_ascii_lowercase();
    for suffix in [
        " official service status",
        " official status",
        " service status",
        " status",
        " 官方服务状态",
        " 官方状态",
        " 服务状态",
    ] {
        if lower.ends_with(suffix) {
            return value[..value.len() - suffix.len()]
                .trim_end_matches([' ', '·', '-', '/'])
                .to_string();
        }
    }
    value.to_string()
}

fn abbreviate(value: &str, limit: usize) -> String {
    let mut text = value.chars().take(limit).collect::<String>();
    if value.chars().count() > limit {
        text.push('…');
    }
    text
}

pub fn format_tokens(tokens: i64) -> String {
    let tokens = tokens.max(0) as f64;
    if tokens >= 1_000_000_000.0 {
        format!("{:.2}B", tokens / 1_000_000_000.0)
    } else if tokens >= 1_000_000.0 {
        format!("{:.2}M", tokens / 1_000_000.0)
    } else if tokens >= 1_000.0 {
        format!("{:.1}K", tokens / 1_000.0)
    } else {
        format!("{}", tokens as i64)
    }
}

pub fn estimated_today_usd_micros(snapshot: &GlobalSnapshot) -> Option<i64> {
    snapshot.today.estimated_usd_micros.or_else(|| {
        let mut any_priced = false;
        let subtotal = snapshot.models.iter().fold(0_i64, |total, model| {
            if let Some(cost) = model.totals.estimated_usd_micros {
                any_priced = true;
                total.saturating_add(cost)
            } else {
                total
            }
        });
        any_priced.then_some(subtotal)
    })
}

pub fn format_usd(micros: Option<i64>) -> String {
    micros
        .map(|value| format!("≈ ${:.4}", value as f64 / 1_000_000.0))
        .unwrap_or_else(|| "USD 未定价".into())
}

pub fn model_display_name(capabilities: Option<&CapabilitiesResponse>, model: &str) -> String {
    capabilities
        .and_then(|capabilities| {
            capabilities
                .models
                .iter()
                .find(|descriptor| descriptor.model.eq_ignore_ascii_case(model))
        })
        .map(|descriptor| descriptor.display_name.clone())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or_else(|| model.to_string())
}

pub fn card_enabled(
    settings: &ClientSettings,
    capabilities: Option<&CapabilitiesResponse>,
    card: &str,
) -> bool {
    settings
        .card_overrides
        .get(card)
        .copied()
        .unwrap_or_else(|| {
            capabilities.is_none_or(|capabilities| {
                capabilities.defaults.cards.is_empty()
                    || capabilities
                        .defaults
                        .cards
                        .iter()
                        .any(|candidate| candidate.eq_ignore_ascii_case(card))
            })
        })
}

pub fn legacy_capabilities(snapshot: &GlobalSnapshot) -> CapabilitiesResponse {
    let mut descriptors = BTreeMap::<(String, String), ModelDescriptor>::new();
    for model in &snapshot.models {
        let descriptor = descriptors
            .entry((model.provider.clone(), model.model.clone()))
            .or_insert_with(|| ModelDescriptor {
                provider: model.provider.clone(),
                model: model.model.clone(),
                display_name: model.model.clone(),
                reasoning_efforts: Vec::new(),
                priced: false,
                has_intelligence: false,
            });
        descriptor.priced |= model.totals.estimated_usd_micros.is_some();
        if let Some(effort) = model.reasoning_effort.as_deref() {
            if !descriptor
                .reasoning_efforts
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(effort))
            {
                descriptor.reasoning_efforts.push(effort.into());
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
        let key = descriptors
            .keys()
            .find(|(_, candidate)| candidate.eq_ignore_ascii_case(model))
            .cloned()
            .unwrap_or_else(|| (String::new(), model.into()));
        let descriptor = descriptors.entry(key).or_insert_with(|| ModelDescriptor {
            provider: String::new(),
            model: model.into(),
            display_name: model.into(),
            reasoning_efforts: Vec::new(),
            priced: false,
            has_intelligence: true,
        });
        descriptor.has_intelligence = true;
        if let Some(effort) = signal.reasoning_effort.as_deref() {
            if !descriptor
                .reasoning_efforts
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(effort))
            {
                descriptor.reasoning_efforts.push(effort.into());
            }
        }
    }
    let service_status = snapshot
        .signals
        .iter()
        .any(|signal| signal.kind == SignalKind::ServiceStatus);
    let intelligence = snapshot
        .signals
        .iter()
        .any(|signal| signal.kind == SignalKind::Intelligence);
    let reset_events = snapshot
        .signals
        .iter()
        .any(|signal| signal.kind == SignalKind::ResetRisk);
    CapabilitiesResponse {
        schema_version: CAPABILITIES_SCHEMA_VERSION,
        plugin_version: snapshot.health.plugin_version.clone(),
        minimum_client_version: String::new(),
        instance: InstanceCapabilities {
            display_name: if snapshot.scope_label.trim().is_empty() {
                "CLIProxyAPI".into()
            } else {
                snapshot.scope_label.clone()
            },
            scope: if snapshot.scope.is_empty() {
                GLOBAL_SCOPE.into()
            } else {
                snapshot.scope.clone()
            },
            scope_label: snapshot.scope_label.clone(),
            supports_user_attribution: snapshot.supports_user_attribution,
            timezone: snapshot.timezone.clone(),
        },
        features: FeatureCapabilities {
            pricing: snapshot.today.estimated_usd_micros.is_some()
                || snapshot
                    .models
                    .iter()
                    .any(|model| model.totals.estimated_usd_micros.is_some()),
            quota: snapshot
                .accounts
                .iter()
                .any(|account| account.quota.available),
            external_signals: !snapshot.signals.is_empty(),
            intelligence,
            reset_events,
            service_status,
        },
        models: descriptors.into_values().collect(),
        quota_providers: snapshot
            .accounts
            .iter()
            .filter(|account| account.quota.available)
            .map(|account| account.provider.clone())
            .collect::<std::collections::BTreeSet<_>>()
            .into_iter()
            .collect(),
        defaults: PresentationDefaults {
            focus_model: None,
            focus_reasoning_effort: None,
            poll_interval_seconds: 60,
            cards: [
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
            .collect(),
        },
    }
}

fn best_focus_model(
    capabilities: &CapabilitiesResponse,
    snapshot: Option<&GlobalSnapshot>,
) -> Option<String> {
    capabilities
        .models
        .iter()
        .max_by_key(|descriptor| {
            let tokens = snapshot
                .and_then(|snapshot| {
                    snapshot
                        .models
                        .iter()
                        .filter(|model| model.model.eq_ignore_ascii_case(&descriptor.model))
                        .map(|model| model.totals.tokens.total_tokens)
                        .max()
                })
                .unwrap_or(0);
            (
                i64::from(descriptor.has_intelligence && tokens > 0),
                i64::from(descriptor.priced && tokens > 0),
                tokens,
            )
        })
        .map(|descriptor| descriptor.model.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_large_token_counts() {
        assert_eq!(format_tokens(12_345_678), "12.35M");
    }

    #[test]
    fn compacts_status_sources_without_vendor_hardcoding() {
        assert_eq!(compact_status_source("OpenAI official status"), "OpenAI");
        assert_eq!(compact_status_source("Anthropic 服务状态"), "Anthropic");
        assert_eq!(compact_status_source("Local Provider"), "Local Provider");
    }

    #[test]
    fn falls_back_to_priced_model_subtotal() {
        let mut snapshot = GlobalSnapshot::empty("epoch", "Asia/Shanghai");
        snapshot.today.estimated_usd_micros = None;
        let mut priced = whale_protocol::ModelUsage {
            model: "gpt-5.6-sol".into(),
            reasoning_effort: Some("xhigh".into()),
            provider: "codex".into(),
            totals: whale_protocol::UsageTotals::default(),
        };
        priced.totals.estimated_usd_micros = Some(12_000_000);
        let unpriced = whale_protocol::ModelUsage {
            model: "gpt-image-2".into(),
            reasoning_effort: None,
            provider: "openai".into(),
            totals: whale_protocol::UsageTotals::default(),
        };
        snapshot.models = vec![priced, unpriced];
        assert_eq!(estimated_today_usd_micros(&snapshot), Some(12_000_000));
    }

    #[test]
    fn computes_startup_usd_from_priced_models_when_total_is_unknown() {
        let mut settings = ClientSettings::default();
        let mut runtime = RuntimeState::default();
        let mut first = GlobalSnapshot::empty("epoch", "Asia/Shanghai");
        first.reporting_day = "2026-08-30".into();
        let mut model = whale_protocol::ModelUsage {
            model: "gpt-5.6-sol".into(),
            reasoning_effort: Some("xhigh".into()),
            provider: "codex".into(),
            totals: whale_protocol::UsageTotals::default(),
        };
        model.totals.estimated_usd_micros = Some(10_000_000);
        first.models.push(model.clone());
        runtime.apply_snapshot(&mut settings, first);
        assert_eq!(runtime.startup_priced_usd_micros, Some(0));

        let mut second = GlobalSnapshot::empty("epoch", "Asia/Shanghai");
        second.reporting_day = "2026-08-30".into();
        model.totals.estimated_usd_micros = Some(12_500_000);
        second.models.push(model);
        runtime.apply_snapshot(&mut settings, second);
        assert_eq!(runtime.startup_priced_usd_micros, Some(2_500_000));
    }

    #[test]
    fn account_remaining_prefers_account_primary_over_additional_limits() {
        let account = whale_protocol::AccountSnapshot {
            auth_index: "account".into(),
            label: "Account".into(),
            provider: "codex".into(),
            status: "active".into(),
            unavailable: false,
            totals: whale_protocol::UsageTotals::default(),
            quota: whale_protocol::QuotaSnapshot {
                available: true,
                windows: vec![
                    whale_protocol::QuotaWindow {
                        name: "bengalfox primary".into(),
                        limit_name: Some("Model Limit".into()),
                        used_percent: Some(80.0),
                        remaining_percent: Some(20.0),
                        window_minutes: Some(300),
                        reset_after_seconds: None,
                        reset_at: None,
                        allowed: Some(true),
                        limit_reached: Some(false),
                    },
                    whale_protocol::QuotaWindow {
                        name: "primary".into(),
                        limit_name: None,
                        used_percent: Some(25.0),
                        remaining_percent: Some(75.0),
                        window_minutes: Some(10_080),
                        reset_after_seconds: None,
                        reset_at: None,
                        allowed: Some(true),
                        limit_reached: Some(false),
                    },
                ],
                ..whale_protocol::QuotaSnapshot::default()
            },
            updated_at: None,
        };
        assert_eq!(account_primary_remaining_percent(&account), Some(75.0));
    }

    #[test]
    fn legacy_capabilities_keep_all_available_quota_providers() {
        let mut snapshot = GlobalSnapshot::empty("epoch", "UTC");
        for provider in ["codex", "xai"] {
            snapshot.accounts.push(whale_protocol::AccountSnapshot {
                auth_index: provider.into(),
                label: provider.into(),
                provider: provider.into(),
                status: "active".into(),
                unavailable: false,
                totals: whale_protocol::UsageTotals::default(),
                quota: whale_protocol::QuotaSnapshot {
                    available: true,
                    ..whale_protocol::QuotaSnapshot::default()
                },
                updated_at: None,
            });
        }
        let capabilities = legacy_capabilities(&snapshot);
        assert_eq!(capabilities.quota_providers, vec!["codex", "xai"]);
        assert!(capabilities.features.quota);
    }

    #[test]
    fn new_install_has_no_maintainer_endpoint() {
        let settings = ClientSettings::default();
        assert!(settings.endpoint.is_empty());
        assert!(settings.model.is_empty());
        assert!(settings.reasoning_effort.is_empty());
    }

    #[test]
    fn capabilities_choose_configured_non_gpt_focus() {
        let capabilities = CapabilitiesResponse {
            schema_version: CAPABILITIES_SCHEMA_VERSION,
            plugin_version: "0.3.0".into(),
            minimum_client_version: "0.3.0".into(),
            instance: InstanceCapabilities {
                display_name: "Home CPA".into(),
                scope: "global".into(),
                scope_label: "Home CPA".into(),
                supports_user_attribution: false,
                timezone: "UTC".into(),
            },
            features: FeatureCapabilities::default(),
            models: vec![ModelDescriptor {
                provider: "example".into(),
                model: "aurora-model".into(),
                display_name: "Aurora".into(),
                reasoning_efforts: vec!["deep".into()],
                priced: true,
                has_intelligence: true,
            }],
            quota_providers: Vec::new(),
            defaults: PresentationDefaults {
                focus_model: Some("aurora-model".into()),
                focus_reasoning_effort: Some("deep".into()),
                poll_interval_seconds: 90,
                cards: vec!["startup".into()],
            },
        };
        let mut settings = ClientSettings::default();
        let mut runtime = RuntimeState::default();
        runtime.apply_capabilities(&mut settings, capabilities.clone());
        assert_eq!(settings.model, "aurora-model");
        assert_eq!(settings.reasoning_effort, "deep");
        assert_eq!(
            model_display_name(Some(&capabilities), &settings.model),
            "Aurora"
        );
        let cards = build_entertainment_deck(1, &settings, Some(&capabilities), None, None, None);
        assert_eq!(cards.len(), 1);
    }

    #[test]
    fn no_click_baseline_is_serialized() {
        let value = serde_json::to_value(ClientSettings::default()).unwrap();
        assert!(value.get("click_baseline").is_none());
    }

    #[test]
    fn removed_settings_are_ignored() {
        let old = r#"{
          "endpoint":"https://example.test",
          "click_through":true,
          "click_baseline":{"epoch":"old","sequence":1,"captured_at":"2026-08-30T00:00:00Z","totals":{"requests":0,"successful_requests":0,"failed_requests":0,"tokens":{"input_tokens":0,"output_tokens":0,"reasoning_tokens":0,"cached_tokens":0,"cache_read_tokens":0,"cache_write_tokens":0,"total_tokens":0},"estimated_usd_micros":0}}
        }"#;
        let settings: ClientSettings = serde_json::from_str(old).unwrap();
        assert_eq!(settings.endpoint, "https://example.test");
        let serialized = serde_json::to_value(settings).unwrap();
        assert!(serialized.get("click_through").is_none());
        assert!(serialized.get("click_baseline").is_none());
    }

    #[test]
    fn entertainment_card_is_stable_until_committed() {
        let settings = ClientSettings::default();
        let mut runtime = RuntimeState::default();
        runtime.open_data_bubble(true);
        assert_eq!(
            runtime.begin_random_transition(&settings),
            BubbleAction::SwitchContent
        );
        let pending = runtime.pending_content.clone();
        for _ in 0..300 {
            assert_eq!(runtime.pending_content, pending);
            assert_eq!(
                displayed_card(&runtime),
                RandomCard::Lines(data_card_lines(&runtime))
            );
        }
        runtime.commit_pending_content();
        let displayed = displayed_card(&runtime);
        for _ in 0..300 {
            assert_eq!(displayed_card(&runtime), displayed);
        }
    }

    #[test]
    fn entertainment_deck_can_be_advanced_before_closing() {
        let settings = ClientSettings::default();
        let mut runtime = RuntimeState::default();
        runtime.open_data_bubble(true);
        let mut shown = Vec::new();
        for _ in 0..10 {
            assert_eq!(
                runtime.begin_random_transition(&settings),
                BubbleAction::SwitchContent
            );
            runtime.commit_pending_content();
            shown.push(displayed_card(&runtime));
        }
        assert!(shown.iter().any(|card| matches!(card, RandomCard::RuaGif)));
        let text = shown
            .iter()
            .filter_map(|card| match card {
                RandomCard::Lines(lines) => Some(
                    lines
                        .iter()
                        .flatten()
                        .map(|line| line.text.as_str())
                        .collect::<Vec<_>>()
                        .join(" "),
                ),
                RandomCard::RuaGif => None,
            })
            .collect::<Vec<_>>()
            .join(" ");
        assert!(text.contains("额度"));
        assert!(text.contains("重置"));
        assert!(text.contains("服务状态"));
        assert_eq!(
            runtime.begin_random_transition(&settings),
            BubbleAction::Close
        );
    }

    #[test]
    fn snapshot_updates_do_not_replace_entertainment() {
        let settings = ClientSettings::default();
        let mut runtime = RuntimeState::default();
        runtime.open_data_bubble(true);
        runtime.pending_content = Some(BubbleContent::Random(RandomCard::RuaGif));
        runtime.commit_pending_content();
        let mut mutable_settings = settings;
        runtime.apply_snapshot(
            &mut mutable_settings,
            GlobalSnapshot::empty("epoch", "Asia/Shanghai"),
        );
        assert_eq!(
            runtime.bubble_content,
            BubbleContent::Random(RandomCard::RuaGif)
        );
    }
}
