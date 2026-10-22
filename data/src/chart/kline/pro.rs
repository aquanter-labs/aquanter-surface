use serde::{Deserialize, Serialize};
use super::{Config, IndicatorConfig, IndicatorId, IndicatorInstance, IndicatorInstanceId};

pub fn rgba(color: iced_core::Color) -> u32 {
    let channel = |v: f32| (v.clamp(0.0, 1.0) * 255.0).round() as u32;
    channel(color.r) << 24 | channel(color.g) << 16 | channel(color.b) << 8 | channel(color.a)
}

pub fn migrate_legacy_chart(value: serde_json::Value, enabled: &[String]) -> Result<Config, serde_json::Error> {
    let mut pro: ProConfig = if value.is_null() { ProConfig::default() } else { serde_json::from_value(value)? };
    for name in enabled {
        if !pro.is_enabled(name) {
            pro.enabled.push(name.clone());
        }
    }
    let instances = pro.migrate_indicators(enabled);
    let mut composition = super::composition::ChartComposition::prototype_kline();
    for instance in &instances {
        if instance.kind.is_registered() {
            composition.add_indicator(instance.id, instance.kind.clone());
        }
    }
    let mut config = Config::default();
    config.settings.options.legend_visibility = if pro.data_labels_always_visible {
        super::LegendVisibility::KeepLegendAndValuesVisible
    } else { super::LegendVisibility::KeepLegendVisible };
    config.settings.options.pro = pro;
    config.settings.panels = composition.panels.iter().map(super::panel_config_from_spec).collect();
    config.settings.splits = composition.splits;
    config.settings.indicator_instances = instances;
    Ok(config)
}

impl ProConfig {
    pub fn is_enabled(&self, name: &str) -> bool {
        self.enabled.iter().any(|enabled| enabled == name)
    }

    pub fn adopt_order_book(&mut self) {
        if self.show_footprint_orderbook && !self.is_enabled("OrderBook") {
            self.enabled.push("OrderBook".into());
        }
    }

