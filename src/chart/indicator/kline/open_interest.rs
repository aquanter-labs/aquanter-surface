use crate::chart::{
    Basis, Caches, Message, ViewState,
    indicator::{
        indicator_row,
        kline::{AvailabilityCause, FetchCtx, IndicatorAvailability, KlineIndicatorImpl},
        plot::{AnySeries, PlotTooltip, candle::StepCandlePlot, line::LinePlot},
    },
};
use crate::connector::fetcher::FetchRange;

use data::chart::{
    PlotData,
    kline::{IndicatorPlotStyle, IndicatorSeriesConfig, KlineDataPoint},
};
use data::util::format_with_commas;
use exchange::adapter::Exchange;
use exchange::{Kline, Trade, UnixMs};

use iced::widget::{center, row, text};
use std::{collections::BTreeMap, ops::RangeInclusive};

pub struct OpenInterestIndicator {
    config: IndicatorSeriesConfig,
    cache: Caches,
    pub data: BTreeMap<UnixMs, f64>,
}

impl OpenInterestIndicator {
    pub fn new(config: IndicatorSeriesConfig) -> Self {
        Self {
            config,
            cache: Caches::default(),
            data: BTreeMap::new(),
        }
    }

    fn indicator_elem<'a>(
        &'a self,
        main_chart: &'a ViewState,
        data_labels_always_visible: bool,
        visible_range: RangeInclusive<u64>,
    ) -> iced::Element<'a, Message> {
        if let Some(message) = self.unavailable_message(main_chart, "Open Interest") {
            return center(text(message)).into();
        }

        let (earliest, latest) = visible_range.clone().into_inner();
        if latest < earliest {
            return row![].into();
        }

        let tooltip = |value: &f64, next: Option<&f64>| {
            let value_text = format!("Open Interest: {}", format_with_commas(*value));
            let change_text = if let Some(next_value) = next {
                let delta = next_value - *value;
                let sign = if delta >= 0.0 { "+" } else { "" };
                format!("Change: {}{}", sign, format_with_commas(delta))
            } else {
                "Change: N/A".to_string()
            };
            PlotTooltip::new(format!("{value_text}\n{change_text}"))
        };

        let value_fn = |v: &f64| *v as f32;

        match self.config.style {
            IndicatorPlotStyle::Line => {
                let plot = LinePlot::new(value_fn)
                    .stroke_width(self.config.line_width.clamp(0.5, 5.0))
                    .color(self.config.line_color)
                    .show_points(true)
                    .point_radius_factor(0.2)
                    .shift(-1)
                    .padding(0.08)
                    .with_tooltip(tooltip);
                indicator_row(
                    main_chart,
                    &self.cache,
                    data_labels_always_visible,
                    plot,
                    AnySeries::forward_unix_ms(&self.data),
                    visible_range,
                )
            }
            IndicatorPlotStyle::Candles => {
                let plot = StepCandlePlot::new(value_fn, self.config)
                    .shift(-1)
                    .with_tooltip(tooltip);
                indicator_row(
                    main_chart,
                    &self.cache,
                    data_labels_always_visible,
                    plot,
                    AnySeries::forward_unix_ms(&self.data),
                    visible_range,
                )
            }
        }
    }

    // helper to compute (earliest, latest) present OI keys
    fn oi_timerange(&self, latest_kline: UnixMs) -> (UnixMs, UnixMs) {
        let mut from_time = latest_kline;
        let mut to_time = UnixMs::ZERO;

        self.data.iter().for_each(|(time, _)| {
            from_time = from_time.min(*time);
            to_time = to_time.max(*time);
        });
        (from_time, to_time)
    }

    fn availability_for(basis: Basis, exchange: Exchange) -> IndicatorAvailability {
        match basis {
            Basis::Tick(_) => IndicatorAvailability::Unavailable(AvailabilityCause::Basis(basis)),
            Basis::Time(timeframe) => {
                if !exchange.is_perps() {
                    IndicatorAvailability::Unavailable(AvailabilityCause::Exchange(exchange))
                } else if exchange.open_interest_source_timeframe(timeframe).is_none() {
                    IndicatorAvailability::Unavailable(AvailabilityCause::Timeframe(timeframe))
                } else {
                    IndicatorAvailability::Available
                }
            }
        }
    }
}

impl KlineIndicatorImpl for OpenInterestIndicator {
    fn clear_all_caches(&mut self) {
        self.cache.clear_all();
    }

    fn clear_crosshair_caches(&mut self) {
        self.cache.clear_crosshair();
    }

    fn element<'a>(
        &'a self,
        chart: &'a ViewState,
        data_labels_always_visible: bool,
        visible_range: RangeInclusive<u64>,
    ) -> iced::Element<'a, Message> {
        self.indicator_elem(chart, data_labels_always_visible, visible_range)
    }

    fn availability(&self, chart: &ViewState) -> IndicatorAvailability {
        Self::availability_for(chart.basis, chart.ticker_info.exchange())
    }

    fn fetch_range(&mut self, ctx: &FetchCtx) -> Option<FetchRange> {
        let availability = Self::availability_for(
            Basis::Time(ctx.timeframe),
            ctx.main_chart.ticker_info.exchange(),
        );
        if !matches!(availability, IndicatorAvailability::Available) {
            return None;
        }

        let (oi_earliest, oi_latest) = self.oi_timerange(ctx.kline_latest);

        if ctx.visible_earliest < oi_earliest {
            return Some(FetchRange::OpenInterest(ctx.prefetch_earliest, oi_earliest));
        }

        if oi_latest < ctx.kline_latest {
            return Some(FetchRange::OpenInterest(
                oi_latest.max(ctx.prefetch_earliest),
                ctx.kline_latest,
            ));
        }

        None
    }

    fn rebuild_from_source(&mut self, _source: &PlotData<KlineDataPoint>) {
        // OI comes from network via external fetches(trade-fetch alike)
        self.clear_all_caches();
    }

    fn on_insert_klines(&mut self, _klines: &[Kline], _source: &PlotData<KlineDataPoint>) {}

    fn on_insert_trades(
        &mut self,
        _trades: &[Trade],
        _old_dp_len: usize,
        _source: &PlotData<KlineDataPoint>,
    ) {
    }

    fn on_ticksize_change(&mut self, _source: &PlotData<KlineDataPoint>) {}

    fn on_basis_change(&mut self, _source: &PlotData<KlineDataPoint>) {}

    fn on_open_interest(&mut self, data: &[exchange::OpenInterest]) {
        self.data.extend(data.iter().map(|oi| (oi.time, oi.value)));
        self.clear_all_caches();
    }

    fn on_visual_config_change(
        &mut self,
        config: &data::chart::kline::Config,
        _source: &PlotData<KlineDataPoint>,
    ) {
        self.config = config.open_interest;
        self.clear_all_caches();
    }
}
