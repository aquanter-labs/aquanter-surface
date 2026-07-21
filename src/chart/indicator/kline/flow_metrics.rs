use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use data::chart::kline::{
    KlineDataPoint, LiquidationPressureConfig, OiDeltaDivergenceConfig, OiFlowNetConfig,
    OiWeightedDeltaConfig,
};
use data::chart::{Basis, PlotData};
use data::util::abbr_large_numbers;
use exchange::adapter::Exchange;
use exchange::{Kline, OpenInterest, Trade, UnixMs};
use iced::widget::{canvas, center, text};
use iced::{Alignment, Element, Point, Size, Theme};

use super::{AvailabilityCause, FetchCtx, IndicatorAvailability, KlineIndicatorImpl};
use crate::chart::indicator::indicator_row;
use crate::chart::indicator::plot::{
    AnySeries, Plot, PlotTooltip, Series, TooltipFn, YScale, bar::SignedBarPlot,
};
use crate::chart::{Caches, Message, ViewState};
use crate::connector::fetcher::FetchRange;

#[derive(Clone, Copy)]
struct FlowBar {
    high: f64,
    low: f64,
    close: f64,
    delta: f64,
}

#[derive(Clone, Copy)]
struct AlignedFlow {
    time: UnixMs,
    bar: FlowBar,
    oi: f64,
    previous_oi: f64,
}

#[derive(Default)]
struct FlowSource {
    bars: BTreeMap<UnixMs, FlowBar>,
    oi: BTreeMap<UnixMs, f64>,
}

impl FlowSource {
    fn rebuild(&mut self, source: &PlotData<KlineDataPoint>) {
        let PlotData::TimeBased(series) = source else {
            self.bars.clear();
            return;
        };
        self.bars = series
            .datapoints
            .iter()
            .map(|(&time, point)| {
                (
                    time,
                    FlowBar {
                        high: point.kline.high.to_f64(),
                        low: point.kline.low.to_f64(),
                        close: point.kline.close.to_f64(),
                        delta: point.volume_delta().to_f64(),
                    },
                )
            })
            .collect();
    }

    fn insert_oi(&mut self, data: &[OpenInterest]) {
        self.oi
            .extend(data.iter().map(|item| (item.time, item.value)));
    }

    fn aligned(&self) -> Vec<AlignedFlow> {
        let mut result = Vec::new();
        let mut previous_oi = None;
        for (&time, &bar) in &self.bars {
            let Some(oi) = self.oi.range(..=time).next_back().map(|(_, value)| *value) else {
                continue;
            };
            if let Some(previous_oi) = previous_oi {
                result.push(AlignedFlow {
                    time,
                    bar,
                    oi,
                    previous_oi,
                });
            }
            previous_oi = Some(oi);
        }
        result
    }
}

fn oi_availability(basis: Basis, exchange: Exchange) -> IndicatorAvailability {
    let Basis::Time(timeframe) = basis else {
        return IndicatorAvailability::Unavailable(AvailabilityCause::Basis(basis));
    };
    if !exchange.is_perps() {
        IndicatorAvailability::Unavailable(AvailabilityCause::Exchange(exchange))
    } else if exchange.open_interest_source_timeframe(timeframe).is_none() {
        IndicatorAvailability::Unavailable(AvailabilityCause::Timeframe(timeframe))
    } else {
        IndicatorAvailability::Available
    }
}

fn oi_fetch_range(oi: &BTreeMap<UnixMs, f64>, ctx: &FetchCtx) -> Option<FetchRange> {
    if oi_availability(
        Basis::Time(ctx.timeframe),
        ctx.main_chart.ticker_info.exchange(),
    ) != IndicatorAvailability::Available
    {
        return None;
    }
    let earliest = oi.keys().next().copied().unwrap_or(ctx.kline_latest);
    let latest = oi.keys().next_back().copied().unwrap_or(UnixMs::ZERO);
    if ctx.visible_earliest < earliest {
        Some(FetchRange::OpenInterest(ctx.prefetch_earliest, earliest))
    } else if latest < ctx.kline_latest {
        Some(FetchRange::OpenInterest(
            latest.max(ctx.prefetch_earliest),
            ctx.kline_latest,
        ))
    } else {
        None
    }
}

fn wilder_rsi(changes: &[f64], period: usize) -> Vec<Option<f64>> {
    let period = period.max(1);
    let mut result = vec![None; changes.len()];
    if changes.len() <= period {
        return result;
    }
    let mut gain = changes[1..=period]
        .iter()
        .map(|change| change.max(0.0))
        .sum::<f64>()
        / period as f64;
    let mut loss = changes[1..=period]
        .iter()
        .map(|change| (-change).max(0.0))
        .sum::<f64>()
        / period as f64;
    result[period] = Some(rsi_value(gain, loss));
    for index in (period + 1)..changes.len() {
        gain = (gain * (period - 1) as f64 + changes[index].max(0.0)) / period as f64;
        loss = (loss * (period - 1) as f64 + (-changes[index]).max(0.0)) / period as f64;
        result[index] = Some(rsi_value(gain, loss));
    }
    result
}

