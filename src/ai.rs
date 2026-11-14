use data::config::ai::{AiConfig, AiProvider};
use iced::{
    Alignment, Element, Length, Padding, Task,
    widget::{
        button, checkbox, column, container, pick_list, row, rule, scrollable, text, text_input,
        tooltip,
    },
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{fmt, io::ErrorKind, time::Duration};

const PROMPT_VERSION: &str = "microstructure-v4";
const ANALYSIS_HISTORY_PATH: &str = "ai/analysis-history.jsonl";
const MAX_HISTORY_RECORDS: usize = 500;

fn icon_button<'a>(
    icon: crate::style::LucideIcon,
    label: &'static str,
    message: Option<Message>,
) -> Element<'a, Message> {
    let button = button(
        crate::style::lucide_icon_text(icon, 13)
            .width(18)
            .align_x(Alignment::Center),
    )
    .padding(4)
    .style(|theme, status| crate::style::button::transparent(theme, status, false));
    let button = if let Some(message) = message {
        button.on_press(message)
    } else {
        button
    };
    tooltip(button, Some(label), tooltip::Position::Bottom).into()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum AnalysisMode {
    #[default]
    Scalping,
    NextCandle,
    Intraday,
    Swing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChartSource {
    pub pane_id: uuid::Uuid,
    pub label: String,
    pub symbol: String,
    pub venue: String,
    pub basis: String,
}

pub(crate) struct AiAlert {
    pub title: String,
    pub body: String,
}

impl fmt::Display for ChartSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

impl AnalysisMode {
    const ALL: [Self; 4] = [
        Self::Scalping,
        Self::NextCandle,
        Self::Intraday,
        Self::Swing,
    ];
}

impl fmt::Display for AnalysisMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Scalping => write!(f, "Scalping"),
            Self::NextCandle => write!(f, "Next K"),
            Self::Intraday => write!(f, "Intraday"),
            Self::Swing => write!(f, "Swing"),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MarketBar {
    pub time_ms: u64,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub buy_volume: Option<f64>,
    pub sell_volume: Option<f64>,
    pub delta: Option<f64>,
    pub cvd: Option<f64>,
    pub open_interest: Option<f64>,
}

pub(crate) fn populate_relative_cvd(bars: &mut [MarketBar]) {
    let mut cvd = 0.0;
    for bar in bars {
        if let Some(delta) = bar.delta {
            cvd += delta;
            bar.cvd = Some(cvd);
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct DepthLevel {
    pub side: &'static str,
    pub price: f64,
    pub quantity: f64,
}

#[derive(Debug, Clone, Serialize)]
pub struct LiquidationSummary {
    pub long_quantity: f64,
    pub short_quantity: f64,
    pub event_count: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct MarketFeatures {
    pub short_return_pct: Option<f64>,
    pub long_return_pct: Option<f64>,
    pub cumulative_delta: Option<f64>,
    pub recent_delta: Option<f64>,
    pub open_interest_change_pct: Option<f64>,
    pub best_bid: Option<f64>,
    pub best_ask: Option<f64>,
    pub spread_bps: Option<f64>,
    pub depth_imbalance: Option<f64>,
    pub vwap: Option<f64>,
    pub vwap_deviation_pct: Option<f64>,
    pub volume_profile_poc: Option<f64>,
    pub support_levels: Vec<f64>,
    pub resistance_levels: Vec<f64>,
}

impl MarketFeatures {
    pub fn calculate(bars: &[MarketBar], depth: &[DepthLevel], short_bar_count: usize) -> Self {
        let short_start = bars.len().saturating_sub(short_bar_count.max(2));
        let short_bars = &bars[short_start..];
        let last_close = bars.last().map(|bar| bar.close);
        let return_pct = |first: Option<&MarketBar>| {
            first
                .zip(last_close)
                .filter(|(bar, _)| bar.close.abs() > f64::EPSILON)
                .map(|(bar, last)| (last / bar.close - 1.0) * 100.0)
        };
        let cumulative_delta = sum_options(bars.iter().map(|bar| bar.delta));
        let recent_delta = sum_options(short_bars.iter().map(|bar| bar.delta));
        let oi_values = short_bars
            .iter()
            .filter_map(|bar| bar.open_interest)
            .collect::<Vec<_>>();
        let open_interest_change_pct = oi_values
            .first()
            .zip(oi_values.last())
            .filter(|(first, _)| first.abs() > f64::EPSILON)
            .map(|(first, last)| (last / first - 1.0) * 100.0);

        let best_bid = depth
            .iter()
            .filter(|level| level.side == "bid")
            .map(|level| level.price)
            .max_by(f64::total_cmp);
        let best_ask = depth
            .iter()
            .filter(|level| level.side == "ask")
            .map(|level| level.price)
            .min_by(f64::total_cmp);
        let spread_bps = best_bid
            .zip(best_ask)
            .filter(|(bid, ask)| *ask > *bid && (*ask + *bid).abs() > f64::EPSILON)
            .map(|(bid, ask)| (ask - bid) / ((ask + bid) / 2.0) * 10_000.0);
        let bid_depth = depth
            .iter()
            .filter(|level| level.side == "bid")
            .map(|level| level.quantity)
            .sum::<f64>();
        let ask_depth = depth
            .iter()
            .filter(|level| level.side == "ask")
            .map(|level| level.quantity)
            .sum::<f64>();
        let depth_imbalance = ((bid_depth + ask_depth) > f64::EPSILON)
            .then_some((bid_depth - ask_depth) / (bid_depth + ask_depth));
        let total_volume = bars.iter().map(|bar| bar.volume).sum::<f64>();
        let vwap = (total_volume > f64::EPSILON).then(|| {
            bars.iter()
                .map(|bar| ((bar.high + bar.low + bar.close) / 3.0) * bar.volume)
                .sum::<f64>()
                / total_volume
        });
        let vwap_deviation_pct = vwap
            .zip(last_close)
            .filter(|(vwap, _)| vwap.abs() > f64::EPSILON)
            .map(|(vwap, last)| (last / vwap - 1.0) * 100.0);
        let (mut support_levels, mut resistance_levels) = last_close.map_or_else(
            || (Vec::new(), Vec::new()),
            |last| local_levels(short_bars, depth, last),
        );
        if let (Some(vwap), Some(last)) = (vwap, last_close) {
            insert_reference_level(vwap, last, &mut support_levels, &mut resistance_levels);
        }

        Self {
            short_return_pct: return_pct(short_bars.first()),
            long_return_pct: return_pct(bars.first()),
            cumulative_delta,
            recent_delta,
            open_interest_change_pct,
            best_bid,
            best_ask,
            spread_bps,
            depth_imbalance,
            vwap,
            vwap_deviation_pct,
            volume_profile_poc: None,
            support_levels,
            resistance_levels,
        }
    }

    pub fn set_volume_profile_poc(&mut self, poc: Option<f64>, last_price: Option<f64>) {
        self.volume_profile_poc = poc;
        if let (Some(poc), Some(last)) = (poc, last_price) {
            insert_reference_level(
                poc,
                last,
                &mut self.support_levels,
                &mut self.resistance_levels,
            );
        }
    }
}

fn insert_reference_level(
    level: f64,
    last: f64,
    supports: &mut Vec<f64>,
    resistances: &mut Vec<f64>,
) {
    if (level - last).abs() <= last.abs() * 0.0005 {
        return;
    }
    let levels = if level <= last { supports } else { resistances };
    levels.push(level);
    levels.sort_by(|a, b| (last - *a).abs().total_cmp(&(last - *b).abs()));
    dedup_levels(levels, last);
    levels.truncate(5);
}

#[derive(Debug, Clone, Serialize)]
pub struct MarketSnapshot {
    pub generated_at_ms: i64,
    pub symbol: String,
    pub venue: String,
    pub market: String,
    pub basis: String,
    pub timeframe_ms: u64,
    pub latest_market_time_ms: u64,
    pub data_age_ms: u64,
    pub bars: Vec<MarketBar>,
    pub depth: Vec<DepthLevel>,
    pub liquidations: LiquidationSummary,
    pub features: MarketFeatures,
    pub data_quality: f32,
    pub missing_data: Vec<&'static str>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AnalysisBundle {
    pub primary: MarketSnapshot,
    pub higher_timeframe_contexts: Vec<MarketSnapshot>,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum TradeAction {
    Long,
    Short,
    Wait,
    Avoid,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum TradeBias {
    Long,
    Short,
    #[default]
    Neutral,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum ExecutionState {
    Ready,
    TriggerPending,
    #[default]
    NoEdge,
    Unavailable,
}

impl fmt::Display for ExecutionState {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Ready => write!(f, "READY"),
            Self::TriggerPending => write!(f, "TRIGGER PENDING"),
            Self::NoEdge => write!(f, "NO EDGE"),
            Self::Unavailable => write!(f, "UNAVAILABLE"),
        }
    }
}

impl fmt::Display for TradeBias {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Long => write!(f, "LONG"),
            Self::Short => write!(f, "SHORT"),
            Self::Neutral => write!(f, "NEUTRAL"),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
enum LevelRole {
    Support,
    Resistance,
    #[default]
    Pivot,
}

impl fmt::Display for LevelRole {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Support => write!(f, "Support"),
            Self::Resistance => write!(f, "Resistance"),
            Self::Pivot => write!(f, "Pivot"),
        }
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
struct KeyLevel {
    price: f64,
    role: LevelRole,
    strength: f32,
    reason: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ChartSignal {
    Long,
    Short,
    Neutral,
}

#[derive(Debug, Clone)]
pub(crate) struct ChartAnnotation {
    pub signal: ChartSignal,
    pub time_ms: Option<u64>,
    pub mark_time: bool,
    pub entry_low: Option<f64>,
    pub entry_high: Option<f64>,
    pub stop_loss: Option<f64>,
    pub targets: Vec<f64>,
    pub levels: Vec<ChartLevel>,
    expires_at: Option<std::time::Instant>,
}

#[derive(Debug, Clone)]
pub(crate) struct ChartLevel {
    pub price: f64,
    pub is_support: bool,
    pub strength: f32,
}

impl ChartAnnotation {
    pub fn is_valid(&self) -> bool {
        self.expires_at
            .is_none_or(|expires_at| std::time::Instant::now() < expires_at)
    }
}

fn build_chart_annotation(
    analysis: &AnalysisResult,
    latest_price: Option<f64>,
    time_ms: Option<u64>,
    mark_time: bool,
    expires_at: Option<std::time::Instant>,
) -> Option<ChartAnnotation> {
    let mut levels = analysis
        .key_levels
        .iter()
        .filter(|level| level.strength >= 50.0)
        .map(|level| ChartLevel {
            price: level.price,
            is_support: match level.role {
                LevelRole::Support => true,
                LevelRole::Resistance => false,
                LevelRole::Pivot => latest_price.is_some_and(|last| level.price <= last),
            },
            strength: level.strength,
        })
        .collect::<Vec<_>>();
    if levels.is_empty() {
        levels.extend(
            analysis
                .support_levels
                .first()
                .map(|price| ChartLevel {
                    price: *price,
                    is_support: true,
                    strength: 50.0,
                })
                .into_iter()
                .chain(analysis.resistance_levels.first().map(|price| ChartLevel {
                    price: *price,
                    is_support: false,
                    strength: 50.0,
                })),
        );
    }
    let annotation = ChartAnnotation {
        signal: match analysis.action {
            TradeAction::Long => ChartSignal::Long,
            TradeAction::Short => ChartSignal::Short,
            TradeAction::Wait | TradeAction::Avoid => ChartSignal::Neutral,
        },
        time_ms,
        mark_time,
        entry_low: analysis.entry_zone.low,
        entry_high: analysis.entry_zone.high,
        stop_loss: analysis.stop_loss,
        targets: analysis.targets.clone(),
        levels,
        expires_at,
    };
    annotation.is_valid().then_some(annotation)
}

impl fmt::Display for TradeAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Long => write!(f, "LONG"),
            Self::Short => write!(f, "SHORT"),
            Self::Wait => write!(f, "WAIT"),
            Self::Avoid => write!(f, "AVOID"),
        }
    }
}

fn analysis_title(mode: AnalysisMode, action: TradeAction) -> String {
    if mode == AnalysisMode::NextCandle {
        match action {
            TradeAction::Long => "NEXT K: UP".to_string(),
            TradeAction::Short => "NEXT K: DOWN".to_string(),
            TradeAction::Wait | TradeAction::Avoid => "NEXT K: UNAVAILABLE".to_string(),
        }
    } else {
        action.to_string()
    }
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(default)]
struct PriceZone {
    low: Option<f64>,
    high: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(default)]
struct AnalysisResult {
    action: TradeAction,
    preferred_bias: TradeBias,
    execution_state: ExecutionState,
    bullish_score: f32,
    bearish_score: f32,
    market_regime: String,
    decision_summary: String,
    setup: String,
    confidence: f32,
    valid_for_seconds: u64,
    entry_zone: PriceZone,
    stop_loss: Option<f64>,
    targets: Vec<f64>,
    risk_reward: Option<f64>,
    support_levels: Vec<f64>,
    resistance_levels: Vec<f64>,
    key_levels: Vec<KeyLevel>,
    no_trade_reason: String,
    trigger_conditions: Vec<String>,
    execution_steps: Vec<String>,
    bullish_evidence: Vec<String>,
    bearish_evidence: Vec<String>,
    invalidation: Vec<String>,
    risks: Vec<String>,
    data_quality_notes: Vec<String>,
    recheck_conditions: Vec<String>,
}

impl Default for AnalysisResult {
    fn default() -> Self {
        Self {
            action: TradeAction::Wait,
            preferred_bias: TradeBias::Neutral,
            execution_state: ExecutionState::NoEdge,
            bullish_score: 0.0,
            bearish_score: 0.0,
            market_regime: String::new(),
            decision_summary: String::new(),
            setup: String::new(),
            confidence: 0.0,
            valid_for_seconds: 0,
            entry_zone: PriceZone::default(),
            stop_loss: None,
            targets: Vec::new(),
            risk_reward: None,
            support_levels: Vec::new(),
            resistance_levels: Vec::new(),
            key_levels: Vec::new(),
            no_trade_reason: String::new(),
            trigger_conditions: Vec::new(),
            execution_steps: Vec::new(),
            bullish_evidence: Vec::new(),
            bearish_evidence: Vec::new(),
            invalidation: Vec::new(),
            risks: Vec::new(),
            data_quality_notes: Vec::new(),
            recheck_conditions: Vec::new(),
        }
    }
}

impl AnalysisResult {
    fn normalize(&mut self) {
        self.bullish_score = self.bullish_score.clamp(0.0, 100.0);
        self.bearish_score = self.bearish_score.clamp(0.0, 100.0);
        self.key_levels.retain(|level| {
            level.price.is_finite() && level.price > 0.0 && level.strength.is_finite()
        });
        for level in &mut self.key_levels {
            level.strength = level.strength.clamp(0.0, 100.0);
        }
        self.key_levels
            .sort_by(|left, right| right.strength.total_cmp(&left.strength));
        let mut accepted = Vec::<KeyLevel>::new();
        for level in self.key_levels.drain(..) {
            let tolerance = level.price.abs() * 0.0005;
            if accepted
                .iter()
                .all(|existing| (existing.price - level.price).abs() > tolerance)
            {
                accepted.push(level);
            }
            if accepted.len() == 4 {
                break;
            }
        }
        self.key_levels = accepted;
        self.support_levels
            .retain(|price| price.is_finite() && *price > 0.0);
        self.resistance_levels
            .retain(|price| price.is_finite() && *price > 0.0);
        self.support_levels.truncate(2);
        self.resistance_levels.truncate(2);
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct AnalysisHistoryRecord {
    analyzed_at_ms: i64,
    provider: AiProvider,
    model: String,
    prompt_version: String,
    mode: AnalysisMode,
    symbol: String,
    venue: String,
    basis: String,
    snapshot_id: String,
    snapshot_generated_at_ms: i64,
    latest_price: Option<f64>,
    data_quality: f32,
    context_count: usize,
    latency_ms: u64,
    #[serde(default)]
    source_pane_id: Option<String>,
    #[serde(default)]
    snapshot_bar_time_ms: Option<u64>,
    #[serde(default)]
    raw_output: String,
    #[serde(default)]
    outcome: SignalOutcome,
    #[serde(default)]
    entry_triggered_at_ms: Option<u64>,
    analysis: AnalysisResult,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
enum SignalOutcome {
    #[default]
    Unscored,
    Pending,
    Win,
    Loss,
    Ambiguous,
    NoEntry,
}

fn signal_outcome_label(outcome: SignalOutcome, entered: bool) -> &'static str {
    match outcome {
        SignalOutcome::Unscored => "Not scored",
        SignalOutcome::Pending if entered => "Awaiting TP/SL",
        SignalOutcome::Pending => "Awaiting entry",
        SignalOutcome::Win => "TP hit",
        SignalOutcome::Loss => "SL hit",
        SignalOutcome::Ambiguous => "Ambiguous",
        SignalOutcome::NoEntry => "No entry",
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum HistoryPeriod {
    #[default]
    All,
    Day,
    Week,
    Month,
}

impl HistoryPeriod {
    const ALL: [Self; 4] = [Self::All, Self::Day, Self::Week, Self::Month];

    fn cutoff_ms(self, now_ms: i64) -> i64 {
        let duration = match self {
            Self::All => return i64::MIN,
            Self::Day => 24 * 60 * 60 * 1_000,
            Self::Week => 7 * 24 * 60 * 60 * 1_000,
            Self::Month => 30 * 24 * 60 * 60 * 1_000,
        };
        now_ms.saturating_sub(duration)
    }
}

impl fmt::Display for HistoryPeriod {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::All => "All time",
            Self::Day => "24 hours",
            Self::Week => "7 days",
            Self::Month => "30 days",
        })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ConfidenceFilter {
    #[default]
    All,
    AtLeast50,
    AtLeast60,
    AtLeast70,
    AtLeast80,
    AtLeast90,
}

impl ConfidenceFilter {
    const ALL: [Self; 6] = [
        Self::All,
        Self::AtLeast50,
        Self::AtLeast60,
        Self::AtLeast70,
        Self::AtLeast80,
        Self::AtLeast90,
    ];

    fn matches(self, confidence: f32) -> bool {
        let minimum = match self {
            Self::All => return true,
            Self::AtLeast50 => 50.0,
            Self::AtLeast60 => 60.0,
            Self::AtLeast70 => 70.0,
            Self::AtLeast80 => 80.0,
            Self::AtLeast90 => 90.0,
        };
        confidence >= minimum
    }
}

impl fmt::Display for ConfidenceFilter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::All => "All confidence",
            Self::AtLeast50 => "50%+",
            Self::AtLeast60 => "60%+",
            Self::AtLeast70 => "70%+",
            Self::AtLeast80 => "80%+",
            Self::AtLeast90 => "90%+",
        })
    }
}

fn sum_options(values: impl Iterator<Item = Option<f64>>) -> Option<f64> {
    let values = values.flatten().collect::<Vec<_>>();
    (!values.is_empty()).then(|| values.into_iter().sum())
}

fn local_levels(bars: &[MarketBar], depth: &[DepthLevel], last_price: f64) -> (Vec<f64>, Vec<f64>) {
    let depth_walls = |side: &str, below: bool| {
        let levels = depth
            .iter()
            .filter(|level| {
                level.side == side
                    && if below {
                        level.price < last_price
                    } else {
                        level.price > last_price
                    }
            })
            .collect::<Vec<_>>();
        if levels.len() < 3 {
            return Vec::new();
        }
        let mut quantities = levels
            .iter()
            .map(|level| level.quantity)
            .collect::<Vec<_>>();
        quantities.sort_by(f64::total_cmp);
        let threshold = quantities[quantities.len() / 2] * 2.0;
        let mut walls = levels
            .into_iter()
            .filter(|level| level.quantity >= threshold)
            .collect::<Vec<_>>();
        walls.sort_by(|left, right| right.quantity.total_cmp(&left.quantity));
        walls
            .into_iter()
            .take(2)
            .map(|level| level.price)
            .collect::<Vec<_>>()
    };
    let mut supports = depth_walls("bid", true);
    let mut resistances = depth_walls("ask", false);

    for window in bars.windows(5) {
        let center = &window[2];
        if center.low <= window[0].low
            && center.low <= window[1].low
            && center.low <= window[3].low
            && center.low <= window[4].low
            && center.low < last_price
        {
            supports.push(center.low);
        }
        if center.high >= window[0].high
            && center.high >= window[1].high
            && center.high >= window[3].high
            && center.high >= window[4].high
            && center.high > last_price
        {
            resistances.push(center.high);
        }
    }

    supports.sort_by(|a, b| (last_price - *a).total_cmp(&(last_price - *b)));
    resistances.sort_by(|a, b| (*a - last_price).total_cmp(&(*b - last_price)));
    dedup_levels(&mut supports, last_price);
    dedup_levels(&mut resistances, last_price);
    supports.truncate(5);
    resistances.truncate(5);
    (supports, resistances)
}

fn dedup_levels(levels: &mut Vec<f64>, reference: f64) {
    let tolerance = reference.abs() * 0.0005;
    let mut deduped = Vec::with_capacity(levels.len());
    for level in levels.drain(..) {
        if deduped
            .iter()
            .all(|existing: &f64| (*existing - level).abs() > tolerance)
        {
            deduped.push(level);
        }
    }
    *levels = deduped;
}

impl MarketSnapshot {
    fn latest_price(&self) -> Option<f64> {
        self.bars.last().map(|bar| bar.close)
    }
}

#[derive(Debug, Clone)]
pub enum Message {
    ProviderChanged(AiProvider),
    BaseUrlChanged(String),
    ModelChanged(String),
    FetchModels,
    ModelsFetched(Result<Vec<String>, String>),
    SourceChanged(ChartSource),
    ApiKeyChanged(String),
    SaveApiKey,
    ShortBarsChanged(String),
    LongBarsChanged(String),
    TimeoutChanged(String),
    ModeChanged(AnalysisMode),
    RefreshSnapshot,
    Analyze,
    AutoAnalyze,
    ToggleHistory,
    SelectHistory(usize),
    CloseHistoryDetail,
    ClearHistory,
    HistoryPeriodChanged(HistoryPeriod),
    HistorySymbolChanged(String),
    HistoryBasisChanged(String),
    HistoryModelChanged(String),
    HistoryConfidenceChanged(ConfidenceFilter),
    TogglePromptReview,
    PromptWrapChanged(bool),
    ToggleRawJson,
    AnalysisCompleted(Result<String, String>),
}

pub struct Agent {
    config: AiConfig,
    api_key: String,
    short_bars_input: String,
    long_bars_input: String,
    timeout_input: String,
    models: Vec<String>,
    sources: Vec<ChartSource>,
    selected_source: Option<uuid::Uuid>,
    analysis_source: Option<uuid::Uuid>,
    mode: AnalysisMode,
    snapshot: Option<AnalysisBundle>,
    prompt: String,
    result: String,
    analysis: Option<AnalysisResult>,
    analyzed_at: Option<std::time::Instant>,
    request_started_at: Option<std::time::Instant>,
    history: Vec<AnalysisHistoryRecord>,
    show_history: bool,
    selected_history: Option<usize>,
    history_period: HistoryPeriod,
    history_symbol: String,
    history_basis: String,
    history_model: String,
    history_confidence: ConfidenceFilter,
    show_prompt_review: bool,
    prompt_wrap: bool,
    show_raw_json: bool,
    status: String,
    is_loading: bool,
    is_fetching_models: bool,
    automatic_request: bool,
}

impl Default for Agent {
    fn default() -> Self {
        Self::new(AiConfig::default())
    }
}

impl Agent {
    pub fn new(mut config: AiConfig) -> Self {
        config.timeout_secs = config.timeout_secs.clamp(15, 600);
        let api_key = data::config::ai::load_api_key(&config).unwrap_or_default();
        Self {
            short_bars_input: config.short_bars.to_string(),
            long_bars_input: config.long_bars.to_string(),
            timeout_input: config.timeout_secs.to_string(),
            models: Vec::new(),
            sources: Vec::new(),
            selected_source: None,
            analysis_source: None,
            config,
            api_key,
            mode: AnalysisMode::default(),
            snapshot: None,
            prompt: String::new(),
            result: String::new(),
            analysis: None,
            analyzed_at: None,
            request_started_at: None,
            history: load_history(),
            show_history: false,
            selected_history: None,
            history_period: HistoryPeriod::All,
            history_symbol: "All symbols".to_string(),
            history_basis: "All timeframes".to_string(),
            history_model: "All models".to_string(),
            history_confidence: ConfidenceFilter::All,
            show_prompt_review: false,
            prompt_wrap: true,
            show_raw_json: false,
            status: "Select a Kline or Footprint pane, then refresh the snapshot.".to_string(),
            is_loading: false,
            is_fetching_models: false,
            automatic_request: false,
        }
    }

    pub fn config(&self) -> &AiConfig {
        &self.config
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn is_loading(&self) -> bool {
        self.is_loading
    }

    pub fn set_sources(&mut self, sources: Vec<ChartSource>) {
        let selected_exists = self
            .selected_source
            .is_some_and(|selected| sources.iter().any(|source| source.pane_id == selected));
        if !selected_exists {
            self.selected_source = sources.first().map(|source| source.pane_id);
            self.snapshot = None;
            self.prompt.clear();
        }
        self.sources = sources;
    }

    pub fn selected_source(&self) -> Option<uuid::Uuid> {
        self.selected_source
    }

    pub fn select_source(&mut self, pane_id: uuid::Uuid) -> bool {
        if self.sources.iter().any(|source| source.pane_id == pane_id) {
            self.selected_source = Some(pane_id);
            self.snapshot = None;
            self.prompt.clear();
            true
        } else {
            false
        }
    }

    pub(crate) fn analysis_source(&self) -> Option<uuid::Uuid> {
        self.analysis_source
    }

    pub(crate) fn chart_annotation(&self) -> Option<ChartAnnotation> {
        let analysis = self.analysis.as_ref()?;
        let analyzed_at = self.analyzed_at?;
        let snapshot = self.snapshot.as_ref()?;
        let expires_at =
            analyzed_at.checked_add(Duration::from_secs(analysis.valid_for_seconds))?;
        build_chart_annotation(
            analysis,
            snapshot.primary.latest_price(),
            snapshot.primary.bars.last().map(|bar| bar.time_ms),
            false,
            Some(expires_at),
        )
    }

    pub(crate) fn displayed_chart_annotation(&self) -> Option<(uuid::Uuid, ChartAnnotation)> {
        if self.show_history
            && let Some(record) = self
                .selected_history
                .and_then(|index| self.history.get(index))
        {
            let saved_source = record
                .source_pane_id
                .as_deref()
                .and_then(|source| source.parse().ok())
                .filter(|source| {
                    self.sources
                        .iter()
                        .any(|candidate| candidate.pane_id == *source)
                });
            let source = saved_source.or_else(|| {
                self.sources
                    .iter()
                    .find(|source| {
                        source.symbol == record.symbol
                            && source.venue == record.venue
                            && source.basis == record.basis
                    })
                    .map(|source| source.pane_id)
            })?;
            let time_ms = record.snapshot_bar_time_ms.or_else(|| {
                u64::try_from(record.snapshot_generated_at_ms)
                    .ok()
                    .filter(|value| *value > 0)
            });
            return build_chart_annotation(
                &record.analysis,
                record.latest_price,
                time_ms,
                true,
                None,
            )
            .map(|annotation| (source, annotation));
        }
        self.analysis_source.zip(self.chart_annotation())
    }

    pub fn bar_limits(&self) -> (usize, usize) {
        let short = usize::from(self.config.short_bars.clamp(10, 200));
        let long = usize::from(self.config.long_bars.clamp(self.config.short_bars, 500));
        (short, long)
    }

    pub(crate) fn take_auto_alert(&mut self) -> Option<AiAlert> {
        if !std::mem::take(&mut self.automatic_request) {
            return None;
        }
        let analysis = self.analysis.as_ref()?;
        let primary = &self.snapshot.as_ref()?.primary;
        let summary = if analysis.decision_summary.is_empty() {
            analysis.no_trade_reason.as_str()
        } else {
            analysis.decision_summary.as_str()
        };
        Some(AiAlert {
            title: format!("{} {}", primary.symbol, analysis.action),
            body: format!(
                "{} | confidence {:.0}% | {}",
                primary.basis,
                analysis.confidence.clamp(0.0, 100.0),
                summary
            ),
        })
    }

    pub fn update(
        &mut self,
        message: Message,
        snapshot: Option<Result<AnalysisBundle, String>>,
    ) -> Task<Message> {
        match message {
            Message::ProviderChanged(provider) => {
                let short_bars = self.config.short_bars;
                let long_bars = self.config.long_bars;
                let timeout_secs = self.config.timeout_secs;
                self.config = AiConfig::for_provider(provider);
                self.config.short_bars = short_bars;
                self.config.long_bars = long_bars;
                self.config.timeout_secs = timeout_secs;
                self.api_key = data::config::ai::load_api_key(&self.config).unwrap_or_default();
                self.models.clear();
                self.status = format!("Using {provider}. Refresh the snapshot before analysis.");
            }
            Message::BaseUrlChanged(value) => {
                self.config.base_url = value;
                self.models.clear();
            }
            Message::ModelChanged(value) => self.config.model = value,
            Message::FetchModels => {
                if self.api_key.trim().is_empty() && self.config.provider != AiProvider::Custom {
                    self.status = "API key is required to fetch models.".to_string();
                    return Task::none();
                }
                self.is_fetching_models = true;
                self.status = "Fetching available models...".to_string();
                return Task::perform(
                    fetch_models(
                        self.config.provider,
                        self.config.base_url.clone(),
                        self.api_key.clone(),
                        self.config.timeout_secs,
                    ),
                    Message::ModelsFetched,
                );
            }
            Message::ModelsFetched(result) => {
                self.is_fetching_models = false;
                match result {
                    Ok(models) => {
                        self.status = format!("Fetched {} available models.", models.len());
                        self.models = models;
                    }
                    Err(error) => {
                        log::warn!("AI analysis failed: {error}");
                        self.status = error;
                    }
                }
            }
            Message::SourceChanged(source) => {
                self.selected_source = Some(source.pane_id);
                self.selected_history = None;
                self.snapshot = None;
                self.prompt.clear();
                self.status = format!("Source changed to {}. Refresh the snapshot.", source.label);
            }
            Message::ApiKeyChanged(value) => self.api_key = value,
            Message::SaveApiKey => {
                self.status = match data::config::ai::save_api_key(&self.config, &self.api_key) {
                    Ok(()) => "API key stored in the system keychain.".to_string(),
                    Err(error) => error,
                };
            }
            Message::ShortBarsChanged(value) => {
                self.short_bars_input = value.clone();
                if let Ok(value) = value.parse::<u16>() {
                    self.config.short_bars = value.clamp(10, 200);
                }
            }
            Message::LongBarsChanged(value) => {
                self.long_bars_input = value.clone();
                if let Ok(value) = value.parse::<u16>() {
                    self.config.long_bars = value.clamp(20, 500);
                }
            }
            Message::TimeoutChanged(value) => {
                self.timeout_input = value.clone();
                if let Ok(value) = value.parse::<u16>() {
                    self.config.timeout_secs = value.clamp(15, 600);
                }
            }
            Message::ModeChanged(mode) => {
                self.mode = mode;
                self.snapshot = None;
                self.prompt.clear();
                self.result.clear();
                self.analysis = None;
                self.analyzed_at = None;
                self.status = "Mode changed. Refresh the market snapshot.".to_string();
            }
            Message::RefreshSnapshot => self.apply_snapshot(snapshot),
            message @ (Message::Analyze | Message::AutoAnalyze) => {
                let automatic = matches!(message, Message::AutoAnalyze);
                self.apply_snapshot(snapshot);
                if self.snapshot.is_none() {
                    return automatic_failure(automatic, &self.status);
                }
                if let Some(reason) = self.analysis_block_reason() {
                    self.status = reason;
                    return automatic_failure(automatic, &self.status);
                }
                if self.api_key.trim().is_empty() {
                    self.status = "API key is required.".to_string();
                    return automatic_failure(automatic, &self.status);
                }
                if self.config.base_url.trim().is_empty() || self.config.model.trim().is_empty() {
                    self.status = "Base URL and model are required.".to_string();
                    return automatic_failure(automatic, &self.status);
                }

                self.is_loading = true;
                self.automatic_request = automatic;
                self.analysis_source = self.selected_source;
                self.result.clear();
                self.analysis = None;
                self.analyzed_at = None;
                self.request_started_at = Some(std::time::Instant::now());
                self.status = "Analyzing current snapshot...".to_string();
                let request = ProviderRequest {
                    provider: self.config.provider,
                    mode: self.mode,
                    base_url: self.config.base_url.clone(),
                    model: self.config.model.clone(),
                    api_key: self.api_key.clone(),
                    timeout_secs: self.config.timeout_secs,
                    system_prompt: system_prompt(self.mode),
                    user_prompt: self.prompt.clone(),
                };
                return Task::perform(call_provider(request), Message::AnalysisCompleted);
            }
            Message::AnalysisCompleted(result) => {
                self.is_loading = false;
                match result {
                    Ok(result) => match serde_json::from_str::<AnalysisResult>(&result) {
                        Ok(mut analysis) => {
                            analysis.normalize();
                            // Entry is scored on bars after the signal bar, so a window shorter
                            // than one bar (e.g. 120s on 10m) would mark every signal NoEntry.
                            if analysis.valid_for_seconds > 0
                                && let Some(snapshot) = &self.snapshot
                            {
                                analysis.valid_for_seconds = analysis
                                    .valid_for_seconds
                                    .max(snapshot.primary.timeframe_ms / 1_000);
                            }
                            if self.mode == AnalysisMode::NextCandle
                                && !matches!(
                                    analysis.action,
                                    TradeAction::Long | TradeAction::Short
                                )
                            {
                                self.status =
                                    "Next K analysis returned no UP/DOWN direction.".to_string();
                                return Task::none();
                            }
                            self.analysis = Some(analysis);
                            self.analyzed_at = Some(std::time::Instant::now());
                            self.result = result;
                            self.status = self.record_analysis().map_or_else(
                                |error| format!("Analysis completed, but history failed: {error}"),
                                |()| {
                                    "Analysis completed. Verify validity before acting.".to_string()
                                },
                            );
                        }
                        Err(error) => {
                            self.status = format!("Invalid structured analysis: {error}");
                        }
                    },
                    Err(error) => self.status = error,
                }
            }
            Message::ToggleHistory => {
                self.show_history = !self.show_history;
                if !self.show_history {
                    self.selected_history = None;
                }
            }
            Message::SelectHistory(index) => {
                if index < self.history.len() {
                    self.selected_history = Some(index);
                }
            }
            Message::CloseHistoryDetail => self.selected_history = None,
            Message::TogglePromptReview => {
                self.show_prompt_review = !self.show_prompt_review;
                self.show_raw_json = false;
            }
            Message::PromptWrapChanged(value) => self.prompt_wrap = value,
            Message::ToggleRawJson => {
                self.show_raw_json = !self.show_raw_json;
                self.show_prompt_review = false;
            }
            Message::ClearHistory => match clear_history() {
                Ok(()) => {
                    self.history.clear();
                    self.selected_history = None;
                    self.status = "Analysis history cleared.".to_string();
                }
                Err(error) => self.status = error,
            },
            Message::HistoryPeriodChanged(value) => self.history_period = value,
            Message::HistorySymbolChanged(value) => self.history_symbol = value,
            Message::HistoryBasisChanged(value) => self.history_basis = value,
            Message::HistoryModelChanged(value) => self.history_model = value,
            Message::HistoryConfidenceChanged(value) => self.history_confidence = value,
        }
        Task::none()
    }

    fn record_analysis(&mut self) -> Result<(), String> {
        let snapshot = self
            .snapshot
            .as_ref()
            .ok_or_else(|| "Snapshot is unavailable.".to_string())?;
        let analysis = self
            .analysis
            .clone()
            .ok_or_else(|| "Structured analysis is unavailable.".to_string())?;
        let latency_ms = self
            .request_started_at
            .take()
            .map_or(0, |started| started.elapsed().as_millis() as u64);
        let primary = &snapshot.primary;
        self.history.push(AnalysisHistoryRecord {
            analyzed_at_ms: chrono::Utc::now().timestamp_millis(),
            provider: self.config.provider,
            model: self.config.model.clone(),
            prompt_version: PROMPT_VERSION.to_string(),
            mode: self.mode,
            symbol: primary.symbol.clone(),
            venue: primary.venue.clone(),
            basis: primary.basis.clone(),
            snapshot_id: snapshot_fingerprint(snapshot),
            snapshot_generated_at_ms: primary.generated_at_ms,
            latest_price: primary.latest_price(),
            data_quality: primary.data_quality,
            context_count: snapshot.higher_timeframe_contexts.len(),
            latency_ms,
            source_pane_id: self.analysis_source.map(|source| source.to_string()),
            snapshot_bar_time_ms: primary.bars.last().map(|bar| bar.time_ms),
            raw_output: self.result.clone(),
            outcome: SignalOutcome::Unscored,
            entry_triggered_at_ms: None,
            analysis,
        });
        if self.history.len() > MAX_HISTORY_RECORDS {
            self.history
                .drain(..self.history.len() - MAX_HISTORY_RECORDS);
        }
        save_history(&self.history)
    }

    fn apply_snapshot(&mut self, snapshot: Option<Result<AnalysisBundle, String>>) {
        let Some(snapshot) = snapshot else {
            self.status = "No Kline or Footprint pane is available.".to_string();
            return;
        };
        match snapshot {
            Ok(snapshot) => {
                if settle_history(&mut self.history, &snapshot.primary)
                    && let Err(error) = save_history(&self.history)
                {
                    log::warn!("Failed to save AI signal outcomes: {error}");
                }
                self.prompt = build_prompt(self.mode, &snapshot);
                self.status = format!(
                    "{} {} | {} bars + {} contexts | quality {:.0}%",
                    snapshot.primary.venue,
                    snapshot.primary.symbol,
                    snapshot.primary.bars.len(),
                    snapshot.higher_timeframe_contexts.len(),
                    snapshot.primary.data_quality * 100.0
                );
                self.snapshot = Some(snapshot);
            }
            Err(error) => {
                self.snapshot = None;
                self.prompt.clear();
                self.status = error;
            }
        }
    }

    fn analysis_block_reason(&self) -> Option<String> {
        let snapshot = &self.snapshot.as_ref()?.primary;
        if snapshot.data_quality < 0.35 {
            return Some(format!(
                "Analysis blocked: market data quality is only {:.0}%.",
                snapshot.data_quality * 100.0
            ));
        }
        if matches!(self.mode, AnalysisMode::Scalping | AnalysisMode::NextCandle)
            && snapshot.data_age_ms > snapshot.timeframe_ms.saturating_mul(2)
        {
            return Some(format!(
                "Analysis blocked: market data is {} seconds stale.",
                snapshot.data_age_ms / 1_000
            ));
        }
        None
    }

    pub fn settings_view(&self) -> Element<'_, Message> {
        let fetch_models = if self.is_fetching_models {
            icon_button(
                crate::style::LucideIcon::RefreshCw,
                "Fetching models...",
                None,
            )
        } else {
            icon_button(
                crate::style::LucideIcon::RefreshCw,
                "Fetch models",
                Some(Message::FetchModels),
            )
        };
        let model_selector: Element<'_, Message> = if self.models.is_empty() {
            row![].into()
        } else {
            column![
                text("Available models"),
                pick_list(
                    self.models.clone(),
                    Some(self.config.model.clone()),
                    Message::ModelChanged
                )
                .width(Length::Fill),
            ]
            .spacing(8)
            .into()
        };
        container(scrollable(
            column![
                row![
                    column![
                        text("Short bars"),
                        text_input("10-200", &self.short_bars_input)
                            .on_input(Message::ShortBarsChanged),
                    ]
                    .spacing(4)
                    .width(Length::Fill),
                    column![
                        text("Long bars"),
                        text_input("20-500", &self.long_bars_input)
                            .on_input(Message::LongBarsChanged),
                    ]
                    .spacing(4)
                    .width(Length::Fill),
                ]
                .spacing(8),
                row![
                    text("Request timeout (seconds)").width(Length::Fill),
                    text_input("15-600", &self.timeout_input)
                        .on_input(Message::TimeoutChanged)
                        .width(90),
                ]
                .spacing(8)
                .align_y(Alignment::Center),
                rule::horizontal(1),
                text("Provider"),
                pick_list(
                    AiProvider::ALL,
                    Some(self.config.provider),
                    Message::ProviderChanged
                )
                .width(Length::Fill),
                text_input("Provider base URL", &self.config.base_url)
                    .on_input(Message::BaseUrlChanged),
                row![
                    text_input("Model", &self.config.model)
                        .on_input(Message::ModelChanged)
                        .width(Length::Fill),
                    fetch_models,
                ]
                .spacing(8),
                model_selector,
                row![
                    text_input("API key", &self.api_key)
                        .secure(true)
                        .on_input(Message::ApiKeyChanged)
                        .width(Length::Fill),
                    icon_button(
                        crate::style::LucideIcon::KeyRound,
                        "Save API key to keychain",
                        Some(Message::SaveApiKey),
                    ),
                ]
                .spacing(8),
            ]
            .spacing(8),
        ))
        .width(Length::Shrink)
        .max_width(360)
        .padding(20)
        .style(crate::style::chart_modal)
        .into()
    }

    pub fn view(&self, timezone: data::UserTimezone) -> Element<'_, Message> {
        let analyze = if self.is_loading {
            icon_button(crate::style::LucideIcon::Sparkles, "Analyzing...", None)
        } else {
            icon_button(
                crate::style::LucideIcon::Sparkles,
                "Analyze",
                Some(Message::Analyze),
            )
        };
        let selected_source = self
            .selected_source
            .and_then(|id| self.sources.iter().find(|source| source.pane_id == id))
            .cloned();
        let source_selector = pick_list(
            self.sources.clone(),
            selected_source,
            Message::SourceChanged,
        )
        .placeholder("No Kline/Footprint panes")
        .width(Length::Fill);
        let controls = column![
            row![
                pick_list(AnalysisMode::ALL, Some(self.mode), Message::ModeChanged).width(104),
                source_selector,
                icon_button(
                    crate::style::LucideIcon::RefreshCw,
                    "Refresh chart source and snapshot",
                    Some(Message::RefreshSnapshot),
                ),
                analyze,
            ]
            .spacing(4)
            .align_y(Alignment::Center),
            text(&self.status),
        ]
        .spacing(10);
        let output = if self.show_history {
            self.history_view(timezone)
        } else {
            self.analysis_view()
        };
        let output_title = if self.show_history {
            "History"
        } else {
            "Analysis"
        };
        let clear_history: Element<'_, Message> = if self.show_history {
            icon_button(
                crate::style::LucideIcon::Trash2,
                "Clear history",
                Some(Message::ClearHistory),
            )
        } else {
            row![].into()
        };
        let base = container(
            column![
                controls,
                rule::horizontal(1),
                row![
                    row![text(output_title), clear_history]
                        .spacing(4)
                        .align_y(Alignment::Center)
                        .width(Length::Fill),
                    icon_button(
                        crate::style::LucideIcon::History,
                        if self.show_history {
                            "Show latest analysis"
                        } else {
                            "Show history"
                        },
                        Some(Message::ToggleHistory),
                    ),
                    icon_button(
                        crate::style::LucideIcon::FileText,
                        "Prompt review",
                        Some(Message::TogglePromptReview),
                    ),
                    icon_button(
                        crate::style::LucideIcon::Braces,
                        "Raw JSON",
                        Some(Message::ToggleRawJson),
                    ),
                ]
                .spacing(4)
                .align_y(Alignment::Center),
                scrollable(output).height(Length::Fill),
            ]
            .spacing(10),
        )
        .width(Length::Fill)
        .height(Length::Fill)
        .padding(16)
        .into();

        if self.show_prompt_review {
            let prompt = if self.prompt.is_empty() {
                "No prompt preview yet."
            } else {
                &self.prompt
            };
            let scrollbar = scrollable::Scrollbar::new().width(4).scroller_width(4);
            let direction = if self.prompt_wrap {
                scrollable::Direction::Vertical(scrollbar)
            } else {
                scrollable::Direction::Both {
                    vertical: scrollbar,
                    horizontal: scrollbar,
                }
            };
            let prompt_text = text(prompt).wrapping(if self.prompt_wrap {
                iced::widget::text::Wrapping::WordOrGlyph
            } else {
                iced::widget::text::Wrapping::None
            });
            let prompt_text = if self.prompt_wrap {
                prompt_text.width(Length::Fill)
            } else {
                prompt_text
            };
            let modal = container(
                column![
                    row![
                        text("Prompt review")
                            .size(crate::style::text_size::TITLE)
                            .width(Length::Fill),
                        checkbox(self.prompt_wrap)
                            .label("Wrap")
                            .on_toggle(Message::PromptWrapChanged),
                        icon_button(
                            crate::style::LucideIcon::X,
                            "Close",
                            Some(Message::TogglePromptReview),
                        ),
                    ]
                    .spacing(12)
                    .align_y(Alignment::Center),
                    scrollable(prompt_text)
                        .direction(direction)
                        .height(Length::Fill),
                ]
                .spacing(12),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(16)
            .style(crate::style::chart_modal);

            crate::modal::pane::stack_modal(
                base,
                modal,
                Message::TogglePromptReview,
                Padding::new(24.0),
                Alignment::Center,
            )
        } else if self.show_raw_json {
            let raw_json = if self.result.is_empty() {
                "No raw response yet."
            } else {
                &self.result
            };
            let modal = container(
                column![
                    row![
                        text("Raw JSON")
                            .size(crate::style::text_size::TITLE)
                            .width(Length::Fill),
                        icon_button(
                            crate::style::LucideIcon::X,
                            "Close",
                            Some(Message::ToggleRawJson),
                        ),
                    ]
                    .spacing(12)
                    .align_y(Alignment::Center),
                    scrollable(text(raw_json)).height(Length::Fill),
                ]
                .spacing(12),
            )
            .width(Length::Fill)
            .height(Length::Fill)
            .padding(16)
            .style(crate::style::chart_modal);

            crate::modal::pane::stack_modal(
                base,
                modal,
                Message::ToggleRawJson,
                Padding::new(24.0),
                Alignment::Center,
            )
        } else {
            base
        }
    }

    fn analysis_view(&self) -> Element<'_, Message> {
        let Some(analysis) = &self.analysis else {
            return text("No analysis result yet.").into();
        };
        let remaining = self.analyzed_at.map_or(0, |at| {
            analysis
                .valid_for_seconds
                .saturating_sub(at.elapsed().as_secs())
        });
        let validity = if remaining == 0 {
            "Expired".to_string()
        } else {
            format!("Valid for {remaining}s")
        };
        Self::analysis_details(analysis, validity, self.mode)
    }

    fn analysis_details(
        analysis: &AnalysisResult,
        validity: String,
        mode: AnalysisMode,
    ) -> Element<'_, Message> {
        let prices = |values: &[f64]| {
            if values.is_empty() {
                "n/a".to_string()
            } else {
                values
                    .iter()
                    .map(|value| format!("{value:.6}"))
                    .collect::<Vec<_>>()
                    .join(", ")
            }
        };
        let lines = |values: &[String]| {
            if values.is_empty() {
                "n/a".to_string()
            } else {
                values.join("\n")
            }
        };
        let entry = match (analysis.entry_zone.low, analysis.entry_zone.high) {
            (Some(low), Some(high)) => format!("{low:.6} - {high:.6}"),
            _ => "n/a".to_string(),
        };
        let key_levels = if analysis.key_levels.is_empty() {
            "n/a".to_string()
        } else {
            analysis
                .key_levels
                .iter()
                .map(|level| {
                    format!(
                        "{} {:.6} | {:.0}/100 | {}",
                        level.role, level.price, level.strength, level.reason
                    )
                })
                .collect::<Vec<_>>()
                .join("\n")
        };

        if mode == AnalysisMode::NextCandle {
            return column![
                text(analysis_title(mode, analysis.action)).size(crate::style::text_size::TITLE),
                row![
                    text(format!(
                        "Evidence strength {:.0}%",
                        analysis.confidence.clamp(0.0, 100.0)
                    )),
                    text(validity),
                ]
                .spacing(16)
                .align_y(Alignment::Center),
                text(format!(
                    "UP {:.0} / DOWN {:.0}",
                    analysis.bullish_score, analysis.bearish_score
                )),
                text(format!("Decision: {}", analysis.decision_summary)),
                text(format!("Regime: {}", analysis.market_regime)),
                text(format!("Support: {}", prices(&analysis.support_levels))),
                text(format!(
                    "Resistance: {}",
                    prices(&analysis.resistance_levels)
                )),
                text("Priority levels"),
                text(key_levels),
                text("Evidence for UP"),
                text(lines(&analysis.bullish_evidence)),
                text("Evidence for DOWN"),
                text(lines(&analysis.bearish_evidence)),
                text("Risks and data quality"),
                text(format!(
                    "{}\n{}",
                    lines(&analysis.risks),
                    lines(&analysis.data_quality_notes)
                )),
            ]
            .spacing(8)
            .into();
        }

        column![
            text(analysis_title(mode, analysis.action)).size(crate::style::text_size::TITLE),
            row![
                text(format!(
                    "Confidence {:.0}%",
                    analysis.confidence.clamp(0.0, 100.0)
                )),
                text(validity),
            ]
            .spacing(16)
            .align_y(Alignment::Center),
            text(format!("Bias: {}", analysis.preferred_bias)),
            text(format!(
                "State: {} | Bull {:.0} / Bear {:.0}",
                analysis.execution_state, analysis.bullish_score, analysis.bearish_score
            )),
            text(format!("Decision: {}", analysis.decision_summary)),
            text(format!("Setup: {}", analysis.setup)),
            text(format!("Regime: {}", analysis.market_regime)),
            text(format!("Entry: {entry}")),
            text(format!(
                "Stop: {}",
                analysis
                    .stop_loss
                    .map_or_else(|| "n/a".to_string(), |value| format!("{value:.6}"))
            )),
            text(format!(
                "R:R: {}",
                analysis
                    .risk_reward
                    .map_or_else(|| "n/a".to_string(), |value| format!("{value:.2}"))
            )),
            text(format!("Targets: {}", prices(&analysis.targets))),
            text(format!("Support: {}", prices(&analysis.support_levels))),
            text(format!(
                "Resistance: {}",
                prices(&analysis.resistance_levels)
            )),
            text("Priority levels"),
            text(key_levels),
            text(format!("Why wait: {}", analysis.no_trade_reason)),
            text("Trigger conditions"),
            text(lines(&analysis.trigger_conditions)),
            text("Execution guide"),
            text(lines(&analysis.execution_steps)),
            rule::horizontal(1),
            text("Bullish evidence"),
            text(lines(&analysis.bullish_evidence)),
            text("Bearish evidence"),
            text(lines(&analysis.bearish_evidence)),
            text("Invalidation"),
            text(lines(&analysis.invalidation)),
            text("Risks and data quality"),
            text(format!(
                "{}\n{}",
                lines(&analysis.risks),
                lines(&analysis.data_quality_notes)
            )),
            text("Recheck when"),
            text(lines(&analysis.recheck_conditions)),
        ]
        .spacing(8)
        .into()
    }

    fn history_view(&self, timezone: data::UserTimezone) -> Element<'_, Message> {
        if self.history.is_empty() {
            return text("No saved analyses.").into();
        }
        if let Some(record) = self
            .selected_history
            .and_then(|index| self.history.get(index))
        {
            let analyzed_at = timezone
                .format_with_kind(
                    record.analyzed_at_ms,
                    data::config::timezone::TimeLabelKind::Custom("%Y-%m-%d %H:%M:%S"),
                )
                .unwrap_or_else(|| "Unknown time".to_string());
            let snapshot_at = timezone
                .format_with_kind(
                    record.snapshot_generated_at_ms,
                    data::config::timezone::TimeLabelKind::Custom("%Y-%m-%d %H:%M:%S"),
                )
                .unwrap_or_else(|| "Unknown time".to_string());
            let raw_output = if record.raw_output.is_empty() {
                "Raw output was not stored for this older record."
            } else {
                &record.raw_output
            };
            return container(
                column![
                    row![
                        text(format!(
                            "{} {} {}",
                            analysis_title(record.mode, record.analysis.action),
                            record.symbol,
                            record.basis
                        ))
                        .size(crate::style::text_size::TITLE)
                        .width(Length::Fill),
                        icon_button(
                            crate::style::LucideIcon::X,
                            "Back to history",
                            Some(Message::CloseHistoryDetail),
                        ),
                    ]
                    .align_y(Alignment::Center),
                    text(format!(
                        "{} | {} | {} | {:.0}% quality | {} ms",
                        analyzed_at,
                        record.provider,
                        record.model,
                        record.data_quality * 100.0,
                        record.latency_ms
                    )),
                    text(format!(
                        "Snapshot {} | {} | {} contexts",
                        record.snapshot_id, snapshot_at, record.context_count
                    )),
                    text(format!(
                        "Signal outcome: {}",
                        signal_outcome_label(
                            record.outcome,
                            record.entry_triggered_at_ms.is_some()
                        )
                    )),
                    Self::analysis_details(
                        &record.analysis,
                        "Historical result".to_string(),
                        record.mode,
                    ),
                    rule::horizontal(1),
                    text("Raw output"),
                    text(raw_output),
                ]
                .spacing(8),
            )
            .padding(iced::padding::right(12))
            .into();
        }
        let options = |all: &str, values: Vec<&str>| {
            let mut values = values.into_iter().map(str::to_string).collect::<Vec<_>>();
            values.sort();
            values.dedup();
            values.insert(0, all.to_string());
            values
        };
        let symbols = options(
            "All symbols",
            self.history
                .iter()
                .map(|record| record.symbol.as_str())
                .collect(),
        );
        let bases = options(
            "All timeframes",
            self.history
                .iter()
                .map(|record| record.basis.as_str())
                .collect(),
        );
        let models = options(
            "All models",
            self.history
                .iter()
                .map(|record| record.model.as_str())
                .collect(),
        );
        let cutoff = self
            .history_period
            .cutoff_ms(chrono::Utc::now().timestamp_millis());
        let filtered = (0..self.history.len())
            .filter(|index| {
                let record = &self.history[*index];
                record.analyzed_at_ms >= cutoff
                    && (self.history_symbol == "All symbols"
                        || record.symbol == self.history_symbol)
                    && (self.history_basis == "All timeframes"
                        || record.basis == self.history_basis)
                    && (self.history_model == "All models" || record.model == self.history_model)
                    && self.history_confidence.matches(record.analysis.confidence)
            })
            .collect::<Vec<_>>();
        let wins = filtered
            .iter()
            .filter(|index| self.history[**index].outcome == SignalOutcome::Win)
            .count();
        let losses = filtered
            .iter()
            .filter(|index| self.history[**index].outcome == SignalOutcome::Loss)
            .count();
        let pending = filtered
            .iter()
            .filter(|index| self.history[**index].outcome == SignalOutcome::Pending)
            .count();
        let ambiguous = filtered
            .iter()
            .filter(|index| self.history[**index].outcome == SignalOutcome::Ambiguous)
            .count();
        let no_entry = filtered
            .iter()
            .filter(|index| self.history[**index].outcome == SignalOutcome::NoEntry)
            .count();
        let directional = filtered
            .iter()
            .filter(|index| {
                matches!(
                    self.history[**index].analysis.action,
                    TradeAction::Long | TradeAction::Short
                )
            })
            .count();
        let resolved = wins + losses;
        let accuracy = (resolved > 0).then(|| wins as f64 * 100.0 / resolved as f64);
        let filters = column![
            row![
                column![
                    text("Time").size(crate::style::text_size::SMALL),
                    pick_list(
                        HistoryPeriod::ALL,
                        Some(self.history_period),
                        Message::HistoryPeriodChanged,
                    )
                    .width(Length::Fill),
                ]
                .spacing(2)
                .width(Length::Fill),
                column![
                    text("Symbol").size(crate::style::text_size::SMALL),
                    pick_list(
                        symbols,
                        Some(self.history_symbol.clone()),
                        Message::HistorySymbolChanged,
                    )
                    .width(Length::Fill),
                ]
                .spacing(2)
                .width(Length::Fill),
            ]
            .spacing(6),
            row![
                column![
                    text("Timeframe").size(crate::style::text_size::SMALL),
                    pick_list(
                        bases,
                        Some(self.history_basis.clone()),
                        Message::HistoryBasisChanged,
                    )
                    .width(Length::Fill),
                ]
                .spacing(2)
                .width(Length::Fill),
                column![
                    text("Model").size(crate::style::text_size::SMALL),
                    pick_list(
                        models,
                        Some(self.history_model.clone()),
                        Message::HistoryModelChanged,
                    )
                    .width(Length::Fill),
                ]
                .spacing(2)
                .width(Length::Fill),
            ]
            .spacing(6),
            column![
                text("Minimum confidence").size(crate::style::text_size::SMALL),
                pick_list(
                    ConfidenceFilter::ALL,
                    Some(self.history_confidence),
                    Message::HistoryConfidenceChanged,
                )
                .width(Length::Fill),
            ]
            .spacing(2),
            text(format!(
                "Accuracy {} | {}W / {}L | {} pending | {} directional signals",
                accuracy.map_or_else(|| "--".to_string(), |value| format!("{value:.1}%")),
                wins,
                losses,
                pending,
                directional
            )),
            text(format!("{ambiguous} ambiguous | {no_entry} no entry"))
                .size(crate::style::text_size::SMALL),
            rule::horizontal(1),
        ]
        .spacing(6);
        let records =
            filtered
                .into_iter()
                .rev()
                .take(100)
                .fold(column![].spacing(8), |records, index| {
                    let record = &self.history[index];
                    let analyzed_at = timezone
                        .format_with_kind(
                            record.analyzed_at_ms,
                            data::config::timezone::TimeLabelKind::Custom("%Y-%m-%d %H:%M:%S"),
                        )
                        .unwrap_or_else(|| "Unknown time".to_string());
                    let outcome = record.outcome;
                    let outcome_label =
                        signal_outcome_label(outcome, record.entry_triggered_at_ms.is_some());
                    records.push(
                        column![
                            row![
                                text(format!(
                                    "{} {} {}",
                                    analysis_title(record.mode, record.analysis.action),
                                    record.symbol,
                                    record.basis
                                ))
                                .width(Length::Fill),
                                icon_button(
                                    crate::style::LucideIcon::FileText,
                                    "View analysis details",
                                    Some(Message::SelectHistory(index)),
                                ),
                            ]
                            .align_y(Alignment::Center)
                            .spacing(12),
                            row![
                                text(outcome_label).style(move |theme: &iced::Theme| {
                                    let palette = theme.extended_palette();
                                    iced::widget::text::Style {
                                        color: Some(match outcome {
                                            SignalOutcome::Win => palette.success.base.color,
                                            SignalOutcome::Loss => palette.danger.base.color,
                                            SignalOutcome::Pending => palette.primary.base.color,
                                            SignalOutcome::Ambiguous => palette.warning.base.color,
                                            SignalOutcome::NoEntry | SignalOutcome::Unscored => {
                                                palette.background.weak.text
                                            }
                                        }),
                                    }
                                }),
                                text(format!(
                                    "{} | {:.0}% | {} | {} ms",
                                    analyzed_at,
                                    record.analysis.confidence.clamp(0.0, 100.0),
                                    record.provider,
                                    record.latency_ms
                                ))
                                .width(Length::Fill),
                            ]
                            .spacing(10)
                            .align_y(Alignment::Center),
                            rule::horizontal(1),
                        ]
                        .spacing(4),
                    )
                });
        container(column![filters, records].spacing(8))
            .padding(iced::padding::right(12))
            .into()
    }
}

