use crate::chart::{Basis, Message, ViewState};
use crate::connector::fetcher::FetchRange;

use data::chart::indicator::KlineIndicator;
use data::chart::kline::{Config, KlineDataPoint};
use data::chart::{BasisSeries, PlotData};
use exchange::adapter::Exchange;
use exchange::{Kline, Timeframe, Trade, UnixMs};

use super::plot::AnySeries;
use iced::{Color, Theme, widget::canvas};

pub mod anchored_vwap;
pub mod bar_analysis;
pub mod cumulative_delta;
pub mod ema;
pub mod flow_metrics;
pub mod liquidation_heatmap;
pub mod open_interest;
pub mod stoch_rsi;
pub mod volume;
pub mod volume_profile;
pub mod vwap;

#[derive(Clone, Copy)]
pub enum ThemeColor {
    Primary,
    Secondary,
    Success,
    Danger,
    Warning,
}

pub fn themed_color(theme: &Theme, configured: Color, default: Color, role: ThemeColor) -> Color {
    if configured != default {
        return configured;
    }
    let palette = theme.extended_palette();
    match role {
        ThemeColor::Primary => palette.primary.strong.color,
        ThemeColor::Secondary => palette.secondary.strong.color,
        ThemeColor::Success => palette.success.base.color,
        ThemeColor::Danger => palette.danger.base.color,
        ThemeColor::Warning => palette.warning.strong.color,
    }
}

/// UI adapter methods for converting domain `BasisSeries` into plot-ready series.
trait BasisSeriesExt<T> {
    fn as_plot_series(&self) -> AnySeries<'_, T>;
}

impl<T> BasisSeriesExt<T> for BasisSeries<T> {
    fn as_plot_series(&self) -> AnySeries<'_, T> {
        match self {
            BasisSeries::Time(data) => AnySeries::forward_unix_ms(data),
            BasisSeries::Tick(data) => AnySeries::reversed_u64(data),
        }
    }
}

#[allow(dead_code)]
#[derive(Debug, Clone, Default, PartialEq)]
pub enum IndicatorAvailability {
    /// Indicator can be rendered normally.
    #[default]
    Available,
    /// Availability cannot be determined yet (e.g. no datapoints loaded).
    Unknown,
    /// Indicator cannot be rendered for the current source/context.
    Unavailable(AvailabilityCause),
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq)]
pub enum AvailabilityCause {
    Exchange(Exchange),
    Timeframe(Timeframe),
    Basis(Basis),
    TradeData,
}

impl IndicatorAvailability {
    pub fn unavailable_message(&self, indicator: &str) -> Option<String> {
        match self {
            IndicatorAvailability::Available | IndicatorAvailability::Unknown => None,
            IndicatorAvailability::Unavailable(cause) => Some(match cause {
                AvailabilityCause::Exchange(exchange) => {
                    format!("{indicator} is not available for {exchange}.")
                }
                AvailabilityCause::Timeframe(timeframe) => {
                    format!("{indicator} is not available on {timeframe} timeframe.")
                }
                AvailabilityCause::Basis(Basis::Tick(_)) => {
                    format!("{indicator} is not available for tick charts.")
                }
                AvailabilityCause::Basis(basis) => {
                    format!("{indicator} is not available on {basis} basis.")
                }
                AvailabilityCause::TradeData => {
                    format!("{indicator} requires directional trade-volume data.")
                }
            }),
        }
    }
}

pub trait KlineIndicatorImpl {
    /// Clear all caches for a full redraw
    fn clear_all_caches(&mut self);

    /// Clear caches related to crosshair only
    /// e.g. tooltips and scale labels for a partial redraw
    fn clear_crosshair_caches(&mut self);