fn rsi_value(gain: f64, loss: f64) -> f64 {
    if loss <= f64::EPSILON {
        if gain <= f64::EPSILON { 50.0 } else { 100.0 }
    } else {
        100.0 - 100.0 / (1.0 + gain / loss)
    }
}

fn divergence_direction(
    high: f64,
    low: f64,
    prior_high: f64,
    prior_low: f64,
    cvd_rsi: f64,
    oi_rsi: f64,
    prior_cvd_extreme: (f64, f64),
    prior_oi_extreme: (f64, f64),
) -> Option<bool> {
    if high > prior_high && cvd_rsi < prior_cvd_extreme.1 && oi_rsi < prior_oi_extreme.1 {
        Some(false)
    } else if low < prior_low && cvd_rsi > prior_cvd_extreme.0 && oi_rsi > prior_oi_extreme.0 {
        Some(true)
    } else {
        None
    }
}

#[derive(Clone, Copy)]
struct DivergenceMarker {
    time: UnixMs,
    price: f64,
    bullish: bool,
    strength: f32,
}

fn draw_direction_arrow(
    frame: &mut canvas::Frame,
    x: f32,
    candle_y: f32,
    bullish: bool,
    size_px: f32,
    scaling: f32,
    color: iced::Color,
) -> Point {
    let size = size_px / scaling;
    let gap = 5.0 / scaling;
    let tip_y = if bullish {
        candle_y + gap
    } else {
        candle_y - gap
    };
    let base_y = if bullish { tip_y + size } else { tip_y - size };
    let mut builder = canvas::path::Builder::new();
    builder.move_to(Point::new(x, tip_y));
    builder.line_to(Point::new(x - size, base_y));
    builder.line_to(Point::new(x + size, base_y));
    builder.close();
    frame.fill(&builder.build(), color);
    frame.fill_rectangle(
        Point::new(x - size * 0.3, if bullish { base_y } else { base_y - size }),
        Size::new(size * 0.6, size),
        color,
    );
    Point::new(
        x,
        if bullish {
            base_y + size
        } else {
            base_y - size
        },
    )
}

#[derive(Clone, Copy)]
struct RsiPoint {
    cvd: f64,
    oi: f64,
}

struct RsiPlot {
    config: OiDeltaDivergenceConfig,
    tooltip: TooltipFn<RsiPoint>,
}

impl Plot<AnySeries<'_, RsiPoint>> for RsiPlot {
    fn y_extents(
        &self,
        data: &AnySeries<'_, RsiPoint>,
        range: RangeInclusive<u64>,
    ) -> Option<(f32, f32)> {
        let mut found = false;
        data.for_each_in(range, |_, _| found = true);
        found.then_some((0.0, 100.0))
    }

    fn draw(
        &self,
        frame: &mut canvas::Frame,
        chart: &ViewState,
        theme: &Theme,
        data: &AnySeries<'_, RsiPoint>,
        range: RangeInclusive<u64>,
        scale: &YScale,
    ) {
        let guide = theme
            .extended_palette()
            .background
            .strong
            .text
            .scale_alpha(0.2);
        let visible_width = frame.width() / chart.scaling;
        let left = -chart.translation.x - visible_width / 2.0;
        let right = left + visible_width;
        for value in [30.0, 70.0] {
            let y = scale.to_y(value);
            frame.stroke(
                &canvas::Path::line(Point::new(left, y), Point::new(right, y)),
                canvas::Stroke::default().with_width(1.0).with_color(guide),
            );
        }
        let width = self.config.line_width.clamp(0.5, 5.0);
        let mut previous_cvd = None;
        let mut previous_oi = None;
        data.for_each_in(range, |time, point| {
            let x = chart.interval_to_x(time);
            let cvd = Point::new(x, scale.to_y(point.cvd as f32));
            let oi = Point::new(x, scale.to_y(point.oi as f32));
            if let Some(previous) = previous_cvd {
                frame.stroke(
                    &canvas::Path::line(previous, cvd),
                    canvas::Stroke::default()
                        .with_width(width)
                        .with_color(self.config.cvd_color),
                );
            }
            if let Some(previous) = previous_oi {
                frame.stroke(
                    &canvas::Path::line(previous, oi),
                    canvas::Stroke::default()
                        .with_width(width)
                        .with_color(self.config.oi_color),
                );
            }
            previous_cvd = Some(cvd);
            previous_oi = Some(oi);
        });
    }

    fn tooltip_fn(&self) -> Option<&TooltipFn<RsiPoint>> {
        Some(&self.tooltip)
    }
}

pub struct OiDeltaDivergenceIndicator {
    config: OiDeltaDivergenceConfig,
    cache: Caches,
    source: FlowSource,
    data: BTreeMap<UnixMs, RsiPoint>,
    markers: Vec<DivergenceMarker>,
}

impl OiDeltaDivergenceIndicator {
    pub fn new(config: OiDeltaDivergenceConfig) -> Self {
        Self {
            config,
            cache: Caches::default(),
            source: FlowSource::default(),
            data: BTreeMap::new(),
            markers: Vec::new(),
        }
    }

