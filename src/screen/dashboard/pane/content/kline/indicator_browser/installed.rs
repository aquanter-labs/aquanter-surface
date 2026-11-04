use super::Message as BrowserMessage;
use crate::screen::dashboard::pane::content::kline::{
    KlineChart, presentation::IndicatorPresentationResolver, studies::StudyAvailability,
};
use crate::style::{self, Icon, icon_text};
use crate::widget::inspector::{InspectorContent, InspectorSection};
use data::chart::kline::IndicatorId;
use data::chart::kline::{
    IndicatorUnsupportedReason, KlineStudy, StudyDataSource,
    indicators::all_indicators,
};
use iced::{
    Alignment, Element, Length,
    widget::{button, column, container, row, scrollable, space, text_input, tooltip::Position},
};

#[derive(Debug, Clone, PartialEq)]
pub(super) struct ViewState {
    search_query: String,
    search_box_id: iced::widget::Id,
    scroll_id: iced::widget::Id,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ActiveIndicator {
    kind: IndicatorId,
    count: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AvailableStudy {
    study: KlineStudy,
    availability: StudyAvailability,
    active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct AvailableIndicator {
    indicator: IndicatorId,
    unavailable_reason: Option<IndicatorUnsupportedReason>,
    active_count: usize,
}

/// Menu sections, listed in declaration order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum MenuGroup {
    Trend,
    Vwap,
    Oscillators,
    OrderFlow,
    OpenInterest,
    Profiles,
    Other,
}

impl MenuGroup {
    fn for_indicator(indicator: &IndicatorId) -> Self {
        match indicator.id() {
            "core.moving-averages" | "core.bollinger-bands" => Self::Trend,
            "core.vwap" | "core.anchored-vwap" => Self::Vwap,
            "core.rsi" | "pro.stoch-rsi" => Self::Oscillators,
            "core.volume" | "core.cumulative-volume-delta" | "pro.pro-cvd" | "core.bar-analysis"
            | "pro.order-flow-bias" => Self::OrderFlow,
            "core.open-interest" | "pro.pro-open-interest" | "pro.oi-weighted-delta"
            | "pro.oi-flow-net" | "pro.oi-delta-divergence" | "pro.liquidation-pressure" => {
                Self::OpenInterest
            }
            _ => Self::Other,
        }
    }

    fn for_study(study: KlineStudy) -> Self {
        match study {
            KlineStudy::VolumeProfile { .. } | KlineStudy::TpoProfile { .. } => Self::Profiles,
            KlineStudy::BigTrades { .. } => Self::OrderFlow,
        }
    }

    fn for_pro(id: &str) -> Self {
        match id {
            "SessionVwap" | "AnchoredVwap" => Self::Vwap,
            "VolumeProfile" | "FixedVolumeProfile" => Self::Profiles,
            "OrderBook" => Self::OrderFlow,
            "LiquidationHeatmap" => Self::OpenInterest,
            _ => Self::Other,
        }
    }

    const fn label(self) -> &'static str {
        match self {
            Self::Trend => "Trend",
            Self::Vwap => "VWAP",
            Self::Oscillators => "Oscillators",
            Self::OrderFlow => "Volume & Order Flow",
            Self::OpenInterest => "Open Interest & Liquidations",
            Self::Profiles => "Profiles",
            Self::Other => "Other",
        }
    }
}

impl Default for ViewState {
    fn default() -> Self {
        Self {
            search_query: String::new(),
            search_box_id: iced::widget::Id::unique(),
            scroll_id: iced::widget::Id::unique(),
        }
    }
}

impl ViewState {
    pub(super) fn set_search(&mut self, query: String) {
        self.search_query = query;
    }

    pub(super) fn search_box_id(&self) -> iced::widget::Id {
        self.search_box_id.clone()
    }

