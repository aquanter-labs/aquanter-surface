use super::{
    Action, Basis, Chart, Drawing, DrawingAnchor, DrawingPart, DrawingTool, Interaction, Message,
    PlotConstants, PlotData, TEXT_SIZE, ViewState, indicator, request_fetch,
    scale::linear::PriceInfoLabel,
};
use crate::chart::indicator::kline::KlineIndicatorImpl;
use crate::connector::fetcher::{FetchRange, RequestHandler, is_trade_fetch_enabled};
use crate::{modal::pane::settings::study, style};
use data::aggr::ticks::TickAggr;
use data::aggr::time::TimeSeries;
use data::chart::indicator::{Indicator, KlineIndicator};
use data::chart::kline::{
    ClusterKind, ClusterScaling, Config, FootprintStudy, FootprintSummary, KlineDataPoint,
    KlineTrades, NPoc, PointOfControl, QuantityFormat,
};
use data::chart::{Autoscale, KlineChartKind, ViewConfig};

use data::config::theme::{composite_color, contrast_ratio, mix_color};
use data::panel::ladder::{GroupedDepth, Side};
use exchange::depth::Depth;
use exchange::unit::{Price, PriceStep, Qty};
use exchange::{Kline, Liquidation, OpenInterest as OIData, TickerInfo, Trade, UnixMs};

use iced::task::Handle;
use iced::theme::palette::Extended;
use iced::widget::canvas::{self, Event, Geometry, Path, Stroke};
use iced::{Alignment, Color, Element, Point, Rectangle, Renderer, Size, Theme, Vector, mouse};

use enum_map::EnumMap;
use std::cell::Cell;
use std::collections::{BTreeMap, HashSet, VecDeque};
use std::time::Instant;

const MAX_CACHED_LIQUIDATIONS: usize = 10_000;
const FOOTPRINT_ORDERBOOK_WIDTH: f32 = 104.0;

fn footprint_orderbook_margin(kind: &KlineChartKind, config: &Config, scaling: f32) -> f32 {
    if matches!(kind, KlineChartKind::Footprint { .. }) && config.show_footprint_orderbook {
        FOOTPRINT_ORDERBOOK_WIDTH / scaling
    } else {
        0.0
    }
}

fn market_latency_ms(received_at_ms: u64, event_at_ms: u64) -> u64 {
    received_at_ms.saturating_sub(event_at_ms)
}

fn smooth_market_latency(current: Option<f64>, sample_ms: u64) -> Option<f64> {
    if sample_ms == 0 {
        return current;
    }
    Some(current.map_or(sample_ms as f64, |value| {
        value * 0.9 + sample_ms as f64 * 0.1
    }))
}

fn displayed_market_latency(value: Option<f64>) -> Option<u64> {
    value.map(|value| ((value / 5.0).round() as u64 * 5).max(5))
}

fn missing_open_interest_range(
    open_interest: &BTreeMap<UnixMs, f64>,
    required_earliest: UnixMs,
    kline_latest: UnixMs,
) -> Option<FetchRange> {
    let earliest = open_interest.keys().next().copied().unwrap_or(kline_latest);
    let latest = open_interest
        .keys()
        .next_back()
        .copied()
        .unwrap_or(UnixMs::ZERO);
    if required_earliest < earliest {
        Some(FetchRange::OpenInterest(required_earliest, earliest))
    } else if latest < kline_latest {
        Some(FetchRange::OpenInterest(latest, kline_latest))
    } else {
        None
    }
}

impl Chart for KlineChart {
    type IndicatorKind = KlineIndicator;

    fn state(&self) -> &ViewState {
        &self.chart
    }

    fn mut_state(&mut self) -> &mut ViewState {
        &mut self.chart
    }

    fn invalidate_crosshair(&mut self) {
        self.chart.cache.clear_crosshair();
        self.indicators
            .values_mut()
            .filter_map(Option::as_mut)
            .for_each(|indi| indi.clear_crosshair_caches());
    }

    fn invalidate_all(&mut self) {
        self.invalidate(None);
    }

    fn view_indicators(&'_ self, enabled: &[Self::IndicatorKind]) -> Vec<Element<'_, Message>> {
        let chart_state = self.state();
        let visible_region = chart_state.visible_region(chart_state.bounds.size());
        let (earliest, latest) = chart_state.interval_range(&visible_region);
        if earliest > latest {
            return vec![];
        }

        let data_labels_always_visible = self.visual_config.data_labels_always_visible;

        let market = chart_state.ticker_info.market_type();
        let mut elements = vec![];

        for selected_indicator in enabled {
            if !self.kind.allows_indicator(*selected_indicator)
                || !KlineIndicator::for_market(market).contains(selected_indicator)
            {
                continue;
            }
            if let Some(indi) = self.indicators[*selected_indicator].as_ref() {
                if indi.is_overlay() {
                    continue;
                }
                elements.push(indi.element(
                    chart_state,
                    data_labels_always_visible,
                    earliest..=latest,
                ));
            }
        }
        elements
    }

    fn visible_timerange(&self) -> Option<(u64, u64)> {
        let chart = self.state();
        let region = chart.visible_region(chart.bounds.size());

        if region.width == 0.0 {
            return None;
        }

        Some(chart.interval_range(&region))
    }

    fn interval_keys(&self) -> Option<Vec<u64>> {
        match &self.data_source {
            PlotData::TimeBased(_) => None,
            PlotData::TickBased(tick_aggr) => Some(
                tick_aggr
                    .datapoints
                    .iter()
                    .map(|dp| dp.kline.time.as_u64())
                    .collect(),
            ),
        }
    }

    fn autoscaled_coords(&self) -> Vector {
        let chart = self.state();
        let x_translation = match &self.kind {
            KlineChartKind::Footprint { .. } => {
                0.5 * (chart.bounds.width / chart.scaling)
                    - (chart.cell_width / chart.scaling)
                    - footprint_orderbook_margin(&self.kind, &self.visual_config, chart.scaling)
            }
            KlineChartKind::Candles => {
                0.5 * (chart.bounds.width / chart.scaling)
                    - (8.0 * chart.cell_width / chart.scaling)
            }
        };
        Vector::new(x_translation, chart.translation.y)
    }

    fn supports_fit_autoscaling(&self) -> bool {
        true
    }

    fn is_empty(&self) -> bool {
        match &self.data_source {
            PlotData::TimeBased(timeseries) => timeseries.datapoints.is_empty(),
            PlotData::TickBased(tick_aggr) => tick_aggr.datapoints.is_empty(),
        }
    }

    fn set_timezone(&self, timezone: data::UserTimezone) {
        if self.timezone.replace(timezone) != timezone {
            self.chart.cache.profiles.clear();
            self.indicators
                .values()
                .filter_map(Option::as_ref)
                .for_each(|indicator| indicator.on_timezone_change(timezone));
        }
    }

    fn interval_selection_mode(&self) -> Option<super::IntervalSelectionMode> {
        self.interval_selection
            .map(|_| super::IntervalSelectionMode::Point)
    }
}

impl PlotConstants for KlineChart {
    fn max_cell_width(&self) -> f32 {
        self.kind.max_cell_width()
    }

    fn min_cell_width(&self) -> f32 {
        self.kind.min_cell_width()
    }

    fn max_cell_height(&self) -> f32 {
        self.kind.max_cell_height()
    }

    fn min_cell_height(&self) -> f32 {
        self.kind.min_cell_height()
    }

    fn default_cell_width(&self) -> f32 {
        self.kind.default_cell_width()
    }
}

pub struct KlineChart {
    chart: ViewState,
    data_source: PlotData<KlineDataPoint>,
    raw_trades: Vec<Trade>,
    open_interest: BTreeMap<UnixMs, f64>,
    liquidations: VecDeque<Liquidation>,
    orderbook: [GroupedDepth; 2],
    latest_market_latency_ms: Option<f64>,
    indicators: EnumMap<KlineIndicator, Option<Box<dyn KlineIndicatorImpl>>>,
    fetching_trades: (bool, Option<Handle>),
    pub(crate) kind: KlineChartKind,
    request_handler: RequestHandler,
    study_configurator: study::Configurator<FootprintStudy>,
    last_tick: Instant,
    visual_config: Config,
    interval_selection: Option<IntervalSelection>,
    timezone: Cell<data::UserTimezone>,
    ai_annotation: Option<crate::ai::ChartAnnotation>,
    trading_overlay: Option<crate::trading::PositionOverlay>,
    dragged_trading_line: Cell<Option<crate::trading::TradingLineId>>,
    drawing_tool: Option<DrawingTool>,
    drawings: Vec<Drawing>,
    selected_drawing: Option<usize>,
    drawing_context_menu: Option<DrawingContextMenu>,
}

#[derive(Debug, Clone, Copy)]
struct DrawingContextMenu {
    index: Option<usize>,
    position: Point,
}

#[derive(Debug, Clone, Copy)]
pub enum IntervalSelection {
    AnchoredVwap,
}

impl KlineChart {
    pub(crate) fn alert_observation(
        &self,
        pane_id: uuid::Uuid,
    ) -> Option<crate::alerts::Observation> {
        let PlotData::TimeBased(timeseries) = &self.data_source else {
            return None;
        };
        let mut datapoints = timeseries.datapoints.values().rev();
        let latest = datapoints.next()?;
        let previous = datapoints.next();
        Some(crate::alerts::Observation {
            pane_id,
            price: latest.kline.close.to_f64(),
            volume: latest.kline.volume.total().to_f64(),
            reference_volume: previous.map(|point| point.kline.volume.total().to_f64()),
            market_time_ms: latest.kline.time.as_u64(),
        })
    }

    pub fn market_snapshot(
        &self,
        short_bar_count: usize,
        long_bar_count: usize,
        exclude_current_bar: bool,
    ) -> Result<crate::ai::MarketSnapshot, String> {
        let PlotData::TimeBased(timeseries) = &self.data_source else {
            return Err("AI analysis currently requires a time-based Kline chart.".to_string());
        };
        let limit = long_bar_count.max(short_bar_count).clamp(20, 500);
        let mut datapoints = timeseries
            .datapoints
            .values()
            .rev()
            .take(limit)
            .collect::<Vec<_>>();
        datapoints.reverse();
        if exclude_current_bar {
            datapoints.pop();
        }
        if datapoints.len() < short_bar_count.min(20) {
            return Err(format!(
                "Not enough loaded history for analysis: {} bars available.",
                datapoints.len()
            ));
        }

        let mut directional_count = 0usize;
        let mut bars = datapoints
            .iter()
            .copied()
            .map(|datapoint| {
                let kline = datapoint.kline;
                let (buy_volume, sell_volume) = kline
                    .volume
                    .buy_sell()
                    .map(|(buy, sell)| (Some(buy.to_f64()), Some(sell.to_f64())))
                    .unwrap_or_else(|| {
                        let buy = datapoint
                            .footprint
                            .trades
                            .values()
                            .map(|group| group.buy_qty.to_f64())
                            .sum::<f64>();
                        let sell = datapoint
                            .footprint
                            .trades
                            .values()
                            .map(|group| group.sell_qty.to_f64())
                            .sum::<f64>();
                        if buy > 0.0 || sell > 0.0 {
                            (Some(buy), Some(sell))
                        } else {
                            (None, None)
                        }
                    });
                if buy_volume.is_some() {
                    directional_count += 1;
                }
                crate::ai::MarketBar {
                    time_ms: kline.time.as_u64(),
                    open: kline.open.to_f64(),
                    high: kline.high.to_f64(),
                    low: kline.low.to_f64(),
                    close: kline.close.to_f64(),
                    volume: kline.volume.total().to_f64(),
                    buy_volume,
                    sell_volume,
                    delta: buy_volume.zip(sell_volume).map(|(buy, sell)| buy - sell),
                    cvd: None,
                    open_interest: self
                        .open_interest
                        .range(..=kline.time)
                        .next_back()
                        .map(|(_, value)| *value),
                }
            })
            .collect::<Vec<_>>();
        crate::ai::populate_relative_cvd(&mut bars);

        let oldest_time = bars.first().map_or(0, |bar| bar.time_ms);
        let recent_liquidations = self
            .liquidations
            .iter()
            .filter(|liquidation| liquidation.time.as_u64() >= oldest_time)
            .collect::<Vec<_>>();
        let liquidations = crate::ai::LiquidationSummary {
            long_quantity: recent_liquidations
                .iter()
                .filter(|liquidation| liquidation.is_sell)
                .map(|liquidation| liquidation.qty.to_f64())
                .sum(),
            short_quantity: recent_liquidations
                .iter()
                .filter(|liquidation| !liquidation.is_sell)
                .map(|liquidation| liquidation.qty.to_f64())
                .sum(),
            event_count: recent_liquidations.len(),
        };

        let depth = self.orderbook[Side::Bid.idx()]
            .orders
            .iter()
            .rev()
            .take(10)
            .map(|(price, quantity)| crate::ai::DepthLevel {
                side: "bid",
                price: price.to_f64(),
                quantity: quantity.to_f64(),
            })
            .chain(self.orderbook[Side::Ask.idx()].orders.iter().take(10).map(
                |(price, quantity)| crate::ai::DepthLevel {
                    side: "ask",
                    price: price.to_f64(),
                    quantity: quantity.to_f64(),
                },
            ))
            .collect::<Vec<_>>();

        let mut missing_data = Vec::new();
        if directional_count == 0 {
            missing_data.push("order_flow");
        }
        if self.open_interest.is_empty() {
            missing_data.push("open_interest");
        }
        if depth.is_empty() {
            missing_data.push("order_book");
        }
        let now_ms = chrono::Utc::now().timestamp_millis().max(0) as u64;
        let timeframe_ms = timeseries.interval.to_milliseconds();
        let latest_market_time_ms = bars.last().map_or(0, |bar| bar.time_ms);
        let expected_close = latest_market_time_ms.saturating_add(timeframe_ms);
        let data_age_ms = now_ms.saturating_sub(expected_close);
        let freshness_quality = if data_age_ms <= timeframe_ms {
            1.0
        } else {
            (timeframe_ms as f32 / data_age_ms as f32).clamp(0.0, 1.0)
        };
        let history_quality = (bars.len() as f32 / limit as f32).min(1.0);
        let directional_quality = directional_count as f32 / bars.len() as f32;
        let data_quality = (history_quality * 0.4
            + directional_quality * 0.2
            + if depth.is_empty() { 0.0 } else { 0.15 }
            + if self.open_interest.is_empty() {
                0.0
            } else {
                0.1
            })
            + freshness_quality * 0.15;
        let data_quality = data_quality.clamp(0.0, 1.0);
        let volume_profile_poc = datapoints
            .iter()
            .rev()
            .take(short_bar_count)
            .flat_map(|datapoint| datapoint.footprint.trades.iter())
            .fold(
                BTreeMap::<Price, f64>::new(),
                |mut profile, (price, group)| {
                    *profile.entry(*price).or_default() += group.total_qty().to_f64();
                    profile
                },
            )
            .into_iter()
            .max_by(|(_, left), (_, right)| left.total_cmp(right))
            .map(|(price, _)| price.to_f64());
        let mut features = crate::ai::MarketFeatures::calculate(&bars, &depth, short_bar_count);
        features.set_volume_profile_poc(volume_profile_poc, bars.last().map(|bar| bar.close));

        Ok(crate::ai::MarketSnapshot {
            generated_at_ms: now_ms as i64,
            symbol: self.chart.ticker_info.ticker.to_string(),
            venue: format!("{:?}", self.chart.ticker_info.exchange()),
            market: format!("{:?}", self.chart.ticker_info.market_type()),
            basis: self.basis().to_string(),
            timeframe_ms,
            latest_market_time_ms,
            data_age_ms,
            bars,
            depth,
            liquidations,
            features,
            data_quality,
            missing_data,
        })
    }