    pub fn scripted_indicator_id(name: &str) -> Option<&'static str> {
        Some(match name {
            "Ema" => "pro.ema",
            "StochRsi" => "pro.stoch-rsi",
            "CumulativeDelta" => "pro.pro-cvd",
            "OpenInterest" => "pro.pro-open-interest",
            "OiDeltaDivergence" => "pro.oi-delta-divergence",
            "OiWeightedDelta" => "pro.oi-weighted-delta",
            "OiFlowNet" => "pro.oi-flow-net",
            "LiquidationPressureIndex" => "pro.liquidation-pressure",
            "OrderFlowBias" => "pro.order-flow-bias",
            "Volume" => "core.volume",
            "BarAnalysis" => "core.bar-analysis",
            _ => return None,
        })
    }

    pub fn migrate_indicators(&self, enabled: &[String]) -> Vec<IndicatorInstance> {
        use serde_json::json;
        let mut result = Vec::new();
        let mut add = |id: &str, config: serde_json::Value, widths: &[(&str, f32)]| {
            let kind = IndicatorId::parse(id).expect("static Pro indicator ID");
            let mut instance = IndicatorInstance::new(IndicatorInstanceId::new(result.len() as u64 + 1), kind);
            instance.config = IndicatorConfig::from_value(config);
            for &(slot, width) in widths { instance.style_overrides.set_precise_line_width(slot, width); }
            result.push(instance);
        };
        for name in enabled {
            match name.as_str() {
                "Volume" => add("core.volume", json!({}), &[]),
                "BarAnalysis" => add("core.bar-analysis", json!({}), &[]),
                "Ema" => for config in &self.ema {
                    add("pro.ema", json!({"period":config.period.max(1),"color":rgba(config.color)}), &[("main",config.line_width)]);
                },
                "StochRsi" => {
                    let c = self.stoch_rsi;
                    add("pro.stoch-rsi", json!({"rsi_length":c.rsi_length.max(2),"stochastic_length":c.stochastic_length.max(1),"k_smoothing":c.k_smoothing.max(1),"d_smoothing":c.d_smoothing.max(1),"k_color":rgba(c.k_color),"d_color":rgba(c.d_color)}), &[("k",c.line_width),("d",c.line_width)]);
                },
                "CumulativeDelta" | "OpenInterest" => {
                    let (id, c) = if name == "OpenInterest" { ("pro.pro-open-interest", self.open_interest) } else { ("pro.pro-cvd", self.cumulative_delta) };
                    add(id, json!({"candles": c.style == IndicatorPlotStyle::Candles, "line_color": rgba(c.line_color), "up_color": rgba(c.up_color), "down_color": rgba(c.down_color)}), &[("main", c.line_width)]);
                },
                "OiDeltaDivergence" => {
                    let c = self.oi_delta_divergence;
                    add("pro.oi-delta-divergence",json!({"lookback":c.lookback.max(2),"rsi_period":c.rsi_period.max(2),"marker_size":c.marker_size,"bullish_color":rgba(c.bullish_color),"bearish_color":rgba(c.bearish_color),"cvd_color":rgba(c.cvd_color),"oi_color":rgba(c.oi_color)}), &[("cvd",c.line_width),("oi",c.line_width)]);
                },
                "OiWeightedDelta" => {
                    let c = self.oi_weighted_delta;
                    add("pro.oi-weighted-delta",json!({"sensitivity":c.sensitivity,"max_weight":c.max_weight.max(1.0),"positive_color":rgba(c.positive_color),"negative_color":rgba(c.negative_color)}), &[]);
                },
                "OiFlowNet" => {
                    let c = self.oi_flow_net;
                    add("pro.oi-flow-net",json!({"basis_length":c.basis_length.max(2),"burst_sensitivity":c.burst_sensitivity.clamp(1,5),"static_color":c.static_color,"show_price_line":c.show_price_line,"long_color":rgba(c.long_color),"short_color":rgba(c.short_color),"buy_outflow_color":rgba(c.buy_outflow_color),"sell_outflow_color":rgba(c.sell_outflow_color),"extreme_band_color":rgba(c.extreme_band_color)}), &[]);
                },
                "LiquidationPressureIndex" => {
                    let c = self.liquidation_pressure;
                    add("pro.liquidation-pressure",json!({"lookback":c.lookback.max(2),"oi_drop_threshold":c.oi_drop_threshold.max(0.01),"delta_zscore":c.delta_zscore.max(0.0),"positive_color":rgba(c.positive_color),"negative_color":rgba(c.negative_color)}), &[]);
                },
                "OrderFlowBias" => {
                    let c = self.order_flow_bias;
                    add("pro.order-flow-bias",json!({"lookback":c.lookback.clamp(2,100),"signal_threshold":c.signal_threshold.clamp(5.0,100.0),"absorption_delta":c.absorption_delta.clamp(0.1,0.95),"max_price_response":c.max_price_response.clamp(0.05,1.0),"imbalance_ratio":c.imbalance_ratio.clamp(1.1,10.0)}), &[]);
                },
                _ => {},
            }
        }
        result
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct ProConfig {
    pub vwap_mode: Option<VwapMode>,
    pub enabled: Vec<String>,
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
    pub order_flow_bias: OrderFlowBiasConfig,
}

impl Default for ProConfig {
    fn default() -> Self {
        Self {
            vwap_mode: None,
            enabled: Vec::new(),
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
            order_flow_bias: OrderFlowBiasConfig::default(),
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
pub enum VwapMode {
    Session { hour_utc: u8 },
    Anchor { time_ms: u64 },
    Rolling { bars: u16 },
    Week,
    Month,
    Visible { time_ms: u64 },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub enum OrderFlowBiasPreset {
    #[default]
    Conservative,
    Balanced,
    Aggressive,
    Custom,
}

impl OrderFlowBiasPreset {
    pub const ALL: [Self; 4] = [
        Self::Conservative,
        Self::Balanced,
        Self::Aggressive,
        Self::Custom,
    ];
}

impl std::fmt::Display for OrderFlowBiasPreset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Conservative => write!(f, "Conservative"),
            Self::Balanced => write!(f, "Balanced"),
            Self::Aggressive => write!(f, "Aggressive"),
            Self::Custom => write!(f, "Custom"),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct OrderFlowBiasConfig {
    pub preset: OrderFlowBiasPreset,
    pub lookback: u16,
    pub signal_threshold: f32,
    pub absorption_delta: f32,
    pub max_price_response: f32,
    pub imbalance_ratio: f32,
}

impl OrderFlowBiasConfig {
    pub fn from_preset(preset: OrderFlowBiasPreset) -> Self {
        match preset {
            OrderFlowBiasPreset::Conservative => Self {
                preset,
                lookback: 20,
                signal_threshold: 35.0,
                absorption_delta: 0.75,
                max_price_response: 0.35,
                imbalance_ratio: 3.0,
            },
            OrderFlowBiasPreset::Balanced => Self {
                preset,
                lookback: 14,
                signal_threshold: 25.0,
                absorption_delta: 0.60,
                max_price_response: 0.45,
                imbalance_ratio: 2.5,
            },
            OrderFlowBiasPreset::Aggressive => Self {
                preset,
                lookback: 8,
                signal_threshold: 15.0,
                absorption_delta: 0.45,
                max_price_response: 0.60,
                imbalance_ratio: 2.0,
            },
            OrderFlowBiasPreset::Custom => Self {
                preset,
                ..Self::default()
            },
        }
    }

    pub fn custom(mut self) -> Self {
        self.preset = OrderFlowBiasPreset::Custom;
        self
    }
}

impl Default for OrderFlowBiasConfig {
    fn default() -> Self {
        Self::from_preset(OrderFlowBiasPreset::Conservative)
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    #[test]
    fn legacy_order_flow_bias_migrates_with_its_parameters() {
        let pro = ProConfig {
            order_flow_bias: OrderFlowBiasConfig::from_preset(OrderFlowBiasPreset::Aggressive),
            ..ProConfig::default()
        };
        assert_eq!(ProConfig::scripted_indicator_id("OrderFlowBias"), Some("pro.order-flow-bias"));
        let instances = pro.migrate_indicators(&["OrderFlowBias".to_owned()]);
        let [instance] = instances.as_slice() else { panic!("expected one migrated instance") };
        assert_eq!(instance.kind.id(), "pro.order-flow-bias");
        let source = include_str!("../../../indicators/sources/order-flow-bias.ind");
        let compiled = indicator_engine::compile(source, "order-flow-bias.ind").expect("bias script compiles");
        let config = instance.config.value();
        let migrated = config.as_object().expect("object config");
        for (key, value) in migrated {
            let input = compiled.descriptor.inputs.iter().find(|input| &input.id == key)
                .unwrap_or_else(|| panic!("{key} is not a bias input"));
            let value = value.as_f64().expect("numeric input");
            assert!(input.min.is_none_or(|min| value >= min) && input.max.is_none_or(|max| value <= max), "{key} out of range");
        }
        assert_eq!(migrated.len(), compiled.descriptor.inputs.len());
        assert_eq!(config["lookback"], 8);
        assert_eq!(config["signal_threshold"], 15.0);
        assert_eq!(config["imbalance_ratio"], 2.0);
    }
}


