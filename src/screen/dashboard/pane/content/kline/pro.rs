use super::{
    KlineChart,
    drawing::{DrawingMessage, Event},
};
use crate::ai::{
    ChartAnnotation, DepthLevel, LiquidationSummary, MarketBar, MarketFeatures, MarketSnapshot,
};
use data::chart::Basis;
use exchange::{Liquidation, depth::Depth, unit::Price};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct ProState {
    pub(super) annotations: Vec<ChartAnnotation>,
    pub(super) trading: Option<crate::trading::PositionOverlay>,
    pub(super) depth: Depth,
    latency_ms: Option<u64>,
    liquidations: Vec<Liquidation>,
}

impl KlineChart {
    pub fn market_snapshot(
        &self,
        short: usize,
        long: usize,
        exclude: bool,
    ) -> Result<MarketSnapshot, String> {
        let Basis::Time(timeframe) = self.basis else {
            return Err("AI analysis currently requires a time-based Kline chart.".into());
        };
        let series = self.data.base_series().ok_or("No primary series loaded.")?;
        let limit = long.max(short).clamp(20, 500);
        let mut points = series
            .points()
            .rev()
            .skip(usize::from(exclude))
            .take(limit)
            .collect::<Vec<_>>();
        points.reverse();
        if points.is_empty() || points.len() < short.min(20) {
            return Err(format!(
                "Not enough loaded history for analysis: {} bars available.",
                points.len()
            ));
        }
        let mut bars = points
            .iter()
            .map(|point| {
                let k = point.kline;
                let directional = k
                    .volume
                    .buy_sell()
                    .map(|(buy, sell)| (buy.to_f64(), sell.to_f64()))
                    .or_else(|| {
                        let (buy, sell) = point.footprint.trades.values().fold(
                            (0.0, 0.0),
                            |(buy, sell), group| {
                                (buy + group.buy_qty.to_f64(), sell + group.sell_qty.to_f64())
                            },
                        );
                        (buy > 0.0 || sell > 0.0).then_some((buy, sell))
                    });
                MarketBar {
                    time_ms: k.time.as_u64(),
                    open: k.open.to_f64(),
                    high: k.high.to_f64(),
                    low: k.low.to_f64(),
                    close: k.close.to_f64(),
                    volume: k.volume.total().to_f64(),
                    buy_volume: directional.map(|v| v.0),
                    sell_volume: directional.map(|v| v.1),
                    delta: directional.map(|v| v.0 - v.1),
                    cvd: None,
                    open_interest: series
                        .open_interest()
                        .and_then(|oi| oi.latest_at_or_before(k.time)),
                }
            })
            .collect::<Vec<_>>();
        crate::ai::populate_relative_cvd(&mut bars);
        let first = bars.first().unwrap().time_ms;
        let end = bars
            .last()
            .unwrap()
            .time_ms
            .saturating_add(timeframe.to_milliseconds());
        let mut liquidations = LiquidationSummary {
            long_quantity: 0.0,
            short_quantity: 0.0,
            event_count: 0,
        };
        for event in self
            .pro
            .liquidations
            .iter()
            .filter(|e| (first..end).contains(&e.time.as_u64()))
        {
            if event.is_sell {
                liquidations.long_quantity += event.qty.to_f64();
            } else {
                liquidations.short_quantity += event.qty.to_f64();
            }
            liquidations.event_count += 1;
        }
        let depth = self
            .pro
            .depth
            .bids
            .iter()
            .rev()
            .take(10)
            .map(|(p, q)| DepthLevel {
                side: "bid",
                price: p.to_f64(),
                quantity: q.to_f64(),
            })
            .chain(
                self.pro
                    .depth
                    .asks
                    .iter()
                    .take(10)
                    .map(|(p, q)| DepthLevel {
                        side: "ask",
                        price: p.to_f64(),
                        quantity: q.to_f64(),
                    }),
            )
            .collect::<Vec<_>>();
        let mut profile = BTreeMap::<Price, f64>::new();
        for point in points.iter().rev().take(short) {
            for (price, group) in &point.footprint.trades {
                *profile.entry(*price).or_default() += group.total_qty().to_f64();
            }
        }
        let poc = profile
            .into_iter()
            .max_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(p, _)| p.to_f64());
        let mut features = MarketFeatures::calculate(&bars, &depth, short);
        features.set_volume_profile_poc(poc, bars.last().map(|b| b.close));
        let directional = bars.iter().filter(|b| b.delta.is_some()).count();
        let has_oi = bars.iter().any(|b| b.open_interest.is_some());
        let mut missing_data = Vec::new();
        if directional == 0 {
            missing_data.push("order_flow");
        }
        if !has_oi {
            missing_data.push("open_interest");
        }
        if depth.is_empty() {
            missing_data.push("order_book");
        }
        let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
        let timeframe_ms = timeframe.to_milliseconds();
        let data_age_ms = now.saturating_sub(end);
        let freshness = if data_age_ms <= timeframe_ms {
            1.0
        } else {
            timeframe_ms as f32 / data_age_ms as f32
        };
        let data_quality = (bars.len() as f32 / limit as f32 * 0.4
            + directional as f32 / bars.len() as f32 * 0.2
            + if depth.is_empty() { 0.0 } else { 0.15 }
            + if has_oi { 0.1 } else { 0.0 }
            + freshness * 0.15)
            .clamp(0.0, 1.0);
        let ticker = self.data.base_ticker();
        Ok(MarketSnapshot {
            generated_at_ms: now as i64,
            symbol: ticker.ticker.to_string(),
            venue: format!("{:?}", ticker.exchange()),
            market: format!("{:?}", ticker.market_type()),
            basis: self.basis.to_string(),
            timeframe_ms,
            latest_market_time_ms: bars.last().unwrap().time_ms,
            data_age_ms,
            bars,
            depth,
            liquidations,
            features,
            data_quality,
            missing_data,
        })
    }

    pub(crate) fn alert_observation(
        &self,
        pane_id: uuid::Uuid,
    ) -> Option<crate::alerts::Observation> {
        if !self.basis.is_time() {
            return None;
        }
        let mut points = self.data.base_series()?.points().rev();
        let latest = points.next()?.kline;
        Some(crate::alerts::Observation {
            pane_id,
            price: latest.close.to_f64(),
            volume: latest.volume.total().to_f64(),
            reference_volume: points.next().map(|p| p.kline.volume.total().to_f64()),
            market_time_ms: latest.time.as_u64(),
        })
    }

    pub(crate) fn trading_market_info(&self) -> (Option<u64>, Option<f64>, Option<f64>) {
        (
            self.pro.latency_ms,
            self.pro.depth.bids.keys().next_back().map(|p| p.to_f64()),
            self.pro.depth.asks.keys().next().map(|p| p.to_f64()),
        )
    }

    pub(super) fn liquidations(&self) -> &[Liquidation] {
        &self.pro.liquidations
    }

    pub fn insert_depth(&mut self, depth: &Depth, latency_ms: Option<u64>) {
        self.pro.depth = depth.clone();
        self.pro.latency_ms = latency_ms;
        self.render_invalidation.invalidate();
    }

    pub fn insert_liquidations(&mut self, events: &[Liquidation]) {
        self.pro.liquidations.extend(events.iter().copied());
        let extra = self.pro.liquidations.len().saturating_sub(20_000);
        if extra > 0 {
            self.pro.liquidations.drain(0..extra);
        }
        self.render_invalidation.invalidate();
    }

    pub(crate) fn set_ai_annotation(&mut self, annotation: ChartAnnotation) {
        // Keep historical signals, but replace repeat analysis of the same candle.
        self.pro
            .annotations
            .retain(|a| a.time_ms != annotation.time_ms);
        self.pro.annotations.push(annotation);
        if self.pro.annotations.len() > 500 {
            self.pro.annotations.remove(0);
        }
        self.render_invalidation.invalidate();
    }

    pub(crate) fn clear_ai_annotation(&mut self) {
        self.pro.annotations.clear();
        self.render_invalidation.invalidate();
    }

    pub(crate) fn set_trading_overlay(&mut self, overlay: Option<crate::trading::PositionOverlay>) {
        self.pro.trading = overlay;
        self.render_invalidation.invalidate();
    }

    pub fn clear_all_drawings(&mut self) {
        let update = self.drawings.update(
            Event::Tools(DrawingMessage::DeleteAllDrawings),
            &mut Default::default(),
        );
        self.apply_drawing_update(update);
        self.clear_ai_annotation();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use exchange::{
        Kline, Ticker, TickerInfo, Timeframe, Trade, UnixMs, Volume, adapter::Exchange, unit::Qty,
    };

    fn chart() -> KlineChart {
        let ticker = TickerInfo::new(
            Ticker::new("BTCUSDT", Exchange::BinanceLinear),
            0.1,
            0.001,
            None,
        );
        let mut chart = KlineChart::new(Basis::Time(Timeframe::M1), &[ticker]);
        let bars = (1..=25)
            .map(|i| {
                Kline::new(
                    UnixMs::new(i * 60_000),
                    100.0,
                    102.0,
                    99.0,
                    101.0,
                    Volume::TotalOnly(Qty::from_f64(10.0)),
                    ticker.min_ticksize,
                )
            })
            .collect::<Vec<_>>();
        chart
            .data
            .insert_historical_klines(ticker, &bars, chart.basis);
        chart
    }

    #[test]
    fn pro_snapshot_closed_bar_limit_and_missing_inputs() {
        let chart = chart();
        let snapshot = chart.market_snapshot(20, 20, true).unwrap();
        assert_eq!(snapshot.bars.len(), 20);
        assert_eq!(snapshot.latest_market_time_ms, 24 * 60_000);
        assert_eq!(snapshot.bars[0].time_ms, 5 * 60_000);
        assert_eq!(
            snapshot.missing_data,
            vec!["order_flow", "open_interest", "order_book"]
        );
        assert!(snapshot.data_quality.is_finite());
        let observation = chart.alert_observation(uuid::Uuid::nil()).unwrap();
        assert_eq!(observation.market_time_ms, 25 * 60_000);
        assert_eq!(observation.reference_volume, Some(10.0));
    }

    #[test]
    fn pro_snapshot_footprints_oi_depth_and_bounded_liquidations() {
        let mut chart = chart();
        let ticker = chart.data.base_ticker();
        let trades = [
            Trade {
                time: UnixMs::new(24 * 60_000 + 1),
                is_sell: false,
                price: Price::from_f64(100.0),
                qty: Qty::from_f64(4.0),
            },
            Trade {
                time: UnixMs::new(24 * 60_000 + 2),
                is_sell: true,
                price: Price::from_f64(100.0),
                qty: Qty::from_f64(1.0),
            },
        ];
        chart
            .data
            .insert_historical_trades(ticker, &trades, true, chart.basis);
        let oi = data::chart::kline::OpenInterestSeries::from_points([
            (UnixMs::new(23 * 60_000), 123.0),
            (UnixMs::new(25 * 60_000), 999.0),
        ]);
        chart
            .data
            .insert_historical_open_interest(ticker, &oi, &chart.indicators.instances);
        let mut depth = Depth::default();
        depth
            .bids
            .insert(Price::from_f64(100.0), Qty::from_f64(3.0));
        depth
            .asks
            .insert(Price::from_f64(101.0), Qty::from_f64(2.0));
        chart.insert_depth(&depth, Some(42));
        chart.insert_liquidations(&[
            Liquidation {
                time: UnixMs::new(24 * 60_000 + 3),
                is_sell: true,
                price: Price::from_f64(100.0),
                qty: Qty::from_f64(7.0),
            },
            Liquidation {
                time: UnixMs::new(25 * 60_000 + 3),
                is_sell: false,
                price: Price::from_f64(100.0),
                qty: Qty::from_f64(99.0),
            },
        ]);
        let snapshot = chart.market_snapshot(20, 20, true).unwrap();
        let latest = snapshot.bars.last().unwrap();
        assert_eq!(latest.delta, Some(3.0));
        assert_eq!(latest.open_interest, Some(123.0));
        assert_eq!(latest.cvd, Some(3.0));
        assert_eq!(snapshot.features.volume_profile_poc, Some(100.0));
        assert_eq!(snapshot.liquidations.event_count, 1);
        assert_eq!(snapshot.liquidations.long_quantity, 7.0);
        assert_eq!(snapshot.liquidations.short_quantity, 0.0);
        assert_eq!(
            chart.trading_market_info(),
            (Some(42), Some(100.0), Some(101.0))
        );
        assert_eq!(snapshot.depth.len(), 2);
    }
}