    pub fn new(
        layout: ViewConfig,
        basis: Basis,
        step: PriceStep,
        klines_raw: &[Kline],
        raw_trades: Vec<Trade>,
        enabled_indicators: &[KlineIndicator],
        ticker_info: TickerInfo,
        kind: &KlineChartKind,
        visual_config: Option<Config>,
    ) -> Self {
        let visual_config = visual_config.unwrap_or_default();
        let kind = kind.clone();

        match basis {
            Basis::Time(interval) => {
                let timeseries = TimeSeries::<KlineDataPoint>::new(interval, step, klines_raw)
                    .with_trades(&raw_trades);

                let base_price_y = timeseries.base_price();
                let latest_x = timeseries
                    .latest_timestamp()
                    .map_or(0, |timestamp| timestamp.as_u64());
                let (scale_high, scale_low) = timeseries.price_scale({
                    match &kind {
                        KlineChartKind::Footprint { .. } => 12,
                        KlineChartKind::Candles => 60,
                    }
                });

                let low_rounded = scale_low.round_to_side_step(true, step);
                let high_rounded = scale_high.round_to_side_step(false, step);

                let y_ticks = Price::steps_between_inclusive(low_rounded, high_rounded, step)
                    .map(|n| n.saturating_sub(1))
                    .unwrap_or(1)
                    .max(1) as f32;

                let cell_width = match &kind {
                    KlineChartKind::Footprint { .. } => 80.0,
                    KlineChartKind::Candles => 4.0,
                };
                let cell_height = match &kind {
                    KlineChartKind::Footprint { .. } => 800.0 / y_ticks,
                    KlineChartKind::Candles => 200.0 / y_ticks,
                };

                let mut chart = ViewState::new(
                    basis,
                    step,
                    step.decimal_places(),
                    ticker_info,
                    ViewConfig {
                        splits: layout.splits.clone(),
                        autoscale: Some(Autoscale::FitToVisible),
                    },
                    cell_width,
                    cell_height,
                );
                chart.base_price_y = base_price_y;
                chart.latest_x = latest_x;

                let x_translation = match &kind {
                    KlineChartKind::Footprint { .. } => {
                        0.5 * (chart.bounds.width / chart.scaling)
                            - (chart.cell_width / chart.scaling)
                            - footprint_orderbook_margin(&kind, &visual_config, chart.scaling)
                    }
                    KlineChartKind::Candles => {
                        0.5 * (chart.bounds.width / chart.scaling)
                            - (8.0 * chart.cell_width / chart.scaling)
                    }
                };
                chart.translation.x = x_translation;

                let data_source = PlotData::TimeBased(timeseries);

                let mut indicators = EnumMap::default();
                for &i in enabled_indicators {
                    if !kind.allows_indicator(i) {
                        continue;
                    }
                    let mut indi = indicator::kline::make_empty(
                        i,
                        &visual_config,
                        matches!(&kind, KlineChartKind::Footprint { .. }),
                    );
                    indi.rebuild_from_source(&data_source);
                    indicators[i] = Some(indi);
                }

                KlineChart {
                    chart,
                    visual_config,
                    interval_selection: None,
                    timezone: Cell::new(data::UserTimezone::Utc),
                    ai_annotation: None,
                    trading_overlay: None,
                    dragged_trading_line: Cell::new(None),
                    drawing_tool: None,
                    drawings: Vec::new(),
                    selected_drawing: None,
                    drawing_context_menu: None,
                    data_source,
                    raw_trades,
                    open_interest: BTreeMap::new(),
                    liquidations: VecDeque::new(),
                    orderbook: [GroupedDepth::new(), GroupedDepth::new()],
                    latest_market_latency_ms: None,
                    indicators,
                    fetching_trades: (false, None),
                    request_handler: RequestHandler::default(),
                    kind: kind.clone(),
                    study_configurator: study::Configurator::new(),
                    last_tick: Instant::now(),
                }
            }
            Basis::Tick(interval) => {
                let cell_width = match &kind {
                    KlineChartKind::Footprint { .. } => 80.0,
                    KlineChartKind::Candles => 4.0,
                };
                let cell_height = match &kind {
                    KlineChartKind::Footprint { .. } => 90.0,
                    KlineChartKind::Candles => 8.0,
                };

                let mut chart = ViewState::new(
                    basis,
                    step,
                    step.decimal_places(),
                    ticker_info,
                    ViewConfig {
                        splits: layout.splits.clone(),
                        autoscale: Some(Autoscale::FitToVisible),
                    },
                    cell_width,
                    cell_height,
                );

                let x_translation = match &kind {
                    KlineChartKind::Footprint { .. } => {
                        0.5 * (chart.bounds.width / chart.scaling)
                            - (chart.cell_width / chart.scaling)
                            - footprint_orderbook_margin(&kind, &visual_config, chart.scaling)
                    }
                    KlineChartKind::Candles => {
                        0.5 * (chart.bounds.width / chart.scaling)
                            - (8.0 * chart.cell_width / chart.scaling)
                    }
                };
                chart.translation.x = x_translation;

                let data_source = PlotData::TickBased(TickAggr::new(interval, step, &[]));

                let mut indicators = EnumMap::default();
                for &i in enabled_indicators {
                    if !kind.allows_indicator(i) {
                        continue;
                    }
                    let mut indi = indicator::kline::make_empty(
                        i,
                        &visual_config,
                        matches!(&kind, KlineChartKind::Footprint { .. }),
                    );
                    indi.rebuild_from_source(&data_source);
                    indicators[i] = Some(indi);
                }

                KlineChart {
                    chart,
                    visual_config,
                    interval_selection: None,
                    timezone: Cell::new(data::UserTimezone::Utc),
                    ai_annotation: None,
                    trading_overlay: None,
                    dragged_trading_line: Cell::new(None),
                    drawing_tool: None,
                    drawings: Vec::new(),
                    selected_drawing: None,
                    drawing_context_menu: None,
                    data_source,
                    raw_trades,
                    open_interest: BTreeMap::new(),
                    liquidations: VecDeque::new(),
                    orderbook: [GroupedDepth::new(), GroupedDepth::new()],
                    latest_market_latency_ms: None,
                    indicators,
                    fetching_trades: (false, None),
                    request_handler: RequestHandler::default(),
                    kind: kind.clone(),
                    study_configurator: study::Configurator::new(),
                    last_tick: Instant::now(),
                }
            }
        }
    }

    pub fn update_latest_kline(&mut self, kline: &Kline) {
        match self.data_source {
            PlotData::TimeBased(ref mut timeseries) => {
                timeseries.insert_klines(&[*kline]);

                self.indicators
                    .values_mut()
                    .filter_map(Option::as_mut)
                    .for_each(|indi| indi.on_insert_klines(&[*kline], &self.data_source));

                let chart = self.mut_state();

                if kline.time.as_u64() > chart.latest_x {
                    chart.latest_x = kline.time.as_u64();
                }

                chart.last_price = Some(PriceInfoLabel::new(kline.close, kline.open));
            }
            PlotData::TickBased(_) => {}
        }
    }

    pub fn insert_depth(&mut self, depth: &Depth, update_t: UnixMs) {
        if !matches!(self.kind, KlineChartKind::Footprint { .. }) {
            return;
        }
        self.latest_market_latency_ms = smooth_market_latency(
            self.latest_market_latency_ms,
            market_latency_ms(
                chrono::Utc::now().timestamp_millis().max(0) as u64,
                update_t.as_u64(),
            ),
        );
        let step = self.chart.tick_size;
        self.orderbook[Side::Bid.idx()].regroup_from_raw(&depth.bids, Side::Bid, step);
        self.orderbook[Side::Ask.idx()].regroup_from_raw(&depth.asks, Side::Ask, step);
    }

    pub fn kind(&self) -> &KlineChartKind {
        &self.kind
    }

    pub fn ticker_info(&self) -> TickerInfo {
        self.chart.ticker_info
    }

    pub(crate) fn trading_market_info(&self) -> (Option<u64>, Option<f64>, Option<f64>) {
        let bid = self.orderbook[Side::Bid.idx()]
            .orders
            .keys()
            .next_back()
            .map(|price| price.to_f64());
        let ask = self.orderbook[Side::Ask.idx()]
            .orders
            .keys()
            .next()
            .map(|price| price.to_f64());
        (displayed_market_latency(self.latest_market_latency_ms), bid, ask)
    }

    pub(crate) fn set_ai_annotation(&mut self, annotation: crate::ai::ChartAnnotation) {
        self.ai_annotation = Some(annotation);
    }

    pub(crate) fn clear_ai_annotation(&mut self) {
        self.ai_annotation = None;
    }

    pub(crate) fn set_trading_overlay(&mut self, overlay: Option<crate::trading::PositionOverlay>) {
        self.trading_overlay = overlay;
    }

    pub fn drawing_tool(&self) -> Option<DrawingTool> {
        self.drawing_tool
    }

    pub fn select_drawing_tool(&mut self, tool: Option<DrawingTool>) {
        self.drawing_tool = tool;
        self.selected_drawing = None;
        self.drawing_context_menu = None;
    }

    pub fn clear_drawings(&mut self) {
        self.drawings.clear();
        self.selected_drawing = None;
        self.ai_annotation = None;
        self.drawing_context_menu = None;
    }

    pub fn handle_drawing_message(&mut self, message: &Message) -> bool {
        match *message {
            Message::DrawingCreated(drawing) => {
                self.drawings.push(drawing);
                self.selected_drawing = Some(self.drawings.len() - 1);
            }
            Message::DrawingSelected(index) => {
                self.selected_drawing = (index < self.drawings.len()).then_some(index);
            }
            Message::DrawingChanged(index, drawing) => {
                if let Some(current) = self.drawings.get_mut(index) {
                    *current = drawing;
                }
            }
            Message::DrawingFinished => {
                self.drawing_tool = None;
            }
            Message::DrawingDeleted(index) => {
                if index < self.drawings.len() {
                    self.drawings.remove(index);
                }
                self.selected_drawing = None;
                self.drawing_context_menu = None;
            }
            Message::DrawingsCleared => self.clear_drawings(),
            Message::DrawingContextMenu(index, position) => {
                if index.is_none_or(|index| index < self.drawings.len()) {
                    self.selected_drawing = index;
                    self.drawing_context_menu = Some(DrawingContextMenu { index, position });
                }
            }
            Message::DrawingContextMenuClosed => {
                self.drawing_context_menu = None;
            }
            _ => return false,
        }
        true
    }

    fn fetch_missing_data(&mut self) -> Option<Action> {
        match &self.data_source {
            PlotData::TimeBased(timeseries) => {
                let timeframe_ms = timeseries.interval.to_milliseconds();

                if timeseries.datapoints.is_empty() {
                    let latest = chrono::Utc::now().timestamp_millis() as u64;
                    let earliest = latest.saturating_sub(450 * timeframe_ms);

                    let range = FetchRange::Kline(UnixMs::new(earliest), UnixMs::new(latest));
                    if let Some(action) = request_fetch(&mut self.request_handler, range) {
                        return Some(action);
                    }
                }

                let (visible_earliest, visible_latest) = self.visible_timerange()?;
                let (kline_earliest, kline_latest) = timeseries.timerange();
                let visible_earliest_ms = UnixMs::new(visible_earliest);
                let visible_latest_ms = UnixMs::new(visible_latest);
                let visible_span = visible_latest.saturating_sub(visible_earliest);
                let prefetch_earliest = visible_earliest.saturating_sub(visible_span);

                // priority 1, initial klines for visible range
                if visible_earliest_ms < kline_earliest {
                    let range = FetchRange::Kline(UnixMs::new(prefetch_earliest), kline_earliest);

                    if let Some(action) = request_fetch(&mut self.request_handler, range) {
                        return Some(action);
                    }
                }

                // priority 2, trades
                if let KlineChartKind::Footprint { .. } = self.kind
                    && !self.fetching_trades.0
                    && is_trade_fetch_enabled()
                    && let Some((fetch_from, fetch_to)) =
                        timeseries.suggest_trade_fetch_range(visible_earliest_ms, visible_latest_ms)
                {
                    let range = FetchRange::Trades(fetch_from, fetch_to);
                    if let Some(action) = request_fetch(&mut self.request_handler, range) {
                        self.fetching_trades = (true, None);
                        return Some(action);
                    }
                }

                // priority 3, open interest used by AI and indicators
                let exchange = self.chart.ticker_info.exchange();
                if let Some(source_timeframe) =
                    exchange.open_interest_source_timeframe(timeseries.interval)
                {
                    let required_earliest = if matches!(
                        exchange,
                        exchange::adapter::Exchange::BinanceLinear
                            | exchange::adapter::Exchange::BinanceInverse
                    ) {
                        const BINANCE_OI_HISTORY_MS: u64 = 30 * 24 * 60 * 60 * 1_000;
                        let cutoff = (chrono::Utc::now().timestamp_millis().max(0) as u64)
                            .saturating_sub(BINANCE_OI_HISTORY_MS);
                        let interval = source_timeframe.to_milliseconds();
                        kline_earliest.max(UnixMs::new(cutoff.div_ceil(interval) * interval))
                    } else {
                        kline_earliest
                    };

                    if let Some(range) = missing_open_interest_range(
                        &self.open_interest,
                        required_earliest,
                        kline_latest,
                    ) && let Some(action) = request_fetch(&mut self.request_handler, range)
                    {
                        return Some(action);
                    }
                }

                // priority 4, indicators
                let ctx = indicator::kline::FetchCtx {
                    main_chart: &self.chart,
                    timeframe: timeseries.interval,
                    visible_earliest: visible_earliest_ms,
                    kline_latest,
                    prefetch_earliest: UnixMs::new(prefetch_earliest),
                };
                for indi in self.indicators.values_mut().filter_map(Option::as_mut) {
                    if let Some(range) = indi.fetch_range(&ctx)
                        && let Some(action) = request_fetch(&mut self.request_handler, range)
                    {
                        return Some(action);
                    }
                }

                // priority 5, missing klines & integrity check
                let check_earliest = UnixMs::new(prefetch_earliest).max(kline_earliest);
                let check_latest = visible_latest_ms.saturating_add(timeframe_ms);

                if let Some(missing_keys) =
                    timeseries.check_kline_integrity(check_earliest, check_latest)
                {
                    let latest = missing_keys
                        .iter()
                        .max()
                        .unwrap_or(&visible_latest_ms)
                        .saturating_add(timeframe_ms);
                    let earliest = missing_keys
                        .iter()
                        .min()
                        .unwrap_or(&visible_earliest_ms)
                        .saturating_sub(timeframe_ms);

                    let range = FetchRange::Kline(earliest, latest);
                    if let Some(action) = request_fetch(&mut self.request_handler, range) {
                        return Some(action);
                    }
                }
            }
            PlotData::TickBased(_) => {
                // TODO: implement trade fetch
            }
        }

        None
    }

    pub fn reset_request_handler(&mut self) {
        self.request_handler = RequestHandler::default();
        self.fetching_trades = (false, None);
    }

    pub fn raw_trades(&self) -> Vec<Trade> {
        self.raw_trades.clone()
    }

    pub fn set_handle(&mut self, handle: Handle) {
        self.fetching_trades.1 = Some(handle);
    }

    pub fn tick_size(&self) -> PriceStep {
        self.chart.tick_size
    }

    pub fn study_configurator(&self) -> &study::Configurator<FootprintStudy> {
        &self.study_configurator
    }

    pub fn update_study_configurator(&mut self, message: study::Message<FootprintStudy>) {
        let KlineChartKind::Footprint {
            ref mut studies, ..
        } = self.kind
        else {
            return;
        };

        match self.study_configurator.update(message) {
            Some(study::Action::ToggleStudy(study, is_selected)) => {
                if is_selected {
                    let already_exists = studies.iter().any(|s| s.is_same_type(&study));
                    if !already_exists {
                        studies.push(study);
                    }
                } else {
                    studies.retain(|s| !s.is_same_type(&study));
                }
            }
            Some(study::Action::ConfigureStudy(study)) => {
                if let Some(existing_study) = studies.iter_mut().find(|s| s.is_same_type(&study)) {
                    *existing_study = study;
                }
            }
            None => {}
        }

        self.invalidate(None);
    }

    pub fn chart_layout(&self) -> ViewConfig {
        self.chart.layout()
    }

    pub fn visual_config(&self) -> Config {
        self.visual_config.clone()
    }

    pub fn set_visual_config(&mut self, visual_config: Config) {
        if self.visual_config.show_footprint_orderbook != visual_config.show_footprint_orderbook
            && matches!(self.kind, KlineChartKind::Footprint { .. })
        {
            let margin = FOOTPRINT_ORDERBOOK_WIDTH / self.chart.scaling;
            self.chart.translation.x += if visual_config.show_footprint_orderbook {
                -margin
            } else {
                margin
            };
        }
        self.visual_config = visual_config.clone();
        self.chart.cache.clear_all();
        self.indicators
            .values_mut()
            .filter_map(Option::as_mut)
            .for_each(|indi| {
                indi.on_visual_config_change(&visual_config, &self.data_source);
                indi.clear_all_caches();
            });
    }

    pub fn begin_interval_selection(&mut self, selection: IntervalSelection) {
        self.interval_selection = Some(selection);
    }

    pub fn apply_interval_selection(&mut self, interval: u64) {
        let Some(selection) = self.interval_selection else {
            return;
        };
        let time = match self.chart.basis {
            Basis::Time(timeframe) => UnixMs::new(interval).floor_to(timeframe),
            Basis::Tick(_) => return,
        };
        let mut config = self.visual_config.clone();
        match selection {
            IntervalSelection::AnchoredVwap => config.anchored_vwap.anchor = Some(time),
        }
        self.interval_selection = None;
        self.set_visual_config(config);
    }

