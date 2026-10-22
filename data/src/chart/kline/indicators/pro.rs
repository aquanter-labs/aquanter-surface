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

pub fn vwap_for_series(points: KlineSeriesPoints<'_>, mode: VwapMode) -> Vec<VwapPoint> {
    use chrono::Datelike;
    let mut output = Vec::new();
    let mut window = std::collections::VecDeque::new();
    let (mut weight,mut price_sum,mut square_sum) = (0.0,0.0,0.0);
    let mut previous_session = None;
    for point in points {
        let k = point.kline;
        let time = k.time.as_u64().min(i64::MAX as u64) as i64;
        let session = match mode {
            VwapMode::Session { hour_utc } => (time-i64::from(hour_utc.min(23))*3_600_000).div_euclid(86_400_000),
            VwapMode::Week => (time+3*86_400_000).div_euclid(7*86_400_000),
            VwapMode::Month => {
                let Some(date) = chrono::DateTime::from_timestamp_millis(time) else { continue; };
                i64::from(date.year())*12+i64::from(date.month())
            },
            VwapMode::Anchor { time_ms } | VwapMode::Visible { time_ms } => { if k.time.as_u64() < time_ms { continue; } 0 },
            VwapMode::Rolling { .. } => 0,
        };
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