fn automatic_failure(automatic: bool, reason: &str) -> Task<Message> {
    if automatic {
        Task::done(Message::AnalysisCompleted(Err(reason.to_string())))
    } else {
        Task::none()
    }
}

pub fn build_prompt(mode: AnalysisMode, snapshot: &AnalysisBundle) -> String {
    let snapshot_json = serde_json::to_string(snapshot).unwrap_or_else(|_| "{}".to_string());
    let mode_task = match mode {
        AnalysisMode::NextCandle => format!(
            "Predict whether the immediately next {} candle closes above or below its own open. \
             LONG means UP and SHORT means DOWN. Produce one direction, not a trade setup. Base \
             the prediction on current price location, support/resistance, VWAP/POC, recent \
             footprint buy/sell volume, Delta/CVD, open interest and its change, order-book depth \
             and imbalance, liquidations, volatility, and higher-timeframe context. Weight the \
             most recent completed bars most heavily and do not use future information. The last \
             bar may still be forming; use it as current state but never treat its close as final. Set \
             valid_for_seconds to the primary timeframe length in seconds.",
            snapshot.primary.basis
        ),
        _ => "Evaluate the best current trade decision for the selected analysis horizon."
            .to_string(),
    };
    format!(
        "Prompt version: {PROMPT_VERSION}\nAnalysis mode: {mode}\n\
         Task: {mode_task}\n\
         Analyze only the supplied snapshot. Treat missing fields as unavailable, not zero. \
         Give extra weight to fresh order flow for Scalping, but use the longer series to avoid \
         trading directly into higher-timeframe structure. The primary snapshot is the execution \
         timeframe; higher_timeframe_contexts are directional context only. Each bar includes \
         OHLC, volume, buy/sell volume, Delta, relative CVD and OI when available. CVD starts at \
         zero before the first supplied bar. Depth, liquidations and deterministic features are \
         supplied separately. Deterministic support/resistance values are candidates, not facts; \
         reject nearby book noise and rank only levels confirmed by multiple independent inputs. \
         Return JSON only.\n\
         Market snapshots:\n{snapshot_json}"
    )
}

