use super::indicators::{
    IndicatorHost, IndicatorInputCoverage, IndicatorInputSource, OpenInterestSeries,
};
use super::series::KlineSeries;
use super::{
    IndicatorInstanceId, IndicatorInstanceStore, IndicatorUpdateScope, KlineIndexChange,
    KlineStudy, KlineUpdate, LineStrokeStyle, MIN_LINE_WIDTH, SeriesColor, StudyDataSource,
    ThemeAccent,
};
use crate::chart::Basis;
use exchange::{Kline, TickMultiplier, TickerInfo, Timeframe, Trade, UnixMs, unit::PriceStep};
use indicator_engine::IndicatorFrame;
use rustc_hash::FxHashMap;
use std::sync::Arc;

pub struct LiveIndicatorUpdate {
    pub committed_bar: bool,
    pub rebuild_scope: Option<IndicatorUpdateScope>,
    pub closed_trade_correction: bool,
    pub coverage_frames: Vec<IndicatorFrame>,
    pub index_change: KlineIndexChange,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoricalInputUpdate {
    Rejected,
    NoData { has_consumer: bool },
    Ignored,
    Applied,
}

impl HistoricalInputUpdate {
    pub fn needs_rebuild(self) -> bool {
        matches!(self, Self::NoData { has_consumer: true } | Self::Applied)
    }
}

pub struct KlineData {
    series: Vec<KlineSeries>,
    indicator_host: IndicatorHost,
    base_ticker: TickerInfo,
    series_index: FxHashMap<TickerInfo, usize>,
    tick_multiplier: TickMultiplier,
    /// Pro: other venues whose trades merge into the base series' order flow.
    aggregate_sources: Vec<TickerInfo>,
    /// Pro: shift source prices by their estimated spread to the base venue.
    aggregate_basis_adjust: bool,
    /// Pro: per-source EMA of (source price - base price).
    aggregate_basis: FxHashMap<TickerInfo, f64>,
}

/// EMA weight of each trade batch's median spread.
const BASIS_ALPHA: f64 = 0.2;

impl KlineData {
    pub fn new(base_ticker: TickerInfo, tick_multiplier: TickMultiplier) -> Self {
        Self {
            series: Vec::new(),
            indicator_host: IndicatorHost::default(),
            base_ticker,
            series_index: FxHashMap::default(),
            tick_multiplier,
            aggregate_sources: Vec::new(),
            aggregate_basis_adjust: false,
            aggregate_basis: FxHashMap::default(),
        }
    }

    pub fn aggregate_sources(&self) -> &[TickerInfo] {
        &self.aggregate_sources
    }

    /// Returns `true` when the source set or the basis mode changed.
    pub fn set_aggregate_sources(&mut self, sources: Vec<TickerInfo>, basis_adjust: bool) -> bool {
        // The basis mode only matters (and only forces a reload) with sources present.
        let changed = self.aggregate_sources != sources
            || (!sources.is_empty() && self.aggregate_basis_adjust != basis_adjust);
        if changed {
            self.aggregate_basis.clear();
        }
        self.aggregate_sources = sources;
        self.aggregate_basis_adjust = basis_adjust;
        changed
    }

    /// Base price a source trade at `time` should be compared with: the live close
    /// for the forming bar, else the bar's typical price.
    fn base_reference(&self, time: UnixMs) -> Option<f64> {
        let series = self.base_series()?;
        let end = series.time_index_range(UnixMs(0), time).end;
        let point = series.point_at(end.checked_sub(1)?)?;
        let k = point.kline;
        let latest = series
            .latest_point()
            .is_some_and(|l| l.index == point.index);
        Some(if latest {
            k.close.to_f64()
        } else {
            (k.high.to_f64() + k.low.to_f64() + k.close.to_f64()) / 3.0
        })
    }

