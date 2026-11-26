use data::config::alerts::{AlertActions, AlertConfig, AlertRule, AlertTrigger, AlertTriggerKind};
use data::i18n::tr;
use iced::{
    Alignment, Element, Length,
    widget::{button, checkbox, column, container, pick_list, row, text, text_input, tooltip},
};
use rustc_hash::{FxHashMap, FxHashSet};
use serde::{Deserialize, Serialize};
use std::fmt;

const ALERT_HISTORY_PATH: &str = "alerts/history.jsonl";
const MAX_HISTORY_RECORDS: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Source {
    pub key: String,
    pub label: String,
    pub layout_id: uuid::Uuid,
    pub pane_id: uuid::Uuid,
    pub ai_source_pane: Option<uuid::Uuid>,
}

impl fmt::Display for Source {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.label)
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Observation {
    pub pane_id: uuid::Uuid,
    pub price: f64,
    pub volume: f64,
    pub reference_volume: Option<f64>,
    pub market_time_ms: u64,
}

#[derive(Debug, Clone)]
pub struct Triggered {
    pub layout_id: uuid::Uuid,
    pub ai_source_pane: Option<uuid::Uuid>,
    pub actions: AlertActions,
    pub title: String,
    pub body: String,
}

#[derive(Debug, Clone)]
pub enum Message {
    Add,
    Remove(u64),
    Toggle(u64, bool),
    SourceChanged(u64, Source),
    TriggerChanged(u64, AlertTriggerKind),
    ValueChanged(u64, String),
    AiActionChanged(u64, bool),
    SystemNotificationChanged(u64, bool),
    ToggleExpanded(u64),
    ToggleHistory,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct HistoryRecord {
    triggered_at_ms: i64,
    source: String,
    event: String,
    ai_analysis: bool,
    system_notification: bool,
}

#[derive(Debug, Default)]
struct RuntimeState {
    previous_price: Option<f64>,
    previous_volume: Option<f64>,
    last_bar_bucket: Option<u64>,
    volume_fired_at: Option<u64>,
}

pub struct Alerts {
    config: AlertConfig,
    sources: Vec<Source>,
    runtime: FxHashMap<u64, RuntimeState>,
    history: Vec<HistoryRecord>,
    expanded_rule: Option<u64>,
    show_history: bool,
}

impl Alerts {
    pub fn new(config: AlertConfig) -> Self {
        let mut config = config;
        config.next_id = config
            .rules
            .iter()
            .map(|rule| rule.id.saturating_add(1))
            .max()
            .unwrap_or(0)
            .max(config.next_id);
        Self {
            config,
            sources: Vec::new(),
            runtime: FxHashMap::default(),
            history: load_history(),
            expanded_rule: None,
            show_history: false,
        }
    }

    pub fn config(&self) -> &AlertConfig {
        &self.config
    }

    pub fn set_sources(&mut self, mut sources: Vec<Source>) {
        let mut seen = FxHashSet::default();
        sources.retain(|source| seen.insert(source.key.clone()));
        self.sources = sources;
    }