    pub fn set_cluster_kind(&mut self, new_kind: ClusterKind) {
        if let KlineChartKind::Footprint {
            ref mut clusters, ..
        } = self.kind
        {
            *clusters = new_kind;
        }

        self.invalidate(None);
    }

    pub fn set_cluster_scaling(&mut self, new_scaling: ClusterScaling) {
        if let KlineChartKind::Footprint {
            ref mut scaling, ..
        } = self.kind
        {
            *scaling = new_scaling;
        }

        self.invalidate(None);
    }

    pub fn basis(&self) -> Basis {
        self.chart.basis
    }

    pub fn change_tick_size(&mut self, new_step: PriceStep) {
        self.orderbook = [GroupedDepth::new(), GroupedDepth::new()];
        let chart = self.mut_state();

        chart.cell_height *= (new_step.units as f32) / (chart.tick_size.units as f32);
        chart.tick_size = new_step;

        match self.data_source {
            PlotData::TickBased(ref mut tick_aggr) => {
                tick_aggr.change_tick_size(new_step, &self.raw_trades);
            }
            PlotData::TimeBased(ref mut timeseries) => {
                timeseries.change_tick_size(new_step, &self.raw_trades);
            }
        }

        self.indicators
            .values_mut()
            .filter_map(Option::as_mut)
            .for_each(|indi| indi.on_ticksize_change(&self.data_source));

        self.invalidate(None);
    }

    pub fn set_basis(&mut self, new_basis: Basis) -> Option<Action> {
        let previous_basis = self.chart.basis;

        self.chart.last_price = None;
        self.chart.basis = new_basis;

        match new_basis {
            Basis::Time(interval) => {
                if matches!(previous_basis, Basis::Tick(_)) {
                    self.raw_trades.clear();
                };

                let step = self.chart.tick_size;
                let timeseries = TimeSeries::<KlineDataPoint>::new(interval, step, &[]);
                self.data_source = PlotData::TimeBased(timeseries);
            }
            Basis::Tick(tick_count) => {
                let trades = if matches!(previous_basis, Basis::Tick(_)) {
                    &self.raw_trades
                } else {
                    self.raw_trades.clear();
                    &vec![]
                };

                let step = self.chart.tick_size;
                let tick_aggr = TickAggr::new(tick_count, step, trades);
                self.data_source = PlotData::TickBased(tick_aggr);
            }
        }

        self.indicators
            .values_mut()
            .filter_map(Option::as_mut)
            .for_each(|indi| indi.on_basis_change(&self.data_source));

        self.reset_request_handler();
        self.invalidate(Some(Instant::now()))
    }

    pub fn studies(&self) -> Option<Vec<FootprintStudy>> {
        match &self.kind {
            KlineChartKind::Footprint { studies, .. } => Some(studies.clone()),
            _ => None,
        }
    }

    pub fn set_studies(&mut self, new_studies: Vec<FootprintStudy>) {
        if let KlineChartKind::Footprint {
            ref mut studies, ..
        } = self.kind
        {
            *studies = new_studies;
        }

        self.invalidate(None);
    }

    pub fn insert_trades(&mut self, buffer: &[Trade], update_t: UnixMs) {
        self.latest_market_latency_ms = smooth_market_latency(
            self.latest_market_latency_ms,
            market_latency_ms(
                chrono::Utc::now().timestamp_millis().max(0) as u64,
                update_t.as_u64(),
            ),
        );
        self.raw_trades.extend_from_slice(buffer);

        match self.data_source {
            PlotData::TickBased(ref mut tick_aggr) => {
                let old_dp_len = tick_aggr.datapoints.len();
                tick_aggr.insert_trades(buffer);

                if let Some(last_dp) = tick_aggr.datapoints.last() {
                    self.chart.last_price =
                        Some(PriceInfoLabel::new(last_dp.kline.close, last_dp.kline.open));
                } else {
                    self.chart.last_price = None;
                }

                self.indicators
                    .values_mut()
                    .filter_map(Option::as_mut)
                    .for_each(|indi| indi.on_insert_trades(buffer, old_dp_len, &self.data_source));

                self.invalidate(None);
            }
            PlotData::TimeBased(ref mut timeseries) => {
                timeseries.insert_trades_existing_buckets(buffer);

                self.indicators
                    .values_mut()
                    .filter_map(Option::as_mut)
                    .for_each(|indi| indi.on_insert_trades(buffer, 0, &self.data_source));

                self.invalidate(None);
            }
        }
    }

    pub fn insert_liquidations(&mut self, liquidations: &[Liquidation]) {
        self.liquidations.extend(liquidations.iter().copied());
        while self.liquidations.len() > MAX_CACHED_LIQUIDATIONS {
            self.liquidations.pop_front();
        }
        self.indicators
            .values_mut()
            .filter_map(Option::as_mut)
            .for_each(|indicator| indicator.on_liquidations(liquidations));
        self.invalidate(None);
    }

    pub fn insert_raw_trades(
        &mut self,
        raw_trades: Vec<Trade>,
        is_batches_done: bool,
        req_id: uuid::Uuid,
    ) {
        if matches!(&self.data_source, PlotData::TickBased(_)) {
            if is_batches_done {
                self.fetching_trades = (false, None);
                self.request_handler.mark_completed(req_id);
            }
            return;
        }

        if let PlotData::TimeBased(ref mut timeseries) = self.data_source {
            timeseries.insert_trades_existing_buckets(&raw_trades);
        }

        self.raw_trades.extend_from_slice(&raw_trades);

        self.indicators
            .values_mut()
            .filter_map(Option::as_mut)
            .for_each(|indi| indi.on_insert_trades(&raw_trades, 0, &self.data_source));

        if is_batches_done {
            self.fetching_trades = (false, None);
            self.request_handler.mark_completed(req_id);
        }

        self.invalidate(None);
    }

    pub fn fail_request(&mut self, req_id: Option<uuid::Uuid>, error: String) {
        if let Some(req_id) = req_id {
            if self.request_handler.mark_failed(req_id, error) {
                self.fetching_trades = (false, None);
            }
        }
    }

    pub fn insert_shared_klines(&mut self, klines_raw: &[Kline]) {
        if let PlotData::TimeBased(ref mut timeseries) = self.data_source {
            timeseries.insert_klines(klines_raw);
            timeseries.insert_trades_existing_buckets(&self.raw_trades);
            let latest = timeseries.latest_timestamp();

            self.indicators
                .values_mut()
                .filter_map(Option::as_mut)
                .for_each(|indi| indi.on_insert_klines(klines_raw, &self.data_source));

            if let Some(latest) = latest {
                self.chart.latest_x = self.chart.latest_x.max(latest.as_u64());
            }
            self.invalidate(None);
        }
    }

    pub fn insert_hist_klines(&mut self, req_id: uuid::Uuid, klines_raw: &[Kline]) {
        self.insert_shared_klines(klines_raw);

        if klines_raw.is_empty() {
            self.request_handler
                .mark_failed(req_id, "No data received".to_string());
        } else {
            self.request_handler.mark_completed(req_id);
        }
    }

    pub fn insert_open_interest(&mut self, req_id: Option<uuid::Uuid>, oi_data: &[OIData]) {
        if let Some(req_id) = req_id {
            if oi_data.is_empty() {
                self.request_handler
                    .mark_failed(req_id, "No data received".to_string());
            } else {
                self.request_handler.mark_completed(req_id);
            }
        }

        self.open_interest
            .extend(oi_data.iter().map(|item| (item.time, item.value)));

        self.indicators
            .values_mut()
            .filter_map(Option::as_mut)
            .for_each(|indicator| indicator.on_open_interest(oi_data));
        self.invalidate(None);
    }

    fn calc_qty_scales(
        &self,
        earliest: u64,
        latest: u64,
        highest: Price,
        lowest: Price,
        step: PriceStep,
        cluster_kind: ClusterKind,
    ) -> f64 {
        let rounded_highest = highest.round_to_side_step(false, step).add_steps(1, step);
        let rounded_lowest = lowest.round_to_side_step(true, step).add_steps(-1, step);

        match &self.data_source {
            PlotData::TimeBased(timeseries) => timeseries
                .max_qty_ts_range(
                    cluster_kind,
                    UnixMs::new(earliest),
                    UnixMs::new(latest),
                    rounded_highest,
                    rounded_lowest,
                )
                .to_f64(),
            PlotData::TickBased(tick_aggr) => {
                let earliest = earliest as usize;
                let latest = latest as usize;

                tick_aggr
                    .max_qty_idx_range(
                        cluster_kind,
                        earliest,
                        latest,
                        rounded_highest,
                        rounded_lowest,
                    )
                    .to_f64()
            }
        }
    }

    pub fn last_update(&self) -> Instant {
        self.last_tick
    }

    pub fn invalidate(&mut self, now: Option<Instant>) -> Option<Action> {
        let chart = &mut self.chart;

        if let Some(autoscale) = chart.layout.autoscale {
            match autoscale {
                super::Autoscale::CenterLatest => {
                    let x_translation = match &self.kind {
                        KlineChartKind::Footprint { .. } => {
                            0.5 * (chart.bounds.width / chart.scaling)
                                - (chart.cell_width / chart.scaling)
                                - footprint_orderbook_margin(
                                    &self.kind,
                                    &self.visual_config,
                                    chart.scaling,
                                )
                        }
                        KlineChartKind::Candles => {
                            0.5 * (chart.bounds.width / chart.scaling)
                                - (8.0 * chart.cell_width / chart.scaling)
                        }
                    };
                    chart.translation.x = x_translation;

                    let calculate_target_y = |kline: exchange::Kline| -> f32 {
                        let y_low = chart.price_to_y(kline.low);
                        let y_high = chart.price_to_y(kline.high);
                        let y_close = chart.price_to_y(kline.close);

                        let mut target_y_translation = -(y_low + y_high) / 2.0;

                        if chart.bounds.height > f32::EPSILON && chart.scaling > f32::EPSILON {
                            let visible_half_height = (chart.bounds.height / chart.scaling) / 2.0;

                            let view_center_y_centered = -target_y_translation;

                            let visible_y_top = view_center_y_centered - visible_half_height;
                            let visible_y_bottom = view_center_y_centered + visible_half_height;

                            let padding = chart.cell_height;

                            if y_close < visible_y_top {
                                target_y_translation = -(y_close - padding + visible_half_height);
                            } else if y_close > visible_y_bottom {
                                target_y_translation = -(y_close + padding - visible_half_height);
                            }
                        }
                        target_y_translation
                    };

                    chart.translation.y = self.data_source.latest_y_midpoint(calculate_target_y);
                }
                super::Autoscale::FitToVisible => {
                    let visible_region = chart.visible_region(chart.bounds.size());
                    let (start_interval, end_interval) = chart.interval_range(&visible_region);

                    if let Some((lowest, highest)) = self
                        .data_source
                        .visible_price_range(start_interval, end_interval)
                    {
                        let chart_height = chart.bounds.height;
                        let tick_size = chart.tick_size.to_f32_lossy();

                        if chart_height > f32::EPSILON && tick_size > 0.0 {
                            let (fit_lowest, fit_highest) =
                                if let KlineChartKind::Footprint { .. } = self.kind {
                                    if let Some((footprint_low, footprint_high)) = self
                                        .data_source
                                        .visible_footprint_price_range(start_interval, end_interval)
                                    {
                                        let half_tick = tick_size * 0.5;
                                        (
                                            footprint_low.to_f32_lossy() - half_tick,
                                            footprint_high.to_f32_lossy() + half_tick,
                                        )
                                    } else {
                                        (lowest, highest)
                                    }
                                } else {
                                    (lowest, highest)
                                };

                            let visible_span = (fit_highest - fit_lowest).max(tick_size);
                            let base_padding = visible_span * 0.05; // 5% padding on top and bottom

                            let mut top_padding = base_padding;
                            let mut bottom_padding = base_padding;

                            if let KlineChartKind::Footprint { .. } = self.kind {
                                let provisional_span = visible_span + top_padding + bottom_padding;
                                if provisional_span > 0.0 {
                                    let provisional_cell_height =
                                        (chart_height * tick_size) / provisional_span;

                                    let outer_padding = price_padding_from_pixels(
                                        provisional_cell_height,
                                        tick_size,
                                    );

                                    top_padding += outer_padding;
                                    bottom_padding += outer_padding;

                                    if self.visual_config.show_footprint_summary {
                                        bottom_padding =
                                            bottom_padding.max(FootprintSummaryLayout::padding(
                                                provisional_cell_height,
                                                chart.scaling,
                                                tick_size,
                                            ));
                                    }
                                }
                            }

                            let padded_span = visible_span + top_padding + bottom_padding;
                            if padded_span > 0.0 {
                                chart.cell_height = (chart_height * tick_size) / padded_span;
                                chart.base_price_y = Price::from_f32(fit_highest + top_padding);
                                chart.translation.y = -chart_height / 2.0;
                            }
                        }
                    }
                }
            }
        }

        chart.cache.clear_all();
        for indi in self.indicators.values_mut().filter_map(Option::as_mut) {
            indi.clear_all_caches();
        }

        if let Some(t) = now {
            self.last_tick = t;
            self.fetch_missing_data()
        } else {
            None
        }
    }

    pub fn toggle_indicator(&mut self, indicator: KlineIndicator) {
        if !self.kind.allows_indicator(indicator) {
            return;
        }

        let panel_count =
            |indicators: &EnumMap<KlineIndicator, Option<Box<dyn KlineIndicatorImpl>>>| {
                indicators
                    .values()
                    .filter_map(Option::as_ref)
                    .filter(|indicator| !indicator.is_overlay())
                    .count()
            };
        let prev_indi_count = panel_count(&self.indicators);

        if self.indicators[indicator].is_some() {
            self.indicators[indicator] = None;
        } else {
            let mut box_indi = indicator::kline::make_empty(
                indicator,
                &self.visual_config,
                matches!(&self.kind, KlineChartKind::Footprint { .. }),
            );
            box_indi.rebuild_from_source(&self.data_source);
            let cached_oi = self
                .open_interest
                .iter()
                .map(|(&time, &value)| OIData { time, value })
                .collect::<Vec<_>>();
            box_indi.on_open_interest(&cached_oi);
            let cached_liquidations = self.liquidations.iter().copied().collect::<Vec<_>>();
            box_indi.on_liquidations(&cached_liquidations);
            self.indicators[indicator] = Some(box_indi);
        }

        if let Some(main_split) = self.chart.layout.splits.first() {
            let current_indi_count = panel_count(&self.indicators);
            self.chart.layout.splits = data::util::calc_panel_splits(
                *main_split,
                current_indi_count,
                Some(prev_indi_count),
            );
        }
    }
}

impl KlineChart {
    fn drawing_anchor(&self, position: Point, bounds: Rectangle) -> DrawingAnchor {
        let center = canvas_local_center(bounds);
        let world = (position - center) * (1.0 / self.chart.scaling) - self.chart.translation;
        let interval = match self.chart.basis {
            Basis::Time(_) => self.chart.x_to_interval(world.x),
            Basis::Tick(_) => {
                if world.x >= 0.0 {
                    0
                } else {
                    self.chart.x_to_interval(world.x)
                }
            }
        };
        DrawingAnchor {
            interval,
            price: self.chart.y_to_price(world.y).to_f64(),
        }
    }

    fn drawing_screen_point(&self, anchor: DrawingAnchor, bounds: Rectangle) -> Point {
        let world = Point::new(
            self.chart.interval_to_x(anchor.interval),
            self.chart.price_to_y(Price::from_f64(anchor.price)),
        );
        Point::new(
            canvas_local_center(bounds).x
                + (world.x + self.chart.translation.x) * self.chart.scaling,
            canvas_local_center(bounds).y
                + (world.y + self.chart.translation.y) * self.chart.scaling,
        )
    }

