use super::ScaleFactor;
use super::sidebar::Sidebar;
use super::timezone::UserTimezone;
use crate::config::network::Network;
use crate::layout::WindowSpec;
use crate::{AudioStream, Layout, Theme};

use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct Layouts {
    pub layouts: Vec<Layout>,
    pub active_layout: Option<String>,
    #[serde(default)]
    pub chart_presets: Vec<crate::chart::kline::ChartPreset>,
}

#[derive(Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct State {
    #[serde(deserialize_with = "deserialize_layouts")]
    pub layout_manager: Layouts,
    pub selected_theme: Theme,
    pub custom_theme: Option<Theme>,
    pub main_window: Option<WindowSpec>,
    pub timezone: UserTimezone,
    pub sidebar: Sidebar,
    pub scale_factor: ScaleFactor,
    pub audio_cfg: AudioStream,
    pub ai_cfg: super::ai::AiConfig,
    pub alert_cfg: super::alerts::AlertConfig,
    pub trading_cfg: super::trading::TradingConfig,
    pub network: Network,
    #[serde(skip_serializing)]
    pub proxy_cfg: Option<exchange::proxy::Proxy>,
    #[serde(skip_serializing)]
    pub trade_fetch_enabled: Option<bool>,
    pub size_in_quote_ccy: exchange::SizeUnit,
    pub cache_market_metadata: bool,
    #[serde(default = "default_antialiasing")]
    pub antialiasing: bool,
}

impl State {
    pub fn from_parts(
        layout_manager: Layouts,
        selected_theme: Theme,
        custom_theme: Option<Theme>,
        main_window: Option<WindowSpec>,
        timezone: UserTimezone,
        sidebar: Sidebar,
        scale_factor: ScaleFactor,
        audio_cfg: AudioStream,
        network: Network,
        volume_size_unit: exchange::SizeUnit,
        cache_market_metadata: bool,
        antialiasing: bool,
        ai_cfg: super::ai::AiConfig,
        alert_cfg: super::alerts::AlertConfig,
    ) -> Self {
        State {
            layout_manager,
            selected_theme: Theme(selected_theme.0),
            custom_theme: custom_theme.map(|t| Theme(t.0)),
            main_window,
            timezone,
            sidebar,
            scale_factor,
            audio_cfg,
            network,
            proxy_cfg: None,
            trade_fetch_enabled: None,
            size_in_quote_ccy: volume_size_unit,
            cache_market_metadata,
            antialiasing,
            ai_cfg,
            alert_cfg,
            trading_cfg: Default::default(),
        }
    }
}

const fn default_antialiasing() -> bool {
    true
}

fn deserialize_layouts<'de, D: serde::Deserializer<'de>>(deserializer: D) -> Result<Layouts, D::Error> {
    let mut value = serde_json::Value::deserialize(deserializer)?;
    if let Some(layouts) = value.get_mut("layouts").and_then(serde_json::Value::as_array_mut) {
        for layout in layouts {
            if let Some(dashboard) = layout.get_mut("dashboard") {
                if let Some(pane) = dashboard.get_mut("pane") {
                    crate::layout::pane::migrate_legacy_pane(pane).map_err(serde::de::Error::custom)?;
                }
                if let Some(popouts) = dashboard.get_mut("popout").and_then(serde_json::Value::as_array_mut) {
                    for popout in popouts {
                        if let Some(pane) = popout.get_mut(0) {
                            crate::layout::pane::migrate_legacy_pane(pane).map_err(serde::de::Error::custom)?;
                        }
                    }
                }
            }
        }
    }
    serde_json::from_value(value).map_err(serde::de::Error::custom)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pre_pro_state_defaults_optional_features() {
        let state: State = serde_json::from_str("{}").unwrap();
        assert!(state.alert_cfg.rules.is_empty());
        assert!(state.layout_manager.layouts.is_empty());
        assert!(state.antialiasing);
    }

    #[test]
    fn legacy_pro_layout_keeps_panels_in_splits_and_popouts() {
        let state: State = serde_json::from_value(serde_json::json!({
            "trade_fetch_enabled": true,
            "layout_manager": {"active_layout": "Pro", "layouts": [{
                "name": "Pro", "dashboard": {
                    "pane": {"Split": {"axis": "Vertical", "ratio": 0.5,
                        "a": "AiAnalysis", "b": {"TradingPanel": {"link_group": "B"}}}},
                    "popout": [[{"AiAnalysisPanel": {"link_group": "B"}}, {"pos_x": 0.0, "pos_y": 0.0, "width": 800.0, "height": 600.0}]]
                }
            }]}
        })).unwrap();
        assert_eq!(state.trade_fetch_enabled, Some(true));
        let dashboard = &state.layout_manager.layouts[0].dashboard;
        assert!(matches!(dashboard.pane, crate::layout::pane::Pane::Split { .. }));
        assert!(matches!(dashboard.popout[0].0, crate::layout::pane::Pane::AiAnalysisPanel { link_group: Some(crate::layout::pane::LinkGroup::B) }));
    }
}
