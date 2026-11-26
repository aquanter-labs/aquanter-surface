use super::*;
use crate::ai::{ChartAnnotation, ChartSignal};
use crate::trading::{PositionOverlay, TradingLineId};
use data::chart::kline::indicators::pro::{VwapPoint, vwap_for_series};
use data::chart::kline::pro::VwapMode;
use data::i18n::tr;
use exchange::UnixMs;

struct TradingLine {
    id: Option<TradingLineId>,
    price: f64,
    tag: String,
    /// Estimated PnL if price reaches the line; shown only while it is dragged.
    pnl: Option<f64>,
    color: iced::Color,
}

fn trading_lines(overlay: &PositionOverlay, palette: &Extended) -> Vec<TradingLine> {
    let mut lines = Vec::new();
    let mut add = |price: Option<f64>, id, tag: &str, pnl, color| {
        if let Some(price) = price.filter(|p| p.is_finite() && *p > 0.0) {
            lines.push(TradingLine {
                id,
                price,
                tag: tag.to_string(),
                pnl,
                color,
            });
        }
    };
    let (position_price, position_tag) = if overlay.price_with_fees {
        (overlay.break_even_price, "BE")
    } else {
        (
            overlay.entry_price,
            tr(if overlay.is_long { "Long" } else { "Short" }),
        )
    };
    add(
        position_price,
        None,
        position_tag,
        None,
        palette.primary.base.color,
    );
    add(
        overlay.liquidation_price,
        None,
        "Liq",
        None,
        palette.danger.base.color,
    );
    add(
        overlay.stop_loss,
        Some(TradingLineId::StopLoss),
        "SL",
        overlay.stop_loss_pnl,
        palette.danger.base.color,
    );
    add(
        overlay.take_profit,
        Some(TradingLineId::TakeProfit),
        "TP",
        overlay.take_profit_pnl,
        palette.success.base.color,
    );
    for order in &overlay.orders {
        let buy = order.side == crate::trading::Side::Buy;
        add(
            Some(order.price),
            Some(order.id),
            tr(if buy { "Buy" } else { "Sell" }),
            None,
            if buy {
                palette.success.base.color
            } else {
                palette.danger.base.color
            },
        );
    }
    lines
}

struct PriceLine {
    price: f64,
    /// Short tag at the left edge (TP, SL, S, R ...).
    tag: String,
    /// Extra text beside the tag, e.g. the PnL while a line is dragged.
    detail: Option<String>,
    color: iced::Color,
    width: f32,
    /// Dashed so the candles under the line stay readable.
    dashed: bool,
    /// Trading lines get a filled tag; AI guides an outlined one.
    filled: bool,
}

const LINE_LABEL_SIZE: f32 = 11.0;
const LINE_LABEL_HEIGHT: f32 = 15.0;
const LINE_LABEL_GAP: f32 = 4.0;
const LINE_TAG_PAD: f32 = 3.0;

// ponytail: width estimated from the monospace advance (~0.62em, CJK ~1em).
fn label_width(content: &str) -> f32 {
    content
        .chars()
        .map(|c| if c.is_ascii() { 0.62 } else { 1.0 })
        .sum::<f32>()
        * LINE_LABEL_SIZE
}

/// Horizontal price lines with a short tag at the left edge; the price itself
/// goes on the price axis (`paint_axis_prices`). Tags at nearby prices step
/// right instead of stacking, and every line breaks around every tag it would
/// cross, so text never sits on a line.
fn paint_price_lines(
    frame: &mut PaintInputs,
    scene: &Scene,
    bounds: Rectangle,
    lines: &[PriceLine],
    palette: &Extended,
) {
    struct Placed<'a> {
        line: &'a PriceLine,
        y: f32,
        left: f32,
        right: f32,
    }
    let right_edge = bounds.x + bounds.width;
    let mut items = lines
        .iter()
        .filter_map(|line| {
            let y = price_y(scene, line.price)
                .filter(|y| *y >= bounds.y && *y <= bounds.y + bounds.height)?;
            Some((line, y))
        })
        .collect::<Vec<_>>();
    items.sort_by(|a, b| a.1.total_cmp(&b.1));

    let mut placed: Vec<Placed> = Vec::with_capacity(items.len());
    for (line, y) in items {
        let width = label_width(&line.tag)
            + LINE_TAG_PAD * 2.0
            + line
                .detail
                .as_deref()
                .map_or(0.0, |detail| label_width(detail) + LINE_LABEL_GAP);
        let mut left = bounds.x + LINE_LABEL_GAP;
        while let Some(blocker) = placed.iter().find(|other| {
            (other.y - y).abs() < LINE_LABEL_HEIGHT
                && other.left - LINE_LABEL_GAP < left + width
                && left < other.right + LINE_LABEL_GAP
        }) {
            left = blocker.right + LINE_LABEL_GAP;
        }
        placed.push(Placed {
            line,
            y,
            left,
            right: left + width,
        });
    }

    for item in &placed {
        let mut holes = placed
            .iter()
            .filter(|other| (other.y - item.y).abs() < LINE_LABEL_HEIGHT * 0.5 + 1.0)
            .map(|other| (other.left - LINE_LABEL_GAP, other.right + LINE_LABEL_GAP))
            .collect::<Vec<_>>();
        holes.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut stroke = canvas::Stroke::default()
            .with_color(if item.line.filled {
                item.line.color
            } else {
                item.line.color.scale_alpha(0.8)
            })
            .with_width(item.line.width);
        if item.line.dashed {
            stroke.line_dash = canvas::LineDash {
                segments: &[4.0, 3.0],
                offset: 0,
            };
        }
        let mut from = bounds.x;
        for (hole_left, hole_right) in holes.into_iter().chain([(right_edge, right_edge)]) {
            if hole_left > from {
                frame.stroke(
                    &canvas::Path::line(Point::new(from, item.y), Point::new(hole_left, item.y)),
                    stroke,
                );
            }
            from = from.max(hole_right);
        }
    }
    for item in placed {
        let tag_width = label_width(&item.line.tag) + LINE_TAG_PAD * 2.0;
        let tag = canvas::Path::rounded_rectangle(
            Point::new(item.left, item.y - LINE_LABEL_HEIGHT * 0.5),
            iced::Size::new(tag_width, LINE_LABEL_HEIGHT),
            2.0.into(),
        );
        let text_color = if item.line.filled {
            frame.fill(&tag, item.line.color);
            text_on(item.line.color)
        } else {
            frame.fill(&tag, palette.background.base.color.scale_alpha(0.85));
            frame.stroke(
                &tag,
                canvas::Stroke::default()
                    .with_color(item.line.color)
                    .with_width(1.0),
            );
            item.line.color
        };
        frame.fill_text(canvas::Text {
            content: item.line.tag.clone(),
            position: Point::new(item.left + tag_width * 0.5, item.y),
            align_x: iced::alignment::Horizontal::Center.into(),
            align_y: iced::alignment::Vertical::Center,
            color: text_color,
            size: LINE_LABEL_SIZE.into(),
            font: style::AZERET_MONO,
            ..Default::default()
        });
        if let Some(detail) = &item.line.detail {
            frame.fill_text(canvas::Text {
                content: detail.clone(),
                position: Point::new(item.left + tag_width + LINE_LABEL_GAP, item.y),
                align_x: iced::alignment::Horizontal::Left.into(),
                align_y: iced::alignment::Vertical::Center,
                color: item.line.color,
                size: LINE_LABEL_SIZE.into(),
                font: style::AZERET_MONO,
                ..Default::default()
            });
        }
    }
}

/// Each line's price as a colored badge on the price axis.
fn paint_axis_prices(
    frame: &mut PaintInputs,
    scene: &Scene,
    bounds: Rectangle,
    lines: &[PriceLine],
) {
    let axis = scene.layout.regions.y_axis;
    frame.with_clip(axis, |frame| {
        for line in lines {
            let Some(y) = price_y(scene, line.price)
                .filter(|y| *y >= bounds.y && *y <= bounds.y + bounds.height)
            else {
                continue;
            };
            let fill = if line.filled {
                line.color
            } else {
                line.color.scale_alpha(0.7)
            };
            frame.fill_rectangle(
                Point::new(axis.x, y - LINE_LABEL_HEIGHT * 0.5),
                iced::Size::new(axis.width, LINE_LABEL_HEIGHT),
                fill,
            );
            frame.fill_text(canvas::Text {
                content: scene.format_primary_cursor_label(line.price as f32),
                position: Point::new(axis.x + 4.0, y),
                align_x: iced::alignment::Horizontal::Left.into(),
                align_y: iced::alignment::Vertical::Center,
                color: text_on(fill),
                size: LINE_LABEL_SIZE.into(),
                font: style::AZERET_MONO,
                ..Default::default()
            });
        }
    });
}