    /// Updates the source's spread estimate from this batch and returns it.
    // ponytail: spread vs base bar prices, not time-matched base trades; lags a fast basis move by a few batches.
    fn update_basis(&mut self, source: TickerInfo, trades: &[Trade]) -> f64 {
        let mut diffs = trades
            .iter()
            .filter_map(|t| Some(t.price.to_f64() - self.base_reference(t.time)?))
            .collect::<Vec<_>>();
        let prev = self.aggregate_basis.get(&source).copied();
        if diffs.is_empty() {
            return prev.unwrap_or(0.0);
        }
        diffs.sort_by(f64::total_cmp);
        let median = diffs[diffs.len() / 2];
        let basis = prev.map_or(median, |prev| prev + (median - prev) * BASIS_ALPHA);
        self.aggregate_basis.insert(source, basis);
        basis
    }

    /// Trades from an aggregate source, re-targeted at the base series.
    /// Trades priced outside 0.5x..2x of the base close are dropped: that
    /// catches unit mismatches such as `1000PEPE` vs `PEPE`.
    pub fn aggregate_source_trades(
        &mut self,
        ticker_info: &TickerInfo,
        trades: &[Trade],
    ) -> Option<(TickerInfo, Vec<Trade>)> {
        if self.series_index.contains_key(ticker_info)
            || !self.aggregate_sources.contains(ticker_info)
        {
            return None;
        }
        let close = self
            .base_series()
            .and_then(|series| series.latest_point())
            .map(|point| point.kline.close.to_f64())
            .filter(|close| *close > 0.0);
        let mut trades = close.map_or_else(Vec::new, |close| {
            trades
                .iter()
                .filter(|trade| (0.5..=2.0).contains(&(trade.price.to_f64() / close)))
                .copied()
                .collect::<Vec<_>>()
        });
        if self.aggregate_basis_adjust && !trades.is_empty() {
            let basis = self.update_basis(*ticker_info, &trades);
            for trade in &mut trades {
                trade.price = exchange::unit::Price::from_f64(trade.price.to_f64() - basis);
            }
        }
        Some((self.base_ticker, trades))
    }

    pub fn base_ticker(&self) -> TickerInfo {
        self.base_ticker
    }

    pub fn selected_tickers(&self) -> Vec<TickerInfo> {
        self.series
            .iter()
            .map(|series| series.ticker_info)
            .collect()
    }

    pub fn series(&self) -> &[KlineSeries] {
        &self.series
    }

    pub fn indicator_host(&self) -> &IndicatorHost {
        &self.indicator_host
    }

    pub fn merge_indicator_host(
        &mut self,
        rebuilt: IndicatorHost,
        replaced_ids: &[IndicatorInstanceId],
        active_ids: &[IndicatorInstanceId],
        indicators: &IndicatorInstanceStore,
    ) {
        self.indicator_host
            .merge_rebuilt(rebuilt, replaced_ids, active_ids, indicators);
    }

    pub fn remap_indicator_outputs(&mut self, change: KlineIndexChange) {
        match change {
            KlineIndexChange::None => {}
            KlineIndexChange::Prefix(prefix) => self.indicator_host.remap_outputs_prefix(prefix),
            KlineIndexChange::Explicit(previous_indices) => {
                self.indicator_host.remap_outputs(&previous_indices)
            }
        }
    }

    pub fn dependent_indicator_ids(
        &self,
        roots: &[IndicatorInstanceId],
    ) -> Vec<IndicatorInstanceId> {
        self.indicator_host.dependent_ids(roots)
    }

    pub fn calculation_indicator_ids(
        &self,
        roots: &[IndicatorInstanceId],
    ) -> Vec<IndicatorInstanceId> {
        self.indicator_host.calculation_ids(roots)
    }

    pub fn has_indicator_entries(&self, ids: &[IndicatorInstanceId]) -> bool {
        self.indicator_host.has_entries(ids)
    }

    pub fn tick_multiplier(&self) -> TickMultiplier {
        self.tick_multiplier
    }

    pub fn base_series(&self) -> Option<&KlineSeries> {
        let idx = self.series_index.get(&self.base_ticker).copied()?;
        self.series.get(idx)
    }

    fn base_series_mut(&mut self) -> Option<&mut KlineSeries> {
        let idx = self.series_index.get(&self.base_ticker).copied()?;
        self.series.get_mut(idx)
    }

    pub fn configure_recurring_profile_intervals(&mut self, intervals: &[(Timeframe, PriceStep)]) {
        if let Some(base_series) = self.base_series_mut() {
            base_series.configure_recurring_profile_intervals(intervals);
        }
    }