fn system_prompt(mode: AnalysisMode) -> String {
    let base = r#"You are a read-only crypto market microstructure decision-support analyst. You do not place orders. Use only supplied data and never invent news, levels, open interest, liquidity or probabilities.

Decision policy:
1. Classify regime, then score the best LONG and SHORT setup independently from 0 to 100 using only available evidence. Missing optional OI, depth, liquidation, or order-flow fields reduce confidence but must not by themselves veto a setup or force WAIT.
2. Return LONG or SHORT when one side scores at least 55 and leads the other by at least 10 points. A conditional setup is valid: use execution_state TRIGGER_PENDING until its stated trigger occurs. Use READY only when the entry trigger is already present.
3. Reward-to-risk of 1.5 is preferred, not a hard gate. Use structure-derived entry, stop and targets, report the actual ratio, and reject only plans with no positive net asymmetry after likely fees and slippage.
4. WAIT is an active conclusion, not the default. Use it only when both scores are below 55 or differ by less than 10, and set execution_state to NO_EDGE. Provide preferred_bias, the exact no_trade_reason, trigger_conditions that would activate a trade, and a conditional execution guide.
5. Use AVOID with execution_state UNAVAILABLE only for unusable/stale data or abnormal market conditions where analysis itself is unreliable.
6. Never turn ordinary adjacent order-book quotes into support/resistance. Return at most four key_levels, strongest first, only when supported by swing structure, VWAP/POC, repeated reaction, exceptional liquidity concentration, order flow, or higher-timeframe confluence. Strength is evidence quality from 0 to 100.
7. For LONG/SHORT provide a bounded entry, stop, at least one target, invalidation and concise numbered execution_steps. Do not claim certainty.

Return exactly one JSON object with this shape:
{"action":"LONG|SHORT|WAIT|AVOID","preferred_bias":"LONG|SHORT|NEUTRAL","execution_state":"READY|TRIGGER_PENDING|NO_EDGE|UNAVAILABLE","bullish_score":0,"bearish_score":0,"decision_summary":"string","setup":"string","market_regime":"string","confidence":0,"valid_for_seconds":0,"entry_zone":{"low":null,"high":null},"stop_loss":null,"targets":[],"risk_reward":null,"key_levels":[{"price":0,"role":"SUPPORT|RESISTANCE|PIVOT","strength":0,"reason":"string"}],"support_levels":[],"resistance_levels":[],"no_trade_reason":"string","trigger_conditions":[],"execution_steps":[],"bullish_evidence":[],"bearish_evidence":[],"invalidation":[],"risks":[],"data_quality_notes":[],"recheck_conditions":[]}

Confidence is evidence quality from 0 to 100, not win probability. Price levels must be numeric and derived from the snapshot. support_levels and resistance_levels are compatibility summaries of key_levels and must also be strongest-first with no more than two values each. For scalping, explicitly consider spread, depth imbalance, Delta/CVD behavior, OI change, liquidation pressure, nearby high-confluence structure and higher-timeframe conflict."#;
    if mode == AnalysisMode::NextCandle {
        format!(
            "{base}\n\nNext-candle override: predict one binary direction for the immediately next \
             primary-timeframe candle. Return LONG for an UP candle or SHORT for a DOWN candle. \
             Do not return WAIT. Rules about entry zones, reward-to-risk, targets and execution \
             steps do not apply; leave those fields empty or null. Set execution_state to READY. Use AVOID only when supplied \
             data is unusable. Confidence remains evidence quality, not a calibrated probability."
        )
    } else {
        base.to_string()
    }
}