    fn drawing_hit(&self, position: Point, bounds: Rectangle) -> Option<(usize, DrawingPart)> {
        const HANDLE_RADIUS: f32 = 7.0;
        self.drawings
            .iter()
            .enumerate()
            .rev()
            .find_map(|(index, drawing)| {
                let start = self.drawing_screen_point(drawing.start, bounds);
                let end = self.drawing_screen_point(drawing.end, bounds);
                if drawing.tool == DrawingTool::VolumeProfile {
                    let (top, bottom) = self
                        .data_source
                        .visible_price_range(
                            drawing.start.interval.min(drawing.end.interval),
                            drawing.start.interval.max(drawing.end.interval),
                        )
                        .map_or((0.0, bounds.height), |(first, second)| {
                            let first = self
                                .drawing_screen_point(
                                    DrawingAnchor {
                                        interval: drawing.start.interval,
                                        price: f64::from(first),
                                    },
                                    bounds,
                                )
                                .y;
                            let second = self
                                .drawing_screen_point(
                                    DrawingAnchor {
                                        interval: drawing.start.interval,
                                        price: f64::from(second),
                                    },
                                    bounds,
                                )
                                .y;
                            (first.min(second), first.max(second))
                        });
                    if (top - HANDLE_RADIUS..=bottom + HANDLE_RADIUS).contains(&position.y)
                        && (position.x - start.x).abs() <= HANDLE_RADIUS
                    {
                        return Some((index, DrawingPart::Start));
                    }
                    if (top - HANDLE_RADIUS..=bottom + HANDLE_RADIUS).contains(&position.y)
                        && (position.x - end.x).abs() <= HANDLE_RADIUS
                    {
                        return Some((index, DrawingPart::End));
                    }
                    let left = start.x.min(end.x);
                    let right = start.x.max(end.x);
                    return Rectangle {
                        x: left,
                        y: top,
                        width: right - left,
                        height: bottom - top,
                    }
                    .contains(position)
                    .then_some((index, DrawingPart::Body));
                }
                if position.distance(start) <= HANDLE_RADIUS {
                    return Some((index, DrawingPart::Start));
                }
                if drawing.tool != DrawingTool::HorizontalLine
                    && position.distance(end) <= HANDLE_RADIUS
                {
                    return Some((index, DrawingPart::End));
                }
                let hit_body = match drawing.tool {
                    DrawingTool::HorizontalLine => (position.y - start.y).abs() <= HANDLE_RADIUS,
                    DrawingTool::TrendLine => {
                        distance_to_segment(position, start, end) <= HANDLE_RADIUS
                    }
                    DrawingTool::VolumeProfile => unreachable!(),
                };
                hit_body.then_some((index, DrawingPart::Body))
            })
    }

    fn trading_line_hit(
        &self,
        position: Point,
        bounds: Rectangle,
    ) -> Option<crate::trading::TradingLineId> {
        let overlay = self.trading_overlay.as_ref()?;
        let screen_y = |price: f64| {
            (self.chart.price_to_y(Price::from_f64(price)) + self.chart.translation.y)
                * self.chart.scaling
                + bounds.height / 2.0
        };
        let mut levels = Vec::new();
        if let Some(price) = overlay.stop_loss {
            levels.push((crate::trading::TradingLineId::StopLoss, price));
        }
        if let Some(price) = overlay.take_profit {
            levels.push((crate::trading::TradingLineId::TakeProfit, price));
        }
        levels.extend(overlay.orders.iter().map(|order| (order.id, order.price)));
        levels
            .into_iter()
            .filter_map(|(id, price)| {
                let distance = (position.y - screen_y(price)).abs();
                (distance <= 6.0).then_some((id, distance))
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    }

    fn trading_line_interaction(
        &self,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        let Some(position) = cursor.position_in(bounds) else {
            if matches!(
                event,
                Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
            ) && self.dragged_trading_line.take().is_some()
            {
                return Some(canvas::Action::capture());
            }
            return None;
        };
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let id = self.trading_line_hit(position, bounds)?;
                self.dragged_trading_line.set(Some(id));
                Some(canvas::Action::capture())
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let id = self.dragged_trading_line.get()?;
                let price = self.drawing_anchor(position, bounds).price;
                Some(
                    canvas::Action::publish(Message::TradingLevelDragged(id, price, false))
                        .and_capture(),
                )
            }
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left)) => {
                let id = self.dragged_trading_line.take()?;
                let price = self.drawing_anchor(position, bounds).price;
                Some(
                    canvas::Action::publish(Message::TradingLevelDragged(id, price, true))
                        .and_capture(),
                )
            }
            _ => None,
        }
    }

    fn drawing_interaction(
        &self,
        interaction: &mut Interaction,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        if matches!(
            event,
            Event::Mouse(mouse::Event::ButtonReleased(mouse::Button::Left))
        ) && matches!(interaction, Interaction::Drawing { .. })
        {
            *interaction = Interaction::None;
            return Some(canvas::Action::publish(Message::DrawingFinished).and_capture());
        }
        let position = cursor.position_in(bounds)?;
        if let Some(menu) = self.drawing_context_menu {
            match event {
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                    let message = if menu.index.is_some()
                        && drawing_context_menu_item_hit(
                            menu,
                            bounds,
                            position,
                            DrawingMenuItem::Delete,
                        ) {
                        Message::DrawingDeleted(menu.index.expect("selected drawing"))
                    } else if drawing_context_menu_item_hit(
                        menu,
                        bounds,
                        position,
                        DrawingMenuItem::ClearAll,
                    ) {
                        Message::DrawingsCleared
                    } else {
                        Message::DrawingContextMenuClosed
                    };
                    return Some(canvas::Action::publish(message).and_capture());
                }
                Event::Keyboard(iced::keyboard::Event::KeyPressed {
                    key: iced::keyboard::Key::Named(iced::keyboard::key::Named::Escape),
                    ..
                }) => {
                    return Some(
                        canvas::Action::publish(Message::DrawingContextMenuClosed).and_capture(),
                    );
                }
                _ => {}
            }
        }
        match event {
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right)) => {
                let index = self.drawing_hit(position, bounds).map(|(index, _)| index);
                Some(
                    canvas::Action::publish(Message::DrawingContextMenu(index, position))
                        .and_capture(),
                )
            }
            Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) => {
                let anchor = self.drawing_anchor(position, bounds);
                if let Some(tool) = self.drawing_tool {
                    let drawing = Drawing {
                        tool,
                        start: anchor,
                        end: anchor,
                    };
                    *interaction = Interaction::Drawing {
                        index: self.drawings.len(),
                        part: if tool == DrawingTool::HorizontalLine {
                            DrawingPart::Body
                        } else {
                            DrawingPart::End
                        },
                        cursor_start: anchor,
                        original: drawing,
                    };
                    return Some(
                        canvas::Action::publish(Message::DrawingCreated(drawing)).and_capture(),
                    );
                }
                self.drawing_hit(position, bounds).map(|(index, part)| {
                    *interaction = Interaction::Drawing {
                        index,
                        part,
                        cursor_start: anchor,
                        original: self.drawings[index],
                    };
                    canvas::Action::publish(Message::DrawingSelected(index)).and_capture()
                })
            }
            Event::Mouse(mouse::Event::CursorMoved { .. }) => {
                let Interaction::Drawing {
                    index,
                    part,
                    cursor_start,
                    original,
                } = *interaction
                else {
                    return None;
                };
                let current = self.drawing_anchor(position, bounds);
                let drawing = moved_drawing(original, part, cursor_start, current);
                Some(canvas::Action::publish(Message::DrawingChanged(index, drawing)).and_capture())
            }
            _ => None,
        }
    }
}

fn moved_drawing(
    original: Drawing,
    part: DrawingPart,
    cursor_start: DrawingAnchor,
    current: DrawingAnchor,
) -> Drawing {
    let mut drawing = original;
    match part {
        DrawingPart::Start => drawing.start = current,
        DrawingPart::End => drawing.end = current,
        DrawingPart::Body => {
            let interval_delta = current.interval as i128 - cursor_start.interval as i128;
            let shift_interval =
                |value: u64| (value as i128 + interval_delta).clamp(0, u64::MAX as i128) as u64;
            let price_delta = current.price - cursor_start.price;
            drawing.start.interval = shift_interval(original.start.interval);
            drawing.end.interval = shift_interval(original.end.interval);
            drawing.start.price = original.start.price + price_delta;
            drawing.end.price = original.end.price + price_delta;
        }
    }
    drawing
}

fn drawing_context_menu_bounds(menu: DrawingContextMenu, bounds: Rectangle) -> Rectangle {
    const WIDTH: f32 = 184.0;
    let height = if menu.index.is_some() { 68.0 } else { 34.0 };
    Rectangle {
        x: menu.position.x.min((bounds.width - WIDTH).max(0.0)),
        y: menu.position.y.min((bounds.height - height).max(0.0)),
        width: WIDTH,
        height,
    }
}

#[derive(Debug, Clone, Copy)]
enum DrawingMenuItem {
    Delete,
    ClearAll,
}

fn drawing_context_menu_item_hit(
    menu: DrawingContextMenu,
    bounds: Rectangle,
    click: Point,
    item: DrawingMenuItem,
) -> bool {
    let menu_bounds = drawing_context_menu_bounds(menu, bounds);
    let row = match (menu.index.is_some(), item) {
        (true, DrawingMenuItem::Delete) => 0.0,
        (true, DrawingMenuItem::ClearAll) | (false, DrawingMenuItem::ClearAll) => 1.0,
        (false, DrawingMenuItem::Delete) => return false,
    };
    let row = if menu.index.is_none() { 0.0 } else { row };
    Rectangle {
        y: menu_bounds.y + row * 34.0,
        height: 34.0,
        ..menu_bounds
    }
    .contains(click)
}

fn distance_to_segment(point: Point, start: Point, end: Point) -> f32 {
    let segment = end - start;
    let length_squared = segment.x * segment.x + segment.y * segment.y;
    if length_squared <= f32::EPSILON {
        return point.distance(start);
    }
    let from_start = point - start;
    let t =
        ((from_start.x * segment.x + from_start.y * segment.y) / length_squared).clamp(0.0, 1.0);
    point.distance(start + segment * t)
}

fn canvas_local_center(bounds: Rectangle) -> Point {
    Point::new(bounds.width / 2.0, bounds.height / 2.0)
}

impl canvas::Program<Message> for KlineChart {
    type State = Interaction;

    fn update(
        &self,
        interaction: &mut Interaction,
        event: &Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Option<canvas::Action<Message>> {
        if let Some(action) = self.trading_line_interaction(event, bounds, cursor) {
            return Some(action);
        }
        if self.drawing_tool.is_some()
            || self.drawing_context_menu.is_some()
            || matches!(interaction, Interaction::Drawing { .. })
            || matches!(
                event,
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Right))
            )
            || matches!(
                event,
                Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left))
            ) && cursor
                .position_in(bounds)
                .is_some_and(|position| self.drawing_hit(position, bounds).is_some())
        {
            if let Some(action) = self.drawing_interaction(interaction, event, bounds, cursor) {
                return Some(action);
            }
        }
        super::canvas_interaction(self, interaction, event, bounds, cursor)
    }

    fn draw(
        &self,
        interaction: &Interaction,
        renderer: &Renderer,
        theme: &Theme,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> Vec<Geometry> {
        let chart = self.state();

        if chart.bounds.width == 0.0 {
            return vec![];
        }

        let bounds_size = bounds.size();
        let palette = theme.extended_palette();

        let klines = chart.cache.main.draw(renderer, bounds_size, |frame| {
            let center = Vector::new(bounds.width / 2.0, bounds.height / 2.0);

            frame.translate(center);
            frame.scale(chart.scaling);
            frame.translate(chart.translation);

            let region = chart.visible_region(frame.size());
            let (earliest, latest) = chart.interval_range(&region);

            let price_to_y = |price| chart.price_to_y(price);
            let interval_to_x = |interval| chart.interval_to_x(interval);

            match &self.kind {
                KlineChartKind::Footprint {
                    clusters,
                    scaling,
                    studies,
                } => {
                    let (highest, lowest) = chart.price_range(&region);

                    let max_cluster_qty = self.calc_qty_scales(
                        earliest,
                        latest,
                        highest,
                        lowest,
                        chart.tick_size,
                        *clusters,
                    );

                    let candle_width = 0.1 * chart.cell_width;
                    let content_spacing = ContentGaps::from_view(candle_width, chart.scaling);

                    let imbalance = studies.iter().find_map(|study| {
                        if let FootprintStudy::Imbalance {
                            threshold,
                            color_scale,
                            ignore_zeros,
                        } = study
                        {
                            Some((*threshold, *color_scale, *ignore_zeros))
                        } else {
                            None
                        }
                    });

                    let cell_layout = FootprintCellLayout {
                        cell_w: chart.cell_width,
                        cell_h: chart.cell_height,
                        candle_w: candle_width,
                        pal: palette,
                        cluster: *clusters,
                        gaps: content_spacing,
                        quantity_format: self.visual_config.quantity_format,
                    };

                    let text_size = cell_layout.text_size(chart.scaling);
                    let show_text = cell_layout.should_show_text(chart.scaling);

                    draw_all_npocs(
                        &self.data_source,
                        frame,
                        price_to_y,
                        interval_to_x,
                        &cell_layout,
                        studies,
                        earliest,
                        latest,
                        imbalance.is_some(),
                    );

                    render_data_source(
                        &self.data_source,
                        frame,
                        earliest,
                        latest,
                        interval_to_x,
                        |frame, x_position, kline, trades| {
                            let text_is_obscured =
                                self.indicators.values().filter_map(Option::as_ref).any(
                                    |indicator| {
                                        indicator.obscures_footprint_cell(
                                            kline.time.as_u64(),
                                            x_position,
                                            chart,
                                            region,
                                        )
                                    },
                                );
                            let individual_max = trades.max_cluster_qty_all(cell_layout.cluster);
                            let cluster_scaling =
                                scaling.effective_qty(max_cluster_qty, individual_max);

                            draw_clusters(
                                frame,
                                price_to_y,
                                x_position,
                                &cell_layout,
                                chart.scaling,
                                cluster_scaling,
                                text_size,
                                self.tick_size(),
                                show_text && !text_is_obscured,
                                self.visual_config.show_footprint_summary,
                                imbalance,
                                kline,
                                trades,
                            );
                        },
                    );
                }
                KlineChartKind::Candles => {
                    let candle_width = chart.cell_width * 0.8;

                    render_data_source(
                        &self.data_source,
                        frame,
                        earliest,
                        latest,
                        interval_to_x,
                        |frame, x_position, kline, _| {
                            draw_candle_dp(
                                frame,
                                price_to_y,
                                candle_width,
                                palette,
                                x_position,
                                kline,
                            );
                        },
                    );
                }
            }

            for (kind, indicator) in self.indicators.iter() {
                if matches!(
                    kind,
                    KlineIndicator::VolumeProfile | KlineIndicator::FixedVolumeProfile
                ) {
                    continue;
                }
                if let Some(indicator) = indicator {
                    indicator.draw_overlay(frame, chart, theme, earliest..=latest);
                }
            }

            chart.draw_last_price_line(frame, palette, region);
        });

        let profiles = chart.cache.profiles.draw(renderer, bounds_size, |frame| {
            frame.translate(Vector::new(bounds.width / 2.0, bounds.height / 2.0));
            frame.scale(chart.scaling);
            frame.translate(chart.translation);
            let profile_region = chart.visible_region(frame.size());
            let (profile_earliest, profile_latest) = chart.interval_range(&profile_region);
            for kind in [
                KlineIndicator::VolumeProfile,
                KlineIndicator::FixedVolumeProfile,
            ] {
                if let Some(indicator) = &self.indicators[kind] {
                    indicator.draw_overlay(frame, chart, theme, profile_earliest..=profile_latest);
                }
            }
        });

        let crosshair = chart.cache.crosshair.draw(renderer, bounds_size, |frame| {
            let visible_region = chart.visible_region(bounds_size);
            let visible_range = chart.interval_range(&visible_region);

            if let Some(cursor_position) = cursor.position_in(bounds) {
                let (_, rounded_aggregation) =
                    chart.draw_crosshair(frame, theme, bounds_size, cursor_position, interaction);

                draw_crosshair_tooltip(
                    &self.data_source,
                    &chart.ticker_info,
                    frame,
                    palette,
                    chart.basis,
                    Some(rounded_aggregation),
                    visible_range,
                );
            } else if self.visual_config.data_labels_always_visible {
                draw_crosshair_tooltip(
                    &self.data_source,
                    &chart.ticker_info,
                    frame,
                    palette,
                    chart.basis,
                    None,
                    visible_range,
                );
            }
        });

        let mut orderbook_frame = canvas::Frame::new(renderer, bounds_size);
        if self.visual_config.show_footprint_orderbook
            && matches!(self.kind, KlineChartKind::Footprint { .. })
        {
            draw_footprint_orderbook(
                &mut orderbook_frame,
                chart,
                palette,
                &self.orderbook,
                self.visual_config.quantity_format,
            );
        }
        let orderbook = orderbook_frame.into_geometry();

        let mut drawings_frame = canvas::Frame::new(renderer, bounds_size);
        drawings_frame.translate(Vector::new(bounds.width / 2.0, bounds.height / 2.0));
        drawings_frame.scale(chart.scaling);
        drawings_frame.translate(chart.translation);
        for (index, drawing) in self.drawings.iter().enumerate() {
            draw_manual_drawing(
                &mut drawings_frame,
                chart,
                theme,
                &self.data_source,
                self.visual_config.quantity_format,
                *drawing,
                self.selected_drawing == Some(index),
                matches!(self.kind, KlineChartKind::Footprint { .. }),
            );
        }
        let drawings = drawings_frame.into_geometry();

        let mut ai_frame = canvas::Frame::new(renderer, bounds_size);
        if self
            .ai_annotation
            .as_ref()
            .is_some_and(|value| value.is_valid())
            || self.trading_overlay.is_some()
        {
            ai_frame.translate(Vector::new(bounds.width / 2.0, bounds.height / 2.0));
            ai_frame.scale(chart.scaling);
            ai_frame.translate(chart.translation);
        }
        if let Some(annotation) = self.ai_annotation.as_ref().filter(|value| value.is_valid()) {
            draw_ai_annotation(&mut ai_frame, chart, palette, &self.data_source, annotation);
        }
        if let Some(overlay) = &self.trading_overlay {
            draw_trading_overlay(
                &mut ai_frame,
                chart,
                palette,
                &self.data_source,
                matches!(self.kind, KlineChartKind::Footprint { .. }),
                overlay,
            );
        }
        let ai_annotation = ai_frame.into_geometry();

        let mut context_menu_frame = canvas::Frame::new(renderer, bounds_size);
        if let Some(menu) = self.drawing_context_menu {
            let menu_bounds = drawing_context_menu_bounds(menu, bounds);
            let menu_path =
                Path::rounded_rectangle(menu_bounds.position(), menu_bounds.size(), 3.0.into());
            context_menu_frame.fill(&menu_path, palette.background.weakest.color);
            context_menu_frame.stroke(
                &menu_path,
                Stroke::default()
                    .with_width(1.0)
                    .with_color(palette.background.weak.color),
            );
            let mut rows = Vec::new();
            if menu.index.is_some() {
                rows.push(("Delete", menu_bounds.y + 17.0));
            }
            rows.push((
                "Clear all drawings",
                menu_bounds.y + rows.len() as f32 * 34.0 + 17.0,
            ));
            let hovered = cursor.position_in(bounds);
            for (index, (label, y)) in rows.into_iter().enumerate() {
                let row_bounds = Rectangle {
                    x: menu_bounds.x + 2.0,
                    y: menu_bounds.y + index as f32 * 34.0 + 2.0,
                    width: menu_bounds.width - 4.0,
                    height: 30.0,
                };
                if hovered.is_some_and(|position| row_bounds.contains(position)) {
                    context_menu_frame.fill_rectangle(
                        row_bounds.position(),
                        row_bounds.size(),
                        palette.background.weak.color,
                    );
                }
                context_menu_frame.fill_text(canvas::Text {
                    content: char::from_u32(style::LucideIcon::Trash2 as u32)
                        .expect("Lucide icon codepoints must be valid")
                        .to_string(),
                    position: Point::new(menu_bounds.x + 10.0, y),
                    size: iced::Pixels(13.0),
                    color: palette.background.base.text,
                    align_y: Alignment::Center.into(),
                    font: style::LUCIDE_FONT,
                    ..canvas::Text::default()
                });
                context_menu_frame.fill_text(canvas::Text {
                    content: label.to_string(),
                    position: Point::new(menu_bounds.x + 34.0, y),
                    size: iced::Pixels(style::text_size::BODY),
                    color: palette.background.base.text,
                    align_y: Alignment::Center.into(),
                    font: style::AZERET_MONO,
                    ..canvas::Text::default()
                });
            }
        }
        let context_menu = context_menu_frame.into_geometry();

        vec![
            klines,
            profiles,
            orderbook,
            drawings,
            ai_annotation,
            crosshair,
            context_menu,
        ]
    }

    fn mouse_interaction(
        &self,
        interaction: &Interaction,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if self.dragged_trading_line.get().is_some() {
            return mouse::Interaction::Grabbing;
        }
        if cursor
            .position_in(bounds)
            .is_some_and(|position| self.trading_line_hit(position, bounds).is_some())
        {
            return mouse::Interaction::ResizingVertically;
        }
        match interaction {
            Interaction::Panning { .. } => mouse::Interaction::Grabbing,
            Interaction::Zoomin { .. } => mouse::Interaction::ZoomIn,
            Interaction::Drawing { .. } => mouse::Interaction::Grabbing,
            Interaction::None | Interaction::Ruler { .. } => {
                if cursor.is_over(bounds) {
                    if self.drawing_tool.is_some()
                        || cursor
                            .position_in(bounds)
                            .is_some_and(|position| self.drawing_hit(position, bounds).is_some())
                    {
                        mouse::Interaction::Pointer
                    } else {
                        mouse::Interaction::Crosshair
                    }
                } else {
                    mouse::Interaction::default()
                }
            }
        }
    }
}

