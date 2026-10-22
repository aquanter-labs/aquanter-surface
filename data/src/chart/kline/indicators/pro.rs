use crate::chart::kline::pro::OrderFlowBiasConfig;
use exchange::UnixMs;
use std::collections::BTreeMap;
use crate::chart::kline::{KlineSeriesPoints, pro::VwapMode};

pub fn bias_for_series(points: KlineSeriesPoints<'_>, config: OrderFlowBiasConfig) -> BTreeMap<UnixMs, BiasPoint> {
    let bars = points.map(|point| {
        let summary = point.footprint.trade_summary();
        let directional = point.kline.volume.buy_sell().map(|(b,s)| (b.to_f64(),s.to_f64()))
            .or_else(|| summary.map(|s| (s.buy_qty.to_f64(),s.sell_qty.to_f64())));
        let (buy,sell) = point.footprint.trades.values().fold((0_u32,0_u32), |(buy,sell),level| {
            let (b,s) = (level.buy_qty.to_f64(),level.sell_qty.to_f64());
            let ratio = f64::from(config.imbalance_ratio.clamp(1.1,10.0));
            if b > 0.0 && b >= s * ratio { (buy+1,sell) }
            else if s > 0.0 && s >= b * ratio { (buy,sell+1) } else { (buy,sell) }
        });
        BiasBar { time:point.kline.time, open:point.kline.open.to_f64(), high:point.kline.high.to_f64(), low:point.kline.low.to_f64(), close:point.kline.close.to_f64(),
            delta:directional.map_or(0.0,|(b,s)| b-s), directional:directional.is_some(), poc:point.footprint.poc.map(|p|p.price.to_f64()),
            imbalance:if buy+sell == 0 { 0.0 } else { (f64::from(buy)-f64::from(sell))/f64::from(buy+sell) } }
    }).collect::<Vec<_>>();
    order_flow_bias(&bars,config)
}

#[derive(Debug, Clone, Copy)]
pub struct VwapPoint {
    pub time: UnixMs,
    pub value: f64,
    pub deviation: f64,
    pub session: i64,
}

/// VWAP reset key for a bar: UTC sessions, Monday-start weeks, calendar months.
/// `None` skips the bar (before the anchor).
fn vwap_session(time_ms: u64, mode: VwapMode) -> Option<i64> {
    use chrono::Datelike;
    let time = time_ms.min(i64::MAX as u64) as i64;
    Some(match mode {
        VwapMode::Session { hour_utc } => (time-i64::from(hour_utc.min(23))*3_600_000).div_euclid(86_400_000),
        VwapMode::Week => (time+3*86_400_000).div_euclid(7*86_400_000),
        VwapMode::Month => {
            let date = chrono::DateTime::from_timestamp_millis(time)?;
            i64::from(date.year())*12+i64::from(date.month())
        },
        VwapMode::Anchor { time_ms: anchor } | VwapMode::Visible { time_ms: anchor } => { if time_ms < anchor { return None; } 0 },
        VwapMode::Rolling { .. } => 0,
    })
}

pub fn vwap_for_series(points: KlineSeriesPoints<'_>, mode: VwapMode) -> Vec<VwapPoint> {
    let mut output = Vec::new();
    let mut window = std::collections::VecDeque::new();
    let (mut weight,mut price_sum,mut square_sum) = (0.0,0.0,0.0);
    let mut previous_session = None;
    for point in points {
        let k = point.kline;
        let Some(session) = vwap_session(k.time.as_u64(), mode) else { continue; };
        if previous_session != Some(session) {
            weight=0.0; price_sum=0.0; square_sum=0.0; window.clear();
        }
        previous_session=Some(session);
        let volume=k.volume.total().to_f64();
        let price=(k.high.to_f64()+k.low.to_f64()+k.close.to_f64())/3.0;
        if !volume.is_finite() || volume <= 0.0 || !price.is_finite() { continue; }
        weight+=volume; price_sum+=price*volume; square_sum+=price*price*volume;
        if let VwapMode::Rolling { bars } = mode {
            window.push_back((price,volume));
            if window.len() > usize::from(bars.max(1)) {
                let (p,v)=window.pop_front().expect("nonempty rolling window");
                weight-=v; price_sum-=p*v; square_sum-=p*p*v;
            }
        }
        if weight > 0.0 {
            let value=price_sum/weight;
            output.push(VwapPoint { time:k.time,value,deviation:(square_sum/weight-value*value).max(0.0).sqrt(),session });
        }
    }
    output
}