#[derive(Clone)]
struct ProviderRequest {
    provider: AiProvider,
    mode: AnalysisMode,
    base_url: String,
    model: String,
    api_key: String,
    timeout_secs: u16,
    system_prompt: String,
    user_prompt: String,
}

async fn call_provider(request: ProviderRequest) -> Result<String, String> {
    let mode = request.mode;
    let content = call_provider_raw(request.clone()).await?;
    match normalize_analysis_for_mode(&content, mode) {
        Ok(result) => Ok(result),
        Err(first_error) => {
            log::warn!(
                "AI structured response validation failed: {first_error}; raw={}",
                truncate(&content, 2_000)
            );
            let repair = ProviderRequest {
                system_prompt: format!(
                    "{}\n\nRepair task: Correct only the JSON syntax, field types, and required schema. Do not invent market facts, prices, or recommendations. Return exactly one JSON object and no prose.",
                    system_prompt(mode)
                ),
                user_prompt: format!(
                    "Validation error: {first_error}\n\nOriginal model response:\n{}",
                    truncate(&content, 16_000)
                ),
                ..request
            };
            let repaired = call_provider_raw(repair).await.map_err(|error| {
                format!("Malformed AI result ({first_error}); repair request failed: {error}")
            })?;
            normalize_analysis_for_mode(&repaired, mode).map_err(|repair_error| {
                format!(
                    "Malformed AI result ({first_error}); repaired result is still invalid: {repair_error}"
                )
            })
        }
    }
}