    pub fn update(&mut self, message: Message) {
        match message {
            Message::Add => {
                let id = self.config.allocate_id();
                let mut rule = AlertRule {
                    id,
                    ..AlertRule::default()
                };
                if let Some(source) = self.sources.first() {
                    rule.source_key.clone_from(&source.key);
                    rule.source_label.clone_from(&source.label);
                }
                self.config.rules.push(rule);
                self.expanded_rule = Some(id);
            }
            Message::Remove(id) => {
                self.config.rules.retain(|rule| rule.id != id);
                self.runtime.remove(&id);
                if self.expanded_rule == Some(id) {
                    self.expanded_rule = None;
                }
            }
            Message::Toggle(id, enabled) => {
                if let Some(rule) = self.rule_mut(id) {
                    rule.enabled = enabled;
                }
                self.runtime.remove(&id);
            }
            Message::SourceChanged(id, source) => {
                if let Some(rule) = self.rule_mut(id) {
                    rule.source_key = source.key;
                    rule.source_label = source.label;
                }
                self.runtime.remove(&id);
            }
            Message::TriggerChanged(id, kind) => {
                if let Some(rule) = self.rule_mut(id) {
                    rule.trigger = match kind {
                        AlertTriggerKind::PriceTouches => AlertTrigger::PriceTouches {
                            price: String::new(),
                        },
                        AlertTriggerKind::VolumeChange => AlertTrigger::VolumeChange {
                            percent: "25".to_string(),
                        },
                        AlertTriggerKind::BarClose => AlertTrigger::BarClose {
                            minutes: "5".to_string(),
                        },
                    };
                }
                self.runtime.remove(&id);
            }
            Message::ValueChanged(id, value) => {
                if let Some(rule) = self.rule_mut(id) {
                    match &mut rule.trigger {
                        AlertTrigger::PriceTouches { price } => *price = value,
                        AlertTrigger::VolumeChange { percent } => *percent = value,
                        AlertTrigger::BarClose { minutes } => *minutes = value,
                    }
                }
            }
            Message::AiActionChanged(id, enabled) => {
                if let Some(rule) = self.rule_mut(id) {
                    rule.actions.ai_analysis = enabled;
                }
            }
            Message::SystemNotificationChanged(id, enabled) => {
                if let Some(rule) = self.rule_mut(id) {
                    rule.actions.system_notification = enabled;
                }
            }
            Message::ToggleExpanded(id) => {
                self.expanded_rule = (self.expanded_rule != Some(id)).then_some(id);
            }
            Message::ToggleHistory => self.show_history = !self.show_history,
        }
    }

    pub fn poll(&mut self, observations: &[Observation]) -> Vec<Triggered> {
        let observations = observations
            .iter()
            .map(|observation| (observation.pane_id, observation))
            .collect::<FxHashMap<_, _>>();
        let sources = self
            .sources
            .iter()
            .map(|source| (source.key.as_str(), source))
            .collect::<FxHashMap<_, _>>();
        let mut triggered = Vec::new();
        let mut history = Vec::new();

        for rule in &self.config.rules {
            if !rule.enabled || (!rule.actions.ai_analysis && !rule.actions.system_notification) {
                continue;
            }
            let Some(source) = sources.get(rule.source_key.as_str()) else {
                continue;
            };
            let Some(observation) = observations.get(&source.pane_id) else {
                continue;
            };
            let state = self.runtime.entry(rule.id).or_default();
            let detail = evaluate(&rule.trigger, observation, state);
            if let Some(detail) = detail {
                history.push(HistoryRecord {
                    triggered_at_ms: chrono::Utc::now().timestamp_millis(),
                    source: rule.source_label.clone(),
                    event: detail.clone(),
                    ai_analysis: rule.actions.ai_analysis,
                    system_notification: rule.actions.system_notification,
                });
                triggered.push(Triggered {
                    layout_id: source.layout_id,
                    ai_source_pane: source.ai_source_pane,
                    actions: rule.actions.clone(),
                    title: rule.source_label.clone(),
                    body: detail,
                });
            }
        }

        if !history.is_empty() {
            self.history.extend(history);
            if self.history.len() > MAX_HISTORY_RECORDS {
                self.history
                    .drain(..self.history.len() - MAX_HISTORY_RECORDS);
            }
            if let Err(error) = save_history(&self.history) {
                log::warn!("Failed to save alert history: {error}");
            }
        }

        triggered
    }

    pub fn view(&self, timezone: data::UserTimezone) -> Element<'_, Message> {
        let add = icon_button(
            crate::style::LucideIcon::Plus,
            tr("Add alert"),
            Message::Add,
        );
        let history = icon_button(
            crate::style::LucideIcon::History,
            if self.show_history {
                tr("Show alerts")
            } else {
                tr("Show history")
            },
            Message::ToggleHistory,
        );
        let mut rules = column![].spacing(4);

