use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use data::chart::PlotData;
use data::chart::kline::{Config, KlineDataPoint, SessionVwapConfig};
use exchange::{Kline, UnixMs, unit::Price};
use iced::widget::{Space, canvas};
use iced::{Element, Point, Theme};

use crate::chart::indicator::kline::{
    IndicatorAvailability, KlineIndicatorImpl, ThemeColor, themed_color,
};
use crate::chart::{Basis, Message, ViewState};

const DAY_MS: u64 = 86_400_000;

#[derive(Clone, Copy)]
struct VwapPoint {
    value: f64,
    deviation: f64,
    session: u64,
}

#[derive(Default)]
struct WeightedMoments {
    weight: f64,
    weighted_price: f64,
    weighted_price_sq: f64,
}

impl WeightedMoments {
    fn add(&mut self, price: f64, weight: f64) -> Option<(f64, f64)> {
        if weight <= 0.0 {
            return None;
        }
        self.weight += weight;
        self.weighted_price += price * weight;
        self.weighted_price_sq += price * price * weight;

        let mean = self.weighted_price / self.weight;
        let variance = (self.weighted_price_sq / self.weight - mean * mean).max(0.0);
        Some((mean, variance.sqrt()))
    }
}

pub struct SessionVwapIndicator {
    points: BTreeMap<UnixMs, VwapPoint>,
    config: SessionVwapConfig,
}

impl SessionVwapIndicator {
    pub fn new(config: SessionVwapConfig) -> Self {
        Self {
            points: BTreeMap::new(),
            config,
        }
    }

    fn session_start(&self, time: UnixMs) -> u64 {
        let offset = u64::from(self.config.anchor_hour_utc.min(23)) * 3_600_000;
        let time = time.as_u64();
        time.saturating_sub(offset) / DAY_MS * DAY_MS + offset
    }

    fn rebuild_from(&mut self, source: &PlotData<KlineDataPoint>, from: Option<UnixMs>) {
        let PlotData::TimeBased(series) = source else {
            self.points.clear();
            return;
        };

        let start = from.map_or(0, |time| self.session_start(time));
        self.points.retain(|time, _| time.as_u64() < start);

        let mut session = None;
        let mut moments = WeightedMoments::default();
        for (&time, datapoint) in series.datapoints.range(UnixMs::new(start)..) {
            let point_session = self.session_start(time);
            if session != Some(point_session) {
                session = Some(point_session);
                moments = WeightedMoments::default();
            }

            let kline = &datapoint.kline;
            let typical_price =
                (kline.high.to_f64() + kline.low.to_f64() + kline.close.to_f64()) / 3.0;
            if let Some((value, deviation)) =
                moments.add(typical_price, kline.volume.total().to_f64())
            {
                self.points.insert(
                    time,
                    VwapPoint {
                        value,
                        deviation,
                        session: point_session,
                    },
                );
            }
        }
    }

    fn draw_line(
        &self,
        frame: &mut canvas::Frame,
        chart: &ViewState,
        range: RangeInclusive<u64>,
        color: iced::Color,
        deviation_multiple: i8,
    ) {
        let mut previous = None;
        for (&time, point) in self
            .points
            .range(UnixMs::new(*range.start())..=UnixMs::new(*range.end()))
        {
            let price = point.value + f64::from(deviation_multiple) * point.deviation;
            let current = (
                chart.interval_to_x(time.as_u64()),
                chart.price_to_y(Price::from_f64(price)),
                point.session,
            );
            if let Some((x, y, session)) = previous
                && session == point.session
            {
                frame.stroke(
                    &canvas::Path::line(Point::new(x, y), Point::new(current.0, current.1)),
                    canvas::Stroke::with_color(
                        canvas::Stroke {
                            width: if deviation_multiple == 0 {
                                self.config.line_width.clamp(0.5, 5.0)
                            } else {
                                (self.config.line_width.clamp(0.5, 5.0) * 2.0 / 3.0).max(0.5)
                            } / chart.scaling,
                            ..canvas::Stroke::default()
                        },
                        color,
                    ),
                );
            }
            previous = Some(current);
        }
    }
}

impl KlineIndicatorImpl for SessionVwapIndicator {
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

    fn availability(&self, chart: &ViewState) -> IndicatorAvailability {
        if matches!(chart.basis, Basis::Time(_)) {
            IndicatorAvailability::Available
        } else {
            IndicatorAvailability::Unavailable(
                crate::chart::indicator::kline::AvailabilityCause::Basis(chart.basis),
            )
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
        visible_range: RangeInclusive<u64>,
    ) {
        let mut color = themed_color(
            theme,
            self.config.color,
            SessionVwapConfig::default().color,
            ThemeColor::Primary,
        );
        self.draw_line(frame, chart, visible_range.clone(), color, 0);
        color.a = 0.65;
        for band in 1..=self.config.deviation_bands.min(3) as i8 {
            self.draw_line(frame, chart, visible_range.clone(), color, band);
            self.draw_line(frame, chart, visible_range.clone(), color, -band);
        }
    }

    fn rebuild_from_source(&mut self, source: &PlotData<KlineDataPoint>) {
        self.rebuild_from(source, None);
    }

    fn on_insert_klines(&mut self, klines: &[Kline], source: &PlotData<KlineDataPoint>) {
        self.rebuild_from(source, klines.iter().map(|kline| kline.time).min());
    }

    fn on_basis_change(&mut self, source: &PlotData<KlineDataPoint>) {
        self.rebuild_from(source, None);
    }

    fn on_visual_config_change(&mut self, config: &Config, source: &PlotData<KlineDataPoint>) {
        self.config = config.session_vwap;
        self.rebuild_from(source, None);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weighted_moments_match_vwap_and_deviation() {
        let mut moments = WeightedMoments::default();
        moments.add(100.0, 1.0);
        let (vwap, deviation) = moments.add(110.0, 3.0).unwrap();

        assert_eq!(vwap, 107.5);
        assert!((deviation - 4.330_127).abs() < 0.000_001);

        let midnight = SessionVwapIndicator::new(SessionVwapConfig::default());
        assert_eq!(midnight.session_start(UnixMs::new(DAY_MS - 1)), 0);
        assert_eq!(midnight.session_start(UnixMs::new(DAY_MS)), DAY_MS);

        let anchored = SessionVwapIndicator::new(SessionVwapConfig {
            anchor_hour_utc: 8,
            ..SessionVwapConfig::default()
        });
        assert_eq!(
            anchored.session_start(UnixMs::new(DAY_MS * 2 + 7 * 3_600_000)),
            DAY_MS + 8 * 3_600_000
        );
    }
}