#[derive(Clone, Copy)]
pub struct BiasBar {
    pub time: UnixMs,
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub delta: f64,
    pub directional: bool,
    pub poc: Option<f64>,
    pub imbalance: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BiasPoint {
    pub score: f64,
    pub absorption: f64,
    pub divergence: f64,
    pub poc_rejection: f64,
    pub imbalance: f64,
    pub correct: Option<bool>,
    pub wins: u32,
    pub total: u32,
}

fn absorption_score(delta: f64, price_response: f64, config: OrderFlowBiasConfig) -> f64 {
    let threshold = f64::from(config.absorption_delta.clamp(0.1, 0.95));
    let strength = ((delta.abs() - threshold) / (1.0 - threshold)).clamp(0.0, 1.0);
    if strength == 0.0 {
        return 0.0;
    }
    let response_limit = f64::from(config.max_price_response.clamp(0.05, 1.0));
    let failure = if delta.signum() != price_response.signum() {
        1.0
    } else {
        ((response_limit - price_response.abs()) / response_limit).clamp(0.0, 1.0)
    };
    -delta.signum() * strength * failure * 35.0
}

pub fn order_flow_bias(bars: &[BiasBar], config: OrderFlowBiasConfig) -> BTreeMap<UnixMs, BiasPoint> {
    let lookback = usize::from(config.lookback.max(2));
    let signal_threshold = f64::from(config.signal_threshold.clamp(5.0, 100.0));
    let closed_len = bars.len().saturating_sub(1);
    let mut points = BTreeMap::new();
    let (mut wins, mut total) = (0, 0);

    for index in lookback..closed_len {
        let bar = bars[index];
        let history = &bars[(index - lookback)..index];
        let average_delta =
            history.iter().map(|bar| bar.delta.abs()).sum::<f64>() / lookback as f64;
        if !bar.directional || average_delta <= f64::EPSILON {
            continue;
        }
        let delta = (bar.delta / average_delta).clamp(-1.0, 1.0);
        let range = (bar.high - bar.low).max(f64::EPSILON);
        let price_response = ((bar.close - bar.open) / range).clamp(-1.0, 1.0);
        let absorption = absorption_score(delta, price_response, config);

        let average_range =
            history.iter().map(|bar| bar.high - bar.low).sum::<f64>() / lookback as f64;
        let price_change = ((bar.close - history[0].close)
            / (average_range.max(f64::EPSILON) * lookback as f64))
            .clamp(-1.0, 1.0);
        let cvd = bars[(index - lookback)..=index]
            .iter()
            .map(|bar| bar.delta)
            .sum::<f64>();
        let cvd = (cvd / (average_delta * (lookback + 1) as f64)).clamp(-1.0, 1.0);
        let divergence = if price_change * cvd < 0.0 {
            cvd * 30.0
        } else {
            0.0
        };
        let poc_rejection = bar.poc.map_or(0.0, |poc| {
            ((bar.close - poc) / range).clamp(-1.0, 1.0) * 20.0
        });
        let imbalance = bar.imbalance.clamp(-1.0, 1.0) * 15.0;
        let score = (absorption + divergence + poc_rejection + imbalance).clamp(-100.0, 100.0);

        let correct = (score.abs() >= signal_threshold && index + 1 < closed_len).then(|| {
            total += 1;
            let hit = (score > 0.0) == (bars[index + 1].close > bar.close);
            wins += u32::from(hit);
            hit
        });
        points.insert(
            bar.time,
            BiasPoint {
                score,
                absorption,
                divergence,
                poc_rejection,
                imbalance,
                correct,
                wins,
                total,
            },
        );
    }
    points
}

#[cfg(test)]
mod tests {
    use super::*;
    use indicator_engine::{ChartFrame, IndicatorFrame, InstrumentContext, TimeOrTick, TradeAggregate, Value};

    fn imbalance(levels: &[(f64, f64)], ratio: f64) -> f64 {
        let (buy, sell) = levels.iter().fold((0_u32, 0_u32), |(buy, sell), &(b, s)| {
            if b > 0.0 && b >= s * ratio { (buy + 1, sell) }
            else if s > 0.0 && s >= b * ratio { (buy, sell + 1) } else { (buy, sell) }
        });
        if buy + sell == 0 { 0.0 } else { (f64::from(buy) - f64::from(sell)) / f64::from(buy + sell) }
    }

