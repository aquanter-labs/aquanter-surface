use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct AlertConfig {
    pub rules: Vec<AlertRule>,
    pub next_id: u64,
}

impl AlertConfig {
    pub fn allocate_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        id
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct AlertRule {
    pub id: u64,
    pub enabled: bool,
    pub source_key: String,
    pub source_label: String,
    pub trigger: AlertTrigger,
    pub actions: AlertActions,
}

impl Default for AlertRule {
    fn default() -> Self {
        Self {
            id: 0,
            enabled: true,
            source_key: String::new(),
            source_label: "Select a market source".to_string(),
            trigger: AlertTrigger::default(),
            actions: AlertActions::default(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub enum AlertTrigger {
    PriceTouches { price: String },
    VolumeChange { percent: String },
    BarClose { minutes: String },
}

impl Default for AlertTrigger {
    fn default() -> Self {
        Self::BarClose {
            minutes: "5".to_string(),
        }
    }
}

impl AlertTrigger {
    pub const KINDS: [AlertTriggerKind; 3] = [
        AlertTriggerKind::PriceTouches,
        AlertTriggerKind::VolumeChange,
        AlertTriggerKind::BarClose,
    ];

    pub fn kind(&self) -> AlertTriggerKind {
        match self {
            Self::PriceTouches { .. } => AlertTriggerKind::PriceTouches,
            Self::VolumeChange { .. } => AlertTriggerKind::VolumeChange,
            Self::BarClose { .. } => AlertTriggerKind::BarClose,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlertTriggerKind {
    PriceTouches,
    VolumeChange,
    BarClose,
}

impl fmt::Display for AlertTriggerKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PriceTouches => write!(f, "Price touches"),
            Self::VolumeChange => write!(f, "Volume change"),
            Self::BarClose => write!(f, "N-minute close"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct AlertActions {
    pub ai_analysis: bool,
    #[serde(alias = "desktop")]
    pub system_notification: bool,
}

impl Default for AlertActions {
    fn default() -> Self {
        Self {
            ai_analysis: true,
            system_notification: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alert_rules_round_trip() {
        let config = AlertConfig {
            rules: vec![AlertRule {
                id: 4,
                enabled: true,
                source_key: "BTCUSDT|Footprint|10m".to_string(),
                source_label: "BTCUSDT | 10m | Footprint".to_string(),
                trigger: AlertTrigger::VolumeChange {
                    percent: "25".to_string(),
                },
                actions: AlertActions {
                    ai_analysis: true,
                    system_notification: true,
                },
            }],
            next_id: 5,
        };

        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(serde_json::from_str::<AlertConfig>(&json).unwrap(), config);
    }

    #[test]
    fn old_desktop_action_migrates_to_system_notification() {
        let actions: AlertActions =
            serde_json::from_str(r#"{"ai_analysis":false,"in_app":true,"desktop":true}"#).unwrap();
        assert!(!actions.ai_analysis);
        assert!(actions.system_notification);
    }
}