    fn element<'a>(
        &'a self,
        chart: &'a ViewState,
        // Whether to show last value labels on top right/left when not hovering
        data_labels_always_visible: bool,
        visible_range: std::ops::RangeInclusive<u64>,
    ) -> iced::Element<'a, Message>;

    fn availability(&self, _chart: &ViewState) -> IndicatorAvailability {
        IndicatorAvailability::Available
    }

    fn is_overlay(&self) -> bool {
        false
    }

    fn draw_overlay(
        &self,
        _frame: &mut canvas::Frame,
        _chart: &ViewState,
        _theme: &Theme,
        _visible_range: std::ops::RangeInclusive<u64>,
    ) {
    }

    fn unavailable_message(&self, chart: &ViewState, indicator: &str) -> Option<String> {
        self.availability(chart).unavailable_message(indicator)
    }

    /// If the indicator needs data fetching, return the required range
    fn fetch_range(&mut self, _ctx: &FetchCtx) -> Option<FetchRange> {
        None
    }

    /// Rebuild data using kline(OHLCV) source
    fn rebuild_from_source(&mut self, _source: &PlotData<KlineDataPoint>) {}

    fn on_insert_klines(&mut self, _klines: &[Kline], _source: &PlotData<KlineDataPoint>) {}

    fn on_insert_trades(
        &mut self,
        _trades: &[Trade],
        _old_dp_len: usize,
        _source: &PlotData<KlineDataPoint>,
    ) {
    }

    fn on_ticksize_change(&mut self, _source: &PlotData<KlineDataPoint>) {}

    /// Timeframe/tick interval has changed
    fn on_basis_change(&mut self, _source: &PlotData<KlineDataPoint>) {}

    fn on_open_interest(&mut self, _pairs: &[exchange::OpenInterest]) {}

    fn on_liquidations(&mut self, _liquidations: &[exchange::Liquidation]) {}

    fn on_visual_config_change(&mut self, _config: &Config, _source: &PlotData<KlineDataPoint>) {}
}

pub struct FetchCtx<'a> {
    pub main_chart: &'a ViewState,
    pub timeframe: Timeframe,
    pub visible_earliest: UnixMs,
    pub kline_latest: UnixMs,
    pub prefetch_earliest: UnixMs,
}

pub fn make_empty(which: KlineIndicator, config: &Config) -> Box<dyn KlineIndicatorImpl> {
    match which {
        KlineIndicator::Volume => Box::new(super::kline::volume::VolumeIndicator::new()),
        KlineIndicator::BarAnalysis => {
            Box::new(super::kline::bar_analysis::BarAnalysisIndicator::new())
        }
        KlineIndicator::CumulativeDelta => Box::new(
            super::kline::cumulative_delta::CumulativeDeltaIndicator::new(config.cumulative_delta),
        ),
        KlineIndicator::OpenInterest => Box::new(
            super::kline::open_interest::OpenInterestIndicator::new(config.open_interest),
        ),
        KlineIndicator::StochRsi => Box::new(super::kline::stoch_rsi::StochRsiIndicator::new(
            config.stoch_rsi,
        )),
        KlineIndicator::SessionVwap => Box::new(super::kline::vwap::SessionVwapIndicator::new(
            config.session_vwap,
        )),
        KlineIndicator::AnchoredVwap => Box::new(
            super::kline::anchored_vwap::AnchoredVwapIndicator::new(config.anchored_vwap),
        ),
        KlineIndicator::Ema => Box::new(super::kline::ema::EmaIndicator::new(config.ema.clone())),
        KlineIndicator::VolumeProfile => Box::new(
            super::kline::volume_profile::VolumeProfileIndicator::new(config.volume_profile),
        ),
        KlineIndicator::LiquidationHeatmap => Box::new(
            super::kline::liquidation_heatmap::LiquidationHeatmapIndicator::new(
                config.liquidation_heatmap,
            ),
        ),
        KlineIndicator::OiDeltaDivergence => Box::new(
            super::kline::flow_metrics::OiDeltaDivergenceIndicator::new(config.oi_delta_divergence),
        ),
        KlineIndicator::OiFlowNet => Box::new(super::kline::flow_metrics::OiFlowNetIndicator::new(
            config.oi_flow_net,
        )),
        KlineIndicator::OiWeightedDelta => Box::new(
            super::kline::flow_metrics::OiWeightedDeltaIndicator::new(config.oi_weighted_delta),
        ),
        KlineIndicator::LiquidationPressureIndex => Box::new(
            super::kline::flow_metrics::LiquidationPressureIndicator::new(
                config.liquidation_pressure,
            ),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_indicator_colors_follow_theme_but_custom_colors_do_not() {
        let theme = Theme::Dark;
        let default = Color::from_rgb(0.1, 0.2, 0.3);
        assert_eq!(
            themed_color(&theme, default, default, ThemeColor::Success),
            theme.extended_palette().success.base.color
        );
        let custom = Color::from_rgb(0.8, 0.2, 0.1);
        assert_eq!(
            themed_color(&theme, custom, default, ThemeColor::Success),
            custom
        );
    }
}