fn draw_manual_drawing(
    frame: &mut canvas::Frame,
    chart: &ViewState,
    theme: &Theme,
    data_source: &PlotData<KlineDataPoint>,
    quantity_format: QuantityFormat,
    drawing: Drawing,
    selected: bool,
    is_footprint: bool,
) {
    let palette = theme.extended_palette();
    let region = chart.visible_region(frame.size());
    let start = Point::new(
        chart.interval_to_x(drawing.start.interval),
        chart.price_to_y(Price::from_f64(drawing.start.price)),
    );
    let end = Point::new(
        chart.interval_to_x(drawing.end.interval),
        chart.price_to_y(Price::from_f64(drawing.end.price)),
    );
    let color = palette.primary.base.color;
    let width = if selected { 2.0 } else { 1.25 } / chart.scaling;
    match drawing.tool {
        DrawingTool::TrendLine => frame.stroke(
            &Path::line(start, end),
            Stroke::default().with_width(width).with_color(color),
        ),
        DrawingTool::HorizontalLine => frame.stroke(
            &Path::line(
                Point::new(region.x, start.y),
                Point::new(region.x + region.width, start.y),
            ),
            Stroke::default().with_width(width).with_color(color),
        ),
        DrawingTool::VolumeProfile => {
            crate::chart::indicator::kline::volume_profile::draw_selected_range_profile(
                frame,
                chart,
                theme,
                data_source,
                quantity_format,
                drawing.start.interval,
                drawing.end.interval,
                is_footprint,
            );
            let left = start.x.min(end.x);
            let right = start.x.max(end.x);
            let (top, bottom) = data_source
                .visible_price_range(
                    drawing.start.interval.min(drawing.end.interval),
                    drawing.start.interval.max(drawing.end.interval),
                )
                .map_or((region.y, region.y + region.height), |(first, second)| {
                    let first = chart.price_to_y(Price::from_f32(first));
                    let second = chart.price_to_y(Price::from_f32(second));
                    (first.min(second), first.max(second))
                });
            frame.stroke(
                &Path::rectangle(
                    Point::new(left, top),
                    Size::new((right - left).max(1.0 / chart.scaling), bottom - top),
                ),
                Stroke::default()
                    .with_width(width)
                    .with_color(color.scale_alpha(0.7)),
            );
        }
    }
    if selected {
        let radius = 3.5 / chart.scaling;
        if drawing.tool == DrawingTool::VolumeProfile {
            let center_y = data_source
                .visible_price_range(
                    drawing.start.interval.min(drawing.end.interval),
                    drawing.start.interval.max(drawing.end.interval),
                )
                .map_or((region.y + region.height) / 2.0, |(first, second)| {
                    (chart.price_to_y(Price::from_f32(first))
                        + chart.price_to_y(Price::from_f32(second)))
                        / 2.0
                });
            frame.fill(&Path::circle(Point::new(start.x, center_y), radius), color);
            frame.fill(&Path::circle(Point::new(end.x, center_y), radius), color);
        } else {
            frame.fill(&Path::circle(start, radius), color);
            if drawing.tool != DrawingTool::HorizontalLine {
                frame.fill(&Path::circle(end, radius), color);
            }
        }
    }
}

fn draw_ai_annotation(
    frame: &mut canvas::Frame,
    chart: &ViewState,
    palette: &Extended,
    data_source: &PlotData<KlineDataPoint>,
    annotation: &crate::ai::ChartAnnotation,
) {
    let region = chart.visible_region(frame.size());
    let line_width = 1.0 / chart.scaling;
    let draw_level =
        |frame: &mut canvas::Frame, label: &str, price: f64, strength: f32, color: Color| {
            let y = chart.price_to_y(Price::from_f64(price));
            let strength = strength.clamp(0.0, 100.0);
            let color = color.scale_alpha(0.45 + strength / 100.0 * 0.55);
            frame.stroke(
                &Path::line(
                    Point::new(region.x, y),
                    Point::new(region.x + region.width, y),
                ),
                Stroke::default()
                    .with_width(line_width * (1.0 + strength / 100.0))
                    .with_color(color),
            );
            draw_cluster_text(
                frame,
                &format!("{label} {:.0} {price}", strength),
                Point::new(region.x + region.width - 4.0 / chart.scaling, y),
                11.0 / chart.scaling,
                color,
                Alignment::End,
                Alignment::End,
            );
        };

    for level in &annotation.levels {
        if level.is_support {
            draw_level(
                frame,
                "S",
                level.price,
                level.strength,
                palette.success.weak.color,
            );
        } else {
            draw_level(
                frame,
                "R",
                level.price,
                level.strength,
                palette.danger.weak.color,
            );
        }
    }
    for &price in &annotation.targets {
        draw_level(frame, "TP", price, 100.0, palette.success.base.color);
    }
    if let Some(price) = annotation.stop_loss {
        draw_level(frame, "SL", price, 100.0, palette.danger.base.color);
    }
    if let Some(price) = annotation.entry_low {
        draw_level(frame, "ENTRY", price, 100.0, palette.primary.base.color);
    }
    if let Some(price) = annotation.entry_high
        && annotation.entry_low != Some(price)
    {
        draw_level(frame, "ENTRY", price, 100.0, palette.primary.base.color);
    }

    if let (Some(low), Some(high)) = (annotation.entry_low, annotation.entry_high) {
        let top = chart.price_to_y(Price::from_f64(high.max(low)));
        let bottom = chart.price_to_y(Price::from_f64(high.min(low)));
        frame.fill_rectangle(
            Point::new(region.x, top.min(bottom)),
            Size::new(region.width, (bottom - top).abs().max(line_width)),
            palette.primary.weak.color.scale_alpha(0.18),
        );
    }

    let signal_kline = match (data_source, annotation.time_ms) {
        (PlotData::TimeBased(series), Some(time_ms)) => series
            .datapoints
            .range(..=UnixMs(time_ms))
            .next_back()
            .or_else(|| series.datapoints.first_key_value())
            .map(|(_, dp)| &dp.kline),
        (PlotData::TickBased(series), Some(time_ms)) => series
            .datapoints
            .iter()
            .min_by_key(|dp| dp.kline.time.as_u64().abs_diff(time_ms))
            .map(|dp| &dp.kline),
        (PlotData::TimeBased(series), None) => {
            series.datapoints.last_key_value().map(|(_, dp)| &dp.kline)
        }
        (PlotData::TickBased(series), None) => series.datapoints.last().map(|dp| &dp.kline),
    };
    if let Some(kline) = signal_kline {
        let signal_color = match annotation.signal {
            crate::ai::ChartSignal::Long => palette.success.base.color,
            crate::ai::ChartSignal::Short => palette.danger.base.color,
            crate::ai::ChartSignal::Neutral => palette.primary.base.color,
        };
        if annotation.mark_time {
            frame.stroke(
                &Path::circle(
                    Point::new(
                        chart.interval_to_x(kline.time.as_u64()),
                        chart.price_to_y(kline.close),
                    ),
                    4.0 / chart.scaling,
                ),
                Stroke::default()
                    .with_width(1.5 / chart.scaling)
                    .with_color(signal_color),
            );
        }
        match annotation.signal {
            crate::ai::ChartSignal::Long => {
                crate::chart::indicator::kline::flow_metrics::draw_direction_arrow(
                    frame,
                    chart.interval_to_x(kline.time.as_u64()),
                    chart.price_to_y(kline.low),
                    true,
                    5.0,
                    chart.scaling,
                    signal_color,
                );
            }
            crate::ai::ChartSignal::Short => {
                crate::chart::indicator::kline::flow_metrics::draw_direction_arrow(
                    frame,
                    chart.interval_to_x(kline.time.as_u64()),
                    chart.price_to_y(kline.high),
                    false,
                    5.0,
                    chart.scaling,
                    signal_color,
                );
            }
            crate::ai::ChartSignal::Neutral => {}
        }
    }
}

fn draw_trading_overlay(
    frame: &mut canvas::Frame,
    chart: &ViewState,
    palette: &Extended,
    data_source: &PlotData<KlineDataPoint>,
    footprint: bool,
    overlay: &crate::trading::PositionOverlay,
) {
    let region = chart.visible_region(frame.size());
    let line_width = 1.0 / chart.scaling;
    let mut drawn_trades = HashSet::new();
    for trade in &overlay.trade_history {
        let kline = match data_source {
            PlotData::TimeBased(series) => series
                .datapoints
                .range(..=UnixMs(trade.time_ms))
                .next_back()
                .map(|(_, point)| &point.kline),
            PlotData::TickBased(series) => series
                .datapoints
                .iter()
                .min_by_key(|point| point.kline.time.as_u64().abs_diff(trade.time_ms))
                .map(|point| &point.kline),
        };
        let Some(kline) = kline else {
            continue;
        };
        let buy = trade.side == crate::trading::Side::Buy;
        if !drawn_trades.insert((kline.time.as_u64(), buy)) {
            continue;
        }
        let marker_price = if footprint {
            if buy {
                kline.open.min(kline.close)
            } else {
                kline.open.max(kline.close)
            }
        } else if buy {
            kline.low
        } else {
            kline.high
        };
        let x = chart.interval_to_x(kline.time.as_u64());
        let y = chart.price_to_y(marker_price);
        if x >= region.x
            && x <= region.x + region.width
            && y >= region.y
            && y <= region.y + region.height
        {
            crate::chart::indicator::kline::flow_metrics::draw_direction_arrow(
                frame,
                x,
                y,
                buy,
                5.0,
                chart.scaling,
                if buy {
                    palette.success.base.color
                } else {
                    palette.danger.base.color
                },
            );
        }
    }
    let mut levels = Vec::new();
    let mut push_level = |label: String, price: f64, color: Color| {
        if price.is_finite() && price > 0.0 {
            levels.push((
                label,
                price,
                color,
                chart.price_to_y(Price::from_f64(price)),
            ));
        }
    };
    if let Some(price) = overlay.entry_price {
        push_level(
            format!(
                "{} | PnL {:.2} USDT",
                if overlay.is_long { "LONG" } else { "SHORT" },
                overlay.unrealized_pnl
            ),
            price,
            palette.primary.base.color,
        );
    }
    if let Some(price) = overlay.liquidation_price {
        push_level(
            format!(
                "{} {price:.2}",
                if overlay.liquidation_estimated {
                    "LIQ EST"
                } else {
                    "LIQ"
                }
            ),
            price,
            palette.danger.weak.color,
        );
    }
    if let Some(price) = overlay.stop_loss {
        push_level(
            format!(
                "SL {price:.2} | PnL {:.2} USDT",
                overlay.stop_loss_pnl.unwrap_or_default()
            ),
            price,
            palette.danger.base.color,
        );
    }
    if let Some(price) = overlay.take_profit {
        push_level(
            format!(
                "TP {price:.2} | PnL {:.2} USDT",
                overlay.take_profit_pnl.unwrap_or_default()
            ),
            price,
            palette.success.base.color,
        );
    }
    for order in &overlay.orders {
        push_level(
            format!("{} {:.2}", order.label, order.price),
            order.price,
            if order.side == crate::trading::Side::Buy {
                palette.success.base.color
            } else {
                palette.danger.base.color
            },
        );
    }
    let label_half_height = 7.0 / chart.scaling;
    levels.retain(|level| {
        trading_level_is_visible(
            level.3,
            region.y,
            region.y + region.height,
            label_half_height,
        )
    });
    levels.sort_by(|a, b| a.3.total_cmp(&b.3));
    let label_offset = 3.0 / chart.scaling;
    let label_positions = spread_trading_label_positions(
        &levels
            .iter()
            .map(|level| level.3 - label_offset)
            .collect::<Vec<_>>(),
        region.y + label_half_height,
        region.y + region.height - label_half_height,
        14.0 / chart.scaling,
    );
    for ((label, price, color, _), label_y) in levels.into_iter().zip(label_positions) {
        let y = chart.price_to_y(Price::from_f64(price));
        frame.stroke(
            &Path::line(
                Point::new(region.x, y),
                Point::new(region.x + region.width, y),
            ),
            Stroke::default()
                .with_width(line_width)
                .with_color(color.scale_alpha(0.85)),
        );
        draw_cluster_text(
            frame,
            &label,
            Point::new(region.x + 4.0 / chart.scaling, label_y),
            11.0 / chart.scaling,
            color,
            Alignment::Start,
            Alignment::End,
        );
    }
}

