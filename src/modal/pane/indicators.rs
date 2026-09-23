use crate::screen::dashboard::pane::{self, Message};
use crate::style::{self, Icon, icon_text};
use crate::widget::scrollable_content;
use data::chart::indicator::{Indicator, UiIndicator};
use iced::{
    Element, Length, padding,
    widget::{button, column, container, pane_grid, row, space, text},
};

pub fn view<'a, I>(
    pane: pane_grid::Pane,
    state: &'a pane::State,
    selected: &[I],
    market_type: Option<exchange::adapter::MarketKind>,
) -> Element<'a, Message>
where
    I: Indicator + Copy + Into<UiIndicator>,
{
    let content_row = if let Some(market) = market_type {
        content_row(pane, &state.content, selected, market)
    } else {
        column![].spacing(4).into()
    };

    container(scrollable_content(content_row))
        .width(280)
        .max_width(280)
        .padding(16)
        .style(style::chart_modal)
        .into()
}

fn build_indicator_row<'a, I>(
    pane: pane_grid::Pane,
    indicator: &I,
    is_selected: bool,
) -> Element<'a, Message>
where
    I: Indicator + Copy + Into<UiIndicator>,
{
    let content = if is_selected {
        row![
            text(indicator.to_string()),
            space::horizontal(),
            container(icon_text(Icon::Checkmark, 12)),
        ]
        .width(Length::Fill)
    } else {
        row![text(indicator.to_string())].width(Length::Fill)
    };

    let toggle = button(content)
        .on_press(Message::PaneEvent(
            pane,
            pane::Event::ToggleIndicator((*indicator).into()),
        ))
        .width(Length::Fill)
        .style(move |theme, status| style::button::menu_body(theme, status, is_selected));

    let ui_indicator = (*indicator).into();
    if is_selected
        && let data::chart::indicator::UiIndicator::Kline(kline) = ui_indicator
        && matches!(
            kline,
            data::chart::indicator::KlineIndicator::SessionVwap
                | data::chart::indicator::KlineIndicator::CumulativeDelta
                | data::chart::indicator::KlineIndicator::StochRsi
                | data::chart::indicator::KlineIndicator::OpenInterest
                | data::chart::indicator::KlineIndicator::AnchoredVwap
                | data::chart::indicator::KlineIndicator::Ema
                | data::chart::indicator::KlineIndicator::VolumeProfile
                | data::chart::indicator::KlineIndicator::FixedVolumeProfile
                | data::chart::indicator::KlineIndicator::LiquidationHeatmap
                | data::chart::indicator::KlineIndicator::OiDeltaDivergence
                | data::chart::indicator::KlineIndicator::OiFlowNet
                | data::chart::indicator::KlineIndicator::OiWeightedDelta
                | data::chart::indicator::KlineIndicator::LiquidationPressureIndex
                | data::chart::indicator::KlineIndicator::OrderFlowBias
        )
    {
        row![
            toggle,
            button(icon_text(Icon::Cog, 11))
                .on_press(Message::PaneEvent(
                    pane,
                    pane::Event::ShowModal(crate::modal::pane::Modal::IndicatorSettings(kline)),
                ))
                .width(32)
                .style(move |theme, status| style::button::menu_body(theme, status, false))
        ]
        .spacing(4)
        .into()
    } else {
        toggle.into()
    }
}

fn content_row<'a, I>(
    pane: pane_grid::Pane,
    content: &pane::Content,
    selected: &[I],
    market: exchange::adapter::MarketKind,
) -> Element<'a, Message>
where
    I: Indicator + Copy + Into<UiIndicator>,
{
    let indicators: Vec<Element<_>> = I::for_market(market)
        .iter()
        .filter(|indicator| content.allows_indicator((**indicator).into()))
        .map(|indicator| build_indicator_row(pane, indicator, selected.contains(indicator)))
        .collect();

    column![
        container(text("Indicators").size(crate::style::text_size::SECTION))
            .padding(padding::bottom(8)),
        iced::widget::Column::with_children(indicators).spacing(4)
    ]
    .spacing(4)
    .into()
}
