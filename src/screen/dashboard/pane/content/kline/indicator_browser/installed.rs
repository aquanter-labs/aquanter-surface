use super::Message as BrowserMessage;
use crate::screen::dashboard::pane::content::kline::{
    KlineChart, presentation::IndicatorPresentationResolver, studies::StudyAvailability,
};
use crate::style::{self, Icon, icon_text};
use crate::widget::inspector::{InspectorContent, InspectorSection};
use data::chart::kline::IndicatorId;
use data::chart::kline::{
    IndicatorUnsupportedReason, KlineStudy, StudyDataSource,
    indicators::{IndicatorInputSource, all_indicators, declared_input_requirements},
};
use data::i18n::tr;
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IndicatorInputGroup {
    Candles,
    CandlesOrTrades,
    CandlesAndTrades,
    Trades,
    OpenInterest,
    CandlesAndOi,
    CandlesOrTradesAndOi,
    TradesAndOi,
    CandlesTradesAndOi,
    NoExternalData,
}

impl IndicatorInputGroup {
    const ALL: [Self; 10] = [
        Self::Candles,
        Self::CandlesOrTrades,
        Self::CandlesAndTrades,
        Self::Trades,
        Self::OpenInterest,
        Self::CandlesAndOi,
        Self::CandlesOrTradesAndOi,
        Self::TradesAndOi,
        Self::CandlesTradesAndOi,
        Self::NoExternalData,
    ];

    fn for_indicator(indicator: &IndicatorId) -> Self {
        let requirements = declared_input_requirements(indicator);
        let requires = |source| {
            requirements
                .iter()
                .any(|requirement| requirement.source == source && requirement.is_required())
        };

        let requires_directional_volume = requires(IndicatorInputSource::DirectionalVolume);
        if requires_directional_volume {
            return if requires(IndicatorInputSource::OpenInterest) {
                Self::CandlesOrTradesAndOi
            } else {
                Self::CandlesOrTrades
            };
        }

        match (
            requires(IndicatorInputSource::Klines),
            requires(IndicatorInputSource::Trades),
            requires(IndicatorInputSource::OpenInterest),
        ) {
            (true, false, false) => Self::Candles,
            (true, true, false) => Self::CandlesAndTrades,
            (false, true, false) => Self::Trades,
            (false, false, true) => Self::OpenInterest,
            (true, false, true) => Self::CandlesAndOi,
            (false, true, true) => Self::TradesAndOi,
            (true, true, true) => Self::CandlesTradesAndOi,
            (false, false, false) => Self::NoExternalData,
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Candles => tr("Candles"),
            Self::CandlesOrTrades => tr("Candles or trades"),
            Self::CandlesAndTrades => tr("Candles + trades"),
            Self::Trades => tr("Trades"),
            Self::OpenInterest => "OI",
            Self::CandlesAndOi => tr("Candles + OI"),
            Self::CandlesOrTradesAndOi => tr("Candles or trades + OI"),
            Self::TradesAndOi => tr("Trades + OI"),
            Self::CandlesTradesAndOi => tr("Candles + trades + OI"),
            Self::NoExternalData => tr("No external data"),
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

        let pro_group = pro_group_view(chart, self);
        let available_column = available_column(available_indicators, available_studies, pro_group);

        let search = text_input(tr("Search indicators and studies..."), self.search_query())
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

fn available_column(
    indicators: Vec<AvailableIndicator>,
    studies: Vec<AvailableStudy>,
    pro_group: Option<Element<'static, BrowserMessage>>,
) -> Element<'static, BrowserMessage> {
    let mut indicator_groups = indicator_group_views(indicators);
    indicator_groups.extend(pro_group);
    let study_groups = study_group_views(studies);

    let mut sections = Vec::with_capacity(2);
    sections.push(InspectorSection::new(
        indicators_title(),
        iced::widget::Column::with_children(indicator_groups)
            .spacing(10)
            .width(Length::Fill),
    ));
    if !study_groups.is_empty() {
        sections.push(InspectorSection::new(
            crate::widget::text::title(tr("Studies")),
            iced::widget::Column::with_children(study_groups)
                .spacing(10)
                .width(Length::Fill),
        ));
    }

    InspectorContent::new(sections).into_element()
}

/// Pro built-ins are config toggles, not catalog indicators, so they sit in their own group.
fn pro_group_view(
    chart: &KlineChart,
    state: &ViewState,
) -> Option<Element<'static, BrowserMessage>> {
    let rows = crate::screen::dashboard::pane::content::kline::settings::pro_indicators::ITEMS
        .iter()
        .filter(|(_, label)| state.matches_query(label))
        .map(|&(id, label)| pro_row(chart, id, label))
        .collect::<Vec<_>>();
    if rows.is_empty() {
        return None;
    }
    Some(
        column![
            crate::widget::text::emphasis("Pro"),
            iced::widget::Column::with_children(rows)
                .spacing(2)
                .width(Length::Fill),
        ]
        .padding([0, 4])
        .spacing(4)
        .width(Length::Fill)
        .into(),
    )
}

fn pro_row(
    chart: &KlineChart,
    id: &'static str,
    label: &'static str,
) -> Element<'static, BrowserMessage> {
    let active = chart.pro_config().is_enabled(id);
    let toggle = picker_button(
        row![
            iced::widget::text(label).size(crate::style::text_size::BODY),
            space::horizontal()
        ]
        .align_y(Alignment::Center)
        .width(Length::Fill),
        Some(BrowserMessage::TogglePro(id)),
        active,
    );
    let mut row = row![toggle].spacing(2).width(Length::Fill);
    if id != "OrderBook" {
        row = row.push(
            button(icon_text(Icon::Cog, 12))
                .style(|theme, status| style::button::transparent(theme, status, false))
                .on_press(BrowserMessage::ConfigurePro(id)),
        );
    }
    row.into()
}