    #[test]
    fn vwap_sessions_reset_on_calendar_boundaries() {
        use chrono::TimeZone;
        let ms = |y, m, d, h| chrono::Utc.with_ymd_and_hms(y, m, d, h, 0, 0).unwrap().timestamp_millis() as u64;
        let week = |time| vwap_session(time, VwapMode::Week);
        // 2026-10-19 is a Monday; 2026-10-22 a Thursday.
        assert_ne!(week(ms(2026, 10, 19, 0) - 1), week(ms(2026, 10, 19, 0)));
        assert_eq!(week(ms(2026, 10, 19, 0)), week(ms(2026, 10, 22, 12)));
        assert_eq!(week(ms(2026, 10, 22, 0) - 1), week(ms(2026, 10, 22, 0)));
        let month = |time| vwap_session(time, VwapMode::Month);
        assert_eq!(month(ms(2026, 10, 1, 0)), month(ms(2026, 10, 4, 12)));
        assert_eq!(month(ms(2026, 10, 1, 0)), month(ms(2026, 10, 31, 23)));
        assert_ne!(month(ms(2026, 9, 30, 23)), month(ms(2026, 10, 1, 0)));
        let session = |time| vwap_session(time, VwapMode::Session { hour_utc: 8 });
        assert_ne!(session(ms(2026, 10, 22, 7)), session(ms(2026, 10, 22, 8)));
        assert_eq!(vwap_session(5, VwapMode::Anchor { time_ms: 10 }), None);
    }

    #[test]
    fn script_matches_the_original_order_flow_bias() {
        let config = OrderFlowBiasConfig::default();
        let source = include_str!("../../../../indicators/sources/order-flow-bias.ind");
        let compiled = indicator_engine::compile(source, "order-flow-bias.ind").expect("bias script compiles");
        let mut instance = compiled.instantiate(Default::default()).expect("default inputs bind");

        let mut bars = Vec::new();
        let mut outputs = Vec::new();
        for index in 0..160_usize {
            let t = index as f64;
            let open = 100.0 + (t * 0.37).sin() * 4.0;
            let close = open + (t * 1.3).cos() * 1.5;
            let high = open.max(close) + 0.5 + (t * 0.7).sin().abs();
            let low = open.min(close) - 0.5 - (t * 0.9).cos().abs();
            let buy = 50.0 + (t * 0.51).sin() * 40.0;
            let sell = 50.0 + (t * 0.23).cos() * 40.0;
            let levels = vec![(buy * 0.6, sell * 0.1), (buy * 0.3, sell * 0.5), (buy * 0.1, sell * 0.4)];
            let poc = (index % 7 != 0).then_some(low + (high - low) * ((t * 0.3).sin() * 0.5 + 0.5));
            bars.push(BiasBar {
                time: UnixMs::new(index as u64 * 60_000),
                open, high, low, close,
                delta: buy - sell,
                directional: true,
                poc,
                imbalance: imbalance(&levels, f64::from(config.imbalance_ratio)),
            });
            let frame = IndicatorFrame {
                index,
                time: index as i64 * 60_000,
                basis: TimeOrTick::Time,
                instrument: InstrumentContext { min_tick: Some(0.01) },
                chart: ChartFrame {
                    open: Some(open), high: Some(high), low: Some(low), close: Some(close),
                    volume: Some(buy + sell),
                    directional_volume_available: true,
                    buy_qty: buy,
                    sell_qty: sell,
                    trade: Some(TradeAggregate { buy_qty: buy, sell_qty: sell, buy_count: 1, sell_count: 1, poc }),
                    footprint_levels: levels.into(),
                },
                open_interest: None,
                series_inputs: Vec::new().into(),
            };
            outputs.push(instance.evaluate_closed(frame).expect("bias evaluates"));
        }

        let plot = |index: usize, id: &str| {
            outputs[index].plots.iter().find(|plot| plot.id == id).and_then(|plot| match plot.value {
                Value::Float(value) => Some(value),
                _ => None,
            })
        };
        let expected = order_flow_bias(&bars, config);
        assert!(expected.values().any(|point| point.correct.is_some()));
        for (index, bar) in bars.iter().enumerate().take(bars.len() - 1) {
            let Some(point) = expected.get(&bar.time) else {
                assert_eq!(plot(index, "value"), None, "bar {index} should have no score");
                continue;
            };
            // The original reads its parameters as f32; the script uses f64.
            let close = |a: Option<f64>, b: f64, what: &str| {
                let a = a.unwrap_or_else(|| panic!("bar {index} {what} missing"));
                assert!((a - b).abs() < 1e-5, "bar {index} {what}: script {a} vs original {b}");
            };
            close(plot(index, "value"), point.score, "score");
            close(plot(index, "absorption"), point.absorption, "absorption");
            close(plot(index, "divergence"), point.divergence, "divergence");
            close(plot(index, "poc_rejection"), point.poc_rejection, "poc rejection");
            close(plot(index, "imbalance"), point.imbalance, "imbalance");
            if point.correct.is_some() {
                let rate = f64::from(point.wins) / f64::from(point.total) * 100.0;
                close(plot(index + 1, "win_rate"), rate, "win rate");
            }
        }
    }
}

