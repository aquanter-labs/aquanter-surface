use std::ops::RangeInclusive;

use data::chart::kline::IndicatorSeriesConfig;
use iced::widget::canvas::{self, Path, Stroke};
use iced::{Point, Size, Theme};

use super::{Plot, PlotTooltip, Series, TooltipFn, YScale};
use crate::chart::ViewState;

pub struct StepCandlePlot<V, T> {
    value: V,
    config: IndicatorSeriesConfig,
    tooltip: Option<TooltipFn<T>>,
    x_shift_buckets: i32,
}

impl<V, T> StepCandlePlot<V, T> {
    pub fn new(value: V, config: IndicatorSeriesConfig) -> Self {
        Self {
            value,
            config,
            tooltip: None,
            x_shift_buckets: 0,
        }
    }

    pub fn shift(mut self, buckets: i32) -> Self {
        self.x_shift_buckets = buckets;
        self
    }

    pub fn with_tooltip<F>(mut self, tooltip: F) -> Self
    where
        F: Fn(&T, Option<&T>) -> PlotTooltip + 'static,
    {
        self.tooltip = Some(Box::new(tooltip));
        self
    }
}

impl<S, V> Plot<S> for StepCandlePlot<V, S::Y>
where
    S: Series,
    V: Fn(&S::Y) -> f32,
{
    fn y_extents(&self, data: &S, range: RangeInclusive<u64>) -> Option<(f32, f32)> {
        let mut min = f32::MAX;
        let mut max = f32::MIN;
        data.for_each_in(range, |_, value| {
            let value = (self.value)(value);
            min = min.min(value);
            max = max.max(value);
        });
        (min != f32::MAX).then_some((min, max.max(min + f32::EPSILON)))
    }

    fn adjust_extents(&self, min: f32, max: f32) -> (f32, f32) {
        let padding = (max - min) * 0.08;
        (min - padding, max + padding)
    }

    fn draw(
        &self,
        frame: &mut canvas::Frame,
        chart: &ViewState,
        _theme: &Theme,
        data: &S,
        range: RangeInclusive<u64>,
        scale: &YScale,
    ) {
        let shift = self.x_shift_buckets as f32 * chart.cell_width;
        let body_width = chart.cell_width * 0.55;
        let mut previous = None;
        data.for_each_in(range, |time, point| {
            let close = (self.value)(point);
            let Some(open) = previous.replace(close) else {
                return;
            };
            let x = chart.interval_to_x(time) + shift;
            let open_y = scale.to_y(open);
            let close_y = scale.to_y(close);
            let color = if close >= open {
                self.config.up_color
            } else {
                self.config.down_color
            };
            frame.stroke(
                &Path::line(Point::new(x, open_y), Point::new(x, close_y)),
                Stroke::default()
                    .with_width(1.0 / chart.scaling)
                    .with_color(color),
            );
            frame.fill_rectangle(
                Point::new(x - body_width / 2.0, open_y.min(close_y)),
                Size::new(
                    body_width,
                    (open_y - close_y).abs().max(1.0 / chart.scaling),
                ),
                color,
            );
        });
    }

    fn tooltip_fn(&self) -> Option<&TooltipFn<S::Y>> {
        self.tooltip.as_ref()
    }

    fn x_shift_buckets(&self) -> i32 {
        self.x_shift_buckets
    }
}