    fn recompute(&mut self) {
        self.markers.clear();
        self.data.clear();
        let points = self.source.aligned();
        let cvd_changes = points
            .iter()
            .map(|point| point.bar.delta)
            .collect::<Vec<_>>();
        let oi_changes = points
            .iter()
            .map(|point| point.oi - point.previous_oi)
            .collect::<Vec<_>>();
        let period = usize::from(self.config.rsi_period.max(2));
        let cvd_rsi = wilder_rsi(&cvd_changes, period);
        let oi_rsi = wilder_rsi(&oi_changes, period);
        for (index, point) in points.iter().enumerate() {
            if let (Some(cvd), Some(oi)) = (cvd_rsi[index], oi_rsi[index]) {
                self.data.insert(point.time, RsiPoint { cvd, oi });
            }
        }
        let lookback = usize::from(self.config.lookback.max(2));

        for index in lookback.max(period)..points.len() {
            let (Some(cvd), Some(oi)) = (cvd_rsi[index], oi_rsi[index]) else {
                continue;
            };
            let start = index - lookback;
            let previous = &points[start..index];
            let previous_cvd = cvd_rsi[start..index].iter().flatten().copied();
            let previous_oi = oi_rsi[start..index].iter().flatten().copied();
            let Some(max_cvd) = previous_cvd.clone().reduce(f64::max) else {
                continue;
            };
            let Some(min_cvd) = previous_cvd.reduce(f64::min) else {
                continue;
            };
            let Some(max_oi) = previous_oi.clone().reduce(f64::max) else {
                continue;
            };
            let Some(min_oi) = previous_oi.reduce(f64::min) else {
                continue;
            };
            let prior_high = previous
                .iter()
                .map(|point| point.bar.high)
                .fold(f64::NEG_INFINITY, f64::max);
            let prior_low = previous
                .iter()
                .map(|point| point.bar.low)
                .fold(f64::INFINITY, f64::min);
            let point = points[index];
            if let Some(bullish) = divergence_direction(
                point.bar.high,
                point.bar.low,
                prior_high,
                prior_low,
                cvd,
                oi,
                (min_cvd, max_cvd),
                (min_oi, max_oi),
            ) {
                self.markers.push(DivergenceMarker {
                    time: point.time,
                    price: if bullish {
                        point.bar.low
                    } else {
                        point.bar.high
                    },
                    bullish,
                    strength: if bullish {
                        ((cvd - min_cvd).abs() + (oi - min_oi).abs()) as f32 / 25.0
                    } else {
                        ((max_cvd - cvd).abs() + (max_oi - oi).abs()) as f32 / 25.0
                    },
                });
            }
        }
        self.cache.clear_all();
    }
}

impl KlineIndicatorImpl for OiDeltaDivergenceIndicator {
    fn clear_all_caches(&mut self) {
        self.cache.clear_all();
    }
    fn clear_crosshair_caches(&mut self) {
        self.cache.clear_crosshair();
    }
    fn element<'a>(
        &'a self,
        chart: &'a ViewState,
        labels: bool,
        range: RangeInclusive<u64>,
    ) -> Element<'a, Message> {
        if let Some(message) = self.unavailable_message(chart, "OI-Delta Divergence") {
            return center(text(message)).into();
        }
        let plot = RsiPlot {
            config: self.config,
            tooltip: Box::new(|point, _| {
                PlotTooltip::new(format!(
                    "CVD RSI: {:.1}\nOI RSI: {:.1}",
                    point.cvd, point.oi
                ))
            }),
        };
        indicator_row(
            chart,
            &self.cache,
            labels,
            plot,
            AnySeries::forward_unix_ms(&self.data),
            range,
        )
    }
    fn availability(&self, chart: &ViewState) -> IndicatorAvailability {
        oi_availability(chart.basis, chart.ticker_info.exchange())
    }
    fn draw_overlay(
        &self,
        frame: &mut canvas::Frame,
        chart: &ViewState,
        _theme: &Theme,
        visible: RangeInclusive<u64>,
    ) {
        for marker in self
            .markers
            .iter()
            .filter(|marker| visible.contains(&marker.time.as_u64()))
        {
            let size = (self.config.marker_size + marker.strength).clamp(2.0, 12.0);
            let mut color = if marker.bullish {
                self.config.bullish_color
            } else {
                self.config.bearish_color
            };
            color.a = 0.9;
            let y = chart.price_to_y(exchange::unit::Price::from_f64(marker.price));
            draw_direction_arrow(
                frame,
                chart.interval_to_x(marker.time.as_u64()),
                y,
                marker.bullish,
                size,
                chart.scaling,
                color,
            );
        }
    }
    fn fetch_range(&mut self, ctx: &FetchCtx) -> Option<FetchRange> {
        oi_fetch_range(&self.source.oi, ctx)
    }
    fn rebuild_from_source(&mut self, source: &PlotData<KlineDataPoint>) {
        self.source.rebuild(source);
        self.recompute();
    }
    fn on_insert_klines(&mut self, _klines: &[Kline], source: &PlotData<KlineDataPoint>) {
        self.rebuild_from_source(source);
    }
    fn on_insert_trades(
        &mut self,
        _trades: &[Trade],
        _old: usize,
        source: &PlotData<KlineDataPoint>,
    ) {
        self.rebuild_from_source(source);
    }
    fn on_basis_change(&mut self, source: &PlotData<KlineDataPoint>) {
        self.rebuild_from_source(source);
    }
    fn on_open_interest(&mut self, data: &[OpenInterest]) {
        self.source.insert_oi(data);
        self.recompute();
    }
    fn on_visual_config_change(
        &mut self,
        config: &data::chart::kline::Config,
        _source: &PlotData<KlineDataPoint>,
    ) {
        self.config = config.oi_delta_divergence;
        self.recompute();
    }
}