/// Black or white, whichever reads better on `fill`.
fn text_on(fill: iced::Color) -> iced::Color {
    let luminance = 0.299 * fill.r + 0.587 * fill.g + 0.114 * fill.b;
    if luminance > 0.6 {
        iced::Color::from_rgb(0.08, 0.09, 0.11)
    } else {
        iced::Color::WHITE
    }
}

fn primary_bounds(scene: &Scene) -> Rectangle {
    let panel = scene.primary_plot();
    Rectangle {
        x: scene.layout.regions.plot.x + panel.x,
        y: scene.layout.regions.plot.y + panel.y,
        ..*panel
    }
}

fn tick_row(
    scene: &Scene,
    price: exchange::unit::Price,
    tick: exchange::unit::PriceStep,
) -> Option<(f32, f32)> {
    let anchor = scene.primary.anchor;
    let center = scene.map_primary_plot_with_anchor_f32(price.to_f32_lossy(), anchor);
    let height = super::footprint::row_height(scene, price, tick, anchor);
    if !center.is_finite() || !height.is_finite() || height <= 0.0 {
        return None;
    }
    Some((scene.layout.regions.plot.y + center - height * 0.5, height))
}

fn price_y(scene: &Scene, price: f64) -> Option<f32> {
    scene
        .map_primary_plot_with_anchor(price, scene.primary.anchor)
        .map(|y| scene.layout.regions.plot.y + y)
}