        if self.config.rules.is_empty() {
            rules = rules.push(text(tr("No alerts configured.")));
        }

        for configured in &self.config.rules {
            let id = configured.id;
            let is_expanded = self.expanded_rule == Some(id);
            let selected_source = self
                .sources
                .iter()
                .find(|source| source.key == configured.source_key)
                .cloned();
            let source = pick_list(self.sources.clone(), selected_source, move |source| {
                Message::SourceChanged(id, source)
            })
            .placeholder(&configured.source_label)
            .width(Length::Fill);
            let trigger = pick_list(
                AlertTrigger::KINDS,
                Some(configured.trigger.kind()),
                move |kind| Message::TriggerChanged(id, kind),
            )
            .width(Length::Fill);
            let (value, placeholder, suffix) = match &configured.trigger {
                AlertTrigger::PriceTouches { price } => (price, "Price", ""),
                AlertTrigger::VolumeChange { percent } => (percent, tr("Change"), "%"),
                AlertTrigger::BarClose { minutes } => (minutes, tr("Minutes"), "min"),
            };
            let value = row![
                text_input(placeholder, value)
                    .on_input(move |value| Message::ValueChanged(id, value))
                    .width(Length::Fill),
                text(suffix),
            ]
            .spacing(6)
            .align_y(Alignment::Center);
            let remove = icon_button(
                crate::style::LucideIcon::Trash2,
                tr("Delete alert"),
                Message::Remove(id),
            );
            let expand = icon_button(
                if is_expanded {
                    crate::style::LucideIcon::ChevronUp
                } else {
                    crate::style::LucideIcon::ChevronDown
                },
                if is_expanded {
                    tr("Collapse alert")
                } else {
                    tr("Edit alert")
                },
                Message::ToggleExpanded(id),
            );
            let summary = row![
                checkbox(configured.enabled).on_toggle(move |enabled| Message::Toggle(id, enabled)),
                column![
                    text(&configured.source_label),
                    text(trigger_summary(&configured.trigger)).size(crate::style::text_size::SMALL),
                ]
                .spacing(2)
                .width(Length::Fill),
                expand,
            ]
            .spacing(8)
            .align_y(Alignment::Center);
            let mut rule_content = column![summary].spacing(8).padding(8);
            if is_expanded {
                rule_content = rule_content.push(
                    column![
                        text(tr("Source")).size(crate::style::text_size::SMALL),
                        source,
                        row![
                            column![
                                text(tr("Event")).size(crate::style::text_size::SMALL),
                                trigger,
                            ]
                            .spacing(4)
                            .width(Length::FillPortion(2)),
                            column![
                                text(tr("Value")).size(crate::style::text_size::SMALL),
                                value
                            ]
                            .spacing(4)
                            .width(Length::FillPortion(1)),
                        ]
                        .spacing(8)
                        .align_y(Alignment::Center),
                        text("Actions").size(crate::style::text_size::SMALL),
                        column![
                            checkbox(configured.actions.ai_analysis)
                                .label(tr("AI analysis"))
                                .on_toggle(move |enabled| Message::AiActionChanged(id, enabled)),
                            checkbox(configured.actions.system_notification)
                                .label(tr("System notification"))
                                .on_toggle(move |enabled| Message::SystemNotificationChanged(
                                    id, enabled
                                )),
                        ]
                        .spacing(4),
                        row![iced::widget::space::horizontal(), remove],
                    ]
                    .spacing(7),
                );
            }
            rules = rules.push(
                container(rule_content)
                    .width(Length::Fill)
                    .style(crate::style::modal_container),
            );
        }

        let body: Element<'_, Message> = if self.show_history {
            self.history_view(timezone)
        } else {
            rules.into()
        };
        let content = column![
            row![
                text(tr("Alert"))
                    .size(crate::style::text_size::SECTION)
                    .width(Length::Fill),
                history,
                add,
            ]
            .align_y(Alignment::Center),
            text(if self.show_history {
                tr("Triggered alert history")
            } else {
                tr("Saved with the application state.")
            }),
            body,
        ]
        .spacing(8);