#[derive(Clone, Copy, Debug)]
struct OiFlowPoint {
    long: f64,
    short: f64,
    delta: f64,
    oi_change: f64,
    relative_oi: f64,
    relative_price: f64,
    state: OiFlowState,
    burst: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OiFlowState {
    BuyInflow,
    SellInflow,
    BuyOutflow,
    SellOutflow,
}

impl OiFlowState {
    fn label(self) -> &'static str {
        match self {
            Self::BuyInflow => "Buy inflow",
            Self::SellInflow => "Sell inflow",
            Self::BuyOutflow => "Buy outflow",
            Self::SellOutflow => "Sell outflow",
        }
    }
}

fn oi_flow(oi_change: f64, delta: f64) -> OiFlowPoint {
    let amount = oi_change.abs();
    let allocated_delta = delta.clamp(-amount, amount);
    if oi_change >= 0.0 {
        OiFlowPoint {
            long: (amount + allocated_delta) / 2.0,
            short: (amount - allocated_delta) / 2.0,
            delta,
            oi_change,
            relative_oi: 0.0,
            relative_price: 0.0,
            state: OiFlowState::BuyInflow,
            burst: false,
        }
    } else {
        OiFlowPoint {
            long: -(amount - allocated_delta) / 2.0,
            short: -(amount + allocated_delta) / 2.0,
            delta,
            oi_change,
            relative_oi: 0.0,
            relative_price: 0.0,
            state: OiFlowState::BuyOutflow,
            burst: false,
        }
    }
}

fn relative_values(values: &[f64], basis_length: usize) -> Vec<Option<f64>> {
    let basis_length = basis_length.max(2);
    let mut spreads = vec![None; values.len()];
    let mut sum = 0.0;
    for (index, value) in values.iter().copied().enumerate() {
        sum += value;
        if index >= basis_length {
            sum -= values[index - basis_length];
        }
        if index + 1 >= basis_length {
            let average = sum / basis_length as f64;
            if average.abs() > f64::EPSILON {
                spreads[index] = Some((value / average - 1.0) * 100.0);
            }
        }
    }

    let mut highs = Vec::new();
    let mut lows = Vec::new();
    let mut result = vec![None; values.len()];
    for index in 0..values.len() {
        if index >= 10 {
            let center = index - 5;
            if let Some(candidate) = spreads[center] {
                let window = &spreads[(center - 5)..=(center + 5)];
                if candidate > 0.0 && window.iter().flatten().all(|value| candidate >= *value) {
                    highs.push(candidate);
                    if highs.len() > 20 {
                        highs.remove(0);
                    }
                } else if candidate < 0.0
                    && window.iter().flatten().all(|value| candidate <= *value)
                {
                    lows.push(candidate);
                    if lows.len() > 20 {
                        lows.remove(0);
                    }
                }
            }
        }

        let Some(spread) = spreads[index] else {
            continue;
        };
        let pivots = if spread >= 0.0 { &highs } else { &lows };
        if !pivots.is_empty() {
            let average = pivots.iter().sum::<f64>() / pivots.len() as f64;
            if average.abs() > f64::EPSILON {
                result[index] = Some(if spread >= 0.0 {
                    spread / average
                } else {
                    -(spread / average)
                });
            }
        }
    }
    result
}

fn oi_flow_state(relative_oi: f64, relative_price: f64) -> OiFlowState {
    match (relative_price >= 0.0, relative_oi >= 0.0) {
        (true, true) => OiFlowState::BuyInflow,
        (false, true) => OiFlowState::SellInflow,
        (false, false) => OiFlowState::BuyOutflow,
        (true, false) => OiFlowState::SellOutflow,
    }
}

fn is_oi_burst(current: f64, previous: f64, sensitivity: u8) -> bool {
    let sensitivity = sensitivity.clamp(1, 5) as f64;
    let gain = 1.0 + sensitivity * 0.1;
    let position = 0.7 + sensitivity * 0.1;
    (current > previous * gain && current > position)
        || (current < previous * gain && current < -position)
}

struct OiFlowPlot {
    config: OiFlowNetConfig,
    tooltip: TooltipFn<OiFlowPoint>,
}