/// Bottom-docked Δ / Vol strip, one cell per bar, so labels never sit on the candles.
fn paint_footprint_summary(
    frame: &mut PaintInputs,
    plot: Rectangle,
    time_x: impl Fn(u64) -> Option<f32>,
    points: &[data::chart::kline::series::KlineSeriesPoint<'_>],
    config: &data::chart::kline::pro::ProConfig,
    palette: &Extended,
) {
    let cells = points
        .iter()
        .filter_map(|point| {
            let summary = point.footprint.trade_summary()?;
            let x = time_x(point.kline.time.as_u64())?;
            let (buy, sell) = (summary.buy_qty.to_f64(), summary.sell_qty.to_f64());
            Some((x, buy - sell, buy + sell))
        })
        .filter(|(x, ..)| *x >= plot.x - 50.0 && *x <= plot.x + plot.width + 50.0)
        .collect::<Vec<_>>();
    if cells.is_empty() {
        return;
    }
    let step = cells
        .windows(2)
        .map(|pair| (pair[1].0 - pair[0].0).abs())
        .filter(|width| *width > 0.0)
        .fold(f32::INFINITY, f32::min);
    let cell_width = if step.is_finite() { step } else { 60.0 };
    let font = (cell_width / 4.5).clamp(10.0, 13.0);
    let row = font + 6.0;
    let strip = Rectangle {
        x: plot.x,
        y: plot.y + plot.height - row * 2.0,
        width: plot.width,
        height: row * 2.0,
    };
    frame.fill_rectangle(
        strip.position(),
        strip.size(),
        palette.background.weakest.color.scale_alpha(0.94),
    );
    frame.stroke(
        &canvas::Path::line(strip.position(), Point::new(strip.x + strip.width, strip.y)),
        canvas::Stroke::default()
            .with_color(palette.background.strong.color)
            .with_width(1.0),
    );
    let max_delta = cells
        .iter()
        .map(|(_, delta, _)| delta.abs())
        .fold(0.0, f64::max)
        .max(f64::EPSILON);
    // ponytail: monospace width estimate (0.62em/char); measure glyphs if labels clip.
    let fits = |text: &str| text.chars().count() as f32 * font * 0.62 <= cell_width - 2.0;
    for (x, delta, volume) in cells {
        let left = x - cell_width / 2.0;
        let color = if delta >= 0.0 {
            palette.success.base.color
        } else {
            palette.danger.base.color
        };
        let strength = (delta.abs() / max_delta) as f32;
        frame.fill_rectangle(
            Point::new(left + 0.5, strip.y + 1.0),
            Size::new((cell_width - 1.0).max(1.0), row - 1.0),
            color.scale_alpha(0.08 + 0.32 * strength),
        );
        for (index, (text, text_color)) in [
            (config.quantity_format.format(delta), color),
            (
                config.quantity_format.format(volume),
                palette.background.base.text,
            ),
        ]
        .into_iter()
        .enumerate()
        {
            if !fits(&text) {
                continue;
            }
            frame.fill_text(canvas::Text {
                content: text,
                position: Point::new(x, strip.y + row * (index as f32 + 0.5)),
                align_x: iced::alignment::Horizontal::Center.into(),
                align_y: iced::alignment::Vertical::Center,
                size: font.into(),
                color: text_color,
                font: style::AZERET_MONO,
                ..Default::default()
            });
        }
    }
}

/// Smallest 1-2-5 multiplier whose rows reach 16px, given the current row height.
fn auto_tick_multiplier(current: u16, row_px: f32) -> u16 {
    const TARGET_PX: f64 = 16.0;
    const MAX: f64 = 10_000.0;
    let wanted = f64::from(current) * TARGET_PX / f64::from(row_px);
    let mut base = 1.0;
    loop {
        for mantissa in [1.0, 2.0, 5.0] {
            let value = mantissa * base;
            if value >= wanted || value >= MAX {
                return value.min(MAX) as u16;
            }
        }
        base *= 10.0;
    }
}

#[cfg(test)]
fn price_row_span(scene: &Scene, price: f64, tick: f64) -> Option<(f32, f32)> {
    let price = exchange::unit::Price::from_f64(price);
    let tick = exchange::unit::PriceStep {
        units: exchange::unit::Price::from_f64(tick).units,
    };
    tick_row(scene, price, tick)
}

fn price_at(scene: &Scene, position: Point) -> Option<f64> {
    let bounds = primary_bounds(scene);
    let ratio = scene.primary_value_ratio_at_plot_y(position.y - bounds.y);
    let (min, max) = scene.primary_domain_display_values();
    let price = f64::from(scene.primary_display_to_value(min + (max - min) * ratio));
    (price.is_finite() && price > 0.0).then_some(price)
}

impl<'a> KlineWidget<'a> {
    pub(crate) fn with_pro_market(
        mut self,
        config: &'a data::chart::kline::pro::ProConfig,
        depth: &'a exchange::depth::Depth,
    ) -> Self {
        self.pro_market = Some((config, depth));
        self
    }

    pub(crate) fn with_pro_liquidations(
        mut self,
        liquidations: &'a [exchange::Liquidation],
    ) -> Self {
        self.pro_liquidations = liquidations;
        self
    }

    /// Auto tick: keep the step while footprint rows are 10-40px, else jump to
    /// the smallest 1-2-5 multiplier giving rows of at least 16px.
    pub(super) fn pro_auto_tick(&self, scene: &Scene) -> Option<exchange::TickMultiplier> {
        let (config, _) = self.pro_market?;
        if !config.auto_tick_size || !self.footprint.visible {
            return None;
        }
        let series = self.series.first()?;
        let tick = series.tick_size();
        let (_, height) = tick_row(scene, series.latest_point()?.kline.close, tick)?;
        if !height.is_finite() || height <= 0.0 || (10.0..=40.0).contains(&height) {
            return None;
        }
        let min_units = exchange::unit::PriceStep::from(series.ticker_info.min_ticksize)
            .units
            .max(1);
        let current = (tick.units / min_units).max(1) as u16;
        let next = auto_tick_multiplier(current, height);
        (next != current).then_some(exchange::TickMultiplier(next))
    }

    pub(super) fn pro_main_bounds(&self, scene: &Scene) -> Rectangle {
        let bounds = primary_bounds(scene);
        let Some((config, _)) = self.pro_market else {
            return bounds;
        };
        if !self.footprint.visible {
            return bounds;
        }
        let left = if config.enabled.iter().any(|s| s == "VolumeProfile") {
            bounds.width * f32::from(config.volume_profile.max_width_percent.min(50)) / 100.0
        } else {
            0.0
        };
        let right = if config.is_enabled("OrderBook") && !config.order_book_transparent {
            super::order_book_strip_width(bounds.width)
        } else {
            0.0
        };
        Rectangle {
            x: bounds.x + left,
            width: (bounds.width - left - right).max(1.0),
            ..bounds
        }
    }

    pub(super) fn pro_panel_clips(&self, scene: &Scene, plot_local: bool) -> Vec<Rectangle> {
        scene
            .layout
            .panels
            .iter()
            .enumerate()
            .map(|(index, panel)| {
                let mut clip = if index == scene.primary.panel_index {
                    self.pro_main_bounds(scene)
                } else {
                    Rectangle {
                        x: scene.layout.regions.plot.x + panel.plot.x,
                        y: scene.layout.regions.plot.y + panel.plot.y,
                        ..panel.plot
                    }
                };
                if plot_local {
                    clip.x -= scene.layout.regions.plot.x;
                    clip.y -= scene.layout.regions.plot.y;
                }
                clip
            })
            .collect()
    }

    fn paint_pro_market(&self, frame: &mut PaintInputs, scene: &Scene, palette: &Extended) {
        let Some((config, depth)) = self.pro_market else {
            return;
        };
        let bounds = primary_bounds(scene);
        let main = self.pro_main_bounds(scene);
        let tick = self
            .series
            .first()
            .map(|series| series.tick_size())
            .unwrap_or(exchange::unit::PriceStep { units: 1 });
        if self.footprint.visible && config.is_enabled("OrderBook") {
            let width = super::order_book_strip_width(bounds.width);
            let book = Rectangle {
                x: bounds.x + bounds.width - width,
                width,
                ..bounds
            };
            let mut bids = std::collections::BTreeMap::<exchange::unit::Price, f64>::new();
            let mut asks = std::collections::BTreeMap::<exchange::unit::Price, f64>::new();
            for (price, qty) in &depth.bids {
                *bids
                    .entry(price.round_to_side_step(true, tick))
                    .or_default() += qty.to_f64();
            }
            for (price, qty) in &depth.asks {
                *asks
                    .entry(price.round_to_side_step(false, tick))
                    .or_default() += qty.to_f64();
            }
            let mut keys = bids.keys().chain(asks.keys()).copied().collect::<Vec<_>>();
            keys.sort_unstable();
            keys.dedup();
            let mut visible_qty = Vec::new();
            for price in &keys {
                let Some((top, height)) = tick_row(scene, *price, tick) else {
                    continue;
                };
                if top + height < book.y || top > book.y + book.height {
                    continue;
                }
                for qty in [
                    bids.get(price).copied().unwrap_or(0.0),
                    asks.get(price).copied().unwrap_or(0.0),
                ] {
                    if qty > 0.0 {
                        visible_qty.push(qty);
                    }
                }
            }
            let scale = visible_qty.into_iter().fold(0.0_f64, f64::max);
            if scale > 0.0 && book.width > 1.0 {
                frame.with_clip(book, |frame| {
                    if !config.order_book_transparent {
                        frame.fill_rectangle(
                            Point::new(book.x, book.y),
                            Size::new(book.width, book.height),
                            palette.background.weakest.color,
                        );
                    }
                    for price in keys {
                        let Some((top, height)) = tick_row(scene, price, tick) else {
                            continue;
                        };
                        if top + height < book.y || top > book.y + book.height {
                            continue;
                        }
                        let bid = bids.get(&price).copied().unwrap_or(0.0);
                        let ask = asks.get(&price).copied().unwrap_or(0.0);
                        let mut paint = |qty: f64, color: iced::Color| {
                            if qty <= 0.0 || height <= 0.0 {
                                return;
                            }
                            let width = ((qty / scale) as f32 * (book.width - 4.0)).max(1.0);
                            frame.fill_rectangle(
                                Point::new(book.x + book.width - width, top),
                                Size::new(width, height),
                                color.scale_alpha(0.28),
                            );
                            if height >= 10.0 {
                                frame.fill_text(canvas::Text {
                                    content: config.quantity_format.format(qty),
                                    position: Point::new(
                                        book.x + book.width - 4.0,
                                        top + height / 2.0,
                                    ),
                                    align_x: iced::alignment::Horizontal::Right.into(),
                                    align_y: iced::alignment::Vertical::Center,
                                    size: (height.min(12.0) - 2.0).into(),
                                    color,
                                    font: style::AZERET_MONO,
                                    ..Default::default()
                                });
                            }
                        };
                        if ask > 0.0 {
                            paint(ask, palette.danger.base.color);
                        }
                        if bid > 0.0 {
                            paint(bid, palette.success.base.color);
                        }
                    }
                });
            }
        }
        if !config.enabled.iter().any(|name| name == "VolumeProfile") {
            return;
        }
        let Some(series) = self.series.first() else {
            return;
        };
        let profile = config.volume_profile;
        let latest = series
            .latest_point()
            .map_or(0, |point| point.kline.time.as_u64());
        let visible_end = scene
            .time_for_x_unit(scene.x.max_unit)
            .map_or(latest, |time| time.as_u64());
        let end = match profile.past_duration_end {
            data::chart::kline::pro::VolumeProfilePastEnd::LatestData => latest,
            _ => visible_end,
        };
        let start = match profile.range {
            data::chart::kline::pro::VolumeProfileRange::Visible => scene
                .time_for_x_unit(scene.x.min_unit)
                .map_or(0, |time| time.as_u64()),
            data::chart::kline::pro::VolumeProfileRange::PastDuration => {
                let step = match self.basis {
                    Basis::Time(timeframe) => timeframe.to_milliseconds(),
                    _ => 60_000,
                };
                end.saturating_sub(
                    step.saturating_mul(u64::from(profile.timeframe_multiplier.max(1))),
                )
            }
            data::chart::kline::pro::VolumeProfileRange::Loaded => 0,
        };
        let end = if profile.range == data::chart::kline::pro::VolumeProfileRange::Loaded {
            latest
        } else {
            end
        };
        let mut points = series
            .points()
            .filter(|point| {
                let time = point.kline.time.as_u64();
                time >= start && time <= end
            })
            .collect::<Vec<_>>();
        if points.is_empty() {
            points = series.points().collect();
        }
        let samples = points
            .iter()
            .flat_map(|point| profile_samples(point))
            .collect::<Vec<_>>();
        // Footprint rows are one tick tall, so the profile follows them; Rows applies to candles.
        let bins = if self.footprint.visible {
            tick_profile_bins(&samples, tick.to_f64_lossy())
        } else {
            profile_bins(&samples, usize::from(profile.rows.clamp(16, 400)))
        };
        let Some((low, step, bins)) = bins else {
            return;
        };
        let delta = matches!(
            profile.display,
            data::chart::kline::pro::VolumeProfileDisplay::Delta
        );
        let value = |bin: &ProfileBin| {
            if delta {
                bin.buy - bin.sell
            } else {
                bin.buy + bin.sell + bin.neutral
            }
        };
        let max = bins.iter().map(value).map(f64::abs).fold(0.0, f64::max);
        let width = if self.footprint.visible {
            main.x - bounds.x
        } else {
            bounds.width * f32::from(profile.max_width_percent.min(50)) / 100.0
        };
        if width <= 0.0 {
            return;
        }
        let column = Rectangle { width, ..bounds };
        frame.with_clip(column, |frame| {
            frame.fill_rectangle(
                Point::new(column.x, column.y),
                Size::new(column.width, column.height),
                palette.background.weakest.color,
            );
            frame.fill_rectangle(
                Point::new(column.x + column.width - 1.0, column.y),
                Size::new(1.0, column.height),
                palette.background.strong.color.scale_alpha(0.25),
            );
            if max <= 0.0 {
                return;
            }
            for (index, bin) in bins.iter().enumerate() {
                let signed = value(bin);
                if signed.abs() <= f64::EPSILON {
                    continue;
                }
                let row_low = low + index as f64 * step;
                let row = if self.footprint.visible {
                    tick_row(
                        scene,
                        exchange::unit::Price::from_f64(row_low + step / 2.0),
                        tick,
                    )
                } else {
                    price_y(scene, row_low)
                        .zip(price_y(scene, row_low + step))
                        .map(|(y0, y1)| (y0.min(y1), (y1 - y0).abs().max(1.0)))
                };
                let Some((top, height)) = row else {
                    continue;
                };
                if delta {
                    let color = if signed < 0.0 {
                        palette.danger.base.color
                    } else {
                        palette.success.base.color
                    };
                    frame.fill_rectangle(
                        Point::new(column.x, top),
                        Size::new((signed.abs() / max) as f32 * width, height),
                        color.scale_alpha(0.75),
                    );
                } else {
                    let mut x = column.x;
                    for (qty, color) in [
                        (bin.sell, palette.danger.base.color),
                        (bin.neutral, palette.secondary.base.color),
                        (bin.buy, palette.success.base.color),
                    ] {
                        let bar = (qty / max) as f32 * width;
                        if bar > 0.0 {
                            frame.fill_rectangle(
                                Point::new(x, top),
                                Size::new(bar, height),
                                color.scale_alpha(0.75),
                            );
                            x += bar;
                        }
                    }
                }
                paint_profile_qty(
                    frame,
                    config,
                    column.x,
                    top,
                    height,
                    width,
                    signed,
                    palette.background.base.text,
                );
            }
        });
    }

    pub(crate) fn with_pro_overlays(
        mut self,
        annotations: &'a [ChartAnnotation],
        trading: Option<&'a PositionOverlay>,
    ) -> Self {
        self.pro_annotations = annotations;
        self.pro_trading = trading;
        self
    }

    /// The candle holding `time`: its open time, high and low.
    fn pro_bar_at(&self, time: u64) -> Option<(u64, f64, f64)> {
        let time = UnixMs::new(time);
        let point = self
            .series
            .first()?
            .points()
            .rfind(|p| p.kline.time <= time)?;
        Some((
            point.kline.time.as_u64(),
            point.kline.high.to_f64(),
            point.kline.low.to_f64(),
        ))
    }

    fn pro_time_x(&self, scene: &Scene, time: u64) -> Option<f32> {
        let series = self.series.first()?;
        let time = UnixMs::new(time);
        let unit = match scene.x.axis {
            XAxis::Time { .. } => scene.x.axis.unit_from_time(time),
            XAxis::Tick { .. } => {
                let index = series.points().rfind(|p| p.kline.time <= time)?.index;
                series.bar_unit_at_index(scene.x.axis, index)?
            }
        };
        Some(scene.layout.regions.plot.x + scene.map_x_plot(unit))
    }

    pub(super) fn pro_line_hit(
        &self,
        scene: &Scene,
        position: Point,
    ) -> Option<(TradingLineId, f64)> {
        if !self.pro_main_bounds(scene).contains(position) {
            return None;
        }
        let overlay = self.pro_trading.filter(|overlay| overlay.draggable)?;
        let mut candidates = vec![
            (TradingLineId::StopLoss, overlay.stop_loss),
            (TradingLineId::TakeProfit, overlay.take_profit),
        ];
        candidates.extend(overlay.orders.iter().map(|o| (o.id, Some(o.price))));
        candidates
            .into_iter()
            .filter_map(|(id, price)| {
                let price = price?;
                let distance = (price_y(scene, price)? - position.y).abs();
                (distance <= 7.0).then_some((id, price, distance))
            })
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(id, price, _)| (id, price))
    }

    fn paint_pro_studies(&self, frame: &mut PaintInputs, scene: &Scene, palette: &Extended) {
        let Some((config, _)) = self.pro_market else {
            return;
        };
        let Some(series) = self.series.first() else {
            return;
        };
        let plot = self.pro_main_bounds(scene);
        let points = series.points().collect::<Vec<_>>();
        if points.is_empty() {
            return;
        }
        frame.with_clip(plot, |frame| {
            if config.is_enabled("SessionVwap") {
                let mode = match config.vwap_mode.unwrap_or(VwapMode::Session {
                    hour_utc: config.session_vwap.anchor_hour_utc,
                }) {
                    VwapMode::Visible { .. } => VwapMode::Visible {
                        time_ms: scene
                            .time_for_x_unit(scene.x.min_unit)
                            .map_or(0, |time| time.as_u64()),
                    },
                    mode => mode,
                };
                if !matches!(mode, VwapMode::Rolling { .. }) {
                    let vwap = vwap_for_series(series.points(), mode);
                    paint_vwap_line(
                        frame,
                        scene,
                        self,
                        &vwap,
                        config.session_vwap.color,
                        config.session_vwap.line_width,
                        config.session_vwap.deviation_bands,
                    );
                }
            }
            if config.is_enabled("AnchoredVwap") {
                // No anchor set: anchor at the first visible bar, like SessionVwap's Visible mode.
                let anchor = config
                    .anchored_vwap
                    .anchor
                    .map(|time| time.as_u64())
                    .or_else(|| {
                        scene
                            .time_for_x_unit(scene.x.min_unit)
                            .map(|time| time.as_u64())
                    });
                if let Some(anchor) = anchor {
                    let vwap =
                        vwap_for_series(series.points(), VwapMode::Anchor { time_ms: anchor });
                    paint_vwap_line(
                        frame,
                        scene,
                        self,
                        &vwap,
                        config.anchored_vwap.color,
                        config.anchored_vwap.line_width,
                        config.anchored_vwap.deviation_bands,
                    );
                }
            }
            if config.is_enabled("LiquidationHeatmap") && !self.pro_liquidations.is_empty() {
                let max = self
                    .pro_liquidations
                    .iter()
                    .map(|event| event.qty.to_f64())
                    .fold(0.0, f64::max);
                let height = config.liquidation_heatmap.row_height.max(1.0);
                if max > 0.0 {
                    for event in self.pro_liquidations {
                        let Some(x) = self.pro_time_x(scene, event.time.as_u64()) else {
                            continue;
                        };
                        let Some(y) = price_y(scene, event.price.to_f64()) else {
                            continue;
                        };
                        let color = if event.is_sell {
                            config.liquidation_heatmap.long_color
                        } else {
                            config.liquidation_heatmap.short_color
                        };
                        let width = (event.qty.to_f64() / max).clamp(0.05, 1.0) as f32 * 28.0;
                        frame.fill_rectangle(
                            Point::new(x - width / 2.0, y - height / 2.0),
                            Size::new(width, height),
                            color.scale_alpha(config.liquidation_heatmap.opacity),
                        );
                    }
                }
            }
            if config.show_footprint_summary && self.footprint.visible {
                paint_footprint_summary(
                    frame,
                    plot,
                    |time| self.pro_time_x(scene, time),
                    &points,
                    config,
                    palette,
                );
            }
            if config.is_enabled("FixedVolumeProfile") {
                paint_fixed_profiles(frame, scene, self, config, &points, palette);
            }
            self.paint_divergence_markers(frame, scene, series, &points, palette);
        });
    }

    fn paint_divergence_markers(
        &self,
        frame: &mut PaintInputs,
        scene: &Scene,
        series: &KlineSeries,
        points: &[data::chart::kline::series::KlineSeriesPoint<'_>],
        palette: &Extended,
    ) {
        let accents = self.theme.accents();
        let instances = self
            .indicator_instances
            .instances()
            .iter()
            .filter(|instance| instance.kind.id() == "pro.oi-delta-divergence");
        for instance in instances {
            let visible = self.composition.panels.iter().flat_map(|panel| &panel.layers).any(|layer| layer.visible
                && matches!(&layer.source, data::chart::kline::composition::LayerSource::IndicatorOutput { output, .. } if output.instance_id == instance.id));
            if !visible || self.indicator_is_unavailable(Some(instance.id)) {
                continue;
            }
            for point in points {
                let Some(source_index) = series.indicator_source_index_for_visual(point.index)
                else {
                    continue;
                };
                for (channel, bullish) in [("bullish", true), ("bearish", false)] {
                    if self
                        .indicator_host
                        .output_value_for(instance.id, channel, source_index)
                        .and_then(data::chart::kline::OutputValue::as_boolean)
                        != Some(true)
                    {
                        continue;
                    }
                    let price = if bullish {
                        point.kline.low
                    } else {
                        point.kline.high
                    };
                    let (Some(x), Some(y)) = (
                        self.pro_time_x(scene, point.kline.time.as_u64()),
                        price_y(scene, price.to_f64()),
                    ) else {
                        continue;
                    };
                    let color = instance.configured_color(channel).map_or(
                        palette.primary.base.color,
                        |color| {
                            crate::widget::resolve_theme_color(color, palette, accents.as_array())
                        },
                    );
                    paint_marker(
                        frame,
                        Point::new(x, if bullish { y + 4.0 } else { y - 4.0 }),
                        bullish,
                        color,
                    );
                }
            }
        }
    }

    pub(super) fn paint_pro_overlays(
        &self,
        frame: &mut PaintInputs,
        scene: &Scene,
        palette: &Extended,
        drag: Option<(TradingLineId, f64)>,
    ) {
        self.paint_pro_market(frame, scene, palette);
        self.paint_pro_studies(frame, scene, palette);
        let bounds = self.pro_main_bounds(scene);
        let mut axis_lines = Vec::new();
        frame.with_clip(bounds, |frame| {
            // Collected first so labels can be laid out together, then painted at once.
            let mut lines: Vec<PriceLine> = Vec::new();
            let mut guide = |price: f64, tag: &str, color: iced::Color| {
                lines.push(PriceLine {
                    price,
                    tag: tag.to_string(),
                    detail: None,
                    color,
                    width: 1.0,
                    dashed: true,
                    filled: false,
                });
            };
            let annotations = self
                .pro_annotations
                .iter()
                .filter(|a| a.is_valid())
                .collect::<Vec<_>>();
            if let Some(annotation) = annotations.last() {
                for level in &annotation.levels {
                    if level.is_support {
                        guide(level.price, "S", palette.success.base.color);
                    } else {
                        guide(level.price, "R", palette.danger.base.color);
                    }
                }
                for (price, tag) in [
                    (annotation.entry_low, "E"),
                    (annotation.entry_high, "E"),
                    (annotation.stop_loss, "SL"),
                ] {
                    if let Some(price) = price {
                        guide(price, tag, palette.primary.base.color);
                    }
                }
                for (index, target) in annotation.targets.iter().enumerate() {
                    guide(
                        *target,
                        &format!("T{}", index + 1),
                        palette.primary.base.color,
                    );
                }
            }
            for annotation in annotations {
                if !annotation.mark_time {
                    continue;
                }
                let Some(x) = annotation.time_ms.and_then(|t| self.pro_time_x(scene, t)) else {
                    continue;
                };
                let Some(price) = annotation.entry_low.or(annotation.entry_high) else {
                    continue;
                };
                let Some(y) = price_y(scene, price) else {
                    continue;
                };
                let color = match annotation.signal {
                    ChartSignal::Long => palette.success.base.color,
                    ChartSignal::Short => palette.danger.base.color,
                    ChartSignal::Neutral => palette.primary.base.color,
                };
                paint_marker(
                    frame,
                    Point::new(x, y),
                    annotation.signal != ChartSignal::Short,
                    color,
                );
            }
            if let Some(overlay) = self.pro_trading {
                for item in trading_lines(overlay, palette) {
                    let dragged = drag.filter(|(id, _)| item.id == Some(*id));
                    lines.push(PriceLine {
                        price: dragged.map_or(item.price, |(_, price)| price),
                        tag: item.tag,
                        detail: item
                            .pnl
                            .filter(|_| dragged.is_some())
                            .map(|v| format!("{v:+.2}")),
                        color: item.color,
                        width: 1.5,
                        dashed: true,
                        filled: true,
                    });
                }
            }
            paint_price_lines(frame, scene, bounds, &lines, palette);
            axis_lines = lines;
            if let Some(overlay) = self.pro_trading {
                let mut badged: Vec<(u64, bool)> = Vec::new();
                // Badges already on each candle side, so later ones stack outward.
                let mut stacked: Vec<(u64, bool)> = Vec::new();
                for marker in overlay.trade_history.iter().filter(|_| overlay.fill_badges) {
                    let buy = marker.side == crate::trading::Side::Buy;
                    let Some((bar_time, high, low)) = self.pro_bar_at(marker.time_ms) else {
                        continue;
                    };
                    // One badge per candle and side, however many fills it holds.
                    if badged.contains(&(bar_time, buy)) {
                        continue;
                    }
                    badged.push((bar_time, buy));
                    if let (Some(x), Some(y)) = (
                        self.pro_time_x(scene, bar_time),
                        price_y(scene, if buy { low } else { high }),
                    ) {
                        let stack = stacked.iter().filter(|k| **k == (bar_time, buy)).count();
                        stacked.push((bar_time, buy));
                        paint_fill_badge(
                            frame,
                            Point::new(x, y),
                            buy,
                            if buy { "B" } else { "S" },
                            stack,
                            if buy {
                                palette.success.base.color
                            } else {
                                palette.danger.base.color
                            },
                        );
                    }
                }
                for marker in overlay
                    .trade_history
                    .iter()
                    .filter(|_| !overlay.fill_badges)
                {
                    if let (Some(x), Some(y)) = (
                        self.pro_time_x(scene, marker.time_ms),
                        price_y(scene, marker.price),
                    ) {
                        let buy = marker.side == crate::trading::Side::Buy;
                        paint_marker(
                            frame,
                            Point::new(x, y),
                            buy,
                            if buy {
                                palette.success.base.color
                            } else {
                                palette.danger.base.color
                            },
                        );
                    }
                }
                // Event contracts: open price held across the contract window, result at expiry.
                let mut event_badged: Vec<(u64, bool, bool)> = Vec::new();
                for contract in &overlay.event_contracts {
                    // Settled: W/L badge on the expiry candle, below for Up and
                    // above for Down, like B/S.
                    if overlay.event_badges
                        && let Some(pnl) = contract.pnl()
                    {
                        let won = pnl > 0.0;
                        let below = contract.up;
                        let Some((bar_time, high, low)) =
                            self.pro_bar_at(contract.expiry_ms.saturating_sub(1))
                        else {
                            continue;
                        };
                        if event_badged.contains(&(bar_time, below, won)) {
                            continue;
                        }
                        event_badged.push((bar_time, below, won));
                        if let (Some(x), Some(y)) = (
                            self.pro_time_x(scene, bar_time),
                            price_y(scene, if below { low } else { high }),
                        ) {
                            let stack = stacked.iter().filter(|k| **k == (bar_time, below)).count();
                            stacked.push((bar_time, below));
                            paint_fill_badge(
                                frame,
                                Point::new(x, y),
                                below,
                                if won { "W" } else { "L" },
                                stack,
                                if won {
                                    palette.success.base.color
                                } else {
                                    palette.danger.base.color
                                },
                            );
                        }
                        continue;
                    }
                    let (Some(x0), Some(x1), Some(y)) = (
                        self.pro_time_x(scene, contract.open_time_ms),
                        self.pro_time_x(scene, contract.expiry_ms),
                        price_y(scene, contract.open_price),
                    ) else {
                        continue;
                    };
                    let alpha = if contract.is_open() { 1.0 } else { 0.55 };
                    let color = if contract.up {
                        palette.success.base.color
                    } else {
                        palette.danger.base.color
                    }
                    .scale_alpha(alpha);
                    let stroke = canvas::Stroke::default().with_color(color).with_width(1.5);
                    frame.stroke(
                        &canvas::Path::line(Point::new(x0, y), Point::new(x1, y)),
                        stroke,
                    );
                    frame.stroke(
                        &canvas::Path::line(Point::new(x1, y - 5.0), Point::new(x1, y + 5.0)),
                        stroke,
                    );
                    paint_marker(frame, Point::new(x0, y), contract.up, color);
                    let (label, label_color) = match contract.pnl() {
                        None => (
                            format!(
                                "{} {} {:.0}u",
                                if contract.up { "UP" } else { "DOWN" },
                                contract.duration,
                                contract.stake
                            ),
                            color,
                        ),
                        Some(pnl) => {
                            let result = if pnl > 0.0 {
                                palette.success.base.color
                            } else {
                                palette.danger.base.color
                            };
                            if let Some(settle_y) = contract
                                .settle_price
                                .and_then(|price| price_y(scene, price))
                            {
                                frame.fill(
                                    &canvas::Path::circle(Point::new(x1, settle_y), 3.0),
                                    result,
                                );
                            }
                            (format!("{pnl:+.0}"), result)
                        }
                    };
                    frame.fill_text(canvas::Text {
                        content: label,
                        position: Point::new(x1 + 4.0, y),
                        align_y: iced::alignment::Vertical::Center,
                        color: label_color,
                        size: 11.0.into(),
                        font: style::AZERET_MONO,
                        ..Default::default()
                    });
                }
            }
        });
        paint_axis_prices(frame, scene, bounds, &axis_lines);
    }

    pub(super) fn handle_pro_pointer<M: Clone + From<KlineWidgetEvent>>(
        &self,
        event: &Event,
        state: &mut State,
        layout: Layout<'_>,
        cursor: mouse::Cursor,
        shell: &mut Shell<'_, M>,
    ) -> bool {
        if state.pro_drag.is_some()
            && matches!(
                event,
                Event::Keyboard(keyboard::Event::KeyPressed {
                    key: keyboard::Key::Named(keyboard::key::Named::Escape),
                    ..
                })
            )
        {
            state.pro_drag = None;
            state.scheduler.request(RenderRequest::Interaction);
            shell.capture_event();
            shell.request_redraw();
            return true;
        }
        let Event::Mouse(mouse_event) = event else {
            return false;
        };
        if let Some((id, last_price)) = state.pro_drag {
            if matches!(
                mouse_event,
                mouse::Event::CursorMoved { .. }
                    | mouse::Event::ButtonReleased(mouse::Button::Left)
            ) {
                let price = cursor
                    .position()
                    .and_then(|p| {
                        let scene = self.cached_scene(state, layout, cursor)?;
                        price_at(
                            &scene,
                            Point::new(p.x - layout.bounds().x, p.y - layout.bounds().y),
                        )
                    })
                    .unwrap_or(last_price);
                let finished = matches!(mouse_event, mouse::Event::ButtonReleased(_));
                state.pro_drag = (!finished).then_some((id, price));
                shell.publish(M::from(KlineWidgetEvent::TradingLineDragged(
                    id, price, finished,
                )));
            }
            state.scheduler.request(RenderRequest::Interaction);
            shell.request_redraw();
            shell.capture_event();
            return true;
        }
        if !matches!(
            mouse_event,
            mouse::Event::ButtonPressed(mouse::Button::Left)
        ) || state.interaction.motion().is_some()
            || self.drawing_snapshot.active_tool != DrawingTool::Cursor
            || self.drawing_snapshot.drawing_draft.is_some()
        {
            return false;
        }
        let Some(position) = cursor.position_in(layout.bounds()) else {
            return false;
        };
        let Some(scene) = self.cached_scene(state, layout, cursor) else {
            return false;
        };
        // Native drawings retain priority when their handles or bodies are hit.
        if !matches!(scene.interaction.target, CursorTarget::Plot(_)) {
            return false;
        }
        if let Some(hit) = self.pro_line_hit(&scene, position) {
            state.pro_drag = Some(hit);
            shell.capture_event();
            shell.request_redraw();
            return true;
        }
        false
    }
}

fn paint_profile_qty(
    frame: &mut PaintInputs,
    config: &data::chart::kline::pro::ProConfig,
    x: f32,
    top: f32,
    height: f32,
    width: f32,
    value: f64,
    color: iced::Color,
) {
    if height < 12.0 || value.abs() <= f64::EPSILON || width < 28.0 {
        return;
    }
    frame.fill_text(canvas::Text {
        content: config.quantity_format.format(value),
        position: Point::new(x + 3.0, top + height / 2.0),
        align_x: iced::alignment::Horizontal::Left.into(),
        align_y: iced::alignment::Vertical::Center,
        size: (height.min(16.0) - 3.0).max(8.0).into(),
        color,
        font: style::AZERET_MONO,
        ..Default::default()
    });
}

fn paint_vwap_line(
    frame: &mut PaintInputs,
    scene: &Scene,
    widget: &KlineWidget<'_>,
    vwap: &[VwapPoint],
    color: iced::Color,
    width: f32,
    bands: u8,
) {
    let mut previous: Option<(f32, f64, f64, i64)> = None;
    let band_color = color.scale_alpha(0.45);
    for point in vwap {
        let (mean, deviation, key) = (point.value, point.deviation, point.session);
        if previous.is_some_and(|(.., pkey)| pkey != key) {
            previous = None;
        }
        let Some(x) = widget.pro_time_x(scene, point.time.as_u64()) else {
            continue;
        };
        let Some(y) = price_y(scene, mean) else {
            continue;
        };
        if let Some((px, prev_mean, prev_dev, pkey)) = previous
            && pkey == key
        {
            if let Some(py) = price_y(scene, prev_mean) {
                frame.stroke(
                    &canvas::Path::line(Point::new(px, py), Point::new(x, y)),
                    canvas::Stroke::default()
                        .with_color(color)
                        .with_width(width),
                );
            }
            for band in 1..=i32::from(bands.min(6)) {
                let scale = f64::from(band);
                if let (Some(y0), Some(y1)) = (
                    price_y(scene, prev_mean + prev_dev * scale),
                    price_y(scene, mean + deviation * scale),
                ) {
                    frame.stroke(
                        &canvas::Path::line(Point::new(px, y0), Point::new(x, y1)),
                        canvas::Stroke::default()
                            .with_color(band_color)
                            .with_width(1.0),
                    );
                }
                if let (Some(y0), Some(y1)) = (
                    price_y(scene, prev_mean - prev_dev * scale),
                    price_y(scene, mean - deviation * scale),
                ) {
                    frame.stroke(
                        &canvas::Path::line(Point::new(px, y0), Point::new(x, y1)),
                        canvas::Stroke::default()
                            .with_color(band_color)
                            .with_width(1.0),
                    );
                }
            }
        }
        previous = Some((x, mean, deviation, key));
    }
}

fn paint_fixed_profiles(
    frame: &mut PaintInputs,
    scene: &Scene,
    widget: &KlineWidget<'_>,
    config: &data::chart::kline::pro::ProConfig,
    points: &[data::chart::kline::series::KlineSeriesPoint<'_>],
    palette: &Extended,
) {
    let step_ms = match widget.basis {
        Basis::Time(timeframe) => timeframe.to_milliseconds(),
        _ => 60_000,
    };
    let duration = step_ms
        .saturating_mul(u64::from(
            config.fixed_volume_profile.timeframe_multiplier.max(1),
        ))
        .max(step_ms);
    let profile = config.fixed_volume_profile;
    let (Some(first), Some(last)) = (points.first(), points.last()) else {
        return;
    };
    let (first, last) = (first.kline.time.as_u64(), last.kline.time.as_u64());
    let visible = (
        scene
            .time_for_x_unit(scene.x.min_unit)
            .map_or(first, |time| time.as_u64()),
        scene
            .time_for_x_unit(scene.x.max_unit)
            .map_or(last, |time| time.as_u64()),
    );
    let Some((mut window, end)) = fixed_profile_windows(first, last, visible, duration) else {
        return;
    };
    while window <= end {
        let next = window.saturating_add(duration);
        let samples = points
            .iter()
            .filter(|point| {
                let time = point.kline.time.as_u64();
                time >= window && time < next
            })
            .collect::<Vec<_>>();
        let samples = samples
            .iter()
            .flat_map(|point| profile_samples(point))
            .collect::<Vec<_>>();
        let delta = matches!(
            profile.display,
            data::chart::kline::pro::VolumeProfileDisplay::Delta
        );
        if let Some((low, step, bins)) =
            profile_bins(&samples, usize::from(profile.rows.clamp(16, 400)))
            && let (Some(x0), Some(x1)) = (
                widget.pro_time_x(scene, window),
                widget.pro_time_x(scene, next.saturating_sub(1)),
            )
        {
            let value = |bin: &ProfileBin| {
                if delta {
                    bin.buy - bin.sell
                } else {
                    bin.buy + bin.sell + bin.neutral
                }
            };
            let max = bins.iter().map(value).map(f64::abs).fold(0.0, f64::max);
            let left = x0.min(x1);
            let width = (x1 - x0).abs().max(8.0)
                * f32::from(profile.max_width_percent.clamp(5, 100))
                / 100.0;
            for (index, bin) in bins.iter().enumerate() {
                let signed = value(bin);
                if max <= 0.0 || signed.abs() <= f64::EPSILON {
                    continue;
                }
                let price = low + index as f64 * step;
                let (Some(y0), Some(y1)) = (price_y(scene, price), price_y(scene, price + step))
                else {
                    continue;
                };
                let (top, height) = (y0.min(y1), (y1 - y0).abs().max(1.0));
                if delta {
                    let color = if signed < 0.0 {
                        palette.danger.base.color
                    } else {
                        palette.success.base.color
                    };
                    frame.fill_rectangle(
                        Point::new(left, top),
                        Size::new((signed.abs() / max) as f32 * width, height),
                        color.scale_alpha(0.35),
                    );
                } else {
                    let mut x = left;
                    for (qty, color) in [
                        (bin.sell, palette.danger.base.color),
                        (bin.neutral, palette.secondary.base.color),
                        (bin.buy, palette.success.base.color),
                    ] {
                        let bar = (qty / max) as f32 * width;
                        if bar > 0.0 {
                            frame.fill_rectangle(
                                Point::new(x, top),
                                Size::new(bar, height),
                                color.scale_alpha(0.35),
                            );
                            x += bar;
                        }
                    }
                }
            }
        }
        if next <= window {
            break;
        }
        window = next;
    }
}

const MAX_FIXED_PROFILES: u64 = 48;

/// First and last window starts to draw: the windows overlapping the visible
/// time range, keeping the most recent ones when more than the cap are visible.
fn fixed_profile_windows(
    first: u64,
    last: u64,
    visible: (u64, u64),
    duration: u64,
) -> Option<(u64, u64)> {
    let start = visible.0.min(visible.1).max(first);
    let end = visible.0.max(visible.1).min(last);
    if start > end {
        return None;
    }
    let last_window = end / duration * duration;
    let earliest = last_window.saturating_sub((MAX_FIXED_PROFILES - 1).saturating_mul(duration));
    Some(((start / duration * duration).max(earliest), last_window))
}

#[derive(Clone, Copy)]
enum ProfileSample {
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

#[derive(Clone, Copy, Debug, Default, PartialEq)]
struct ProfileBin {
    buy: f64,
    sell: f64,
    neutral: f64,
}

/// Footprint trades when loaded; otherwise the bar volume spread over its
/// range, kept neutral when the venue does not report direction.
fn profile_samples(point: &data::chart::kline::series::KlineSeriesPoint<'_>) -> Vec<ProfileSample> {
    if !point.footprint.trades.is_empty() {
        return point
            .footprint
            .trades
            .iter()
            .map(|(price, group)| ProfileSample::Price {
                price: price.to_f64(),
                buy: group.buy_qty.to_f64(),
                sell: group.sell_qty.to_f64(),
            })
            .collect();
    }
    let (buy, sell, neutral) = point.kline.volume.buy_sell().map_or_else(
        || (0.0, 0.0, point.kline.volume.total().to_f64()),
        |(buy, sell)| (buy.to_f64(), sell.to_f64(), 0.0),
    );
    vec![ProfileSample::Range {
        low: point.kline.low.to_f64(),
        high: point.kline.high.to_f64(),
        buy,
        sell,
        neutral,
    }]
}

fn sample_bounds(samples: &[ProfileSample]) -> Option<(f64, f64)> {
    let bounds = |sample: &ProfileSample| match *sample {
        ProfileSample::Price { price, .. } => (price, price),
        ProfileSample::Range { low, high, .. } => (low, high),
    };
    let low = samples
        .iter()
        .map(|sample| bounds(sample).0)
        .min_by(f64::total_cmp)?;
    let high = samples
        .iter()
        .map(|sample| bounds(sample).1)
        .max_by(f64::total_cmp)?;
    Some((low, high))
}

/// `rows` equal rows spanning the samples' price range.
fn profile_bins(samples: &[ProfileSample], rows: usize) -> Option<(f64, f64, Vec<ProfileBin>)> {
    let (low, high) = sample_bounds(samples)?;
    let rows = rows.max(1);
    let step = ((high - low) / rows as f64).max(f64::EPSILON);
    Some((low, step, bin_samples(samples, low, step, rows)))
}

/// One row per tick, centered on the tick prices like footprint rows.
fn tick_profile_bins(samples: &[ProfileSample], tick: f64) -> Option<(f64, f64, Vec<ProfileBin>)> {
    const MAX_ROWS: f64 = 20_000.0;
    let (low, high) = sample_bounds(samples)?;
    if !tick.is_finite() || tick <= 0.0 {
        return None;
    }
    let step = tick * ((high - low) / tick / MAX_ROWS).ceil().max(1.0);
    // Candle-only bounds can sit between ticks; snap to the footprint grid (floor_to_step).
    let snap = |price: f64| (price / step + 1e-9).floor() * step;
    let (low, high) = (snap(low), snap(high));
    let rows = ((high - low) / step).round() as usize + 1;
    let low = low - step / 2.0;
    Some((low, step, bin_samples(samples, low, step, rows)))
}

fn bin_samples(samples: &[ProfileSample], low: f64, step: f64, rows: usize) -> Vec<ProfileBin> {
    let index = |price: f64| (((price - low) / step).max(0.0) as usize).min(rows - 1);
    let mut bins = vec![ProfileBin::default(); rows];
    for sample in samples {
        match *sample {
            ProfileSample::Price { price, buy, sell } => {
                bins[index(price)].buy += buy;
                bins[index(price)].sell += sell;
            }
            ProfileSample::Range {
                low: range_low,
                high: range_high,
                buy,
                sell,
                neutral,
            } => {
                let range = range_high - range_low;
                if range <= f64::EPSILON {
                    let bin = &mut bins[index(range_low)];
                    bin.buy += buy;
                    bin.sell += sell;
                    bin.neutral += neutral;
                    continue;
                }
                for (row, bin) in bins
                    .iter_mut()
                    .enumerate()
                    .take(index(range_high) + 1)
                    .skip(index(range_low))
                {
                    let bin_low = low + row as f64 * step;
                    let overlap = range_high.min(bin_low + step) - range_low.max(bin_low);
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
    bins
}

/// Exchange-style fill badge: a rounded box with B / S, below the candle's
/// low for buys and above its high for sells, pointing at the candle.
/// Letter badge under (`below`) or over a candle; `stack` > 0 sits further
/// out, past the badges already on that side.
fn paint_fill_badge(
    frame: &mut PaintInputs,
    anchor: Point,
    below: bool,
    letter: &str,
    stack: usize,
    color: iced::Color,
) {
    const SIZE: f32 = 15.0;
    const GAP: f32 = 7.0;
    let dir = if below { 1.0 } else { -1.0 };
    let offset = GAP + SIZE / 2.0 + stack as f32 * (SIZE + 2.0);
    let center = Point::new(anchor.x, anchor.y + dir * offset);
    if stack == 0 {
        let base_y = center.y - dir * SIZE / 2.0;
        let pointer = canvas::Path::new(|p| {
            p.move_to(Point::new(anchor.x, anchor.y + dir * 2.0));
            p.line_to(Point::new(anchor.x - 3.5, base_y));
            p.line_to(Point::new(anchor.x + 3.5, base_y));
            p.close();
        });
        frame.fill(&pointer, color);
    }
    frame.fill(
        &canvas::Path::rounded_rectangle(
            Point::new(center.x - SIZE / 2.0, center.y - SIZE / 2.0),
            iced::Size::new(SIZE, SIZE),
            3.0.into(),
        ),
        color,
    );
    frame.fill_text(canvas::Text {
        content: letter.to_string(),
        position: center,
        align_x: iced::alignment::Horizontal::Center.into(),
        align_y: iced::alignment::Vertical::Center,
        color: iced::Color::WHITE,
        size: 10.0.into(),
        font: style::AZERET_MONO,
        ..Default::default()
    });
}

fn paint_marker(frame: &mut PaintInputs, point: Point, buy: bool, color: iced::Color) {
    let direction = if buy { 1.0 } else { -1.0 };
    let path = canvas::Path::new(|p| {
        p.move_to(point);
        p.line_to(Point::new(point.x - 4.0, point.y + direction * 8.0));
        p.line_to(Point::new(point.x + 4.0, point.y + direction * 8.0));
        p.close();
    });
    frame.fill(&path, color);
}

#[cfg(test)]
mod tests {
    use super::super::scene::{ChromeScene, SceneInteraction};
    use super::*;
    use data::chart::kline::{ChartStepMs, pro::ProConfig};

    #[test]
    fn auto_tick_lands_rows_in_the_readable_band() {
        assert_eq!(auto_tick_multiplier(1, 4.0), 5);
        assert_eq!(auto_tick_multiplier(10, 2.0), 100);
        assert_eq!(auto_tick_multiplier(100, 80.0), 20);
        assert_eq!(auto_tick_multiplier(1, 0.0001), 10_000);
        for (current, px) in [(1u16, 3.3f32), (25, 7.0), (50, 90.0), (200, 41.0)] {
            let next = auto_tick_multiplier(current, px);
            let rows = px * f32::from(next) / f32::from(current);
            assert!(
                (16.0..=40.0).contains(&rows),
                "{current}@{px}px -> {next} gives {rows}px"
            );
        }
    }

    #[test]
    fn fixed_profiles_follow_the_visible_range() {
        const HOUR: u64 = 3_600_000;
        let (first, last) = (0, 1_000 * HOUR);
        assert_eq!(
            fixed_profile_windows(first, last, (900 * HOUR + 5, 910 * HOUR), HOUR),
            Some((900 * HOUR, 910 * HOUR))
        );
        assert_eq!(
            fixed_profile_windows(first, last, (0, 500 * HOUR), HOUR),
            Some((453 * HOUR, 500 * HOUR))
        );
        assert_eq!(
            fixed_profile_windows(first, last, (2_000 * HOUR, 2_010 * HOUR), HOUR),
            None
        );
    }

    #[test]
    fn fixed_profile_bins_trades_at_their_prices() {
        let samples = [
            ProfileSample::Price {
                price: 100.0,
                buy: 8.0,
                sell: 2.0,
            },
            ProfileSample::Price {
                price: 101.0,
                buy: 0.0,
                sell: 1.0,
            },
        ];
        let (low, step, bins) = profile_bins(&samples, 2).unwrap();
        assert_eq!((low, step), (100.0, 0.5));
        assert_eq!(
            bins[0],
            ProfileBin {
                buy: 8.0,
                sell: 2.0,
                neutral: 0.0
            }
        );
        assert_eq!(
            bins[1],
            ProfileBin {
                buy: 0.0,
                sell: 1.0,
                neutral: 0.0
            }
        );

        let undirected = [ProfileSample::Range {
            low: 100.0,
            high: 102.0,
            buy: 0.0,
            sell: 0.0,
            neutral: 4.0,
        }];
        let (_, _, bins) = profile_bins(&undirected, 2).unwrap();
        assert_eq!(
            bins,
            vec![
                ProfileBin {
                    neutral: 2.0,
                    ..ProfileBin::default()
                };
                2
            ]
        );
        assert_eq!(profile_bins(&undirected, 16).unwrap().2.len(), 16);
    }

    #[test]
    fn footprint_profile_rows_center_on_ticks() {
        let samples = [
            ProfileSample::Price {
                price: 100.0,
                buy: 1.0,
                sell: 0.0,
            },
            ProfileSample::Price {
                price: 100.3,
                buy: 0.0,
                sell: 2.0,
            },
        ];
        let (low, step, bins) = tick_profile_bins(&samples, 0.1).unwrap();
        assert_eq!(bins.len(), 4);
        assert!((low + step / 2.0 - 100.0).abs() < 1e-9);
        assert_eq!(bins[0].buy, 1.0);
        assert_eq!(bins[3].sell, 2.0);
        assert_eq!(bins[1..3], [ProfileBin::default(); 2]);
    }

    #[test]
    fn mixed_history_profile_rows_stay_on_tick_grid() {
        let samples = [
            ProfileSample::Range {
                low: 100.03,
                high: 100.13,
                buy: 0.0,
                sell: 0.0,
                neutral: 1.0,
            },
            ProfileSample::Price {
                price: 100.2,
                buy: 3.0,
                sell: 0.0,
            },
        ];
        let (low, step, bins) = tick_profile_bins(&samples, 0.1).unwrap();
        let center = |index: usize| low + (index as f64 + 0.5) * step;
        let row = bins.iter().position(|bin| bin.buy == 3.0).unwrap();
        assert!((center(row) - 100.2).abs() < 1e-9);
        assert!((center(0) - 100.0).abs() < 1e-9);
    }

    fn scene(mode: PanelScaleMode, inverted: bool) -> Scene {
        let mut composition = ChartComposition::prototype_kline();
        let mut auxiliary = composition.panels[0].clone();
        auxiliary.id =
            data::chart::kline::composition::PanelId::Aux(uuid::Uuid::from_u64_pair(0, 2));
        auxiliary.base_layer = None;
        auxiliary.layers.clear();
        composition.panels.push(auxiliary);
        let stack = layout::build_panel_stack(
            composition.panels.len(),
            &[0.7],
            Size::new(600.0, 300.0),
            4.0,
        );
        let row = iced_layout::Node::with_children(
            Size::new(666.0, 300.0),
            vec![
                stack,
                iced_layout::Node::new(Size::new(66.0, 300.0)).move_to(Point::new(600.0, 0.0)),
            ],
        );
        let axis = iced_layout::Node::with_children(
            Size::new(666.0, 24.0),
            vec![
                iced_layout::Node::new(Size::new(600.0, 24.0)),
                iced_layout::Node::new(Size::new(66.0, 24.0)).move_to(Point::new(600.0, 0.0)),
            ],
        );
        let root = iced_layout::Node::with_children(
            Size::new(686.0, 354.0),
            vec![
                row.move_to(Point::new(10.0, 20.0)),
                axis.move_to(Point::new(10.0, 320.0)),
            ],
        );
        let layout = PanelLayoutTree::from_layout(Layout::new(&root), &composition.panels).unwrap();
        let scale = PanelValueScale::new(None, None);
        Scene {
            layout,
            x: XViewport {
                axis: XAxis::Time {
                    timeframe: exchange::Timeframe::M1,
                    anchor: UnixMs::new(1_500_000),
                },
                world: HorizontalWorld::new(20.0, 0.0).axis_at_right_unit(0.0),
                fractional_offset: 0.0,
                min_unit: -20,
                max_unit: 0,
            },
            bar_units: Arc::from([]),
            primary: PrimaryScale {
                panel_index: 0,
                domain: PanelDomainState::new(YUnitRange::new(
                    scale.value_to_unit(100.0),
                    scale.value_to_unit(200.0),
                )),
                display_domain_override: None,
                value_scale: scale,
                mark: MarkKind::Candle,
                mode,
                inverted,
                anchor: Some(100.0),
                value_step: Some(0.1),
                value_decimals: Some(1),
                series_percent_anchors: vec![],
            },
            indicator_panels: vec![],
            chrome: ChromeScene::default(),
            interaction: SceneInteraction::default(),
            x_right_inset: 0.0,
        }
    }

    #[test]
    fn pro_overlay_price_roundtrip_on_native_scales() {
        for mode in [
            PanelScaleMode::Absolute,
            PanelScaleMode::Logarithmic,
            PanelScaleMode::PercentFromBase,
        ] {
            for inverted in [false, true] {
                let scene = scene(mode, inverted);
                for price in [110.0, 150.0, 190.0] {
                    let y = price_y(&scene, price).unwrap();
                    let recovered = price_at(&scene, Point::new(100.0, y)).unwrap();
                    assert!(
                        (recovered - price).abs() < 0.001,
                        "{mode:?} inverted={inverted}: {recovered} != {price}"
                    );
                }
            }
        }
    }

    #[test]
    fn pro_overlay_shared_clip_preserves_lower_panel() {
        let scene = scene(PanelScaleMode::Absolute, false);
        let composition = ChartComposition::prototype_kline();
        let host = IndicatorHost::default();
        let instances = IndicatorInstanceStore::default();
        let theme = ThemeState::new(data::Theme(Theme::Dark));
        let mut config = ProConfig::default();
        config.enabled.push("VolumeProfile".into());
        config.enabled.push("OrderBook".into());
        let depth = exchange::depth::Depth::default();
        let widget = KlineWidget::new(
            &[],
            &host,
            Basis::Time(exchange::Timeframe::M1),
            &composition,
            &instances,
            IndicatorPresentationContext::new(
                data::chart::kline::indicators::IndicatorInputCoverage::default(),
                Arc::from([]),
            ),
            &theme,
        )
        .with_footprint_config(FootprintConfig {
            visible: true,
            ..Default::default()
        })
        .with_pro_market(&config, &depth);
        let clips = widget.pro_panel_clips(&scene, false);
        let bounds = primary_bounds(&scene);
        assert!(clips[0].x > bounds.x);
        assert!(clips[0].x + clips[0].width < bounds.x + bounds.width);
        let lower = scene.layout.panels[1].plot;
        assert_eq!(clips[1].width, lower.width);
        assert_eq!(clips[1].x, scene.layout.regions.plot.x + lower.x);

        // Without a background the book draws over the candles: no strip reserved.
        let transparent = ProConfig {
            order_book_transparent: true,
            ..config.clone()
        };
        let widget = widget.with_pro_market(&transparent, &depth);
        let clips = widget.pro_panel_clips(&scene, false);
        assert_eq!(clips[0].x + clips[0].width, bounds.x + bounds.width);
        let _ = ChartStepMs::from_u64(60_000);
    }

    #[test]
    fn adjacent_order_rows_use_tick_height() {
        let scene = scene(PanelScaleMode::Absolute, false);
        let (top, height) = price_row_span(&scene, 120.0, 5.0).unwrap();
        let (next_top, next_height) = price_row_span(&scene, 125.0, 5.0).unwrap();
        assert!(
            height < 18.0,
            "row height {height} still uses the overlapping cap"
        );
        let overlap_start = top.max(next_top);
        let overlap_end = (top + height).min(next_top + next_height);
        assert!(
            overlap_start + 0.5 >= overlap_end,
            "rows overlap by {}",
            overlap_end - overlap_start
        );
    }
}
