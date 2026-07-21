use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use data::chart::kline::{KlineDataPoint, StochRsiConfig};
use data::chart::{Basis, PlotData};
use exchange::{Kline, Trade, UnixMs};
use iced::widget::{canvas, center, text};
use iced::{Element, Point, Theme};

use super::{AvailabilityCause, IndicatorAvailability, KlineIndicatorImpl};
use crate::chart::indicator::indicator_row;
use crate::chart::indicator::plot::{AnySeries, Plot, PlotTooltip, Series, TooltipFn, YScale};
use crate::chart::{Caches, Message, ViewState};

#[derive(Clone, Copy)]
struct StochRsiPoint {
    k: f64,
    d: f64,
}

fn rsi(values: &[f64], period: usize) -> Vec<Option<f64>> {
    let period = period.max(1);
    let mut result = vec![None; values.len()];
    if values.len() <= period {
        return result;
    }
    let changes = values
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .collect::<Vec<_>>();
    let mut gain = changes[..period]
        .iter()
        .map(|change| change.max(0.0))
        .sum::<f64>()
        / period as f64;
    let mut loss = changes[..period]
        .iter()
        .map(|change| (-change).max(0.0))
        .sum::<f64>()
        / period as f64;
    result[period] = Some(rsi_value(gain, loss));
    for index in (period + 1)..values.len() {
        let change = changes[index - 1];
        gain = (gain * (period - 1) as f64 + change.max(0.0)) / period as f64;
        loss = (loss * (period - 1) as f64 + (-change).max(0.0)) / period as f64;
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

fn sma(values: &[Option<f64>], period: usize) -> Vec<Option<f64>> {
    let period = period.max(1);
    (0..values.len())
        .map(|index| {
            let start = index + 1;
            if start < period {
                return None;
            }
            let window = &values[(start - period)..=index];
            let sum = window.iter().copied().collect::<Option<Vec<_>>>()?;
            Some(sum.iter().sum::<f64>() / period as f64)
        })
        .collect()
}

fn calculate(values: &[f64], config: StochRsiConfig) -> Vec<Option<StochRsiPoint>> {
    let rsi = rsi(values, usize::from(config.rsi_length.max(1)));
    let length = usize::from(config.stochastic_length.max(1));
    let stochastic = (0..values.len())
        .map(|index| {
            if index + 1 < length {
                return None;
            }
            let window = rsi[(index + 1 - length)..=index]
                .iter()
                .copied()
                .collect::<Option<Vec<_>>>()?;
            let low = window.iter().copied().fold(f64::INFINITY, f64::min);
            let high = window.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            Some(if high - low <= f64::EPSILON {
                0.0
            } else {
                100.0 * (rsi[index]? - low) / (high - low)
            })
        })
        .collect::<Vec<_>>();
    let k = sma(&stochastic, usize::from(config.k_smoothing.max(1)));
    let d = sma(&k, usize::from(config.d_smoothing.max(1)));
    k.into_iter()
        .zip(d)
        .map(|(k, d)| Some(StochRsiPoint { k: k?, d: d? }))
        .collect()
}

struct StochRsiPlot {
    config: StochRsiConfig,
    tooltip: TooltipFn<StochRsiPoint>,
}

impl Plot<AnySeries<'_, StochRsiPoint>> for StochRsiPlot {
    fn y_extents(
        &self,
        data: &AnySeries<'_, StochRsiPoint>,
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
        data: &AnySeries<'_, StochRsiPoint>,
        range: RangeInclusive<u64>,
        scale: &YScale,
    ) {
        let visible_width = frame.width() / chart.scaling;
        let left = -chart.translation.x - visible_width / 2.0;
        let right = left + visible_width;
        let guide = theme
            .extended_palette()
            .background
            .strong
            .text
            .scale_alpha(0.2);
        for value in [20.0, 80.0] {
            let y = scale.to_y(value);
            frame.stroke(
                &canvas::Path::line(Point::new(left, y), Point::new(right, y)),
                canvas::Stroke::default().with_color(guide),
            );
        }
        let mut previous_k = None;
        let mut previous_d = None;
        data.for_each_in(range, |time, point| {
            let x = chart.interval_to_x(time);
            let k = Point::new(x, scale.to_y(point.k as f32));
            let d = Point::new(x, scale.to_y(point.d as f32));
            for (previous, current, color) in [
                (previous_k, k, self.config.k_color),
                (previous_d, d, self.config.d_color),
            ] {
                if let Some(previous) = previous {
                    frame.stroke(
                        &canvas::Path::line(previous, current),
                        canvas::Stroke::default()
                            .with_width(self.config.line_width.clamp(0.5, 5.0))
                            .with_color(color),
                    );
                }
            }
            previous_k = Some(k);
            previous_d = Some(d);
        });
    }

    fn tooltip_fn(&self) -> Option<&TooltipFn<StochRsiPoint>> {
        Some(&self.tooltip)
    }
}

pub struct StochRsiIndicator {
    config: StochRsiConfig,
    cache: Caches,
    data: BTreeMap<UnixMs, StochRsiPoint>,
}

impl StochRsiIndicator {
    pub fn new(config: StochRsiConfig) -> Self {
        Self {
            config,
            cache: Caches::default(),
            data: BTreeMap::new(),
        }
    }

    fn recompute(&mut self, source: &PlotData<KlineDataPoint>) {
        self.data.clear();
        if let PlotData::TimeBased(series) = source {
            let entries = series.datapoints.iter().collect::<Vec<_>>();
            let closes = entries
                .iter()
                .map(|(_, point)| point.kline.close.to_f64())
                .collect::<Vec<_>>();
            for ((&time, _), value) in entries.into_iter().zip(calculate(&closes, self.config)) {
                if let Some(value) = value {
                    self.data.insert(time, value);
                }
            }
        }
        self.cache.clear_all();
    }
}

impl KlineIndicatorImpl for StochRsiIndicator {
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
        if let Some(message) = self.unavailable_message(chart, "Stoch RSI") {
            return center(text(message)).into();
        }
        let plot = StochRsiPlot {
            config: self.config,
            tooltip: Box::new(|point, _| {
                PlotTooltip::new(format!("%K: {:.1}\n%D: {:.1}", point.k, point.d))
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
        if matches!(chart.basis, Basis::Time(_)) {
            IndicatorAvailability::Available
        } else {
            IndicatorAvailability::Unavailable(AvailabilityCause::Basis(chart.basis))
        }
    }
    fn rebuild_from_source(&mut self, source: &PlotData<KlineDataPoint>) {
        self.recompute(source);
    }
    fn on_insert_klines(&mut self, _: &[Kline], source: &PlotData<KlineDataPoint>) {
        self.recompute(source);
    }
    fn on_insert_trades(&mut self, _: &[Trade], _: usize, source: &PlotData<KlineDataPoint>) {
        self.recompute(source);
    }
    fn on_basis_change(&mut self, source: &PlotData<KlineDataPoint>) {
        self.recompute(source);
    }
    fn on_visual_config_change(
        &mut self,
        config: &data::chart::kline::Config,
        source: &PlotData<KlineDataPoint>,
    ) {
        self.config = config.stoch_rsi;
        self.recompute(source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tradingview_defaults_produce_bounded_stoch_rsi() {
        let values = (0..80)
            .map(|index| 100.0 + (index as f64 / 4.0).sin() * 10.0)
            .collect::<Vec<_>>();
        let points = calculate(&values, StochRsiConfig::default());
        assert!(
            points
                .iter()
                .flatten()
                .all(|point| (0.0..=100.0).contains(&point.k) && (0.0..=100.0).contains(&point.d))
        );
        assert!(points.iter().flatten().count() > 20);
    }
}