async fn call_provider_raw(request: ProviderRequest) -> Result<String, String> {
    match request.provider {
        AiProvider::Anthropic => call_anthropic_raw(request).await,
        AiProvider::Gemini => call_gemini_raw(request).await,
        AiProvider::OpenAi
        | AiProvider::OpenRouter
        | AiProvider::DeepSeek
        | AiProvider::XAi
        | AiProvider::Groq
        | AiProvider::Mistral
        | AiProvider::Custom => call_openai_compatible_raw(request).await,
    }
}

async fn fetch_models(
    provider: AiProvider,
    base_url: String,
    api_key: String,
    timeout_secs: u16,
) -> Result<Vec<String>, String> {
    let base_url = provider_base_url(&base_url);
    let endpoint = match provider {
        AiProvider::Anthropic => format!("{base_url}/models?limit=1000"),
        AiProvider::Gemini => format!("{base_url}/models?pageSize=1000"),
        _ => format!("{base_url}/models"),
    };
    let mut request = ai_client(timeout_secs)?.get(endpoint);
    request = match provider {
        AiProvider::Anthropic => request
            .header("x-api-key", api_key)
            .header("anthropic-version", "2023-06-01"),
        AiProvider::Gemini => request.header("x-goog-api-key", api_key),
        _ if !api_key.trim().is_empty() => request.bearer_auth(api_key),
        _ => request,
    };
    let value = response_json(
        request
            .send()
            .await
            .map_err(|error| request_error(provider, error, timeout_secs))?,
        "Models",
    )
    .await?;
    let models = parse_model_ids(provider, &value);
    if models.is_empty() {
        Err("The provider returned no selectable models.".to_string())
    } else {
        Ok(models)
    }
}