    pub fn prepare_trade_price_distribution(&mut self, earliest: UnixMs, latest: UnixMs) {
        let tick_size = self.tick_size();
        if let Some(series) = self.base_series_mut() {
            series.prepare_trade_price_distribution(earliest, latest, tick_size);
        }
    }

    pub fn configure_big_trades_threshold(&mut self, threshold: Option<u64>) {
        if let Some(base_series) = self.base_series_mut() {
            base_series.configure_big_trades_threshold(threshold);
        }
    }

    pub fn set_tick_multiplier(&mut self, multiplier: TickMultiplier) -> bool {
        if self.tick_multiplier == multiplier {
            return false;
        }
        self.tick_multiplier = multiplier;
        true
    }

    pub fn tick_size(&self) -> PriceStep {
        self.tick_multiplier
            .multiply_with_min_tick_step(self.base_ticker)
    }

    pub fn set_series_color(&mut self, ticker_info: TickerInfo, color: SeriesColor) -> bool {
        let Some(idx) = self.series_index.get(&ticker_info).copied() else {
            return false;
        };

        self.series[idx].set_series_color(color)
    }

    pub fn set_series_line_width(&mut self, ticker_info: TickerInfo, width: u8) -> bool {
        let Some(idx) = self.series_index.get(&ticker_info).copied() else {
            return false;
        };

        self.series[idx].set_line_width(width)
    }

    pub fn set_series_stroke_style(
        &mut self,
        ticker_info: TickerInfo,
        stroke_style: LineStrokeStyle,
    ) -> bool {
        let Some(idx) = self.series_index.get(&ticker_info).copied() else {
            return false;
        };

        self.series[idx].set_stroke_style(stroke_style)
    }

    pub fn reset_series_style(&mut self, ticker_info: TickerInfo) -> bool {
        let Some(index) = self.series_index.get(&ticker_info).copied() else {
            return false;
        };

        let color = self.default_series_color(index);
        let series = &mut self.series[index];
        let color_changed = series.set_series_color(color);
        let width_changed = series.set_line_width(MIN_LINE_WIDTH);
        let stroke_changed = series.set_stroke_style(LineStrokeStyle::Solid);

        color_changed || width_changed || stroke_changed
    }

    fn default_series_color(&self, index: usize) -> SeriesColor {
        if self.series[index].ticker_info == self.base_ticker {
            return SeriesColor::default();
        }

        SeriesColor::with_accent(self.next_unused_accent(Some(index)))
    }

    pub fn add_ticker(&mut self, ticker_info: TickerInfo, basis: Basis) -> bool {
        if self.series_index.contains_key(&ticker_info) {
            return false;
        }

        let idx = self.series.len();
        let tick_size = self
            .tick_multiplier
            .multiply_with_min_tick_step(ticker_info);
        let color = if ticker_info == self.base_ticker {
            SeriesColor::default()
        } else {
            SeriesColor::with_accent(self.next_unused_accent(None))
        };
        let series = KlineSeries::new_with_basis(ticker_info, color, basis, tick_size);

        self.series.push(series);
        self.series_index.insert(ticker_info, idx);
        true
    }

    /// The first accent no series uses, ignoring `excluding` when a single series is
    /// being recolored.
    fn next_unused_accent(&self, excluding: Option<usize>) -> ThemeAccent {
        let is_used = |accent: ThemeAccent| {
            self.series.iter().enumerate().any(|(index, series)| {
                Some(index) != excluding && series.color().accent_value() == Some(accent)
            })
        };

        ThemeAccent::ALL
            .into_iter()
            .find(|accent| !is_used(*accent))
            .unwrap_or(ThemeAccent::One)
    }

    pub fn remove_ticker(&mut self, ticker_info: TickerInfo) -> bool {
        let Some(idx) = self.series_index.remove(&ticker_info) else {
            return false;
        };

        self.series.remove(idx);
        self.rebuild_series_index();
        true
    }

    fn rebuild_series_index(&mut self) {
        self.series_index.clear();
        for (idx, series) in self.series.iter().enumerate() {
            self.series_index.insert(series.ticker_info, idx);
        }
    }

