use super::ScaleFactor;
use super::ai::AiConfig;
use super::alerts::AlertConfig;
use super::sidebar::Sidebar;
use super::timezone::UserTimezone;
use crate::layout::WindowSpec;
use crate::{AudioStream, Layout, Theme};

use serde::{Deserialize, Serialize};

#[derive(Clone, Serialize, Deserialize, Default)]
pub struct Layouts {
    pub layouts: Vec<Layout>,
    pub active_layout: Option<String>,
}

#[derive(Default, Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct State {
    pub layout_manager: Layouts,
    pub selected_theme: Theme,
    pub custom_theme: Option<Theme>,
    pub main_window: Option<WindowSpec>,
    pub timezone: UserTimezone,
    pub sidebar: Sidebar,
    pub scale_factor: ScaleFactor,
    pub audio_cfg: AudioStream,
    pub trade_fetch_enabled: bool,
    pub size_in_quote_ccy: exchange::SizeUnit,
    pub proxy_cfg: Option<exchange::proxy::Proxy>,
    pub ai_cfg: AiConfig,
    pub alert_cfg: AlertConfig,
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
        trade_fetch_enabled: bool,
        volume_size_unit: exchange::SizeUnit,
        proxy_cfg: Option<exchange::proxy::Proxy>,
        ai_cfg: AiConfig,
        alert_cfg: AlertConfig,
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
            trade_fetch_enabled,
            size_in_quote_ccy: volume_size_unit,
            proxy_cfg,
            ai_cfg,
            alert_cfg,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_saved_state_defaults_to_no_alert_rules() {
        let state: State = serde_json::from_str("{}").unwrap();
        assert!(state.alert_cfg.rules.is_empty());
    }
}