    pub(super) fn view<'a>(&'a self, chart: &'a KlineChart) -> Element<'a, BrowserMessage> {
        let active_indicators = chart
            .selected_indicators()
            .into_iter()
            .filter_map(|id| chart.indicator_kind(id))
            .collect::<Vec<_>>();

        let active_indicator_counts = group_active_indicators(&active_indicators);
        let available_indicators: Vec<_> = all_indicators()
            .into_iter()
            .map(|indicator| {
                let unavailable_reason = IndicatorPresentationResolver::new(
                    &chart.indicators,
                    &chart.data,
                    chart.basis,
                    chart.trade_data_enabled(),
                )
                .unavailable_reason(&indicator);

                let active_count = active_indicator_counts
                    .iter()
                    .find(|active| active.kind == indicator)
                    .map_or(0, |active| active.count);

                AvailableIndicator {
                    indicator,
                    unavailable_reason,
                    active_count,
                }
            })
            .filter(|available| self.matches_query(&available.indicator.to_string()))
            .collect();

        let available_studies = KlineStudy::ALL
            .into_iter()
            .map(|study| study.normalized_for_basis(chart.basis()).unwrap_or(study))
            .map(|study| {
                let availability =
                    chart
                        .studies
                        .availability(study, chart.basis, chart.trade_data_enabled());

                let active = chart.studies().any(|active| active.is_same_type(study));

                AvailableStudy {
                    study,
                    availability,
                    active,
                }
            })
            .filter(|available| self.matches_query(&available.study.to_string()))
            .collect();

        let available_column = available_column(chart, self, available_indicators, available_studies);

        let search = text_input("Search indicators and studies...", self.search_query())
            .style(|theme, status| style::validated_text_input(theme, status, true))
            .on_input(BrowserMessage::SearchChanged)
            .id(self.search_box_id())
            .padding(6)
            .width(Length::Fill);

        let scrollable = scrollable::Scrollable::with_direction(
            available_column,
            scrollable::Direction::Vertical(
                scrollable::Scrollbar::new().width(4).scroller_width(4),
            ),
        )
        .id(self.scroll_id())
        .height(Length::Fill)
        .style(style::scroll_bar);

        container(column![search, scrollable].spacing(12))
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }

    fn scroll_id(&self) -> iced::widget::Id {
        self.scroll_id.clone()
    }

    fn search_query(&self) -> &str {
        &self.search_query
    }

    fn matches_query(&self, label: &str) -> bool {
        let query = self.search_query.trim();
        query.is_empty() || label.to_lowercase().contains(&query.to_lowercase())
    }
}

fn pro_row(chart: &KlineChart, id: &'static str, label: &'static str) -> Element<'static, BrowserMessage> {
    let active = chart.pro_config().is_enabled(id);
    let toggle = button(
        row![
            iced::widget::text(label).size(crate::style::text_size::BODY),
            space::horizontal(),
        ]
        .spacing(6)
        .align_y(Alignment::Center)
        .width(Length::Fill),
    )
    .width(Length::Fill)
    .style(move |theme, status| style::button::indicator_row(theme, status, active))
    .on_press(BrowserMessage::TogglePro(id));
    let mut row = row![toggle].spacing(2).width(Length::Fill);
    if id != "OrderBook" {
        let settings = button(icon_text(Icon::Cog, 12))
            .style(move |theme, status| style::button::transparent(theme, status, false))
            .on_press(BrowserMessage::ConfigurePro(id));
        row = row.push(settings);
    }
    row.into()
}

fn available_column(
    chart: &KlineChart,
    picker: &ViewState,
    indicators: Vec<AvailableIndicator>,
    studies: Vec<AvailableStudy>,
) -> Element<'static, BrowserMessage> {
    let groups = indicator_group_views(chart, picker, indicators, studies);
    let sections = vec![InspectorSection::new(
        indicators_title(),
        iced::widget::Column::with_children(groups)
            .spacing(10)
            .width(Length::Fill),
    )];

    InspectorContent::new(sections).into_element()
}

fn indicator_group_views(
    chart: &KlineChart,
    picker: &ViewState,
    indicators: Vec<AvailableIndicator>,
    studies: Vec<AvailableStudy>,
) -> Vec<Element<'static, BrowserMessage>> {
    let mut entries: Vec<(MenuGroup, String, Element<'static, BrowserMessage>)> = Vec::new();
    for available in indicators {
        let label = available.indicator.to_string();
        let group = MenuGroup::for_indicator(&available.indicator);
        let row = indicator_row(
            available.indicator,
            available.unavailable_reason,
            available.active_count,
        );
        entries.push((group, label, row));
    }
    for available in studies {
        entries.push((
            MenuGroup::for_study(available.study),
            available.study.to_string(),
            study_row(available.study, available.availability, available.active),
        ));
    }
    for (id, label) in crate::screen::dashboard::pane::content::kline::settings::pro_indicators::ITEMS {
        if picker.matches_query(label) {
            entries.push((MenuGroup::for_pro(id), (*label).to_owned(), pro_row(chart, id, label)));
        }
    }
    entries.sort_by_cached_key(|(group, label, _)| (*group, label.to_lowercase()));

    let mut views = Vec::new();
    let mut entries = entries.into_iter().peekable();
    while let Some((group, _, row)) = entries.next() {
        let mut rows = vec![row];
        while let Some((_, _, row)) = entries.next_if(|(next, ..)| *next == group) {
            rows.push(row);
        }
        views.push(
            column![
                crate::widget::text::emphasis(group.label()),
                iced::widget::Column::with_children(rows)
                    .spacing(2)
                    .width(Length::Fill),
            ]
            .padding([0, 4])
            .spacing(4)
            .width(Length::Fill)
            .into(),
        );
    }
    views
}