fn indicators_title() -> Element<'static, BrowserMessage> {
    row![
        crate::widget::text::title(tr("Indicators")),
        space::horizontal(),
        iced::widget::tooltip(
            button(crate::widget::text::title("+"))
                .style(|theme, status| style::button::transparent(theme, status, false))
                .on_press(BrowserMessage::OpenCatalog),
            container(iced::widget::text(tr("Get more indicators")))
                .style(style::tooltip)
                .padding(8),
            Position::Bottom,
        ),
    ]
    .align_y(Alignment::Center)
    .into()
}

fn indicator_group_views(
    indicators: Vec<AvailableIndicator>,
) -> Vec<Element<'static, BrowserMessage>> {
    let mut grouped: Vec<(IndicatorInputGroup, Vec<AvailableIndicator>)> = IndicatorInputGroup::ALL
        .into_iter()
        .map(|group| (group, Vec::new()))
        .collect();

    for available in indicators {
        let group = IndicatorInputGroup::for_indicator(&available.indicator);
        if let Some((_, entries)) = grouped
            .iter_mut()
            .find(|(candidate, _)| *candidate == group)
        {
            entries.push(available);
        }
    }

    grouped
        .into_iter()
        .filter_map(|(group, mut indicators)| {
            if indicators.is_empty() {
                return None;
            }

            indicators
                .sort_by_cached_key(|available| available.indicator.to_string().to_lowercase());

            let rows = indicators
                .into_iter()
                .map(|available| {
                    indicator_row(
                        available.indicator,
                        available.unavailable_reason,
                        available.active_count,
                    )
                })
                .collect::<Vec<_>>();

            Some(
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
            )
        })
        .collect()
}

fn study_group_views(studies: Vec<AvailableStudy>) -> Vec<Element<'static, BrowserMessage>> {
    let mut grouped: Vec<(StudyDataSource, Vec<AvailableStudy>)> =
        [StudyDataSource::Kline, StudyDataSource::Trades]
            .into_iter()
            .map(|source| (source, Vec::new()))
            .collect();

    for available in studies {
        let source = available.study.source();
        if let Some((_, entries)) = grouped
            .iter_mut()
            .find(|(candidate, _)| *candidate == source)
        {
            entries.push(available);
        }
    }

    grouped
        .into_iter()
        .filter_map(|(source, mut studies)| {
            if studies.is_empty() {
                return None;
            }

            studies.sort_by_cached_key(|available| available.study.to_string().to_lowercase());

            let rows = studies
                .into_iter()
                .map(|available| {
                    study_row(available.study, available.availability, available.active)
                })
                .collect::<Vec<_>>();
            let label = source_label(source);

            Some(
                column![
                    crate::widget::text::emphasis(label),
                    iced::widget::Column::with_children(rows)
                        .spacing(2)
                        .width(Length::Fill),
                ]
                .padding([0, 4])
                .spacing(4)
                .width(Length::Fill)
                .into(),
            )
        })
        .collect()
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
            Some(tr("Not available for the selected chart basis").to_owned())
        }
        StudyAvailability::RequiresTradeData => {
            Some(tr("Enable trade data to use this study").to_owned())
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
        StudyDataSource::Kline => tr("Candles"),
        StudyDataSource::Trades => tr("Trades"),
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