        content.into()
    }

    fn rule_mut(&mut self, id: u64) -> Option<&mut AlertRule> {
        self.config.rules.iter_mut().find(|rule| rule.id == id)
    }

    fn history_view(&self, timezone: data::UserTimezone) -> Element<'_, Message> {
        if self.history.is_empty() {
            return text(tr("No alert history.")).into();
        }
        self.history
            .iter()
            .rev()
            .take(100)
            .fold(column![].spacing(4), |history, record| {
                let triggered_at = timezone
                    .format_with_kind(
                        record.triggered_at_ms,
                        data::config::timezone::TimeLabelKind::Custom("%Y-%m-%d %H:%M:%S"),
                    )
                    .unwrap_or_else(|| tr("Unknown time").to_string());
                let actions = match (record.ai_analysis, record.system_notification) {
                    (true, true) => tr("AI + system notification"),
                    (true, false) => tr("AI analysis"),
                    (false, true) => tr("System notification"),
                    (false, false) => tr("No action"),
                };
                history.push(
                    container(
                        column![
                            text(&record.source),
                            text(&record.event),
                            text(format!("{triggered_at} | {actions}")),
                        ]
                        .spacing(4)
                        .padding(8),
                    )
                    .width(Length::Fill)
                    .style(crate::style::modal_container),
                )
            })
            .into()
    }
}

fn history_json(records: &[HistoryRecord]) -> Result<String, String> {
    records
        .iter()
        .map(|record| serde_json::to_string(record).map_err(|error| error.to_string()))
        .collect::<Result<Vec<_>, _>>()
        .map(|lines| lines.join("\n"))
}

fn load_history() -> Vec<HistoryRecord> {
    let Ok(contents) = std::fs::read_to_string(data::data_path(Some(ALERT_HISTORY_PATH))) else {
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

fn save_history(records: &[HistoryRecord]) -> Result<(), String> {
    let path = data::data_path(Some(ALERT_HISTORY_PATH));
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create alert history directory: {error}"))?;
    }
    std::fs::write(path, history_json(records)?)
        .map_err(|error| format!("Failed to save alert history: {error}"))
}

fn icon_button<'a>(
    icon: crate::style::LucideIcon,
    label: &'static str,
    message: Message,
) -> Element<'a, Message> {
    let button = button(
        crate::style::lucide_icon_text(icon, 13)
            .width(18)
            .align_x(Alignment::Center),
    )
    .padding(4)
    .style(|theme, status| crate::style::button::transparent(theme, status, false))
    .on_press(message);
    tooltip(button, Some(label), tooltip::Position::Bottom).into()
}

fn trigger_summary(trigger: &AlertTrigger) -> String {
    match trigger {
        AlertTrigger::PriceTouches { price } => format!("Price touches {price}"),
        AlertTrigger::VolumeChange { percent } => format!("Volume change {percent}%"),
        AlertTrigger::BarClose { minutes } => format!("{minutes}-minute close"),
    }
}

