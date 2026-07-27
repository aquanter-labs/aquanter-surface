use crate::{aggr::time::DataPoint, chart::indicator::KlineIndicator};
use exchange::{
    Kline, Trade, UnixMs,
    unit::price::{Price, PriceStep},
    unit::qty::Qty,
};

use rustc_hash::FxHashMap;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct KlineDataPoint {
    pub kline: Kline,
    pub footprint: KlineTrades,
}

impl KlineDataPoint {
    pub fn max_cluster_qty(&self, cluster_kind: ClusterKind, highest: Price, lowest: Price) -> Qty {
        self.footprint
            .max_cluster_qty(cluster_kind, highest, lowest)
    }

    pub fn add_trade(&mut self, trade: &Trade, step: PriceStep) {
        self.footprint.add_trade_to_nearest_bin(trade, step);
    }

    pub fn poc_price(&self) -> Option<Price> {
        self.footprint.poc_price()
    }

    pub fn set_poc_status(&mut self, status: NPoc) {
        self.footprint.set_poc_status(status);
    }

    pub fn clear_trades(&mut self) {
        self.footprint.clear();
    }

    pub fn calculate_poc(&mut self) {
        self.footprint.calculate_poc();
    }

    pub fn last_trade_time(&self) -> Option<UnixMs> {
        self.footprint.last_trade_t()
    }

    pub fn first_trade_time(&self) -> Option<UnixMs> {
        self.footprint.first_trade_t()
    }

    pub fn volume_delta(&self) -> Qty {
        if self.kline.volume.is_directional() {
            self.kline.volume.delta()
        } else if !self.footprint.trades.is_empty() {
            self.footprint
                .trades
                .values()
                .fold(Qty::ZERO, |acc, group| acc + group.delta_qty())
        } else {
            Qty::ZERO
        }
    }

    /// Whether this datapoint has directional (buy vs sell) data.
    pub fn is_directional(&self) -> bool {
        !self.footprint.trades.is_empty() || self.kline.volume.is_directional()
    }
}

impl DataPoint for KlineDataPoint {
    fn add_trade(&mut self, trade: &Trade, step: PriceStep) {
        self.add_trade(trade, step);
    }

    fn clear_trades(&mut self) {
        self.clear_trades();
    }

    fn last_trade_time(&self) -> Option<UnixMs> {
        self.last_trade_time()
    }

    fn first_trade_time(&self) -> Option<UnixMs> {
        self.first_trade_time()
    }

    fn last_price(&self) -> Price {
        self.kline.close
    }

    fn kline(&self) -> Option<&Kline> {
        Some(&self.kline)
    }

    fn value_high(&self) -> Price {
        self.kline.high
    }

    fn value_low(&self) -> Price {
        self.kline.low
    }
}

#[derive(Debug, Clone, Default)]
pub struct GroupedTrades {
    pub buy_qty: Qty,
    pub sell_qty: Qty,
    pub first_time: UnixMs,
    pub last_time: UnixMs,
    pub buy_count: usize,
    pub sell_count: usize,
}

impl GroupedTrades {
    fn new(trade: &Trade) -> Self {
        Self {
            buy_qty: if trade.is_sell {
                Qty::default()
            } else {
                trade.qty
            },
            sell_qty: if trade.is_sell {
                trade.qty
            } else {
                Qty::default()
            },
            first_time: trade.time,
            last_time: trade.time,
            buy_count: if trade.is_sell { 0 } else { 1 },
            sell_count: if trade.is_sell { 1 } else { 0 },
        }
    }

    fn add_trade(&mut self, trade: &Trade) {
        if trade.is_sell {
            self.sell_qty += trade.qty;
            self.sell_count += 1;
        } else {
            self.buy_qty += trade.qty;
            self.buy_count += 1;
        }
        self.last_time = trade.time;
    }

    pub fn total_qty(&self) -> Qty {
        self.buy_qty + self.sell_qty
    }

    pub fn delta_qty(&self) -> Qty {
        self.buy_qty - self.sell_qty
    }

