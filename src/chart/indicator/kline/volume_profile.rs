use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use data::chart::kline::{
    Config, KlineDataPoint, VolumeProfileConfig, VolumeProfileMode, VolumeProfilePosition,
};
use data::chart::{Basis, PlotData};
use exchange::unit::Price;
use exchange::{Kline, UnixMs};
use iced::widget::{Space, canvas};
use iced::{Element, Point, Rectangle, Theme};

use super::{
    AvailabilityCause, IndicatorAvailability, KlineIndicatorImpl, ThemeColor, themed_color,
};
use crate::chart::{Message, ViewState};

#[derive(Clone, Copy)]
enum VolumeSample {
    Price {
        price: f64,
        buy: f64,
        sell: f64,
    },
    Range {
        low: f64,
        high: f64,
        buy: f64,
        sell: f64,
        neutral: f64,
    },
}

impl VolumeSample {
    fn bounds(self) -> (f64, f64) {
        match self {
            Self::Price { price, .. } => (price, price),
            Self::Range { low, high, .. } => (low, high),
        }
    }

    fn volume(self) -> f64 {
        match self {
            Self::Price { buy, sell, .. } => buy + sell,
            Self::Range {
                buy, sell, neutral, ..
            } => buy + sell + neutral,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct VolumeBin {
    buy: f64,
    sell: f64,
    neutral: f64,
}

impl VolumeBin {
    fn total(self) -> f64 {
        self.buy + self.sell + self.neutral
    }
}

fn volume_bins(values: &[VolumeSample], rows: usize) -> Option<(f64, f64, Vec<VolumeBin>)> {
    if values.is_empty() {
        return None;
    }
    let low = values
        .iter()
        .map(|sample| sample.bounds().0)
        .fold(f64::MAX, f64::min);
    let high = values
        .iter()
        .map(|sample| sample.bounds().1)
        .fold(f64::MIN, f64::max);
    let step = ((high - low) / rows as f64).max(f64::EPSILON);
    let mut bins = vec![VolumeBin::default(); rows];
    for &sample in values {
        match sample {
            VolumeSample::Price { price, buy, sell } => {
                let index = (((price - low) / step) as usize).min(rows - 1);
                bins[index].buy += buy;
                bins[index].sell += sell;
            }
            VolumeSample::Range {
                low: range_low,
                high: range_high,
                buy,
                sell,
                neutral,
            } => {
                let range = range_high - range_low;
                if range <= f64::EPSILON {
                    let index = (((range_low - low) / step) as usize).min(rows - 1);
                    bins[index].buy += buy;
                    bins[index].sell += sell;
                    bins[index].neutral += neutral;
                    continue;
                }
                for (index, bin) in bins.iter_mut().enumerate() {
                    let bin_low = low + index as f64 * step;
                    let bin_high = bin_low + step;
                    let overlap = range_high.min(bin_high) - range_low.max(bin_low);
                    if overlap > 0.0 {
                        let ratio = overlap / range;
                        bin.buy += buy * ratio;
                        bin.sell += sell * ratio;
                        bin.neutral += neutral * ratio;
                    }
                }
            }
        }
    }
    Some((low, step, bins))
}

fn profile_horizontal_bounds(
    region: Rectangle,
    position: VolumeProfilePosition,
) -> (f32, f32, bool) {
    match position {
        VolumeProfilePosition::Left => (region.x, region.width * 0.3, true),
        VolumeProfilePosition::Right => (region.x + region.width, region.width * 0.3, false),
    }
}

pub struct VolumeProfileIndicator {
    points: BTreeMap<UnixMs, Vec<VolumeSample>>,
    config: VolumeProfileConfig,
}

impl VolumeProfileIndicator {
    pub fn new(config: VolumeProfileConfig) -> Self {
        Self {
            points: BTreeMap::new(),
            config,
        }
    }

    fn rebuild(&mut self, source: &PlotData<KlineDataPoint>) {
        self.points.clear();
        let PlotData::TimeBased(series) = source else {
            return;
        };
        self.points
            .extend(series.datapoints.iter().map(|(&time, dp)| {
                let values = if dp.footprint.trades.is_empty() {
                    let low = dp.kline.low.to_f64();
                    let high = dp.kline.high.to_f64();
                    let (buy, sell, neutral) = dp.kline.volume.buy_sell().map_or_else(
                        || (0.0, 0.0, dp.kline.volume.total().to_f64()),
                        |(buy, sell)| (buy.to_f64(), sell.to_f64(), 0.0),
                    );
                    vec![VolumeSample::Range {
                        low,
                        high,
                        buy,
                        sell,
                        neutral,
                    }]
                } else {
                    dp.footprint
                        .trades
                        .iter()
                        .map(|(price, trades)| VolumeSample::Price {
                            price: price.to_f64(),
                            buy: trades.buy_qty.to_f64(),
                            sell: trades.sell_qty.to_f64(),
                        })
                        .collect()
                };
                (time, values)
            }));
    }
}

impl KlineIndicatorImpl for VolumeProfileIndicator {
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
        visible: RangeInclusive<u64>,
    ) {
        let range = match self.config.mode {
            VolumeProfileMode::VisibleRange => visible,
            VolumeProfileMode::FixedWindow => {
                let (Some(start), Some(end)) = (self.config.start, self.config.end) else {
                    return;
                };
                start.as_u64()..=end.as_u64()
            }
        };
        let values: Vec<_> = self
            .points
            .range(UnixMs::new(*range.start())..=UnixMs::new(*range.end()))
            .flat_map(|(_, values)| values.iter().copied())
            .filter(|sample| sample.volume() > 0.0)
            .collect();
        if values.is_empty() {
            return;
        }
        let rows = usize::from(self.config.rows.clamp(16, 400));
        let Some((low, step, bins)) = volume_bins(&values, rows) else {
            return;
        };
        let max = bins.iter().map(|bin| bin.total()).fold(0.0_f64, f64::max);
        if max <= 0.0 {
            return;
        }
        let visible_region = chart.visible_region(frame.size());
        let (anchor, max_width, grows_right) = match self.config.mode {
            VolumeProfileMode::VisibleRange => {
                profile_horizontal_bounds(visible_region, self.config.position)
            }
            VolumeProfileMode::FixedWindow => {
                let start_x = chart.interval_to_x(*range.start());
                let end_x = chart.interval_to_x(*range.end());
                let left = start_x.min(end_x) - chart.cell_width / 2.0;
                let right = start_x.max(end_x) + chart.cell_width / 2.0;
                profile_horizontal_bounds(
                    Rectangle {
                        x: left,
                        y: 0.0,
                        width: (right - left).max(chart.cell_width),
                        height: 0.0,
                    },
                    self.config.position,
                )
            }
        };
        let defaults = VolumeProfileConfig::default();
        let mut buy_color = themed_color(
            theme,
            self.config.buy_color,
            defaults.buy_color,
            ThemeColor::Success,
        );
        let mut sell_color = themed_color(
            theme,
            self.config.sell_color,
            defaults.sell_color,
            ThemeColor::Danger,
        );
        let mut neutral_color = themed_color(
            theme,
            self.config.color,
            defaults.color,
            ThemeColor::Secondary,
        );
        buy_color.a = 0.55;
        sell_color.a = 0.55;
        neutral_color.a = 0.45;
        for (index, bin) in bins.into_iter().enumerate() {
            let total = bin.total();
            if total <= 0.0 {
                continue;
            }
            let p0 = low + index as f64 * step;
            let p1 = p0 + step;
            let y0 = chart.price_to_y(Price::from_f64(p0));
            let y1 = chart.price_to_y(Price::from_f64(p1));
            let widths =
                [bin.sell, bin.neutral, bin.buy].map(|volume| max_width * (volume / max) as f32);
            let total_width = widths.iter().sum::<f32>();
            let mut x = if grows_right {
                anchor
            } else {
                anchor - total_width
            };
            for (width, color) in widths
                .into_iter()
                .zip([sell_color, neutral_color, buy_color])
            {
                if width > 0.0 {
                    frame.fill_rectangle(
                        Point::new(x, y0.min(y1)),
                        iced::Size::new(width, (y1 - y0).abs().max(1.0 / chart.scaling)),
                        color,
                    );
                    x += width;
                }
            }
        }

        if self.config.mode == VolumeProfileMode::FixedWindow {
            let top = chart.price_to_y(Price::from_f64(low + rows as f64 * step));
            let bottom = chart.price_to_y(Price::from_f64(low));
            let left = chart.interval_to_x(*range.start());
            let right = chart.interval_to_x(*range.end());
            let width = 1.0 / chart.scaling;
            neutral_color.a = 0.9;
            for x in [left, right] {
                frame.fill_rectangle(
                    Point::new(x - width / 2.0, top.min(bottom)),
                    iced::Size::new(width, (bottom - top).abs()),
                    neutral_color,
                );
            }
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

    fn on_visual_config_change(&mut self, config: &Config, _source: &PlotData<KlineDataPoint>) {
        self.config = config.volume_profile;
    }

    fn on_insert_trades(
        &mut self,
        _trades: &[exchange::Trade],
        _old_dp_len: usize,
        source: &PlotData<KlineDataPoint>,
    ) {
        self.rebuild(source);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bins_volume_and_keeps_highest_price_in_last_row() {
        let (_, _, bins) = volume_bins(
            &[
                VolumeSample::Price {
                    price: 100.0,
                    buy: 2.0,
                    sell: 0.0,
                },
                VolumeSample::Price {
                    price: 100.0,
                    buy: 0.0,
                    sell: 3.0,
                },
                VolumeSample::Price {
                    price: 110.0,
                    buy: 7.0,
                    sell: 0.0,
                },
            ],
            2,
        )
        .unwrap();
        assert_eq!(
            bins[0],
            VolumeBin {
                buy: 2.0,
                sell: 3.0,
                neutral: 0.0
            }
        );
        assert_eq!(
            bins[1],
            VolumeBin {
                buy: 7.0,
                sell: 0.0,
                neutral: 0.0
            }
        );

        let region = Rectangle {
            x: -250.0,
            y: 0.0,
            width: 400.0,
            height: 200.0,
        };
        let (right, width, grows_right) =
            profile_horizontal_bounds(region, VolumeProfilePosition::Right);
        assert_eq!(right, 150.0);
        assert!((width - 120.0).abs() < 0.001);
        assert!(!grows_right);

        let (left, _, grows_right) = profile_horizontal_bounds(region, VolumeProfilePosition::Left);
        assert_eq!(left, -250.0);
        assert!(grows_right);
    }

    #[test]
    fn distributes_candle_volume_continuously_across_rows() {
        let (_, _, bins) = volume_bins(
            &[VolumeSample::Range {
                low: 100.0,
                high: 110.0,
                buy: 0.0,
                sell: 0.0,
                neutral: 100.0,
            }],
            10,
        )
        .unwrap();

        assert!(
            bins.iter()
                .all(|bin| (bin.neutral - 10.0).abs() < 0.000_001)
        );
        assert!((bins.iter().map(|bin| bin.total()).sum::<f64>() - 100.0).abs() < 0.000_001);
    }
}