fn trading_level_is_visible(y: f32, top: f32, bottom: f32, padding: f32) -> bool {
    y >= top + padding && y <= bottom - padding
}

fn spread_trading_label_positions(ys: &[f32], top: f32, bottom: f32, gap: f32) -> Vec<f32> {
    let mut result = Vec::with_capacity(ys.len());
    for &y in ys {
        result.push(
            result
                .last()
                .map_or(y.max(top), |previous| y.max(*previous + gap)),
        );
    }
    if let Some(last) = result.last().copied()
        && last > bottom
    {
        let shift = last - bottom;
        result.iter_mut().for_each(|y| *y -= shift);
    }
    for index in (0..result.len().saturating_sub(1)).rev() {
        result[index] = result[index].min(result[index + 1] - gap);
    }
    if let Some(first) = result.first().copied()
        && first < top
    {
        let shift = top - first;
        result.iter_mut().for_each(|y| *y += shift);
    }
    result
}

fn draw_footprint_orderbook(
    frame: &mut canvas::Frame,
    chart: &ViewState,
    palette: &Extended,
    orderbook: &[GroupedDepth; 2],
    quantity_format: QuantityFormat,
) {
    let right = frame.width();
    let height = frame.height();
    let left = (right - FOOTPRINT_ORDERBOOK_WIDTH).max(0.0);
    let row_height = (chart.cell_height * chart.scaling).max(1.0);
    let screen_y =
        |price| (chart.price_to_y(price) + chart.translation.y) * chart.scaling + height / 2.0;
    let visible_qty = orderbook
        .iter()
        .flat_map(|side| side.orders.iter())
        .filter(|(price, _)| {
            let y = screen_y(**price);
            y + row_height / 2.0 >= 0.0 && y - row_height / 2.0 <= height
        })
        .map(|(_, qty)| qty.to_f64())
        .fold(0.0_f64, f64::max);

    if visible_qty <= f64::EPSILON {
        return;
    }

    let bid_color = palette.success.base.color;
    let ask_color = palette.danger.base.color;

    for (side, color) in [(Side::Bid, bid_color), (Side::Ask, ask_color)] {
        for (price, qty) in &orderbook[side.idx()].orders {
            let y = screen_y(*price);
            if y + row_height / 2.0 < 0.0 || y - row_height / 2.0 > height {
                continue;
            }
            let width = ((qty.to_f64() / visible_qty) as f32 * (right - left - 4.0)).max(1.0);
            frame.fill_rectangle(
                Point::new(right - width, y - row_height / 2.0),
                Size::new(width, row_height),
                color.scale_alpha(0.28),
            );
            if row_height >= 10.0 {
                frame.fill_text(canvas::Text {
                    content: quantity_format.format(qty.to_f64()),
                    position: Point::new(right - 4.0, y),
                    size: iced::Pixels(row_height.min(12.0) - 2.0),
                    color,
                    align_x: Alignment::End.into(),
                    align_y: Alignment::Center.into(),
                    font: style::AZERET_MONO,
                    ..canvas::Text::default()
                });
            }
        }
    }
}

fn draw_footprint_kline(
    frame: &mut canvas::Frame,
    price_to_y: impl Fn(Price) -> f32,
    x_position: f32,
    candle_width: f32,
    kline: &Kline,
    palette: &Extended,
) {
    let y_open = price_to_y(kline.open);
    let y_high = price_to_y(kline.high);
    let y_low = price_to_y(kline.low);
    let y_close = price_to_y(kline.close);

    let body_color = if kline.close >= kline.open {
        palette.success.weak.color
    } else {
        palette.danger.weak.color
    };
    frame.fill_rectangle(
        Point::new(x_position - (candle_width / 8.0), y_open.min(y_close)),
        Size::new(candle_width / 4.0, (y_open - y_close).abs()),
        body_color,
    );

    let wick_color = if kline.close >= kline.open {
        palette.success.weak.color
    } else {
        palette.danger.weak.color
    };
    let marker_line = Stroke::with_color(
        Stroke {
            width: 1.0,
            ..Default::default()
        },
        wick_color.scale_alpha(0.6),
    );
    frame.stroke(
        &Path::line(
            Point::new(x_position, y_high),
            Point::new(x_position, y_low),
        ),
        marker_line,
    );
}

fn draw_candle_dp(
    frame: &mut canvas::Frame,
    price_to_y: impl Fn(Price) -> f32,
    candle_width: f32,
    palette: &Extended,
    x_position: f32,
    kline: &Kline,
) {
    let y_open = price_to_y(kline.open);
    let y_high = price_to_y(kline.high);
    let y_low = price_to_y(kline.low);
    let y_close = price_to_y(kline.close);

    let body_color = if kline.close >= kline.open {
        palette.success.base.color
    } else {
        palette.danger.base.color
    };
    frame.fill_rectangle(
        Point::new(x_position - (candle_width / 2.0), y_open.min(y_close)),
        Size::new(candle_width, (y_open - y_close).abs()),
        body_color,
    );

    let wick_color = if kline.close >= kline.open {
        palette.success.base.color
    } else {
        palette.danger.base.color
    };
    frame.fill_rectangle(
        Point::new(x_position - (candle_width / 8.0), y_high),
        Size::new(candle_width / 4.0, (y_high - y_low).abs()),
        wick_color,
    );
}

fn render_data_source<F>(
    data_source: &PlotData<KlineDataPoint>,
    frame: &mut canvas::Frame,
    earliest: u64,
    latest: u64,
    interval_to_x: impl Fn(u64) -> f32,
    draw_fn: F,
) where
    F: Fn(&mut canvas::Frame, f32, &Kline, &KlineTrades),
{
    match data_source {
        PlotData::TickBased(tick_aggr) => {
            let earliest = earliest as usize;
            let latest = latest as usize;

            tick_aggr
                .datapoints
                .iter()
                .rev()
                .enumerate()
                .filter(|(index, _)| *index <= latest && *index >= earliest)
                .for_each(|(index, tick_aggr)| {
                    let x_position = interval_to_x(index as u64);

                    draw_fn(frame, x_position, &tick_aggr.kline, &tick_aggr.footprint);
                });
        }
        PlotData::TimeBased(timeseries) => {
            if latest < earliest {
                return;
            }

            timeseries
                .datapoints
                .range(UnixMs::new(earliest)..=UnixMs::new(latest))
                .for_each(|(timestamp, dp)| {
                    let x_position = interval_to_x(timestamp.as_u64());

                    draw_fn(frame, x_position, &dp.kline, &dp.footprint);
                });
        }
    }
}

fn draw_all_npocs(
    data_source: &PlotData<KlineDataPoint>,
    frame: &mut canvas::Frame,
    price_to_y: impl Fn(Price) -> f32,
    interval_to_x: impl Fn(u64) -> f32,
    layout: &FootprintCellLayout<'_>,
    studies: &[FootprintStudy],
    visible_earliest: u64,
    visible_latest: u64,
    imb_study_on: bool,
) {
    let Some(lookback) = studies.iter().find_map(|study| {
        if let FootprintStudy::NPoC { lookback } = study {
            Some(*lookback)
        } else {
            None
        }
    }) else {
        return;
    };

    let (filled_color, naked_color) = (
        layout.pal.background.strong.color,
        if layout.pal.is_dark {
            layout.pal.warning.weak.color.scale_alpha(0.5)
        } else {
            layout.pal.warning.strong.color
        },
    );

    let line_height = layout.cell_h.min(1.0);

    let bar_width_factor: f32 = 0.9;
    let inset = (layout.cell_w * (1.0 - bar_width_factor)) / 2.0;

    let candle_lane_factor: f32 = match layout.cluster {
        ClusterKind::VolumeProfile | ClusterKind::DeltaProfile => 0.25,
        ClusterKind::BidAsk | ClusterKind::Table => 1.0,
    };

    let start_x_for = |cell_center_x: f32| -> f32 {
        match layout.cluster {
            ClusterKind::Table => cell_center_x + (layout.cell_w / 2.0) - inset,
            ClusterKind::BidAsk => {
                cell_center_x + (layout.candle_w / 2.0) + layout.gaps.candle_to_cluster
            }
            ClusterKind::VolumeProfile | ClusterKind::DeltaProfile => {
                let content_left = (cell_center_x - (layout.cell_w / 2.0)) + inset;
                let candle_lane_left = content_left
                    + if imb_study_on {
                        layout.candle_w + layout.gaps.marker_to_candle
                    } else {
                        0.0
                    };
                candle_lane_left
                    + layout.candle_w * candle_lane_factor
                    + layout.gaps.candle_to_cluster
            }
        }
    };

    let wick_x_for = |cell_center_x: f32| -> f32 {
        match layout.cluster {
            ClusterKind::BidAsk | ClusterKind::Table => cell_center_x,
            ClusterKind::VolumeProfile | ClusterKind::DeltaProfile => {
                let content_left = (cell_center_x - (layout.cell_w / 2.0)) + inset;
                let candle_lane_left = content_left
                    + if imb_study_on {
                        layout.candle_w + layout.gaps.marker_to_candle
                    } else {
                        0.0
                    };
                candle_lane_left + (layout.candle_w * candle_lane_factor) / 2.0
                    - (layout.gaps.candle_to_cluster * 0.5)
            }
        }
    };

    let end_x_for = |cell_center_x: f32| -> f32 {
        match layout.cluster {
            ClusterKind::Table => {
                let content_left = cell_center_x - (layout.cell_w / 2.0) + inset;
                let content_right = cell_center_x + (layout.cell_w / 2.0) - inset;
                let table_layout = TableLayout::new(
                    content_left,
                    content_right,
                    layout.candle_w,
                    layout.gaps,
                    imb_study_on,
                );
                table_layout.table_left
            }
            ClusterKind::BidAsk => {
                cell_center_x - (layout.candle_w / 2.0) - layout.gaps.candle_to_cluster
            }
            ClusterKind::VolumeProfile | ClusterKind::DeltaProfile => wick_x_for(cell_center_x),
        }
    };

    let rightmost_cell_center_x = {
        let earliest_x = interval_to_x(visible_earliest);
        let latest_x = interval_to_x(visible_latest);
        if earliest_x > latest_x {
            earliest_x
        } else {
            latest_x
        }
    };

    let mut draw_the_line = |interval: u64, poc: &PointOfControl| {
        let start_x = start_x_for(interval_to_x(interval));

        let (line_width, color) = match poc.status {
            NPoc::Naked => {
                let end_x = end_x_for(rightmost_cell_center_x);
                let line_width = end_x - start_x;
                if line_width.abs() <= layout.cell_w {
                    return;
                }
                (line_width, naked_color)
            }
            NPoc::Filled { at } => {
                let end_x = end_x_for(interval_to_x(at));
                let line_width = end_x - start_x;
                if line_width.abs() <= layout.cell_w {
                    return;
                }
                (line_width, filled_color)
            }
            _ => return,
        };

        frame.fill_rectangle(
            Point::new(start_x, price_to_y(poc.price) - line_height / 2.0),
            Size::new(line_width, line_height),
            color,
        );
    };

    match data_source {
        PlotData::TickBased(tick_aggr) => {
            tick_aggr
                .datapoints
                .iter()
                .rev()
                .enumerate()
                .take(lookback)
                .filter_map(|(index, dp)| dp.footprint.poc.as_ref().map(|poc| (index as u64, poc)))
                .for_each(|(interval, poc)| draw_the_line(interval, poc));
        }
        PlotData::TimeBased(timeseries) => {
            timeseries
                .datapoints
                .iter()
                .rev()
                .take(lookback)
                .filter_map(|(timestamp, dp)| {
                    dp.footprint
                        .poc
                        .as_ref()
                        .map(|poc| (timestamp.as_u64(), poc))
                })
                .for_each(|(interval, poc)| draw_the_line(interval, poc));
        }
    }
}

