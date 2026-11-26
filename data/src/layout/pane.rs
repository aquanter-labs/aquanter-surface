use exchange::{TickMultiplier, TickerInfo, Timeframe};
use serde::{Deserialize, Serialize};

use crate::chart::heatmap;
use crate::panel::{ladder, timesales};
use crate::stream::PersistStreamKind;
use crate::util::ok_or_default;

use crate::chart::Basis;

#[derive(Debug, Clone, Copy, Deserialize, Serialize)]
pub enum Axis {
    Horizontal,
    Vertical,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum Pane {
    Split {
        axis: Axis,
        ratio: f32,
        a: Box<Pane>,
        b: Box<Pane>,
    },
    Starter {
        #[serde(deserialize_with = "ok_or_default", default)]
        link_group: Option<LinkGroup>,
    },
    AiAnalysis,
    AiAnalysisPanel {
        #[serde(default)]
        link_group: Option<LinkGroup>,
    },
    Trading,
    TradingPanel {
        #[serde(default)]
        config: crate::config::trading::TradingConfig,
        #[serde(default)]
        link_group: Option<LinkGroup>,
    },
    ShaderHeatmap {
        #[serde(deserialize_with = "ok_or_default", default)]
        indicators: heatmap::indicators::HeatmapIndicatorConfig,
        #[serde(deserialize_with = "ok_or_default", default)]
        stream_type: Vec<PersistStreamKind>,
        #[serde(deserialize_with = "ok_or_default")]
        settings: Settings,
        #[serde(deserialize_with = "ok_or_default", default)]
        link_group: Option<LinkGroup>,
    },
    KlineChart {
        #[serde(default)]
        layout: Option<Box<crate::chart::kline::Config>>,
        #[serde(deserialize_with = "ok_or_default", default)]
        stream_type: Vec<PersistStreamKind>,
        #[serde(deserialize_with = "ok_or_default")]
        settings: Settings,
        #[serde(deserialize_with = "ok_or_default", default)]
        link_group: Option<LinkGroup>,
    },
    TimeAndSales {
        stream_type: Vec<PersistStreamKind>,
        settings: Settings,
        #[serde(deserialize_with = "ok_or_default", default)]
        link_group: Option<LinkGroup>,
    },
    Ladder {
        stream_type: Vec<PersistStreamKind>,
        settings: Settings,
        #[serde(deserialize_with = "ok_or_default", default)]
        link_group: Option<LinkGroup>,
    },
}

impl Default for Pane {
    fn default() -> Self {
        Pane::Starter { link_group: None }
    }
}

pub(crate) fn migrate_legacy_pane(value: &mut serde_json::Value) -> Result<(), serde_json::Error> {
    if let Some(split) = value.get_mut("Split") {
        for side in ["a", "b"] {
            if let Some(child) = split.get_mut(side) {
                migrate_legacy_pane(child)?;
            }
        }
        return Ok(());
    }
    let Some(chart) = value.get_mut("KlineChart") else {
        return Ok(());
    };
    if chart.get("kind").is_none() && chart.get("indicators").is_none() {
        return Ok(());
    }
    let indicators = chart
        .get("indicators")
        .and_then(serde_json::Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_owned)
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let visual = chart
        .pointer("/settings/visual_config/Kline")
        .cloned()
        .unwrap_or_else(|| serde_json::json!({}));
    let mut config = crate::chart::kline::pro::migrate_legacy_chart(visual, &indicators)?;
    if let Some(splits) = chart.pointer("/layout/splits") {
        config.settings.splits = serde_json::from_value(splits.clone())?;
    }
    if let Some(footprint) = chart.pointer("/kind/Footprint") {
        config.settings.options.trade_data_requested = true;
        config.settings.options.footprint.visible = true;
        if let Some(clusters) = footprint.get("clusters") {
            config.settings.options.footprint.cluster_kind =
                serde_json::from_value(clusters.clone())?;
        }
        if let Some(scaling) = footprint.get("scaling") {
            config.settings.options.footprint.cluster_scaling =
                serde_json::from_value(scaling.clone())?;
        }
        let studies = footprint
            .get("studies")
            .and_then(serde_json::Value::as_array);
        for study in studies.into_iter().flatten() {
            let options = &mut config.settings.options.footprint;
            if study.get("NPoC").is_some() {
                options.poc.show_npoc = true;
            } else if let Some(imbalance) = study.get("Imbalance") {
                options.imbalance.show = true;
                if let Some(threshold) = imbalance
                    .get("threshold")
                    .and_then(serde_json::Value::as_u64)
                {
                    options.imbalance.threshold_percent = threshold as usize;
                }
                if let Some(ignore_zeros) = imbalance
                    .get("ignore_zeros")
                    .and_then(serde_json::Value::as_bool)
                {
                    options.imbalance.ignore_zeros = ignore_zeros;
                }
            }
        }
    }
    chart["layout"] = serde_json::to_value(config)?;
    if let Some(settings) = chart
        .get_mut("settings")
        .and_then(serde_json::Value::as_object_mut)
    {
        settings.remove("visual_config");
    }
    if let Some(chart) = chart.as_object_mut() {
        chart.remove("kind");
        chart.remove("indicators");
    }
    Ok(())
}

#[cfg(test)]
mod pro_tests {
    use super::*;

