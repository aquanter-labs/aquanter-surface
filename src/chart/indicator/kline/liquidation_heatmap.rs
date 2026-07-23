use std::collections::{BTreeMap, VecDeque};
use std::ops::RangeInclusive;

use data::chart::Basis;
use data::chart::PlotData;
use data::chart::kline::{KlineDataPoint, LiquidationHeatmapConfig};
use exchange::Liquidation;
use exchange::adapter::Venue;
use iced::widget::{Space, canvas};
use iced::{Element, Point, Theme};

use super::{
    AvailabilityCause, IndicatorAvailability, KlineIndicatorImpl, ThemeColor, themed_color,
};
use crate::chart::{Message, ViewState};

const MAX_EVENTS: usize = 10_000;
pub struct LiquidationHeatmapIndicator {
    config: LiquidationHeatmapConfig,
    events: VecDeque<Liquidation>,
}

impl LiquidationHeatmapIndicator {
    pub fn new(config: LiquidationHeatmapConfig) -> Self {
        Self {
            config,
            events: VecDeque::new(),
        }
    }
}

#[derive(Default)]
struct HeatBin {
    longs: f64,
    shorts: f64,
}

impl HeatBin {
    fn total(&self) -> f64 {
        self.longs + self.shorts
    }
}

fn heat_alpha(value: f64, max: f64, opacity: f32) -> f32 {
    if max <= 0.0 {
        0.0
    } else {
        (opacity.clamp(0.1, 1.0) * (0.17 + 0.83 * (value / max).sqrt()) as f32).min(1.0)
    }
}

impl KlineIndicatorImpl for LiquidationHeatmapIndicator {
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
        if !matches!(chart.basis, Basis::Time(_)) {
            IndicatorAvailability::Unavailable(AvailabilityCause::Basis(chart.basis))
        } else if chart.ticker_info.exchange().venue() != Venue::Binance {
            IndicatorAvailability::Unavailable(AvailabilityCause::Exchange(
                chart.ticker_info.exchange(),
            ))
        } else {
            IndicatorAvailability::Available
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
        visible: RangeInclusive<u64>,
    ) {
        let Basis::Time(timeframe) = chart.basis else {
            return;
        };
        if self.availability(chart) != IndicatorAvailability::Available {
            return;
        }
        let row_height = self.config.row_height.clamp(2.0, 24.0);

        let mut bins: BTreeMap<(u64, i32), HeatBin> = BTreeMap::new();
        for event in &self.events {
            let bucket = event.time.floor_to(timeframe).as_u64();
            if !visible.contains(&bucket) {
                continue;
            }
            let y = (chart.price_to_y(event.price) + chart.translation.y) * chart.scaling;
            let bin = bins
                .entry((bucket, (y / row_height).floor() as i32))
                .or_default();
            if event.is_sell {
                bin.longs += event.qty.to_f64();
            } else {
                bin.shorts += event.qty.to_f64();
            }
        }
        let max = bins.values().map(HeatBin::total).fold(0.0_f64, f64::max);
        if max <= 0.0 {
            return;
        }

        for ((time, row), bin) in bins {
            let total = bin.total();
            let width = chart.cell_width.max(1.0 / chart.scaling);
            let x = chart.interval_to_x(time) - width / 2.0;
            let y = row as f32 * row_height / chart.scaling - chart.translation.y;
            let height = row_height / chart.scaling;
            let long_width = width * (bin.longs / total) as f32;
            let defaults = LiquidationHeatmapConfig::default();
            let mut long_color = themed_color(
                theme,
                self.config.long_color,
                defaults.long_color,
                ThemeColor::Danger,
            );
            let mut short_color = themed_color(
                theme,
                self.config.short_color,
                defaults.short_color,
                ThemeColor::Success,
            );
            long_color.a = heat_alpha(bin.longs, max, self.config.opacity);
            short_color.a = heat_alpha(bin.shorts, max, self.config.opacity);
            if long_width > 0.0 {
                frame.fill_rectangle(
                    Point::new(x, y),
                    iced::Size::new(long_width, height),
                    long_color,
                );
            }
            if width - long_width > 0.0 {
                frame.fill_rectangle(
                    Point::new(x + long_width, y),
                    iced::Size::new(width - long_width, height),
                    short_color,
                );
            }
        }
    }

    fn on_liquidations(&mut self, liquidations: &[Liquidation]) {
        self.events.extend(liquidations.iter().copied());
        while self.events.len() > MAX_EVENTS {
            self.events.pop_front();
        }
    }

    fn on_visual_config_change(
        &mut self,
        config: &data::chart::kline::Config,
        _source: &PlotData<KlineDataPoint>,
    ) {
        self.config = config.liquidation_heatmap;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heat_intensity_is_normalized() {
        assert_eq!(heat_alpha(0.0, 0.0, 0.9), 0.0);
        assert!((heat_alpha(25.0, 100.0, 0.9) - 0.5265).abs() < 0.001);
        assert!((heat_alpha(100.0, 100.0, 0.9) - 0.9).abs() < 0.001);
    }
}