fn indicators_title() -> Element<'static, BrowserMessage> {
    row![
        crate::widget::text::title("Indicators"),
        space::horizontal(),
        iced::widget::tooltip(
            button(crate::widget::text::title("+"))
                .style(|theme, status| style::button::transparent(theme, status, false))
                .on_press(BrowserMessage::OpenCatalog),
            container(iced::widget::text("Get more indicators"))
                .style(style::tooltip)
                .padding(8),
            Position::Bottom,
        ),
    ]
    .align_y(Alignment::Center)
    .into()
}

fn group_active_indicators(indicators: &[IndicatorId]) -> Vec<ActiveIndicator> {
    let mut grouped: Vec<ActiveIndicator> = Vec::new();

    for kind in indicators {
        if let Some(active) = grouped.iter_mut().find(|active| active.kind == *kind) {
            active.count += 1;
        } else {
            grouped.push(ActiveIndicator {
                kind: kind.clone(),
                count: 1,
            });
        }
    }

    grouped
}

fn count_badge(count: usize) -> Option<Element<'static, BrowserMessage>> {
    (count > 0)
        .then(|| format!("x{count}"))
        .map(|label| crate::widget::text::muted(label).into())
}

fn indicator_row(
    indicator: IndicatorId,
    unavailable_reason: Option<IndicatorUnsupportedReason>,
    active_count: usize,
) -> Element<'static, BrowserMessage> {
    let mut content = row![
        iced::widget::text(indicator.to_string()).size(crate::style::text_size::BODY),
        space::horizontal()
    ]
    .spacing(4);

    if let Some(badge) = count_badge(active_count) {
        content = content.push(badge);
    }

    let active = active_count > 0;
    let toggle = unavailable_reason
        .is_none()
        .then_some(BrowserMessage::ToggleIndicator(indicator));

    let row = picker_button(
        content.align_y(Alignment::Center).width(Length::Fill),
        toggle,
        active,
    );

    availability_tooltip(
        row,
        unavailable_reason.map(|reason| reason.indicator_unavailable_tooltip().to_owned()),
    )
}

fn study_row(
    study: KlineStudy,
    availability: StudyAvailability,
    active: bool,
) -> Element<'static, BrowserMessage> {
    let mut content = row![
        iced::widget::text(study.to_string()).size(crate::style::text_size::BODY),
        space::horizontal()
    ];

    if active {
        content = content.push(icon_text(Icon::Checkmark, 12));
    }

    let message = (availability == StudyAvailability::Available && !active)
        .then_some(BrowserMessage::ToggleStudy(study));

    let row = picker_button(
        content.align_y(Alignment::Center).width(Length::Fill),
        message,
        active,
    );

    let tooltip = match availability {
        StudyAvailability::Available => None,
        StudyAvailability::UnsupportedBasis => {
            Some("Not available for the selected chart basis".to_owned())
        }
        StudyAvailability::RequiresTradeData => {
            Some("Enable trade data to use this study".to_owned())
        }
        StudyAvailability::SourceConflict(source) => Some(format!(
            "Source conflict:\n Remove active {} studies before adding this.",
            source_label(source),
        )),
    };

    availability_tooltip(row, tooltip)
}

fn source_label(source: StudyDataSource) -> &'static str {
    match source {
        StudyDataSource::Kline => "Candles",
        StudyDataSource::Trades => "Trades",
    }
}

fn picker_button(
    content: impl Into<Element<'static, BrowserMessage>>,
    message: Option<BrowserMessage>,
    active: bool,
) -> button::Button<'static, BrowserMessage> {
    button(content)
        .width(Length::Fill)
        .style(move |theme, status| style::button::indicator_row(theme, status, active))
        .on_press_maybe(message)
}

fn availability_tooltip(
    row: button::Button<'static, BrowserMessage>,
    message: Option<String>,
) -> Element<'static, BrowserMessage> {
    match message {
        Some(message) => iced::widget::tooltip(
            row,
            container(iced::widget::text(message))
                .style(style::tooltip)
                .padding(8),
            Position::Left,
        )
        .into(),
        None => row.into(),
    }
}