    pub fn reset_for_basis(&mut self, basis: Basis, preserve_raw_trades: bool) {
        for series in &mut self.series {
            series.set_aggregation_basis(basis, preserve_raw_trades);
        }
    }

    pub fn configure_series_for_basis(&mut self, basis: Basis) {
        for series in &mut self.series {
            let tick_size = self
                .tick_multiplier
                .multiply_with_min_tick_step(series.ticker_info);
            if series.basis() != basis {
                series.set_aggregation_basis(basis, true);
            }
            series.set_tick_size(tick_size);
        }
    }

    pub fn configure_indicators(
        &mut self,
        indicators: &IndicatorInstanceStore,
        active_indicators: &[IndicatorInstanceId],
    ) -> Vec<IndicatorInstanceId> {
        self.indicator_host
            .set_active(active_indicators, indicators)
    }

    pub fn can_recalculate_incrementally(
        &self,
        ids: &[IndicatorInstanceId],
        scope: IndicatorUpdateScope,
    ) -> bool {
        let IndicatorUpdateScope::From(start) = scope else {
            return false;
        };
        self.indicator_host.can_append_from(ids, start)
            && self.base_series().is_none_or(|series| {
                series
                    .committed_points()
                    .filter(|point| point.index >= start)
                    .take(2)
                    .count()
                    <= 1
            })
    }

    pub fn append_indicator_frames(
        &mut self,
        indicator_ids: &[IndicatorInstanceId],
        start: usize,
        trade_data_enabled: bool,
    ) -> Arc<[IndicatorFrame]> {
        let frames = self.evaluate_closed_append_from(start, indicator_ids, trade_data_enabled);
        self.preview_indicator_open_for(indicator_ids, trade_data_enabled);
        frames.into()
    }

    pub fn committed_indicator_frames(&self, trade_data_enabled: bool) -> Arc<[IndicatorFrame]> {
        let Some(base_series) = self.base_series() else {
            return Arc::from([]);
        };
        let frames = base_series
            .committed_points()
            .map(|point| self.indicator_frame(point.index, point.kline.time, trade_data_enabled))
            .collect::<Vec<_>>();
        frames.into()
    }

    pub fn latest_indicator_frame(&self, trade_data_enabled: bool) -> Option<IndicatorFrame> {
        let base_series = self.base_series()?;
        let point = base_series.latest_point()?;
        Some(self.indicator_frame(point.index, point.kline.time, trade_data_enabled))
    }

    pub fn indicator_input_coverage_for_frames(
        &self,
        committed_frames: &[IndicatorFrame],
        trade_data_enabled: bool,
    ) -> IndicatorInputCoverage {
        let mut frames = committed_frames.to_vec();
        if let Some(latest) = self.latest_indicator_frame(trade_data_enabled) {
            if let Some(existing) = frames.iter_mut().find(|frame| frame.index == latest.index) {
                *existing = latest;
            } else {
                frames.push(latest);
            }
        }
        IndicatorInputCoverage::from_frames(&frames)
    }

    pub fn preview_indicator_open_for(
        &mut self,
        ids: &[IndicatorInstanceId],
        trade_data_enabled: bool,
    ) -> bool {
        let Some(frame) = self.latest_indicator_frame(trade_data_enabled) else {
            return false;
        };
        self.indicator_host.preview_open_selected(ids, &frame);
        true
    }

    fn evaluate_closed_previous_frame(
        &mut self,
        time: Option<exchange::UnixMs>,
        indicator_ids: &[IndicatorInstanceId],
        trade_data_enabled: bool,
    ) -> Option<IndicatorFrame> {
        let time = time?;
        let base_series = self.base_series()?;

        let committed_len = base_series.point_count().saturating_sub(1);
        let point = base_series
            .points()
            .take(committed_len)
            .next_back()
            .filter(|point| point.kline.time == time)?;

        let frame = self.indicator_frame(point.index, point.kline.time, trade_data_enabled);

        self.indicator_host
            .evaluate_closed_selected(indicator_ids, &frame);
        Some(frame)
    }