impl Plot<AnySeries<'_, OiFlowPoint>> for OiFlowPlot {
    fn y_extents(
        &self,
        data: &AnySeries<'_, OiFlowPoint>,
        range: RangeInclusive<u64>,
    ) -> Option<(f32, f32)> {
        let mut min = -3.0_f32;
        let mut max = 3.0_f32;
        let mut found = false;
        data.for_each_in(range, |_, point| {
            min = min
                .min(point.relative_oi as f32)
                .min(point.relative_price as f32);
            max = max
                .max(point.relative_oi as f32)
                .max(point.relative_price as f32);
            found = true;
        });
        found.then_some((min, max.max(min + f32::EPSILON)))
    }

    fn draw(
        &self,
        frame: &mut canvas::Frame,
        chart: &ViewState,
        theme: &Theme,
        data: &AnySeries<'_, OiFlowPoint>,
        range: RangeInclusive<u64>,
        scale: &YScale,
    ) {
        let palette = theme.extended_palette();
        let left = chart.interval_to_x(*range.start()) - chart.cell_width / 2.0;
        let right = chart.interval_to_x(*range.end()) + chart.cell_width / 2.0;
        for (low, high, color) in [
            (1.0, 2.0, palette.background.strong.color.scale_alpha(0.12)),
            (2.0, 3.0, palette.danger.base.color.scale_alpha(0.10)),
            (
                -2.0,
                -1.0,
                palette.background.strong.color.scale_alpha(0.12),
            ),
            (-3.0, -2.0, palette.danger.base.color.scale_alpha(0.10)),
        ] {
            let y1 = scale.to_y(low);
            let y2 = scale.to_y(high);
            frame.fill_rectangle(
                Point::new(left, y1.min(y2)),
                Size::new((right - left).max(0.0), (y1 - y2).abs()),
                color,
            );
        }

        let zero = scale.to_y(0.0);
        let width = chart.cell_width * 0.8;
        let mut previous_price = None;
        let price_color = palette.secondary.strong.color;
        data.for_each_in(range, |time, point| {
            let center = chart.interval_to_x(time);
            let value = point.relative_oi as f32;
            let y = scale.to_y(value);
            let base_color = match point.state {
                OiFlowState::BuyInflow => self.config.long_color,
                OiFlowState::SellInflow => self.config.short_color,
                OiFlowState::BuyOutflow => self.config.buy_outflow_color,
                OiFlowState::SellOutflow => self.config.sell_outflow_color,
            };
            let color = if self.config.static_color {
                base_color
            } else {
                base_color.scale_alpha(0.3 + 0.7 * value.abs().min(1.0))
            };
            frame.fill_rectangle(
                Point::new(center - width / 2.0, y.min(zero)),
                Size::new(width, (zero - y).abs().max(1.0 / chart.scaling)),
                color,
            );
            if point.burst {
                frame.fill(
                    &canvas::Path::circle(Point::new(center, y), 2.5 / chart.scaling),
                    base_color,
                );
            }
            if self.config.show_price_line {
                let price = Point::new(center, scale.to_y(point.relative_price as f32));
                if let Some(previous) = previous_price {
                    frame.stroke(
                        &canvas::Path::line(previous, price),
                        canvas::Stroke::default()
                            .with_width(1.0 / chart.scaling)
                            .with_color(price_color),
                    );
                }
                previous_price = Some(price);
            }
        });
    }

    fn tooltip_fn(&self) -> Option<&TooltipFn<OiFlowPoint>> {
        Some(&self.tooltip)
    }
}

pub struct OiFlowNetIndicator {
    config: OiFlowNetConfig,
    cache: Caches,
    source: FlowSource,
    data: BTreeMap<UnixMs, OiFlowPoint>,
}

impl OiFlowNetIndicator {
    pub fn new(config: OiFlowNetConfig) -> Self {
        Self {
            config,
            cache: Caches::default(),
            source: FlowSource::default(),
            data: BTreeMap::new(),
        }
    }

    fn recompute(&mut self) {
        let aligned = self.source.aligned();
        let relative_oi = relative_values(
            &aligned.iter().map(|point| point.oi).collect::<Vec<_>>(),
            self.config.basis_length as usize,
        );
        let relative_price = relative_values(
            &aligned
                .iter()
                .map(|point| point.bar.close)
                .collect::<Vec<_>>(),
            self.config.basis_length as usize,
        );
        let mut previous_oi = None;
        self.data = aligned
            .into_iter()
            .zip(relative_oi)
            .zip(relative_price)
            .filter_map(|((point, relative_oi), relative_price)| {
                let relative_oi = relative_oi?;
                let relative_price = relative_price?;
                let mut flow = oi_flow(point.oi - point.previous_oi, point.bar.delta);
                flow.relative_oi = relative_oi;
                flow.relative_price = relative_price;
                flow.state = oi_flow_state(relative_oi, relative_price);
                flow.burst = previous_oi.is_some_and(|previous| {
                    is_oi_burst(relative_oi, previous, self.config.burst_sensitivity)
                });
                previous_oi = Some(relative_oi);
                Some((point.time, flow))
            })
            .collect();
        self.cache.clear_all();
    }
}