fn provider_base_url(base_url: &str) -> String {
    let base_url = base_url.trim_end_matches('/');
    base_url
        .strip_suffix("/chat/completions")
        .or_else(|| base_url.strip_suffix("/messages"))
        .unwrap_or(base_url)
        .to_string()
}

fn parse_model_ids(provider: AiProvider, value: &Value) -> Vec<String> {
    let entries = match provider {
        AiProvider::Gemini => value.get("models"),
        _ => value.get("data"),
    }
    .and_then(Value::as_array);
    let mut models = entries
        .into_iter()
        .flatten()
        .filter(|entry| {
            provider != AiProvider::Gemini
                || entry
                    .get("supportedGenerationMethods")
                    .and_then(Value::as_array)
                    .is_none_or(|methods| methods.iter().any(|method| method == "generateContent"))
        })
        .filter_map(|entry| {
            let field = if provider == AiProvider::Gemini {
                "name"
            } else {
                "id"
            };
            entry
                .get(field)
                .and_then(Value::as_str)
                .map(|id| id.strip_prefix("models/").unwrap_or(id).to_string())
        })
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();
    models
}

fn openai_request_body(request: &ProviderRequest) -> Value {
    let mut body = json!({
        "model": request.model,
        "messages": [
            {"role": "system", "content": request.system_prompt},
            {"role": "user", "content": request.user_prompt}
        ]
    });
    let model = request.model.rsplit('/').next().unwrap_or(&request.model);
    if model.starts_with("gpt-5")
        || model.starts_with("o1")
        || model.starts_with("o3")
        || model.starts_with("o4")
    {
        body["max_completion_tokens"] = json!(3000);
    } else {
        body["max_tokens"] = json!(3000);
        body["temperature"] = json!(0.2);
    }
    body
}

async fn call_openai_compatible_raw(request: ProviderRequest) -> Result<String, String> {
    let endpoint = if request
        .base_url
        .trim_end_matches('/')
        .ends_with("/chat/completions")
    {
        request.base_url.trim_end_matches('/').to_string()
    } else {
        format!(
            "{}/chat/completions",
            request.base_url.trim_end_matches('/')
        )
    };
    let client = ai_client(request.timeout_secs)?;
    let body = openai_request_body(&request);
    let response = client
        .post(endpoint)
        .bearer_auth(request.api_key)
        .json(&body)
        .send()
        .await
        .map_err(|error| request_error(request.provider, error, request.timeout_secs))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Failed to read AI response: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "AI API returned {status}: {}",
            truncate(&body, 300)
        ));
    }
    let value: Value =
        serde_json::from_str(&body).map_err(|error| format!("Invalid AI API response: {error}"))?;
    extract_provider_content(request.provider, &value)
}

async fn call_anthropic_raw(request: ProviderRequest) -> Result<String, String> {
    let endpoint = if request
        .base_url
        .trim_end_matches('/')
        .ends_with("/messages")
    {
        request.base_url.trim_end_matches('/').to_string()
    } else {
        format!("{}/messages", request.base_url.trim_end_matches('/'))
    };
    let response = ai_client(request.timeout_secs)?
        .post(endpoint)
        .header("x-api-key", request.api_key)
        .header("anthropic-version", "2023-06-01")
        .json(&json!({
            "model": request.model,
            "max_tokens": 3000,
            "temperature": 0.2,
            "system": request.system_prompt,
            "messages": [{"role": "user", "content": request.user_prompt}]
        }))
        .send()
        .await
        .map_err(|error| request_error(AiProvider::Anthropic, error, request.timeout_secs))?;
    let value = response_json(response, "Anthropic").await?;
    extract_provider_content(AiProvider::Anthropic, &value)
}

async fn call_gemini_raw(request: ProviderRequest) -> Result<String, String> {
    let endpoint = format!(
        "{}/models/{}:generateContent",
        request.base_url.trim_end_matches('/'),
        request.model
    );
    let response = ai_client(request.timeout_secs)?
        .post(endpoint)
        .header("x-goog-api-key", request.api_key)
        .json(&json!({
            "system_instruction": {"parts": [{"text": request.system_prompt}]},
            "contents": [{"role": "user", "parts": [{"text": request.user_prompt}]}],
            "generationConfig": {
                "temperature": 0.2,
                "maxOutputTokens": 3000,
                "responseMimeType": "application/json"
            }
        }))
        .send()
        .await
        .map_err(|error| request_error(AiProvider::Gemini, error, request.timeout_secs))?;
    let value = response_json(response, "Gemini").await?;
    extract_provider_content(AiProvider::Gemini, &value)
}

fn request_timeout(timeout_secs: u16) -> Duration {
    Duration::from_secs(u64::from(timeout_secs.clamp(15, 600)))
}

fn ai_client(timeout_secs: u16) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(request_timeout(timeout_secs))
        .build()
        .map_err(|error| format!("Failed to create AI client: {error}"))
}

fn request_error(provider: AiProvider, error: reqwest::Error, timeout_secs: u16) -> String {
    if error.is_timeout() {
        format!(
            "{provider} request timed out after {} seconds.",
            request_timeout(timeout_secs).as_secs()
        )
    } else if error.is_connect() {
        format!("Could not connect to {provider}: {error}")
    } else {
        format!("{provider} request failed: {error}")
    }
}

async fn response_json(response: reqwest::Response, provider: &str) -> Result<Value, String> {
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|error| format!("Failed to read {provider} response: {error}"))?;
    if !status.is_success() {
        return Err(format!(
            "{provider} API returned {status}: {}",
            truncate(&body, 300)
        ));
    }
    serde_json::from_str(&body).map_err(|error| format!("Invalid {provider} response: {error}"))
}

fn extract_provider_content(provider: AiProvider, value: &Value) -> Result<String, String> {
    let parts: Vec<&Value> = match provider {
        AiProvider::Anthropic => value
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .collect(),
        AiProvider::Gemini => value
            .pointer("/candidates/0/content/parts")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .collect(),
        AiProvider::OpenAi
        | AiProvider::OpenRouter
        | AiProvider::DeepSeek
        | AiProvider::XAi
        | AiProvider::Groq
        | AiProvider::Mistral
        | AiProvider::Custom => {
            let content = value.pointer("/choices/0/message/content");
            if let Some(items) = content.and_then(Value::as_array) {
                items.iter().collect()
            } else {
                return content
                    .and_then(Value::as_str)
                    .map(str::to_string)
                    .ok_or_else(|| format!("{provider} response has no text content."));
            }
        }
    };
    let content = parts
        .iter()
        .filter_map(|part| {
            part.get("text")
                .and_then(Value::as_str)
                .or_else(|| part.as_str())
        })
        .collect::<Vec<_>>()
        .join("");
    (!content.trim().is_empty())
        .then_some(content)
        .ok_or_else(|| format!("{provider} response has no text content."))
}

fn normalize_analysis_for_mode(content: &str, mode: AnalysisMode) -> Result<String, String> {
    let json = extract_json_object(content)?;
    let value: Value = serde_json::from_str(json)
        .map_err(|error| format!("Model returned invalid JSON: {error}"))?;
    let action = value
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !matches!(action, "LONG" | "SHORT" | "WAIT" | "AVOID") {
        return Err("Model response has an invalid action.".to_string());
    }
    let has_text = |field: &str| {
        value
            .get(field)
            .and_then(Value::as_str)
            .is_some_and(|text| !text.trim().is_empty())
    };
    let has_items = |field: &str| {
        value
            .get(field)
            .and_then(Value::as_array)
            .is_some_and(|items| !items.is_empty())
    };
    if !has_text("decision_summary") {
        return Err("Model response is missing a decision summary.".to_string());
    }
    if mode == AnalysisMode::NextCandle && !matches!(action, "LONG" | "SHORT") {
        return Err("Next K response has no UP/DOWN direction.".to_string());
    }
    if mode != AnalysisMode::NextCandle {
        let bullish = value
            .get("bullish_score")
            .and_then(Value::as_f64)
            .ok_or_else(|| "Model response is missing bullish_score.".to_string())?;
        let bearish = value
            .get("bearish_score")
            .and_then(Value::as_f64)
            .ok_or_else(|| "Model response is missing bearish_score.".to_string())?;
        let state = value
            .get("execution_state")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let consistent = match action {
            "LONG" => bullish >= 55.0 && bullish - bearish >= 10.0
                && matches!(state, "READY" | "TRIGGER_PENDING"),
            "SHORT" => bearish >= 55.0 && bearish - bullish >= 10.0
                && matches!(state, "READY" | "TRIGGER_PENDING"),
            "WAIT" => {
                ((bullish < 55.0 && bearish < 55.0) || (bullish - bearish).abs() < 10.0)
                    && state == "NO_EDGE"
            }
            "AVOID" => state == "UNAVAILABLE",
            _ => false,
        };
        if !consistent {
            return Err("Action, scores, and execution_state conflict with the decision policy."
                .to_string());
        }
    }
    match action {
        "LONG" | "SHORT"
            if mode != AnalysisMode::NextCandle
                && (value
                    .pointer("/entry_zone/low")
                    .and_then(Value::as_f64)
                    .is_none()
                    || value
                        .pointer("/entry_zone/high")
                        .and_then(Value::as_f64)
                        .is_none()
                    || value.get("stop_loss").and_then(Value::as_f64).is_none()
                    || !has_items("targets")
                    || !has_items("execution_steps")) =>
        {
            return Err(
                "Directional analysis is missing entry, stop, target, or execution steps."
                    .to_string(),
            );
        }
        "WAIT"
            if !has_text("no_trade_reason")
                || !has_items("trigger_conditions")
                || !has_items("execution_steps") =>
        {
            return Err(
                "WAIT analysis is missing its reason, trigger conditions, or execution guide."
                    .to_string(),
            );
        }
        _ => {}
    }
    serde_json::to_string_pretty(&value).map_err(|error| error.to_string())
}