    pub fn max_cluster_qty(&self, cluster_kind: ClusterKind) -> Qty {
        match cluster_kind {
            ClusterKind::BidAsk | ClusterKind::Table => self.buy_qty.max(self.sell_qty),
            ClusterKind::DeltaProfile => self.buy_qty.abs_diff(self.sell_qty),
            ClusterKind::VolumeProfile => self.total_qty(),
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct KlineTrades {
    pub trades: FxHashMap<Price, GroupedTrades>,
    pub poc: Option<PointOfControl>,
}

impl KlineTrades {
    pub fn new() -> Self {
        Self {
            trades: FxHashMap::default(),
            poc: None,
        }
    }

    pub fn first_trade_t(&self) -> Option<UnixMs> {
        self.trades.values().map(|group| group.first_time).min()
    }

    pub fn last_trade_t(&self) -> Option<UnixMs> {
        self.trades.values().map(|group| group.last_time).max()
    }

    /// Add trade to the bin at the step multiple computed with side-based rounding.
    /// Intended for order-book ladder/quotes; Floor for sells, ceil for buys.
    /// Introduces side bias at bin edges and should not be used for OHLC/footprint aggregation
    pub fn add_trade_to_side_bin(&mut self, trade: &Trade, step: PriceStep) {
        let price = trade.price.round_to_side_step(trade.is_sell, step);

        self.trades
            .entry(price)
            .and_modify(|group| group.add_trade(trade))
            .or_insert_with(|| GroupedTrades::new(trade));
    }

    /// Add trade to the bin at the nearest step multiple (side-agnostic).
    /// Ties (exactly half a step) round up to the higher multiple.
    /// Intended for footprint/OHLC trade aggregation
    pub fn add_trade_to_nearest_bin(&mut self, trade: &Trade, step: PriceStep) {
        let price = trade.price.round_to_step(step);

        self.trades
            .entry(price)
            .and_modify(|group| group.add_trade(trade))
            .or_insert_with(|| GroupedTrades::new(trade));
    }

    /// Max of some extracted qty across price levels within [`lowest`, `highest`].
    pub fn max_qty_by<F>(&self, highest: Price, lowest: Price, f: F) -> Qty
    where
        F: Fn(&GroupedTrades) -> Qty,
    {
        let mut max_qty = Qty::default();
        for (price, group) in &self.trades {
            if *price >= lowest && *price <= highest {
                max_qty = max_qty.max(f(group));
            }
        }
        max_qty
    }

    /// Max cluster qty considering only price levels within [`lowest`, `highest`].
    /// Used by the aggregate visible-range computation where the viewport may
    /// show only a subset of each bar's full price range (y-pan/zoom).
    pub fn max_cluster_qty(&self, cluster_kind: ClusterKind, highest: Price, lowest: Price) -> Qty {
        self.max_qty_by(highest, lowest, |group| group.max_cluster_qty(cluster_kind))
    }

    /// Max cluster qty across all price levels in this bar (unfiltered).
    /// Used for per-bar individual scaling, the full bar should contribute.
    pub fn max_cluster_qty_all(&self, cluster_kind: ClusterKind) -> Qty {
        self.trades
            .values()
            .map(|group| group.max_cluster_qty(cluster_kind))
            .max()
            .unwrap_or_default()
    }

    pub fn calculate_poc(&mut self) {
        if self.trades.is_empty() {
            return;
        }

        let mut max_volume = Qty::ZERO;
        let mut poc_price = Price::from_f32(0.0);

        for (price, group) in &self.trades {
            let total_volume = group.total_qty();
            if total_volume > max_volume {
                max_volume = total_volume;
                poc_price = *price;
            }
        }

        self.poc = Some(PointOfControl {
            price: poc_price,
            volume: max_volume,
            status: NPoc::default(),
        });
    }

    pub fn set_poc_status(&mut self, status: NPoc) {
        if let Some(poc) = &mut self.poc {
            poc.status = status;
        }
    }

    pub fn poc_price(&self) -> Option<Price> {
        self.poc.map(|poc| poc.price)
    }

    pub fn clear(&mut self) {
        self.trades.clear();
        self.poc = None;
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct FootprintSummary {
    pub buy: Qty,
    pub sell: Qty,
    pub total: Qty,
    pub delta: Qty,
    pub delta_pct: f64,
}

impl FootprintSummary {
    pub fn new(buy: Qty, sell: Qty) -> Self {
        let total = buy + sell;
        let delta = buy - sell;
        let total_f = total.to_f64();
        let delta_pct = if total_f > 0.0 {
            (delta.to_f64() / total_f) * 100.0
        } else {
            0.0
        };

        Self {
            buy,
            sell,
            total,
            delta,
            delta_pct,
        }
    }

    pub fn from_trades(footprint: &KlineTrades) -> Option<Self> {
        if footprint.trades.is_empty() {
            return None;
        }

        let (buy, sell) = footprint
            .trades
            .values()
            .fold((Qty::ZERO, Qty::ZERO), |(buy, sell), group| {
                (buy + group.buy_qty, sell + group.sell_qty)
            });

        Some(Self::new(buy, sell))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub enum KlineChartKind {
    #[default]
    Candles,
    Footprint {
        clusters: ClusterKind,
        #[serde(default)]
        scaling: ClusterScaling,
        studies: Vec<FootprintStudy>,
    },
}

impl KlineChartKind {
    pub fn allows_indicator(&self, indicator: KlineIndicator) -> bool {
        !matches!(
            (self, indicator),
            (KlineChartKind::Candles, KlineIndicator::BarAnalysis)
        )
    }

    pub fn min_scaling(&self) -> f32 {
        match self {
            KlineChartKind::Footprint { .. } => 0.4,
            KlineChartKind::Candles => 0.6,
        }
    }

    pub fn max_scaling(&self) -> f32 {
        match self {
            KlineChartKind::Footprint { .. } => 1.2,
            KlineChartKind::Candles => 2.5,
        }
    }

    pub fn max_cell_width(&self) -> f32 {
        match self {
            KlineChartKind::Footprint { .. } => 360.0,
            KlineChartKind::Candles => 16.0,
        }
    }

    pub fn min_cell_width(&self) -> f32 {
        match self {
            KlineChartKind::Footprint { .. } => 32.0,
            KlineChartKind::Candles => 1.0,
        }
    }

    pub fn max_cell_height(&self) -> f32 {
        match self {
            KlineChartKind::Footprint { .. } => 90.0,
            KlineChartKind::Candles => 8.0,
        }
    }

    pub fn min_cell_height(&self) -> f32 {
        match self {
            KlineChartKind::Footprint { .. } => 1.0,
            KlineChartKind::Candles => 0.001,
        }
    }

    pub fn default_cell_width(&self) -> f32 {
        match self {
            KlineChartKind::Footprint { .. } => 80.0,
            KlineChartKind::Candles => 4.0,
        }
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Default, Deserialize, Serialize)]
pub enum ClusterKind {
    #[default]
    BidAsk,
    VolumeProfile,
    DeltaProfile,
    Table,
}

impl ClusterKind {
    pub const ALL: [ClusterKind; 4] = [
        ClusterKind::BidAsk,
        ClusterKind::VolumeProfile,
        ClusterKind::DeltaProfile,
        ClusterKind::Table,
    ];

    /// Minimum footprint cell width (in unscaled pixels) for the cluster rendering mode.
    pub fn min_footprint_width(self) -> f32 {
        match self {
            ClusterKind::VolumeProfile | ClusterKind::DeltaProfile => 80.0,
            ClusterKind::BidAsk => 120.0,
            ClusterKind::Table => 100.0,
        }
    }
}

impl std::fmt::Display for ClusterKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClusterKind::BidAsk => write!(f, "Bid/Ask"),
            ClusterKind::VolumeProfile => write!(f, "Volume Profile"),
            ClusterKind::DeltaProfile => write!(f, "Delta Profile"),
            ClusterKind::Table => write!(f, "Table"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct Config {
    // Whether to show last value labels on top right/left when not hovering
    // e.g. OHLC/bar change values for the main chart, or last value of an indicator series
    pub data_labels_always_visible: bool,
    // Whether to show the footprint per-bar summary below each candle.
    pub show_footprint_summary: bool,
    pub show_footprint_orderbook: bool,
    pub quantity_format: QuantityFormat,
    pub session_vwap: SessionVwapConfig,
    pub anchored_vwap: AnchoredVwapConfig,
    pub ema: Vec<EmaConfig>,
    pub volume_profile: VolumeProfileConfig,
    pub fixed_volume_profile: VolumeProfileConfig,
    pub stoch_rsi: StochRsiConfig,
    pub open_interest: IndicatorSeriesConfig,
    pub cumulative_delta: IndicatorSeriesConfig,
    pub oi_delta_divergence: OiDeltaDivergenceConfig,
    pub liquidation_pressure: LiquidationPressureConfig,
    pub liquidation_heatmap: LiquidationHeatmapConfig,
    pub oi_flow_net: OiFlowNetConfig,
    pub oi_weighted_delta: OiWeightedDeltaConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            data_labels_always_visible: false,
            show_footprint_summary: false,
            show_footprint_orderbook: false,
            quantity_format: QuantityFormat::default(),
            session_vwap: SessionVwapConfig::default(),
            anchored_vwap: AnchoredVwapConfig::default(),
            ema: vec![EmaConfig::default()],
            volume_profile: VolumeProfileConfig::default(),
            fixed_volume_profile: VolumeProfileConfig::default(),
            stoch_rsi: StochRsiConfig::default(),
            open_interest: IndicatorSeriesConfig::default(),
            cumulative_delta: IndicatorSeriesConfig::default(),
            oi_delta_divergence: OiDeltaDivergenceConfig::default(),
            liquidation_pressure: LiquidationPressureConfig::default(),
            liquidation_heatmap: LiquidationHeatmapConfig::default(),
            oi_flow_net: OiFlowNetConfig::default(),
            oi_weighted_delta: OiWeightedDeltaConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub enum QuantityRounding {
    #[default]
    Round,
    Truncate,
}

impl QuantityRounding {
    pub const ALL: [Self; 2] = [Self::Round, Self::Truncate];
}

impl std::fmt::Display for QuantityRounding {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Round => write!(f, "Round"),
            Self::Truncate => write!(f, "Truncate"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct QuantityFormat {
    pub decimals: u8,
    pub rounding: QuantityRounding,
}

impl Default for QuantityFormat {
    fn default() -> Self {
        Self {
            decimals: 2,
            rounding: QuantityRounding::Round,
        }
    }
}

impl QuantityFormat {
    pub fn format(self, value: f64) -> String {
        let (divisor, suffix) = match value.abs() {
            value if value >= 1_000_000_000.0 => (1_000_000_000.0, "b"),
            value if value >= 1_000_000.0 => (1_000_000.0, "m"),
            value if value >= 1_000.0 => (1_000.0, "k"),
            _ => (1.0, ""),
        };
        let decimals = self.decimals.min(6);
        let factor = 10_f64.powi(i32::from(decimals));
        let scaled = value / divisor * factor;
        let adjusted = match self.rounding {
            QuantityRounding::Round => scaled.round(),
            QuantityRounding::Truncate => scaled.trunc(),
        } / factor;
        let adjusted = if adjusted == 0.0 { 0.0 } else { adjusted };
        format!("{:.*}{suffix}", usize::from(decimals), adjusted)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub enum IndicatorPlotStyle {
    #[default]
    Line,
    Candles,
}

impl IndicatorPlotStyle {
    pub const ALL: [Self; 2] = [Self::Line, Self::Candles];
}

impl std::fmt::Display for IndicatorPlotStyle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Line => write!(f, "Line"),
            Self::Candles => write!(f, "Candles"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct IndicatorSeriesConfig {
    pub style: IndicatorPlotStyle,
    pub line_width: f32,
    pub line_color: iced_core::Color,
    pub up_color: iced_core::Color,
    pub down_color: iced_core::Color,
}

impl Default for IndicatorSeriesConfig {
    fn default() -> Self {
        Self {
            style: IndicatorPlotStyle::Line,
            line_width: 1.5,
            line_color: iced_core::Color::from_rgb(0.43, 0.63, 0.72),
            up_color: iced_core::Color::from_rgb(0.35, 0.68, 0.50),
            down_color: iced_core::Color::from_rgb(0.76, 0.40, 0.42),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct StochRsiConfig {
    pub rsi_length: u16,
    pub stochastic_length: u16,
    pub k_smoothing: u16,
    pub d_smoothing: u16,
    pub k_color: iced_core::Color,
    pub d_color: iced_core::Color,
    pub line_width: f32,
}

impl Default for StochRsiConfig {
    fn default() -> Self {
        Self {
            rsi_length: 14,
            stochastic_length: 14,
            k_smoothing: 3,
            d_smoothing: 3,
            k_color: iced_core::Color::from_rgb(0.43, 0.63, 0.72),
            d_color: iced_core::Color::from_rgb(0.72, 0.58, 0.38),
            line_width: 1.5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct OiDeltaDivergenceConfig {
    pub lookback: u16,
    pub rsi_period: u16,
    pub marker_size: f32,
    pub bullish_color: iced_core::Color,
    pub bearish_color: iced_core::Color,
    pub cvd_color: iced_core::Color,
    pub oi_color: iced_core::Color,
    pub line_width: f32,
}

impl Default for OiDeltaDivergenceConfig {
    fn default() -> Self {
        Self {
            lookback: 20,
            rsi_period: 14,
            marker_size: 4.0,
            bullish_color: iced_core::Color::from_rgb(0.35, 0.68, 0.50),
            bearish_color: iced_core::Color::from_rgb(0.76, 0.40, 0.42),
            cvd_color: iced_core::Color::from_rgb(0.43, 0.63, 0.72),
            oi_color: iced_core::Color::from_rgb(0.72, 0.58, 0.38),
            line_width: 1.5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct LiquidationPressureConfig {
    pub bar_width: f32,
    pub positive_color: iced_core::Color,
    pub negative_color: iced_core::Color,
    pub lookback: u16,
    pub oi_drop_threshold: f32,
    pub delta_zscore: f32,
}

impl Default for LiquidationPressureConfig {
    fn default() -> Self {
        Self {
            bar_width: 0.8,
            positive_color: iced_core::Color::from_rgb(0.35, 0.68, 0.50),
            negative_color: iced_core::Color::from_rgb(0.76, 0.40, 0.42),
            lookback: 20,
            oi_drop_threshold: 2.0,
            delta_zscore: 2.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct OiFlowNetConfig {
    pub basis_length: u16,
    pub burst_sensitivity: u8,
    pub static_color: bool,
    pub show_price_line: bool,
    pub long_color: iced_core::Color,
    pub short_color: iced_core::Color,
    pub buy_outflow_color: iced_core::Color,
    pub sell_outflow_color: iced_core::Color,
    pub extreme_band_color: iced_core::Color,
}

impl Default for OiFlowNetConfig {
    fn default() -> Self {
        Self {
            basis_length: 10,
            burst_sensitivity: 3,
            static_color: false,
            show_price_line: false,
            long_color: iced_core::Color::from_rgb(0.35, 0.68, 0.50),
            short_color: iced_core::Color::from_rgb(0.76, 0.40, 0.42),
            buy_outflow_color: iced_core::Color::from_rgb(0.28, 0.64, 0.64),
            sell_outflow_color: iced_core::Color::from_rgb(0.55, 0.30, 0.36),
            extreme_band_color: iced_core::Color::from_rgb(0.72, 0.74, 0.77),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct OiWeightedDeltaConfig {
    pub sensitivity: f32,
    pub max_weight: f32,
    pub bar_width: f32,
    pub positive_color: iced_core::Color,
    pub negative_color: iced_core::Color,
}

impl Default for OiWeightedDeltaConfig {
    fn default() -> Self {
        Self {
            sensitivity: 10.0,
            max_weight: 2.0,
            bar_width: 0.8,
            positive_color: iced_core::Color::from_rgb(0.35, 0.68, 0.50),
            negative_color: iced_core::Color::from_rgb(0.76, 0.40, 0.42),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct LiquidationHeatmapConfig {
    pub row_height: f32,
    pub opacity: f32,
    pub long_color: iced_core::Color,
    pub short_color: iced_core::Color,
}

impl Default for LiquidationHeatmapConfig {
    fn default() -> Self {
        Self {
            row_height: 8.0,
            opacity: 0.9,
            long_color: iced_core::Color::from_rgb(0.76, 0.40, 0.42),
            short_color: iced_core::Color::from_rgb(0.35, 0.68, 0.50),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct AnchoredVwapConfig {
    pub anchor: Option<exchange::UnixMs>,
    pub deviation_bands: u8,
    pub color: iced_core::Color,
    pub line_width: f32,
}

impl Default for AnchoredVwapConfig {
    fn default() -> Self {
        Self {
            anchor: None,
            deviation_bands: 2,
            color: iced_core::Color::from_rgb(0.95, 0.45, 0.35),
            line_width: 1.5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub enum VolumeProfileRange {
    #[default]
    Visible,
    PastDuration,
}

impl VolumeProfileRange {
    pub const ALL: [Self; 2] = [Self::Visible, Self::PastDuration];
}

impl std::fmt::Display for VolumeProfileRange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Visible => write!(f, "Visible"),
            Self::PastDuration => write!(f, "Past Duration"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub enum VolumeProfilePastEnd {
    #[default]
    ChartRight,
    LatestData,
}

impl VolumeProfilePastEnd {
    pub const ALL: [Self; 2] = [Self::ChartRight, Self::LatestData];
}

impl std::fmt::Display for VolumeProfilePastEnd {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ChartRight => write!(f, "Chart Right"),
            Self::LatestData => write!(f, "Latest Data"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub enum VolumeProfileDisplay {
    #[default]
    Volume,
    Delta,
}

impl VolumeProfileDisplay {
    pub const ALL: [Self; 2] = [Self::Volume, Self::Delta];
}

impl std::fmt::Display for VolumeProfileDisplay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Volume => write!(f, "Volume"),
            Self::Delta => write!(f, "Delta"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct VolumeProfileConfig {
    pub range: VolumeProfileRange,
    pub past_duration_end: VolumeProfilePastEnd,
    pub display: VolumeProfileDisplay,
    pub timeframe_multiplier: u8,
    pub rows: u16,
    pub max_width_percent: u8,
}

impl Default for VolumeProfileConfig {
    fn default() -> Self {
        Self {
            range: VolumeProfileRange::Visible,
            past_duration_end: VolumeProfilePastEnd::ChartRight,
            display: VolumeProfileDisplay::Volume,
            timeframe_multiplier: 10,
            rows: 120,
            max_width_percent: 30,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct EmaConfig {
    pub period: u16,
    pub color: iced_core::Color,
    pub line_width: f32,
}

impl Default for EmaConfig {
    fn default() -> Self {
        Self::for_index(0)
    }
}

impl EmaConfig {
    pub fn for_index(index: usize) -> Self {
        const COLORS: [iced_core::Color; 6] = [
            iced_core::Color::from_rgb(0.72, 0.50, 0.48),
            iced_core::Color::from_rgb(0.43, 0.63, 0.72),
            iced_core::Color::from_rgb(0.56, 0.67, 0.47),
            iced_core::Color::from_rgb(0.68, 0.57, 0.75),
            iced_core::Color::from_rgb(0.75, 0.64, 0.42),
            iced_core::Color::from_rgb(0.42, 0.70, 0.66),
        ];
        Self {
            period: 20,
            color: COLORS[index % COLORS.len()],
            line_width: 1.5,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct SessionVwapConfig {
    pub anchor_hour_utc: u8,
    pub deviation_bands: u8,
    pub color: iced_core::Color,
    pub line_width: f32,
}

impl Default for SessionVwapConfig {
    fn default() -> Self {
        Self {
            anchor_hour_utc: 0,
            deviation_bands: 3,
            color: iced_core::Color::from_rgb(0.2, 0.7, 0.95),
            line_width: 1.5,
        }
    }
}

#[derive(Default, Clone, Copy, Debug, PartialEq, Deserialize, Serialize)]
pub enum ClusterScaling {
    #[default]
    /// Scale based on the maximum quantity in the visible range.
    VisibleRange,
    /// Blend global VisibleRange and per-cluster Individual using a weight in [0.0, 1.0].
    /// weight = fraction of global contribution (1.0 == all-global, 0.0 == all-individual).
    Hybrid { weight: f32 },
    /// Scale based only on the maximum quantity inside the datapoint (per-candle).
    Datapoint,
}

impl ClusterScaling {
    pub const ALL: [ClusterScaling; 3] = [
        ClusterScaling::VisibleRange,
        ClusterScaling::Hybrid { weight: 0.2 },
        ClusterScaling::Datapoint,
    ];

    /// Blend the global visible-range max qty with the per-candle individual max qty
    /// according to the scaling strategy.
    pub fn effective_qty(self, visible_max: f64, individual_max: Qty) -> f64 {
        match self {
            ClusterScaling::VisibleRange => Qty::scale_or_one(visible_max),
            ClusterScaling::Datapoint => individual_max.to_scale_or_one(),
            ClusterScaling::Hybrid { weight } => {
                let w = weight.clamp(0.0, 1.0) as f64;
                Qty::scale_or_one(visible_max * w + individual_max.to_f64() * (1.0 - w))
            }
        }
    }
}

impl std::fmt::Display for ClusterScaling {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClusterScaling::VisibleRange => write!(f, "Visible Range"),
            ClusterScaling::Hybrid { weight } => write!(f, "Hybrid (weight: {:.2})", weight),
            ClusterScaling::Datapoint => write!(f, "Per-candle"),
        }
    }
}

impl std::cmp::Eq for ClusterScaling {}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum FootprintStudy {
    NPoC {
        lookback: usize,
    },
    Imbalance {
        threshold: usize,
        color_scale: Option<usize>,
        ignore_zeros: bool,
    },
}

impl FootprintStudy {
    pub fn is_same_type(&self, other: &Self) -> bool {
        matches!(
            (self, other),
            (FootprintStudy::NPoC { .. }, FootprintStudy::NPoC { .. })
                | (
                    FootprintStudy::Imbalance { .. },
                    FootprintStudy::Imbalance { .. }
                )
        )
    }
}

impl FootprintStudy {
    pub const ALL: [FootprintStudy; 2] = [
        FootprintStudy::NPoC { lookback: 80 },
        FootprintStudy::Imbalance {
            threshold: 200,
            color_scale: Some(400),
            ignore_zeros: true,
        },
    ];
}

impl std::fmt::Display for FootprintStudy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            FootprintStudy::NPoC { .. } => write!(f, "Naked Point of Control"),
            FootprintStudy::Imbalance { .. } => write!(f, "Imbalance"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct PointOfControl {
    pub price: Price,
    pub volume: Qty,
    pub status: NPoc,
}

impl Default for PointOfControl {
    fn default() -> Self {
        Self {
            price: Price::from_f32(0.0),
            volume: Qty::ZERO,
            status: NPoc::default(),
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub enum NPoc {
    #[default]
    None,
    Naked,
    Filled {
        at: u64,
    },
}

impl NPoc {
    pub fn filled(&mut self, at: u64) {
        *self = NPoc::Filled { at };
    }

    pub fn unfilled(&mut self) {
        *self = NPoc::Naked;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn footprint_axis_can_zoom_out_from_default_width() {
        let kind = KlineChartKind::Footprint {
            clusters: ClusterKind::default(),
            scaling: ClusterScaling::default(),
            studies: vec![],
        };

        assert!(kind.min_cell_width() < kind.default_cell_width());
        assert!(kind.allows_indicator(KlineIndicator::VolumeProfile));
        assert!(kind.allows_indicator(KlineIndicator::FixedVolumeProfile));
        assert!(kind.allows_indicator(KlineIndicator::LiquidationHeatmap));
        assert!(KlineIndicator::LiquidationHeatmap.is_overlay());
        assert!(!KlineIndicator::LiquidationPressureIndex.is_overlay());
    }

    #[test]
    fn quantity_format_rounds_or_truncates_at_the_selected_precision() {
        let rounded = QuantityFormat {
            decimals: 1,
            rounding: QuantityRounding::Round,
        };
        let truncated = QuantityFormat {
            decimals: 1,
            rounding: QuantityRounding::Truncate,
        };

        assert_eq!(rounded.format(1_256.0), "1.3k");
        assert_eq!(truncated.format(1_256.0), "1.2k");
        assert_eq!(truncated.format(-12.89), "-12.8");
    }
}