impl KlineIndicatorImpl for OiFlowNetIndicator {
    fn clear_all_caches(&mut self) {
        self.cache.clear_all();
    }
    fn clear_crosshair_caches(&mut self) {
        self.cache.clear_crosshair();
    }
    fn element<'a>(
        &'a self,
        chart: &'a ViewState,
        labels: bool,
        range: RangeInclusive<u64>,
    ) -> Element<'a, Message> {
        if let Some(message) = self.unavailable_message(chart, "OI Flow Net") {
            return center(text(message)).into();
        }
        let plot = OiFlowPlot {
            config: self.config,
            tooltip: Box::new(|point, _| {
                PlotTooltip::new(format!(
                    "{}{}\nRelative OI: {:.2}\nRelative price: {:.2}\nDelta: {}\nLong {}: {}\nShort {}: {}",
                    point.state.label(),
                    if point.burst { " (burst)" } else { "" },
                    point.relative_oi,
                    point.relative_price,
                    abbr_large_numbers(point.delta),
                    if point.oi_change >= 0.0 {
                        "inflow"
                    } else {
                        "closing"
                    },
                    abbr_large_numbers(point.long.abs()),
                    if point.oi_change >= 0.0 {
                        "inflow"
                    } else {
                        "closing"
                    },
                    abbr_large_numbers(point.short.abs()),
                ))
            }),
        };
        indicator_row(
            chart,
            &self.cache,
            labels,
            plot,
            AnySeries::forward_unix_ms(&self.data),
            range,
        )
    }
    fn availability(&self, chart: &ViewState) -> IndicatorAvailability {
        oi_availability(chart.basis, chart.ticker_info.exchange())
    }
    fn fetch_range(&mut self, ctx: &FetchCtx) -> Option<FetchRange> {
        oi_fetch_range(&self.source.oi, ctx)
    }
    fn rebuild_from_source(&mut self, source: &PlotData<KlineDataPoint>) {
        self.source.rebuild(source);
        self.recompute();
    }
    fn on_insert_klines(&mut self, _: &[Kline], source: &PlotData<KlineDataPoint>) {
        self.rebuild_from_source(source);
    }
    fn on_insert_trades(&mut self, _: &[Trade], _: usize, source: &PlotData<KlineDataPoint>) {
        self.rebuild_from_source(source);
    }
    fn on_basis_change(&mut self, source: &PlotData<KlineDataPoint>) {
        self.rebuild_from_source(source);
    }
    fn on_open_interest(&mut self, data: &[OpenInterest]) {
        self.source.insert_oi(data);
        self.recompute();
    }
    fn on_visual_config_change(
        &mut self,
        config: &data::chart::kline::Config,
        _: &PlotData<KlineDataPoint>,
    ) {
        self.config = config.oi_flow_net;
        self.recompute();
    }
}

fn weighted_delta(delta: f64, oi: f64, previous_oi: f64, sensitivity: f64, max_weight: f64) -> f64 {
    let oi_change = oi / previous_oi.max(f64::EPSILON) - 1.0;
    let weight = (1.0 + sensitivity * oi_change).clamp(0.0, max_weight.max(1.0));
    delta * weight
}

pub struct OiWeightedDeltaIndicator {
    config: OiWeightedDeltaConfig,
    cache: Caches,
    source: FlowSource,
    data: BTreeMap<UnixMs, f64>,
}

impl OiWeightedDeltaIndicator {
    pub fn new(config: OiWeightedDeltaConfig) -> Self {
        Self {
            config,
            cache: Caches::default(),
            source: FlowSource::default(),
            data: BTreeMap::new(),
        }
    }
    fn recompute(&mut self) {
        self.data = self
            .source
            .aligned()
            .into_iter()
            .map(|point| {
                (
                    point.time,
                    weighted_delta(
                        point.bar.delta,
                        point.oi,
                        point.previous_oi,
                        self.config.sensitivity as f64,
                        self.config.max_weight as f64,
                    ),
                )
            })
            .collect();
        self.cache.clear_all();
    }
}