fn draw_clusters(
    frame: &mut canvas::Frame,
    price_to_y: impl Fn(Price) -> f32,
    x_position: f32,
    layout: &FootprintCellLayout<'_>,
    scaling: f32,
    max_cluster_qty: f64,
    text_size: f32,
    step: PriceStep,
    show_text: bool,
    show_summary: bool,
    imbalance: Option<(usize, Option<usize>, bool)>,
    kline: &Kline,
    footprint: &KlineTrades,
) {
    let text_color = layout.pal.background.weakest.text;

    let bar_width_factor: f32 = 0.9;
    let inset = (layout.cell_w * (1.0 - bar_width_factor)) / 2.0;

    let cell_left = x_position - (layout.cell_w / 2.0);
    let content_left = cell_left + inset;
    let content_right = x_position + (layout.cell_w / 2.0) - inset;

    let mut table_layout: Option<TableLayout> = None;

    match layout.cluster {
        ClusterKind::VolumeProfile | ClusterKind::DeltaProfile => {
            let area = ProfileArea::new(
                content_left,
                content_right,
                layout.candle_w,
                layout.gaps,
                imbalance.is_some(),
            );
            for (price, group) in &footprint.trades {
                let buy_qty = group.buy_qty.to_f64();
                let sell_qty = group.sell_qty.to_f64();
                let y = price_to_y(*price);

                match layout.cluster {
                    ClusterKind::VolumeProfile => {
                        let label = layout.quantity_format.format(f64::from(group.total_qty()));
                        let row_show_text =
                            show_text && footprint_text_fits(&label, text_size, area.bars_width);
                        super::draw_volume_bar(
                            frame,
                            area.bars_left,
                            y,
                            buy_qty,
                            sell_qty,
                            max_cluster_qty,
                            area.bars_width,
                            layout.cell_h,
                            layout.pal.success.base.color,
                            layout.pal.danger.base.color,
                            if row_show_text { 0.25 } else { 1.0 },
                            true,
                        );

                        if row_show_text {
                            draw_cluster_text(
                                frame,
                                &label,
                                Point::new(area.bars_left, y),
                                text_size,
                                text_color,
                                Alignment::Start,
                                Alignment::Center,
                            );
                        }
                    }
                    ClusterKind::DeltaProfile => {
                        let delta = group.delta_qty().to_f64();
                        let label = layout.quantity_format.format(delta);
                        let row_show_text =
                            show_text && footprint_text_fits(&label, text_size, area.bars_width);
                        if row_show_text {
                            draw_cluster_text(
                                frame,
                                &label,
                                Point::new(area.bars_left, y),
                                text_size,
                                text_color,
                                Alignment::Start,
                                Alignment::Center,
                            );
                        }

                        let bar_width = (delta.abs() / max_cluster_qty) as f32 * area.bars_width;
                        if bar_width > 0.0 {
                            let color = if delta >= 0.0 {
                                layout.pal.success.base.color.scale_alpha(if row_show_text {
                                    0.25
                                } else {
                                    1.0
                                })
                            } else {
                                layout.pal.danger.base.color.scale_alpha(if row_show_text {
                                    0.25
                                } else {
                                    1.0
                                })
                            };
                            frame.fill_rectangle(
                                Point::new(area.bars_left, y - (layout.cell_h / 2.0)),
                                Size::new(bar_width, layout.cell_h),
                                color,
                            );
                        }
                    }
                    _ => {}
                }

                if let Some((threshold, color_scale, ignore_zeros)) = imbalance {
                    let higher_price = price.add_steps(1, step);

                    let rect_w = ((area.imb_marker_width - 1.0) / 2.0).max(1.0);
                    let buyside_x = area.imb_marker_left + area.imb_marker_width - rect_w;
                    let sellside_x =
                        area.imb_marker_left + area.imb_marker_width - (2.0 * rect_w) - 1.0;

                    draw_imbalance_markers(
                        frame,
                        &price_to_y,
                        footprint,
                        *price,
                        sell_qty,
                        higher_price,
                        threshold,
                        color_scale,
                        ignore_zeros,
                        layout.cell_h,
                        layout.pal,
                        buyside_x,
                        sellside_x,
                        rect_w,
                    );
                }
            }

            draw_footprint_kline(
                frame,
                &price_to_y,
                area.candle_center_x,
                layout.candle_w,
                kline,
                layout.pal,
            );
        }
        ClusterKind::Table => {
            let tl = TableLayout::new(
                content_left,
                content_right,
                layout.candle_w,
                layout.gaps,
                imbalance.is_some(),
            );
            let area = TableArea::new(frame, &price_to_y, &tl, layout.candle_w, kline, layout.pal);
            table_layout = Some(tl);
            let table_width = area.width();
            let half_width = table_width / 2.0;
            let cell_border = 1.0;
            let grid_color = layout.pal.background.weakest.text.scale_alpha(0.32);
            for (price, group) in &footprint.trades {
                let buy_qty = group.buy_qty.to_f64();
                let sell_qty = group.sell_qty.to_f64();
                let y = price_to_y(*price);
                let row_top = y - (layout.cell_h / 2.0);

                frame.fill_rectangle(
                    Point::new(area.table_left, row_top),
                    Size::new(half_width, layout.cell_h),
                    ImbalanceSide::Sell.volume_bg_color(sell_qty, max_cluster_qty, layout.pal),
                );
                frame.fill_rectangle(
                    Point::new(area.table_left + half_width, row_top),
                    Size::new(half_width, layout.cell_h),
                    ImbalanceSide::Buy.volume_bg_color(buy_qty, max_cluster_qty, layout.pal),
                );
                let sell_text_color = ImbalanceSide::Sell.volume_text_color(
                    sell_qty,
                    max_cluster_qty,
                    text_color,
                    layout.pal,
                );
                let buy_text_color = ImbalanceSide::Buy.volume_text_color(
                    buy_qty,
                    max_cluster_qty,
                    text_color,
                    layout.pal,
                );

                if let Some((threshold, color_scale, ignore_zeros)) = imbalance {
                    if let Some(alpha) = ImbalanceSide::Sell.color_alpha(
                        footprint,
                        *price,
                        sell_qty,
                        step,
                        threshold,
                        color_scale,
                        ignore_zeros,
                    ) {
                        ImbalanceSide::Sell.draw_table_marker(
                            frame,
                            layout.pal,
                            alpha,
                            sell_qty,
                            max_cluster_qty,
                            Rectangle::new(
                                Point::new(area.table_left, row_top),
                                Size::new(half_width, layout.cell_h),
                            ),
                        );
                    }

                    if let Some(alpha) = ImbalanceSide::Buy.color_alpha(
                        footprint,
                        *price,
                        buy_qty,
                        step,
                        threshold,
                        color_scale,
                        ignore_zeros,
                    ) {
                        ImbalanceSide::Buy.draw_table_marker(
                            frame,
                            layout.pal,
                            alpha,
                            buy_qty,
                            max_cluster_qty,
                            Rectangle::new(
                                Point::new(area.table_left + half_width, row_top),
                                Size::new(half_width, layout.cell_h),
                            ),
                        );
                    }
                }

                frame.fill_rectangle(
                    Point::new(area.table_left, row_top),
                    Size::new(table_width, cell_border),
                    grid_color,
                );
                frame.fill_rectangle(
                    Point::new(area.table_left, row_top + layout.cell_h - cell_border),
                    Size::new(table_width, cell_border),
                    grid_color,
                );
                frame.fill_rectangle(
                    Point::new(area.table_left, row_top),
                    Size::new(cell_border, layout.cell_h),
                    grid_color,
                );
                frame.fill_rectangle(
                    Point::new(area.table_left + half_width, row_top),
                    Size::new(cell_border, layout.cell_h),
                    grid_color,
                );
                frame.fill_rectangle(
                    Point::new(area.table_right - cell_border, row_top),
                    Size::new(cell_border, layout.cell_h),
                    grid_color,
                );

                let sell_label = layout.quantity_format.format(sell_qty);
                let buy_label = layout.quantity_format.format(buy_qty);
                if show_text && footprint_text_fits(&sell_label, text_size, half_width - 6.0) {
                    draw_cluster_text(
                        frame,
                        &sell_label,
                        Point::new(area.table_left + half_width - 3.0, y),
                        text_size,
                        sell_text_color,
                        Alignment::End,
                        Alignment::Center,
                    );
                }
                if show_text && footprint_text_fits(&buy_label, text_size, half_width - 6.0) {
                    draw_cluster_text(
                        frame,
                        &buy_label,
                        Point::new(area.table_left + half_width + 3.0, y),
                        text_size,
                        buy_text_color,
                        Alignment::Start,
                        Alignment::Center,
                    );
                }
            }
        }
        ClusterKind::BidAsk => {
            let area = BidAskArea::new(
                x_position,
                content_left,
                content_right,
                layout.candle_w,
                layout.gaps,
            );

            let imb_marker_reserve = if imbalance.is_some() {
                ((area.imb_marker_width - 1.0) / 2.0).max(1.0)
            } else {
                0.0
            };

            let right_max_x =
                area.bid_area_right - imb_marker_reserve - (2.0 * layout.gaps.marker_to_bars);
            let right_area_width = (right_max_x - area.bid_area_left).max(0.0);

            let left_min_x =
                area.ask_area_left + imb_marker_reserve + (2.0 * layout.gaps.marker_to_bars);
            let left_area_width = (area.ask_area_right - left_min_x).max(0.0);

            for (price, group) in &footprint.trades {
                let buy_qty = group.buy_qty.to_f64();
                let sell_qty = group.sell_qty.to_f64();
                let y = price_to_y(*price);
                let buy_label = layout.quantity_format.format(buy_qty);
                let sell_label = layout.quantity_format.format(sell_qty);
                let show_buy_text =
                    show_text && footprint_text_fits(&buy_label, text_size, right_area_width);
                let show_sell_text =
                    show_text && footprint_text_fits(&sell_label, text_size, left_area_width);

                if buy_qty > 0.0 && right_area_width > 0.0 {
                    if show_buy_text {
                        draw_cluster_text(
                            frame,
                            &buy_label,
                            Point::new(area.bid_area_left, y),
                            text_size,
                            text_color,
                            Alignment::Start,
                            Alignment::Center,
                        );
                    }

                    let bar_width = (buy_qty / max_cluster_qty) as f32 * right_area_width;
                    if bar_width > 0.0 {
                        frame.fill_rectangle(
                            Point::new(area.bid_area_left, y - (layout.cell_h / 2.0)),
                            Size::new(bar_width, layout.cell_h),
                            layout.pal.success.base.color.scale_alpha(if show_buy_text {
                                0.25
                            } else {
                                1.0
                            }),
                        );
                    }
                }
                if sell_qty > 0.0 && left_area_width > 0.0 {
                    if show_sell_text {
                        draw_cluster_text(
                            frame,
                            &sell_label,
                            Point::new(area.ask_area_right, y),
                            text_size,
                            text_color,
                            Alignment::End,
                            Alignment::Center,
                        );
                    }

                    let bar_width = (sell_qty / max_cluster_qty) as f32 * left_area_width;
                    if bar_width > 0.0 {
                        frame.fill_rectangle(
                            Point::new(area.ask_area_right, y - (layout.cell_h / 2.0)),
                            Size::new(-bar_width, layout.cell_h),
                            layout.pal.danger.base.color.scale_alpha(if show_sell_text {
                                0.25
                            } else {
                                1.0
                            }),
                        );
                    }
                }

                if let Some((threshold, color_scale, ignore_zeros)) = imbalance
                    && area.imb_marker_width > 0.0
                {
                    let higher_price = price.add_steps(1, step);

                    let rect_width = ((area.imb_marker_width - 1.0) / 2.0).max(1.0);

                    let buyside_x = area.bid_area_right - rect_width - layout.gaps.marker_to_bars;
                    let sellside_x = area.ask_area_left + layout.gaps.marker_to_bars;

                    draw_imbalance_markers(
                        frame,
                        &price_to_y,
                        footprint,
                        *price,
                        sell_qty,
                        higher_price,
                        threshold,
                        color_scale,
                        ignore_zeros,
                        layout.cell_h,
                        layout.pal,
                        buyside_x,
                        sellside_x,
                        rect_width,
                    );
                }
            }

            draw_footprint_kline(
                frame,
                &price_to_y,
                area.candle_center_x,
                layout.candle_w,
                kline,
                layout.pal,
            );
        }
    }

    if show_summary {
        let Some(summary) = FootprintSummary::from_trades(footprint) else {
            return;
        };

        let summary_layout = FootprintSummaryLayout::new(layout.cell_h, scaling);
        let volume_label = format!(
            "V: {}",
            layout.quantity_format.format(summary.total.to_f64())
        );
        let delta_label = format!(
            "Δ: {}",
            layout.quantity_format.format(summary.delta.to_f64())
        );
        let summary_x = match layout.cluster {
            ClusterKind::Table => {
                let tl = table_layout
                    .as_ref()
                    .expect("TableLayout must be set for Table cluster");
                (tl.table_left + tl.table_right) / 2.0
            }
            _ => x_position,
        };

        let summary_width = table_layout
            .as_ref()
            .map_or(layout.cell_w, |table| table.table_right - table.table_left);
        if !footprint_text_fits(&volume_label, summary_layout.text_size, summary_width)
            || !footprint_text_fits(&delta_label, summary_layout.text_size, summary_width)
        {
            return;
        }

        let lowest_trade_price = footprint.trades.keys().min();

        let summary_y = match lowest_trade_price {
            Some(p) => price_to_y(*p) + layout.cell_h / 2.0 + summary_layout.gap,
            None => price_to_y(kline.low) + layout.cell_h / 2.0 + summary_layout.gap,
        };

        draw_cluster_text(
            frame,
            &volume_label,
            Point::new(summary_x, summary_y),
            summary_layout.text_size,
            layout.pal.background.weakest.text,
            Alignment::Center,
            Alignment::Start,
        );

        let delta_color = if summary.delta >= Qty::ZERO {
            layout.pal.success.base.color
        } else {
            layout.pal.danger.base.color
        };

        draw_cluster_text(
            frame,
            &delta_label,
            Point::new(
                summary_x,
                summary_y + summary_layout.text_size + summary_layout.line_gap,
            ),
            summary_layout.text_size,
            delta_color,
            Alignment::Center,
            Alignment::Start,
        );
    }
}

fn draw_imbalance_markers(
    frame: &mut canvas::Frame,
    price_to_y: &impl Fn(Price) -> f32,
    footprint: &KlineTrades,
    price: Price,
    sell_qty: f64,
    higher_price: Price,
    threshold: usize,
    color_scale: Option<usize>,
    ignore_zeros: bool,
    cell_height: f32,
    palette: &Extended,
    buyside_x: f32,
    sellside_x: f32,
    rect_width: f32,
) {
    if ignore_zeros && sell_qty <= 0.0 {
        return;
    }

    if let Some(group) = footprint.trades.get(&higher_price) {
        let diagonal_buy_qty = group.buy_qty.to_f64();

        if ignore_zeros && diagonal_buy_qty <= 0.0 {
            return;
        }

        let rect_height = cell_height / 2.0;

        let alpha_from_ratio = |ratio: f64| -> f32 {
            if let Some(scale) = color_scale {
                let divisor = (scale as f64 / 10.0) - 1.0;
                (0.2 + 0.8 * ((ratio - 1.0) / divisor).min(1.0)).min(1.0) as f32
            } else {
                1.0
            }
        };

        if diagonal_buy_qty >= sell_qty {
            let required_qty = sell_qty * (100 + threshold) as f64 / 100.0;
            if diagonal_buy_qty > required_qty {
                let ratio = diagonal_buy_qty / required_qty;
                let alpha = alpha_from_ratio(ratio);

                let y = price_to_y(higher_price);
                frame.fill_rectangle(
                    Point::new(buyside_x, y - (rect_height / 2.0)),
                    Size::new(rect_width, rect_height),
                    ImbalanceSide::Buy.marker_bg_color(palette, alpha),
                );
            }
        } else {
            let required_qty = diagonal_buy_qty * (100 + threshold) as f64 / 100.0;
            if sell_qty > required_qty {
                let ratio = sell_qty / required_qty;
                let alpha = alpha_from_ratio(ratio);

                let y = price_to_y(price);
                frame.fill_rectangle(
                    Point::new(sellside_x, y - (rect_height / 2.0)),
                    Size::new(rect_width, rect_height),
                    ImbalanceSide::Sell.marker_bg_color(palette, alpha),
                );
            }
        }
    }
}

fn draw_cluster_text(
    frame: &mut canvas::Frame,
    text: &str,
    position: Point,
    text_size: f32,
    color: iced::Color,
    align_x: Alignment,
    align_y: Alignment,
) {
    frame.fill_text(canvas::Text {
        content: text.to_string(),
        position,
        size: iced::Pixels(text_size),
        color,
        align_x: align_x.into(),
        align_y: align_y.into(),
        font: style::AZERET_MONO,
        ..canvas::Text::default()
    });
}