fn extract_json_object(content: &str) -> Result<&str, String> {
    let start = content
        .find('{')
        .ok_or_else(|| "Model response contains no JSON object.".to_string())?;
    let mut depth = 0_u32;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in content[start..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return Ok(&content[start..start + offset + ch.len_utf8()]);
                }
            }
            _ => {}
        }
    }
    Err("Model response contains a truncated JSON object.".to_string())
}

#[cfg(test)]
fn normalize_analysis(content: &str) -> Result<String, String> {
    normalize_analysis_for_mode(content, AnalysisMode::Scalping)
}

fn truncate(value: &str, max_chars: usize) -> String {
    value.chars().take(max_chars).collect()
}

fn snapshot_fingerprint(snapshot: &AnalysisBundle) -> String {
    let bytes = serde_json::to_vec(snapshot).unwrap_or_default();
    let hash = bytes
        .into_iter()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100000001b3)
        });
    format!("{hash:016x}")
}

fn settle_history(history: &mut [AnalysisHistoryRecord], snapshot: &MarketSnapshot) -> bool {
    let mut changed = false;
    for record in history.iter_mut().filter(|record| {
        record.symbol == snapshot.symbol
            && record.venue == snapshot.venue
            && record.basis == snapshot.basis
            && matches!(
                record.outcome,
                SignalOutcome::Unscored | SignalOutcome::Pending
            )
    }) {
        let (outcome, entered_at) = evaluate_signal(record, &snapshot.bars);
        if outcome != record.outcome || entered_at != record.entry_triggered_at_ms {
            record.outcome = outcome;
            record.entry_triggered_at_ms = entered_at;
            changed = true;
        }
    }
    changed
}

fn evaluate_signal(
    record: &AnalysisHistoryRecord,
    bars: &[MarketBar],
) -> (SignalOutcome, Option<u64>) {
    let is_long = match record.analysis.action {
        TradeAction::Long => true,
        TradeAction::Short => false,
        TradeAction::Wait | TradeAction::Avoid => return (SignalOutcome::Unscored, None),
    };
    let (Some(stop), Some(target), Some(signal_time)) = (
        record.analysis.stop_loss,
        record.analysis.targets.first().copied(),
        record.snapshot_bar_time_ms,
    ) else {
        return (SignalOutcome::Unscored, None);
    };
    let Some(entry) = record.latest_price else {
        return (SignalOutcome::Unscored, None);
    };
    let (entry_low, entry_high) = match (
        record.analysis.entry_zone.low,
        record.analysis.entry_zone.high,
    ) {
        (Some(low), Some(high)) => (low.min(high), low.max(high)),
        _ => (entry, entry),
    };
    let future = bars
        .iter()
        .filter(|bar| bar.time_ms > signal_time)
        .collect::<Vec<_>>();
    let deadline = signal_time.saturating_add(record.analysis.valid_for_seconds * 1_000);
    let entered_at = record.entry_triggered_at_ms.or_else(|| {
        future
            .iter()
            .find(|bar| {
                (record.analysis.valid_for_seconds == 0 || bar.time_ms <= deadline)
                    && bar.low <= entry_high
                    && bar.high >= entry_low
            })
            .map(|bar| bar.time_ms)
    });
    let Some(entered_at) = entered_at else {
        let outcome = if record.analysis.valid_for_seconds > 0
            && future.last().is_some_and(|bar| bar.time_ms > deadline)
        {
            SignalOutcome::NoEntry
        } else {
            SignalOutcome::Pending
        };
        return (outcome, None);
    };
    let entry_index = future
        .iter()
        .position(|bar| bar.time_ms >= entered_at)
        .unwrap_or(future.len());
    for bar in &future[entry_index..] {
        let hit_target = if is_long {
            bar.high >= target
        } else {
            bar.low <= target
        };
        let hit_stop = if is_long {
            bar.low <= stop
        } else {
            bar.high >= stop
        };
        match (hit_target, hit_stop) {
            (true, true) => return (SignalOutcome::Ambiguous, Some(entered_at)),
            (true, false) => return (SignalOutcome::Win, Some(entered_at)),
            (false, true) => return (SignalOutcome::Loss, Some(entered_at)),
            (false, false) => {}
        }
    }
    (SignalOutcome::Pending, Some(entered_at))
}

fn history_json(records: &[AnalysisHistoryRecord]) -> Result<String, String> {
    records
        .iter()
        .map(|record| serde_json::to_string(record).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()
        .map(|lines| lines.join("\n"))
}

fn load_history() -> Vec<AnalysisHistoryRecord> {
    let path = data::data_path(Some(ANALYSIS_HISTORY_PATH));
    let Ok(contents) = std::fs::read_to_string(path) else {
        return Vec::new();
    };
    let mut records = contents
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect::<Vec<_>>();
    if records.len() > MAX_HISTORY_RECORDS {
        records.drain(..records.len() - MAX_HISTORY_RECORDS);
    }
    records
}

fn save_history(records: &[AnalysisHistoryRecord]) -> Result<(), String> {
    let path = data::data_path(Some(ANALYSIS_HISTORY_PATH));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create AI history directory: {error}"))?;
    }
    std::fs::write(path, history_json(records)?)
        .map_err(|error| format!("Failed to save AI analysis history: {error}"))
}

