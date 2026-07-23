use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use data::chart::kline::{AnchoredVwapConfig, Config, KlineDataPoint};
use data::chart::{Basis, PlotData};
use exchange::unit::Price;
use exchange::{Kline, UnixMs};
use iced::widget::{Space, canvas};
use iced::{Element, Point, Theme};

use super::{
    AvailabilityCause, IndicatorAvailability, KlineIndicatorImpl, ThemeColor, themed_color,
};
use crate::chart::{Message, ViewState};

#[derive(Clone, Copy)]
struct PointValue {
    value: f64,
    deviation: f64,
}

pub struct AnchoredVwapIndicator {
    points: BTreeMap<UnixMs, PointValue>,
    config: AnchoredVwapConfig,
}

impl AnchoredVwapIndicator {
    pub fn new(config: AnchoredVwapConfig) -> Self {
        Self {
            points: BTreeMap::new(),
            config,
        }
    }

    fn rebuild(&mut self, source: &PlotData<KlineDataPoint>) {
        self.points.clear();
        let (PlotData::TimeBased(series), Some(anchor)) = (source, self.config.anchor) else {
            return;
        };
        let mut weight = 0.0;
        let mut weighted_price = 0.0;
        let mut weighted_price_sq = 0.0;
        for (&time, datapoint) in series.datapoints.range(anchor..) {
            let kline = &datapoint.kline;
            let price = (kline.high.to_f64() + kline.low.to_f64() + kline.close.to_f64()) / 3.0;
            let volume = kline.volume.total().to_f64();
            if volume <= 0.0 {
                continue;
            }
            weight += volume;
            weighted_price += price * volume;
            weighted_price_sq += price * price * volume;
            let value = weighted_price / weight;
            self.points.insert(
                time,
                PointValue {
                    value,
                    deviation: (weighted_price_sq / weight - value * value).max(0.0).sqrt(),
                },
            );
        }
    }

    fn draw_line(
        &self,
        frame: &mut canvas::Frame,
        chart: &ViewState,
        range: RangeInclusive<u64>,
        multiple: i8,
        color: iced::Color,
    ) {
        let mut previous = None;
        for (&time, point) in self
            .points
            .range(UnixMs::new(*range.start())..=UnixMs::new(*range.end()))
        {
            let current = Point::new(
                chart.interval_to_x(time.as_u64()),
                chart.price_to_y(Price::from_f64(
                    point.value + f64::from(multiple) * point.deviation,
                )),
            );
            if let Some(previous) = previous {
                frame.stroke(
                    &canvas::Path::line(previous, current),
                    canvas::Stroke::with_color(
                        canvas::Stroke {
                            width: if multiple == 0 {
                                self.config.line_width.clamp(0.5, 5.0)
                            } else {
                                (self.config.line_width.clamp(0.5, 5.0) * 2.0 / 3.0).max(0.5)
                            } / chart.scaling,
                            ..Default::default()
                        },
                        color,
                    ),
                );
            }
            previous = Some(current);
        }
    }
}

impl KlineIndicatorImpl for AnchoredVwapIndicator {
    fn clear_all_caches(&mut self) {}
    fn clear_crosshair_caches(&mut self) {}

    fn element<'a>(
        &'a self,
        _chart: &'a ViewState,
        _labels: bool,
        _range: RangeInclusive<u64>,
    ) -> Element<'a, Message> {
        Space::new().into()
    }

    fn availability(&self, chart: &ViewState) -> IndicatorAvailability {
        if matches!(chart.basis, Basis::Time(_)) {
            IndicatorAvailability::Available
        } else {
            IndicatorAvailability::Unavailable(AvailabilityCause::Basis(chart.basis))
        }
    }

    fn is_overlay(&self) -> bool {
        true
    }

    fn draw_overlay(
        &self,
        frame: &mut canvas::Frame,
        chart: &ViewState,
        theme: &Theme,
        range: RangeInclusive<u64>,
    ) {
        let mut color = themed_color(
            theme,
            self.config.color,
            AnchoredVwapConfig::default().color,
            ThemeColor::Warning,
        );
        self.draw_line(frame, chart, range.clone(), 0, color);
        color.a = 0.65;
        for band in 1..=self.config.deviation_bands.min(3) as i8 {
            self.draw_line(frame, chart, range.clone(), band, color);
            self.draw_line(frame, chart, range.clone(), -band, color);
        }
    }

    fn rebuild_from_source(&mut self, source: &PlotData<KlineDataPoint>) {
        self.rebuild(source);
    }

    fn on_insert_klines(&mut self, _klines: &[Kline], source: &PlotData<KlineDataPoint>) {
        self.rebuild(source);
    }

    fn on_basis_change(&mut self, source: &PlotData<KlineDataPoint>) {
        self.rebuild(source);
    }

    fn on_visual_config_change(&mut self, config: &Config, source: &PlotData<KlineDataPoint>) {
        self.config = config.anchored_vwap;
        self.rebuild(source);
    }
}