    fn evaluate_closed_append_from(
        &mut self,
        start: usize,
        indicator_ids: &[IndicatorInstanceId],
        trade_data_enabled: bool,
    ) -> Vec<IndicatorFrame> {
        let points = self
            .base_series()
            .map(|series| {
                series
                    .committed_points()
                    .skip(start)
                    .map(|point| (point.index, point.kline.time))
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let mut frames = Vec::with_capacity(points.len());
        for (index, time) in points {
            let frame = self.indicator_frame(index, time, trade_data_enabled);
            self.indicator_host
                .evaluate_closed_selected(indicator_ids, &frame);
            frames.push(frame);
        }
        frames
    }

    fn indicator_frame(
        &self,
        index: usize,
        time: exchange::UnixMs,
        trade_data_enabled: bool,
    ) -> IndicatorFrame {
        let base_series = self
            .base_series()
            .expect("indicator frame requires a base series");
        // Open interest is an as-of input; plot offsets are presentation metadata.
        let open_interest = base_series
            .open_interest()
            .and_then(|series| series.latest_at_or_before(time));
        let mut frame = base_series.indicator_frame(index, time, open_interest, trade_data_enabled);
        if !self.aggregate_sources.is_empty()
            && let Some(trade) = frame.chart.trade
        {
            frame.chart.buy_qty = trade.buy_qty;
            frame.chart.sell_qty = trade.sell_qty;
            frame.chart.volume = Some(trade.buy_qty + trade.sell_qty);
            frame.chart.directional_volume_available = true;
        }
        frame
    }

    fn closed_trade_correction_start(
        &self,
        trades: &[Trade],
        timeframe: Timeframe,
    ) -> Option<usize> {
        let earliest_trade_bar = trades
            .iter()
            .map(|trade| trade.time.floor_to(timeframe))
            .min()?;
        let base_series = self.base_series()?;
        let latest_closed_time = base_series
            .points()
            .rev()
            .nth(1)
            .map(|point| point.kline.time)?;
        if earliest_trade_bar > latest_closed_time {
            return None;
        }

        Some(
            base_series
                .committed_points()
                .find(|point| point.kline.time >= earliest_trade_bar)
                .map_or(0, |point| point.index),
        )
    }

    pub fn has_closed_trade_correction(&self, trades: &[Trade], basis: Basis) -> bool {
        match basis {
            Basis::Time(timeframe) => self
                .closed_trade_correction_start(trades, timeframe)
                .is_some(),
            Basis::Tick(_) => false,
        }
    }

    fn committed_indicator_frames_from(
        &self,
        start: usize,
        trade_data_enabled: bool,
    ) -> Vec<IndicatorFrame> {
        self.base_series()
            .map(|series| {
                series
                    .committed_points()
                    .skip(start)
                    .map(|point| {
                        self.indicator_frame(point.index, point.kline.time, trade_data_enabled)
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    pub fn insert_historical_klines(
        &mut self,
        ticker_info: TickerInfo,
        klines: &[Kline],
        basis: Basis,
    ) -> Option<KlineUpdate> {
        let idx = self.series_index.get(&ticker_info).copied()?;

        if klines.is_empty() {
            return None;
        }
        self.series[idx].insert_klines(klines, basis)
    }

    pub fn update_live_kline(
        &mut self,
        ticker_info: &TickerInfo,
        kline: &Kline,
        basis: Basis,
        indicator_ids: &[IndicatorInstanceId],
        trade_data_enabled: bool,
    ) -> Option<LiveIndicatorUpdate> {
        let idx = self.series_index.get(ticker_info).copied()?;
        let previous_time = self.series[idx]
            .latest_point()
            .map(|point| point.kline.time);
        let next_time = kline.time;
        let is_base = *ticker_info == self.base_ticker;
        let starts_new_bar = previous_time.is_some_and(|time| next_time > time);
        let update = self.series[idx].update_latest_kline(kline, basis)?;
        let rebuild_scope = (is_base
            && update.scope.is_full()
            && previous_time.is_some_and(|time| next_time < time))
        .then_some(IndicatorUpdateScope::Full);
        let coverage_frames = if starts_new_bar && is_base {
            let committed_frame = self.evaluate_closed_previous_frame(
                previous_time,
                indicator_ids,
                trade_data_enabled,
            );
            self.preview_indicator_open_for(indicator_ids, trade_data_enabled);
            committed_frame.into_iter().collect()
        } else {
            Vec::new()
        };
        Some(LiveIndicatorUpdate {
            committed_bar: starts_new_bar && is_base,
            rebuild_scope,
            closed_trade_correction: false,
            coverage_frames,
            index_change: update.index_change,
        })
    }

    pub fn insert_live_trades(
        &mut self,
        ticker_info: &TickerInfo,
        trades: &[Trade],
        basis: Basis,
        indicator_ids: &[IndicatorInstanceId],
        trade_data_enabled: bool,
    ) -> Option<LiveIndicatorUpdate> {
        if trades.is_empty() {
            return None;
        }
        let index = self.series_index.get(ticker_info).copied()?;

        let is_base = *ticker_info == self.base_ticker;
        let correction_start = is_base
            .then(|| match basis {
                Basis::Time(timeframe) => self.closed_trade_correction_start(trades, timeframe),
                Basis::Tick(_) => None,
            })
            .flatten();
        let previous_committed_len = if is_base {
            self.base_series()
                .map(|series| series.point_count().saturating_sub(1))
                .unwrap_or_default()
        } else {
            0
        };
        let scope = match basis {
            Basis::Tick(_) => self.series[index].insert_tick_trades(trades),
            Basis::Time(_) => self.series[index].insert_time_trades(trades),
        };
        let committed_len = if matches!(basis, Basis::Tick(_)) && is_base {
            self.base_series()
                .map(|series| series.point_count().saturating_sub(1))
                .unwrap_or_default()
        } else {
            previous_committed_len
        };
        let deferred_append = committed_len.saturating_sub(previous_committed_len) > 1;
        let newly_committed_frames = if deferred_append {
            self.committed_indicator_frames_from(previous_committed_len, trade_data_enabled)
        } else if committed_len > previous_committed_len {
            let frames = self.evaluate_closed_append_from(
                previous_committed_len,
                indicator_ids,
                trade_data_enabled,
            );
            self.preview_indicator_open_for(indicator_ids, trade_data_enabled);
            frames
        } else {
            Vec::new()
        };
        let coverage_frames = correction_start
            .map(|start| self.committed_indicator_frames_from(start, trade_data_enabled))
            .unwrap_or(newly_committed_frames);
        Some(LiveIndicatorUpdate {
            committed_bar: committed_len > previous_committed_len,
            rebuild_scope: if is_base && scope.is_full() {
                Some(IndicatorUpdateScope::Full)
            } else if deferred_append {
                Some(IndicatorUpdateScope::From(previous_committed_len))
            } else {
                None
            },
            closed_trade_correction: correction_start.is_some(),
            coverage_frames,
            index_change: KlineIndexChange::None,
        })
    }

    pub fn insert_historical_trades(
        &mut self,
        ticker_info: TickerInfo,
        trades: &[Trade],
        is_batches_done: bool,
        basis: Basis,
    ) -> bool {
        let Basis::Time(_) = basis else {
            return false;
        };

        let Some(series) = self
            .series_index
            .get(&ticker_info)
            .copied()
            .and_then(|index| self.series.get_mut(index))
        else {
            return false;
        };

        if !trades.is_empty() {
            series.insert_time_trades(trades);
        }
        !trades.is_empty() || is_batches_done
    }

    pub fn insert_historical_open_interest(
        &mut self,
        ticker_info: TickerInfo,
        series: &OpenInterestSeries,
        indicators: &IndicatorInstanceStore,
    ) -> HistoricalInputUpdate {
        if ticker_info != self.base_ticker {
            return HistoricalInputUpdate::Rejected;
        }
        let Some(idx) = self.series_index.get(&ticker_info).copied() else {
            return HistoricalInputUpdate::Rejected;
        };

        let has_open_interest_consumer = self
            .indicator_host
            .has_input_consumer(IndicatorInputSource::OpenInterest, indicators);
        if series.is_empty() {
            return HistoricalInputUpdate::NoData {
                has_consumer: has_open_interest_consumer,
            };
        }
        // AI snapshots also consume OI when no plotted indicator requires it.
        self.series[idx].insert_open_interest(series);
        HistoricalInputUpdate::Applied
    }

    pub fn recurring_profile_cache_requirements(
        &self,
        studies: impl IntoIterator<Item = KlineStudy>,
        basis: Basis,
    ) -> Vec<(Timeframe, PriceStep)> {
        let tick_size = self.tick_size();
        let mut recurring_intervals = studies
            .into_iter()
            .filter(|study| study.source() == StudyDataSource::Trades)
            .filter_map(|study| {
                study
                    .recurring_profile_interval()
                    .filter(|_| study.is_interval_available(basis))
                    .map(|interval| (interval, tick_size))
            })
            .collect::<Vec<_>>();
        recurring_intervals.sort_unstable();
        recurring_intervals.dedup();
        recurring_intervals
    }
}

#[cfg(test)]
mod aggregate_tests {
    use super::KlineData;
    use crate::chart::Basis;
    use exchange::adapter::Exchange;
    use exchange::unit::{Price, Qty};
    use exchange::{Kline, TickMultiplier, Ticker, TickerInfo, Timeframe, Trade, Volume};

    #[test]
    fn source_trades_retarget_to_base_and_drop_mismatched_prices() {
        let base = TickerInfo::new(
            Ticker::new("BTCUSDT", Exchange::BinanceLinear),
            0.1,
            0.001,
            None,
        );
        let source = TickerInfo::new(
            Ticker::new("BTC-USDT-SWAP", Exchange::OkexLinear),
            0.1,
            0.01,
            Some(0.01),
        );
        let basis = Basis::Time(Timeframe::M1);
        let mut data = KlineData::new(base, TickMultiplier(1));
        data.add_ticker(base, basis);
        let kline = Kline::new(
            0u64,
            100.0,
            100.0,
            100.0,
            100.0,
            Volume::TotalOnly(Qty::from_f64(1.0)),
            base.min_ticksize,
        );
        data.insert_historical_klines(base, &[kline], basis);
        let trade = |price| Trade {
            time: 1u64.into(),
            is_sell: false,
            price: Price::from_f64(price),
            qty: Qty::from_f64(1.0),
        };

        assert!(
            data.aggregate_source_trades(&source, &[trade(100.0)])
                .is_none()
        );
        data.set_aggregate_sources(vec![source], false);
        let (target, trades) = data
            .aggregate_source_trades(&source, &[trade(101.0), trade(100_000.0)])
            .expect("source trades are re-targeted");
        assert_eq!(target, base);
        assert_eq!(trades.len(), 1);
        assert!(
            data.aggregate_source_trades(&base, &[trade(100.0)])
                .is_none()
        );
    }

    #[test]
    fn basis_adjust_shifts_source_prices_onto_base() {
        let base = TickerInfo::new(
            Ticker::new("BTCUSDT", Exchange::BinanceLinear),
            0.1,
            0.001,
            None,
        );
        let source = TickerInfo::new(
            Ticker::new("BTCUSDT", Exchange::BinanceSpot),
            0.1,
            0.001,
            None,
        );
        let basis = Basis::Time(Timeframe::M1);
        let mut data = KlineData::new(base, TickMultiplier(1));
        data.add_ticker(base, basis);
        let kline = Kline::new(
            0u64,
            100.0,
            100.0,
            100.0,
            100.0,
            Volume::TotalOnly(Qty::from_f64(1.0)),
            base.min_ticksize,
        );
        data.insert_historical_klines(base, &[kline], basis);
        let trade = |price| Trade {
            time: 1u64.into(),
            is_sell: false,
            price: Price::from_f64(price),
            qty: Qty::from_f64(1.0),
        };

        // Source trades 5 above the base close; adjusted, they land on the base price.
        data.set_aggregate_sources(vec![source], true);
        let (_, trades) = data
            .aggregate_source_trades(&source, &[trade(104.0), trade(105.0), trade(106.0)])
            .expect("source trades are re-targeted");
        assert_eq!(trades[1].price, Price::from_f64(100.0));

        // Off: raw prices pass through.
        data.set_aggregate_sources(vec![source], false);
        let (_, trades) = data
            .aggregate_source_trades(&source, &[trade(105.0)])
            .unwrap();
        assert_eq!(trades[0].price, Price::from_f64(105.0));
    }
}