fn clear_history() -> Result<(), String> {
    match std::fs::remove_file(data::data_path(Some(ANALYSIS_HISTORY_PATH))) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!("Failed to clear AI analysis history: {error}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_snapshot() -> MarketSnapshot {
        MarketSnapshot {
            generated_at_ms: 1,
            symbol: "BTCUSDT".into(),
            venue: "Binance".into(),
            market: "LinearPerps".into(),
            basis: "M1".into(),
            timeframe_ms: 60_000,
            latest_market_time_ms: 1,
            data_age_ms: 0,
            bars: vec![],
            depth: vec![],
            liquidations: LiquidationSummary {
                long_quantity: 0.0,
                short_quantity: 0.0,
                event_count: 0,
            },
            features: MarketFeatures::calculate(&[], &[], 30),
            data_quality: 0.5,
            missing_data: vec!["order_flow"],
        }
    }

    #[test]
    fn prompt_is_versioned_and_contains_snapshot_quality() {
        let primary = sample_snapshot();
        let snapshot = AnalysisBundle {
            primary,
            higher_timeframe_contexts: Vec::new(),
        };
        let prompt = build_prompt(AnalysisMode::Scalping, &snapshot);
        assert!(prompt.contains(PROMPT_VERSION));
        assert!(prompt.contains("BTCUSDT"));
        assert!(prompt.contains("order_flow"));
        assert!(prompt.contains("relative CVD"));
        assert!(
            system_prompt(AnalysisMode::Scalping)
                .contains("WAIT is an active conclusion, not the default")
        );
        assert!(system_prompt(AnalysisMode::Scalping).contains("must not by themselves veto"));
        assert!(system_prompt(AnalysisMode::Scalping).contains("bullish_score"));
        assert!(system_prompt(AnalysisMode::Scalping).contains("TRIGGER_PENDING"));
        assert!(system_prompt(AnalysisMode::Scalping).contains("at most four key_levels"));
    }

    #[test]
    fn next_candle_prompt_uses_the_source_timeframe_and_microstructure() {
        let snapshot = AnalysisBundle {
            primary: sample_snapshot(),
            higher_timeframe_contexts: Vec::new(),
        };
        let prompt = build_prompt(AnalysisMode::NextCandle, &snapshot);

        for expected in [
            "immediately next M1 candle",
            "LONG means UP",
            "Delta/CVD",
            "open interest",
            "order-book depth",
            "liquidations",
        ] {
            assert!(prompt.contains(expected), "missing {expected}");
        }
    }

    #[test]
    fn next_candle_mode_requires_an_up_or_down_result() {
        assert_eq!(
            analysis_title(AnalysisMode::NextCandle, TradeAction::Long),
            "NEXT K: UP"
        );
        assert_eq!(
            analysis_title(AnalysisMode::NextCandle, TradeAction::Short),
            "NEXT K: DOWN"
        );
        assert!(system_prompt(AnalysisMode::NextCandle).contains("Do not return WAIT"));

        let mut agent = Agent::default();
        agent.mode = AnalysisMode::NextCandle;
        drop(agent.update(
            Message::AnalysisCompleted(Ok(r#"{"action":"WAIT"}"#.to_string())),
            None,
        ));
        assert!(agent.analysis.is_none());
        assert!(agent.status.contains("no UP/DOWN direction"));
    }

    #[test]
    fn response_requires_a_supported_action() {
        assert!(normalize_analysis(r#"{"action":"WAIT"}"#).is_err());
        assert!(normalize_analysis(
            r#"{"action":"WAIT","execution_state":"NO_EDGE","bullish_score":50,"bearish_score":45,"decision_summary":"No edge yet","no_trade_reason":"Delta conflicts with OI","trigger_conditions":["CVD breaks higher"],"execution_steps":["Wait for confirmation"]}"#
        ).is_ok());
        assert!(normalize_analysis(
            r#"{"action":"WAIT","execution_state":"NO_EDGE","bullish_score":70,"bearish_score":40,"decision_summary":"No edge yet","no_trade_reason":"Mixed flow","trigger_conditions":["Break"],"execution_steps":["Wait"]}"#
        ).is_err());
        assert!(normalize_analysis(r#"{"action":"BUY"}"#).is_err());
        let parsed: AnalysisResult = serde_json::from_str(
            r#"{"action":"LONG","confidence":72,"valid_for_seconds":30,"entry_zone":{"low":99.5,"high":100.0}}"#,
        )
        .unwrap();
        assert_eq!(parsed.action, TradeAction::Long);
        assert_eq!(parsed.entry_zone.low, Some(99.5));
        assert_eq!(parsed.execution_state, ExecutionState::NoEdge);
    }

    #[test]
    fn next_candle_response_does_not_require_a_trade_plan() {
        let prediction = r#"{"action":"LONG","decision_summary":"Order flow favors an up close","confidence":68,"bullish_evidence":["Positive Delta and rising OI"],"bearish_evidence":["Nearby resistance"]}"#;

        assert!(normalize_analysis_for_mode(prediction, AnalysisMode::NextCandle).is_ok());
        assert!(normalize_analysis_for_mode(prediction, AnalysisMode::Scalping).is_err());
    }

    #[test]
    fn analysis_keeps_only_strong_distinct_priority_levels() {
        let mut analysis: AnalysisResult = serde_json::from_str(
            r#"{"key_levels":[{"price":100.0,"role":"SUPPORT","strength":55,"reason":"weak"},{"price":100.01,"role":"SUPPORT","strength":90,"reason":"same zone"},{"price":105.0,"role":"RESISTANCE","strength":80,"reason":"pivot"},{"price":110.0,"role":"RESISTANCE","strength":70,"reason":"HTF"},{"price":95.0,"role":"SUPPORT","strength":60,"reason":"POC"}]}"#,
        )
        .unwrap();

        analysis.normalize();

        assert_eq!(analysis.key_levels.len(), 4);
        assert_eq!(analysis.key_levels[0].price, 100.01);
        assert_eq!(analysis.key_levels[1].price, 105.0);
    }

    #[test]
    fn features_find_nearby_book_levels_and_depth_imbalance() {
        let bars = [MarketBar {
            time_ms: 1,
            open: 100.0,
            high: 101.0,
            low: 99.0,
            close: 100.0,
            volume: 10.0,
            buy_volume: Some(7.0),
            sell_volume: Some(3.0),
            delta: Some(4.0),
            cvd: Some(4.0),
            open_interest: Some(1000.0),
        }];
        let depth = [
            DepthLevel {
                side: "bid",
                price: 99.5,
                quantity: 8.0,
            },
            DepthLevel {
                side: "ask",
                price: 100.5,
                quantity: 2.0,
            },
        ];
        let features = MarketFeatures::calculate(&bars, &depth, 30);
        assert!(features.support_levels.is_empty());
        assert!(features.resistance_levels.is_empty());
        assert_eq!(features.depth_imbalance, Some(0.6));
        assert_eq!(features.recent_delta, Some(4.0));
        assert_eq!(features.vwap, Some(100.0));
    }

    #[test]
    fn ordinary_book_ladders_are_not_treated_as_structure() {
        let bars = [
            MarketBar {
                time_ms: 1,
                open: 100.0,
                high: 101.0,
                low: 99.0,
                close: 100.0,
                volume: 1.0,
                buy_volume: None,
                sell_volume: None,
                delta: None,
                cvd: None,
                open_interest: None,
            },
            MarketBar {
                time_ms: 2,
                open: 100.0,
                high: 100.5,
                low: 98.5,
                close: 99.0,
                volume: 1.0,
                buy_volume: None,
                sell_volume: None,
                delta: None,
                cvd: None,
                open_interest: None,
            },
            MarketBar {
                time_ms: 3,
                open: 99.0,
                high: 100.0,
                low: 98.0,
                close: 99.0,
                volume: 1.0,
                buy_volume: None,
                sell_volume: None,
                delta: None,
                cvd: None,
                open_interest: None,
            },
            MarketBar {
                time_ms: 4,
                open: 99.0,
                high: 100.5,
                low: 98.5,
                close: 100.0,
                volume: 1.0,
                buy_volume: None,
                sell_volume: None,
                delta: None,
                cvd: None,
                open_interest: None,
            },
            MarketBar {
                time_ms: 5,
                open: 100.0,
                high: 101.0,
                low: 99.0,
                close: 100.0,
                volume: 1.0,
                buy_volume: None,
                sell_volume: None,
                delta: None,
                cvd: None,
                open_interest: None,
            },
        ];
        let depth = (1..=5)
            .flat_map(|index| {
                [
                    DepthLevel {
                        side: "bid",
                        price: 100.0 - f64::from(index) * 0.1,
                        quantity: 10.0,
                    },
                    DepthLevel {
                        side: "ask",
                        price: 100.0 + f64::from(index) * 0.1,
                        quantity: 10.0,
                    },
                ]
            })
            .collect::<Vec<_>>();

        let (supports, resistances) = local_levels(&bars, &depth, 100.0);
        assert_eq!(supports, vec![98.0]);
        assert!(resistances.is_empty());
    }

    #[test]
    fn relative_cvd_accumulates_available_delta() {
        let mut bars = vec![
            MarketBar {
                time_ms: 1,
                open: 1.0,
                high: 1.0,
                low: 1.0,
                close: 1.0,
                volume: 1.0,
                buy_volume: Some(1.0),
                sell_volume: Some(0.0),
                delta: Some(1.0),
                cvd: None,
                open_interest: None,
            },
            MarketBar {
                time_ms: 2,
                open: 1.0,
                high: 1.0,
                low: 1.0,
                close: 1.0,
                volume: 2.0,
                buy_volume: Some(0.0),
                sell_volume: Some(2.0),
                delta: Some(-2.0),
                cvd: None,
                open_interest: None,
            },
        ];

        populate_relative_cvd(&mut bars);
        assert_eq!(bars[0].cvd, Some(1.0));
        assert_eq!(bars[1].cvd, Some(-1.0));
    }

    #[test]
    fn scalping_blocks_stale_or_low_quality_snapshots() {
        let mut agent = Agent::default();
        let mut primary = sample_snapshot();
        primary.data_quality = 0.8;
        primary.data_age_ms = 121_000;
        agent.snapshot = Some(AnalysisBundle {
            primary,
            higher_timeframe_contexts: Vec::new(),
        });
        assert!(agent.analysis_block_reason().unwrap().contains("stale"));

        agent.snapshot.as_mut().unwrap().primary.data_age_ms = 0;
        agent.snapshot.as_mut().unwrap().primary.data_quality = 0.2;
        assert!(agent.analysis_block_reason().unwrap().contains("quality"));
    }

    #[test]
    fn provider_response_formats_extract_the_same_json_content() {
        let content = r#"{"action":"WAIT"}"#;
        let fixtures = [
            (
                AiProvider::OpenAi,
                json!({"choices": [{"message": {"content": content}}]}),
            ),
            (
                AiProvider::Anthropic,
                json!({"content": [{"type": "text", "text": content}]}),
            ),
            (
                AiProvider::Gemini,
                json!({"candidates": [{"content": {"parts": [{"text": content}]}}]}),
            ),
        ];
        for (provider, fixture) in fixtures {
            assert_eq!(
                extract_provider_content(provider, &fixture).unwrap(),
                content
            );
        }

        let multipart = json!({"choices": [{"message": {"content": [
            {"type": "text", "text": "{\"action\":"},
            {"type": "text", "text": "\"WAIT\"}"}
        ]}}]});
        assert_eq!(
            extract_provider_content(AiProvider::OpenAi, &multipart).unwrap(),
            content
        );
    }

    #[test]
    fn structured_response_extracts_json_from_prose_and_fences() {
        let response = r#"Result follows:
```json
{"action":"WAIT","execution_state":"NO_EDGE","bullish_score":50,"bearish_score":45,"decision_summary":"No edge","no_trade_reason":"Mixed flow","trigger_conditions":["Break"],"execution_steps":["Wait"]}
```
Do not trade yet."#;
        let normalized = normalize_analysis(response).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&normalized).unwrap()["action"],
            "WAIT"
        );
        assert!(extract_json_object("prefix {\"x\":\"} inside\"} suffix").is_ok());
        assert!(
            extract_json_object("{\"x\":1")
                .unwrap_err()
                .contains("truncated")
        );
    }

    #[test]
    fn provider_model_lists_are_normalized() {
        assert_eq!(
            parse_model_ids(
                AiProvider::OpenAi,
                &json!({"data": [{"id": "gpt-5"}, {"id": "gpt-4.1"}]})
            ),
            vec!["gpt-4.1", "gpt-5"]
        );
        assert_eq!(
            parse_model_ids(
                AiProvider::Gemini,
                &json!({"models": [
                    {"name": "models/gemini-2.5-flash", "supportedGenerationMethods": ["generateContent"]},
                    {"name": "models/embedding-001", "supportedGenerationMethods": ["embedContent"]}
                ]})
            ),
            vec!["gemini-2.5-flash"]
        );
    }

    #[test]
    fn request_timeout_is_configurable_and_bounded() {
        assert_eq!(request_timeout(300), Duration::from_secs(300));
        assert_eq!(request_timeout(0), Duration::from_secs(15));
        assert_eq!(request_timeout(1_000), Duration::from_secs(600));
    }

    #[test]
    fn reasoning_models_use_completion_token_limit() {
        let request = |model: &str| ProviderRequest {
            provider: AiProvider::OpenAi,
            mode: AnalysisMode::Scalping,
            base_url: String::new(),
            model: model.to_string(),
            api_key: String::new(),
            timeout_secs: 180,
            system_prompt: String::new(),
            user_prompt: String::new(),
        };
        let gpt5 = openai_request_body(&request("gpt-5"));
        assert_eq!(gpt5["max_completion_tokens"], 3000);
        assert!(gpt5.get("max_tokens").is_none());
        assert!(gpt5.get("temperature").is_none());

        let legacy = openai_request_body(&request("gpt-4.1-mini"));
        assert_eq!(legacy["max_tokens"], 3000);
        assert_eq!(legacy["temperature"], 0.2);
    }

    #[test]
    fn history_is_compact_and_contains_no_prompt_or_secret_fields() {
        let analysis: AnalysisResult = serde_json::from_str(r#"{"action":"WAIT"}"#).unwrap();
        let record = AnalysisHistoryRecord {
            analyzed_at_ms: 1,
            provider: AiProvider::OpenAi,
            model: "test-model".into(),
            prompt_version: PROMPT_VERSION.into(),
            mode: AnalysisMode::Scalping,
            symbol: "BTCUSDT".into(),
            venue: "Binance".into(),
            basis: "M1".into(),
            snapshot_id: "0123456789abcdef".into(),
            snapshot_generated_at_ms: 1,
            latest_price: Some(100.0),
            data_quality: 0.9,
            context_count: 1,
            latency_ms: 50,
            source_pane_id: None,
            snapshot_bar_time_ms: Some(1),
            raw_output: r#"{"action":"WAIT"}"#.into(),
            outcome: SignalOutcome::Unscored,
            entry_triggered_at_ms: None,
            analysis,
        };
        let json = history_json(&[record]).unwrap();
        assert!(json.contains("BTCUSDT"));
        assert!(json.contains("raw_output"));
        for forbidden in [
            "api_key",
            "user_prompt",
            "system_prompt",
            "\"bars\"",
            "\"depth\"",
        ] {
            assert!(!json.contains(forbidden));
        }

        let legacy = r#"{"analyzed_at_ms":1,"provider":"OpenAi","model":"test","prompt_version":"v1","mode":"Scalping","symbol":"BTCUSDT","venue":"Binance","basis":"M1","snapshot_id":"id","snapshot_generated_at_ms":1,"latest_price":100.0,"data_quality":0.9,"context_count":1,"latency_ms":50,"analysis":{"action":"WAIT"}}"#;
        let record: AnalysisHistoryRecord = serde_json::from_str(legacy).unwrap();
        assert!(record.raw_output.is_empty());
        assert!(record.source_pane_id.is_none());
    }

    #[test]
    fn historical_annotation_recovers_source_without_saved_pane_id() {
        let pane_id = uuid::Uuid::new_v4();
        let mut agent = Agent::new(AiConfig::default());
        agent.sources = vec![ChartSource {
            pane_id,
            label: "BTCUSDT | M1 | Kline".into(),
            symbol: "BTCUSDT".into(),
            venue: "Binance".into(),
            basis: "M1".into(),
        }];
        agent.history = vec![AnalysisHistoryRecord {
            analyzed_at_ms: 1,
            provider: AiProvider::OpenAi,
            model: "test-model".into(),
            prompt_version: PROMPT_VERSION.into(),
            mode: AnalysisMode::Scalping,
            symbol: "BTCUSDT".into(),
            venue: "Binance".into(),
            basis: "M1".into(),
            snapshot_id: "id".into(),
            snapshot_generated_at_ms: 1,
            latest_price: Some(100.0),
            data_quality: 0.9,
            context_count: 0,
            latency_ms: 0,
            source_pane_id: None,
            snapshot_bar_time_ms: Some(1),
            raw_output: String::new(),
            outcome: SignalOutcome::Unscored,
            entry_triggered_at_ms: None,
            analysis: AnalysisResult::default(),
        }];
        agent.show_history = true;
        drop(agent.update(Message::SelectHistory(0), None));

        assert_eq!(
            agent.displayed_chart_annotation().map(|(source, _)| source),
            Some(pane_id)
        );
    }

    #[test]
    fn signal_outcome_uses_entry_then_first_tp_or_sl_touch() {
        let legacy = r#"{"analyzed_at_ms":1,"provider":"OpenAi","model":"test","prompt_version":"v1","mode":"Scalping","symbol":"BTCUSDT","venue":"Binance","basis":"M1","snapshot_id":"id","snapshot_generated_at_ms":1,"latest_price":100.0,"data_quality":0.9,"context_count":1,"latency_ms":50,"snapshot_bar_time_ms":1000,"analysis":{"action":"LONG","entry_zone":{"low":99.0,"high":101.0},"stop_loss":95.0,"targets":[110.0],"valid_for_seconds":60}}"#;
        let record: AnalysisHistoryRecord = serde_json::from_str(legacy).unwrap();
        let bar = |time_ms, low, high| MarketBar {
            time_ms,
            open: 100.0,
            high,
            low,
            close: 100.0,
            volume: 1.0,
            buy_volume: None,
            sell_volume: None,
            delta: None,
            cvd: None,
            open_interest: None,
        };

        assert_eq!(
            evaluate_signal(
                &record,
                &[
                    bar(1_000, 90.0, 120.0),
                    bar(2_000, 99.0, 101.0),
                    bar(3_000, 100.0, 111.0)
                ]
            ),
            (SignalOutcome::Win, Some(2_000))
        );
        assert_eq!(
            evaluate_signal(&record, &[bar(2_000, 94.0, 111.0)]),
            (SignalOutcome::Ambiguous, Some(2_000))
        );
        assert_eq!(
            evaluate_signal(&record, &[bar(62_000, 99.0, 101.0)]),
            (SignalOutcome::NoEntry, None)
        );
    }

    #[test]
    fn confidence_filter_applies_minimum_inclusively() {
        assert!(ConfidenceFilter::All.matches(0.0));
        assert!(ConfidenceFilter::AtLeast70.matches(70.0));
        assert!(!ConfidenceFilter::AtLeast70.matches(69.9));
    }

    #[test]
    fn prompt_wrap_setting_updates_review_layout_mode() {
        let mut agent = Agent::new(AiConfig::default());
        drop(agent.update(Message::PromptWrapChanged(false), None));
        assert!(!agent.prompt_wrap);
    }

    #[test]
    fn history_outcome_labels_distinguish_entry_state() {
        assert_eq!(signal_outcome_label(SignalOutcome::Win, true), "TP hit");
        assert_eq!(signal_outcome_label(SignalOutcome::Loss, true), "SL hit");
        assert_eq!(
            signal_outcome_label(SignalOutcome::Pending, true),
            "Awaiting TP/SL"
        );
        assert_eq!(
            signal_outcome_label(SignalOutcome::Pending, false),
            "Awaiting entry"
        );
        assert_eq!(
            signal_outcome_label(SignalOutcome::NoEntry, false),
            "No entry"
        );
    }
}