impl KlineIndicatorImpl for OiWeightedDeltaIndicator {
    fn clear_all_caches(&mut self) {
        self.cache.clear_all();
    }
    fn clear_crosshair_caches(&mut self) {
        self.cache.clear_crosshair();
    }
    fn element<'a>(
        &'a self,
        chart: &'a ViewState,
        labels: bool,
        range: RangeInclusive<u64>,
    ) -> Element<'a, Message> {
        if let Some(message) = self.unavailable_message(chart, "OI-Weighted Delta") {
            return center(text(message)).into();
        }
        let plot = SignedBarPlot::new(
            |value: &f64| *value as f32,
            self.config.bar_width,
            self.config.positive_color,
            self.config.negative_color,
        )
        .with_tooltip(|value: &f64, _| {
            PlotTooltip::new(format!("OI-weighted Delta: {}", abbr_large_numbers(*value)))
        });
        indicator_row(
            chart,
            &self.cache,
            labels,
            plot,
            AnySeries::forward_unix_ms(&self.data),
            range,
        )
    }
    fn availability(&self, chart: &ViewState) -> IndicatorAvailability {
        oi_availability(chart.basis, chart.ticker_info.exchange())
    }
    fn fetch_range(&mut self, ctx: &FetchCtx) -> Option<FetchRange> {
        oi_fetch_range(&self.source.oi, ctx)
    }
    fn rebuild_from_source(&mut self, source: &PlotData<KlineDataPoint>) {
        self.source.rebuild(source);
        self.recompute();
    }
    fn on_insert_klines(&mut self, _: &[Kline], source: &PlotData<KlineDataPoint>) {
        self.rebuild_from_source(source);
    }
    fn on_insert_trades(&mut self, _: &[Trade], _: usize, source: &PlotData<KlineDataPoint>) {
        self.rebuild_from_source(source);
    }
    fn on_basis_change(&mut self, source: &PlotData<KlineDataPoint>) {
        self.rebuild_from_source(source);
    }
    fn on_open_interest(&mut self, data: &[OpenInterest]) {
        self.source.insert_oi(data);
        self.recompute();
    }
    fn on_visual_config_change(
        &mut self,
        config: &data::chart::kline::Config,
        _: &PlotData<KlineDataPoint>,
    ) {
        self.config = config.oi_weighted_delta;
        self.recompute();
    }
}

#[derive(Clone, Copy)]
struct LiquidationSignal {
    score: f64,
    estimated: f64,
    oi_change_pct: f64,
}

fn liquidation_signal(
    point: AlignedFlow,
    previous_close: f64,
    delta_mean: f64,
    delta_stddev: f64,
    config: LiquidationPressureConfig,
) -> LiquidationSignal {
    let oi_change_pct = (point.oi / point.previous_oi.max(f64::EPSILON) - 1.0) * 100.0;
    let threshold = delta_mean + config.delta_zscore.max(0.1) as f64 * delta_stddev;
    let price_change = point.bar.close - previous_close;
    let direction = if point.bar.delta > 0.0 && price_change > 0.0 {
        1.0
    } else if point.bar.delta < 0.0 && price_change < 0.0 {
        -1.0
    } else {
        0.0
    };
    let triggered = oi_change_pct <= -(config.oi_drop_threshold.max(0.01) as f64)
        && point.bar.delta.abs() >= threshold
        && direction != 0.0;
    LiquidationSignal {
        score: if triggered {
            direction
                * ((-oi_change_pct / config.oi_drop_threshold.max(0.01) as f64)
                    * (point.bar.delta.abs() / threshold.max(f64::EPSILON))
                    * 100.0)
                    .min(500.0)
        } else {
            0.0
        },
        estimated: if triggered {
            (point.oi - point.previous_oi).abs()
        } else {
            0.0
        },
        oi_change_pct,
    }
}

pub struct LiquidationPressureIndicator {
    config: LiquidationPressureConfig,
    cache: Caches,
    source: FlowSource,
    data: BTreeMap<UnixMs, LiquidationSignal>,
}

impl LiquidationPressureIndicator {
    pub fn new(config: LiquidationPressureConfig) -> Self {
        Self {
            config,
            cache: Caches::default(),
            source: FlowSource::default(),
            data: BTreeMap::new(),
        }
    }
    fn recompute(&mut self) {
        self.data.clear();
        let points = self.source.aligned();
        let lookback = usize::from(self.config.lookback.max(2));
        for index in 1..points.len() {
            let start = index.saturating_sub(lookback);
            let deltas = points[start..index]
                .iter()
                .map(|point| point.bar.delta.abs())
                .collect::<Vec<_>>();
            if deltas.len() < lookback.min(index) || deltas.is_empty() {
                continue;
            }
            let mean = deltas.iter().sum::<f64>() / deltas.len() as f64;
            let stddev = (deltas
                .iter()
                .map(|value| (value - mean).powi(2))
                .sum::<f64>()
                / deltas.len() as f64)
                .sqrt();
            self.data.insert(
                points[index].time,
                liquidation_signal(
                    points[index],
                    points[index - 1].bar.close,
                    mean,
                    stddev,
                    self.config,
                ),
            );
        }
        self.cache.clear_all();
    }
}