    #[test]
    fn panel_configs_and_link_groups_round_trip() {
        for pane in [
            Pane::AiAnalysisPanel {
                link_group: Some(LinkGroup::C),
            },
            Pane::TradingPanel {
                config: Default::default(),
                link_group: Some(LinkGroup::H),
            },
            Pane::AiAnalysis,
            Pane::Trading,
        ] {
            let value = serde_json::to_value(&pane).unwrap();
            let decoded: Pane = serde_json::from_value(value.clone()).unwrap();
            assert_eq!(serde_json::to_value(decoded).unwrap(), value);
        }
    }

    #[test]
    fn legacy_footprint_preserves_group_basis_and_indicators() {
        let mut pane = serde_json::json!({"KlineChart": {
            "layout": {"splits": [0.8], "autoscale": "CenterLatest"},
            "kind": {"Footprint": {"clusters": "BidAsk", "scaling": "VisibleRange", "studies": []}},
            "indicators": ["Volume", "Ema"],
            "settings": {"selected_basis": {"Time": "M5"}, "visual_config": {"Kline": {}}},
            "stream_type": [], "link_group": "C"
        }});
        migrate_legacy_pane(&mut pane).unwrap();
        let decoded: Pane = serde_json::from_value(pane.clone()).unwrap();
        let Pane::KlineChart {
            layout: Some(config),
            link_group,
            ..
        } = decoded
        else {
            panic!("missing migrated chart")
        };
        assert_eq!(link_group, Some(LinkGroup::C));
        assert!(config.settings.options.footprint.visible);
        assert_eq!(config.settings.splits, vec![0.8]);
        let once = pane.clone();
        migrate_legacy_pane(&mut pane).unwrap();
        assert_eq!(pane, once);
    }

    #[test]
    fn legacy_footprint_studies_survive_migration() {
        let mut pane = serde_json::json!({"KlineChart": {
            "kind": {"Footprint": {"clusters": "BidAsk", "scaling": "VisibleRange", "studies": [
                {"NPoC": {"lookback": 80}},
                {"Imbalance": {"threshold": 350, "color_scale": 400, "ignore_zeros": false}}
            ]}},
            "indicators": [],
            "settings": {"visual_config": {"Kline": {}}},
            "stream_type": []
        }});
        migrate_legacy_pane(&mut pane).unwrap();
        let decoded: Pane = serde_json::from_value(pane).unwrap();
        let Pane::KlineChart {
            layout: Some(config),
            ..
        } = decoded
        else {
            panic!("missing migrated chart")
        };
        let footprint = config.settings.options.footprint;
        assert!(footprint.poc.show_npoc);
        assert!(footprint.imbalance.show);
        assert_eq!(footprint.imbalance.threshold_percent, 350);
        assert!(!footprint.imbalance.ignore_zeros);
    }
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(default)]
pub struct Settings {
    pub tick_multiply: Option<exchange::TickMultiplier>,
    pub visual_config: Option<VisualConfig>,
    pub selected_basis: Option<Basis>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Deserialize, Serialize)]
pub enum LinkGroup {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
}

impl LinkGroup {
    pub const ALL: [LinkGroup; 9] = [
        LinkGroup::A,
        LinkGroup::B,
        LinkGroup::C,
        LinkGroup::D,
        LinkGroup::E,
        LinkGroup::F,
        LinkGroup::G,
        LinkGroup::H,
        LinkGroup::I,
    ];
}

impl std::fmt::Display for LinkGroup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let c = match self {
            LinkGroup::A => "1",
            LinkGroup::B => "2",
            LinkGroup::C => "3",
            LinkGroup::D => "4",
            LinkGroup::E => "5",
            LinkGroup::F => "6",
            LinkGroup::G => "7",
            LinkGroup::H => "8",
            LinkGroup::I => "9",
        };
        write!(f, "{c}")
    }
}

/// Defines the specific configuration for different types of pane settings.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub enum VisualConfig {
    Heatmap(heatmap::Config),
    TimeAndSales(timesales::Config),
    Ladder(ladder::Config),
}

impl VisualConfig {
    pub fn heatmap(&self) -> Option<heatmap::Config> {
        match self {
            Self::Heatmap(cfg) => Some(*cfg),
            _ => None,
        }
    }

