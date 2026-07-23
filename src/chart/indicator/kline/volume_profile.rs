use std::cell::Cell;
use std::collections::BTreeMap;
use std::ops::RangeInclusive;

use data::chart::kline::{
    Config, KlineDataPoint, QuantityFormat, VolumeProfileConfig, VolumeProfileDisplay,
    VolumeProfilePastEnd, VolumeProfileRange,
};
use data::chart::{Basis, PlotData};
use exchange::unit::Price;
use exchange::{Kline, UnixMs};
use iced::widget::{Space, canvas};
use iced::{Alignment, Element, Point, Rectangle, Theme};

use super::{AvailabilityCause, IndicatorAvailability, KlineIndicatorImpl};
use crate::chart::{Message, ViewState};
use crate::style;

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

    fn value(self, display: VolumeProfileDisplay) -> f64 {
        match display {
            VolumeProfileDisplay::Volume => self.total(),
            VolumeProfileDisplay::Delta => self.buy - self.sell,
        }
    }
}

fn bin_price_bounds(low: f64, step: f64, index: usize, centered: bool) -> (f64, f64) {
    let price = low + index as f64 * step;
    if centered {
        (price - step / 2.0, price + step / 2.0)
    } else {
        (price, price + step)
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

fn volume_bins_by_step(
    values: &[VolumeSample],
    visible_low: f64,
    visible_high: f64,
    step: f64,
) -> Option<(f64, f64, Vec<VolumeBin>)> {
    if values.is_empty()
        || !visible_low.is_finite()
        || !visible_high.is_finite()
        || !step.is_finite()
        || step <= 0.0
        || visible_high <= visible_low
    {
        return None;
    }

    let low = (visible_low / step).floor() * step;
    let high = (visible_high / step).ceil() * step;
    let rows = (((high - low) / step).round() as usize).max(1);
    let mut bins = vec![VolumeBin::default(); rows];

    for &sample in values {
        match sample {
            VolumeSample::Price { price, buy, sell }
                if price >= visible_low && price <= visible_high =>
            {
                let index = (((price - low) / step).floor() as usize).min(rows - 1);
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
                if (range_high - range_low).abs() <= f64::EPSILON {
                    if range_low >= visible_low && range_low <= visible_high {
                        let index = (((range_low - low) / step).floor() as usize).min(rows - 1);
                        bins[index].buy += buy;
                        bins[index].sell += sell;
                        bins[index].neutral += neutral;
                    }
                    continue;
                }
                let clipped_low = range_low.max(visible_low);
                let clipped_high = range_high.min(visible_high);
                let range = range_high - range_low;
                if clipped_high <= clipped_low {
                    continue;
                }

                let first = (((clipped_low - low) / step).floor() as usize).min(rows - 1);
                let last = (((clipped_high - low) / step).floor() as usize).min(rows - 1);
                for (index, bin) in bins.iter_mut().enumerate().take(last + 1).skip(first) {
                    let bin_low = low + index as f64 * step;
                    let bin_high = bin_low + step;
                    let overlap = clipped_high.min(bin_high) - clipped_low.max(bin_low);
                    if overlap > 0.0 {
                        let ratio = overlap / range;
                        bin.buy += buy * ratio;
                        bin.sell += sell * ratio;
                        bin.neutral += neutral * ratio;
                    }
                }
            }
            VolumeSample::Price { .. } => {}
        }
    }

    Some((low, step, bins))
}

fn profile_horizontal_bounds(region: Rectangle, max_width_percent: u8) -> (f32, f32) {
    let max_width = region.width * f32::from(max_width_percent.clamp(5, 50)) / 100.0;
    (region.x, max_width)
}

fn horizontal_ranges_overlap(left_a: f32, right_a: f32, left_b: f32, right_b: f32) -> bool {
    right_a > left_b && left_a < right_b
}

fn aligned_window_start(timestamp: u64, duration_ms: u64, timezone: data::UserTimezone) -> u64 {
    let offset_ms = chrono::DateTime::from_timestamp_millis(timestamp as i64)
        .map(|datetime| {
            i64::from(
                timezone
                    .to_user_datetime(datetime)
                    .offset()
                    .local_minus_utc(),
            ) * 1_000
        })
        .unwrap_or(0);
    let duration_ms = duration_ms.max(1) as i64;
    let local_timestamp = timestamp as i64 + offset_ms;
    (local_timestamp.div_euclid(duration_ms) * duration_ms - offset_ms).max(0) as u64
}

fn profile_duration_ms(timeframe: exchange::Timeframe, multiplier: u8) -> u64 {
    timeframe
        .to_milliseconds()
        .saturating_mul(u64::from(multiplier.max(1)))
}

fn past_duration_end(visible_end: u64, latest: Option<u64>, end: VolumeProfilePastEnd) -> u64 {
    match end {
        VolumeProfilePastEnd::ChartRight => visible_end,
        VolumeProfilePastEnd::LatestData => latest.unwrap_or(visible_end),
    }
}

pub struct VolumeProfileIndicator {
    points: BTreeMap<UnixMs, Vec<VolumeSample>>,
    config: VolumeProfileConfig,
    quantity_format: QuantityFormat,
    is_fixed: bool,
    is_footprint: bool,
    timezone: Cell<data::UserTimezone>,
}

impl VolumeProfileIndicator {
    pub fn new(
        config: VolumeProfileConfig,
        quantity_format: QuantityFormat,
        is_fixed: bool,
        is_footprint: bool,
    ) -> Self {
        Self {
            points: BTreeMap::new(),
            config,
            quantity_format,
            is_fixed,
            is_footprint,
            timezone: Cell::new(data::UserTimezone::Utc),
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

    fn draw_profile(
        &self,
        frame: &mut canvas::Frame,
        chart: &ViewState,
        theme: &Theme,
        values: &[VolumeSample],
        horizontal_region: Rectangle,
        visible_region: Rectangle,
    ) {
        let profile = if self.is_footprint {
            let (highest, lowest) = chart.price_range(&visible_region);
            volume_bins_by_step(
                values,
                lowest.to_f64(),
                highest.to_f64(),
                chart.tick_size.to_f64_lossy(),
            )
        } else {
            volume_bins(values, usize::from(self.config.rows.clamp(16, 400)))
        };
        let Some((low, step, bins)) = profile else {
            return;
        };
        let max = bins
            .iter()
            .map(|bin| bin.value(self.config.display).abs())
            .fold(0.0_f64, f64::max);
        if max <= 0.0 {
            return;
        }

        let (anchor, max_width) =
            profile_horizontal_bounds(horizontal_region, self.config.max_width_percent);
        let palette = theme.extended_palette();
        let (background_low, _) = bin_price_bounds(low, step, 0, self.is_footprint);
        let (_, background_high) = bin_price_bounds(low, step, bins.len() - 1, self.is_footprint);
        let background_top = chart.price_to_y(Price::from_f64(background_high));
        let background_bottom = chart.price_to_y(Price::from_f64(background_low));
        frame.fill_rectangle(
            Point::new(anchor, background_top.min(background_bottom)),
            iced::Size::new(max_width, (background_bottom - background_top).abs()),
            palette.background.weakest.color,
        );
        let divider_width = 1.0 / chart.scaling;
        frame.fill_rectangle(
            Point::new(
                anchor + max_width - divider_width / 2.0,
                background_top.min(background_bottom),
            ),
            iced::Size::new(divider_width, (background_bottom - background_top).abs()),
            palette.background.strong.color.scale_alpha(0.25),
        );
        let buy_color = palette.success.base.color.scale_alpha(0.28);
        let sell_color = palette.danger.base.color.scale_alpha(0.28);
        let neutral_color = palette.secondary.base.color.scale_alpha(0.28);

        for (index, bin) in bins.into_iter().enumerate() {
            let value = bin.value(self.config.display);
            if value.abs() <= f64::EPSILON {
                continue;
            }
            let (p0, p1) = bin_price_bounds(low, step, index, self.is_footprint);
            let y0 = chart.price_to_y(Price::from_f64(p0));
            let y1 = chart.price_to_y(Price::from_f64(p1));
            let row_height = (y1 - y0).abs().max(1.0 / chart.scaling);
            match self.config.display {
                VolumeProfileDisplay::Volume => {
                    let widths = [bin.sell, bin.neutral, bin.buy]
                        .map(|volume| max_width * (volume / max) as f32);
                    let mut x = anchor;
                    for (width, color) in
                        widths
                            .into_iter()
                            .zip([sell_color, neutral_color, buy_color])
                    {
                        if width > 0.0 {
                            frame.fill_rectangle(
                                Point::new(x, y0.min(y1)),
                                iced::Size::new(width, row_height),
                                color,
                            );
                            x += width;
                        }
                    }
                }
                VolumeProfileDisplay::Delta => {
                    let width = max_width * (value.abs() / max) as f32;
                    frame.fill_rectangle(
                        Point::new(anchor, y0.min(y1)),
                        iced::Size::new(width, row_height),
                        if value >= 0.0 { buy_color } else { sell_color },
                    );
                }
            }

            let row_height_px = (y1 - y0).abs() * chart.scaling;
            let label = self.quantity_format.format(value);
            let text_size =
                crate::chart::kline::footprint_cluster_text_size(row_height_px) / chart.scaling;
            if row_height_px >= 12.0
                && crate::chart::kline::footprint_text_fits(
                    &label,
                    text_size,
                    max_width - 6.0 / chart.scaling,
                )
            {
                frame.fill_text(canvas::Text {
                    content: label,
                    position: Point::new(anchor + 3.0 / chart.scaling, (y0 + y1) / 2.0),
                    size: iced::Pixels(text_size),
                    color: palette.background.weakest.text,
                    align_x: Alignment::Start.into(),
                    align_y: Alignment::Center.into(),
                    font: style::AZERET_MONO,
                    ..canvas::Text::default()
                });
            }
        }
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
        let visible_region = chart.visible_region(frame.size());
        let Basis::Time(timeframe) = chart.basis else {
            return;
        };
        let duration_ms = profile_duration_ms(timeframe, self.config.timeframe_multiplier);
        if self.is_fixed {
            let timezone = self.timezone.get();
            let first_window = aligned_window_start(*visible.start(), duration_ms, timezone);
            let mut windows: BTreeMap<u64, Vec<VolumeSample>> = BTreeMap::new();
            for (&time, values) in self
                .points
                .range(UnixMs::new(first_window)..=UnixMs::new(*visible.end()))
            {
                let start = aligned_window_start(time.as_u64(), duration_ms, timezone);
                windows.entry(start).or_default().extend(
                    values
                        .iter()
                        .copied()
                        .filter(|sample| sample.volume() > 0.0),
                );
            }
            for (start, values) in windows {
                let end = start.saturating_add(duration_ms);
                let left = chart.interval_to_x(start) - chart.cell_width / 2.0;
                let right = chart.interval_to_x(end) - chart.cell_width / 2.0;
                self.draw_profile(
                    frame,
                    chart,
                    theme,
                    &values,
                    Rectangle {
                        x: left.min(right),
                        y: 0.0,
                        width: (right - left).abs().max(chart.cell_width),
                        height: 0.0,
                    },
                    visible_region,
                );
            }
        } else {
            let end = past_duration_end(
                *visible.end(),
                self.points.last_key_value().map(|(time, _)| time.as_u64()),
                self.config.past_duration_end,
            );
            let start = if self.config.range == VolumeProfileRange::PastDuration {
                end.saturating_sub(duration_ms)
            } else {
                *visible.start()
            };
            let values: Vec<_> = self
                .points
                .range(UnixMs::new(start)..=UnixMs::new(end))
                .flat_map(|(_, values)| values.iter().copied())
                .filter(|sample| sample.volume() > 0.0)
                .collect();
            self.draw_profile(frame, chart, theme, &values, visible_region, visible_region);
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
        self.config = if self.is_fixed {
            config.fixed_volume_profile
        } else {
            config.volume_profile
        };
        self.quantity_format = config.quantity_format;
    }

    fn on_timezone_change(&self, timezone: data::UserTimezone) {
        self.timezone.set(timezone);
    }

    fn obscures_footprint_cell(
        &self,
        interval: u64,
        cell_x: f32,
        chart: &ViewState,
        visible_region: Rectangle,
    ) -> bool {
        let horizontal_region = if self.is_fixed {
            let Basis::Time(timeframe) = chart.basis else {
                return false;
            };
            let duration_ms = profile_duration_ms(timeframe, self.config.timeframe_multiplier);
            let start = aligned_window_start(interval, duration_ms, self.timezone.get());
            let left = chart.interval_to_x(start) - chart.cell_width / 2.0;
            let right =
                chart.interval_to_x(start.saturating_add(duration_ms)) - chart.cell_width / 2.0;
            Rectangle {
                x: left.min(right),
                y: 0.0,
                width: (right - left).abs().max(chart.cell_width),
                height: 0.0,
            }
        } else {
            visible_region
        };
        let (profile_left, profile_width) =
            profile_horizontal_bounds(horizontal_region, self.config.max_width_percent);
        let cell_left = cell_x - chart.cell_width / 2.0;
        let cell_right = cell_x + chart.cell_width / 2.0;
        horizontal_ranges_overlap(
            cell_left,
            cell_right,
            profile_left,
            profile_left + profile_width,
        )
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
        let (left, width) = profile_horizontal_bounds(region, 30);
        assert_eq!(left, -250.0);
        assert!((width - 120.0).abs() < 0.001);
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

    #[test]
    fn footprint_bins_use_tick_step_and_clip_to_visible_prices() {
        let (low, step, bins) = volume_bins_by_step(
            &[
                VolumeSample::Price {
                    price: 99.5,
                    buy: 10.0,
                    sell: 0.0,
                },
                VolumeSample::Price {
                    price: 100.5,
                    buy: 2.0,
                    sell: 3.0,
                },
                VolumeSample::Price {
                    price: 102.5,
                    buy: 0.0,
                    sell: 10.0,
                },
            ],
            100.0,
            102.0,
            0.5,
        )
        .unwrap();

        assert_eq!(low, 100.0);
        assert_eq!(step, 0.5);
        assert_eq!(bins.len(), 4);
        assert_eq!(bins[1].buy, 2.0);
        assert_eq!(bins[1].sell, 3.0);
        assert_eq!(bins.iter().map(|bin| bin.total()).sum::<f64>(), 5.0);
    }

    #[test]
    fn footprint_rows_are_centered_on_their_price_and_delta_keeps_its_sign() {
        assert_eq!(bin_price_bounds(100.0, 0.5, 1, true), (100.25, 100.75));

        let positive = VolumeBin {
            buy: 8.0,
            sell: 3.0,
            neutral: 2.0,
        };
        let negative = VolumeBin {
            buy: 2.0,
            sell: 7.0,
            neutral: 4.0,
        };
        assert_eq!(positive.value(VolumeProfileDisplay::Delta), 5.0);
        assert_eq!(negative.value(VolumeProfileDisplay::Delta), -5.0);
        assert!(horizontal_ranges_overlap(0.0, 10.0, 8.0, 20.0));
        assert!(!horizontal_ranges_overlap(0.0, 10.0, 10.0, 20.0));
    }

    #[test]
    fn fixed_windows_align_to_the_configured_timezone() {
        assert_eq!(
            aligned_window_start(3_900_000, 3_600_000, data::UserTimezone::Utc),
            3_600_000
        );
        assert_eq!(
            profile_duration_ms(exchange::Timeframe::Custom(10), 25),
            15_000_000
        );
        assert_eq!(
            past_duration_end(
                100,
                Some(200),
                data::chart::kline::VolumeProfilePastEnd::LatestData
            ),
            200
        );
    }
}