impl KlineIndicatorImpl for LiquidationPressureIndicator {
    fn clear_all_caches(&mut self) {
        self.cache.clear_all();
    }
    fn clear_crosshair_caches(&mut self) {
        self.cache.clear_crosshair();
    }
    fn element<'a>(
        &'a self,
        chart: &'a ViewState,
        labels: bool,
        range: RangeInclusive<u64>,
    ) -> Element<'a, Message> {
        if let Some(message) = self.unavailable_message(chart, "Liquidation Pressure Index") {
            return center(text(message)).into();
        }
        let plot = SignedBarPlot::new(
            |value: &LiquidationSignal| value.score as f32,
            self.config.bar_width,
            self.config.positive_color,
            self.config.negative_color,
        )
        .with_tooltip(|value: &LiquidationSignal, _| {
            PlotTooltip::new(format!(
                "LPI: {:.0}\nOI change: {:.2}%\nEstimated liquidation: {}",
                value.score,
                value.oi_change_pct,
                abbr_large_numbers(value.estimated),
            ))
        });
        indicator_row(
            chart,
            &self.cache,
            labels,
            plot,
            AnySeries::forward_unix_ms(&self.data),
            range,
        )
    }
    fn availability(&self, chart: &ViewState) -> IndicatorAvailability {
        oi_availability(chart.basis, chart.ticker_info.exchange())
    }
    fn draw_overlay(
        &self,
        frame: &mut canvas::Frame,
        chart: &ViewState,
        _theme: &Theme,
        visible: RangeInclusive<u64>,
    ) {
        for (&time, signal) in self
            .data
            .range(UnixMs::new(*visible.start())..=UnixMs::new(*visible.end()))
            .filter(|(_, signal)| signal.score != 0.0)
        {
            let Some(bar) = self.source.bars.get(&time) else {
                continue;
            };
            let bullish = signal.score > 0.0;
            let color = if bullish {
                self.config.positive_color
            } else {
                self.config.negative_color
            };
            let price = if bullish { bar.low } else { bar.high };
            let position = Point::new(
                chart.interval_to_x(time.as_u64()),
                chart.price_to_y(exchange::unit::Price::from_f64(price)),
            );
            let label_position = draw_direction_arrow(
                frame,
                position.x,
                position.y,
                bullish,
                4.0,
                chart.scaling,
                color,
            );
            frame.fill_text(canvas::Text {
                content: abbr_large_numbers(signal.estimated),
                position: Point::new(label_position.x + 5.0 / chart.scaling, label_position.y),
                size: iced::Pixels(10.0 / chart.scaling),
                color,
                align_y: Alignment::Center.into(),
                ..canvas::Text::default()
            });
        }
    }
    fn fetch_range(&mut self, ctx: &FetchCtx) -> Option<FetchRange> {
        oi_fetch_range(&self.source.oi, ctx)
    }
    fn rebuild_from_source(&mut self, source: &PlotData<KlineDataPoint>) {
        self.source.rebuild(source);
        self.recompute();
    }
    fn on_insert_klines(&mut self, _: &[Kline], source: &PlotData<KlineDataPoint>) {
        self.rebuild_from_source(source);
    }
    fn on_insert_trades(&mut self, _: &[Trade], _: usize, source: &PlotData<KlineDataPoint>) {
        self.rebuild_from_source(source);
    }
    fn on_basis_change(&mut self, source: &PlotData<KlineDataPoint>) {
        self.rebuild_from_source(source);
    }
    fn on_open_interest(&mut self, data: &[OpenInterest]) {
        self.source.insert_oi(data);
        self.recompute();
    }
    fn on_visual_config_change(
        &mut self,
        config: &data::chart::kline::Config,
        _: &PlotData<KlineDataPoint>,
    ) {
        self.config = config.liquidation_pressure;
        self.recompute();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rsi_flow_weighting_and_liquidation_rules_match_definitions() {
        let rsi = wilder_rsi(&[0.0, 1.0, 1.0, -1.0], 2);
        assert_eq!(rsi[2], Some(100.0));
        assert!((rsi[3].unwrap_or_default() - 50.0).abs() < 0.001);

        let opening = oi_flow(100.0, 60.0);
        assert_eq!((opening.long, opening.short), (80.0, 20.0));
        let closing = oi_flow(-100.0, 60.0);
        assert_eq!((closing.long, closing.short), (-20.0, -80.0));
        assert_eq!(oi_flow_state(1.0, 1.0), OiFlowState::BuyInflow);
        assert_eq!(oi_flow_state(1.0, -1.0), OiFlowState::SellInflow);
        assert_eq!(oi_flow_state(-1.0, -1.0), OiFlowState::BuyOutflow);
        assert_eq!(oi_flow_state(-1.0, 1.0), OiFlowState::SellOutflow);
        assert!(is_oi_burst(1.31, 1.0, 3));
        assert!(!is_oi_burst(1.29, 1.0, 3));

        assert!((weighted_delta(100.0, 102.0, 100.0, 10.0, 2.0) - 120.0).abs() < 0.001);
        assert!((weighted_delta(100.0, 98.0, 100.0, 10.0, 2.0) - 80.0).abs() < 0.001);

        assert_eq!(
            divergence_direction(
                101.0,
                95.0,
                100.0,
                90.0,
                60.0,
                55.0,
                (30.0, 70.0),
                (25.0, 65.0)
            ),
            Some(false)
        );
        assert_eq!(
            divergence_direction(
                99.0,
                89.0,
                100.0,
                90.0,
                40.0,
                45.0,
                (30.0, 70.0),
                (25.0, 65.0)
            ),
            Some(true)
        );

        let signal = liquidation_signal(
            AlignedFlow {
                time: UnixMs::ZERO,
                bar: FlowBar {
                    high: 104.0,
                    low: 99.0,
                    close: 104.0,
                    delta: 100.0,
                },
                oi: 97.0,
                previous_oi: 100.0,
            },
            100.0,
            20.0,
            10.0,
            LiquidationPressureConfig::default(),
        );
        assert!(signal.score > 0.0);
        assert_eq!(signal.estimated, 3.0);
    }
}