fn draw_crosshair_tooltip(
    data: &PlotData<KlineDataPoint>,
    ticker_info: &TickerInfo,
    frame: &mut canvas::Frame,
    palette: &Extended,
    basis: Basis,
    at_interval: Option<u64>,
    visible_range: (u64, u64),
) {
    let (visible_earliest, visible_latest) = visible_range;

    let kline_opt = match (data, at_interval) {
        (PlotData::TimeBased(timeseries), Some(at_interval)) => {
            let in_visible = at_interval >= visible_earliest && at_interval <= visible_latest;

            timeseries
                .datapoints
                .get(&UnixMs::new(at_interval))
                .map(|dp| &dp.kline)
                .or_else(|| {
                    if in_visible {
                        let search_end = at_interval.min(visible_latest);
                        timeseries
                            .datapoints
                            .range(UnixMs::new(visible_earliest)..=UnixMs::new(search_end))
                            .next_back()
                            .map(|(_, dp)| &dp.kline)
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    let right_of_latest = match basis {
                        Basis::Time(_) => at_interval > visible_latest,
                        Basis::Tick(_) => at_interval < visible_earliest,
                    };

                    if right_of_latest {
                        timeseries
                            .datapoints
                            .range(UnixMs::new(visible_earliest)..=UnixMs::new(visible_latest))
                            .next_back()
                            .map(|(_, dp)| &dp.kline)
                    } else {
                        None
                    }
                })
                .or_else(|| {
                    let (last_time, dp) = timeseries.datapoints.last_key_value()?;
                    (at_interval > last_time.as_u64()).then_some(&dp.kline)
                })
        }
        (PlotData::TickBased(tick_aggr), Some(at_interval)) => {
            let kline_at = |interval: u64| {
                let index = (interval / u64::from(tick_aggr.interval.0)) as usize;
                (index < tick_aggr.datapoints.len())
                    .then(|| &tick_aggr.datapoints[tick_aggr.datapoints.len() - 1 - index].kline)
            };

            let in_visible = at_interval >= visible_earliest && at_interval <= visible_latest;

            kline_at(at_interval).or_else(|| {
                let right_of_latest = match basis {
                    Basis::Time(_) => at_interval > visible_latest,
                    Basis::Tick(_) => at_interval < visible_earliest,
                };

                if in_visible || right_of_latest {
                    kline_at(visible_earliest)
                } else {
                    None
                }
            })
        }
        (PlotData::TimeBased(timeseries), None) => timeseries
            .datapoints
            .last_key_value()
            .map(|(_, dp)| &dp.kline),
        (PlotData::TickBased(tick_aggr), None) => tick_aggr.datapoints.last().map(|dp| &dp.kline),
    };

    if let Some(kline) = kline_opt {
        let change_pct = ((kline.close - kline.open) / kline.open * 100.0) as f32;
        let change_color = if change_pct >= 0.0 {
            palette.success.base.color
        } else {
            palette.danger.base.color
        };

        let base_color = palette.background.base.text;
        let precision = ticker_info.min_ticksize;

        let segments = [
            ("O", base_color, false),
            (&kline.open.to_string(precision), change_color, true),
            ("H", base_color, false),
            (&kline.high.to_string(precision), change_color, true),
            ("L", base_color, false),
            (&kline.low.to_string(precision), change_color, true),
            ("C", base_color, false),
            (&kline.close.to_string(precision), change_color, true),
            (&format!("{change_pct:+.2}%"), change_color, true),
        ];

        let total_width: f32 = segments
            .iter()
            .map(|(s, _, _)| s.len() as f32 * (TEXT_SIZE * 0.8))
            .sum();

        let position = Point::new(8.0, 8.0);

        let tooltip_rect = Rectangle {
            x: position.x,
            y: position.y,
            width: total_width,
            height: 16.0,
        };

        frame.fill_rectangle(
            tooltip_rect.position(),
            tooltip_rect.size(),
            palette.background.weakest.color.scale_alpha(0.9),
        );

        let mut x = position.x;
        for (text, seg_color, is_value) in segments {
            frame.fill_text(canvas::Text {
                content: text.to_string(),
                position: Point::new(x, position.y),
                size: iced::Pixels(crate::style::text_size::BODY),
                color: seg_color,
                font: style::AZERET_MONO,
                ..canvas::Text::default()
            });
            x += text.len() as f32 * 8.0;
            x += if is_value { 6.0 } else { 2.0 };
        }
    }
}

#[derive(Clone, Copy)]
enum ImbalanceSide {
    Buy,
    Sell,
}

impl ImbalanceSide {
    fn volume_bg_color(self, qty: f64, max_qty: f64, palette: &Extended) -> Color {
        const MIN_ALPHA: f32 = 0.04;

        let intensity = if max_qty > 0.0 {
            (qty / max_qty).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let alpha = MIN_ALPHA + intensity * (1.0 - MIN_ALPHA);

        match self {
            ImbalanceSide::Buy => palette.success.base.color.scale_alpha(alpha),
            ImbalanceSide::Sell => palette.danger.base.color.scale_alpha(alpha),
        }
    }

    fn volume_text_color(
        self,
        qty: f64,
        max_qty: f64,
        default_color: Color,
        palette: &Extended,
    ) -> Color {
        let cell_color = self.volume_bg_color(qty, max_qty, palette);
        let cell_background = composite_color(cell_color, palette.background.base.color);
        let inverted_color = palette.background.base.color;

        if contrast_ratio(cell_background, inverted_color)
            > contrast_ratio(cell_background, default_color)
        {
            inverted_color
        } else {
            default_color
        }
    }

    fn marker_bg_color(self, palette: &Extended, alpha: f32) -> Color {
        let accent = match self {
            ImbalanceSide::Buy => palette.success.strong.color,
            ImbalanceSide::Sell => palette.danger.strong.color,
        };
        let alpha = alpha.clamp(0.0, 1.0);

        if palette.is_dark {
            let tint = 0.28 + (alpha * 0.32);
            mix_color(accent, palette.background.strongest.color, tint)
        } else {
            let tint = 0.18 + (alpha * 0.24);
            mix_color(accent, palette.background.weak.color, tint)
        }
    }

    fn color_alpha(
        self,
        footprint: &KlineTrades,
        price: Price,
        qty: f64,
        step: PriceStep,
        threshold: usize,
        color_scale: Option<usize>,
        ignore_zeros: bool,
    ) -> Option<f32> {
        let diagonal_price = match self {
            ImbalanceSide::Buy => price.add_steps(-1, step),
            ImbalanceSide::Sell => price.add_steps(1, step),
        };
        let diagonal_qty = footprint
            .trades
            .get(&diagonal_price)
            .map(|group| match self {
                ImbalanceSide::Buy => group.sell_qty.to_f64(),
                ImbalanceSide::Sell => group.buy_qty.to_f64(),
            })
            .unwrap_or_default();

        if ignore_zeros && (qty <= 0.0 || diagonal_qty <= 0.0) {
            return None;
        }

        let required_qty = diagonal_qty * (100 + threshold) as f64 / 100.0;

        if required_qty <= 0.0 {
            return (qty > 0.0).then_some(1.0);
        }

        if qty <= required_qty {
            return None;
        }

        let ratio = qty / required_qty;
        Some(if let Some(scale) = color_scale {
            let divisor = (scale as f64 / 10.0) - 1.0;
            (0.2 + 0.8 * ((ratio - 1.0) / divisor).min(1.0)).min(1.0) as f32
        } else {
            1.0
        })
    }

    fn draw_table_marker(
        self,
        frame: &mut canvas::Frame,
        palette: &Extended,
        alpha: f32,
        qty: f64,
        max_qty: f64,
        cell: Rectangle,
    ) {
        if cell.height <= 0.0 {
            return;
        }

        let bar_width = 2.5;
        let gap = 1.5;

        let volume_intensity = if max_qty > 0.0 {
            (qty / max_qty).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let imbalance_strength = alpha.clamp(0.0, 1.0);
        let marker_alpha = 0.38 + (volume_intensity * 0.24) + (imbalance_strength * 0.38);
        let marker_alpha = marker_alpha.clamp(0.38, 1.0);

        let color = palette.warning.strong.color.scale_alpha(marker_alpha);

        let (x, bar_w) = match self {
            ImbalanceSide::Sell => (cell.x - bar_width - gap, bar_width),
            ImbalanceSide::Buy => (cell.x + cell.width + gap, bar_width),
        };

        frame.fill_rectangle(Point::new(x, cell.y), Size::new(bar_w, cell.height), color);
    }
}

#[derive(Clone, Copy, Debug)]
struct ContentGaps {
    /// Space between imb. markers candle body
    marker_to_candle: f32,
    /// Space between candle body and clusters
    candle_to_cluster: f32,
    /// Inner space reserved between imb. markers and clusters (used for BidAsk)
    marker_to_bars: f32,
}

impl ContentGaps {
    fn from_view(candle_width: f32, scaling: f32) -> Self {
        let px = |p: f32| p / scaling;
        let base = (candle_width * 0.2).max(px(2.0));
        Self {
            marker_to_candle: base,
            candle_to_cluster: base,
            marker_to_bars: px(2.0),
        }
    }
}

/// Layout and style parameters shared across footprint cell draw functions.
struct FootprintCellLayout<'a> {
    cell_w: f32,
    cell_h: f32,
    candle_w: f32,
    pal: &'a Extended,
    cluster: ClusterKind,
    gaps: ContentGaps,
    quantity_format: QuantityFormat,
}

impl FootprintCellLayout<'_> {
    /// Compute the text size for cluster labels based on on-screen cell dimensions.
    fn text_size(&self, scaling: f32) -> f32 {
        footprint_cell_text_size(self.cell_h * scaling, self.cell_w * scaling)
    }

    /// Whether cluster text labels should be drawn given current zoom level.
    fn should_show_text(&self, scaling: f32) -> bool {
        should_show_text(self.cell_h * scaling)
    }
}

struct ProfileArea {
    imb_marker_left: f32,
    imb_marker_width: f32,
    bars_left: f32,
    bars_width: f32,
    candle_center_x: f32,
}

impl ProfileArea {
    fn new(
        content_left: f32,
        content_right: f32,
        candle_width: f32,
        gaps: ContentGaps,
        has_imbalance: bool,
    ) -> Self {
        let candle_lane_left = if has_imbalance {
            content_left + candle_width + gaps.marker_to_candle
        } else {
            content_left
        };
        let candle_lane_width = candle_width * 0.25;

        let bars_left = candle_lane_left + candle_lane_width + gaps.candle_to_cluster;
        let bars_width = (content_right - bars_left).max(0.0);

        let candle_center_x = candle_lane_left + (candle_lane_width / 2.0);

        Self {
            imb_marker_left: content_left,
            imb_marker_width: if has_imbalance { candle_width } else { 0.0 },
            bars_left,
            bars_width,
            candle_center_x,
        }
    }
}

struct BidAskArea {
    bid_area_left: f32,
    bid_area_right: f32,
    ask_area_left: f32,
    ask_area_right: f32,
    candle_center_x: f32,
    imb_marker_width: f32,
}

impl BidAskArea {
    fn new(
        x_position: f32,
        content_left: f32,
        content_right: f32,
        candle_width: f32,
        spacing: ContentGaps,
    ) -> Self {
        let candle_body_width = candle_width * 0.25;

        let candle_left = x_position - (candle_body_width / 2.0);
        let candle_right = x_position + (candle_body_width / 2.0);

        let ask_area_right = candle_left - spacing.candle_to_cluster;
        let bid_area_left = candle_right + spacing.candle_to_cluster;

        Self {
            bid_area_left,
            bid_area_right: content_right,
            ask_area_left: content_left,
            ask_area_right,
            candle_center_x: x_position,
            imb_marker_width: candle_width,
        }
    }
}

struct TableLayout {
    table_left: f32,
    table_right: f32,
    candle_center_x: f32,
}

impl TableLayout {
    fn new(
        content_left: f32,
        content_right: f32,
        candle_width: f32,
        spacing: ContentGaps,
        has_imbalance: bool,
    ) -> Self {
        let (candle_center_x, table_left) = if has_imbalance {
            let ccx = content_left + candle_width / 2.0;
            let tl = (content_left + candle_width + spacing.candle_to_cluster).min(content_right);
            (ccx, tl)
        } else {
            let thin_candle = candle_width * 0.25;
            let ccx = content_left + thin_candle / 2.0;
            let tl = (content_left + thin_candle + spacing.candle_to_cluster).min(content_right);
            (ccx, tl)
        };

        Self {
            table_left,
            table_right: content_right,
            candle_center_x,
        }
    }
}

struct TableArea {
    table_left: f32,
    table_right: f32,
}

impl TableArea {
    fn new(
        frame: &mut canvas::Frame,
        price_to_y: &impl Fn(Price) -> f32,
        table_layout: &TableLayout,
        candle_width: f32,
        kline: &Kline,
        palette: &Extended,
    ) -> Self {
        draw_footprint_kline(
            frame,
            price_to_y,
            table_layout.candle_center_x,
            candle_width,
            kline,
            palette,
        );

        Self {
            table_left: table_layout.table_left,
            table_right: table_layout.table_right,
        }
    }

    fn width(&self) -> f32 {
        (self.table_right - self.table_left).max(0.0)
    }
}

#[inline]
pub(crate) fn footprint_cluster_text_size(cell_height_unscaled: f32) -> f32 {
    cell_height_unscaled.round().min(16.0) - 3.0
}

fn footprint_cell_text_size(cell_height_unscaled: f32, cell_width_unscaled: f32) -> f32 {
    let base = footprint_cluster_text_size(cell_height_unscaled);
    let width_size = (cell_width_unscaled * 0.12).round().min(18.0);
    base.max(width_size)
        .min(cell_height_unscaled.round().min(18.0).max(base))
}

pub(crate) fn footprint_text_fits(text: &str, text_size: f32, available_width: f32) -> bool {
    text.chars().count() as f32 * text_size * 0.62 <= available_width
}

struct FootprintSummaryLayout {
    text_size: f32,
    gap: f32,
    line_gap: f32,
}

impl FootprintSummaryLayout {
    /// Computes the text size, gap, and line gap for footprint summary text.
    /// Scales the font down when the on-screen cell height is too small.
    fn new(cell_height: f32, scaling: f32) -> FootprintSummaryLayout {
        const MIN_SCREEN_CELL_H_PX: f32 = 6.0;
        const MIN_TEXT_SIZE_PX: f32 = 3.0;
        const SUMMARY_GAP_PX: f32 = 8.0;
        const SUMMARY_LINE_GAP_PX: f32 = 2.0;

        let max_text_size = style::text_size::TINY;
        let screen_cell_h = cell_height * scaling;
        let text_size = if screen_cell_h < MIN_SCREEN_CELL_H_PX {
            (max_text_size * (screen_cell_h / MIN_SCREEN_CELL_H_PX)).max(MIN_TEXT_SIZE_PX)
        } else {
            max_text_size
        };

        let gap = SUMMARY_GAP_PX / scaling;
        let line_gap = SUMMARY_LINE_GAP_PX / scaling;

        FootprintSummaryLayout {
            text_size,
            gap,
            line_gap,
        }
    }

    fn padding(cell_height: f32, scaling: f32, tick_size: f32) -> f32 {
        if cell_height <= f32::EPSILON {
            return 0.0;
        }

        let layout = Self::new(cell_height, scaling);

        let first_line_bottom = layout.gap + layout.text_size;
        let second_line_bottom = first_line_bottom + layout.line_gap + layout.text_size;

        let summary_ticks = second_line_bottom / cell_height;
        summary_ticks * tick_size
    }
}

#[inline]
fn price_padding_from_pixels(cell_height: f32, tick_size: f32) -> f32 {
    const OUTER_BOUND_PADDING_PX: f32 = 4.0;

    if cell_height <= f32::EPSILON {
        return 0.0;
    }

    (OUTER_BOUND_PADDING_PX / cell_height) * tick_size
}
#[inline]
fn should_show_text(cell_height_unscaled: f32) -> bool {
    cell_height_unscaled > 8.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn market_latency_is_fixed_at_event_arrival() {
        assert_eq!(market_latency_ms(1_050, 1_000), 50);
        assert_eq!(market_latency_ms(900, 1_000), 0);

        let latency = smooth_market_latency(None, 100);
        assert_eq!(smooth_market_latency(latency, 0), latency);

        let latency = smooth_market_latency(latency, 200);
        assert_eq!(latency, Some(110.0));
        assert_eq!(displayed_market_latency(latency), Some(110));
        assert_eq!(displayed_market_latency(Some(112.0)), Some(110));
    }

    #[test]
    fn open_interest_requests_the_loaded_history() {
        assert_eq!(
            missing_open_interest_range(&BTreeMap::new(), UnixMs::new(1_000), UnixMs::new(4_000),),
            Some(FetchRange::OpenInterest(
                UnixMs::new(1_000),
                UnixMs::new(4_000)
            ))
        );

        let mut partial = BTreeMap::from([(UnixMs::new(2_000), 1.0), (UnixMs::new(3_000), 1.0)]);
        assert_eq!(
            missing_open_interest_range(&partial, UnixMs::new(1_000), UnixMs::new(4_000)),
            Some(FetchRange::OpenInterest(
                UnixMs::new(1_000),
                UnixMs::new(2_000)
            ))
        );
        partial.insert(UnixMs::new(1_000), 1.0);
        assert_eq!(
            missing_open_interest_range(&partial, UnixMs::new(1_000), UnixMs::new(4_000)),
            Some(FetchRange::OpenInterest(
                UnixMs::new(3_000),
                UnixMs::new(4_000)
            ))
        );
    }

    #[test]
    fn footprint_orderbook_reserves_right_margin_only_when_enabled() {
        let kind = KlineChartKind::Footprint {
            clusters: ClusterKind::default(),
            scaling: ClusterScaling::default(),
            studies: vec![],
        };
        let mut config = Config::default();
        assert_eq!(footprint_orderbook_margin(&kind, &config, 1.0), 0.0);

        config.show_footprint_orderbook = true;
        assert_eq!(
            footprint_orderbook_margin(&kind, &config, 0.5),
            FOOTPRINT_ORDERBOOK_WIDTH * 2.0
        );
    }

    #[test]
    fn footprint_text_size_grows_with_available_cell_width() {
        let compact = footprint_cell_text_size(12.0, 80.0);
        let expanded = footprint_cell_text_size(12.0, 120.0);
        assert_eq!(compact, 10.0);
        assert_eq!(expanded, 12.0);
        assert!(expanded > compact);
        assert!(footprint_text_fits("123.45", 9.0, 34.0));
        assert!(!footprint_text_fits("123.45", 9.0, 33.0));
    }

    #[test]
    fn moving_a_drawing_preserves_its_shape() {
        let drawing = Drawing {
            tool: DrawingTool::TrendLine,
            start: DrawingAnchor {
                interval: 100,
                price: 10.0,
            },
            end: DrawingAnchor {
                interval: 200,
                price: 20.0,
            },
        };
        let moved = moved_drawing(
            drawing,
            DrawingPart::Body,
            DrawingAnchor {
                interval: 50,
                price: 5.0,
            },
            DrawingAnchor {
                interval: 75,
                price: 8.0,
            },
        );

        assert_eq!(moved.start.interval, 125);
        assert_eq!(moved.end.interval, 225);
        assert_eq!(moved.start.price, 13.0);
        assert_eq!(moved.end.price, 23.0);
    }

    #[test]
    fn drawing_context_menu_separates_delete_and_clear_all() {
        let bounds = Rectangle::new(Point::ORIGIN, Size::new(300.0, 200.0));
        let menu = DrawingContextMenu {
            index: Some(0),
            position: Point::new(250.0, 190.0),
        };

        assert!(drawing_context_menu_item_hit(
            menu,
            bounds,
            Point::new(200.0, 150.0),
            DrawingMenuItem::Delete,
        ));
        assert!(drawing_context_menu_item_hit(
            menu,
            bounds,
            Point::new(200.0, 180.0),
            DrawingMenuItem::ClearAll,
        ));
    }

    #[test]
    fn drawing_coordinates_use_the_canvas_local_center() {
        let bounds = Rectangle::new(Point::new(80.0, 40.0), Size::new(300.0, 200.0));

        assert_eq!(canvas_local_center(bounds), Point::new(150.0, 100.0));
    }

    #[test]
    fn trading_labels_keep_a_readable_gap() {
        let labels = spread_trading_label_positions(&[50.0, 51.0, 52.0], 0.0, 100.0, 14.0);
        assert!(labels.windows(2).all(|pair| pair[1] - pair[0] >= 14.0));
        assert!(labels[0] >= 0.0 && labels[2] <= 100.0);
    }

    #[test]
    fn trading_labels_ignore_levels_outside_the_chart() {
        assert!(!trading_level_is_visible(-1.0, 0.0, 100.0, 7.0));
        assert!(trading_level_is_visible(50.0, 0.0, 100.0, 7.0));
        assert!(!trading_level_is_visible(101.0, 0.0, 100.0, 7.0));
    }
}