fn evaluate(
    trigger: &AlertTrigger,
    observation: &Observation,
    state: &mut RuntimeState,
) -> Option<String> {
    match trigger {
        AlertTrigger::PriceTouches { price } => {
            let target = price
                .parse::<f64>()
                .ok()
                .filter(|value| value.is_finite())?;
            let previous = state.previous_price.replace(observation.price)?;
            ((previous < target && observation.price >= target)
                || (previous > target && observation.price <= target))
                .then(|| format!("Price touched {target}; current {:.8}", observation.price))
        }
        AlertTrigger::VolumeChange { percent } => {
            let percent = percent.parse::<f64>().ok().filter(|value| *value > 0.0)?;
            let reference = observation.reference_volume.or(state.previous_volume);
            state.previous_volume = Some(observation.volume);
            let reference = reference.filter(|value| *value > 0.0)?;
            let change = (observation.volume / reference - 1.0) * 100.0;
            if change >= percent && state.volume_fired_at != Some(observation.market_time_ms) {
                state.volume_fired_at = Some(observation.market_time_ms);
                Some(format!("Volume increased {change:.1}%"))
            } else {
                None
            }
        }
        AlertTrigger::BarClose { minutes } => {
            let minutes = minutes.parse::<u64>().ok().filter(|value| *value > 0)?;
            let bucket = observation.market_time_ms / minutes.saturating_mul(60_000);
            let previous = state.last_bar_bucket.replace(bucket)?;
            (bucket > previous).then(|| format!("{minutes}-minute bar closed"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn observation(price: f64, volume: f64, time: u64) -> Observation {
        Observation {
            pane_id: uuid::Uuid::nil(),
            price,
            volume,
            reference_volume: Some(100.0),
            market_time_ms: time,
        }
    }

    #[test]
    fn price_touch_only_fires_on_crossing() {
        let trigger = AlertTrigger::PriceTouches {
            price: "100".to_string(),
        };
        let mut state = RuntimeState::default();
        assert!(evaluate(&trigger, &observation(99.0, 0.0, 0), &mut state).is_none());
        assert!(evaluate(&trigger, &observation(101.0, 0.0, 1), &mut state).is_some());
        assert!(evaluate(&trigger, &observation(102.0, 0.0, 2), &mut state).is_none());
    }

    #[test]
    fn bar_close_fires_once_per_new_bucket() {
        let trigger = AlertTrigger::BarClose {
            minutes: "5".to_string(),
        };
        let mut state = RuntimeState::default();
        assert!(evaluate(&trigger, &observation(0.0, 0.0, 0), &mut state).is_none());
        assert!(evaluate(&trigger, &observation(0.0, 0.0, 299_999), &mut state).is_none());
        assert!(evaluate(&trigger, &observation(0.0, 0.0, 300_000), &mut state).is_some());
        assert!(evaluate(&trigger, &observation(0.0, 0.0, 300_001), &mut state).is_none());
    }

    #[test]
    fn loaded_rules_cannot_reuse_an_existing_id() {
        let mut alerts = Alerts::new(AlertConfig {
            rules: vec![AlertRule {
                id: 7,
                ..AlertRule::default()
            }],
            next_id: 0,
        });
        alerts.update(Message::Add);
        assert_eq!(alerts.config.rules[1].id, 8);
        assert_eq!(alerts.expanded_rule, Some(8));
        alerts.update(Message::ToggleExpanded(8));
        assert_eq!(alerts.expanded_rule, None);
    }

    #[test]
    fn duplicate_market_sources_across_layouts_are_collapsed() {
        let mut alerts = Alerts::new(AlertConfig::default());
        let source = |layout_id| Source {
            key: "BTCUSDT|10m|Footprint".to_string(),
            label: "BTCUSDT | 10m | Footprint".to_string(),
            layout_id,
            pane_id: uuid::Uuid::new_v4(),
            ai_source_pane: None,
        };
        alerts.set_sources(vec![
            source(uuid::Uuid::new_v4()),
            source(uuid::Uuid::new_v4()),
        ]);
        assert_eq!(alerts.sources.len(), 1);
    }

    #[test]
    fn alert_history_serialization_preserves_existing_records() {
        let records = vec![
            HistoryRecord {
                triggered_at_ms: 1,
                source: "BTCUSDT | 5m | Kline".to_string(),
                event: "Price touched 100".to_string(),
                ai_analysis: false,
                system_notification: true,
            },
            HistoryRecord {
                triggered_at_ms: 2,
                source: "ETHUSDT | DOM".to_string(),
                event: "Volume increased 25%".to_string(),
                ai_analysis: true,
                system_notification: true,
            },
        ];
        let json = history_json(&records).unwrap();
        let loaded = json
            .lines()
            .map(|line| serde_json::from_str::<HistoryRecord>(line).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].triggered_at_ms, 1);
        assert_eq!(loaded[1].triggered_at_ms, 2);
    }
}