    pub fn time_and_sales(&self) -> Option<timesales::Config> {
        match self {
            Self::TimeAndSales(cfg) => Some(*cfg),
            _ => None,
        }
    }

    pub fn ladder(&self) -> Option<ladder::Config> {
        match self {
            Self::Ladder(cfg) => Some(*cfg),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ContentKind {
    Starter,
    AiAnalysis,
    Trading,
    ShaderHeatmap,
    CandlestickChart,
    TimeAndSales,
    Ladder,
}

impl ContentKind {
    pub const ALL: [ContentKind; 7] = [
        ContentKind::Starter,
        ContentKind::AiAnalysis,
        ContentKind::Trading,
        ContentKind::ShaderHeatmap,
        ContentKind::CandlestickChart,
        ContentKind::TimeAndSales,
        ContentKind::Ladder,
    ];
}

impl std::fmt::Display for ContentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ContentKind::Starter => "Starter Pane",
            ContentKind::AiAnalysis => "AI Analysis",
            ContentKind::Trading => "Trading",
            ContentKind::ShaderHeatmap => "Heatmap Chart",
            ContentKind::CandlestickChart => "Candlestick Chart",
            ContentKind::TimeAndSales => "Time&Sales",
            ContentKind::Ladder => "DOM/Ladder",
        };
        f.write_str(crate::i18n::tr(s))
    }
}

#[derive(Clone, Copy)]
pub struct PaneSetup {
    pub ticker_info: exchange::TickerInfo,
    pub basis: Option<Basis>,
    pub tick_multiplier: Option<TickMultiplier>,
    pub price_step: exchange::unit::PriceStep,
    pub depth_aggr: exchange::adapter::StreamTicksize,
    pub push_freq: exchange::PushFrequency,
}

impl PaneSetup {
    pub fn new(
        content_kind: ContentKind,
        base_ticker: TickerInfo,
        prev_base_ticker: Option<TickerInfo>,
        current_basis: Option<Basis>,
        current_tick_multiplier: Option<TickMultiplier>,
    ) -> Self {
        let exchange = base_ticker.ticker.exchange;

        let is_client_aggr = exchange.is_depth_client_aggr();
        let prev_is_client_aggr = prev_base_ticker
            .map(|ti| ti.ticker.exchange.is_depth_client_aggr())
            .unwrap_or(is_client_aggr);

        let basis =
            match content_kind {
                ContentKind::Ladder => Some(
                    current_basis.unwrap_or_else(|| Basis::default_heatmap_time(Some(base_ticker))),
                ),
                ContentKind::ShaderHeatmap => Some(
                    current_basis.unwrap_or_else(|| Basis::default_heatmap_time(Some(base_ticker))),
                ),
                ContentKind::CandlestickChart => {
                    let current = current_basis.and_then(|b| match b {
                        Basis::Time(tf) if exchange.supports_kline_timeframe(tf) => Some(b),
                        Basis::Tick(_) => Some(b),
                        _ => None,
                    });

                    Some(current.unwrap_or_else(|| {
                        Basis::default_kline_time(Some(base_ticker), Timeframe::M5)
                    }))
                }
                ContentKind::Starter
                | ContentKind::AiAnalysis
                | ContentKind::Trading
                | ContentKind::TimeAndSales => None,
            };

        let tick_multiplier = match content_kind {
            ContentKind::Ladder | ContentKind::ShaderHeatmap => {
                let tm = if !is_client_aggr && prev_is_client_aggr {
                    TickMultiplier(10)
                } else if let Some(tm) = current_tick_multiplier {
                    tm
                } else if is_client_aggr {
                    TickMultiplier(5)
                } else {
                    TickMultiplier(10)
                };
                Some(tm)
            }
            ContentKind::CandlestickChart
            | ContentKind::TimeAndSales
            | ContentKind::AiAnalysis
            | ContentKind::Trading
            | ContentKind::Starter => current_tick_multiplier,
        };

        let price_step = match tick_multiplier {
            Some(tm) => tm.multiply_with_min_tick_step(base_ticker),
            None => base_ticker.min_ticksize.into(),
        };

        let depth_aggr = exchange.stream_ticksize(tick_multiplier, TickMultiplier(50));

        let push_freq = match content_kind {
            ContentKind::ShaderHeatmap if exchange.is_custom_push_freq() => match basis {
                Some(Basis::Time(tf)) if exchange.supports_heatmap_timeframe(tf) => {
                    exchange::PushFrequency::Custom(tf)
                }
                _ => exchange::PushFrequency::ServerDefault,
            },
            _ => exchange::PushFrequency::ServerDefault,
        };

        Self {
            ticker_info: base_ticker,
            basis,
            tick_multiplier,
            price_step,
            depth_aggr,
            push_freq,
        }
    }
}
