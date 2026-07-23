use std::ops::RangeInclusive;

use data::chart::kline::{Config, EmaConfig, KlineDataPoint};
use data::chart::{BasisSeries, PlotData};
use exchange::unit::Price;
use exchange::{Kline, Trade, UnixMs};
use iced::widget::{Space, canvas};
use iced::{Element, Point, Theme};

use crate::chart::indicator::kline::{BasisSeriesExt, KlineIndicatorImpl};
use crate::chart::indicator::plot::Series;
use crate::chart::{Message, ViewState};

fn next_ema(period: u16, previous: Option<f64>, close: f64) -> f64 {
    let alpha = 2.0 / (f64::from(period.max(1)) + 1.0);
    previous.map_or(close, |value| value + alpha * (close - value))
}

struct EmaLine {
    config: EmaConfig,
    points: BasisSeries<f64>,
}

impl EmaLine {
    fn new(config: EmaConfig) -> Self {
        Self {
            config,
            points: BasisSeries::default(),
        }
    }

    fn rebuild_time(&mut self, source: &PlotData<KlineDataPoint>, from: Option<UnixMs>) {
        let PlotData::TimeBased(series) = source else {
            return;
        };
        let points = match &mut self.points {
            BasisSeries::Time(points) => points,
            BasisSeries::Tick(_) => {
                self.points = BasisSeries::time(Default::default());
                self.points.time_mut().unwrap()
            }
        };
        let start = from.unwrap_or(UnixMs::ZERO);
        let mut previous = points.range(..start).next_back().map(|(_, value)| *value);
        points.retain(|time, _| *time < start);
        for (&time, datapoint) in series.datapoints.range(start..) {
            let close = datapoint.kline.close.to_f64();
            let value = next_ema(self.config.period, previous, close);
            points.insert(time, value);
            previous = Some(value);
        }
    }

    fn rebuild_tick(&mut self, source: &PlotData<KlineDataPoint>, from: usize) {
        let PlotData::TickBased(series) = source else {
            return;
        };
        let points = match &mut self.points {
            BasisSeries::Tick(points) => points,
            BasisSeries::Time(_) => {
                self.points = BasisSeries::tick(Default::default());
                self.points.tick_mut().unwrap()
            }
        };
        let start = from as u64;
        let mut previous = points.range(..start).next_back().map(|(_, value)| *value);
        points.retain(|index, _| *index < start);
        for (index, datapoint) in series.datapoints.iter().enumerate().skip(from) {
            let close = datapoint.kline.close.to_f64();
            let value = next_ema(self.config.period, previous, close);
            points.insert(index as u64, value);
            previous = Some(value);
        }
    }
}

pub struct EmaIndicator {
    lines: Vec<EmaLine>,
}

impl EmaIndicator {
    pub fn new(config: Vec<EmaConfig>) -> Self {
        Self {
            lines: config.into_iter().map(EmaLine::new).collect(),
        }
    }

    fn rebuild(&mut self, source: &PlotData<KlineDataPoint>) {
        for line in &mut self.lines {
            match source {
                PlotData::TimeBased(_) => line.rebuild_time(source, None),
                PlotData::TickBased(_) => line.rebuild_tick(source, 0),
            }
        }
    }
}

impl KlineIndicatorImpl for EmaIndicator {
    fn clear_all_caches(&mut self) {}

    fn clear_crosshair_caches(&mut self) {}

    fn element<'a>(
        &'a self,
        _chart: &'a ViewState,
        _data_labels_always_visible: bool,
        _visible_range: RangeInclusive<u64>,
    ) -> Element<'a, Message> {
        Space::new().into()
    }

    fn is_overlay(&self) -> bool {
        true
    }

    fn draw_overlay(
        &self,
        frame: &mut canvas::Frame,
        chart: &ViewState,
        theme: &Theme,
        visible_range: RangeInclusive<u64>,
    ) {
        let palette = theme.extended_palette();
        for line in &self.lines {
            let auto_index =
                (0..6).find(|index| line.config.color == EmaConfig::for_index(*index).color);
            let color = auto_index.map_or(line.config.color, |index| match index {
                0 => palette.danger.base.color,
                1 => palette.primary.strong.color,
                2 => palette.success.base.color,
                3 => palette.secondary.strong.color,
                4 => palette.warning.strong.color,
                _ => palette.primary.weak.color,
            });
            let mut previous = None;
            line.points
                .as_plot_series()
                .for_each_in(visible_range.clone(), |time, value| {
                    let current = Point::new(
                        chart.interval_to_x(time),
                        chart.price_to_y(Price::from_f64(*value)),
                    );
                    if let Some(previous) = previous {
                        frame.stroke(
                            &canvas::Path::line(previous, current),
                            canvas::Stroke::with_color(
                                canvas::Stroke {
                                    width: line.config.line_width.clamp(0.5, 5.0) / chart.scaling,
                                    ..canvas::Stroke::default()
                                },
                                color,
                            ),
                        );
                    }
                    previous = Some(current);
                });
        }
    }

    fn rebuild_from_source(&mut self, source: &PlotData<KlineDataPoint>) {
        self.rebuild(source);
    }

    fn on_insert_klines(&mut self, klines: &[Kline], source: &PlotData<KlineDataPoint>) {
        let from = klines.iter().map(|kline| kline.time).min();
        for line in &mut self.lines {
            line.rebuild_time(source, from);
        }
    }

    fn on_insert_trades(
        &mut self,
        _trades: &[Trade],
        old_dp_len: usize,
        source: &PlotData<KlineDataPoint>,
    ) {
        if matches!(source, PlotData::TickBased(_)) {
            for line in &mut self.lines {
                line.rebuild_tick(source, old_dp_len.saturating_sub(1));
            }
        }
    }

    fn on_basis_change(&mut self, source: &PlotData<KlineDataPoint>) {
        self.rebuild(source);
    }

    fn on_visual_config_change(&mut self, config: &Config, source: &PlotData<KlineDataPoint>) {
        self.lines = config.ema.iter().copied().map(EmaLine::new).collect();
        self.rebuild(source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replacing_current_close_does_not_advance_ema_twice() {
        let first = next_ema(3, None, 10.0);
        let second = next_ema(3, Some(first), 20.0);
        let replaced = next_ema(3, Some(first), 30.0);

        assert_eq!(second, 15.0);
        assert_eq!(replaced, 20.0);
    }

    #[test]
    fn new_ema_lines_cycle_distinct_muted_colors() {
        assert_ne!(EmaConfig::for_index(0).color, EmaConfig::for_index(1).color);
        assert_eq!(EmaConfig::for_index(0).color, EmaConfig::for_index(6).color);
    }
}
