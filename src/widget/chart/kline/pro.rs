use super::*;
use crate::ai::{ChartAnnotation, ChartSignal};
use crate::trading::{PositionOverlay, TradingLineId};
use data::chart::kline::indicators::pro::{VwapPoint, vwap_for_series};
use data::chart::kline::pro::VwapMode;
use exchange::UnixMs;

struct TradingLine {
    id: Option<TradingLineId>,
    price: f64,
    label: String,
    color: iced::Color,
}

fn trading_lines(overlay: &PositionOverlay, palette: &Extended) -> Vec<TradingLine> {
    let mut lines = Vec::new();
    let mut add = |price: Option<f64>, id, label: String, color| {
        if let Some(price) = price.filter(|p| p.is_finite() && *p > 0.0) {
            lines.push(TradingLine { id, price, label, color });
        }
    };
    add(overlay.entry_price, None, format!("Entry  PnL {:.2}", overlay.unrealized_pnl), palette.primary.base.color);
    add(overlay.break_even_price, None, "Break Even".into(), palette.background.base.text);
    add(overlay.liquidation_price, None, if overlay.liquidation_estimated { "Liq (est.)" } else { "Liquidation" }.into(), palette.danger.base.color);
    add(overlay.stop_loss, Some(TradingLineId::StopLoss), overlay.stop_loss_pnl.map_or_else(|| "SL".into(), |v| format!("SL {v:.2}")), palette.danger.base.color);
    add(overlay.take_profit, Some(TradingLineId::TakeProfit), overlay.take_profit_pnl.map_or_else(|| "TP".into(), |v| format!("TP {v:.2}")), palette.success.base.color);
    for order in &overlay.orders {
        add(Some(order.price), Some(order.id), order.label.clone(), match order.side {
            crate::trading::Side::Buy => palette.success.base.color,
            crate::trading::Side::Sell => palette.danger.base.color,
        });
    }
    lines
}

fn primary_bounds(scene: &Scene) -> Rectangle {
    let panel = scene.primary_plot();
    Rectangle { x: scene.layout.regions.plot.x + panel.x, y: scene.layout.regions.plot.y + panel.y, ..*panel }
}

fn tick_row(scene: &Scene, price: exchange::unit::Price, tick: exchange::unit::PriceStep) -> Option<(f32, f32)> {
    let anchor = scene.primary.anchor;
    let center = scene.map_primary_plot_with_anchor_f32(price.to_f32_lossy(), anchor);
    let height = super::footprint::row_height(scene, price, tick, anchor);
    if !center.is_finite() || !height.is_finite() || height <= 0.0 {
        return None;
    }
    Some((scene.layout.regions.plot.y + center - height * 0.5, height))
}

fn price_y(scene: &Scene, price: f64) -> Option<f32> {
    scene.map_primary_plot_with_anchor(price, scene.primary.anchor).map(|y| scene.layout.regions.plot.y + y)
}

#[cfg(test)]
fn price_row_span(scene: &Scene, price: f64, tick: f64) -> Option<(f32, f32)> {
    let price = exchange::unit::Price::from_f64(price);
    let tick = exchange::unit::PriceStep { units: exchange::unit::Price::from_f64(tick).units };
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
    pub(crate) fn with_pro_market(mut self, config: &'a data::chart::kline::pro::ProConfig, depth: &'a exchange::depth::Depth) -> Self {
        self.pro_market = Some((config,depth));
        self
    }

    pub(crate) fn with_pro_liquidations(mut self, liquidations: &'a [exchange::Liquidation]) -> Self {
        self.pro_liquidations = liquidations;
        self
    }

    pub(super) fn pro_main_bounds(&self, scene: &Scene) -> Rectangle {
        let bounds = primary_bounds(scene);
        let Some((config,_)) = self.pro_market else { return bounds; };
        if !self.footprint.visible { return bounds; }
        let left = if config.enabled.iter().any(|s| s == "VolumeProfile") {
            bounds.width * f32::from(config.volume_profile.max_width_percent.min(50)) / 100.0
        } else { 0.0 };
        let right = if config.is_enabled("OrderBook") { super::order_book_strip_width(bounds.width) } else { 0.0 };
        Rectangle { x: bounds.x + left, width: (bounds.width-left-right).max(1.0), ..bounds }
    }

    pub(super) fn pro_panel_clips(&self, scene: &Scene, plot_local: bool) -> Vec<Rectangle> {
        scene.layout.panels.iter().enumerate().map(|(index,panel)| {
            let mut clip = if index == scene.primary.panel_index { self.pro_main_bounds(scene) }
                else { Rectangle { x: scene.layout.regions.plot.x+panel.plot.x, y: scene.layout.regions.plot.y+panel.plot.y, ..panel.plot } };
            if plot_local { clip.x -= scene.layout.regions.plot.x; clip.y -= scene.layout.regions.plot.y; }
            clip
        }).collect()
    }

    fn paint_pro_market(&self, frame: &mut PaintInputs, scene: &Scene, palette: &Extended) {
        let Some((config,depth)) = self.pro_market else { return; };
        let bounds = primary_bounds(scene);
        let main = self.pro_main_bounds(scene);
        let tick = self.series.first().map(|series| series.tick_size()).unwrap_or(exchange::unit::PriceStep { units: 1 });
        if self.footprint.visible && config.is_enabled("OrderBook") {
            let book = Rectangle { x: main.x + main.width, width: bounds.x+bounds.width-main.x-main.width, ..bounds };
            let mut bids = std::collections::BTreeMap::<exchange::unit::Price, f64>::new();
            let mut asks = std::collections::BTreeMap::<exchange::unit::Price, f64>::new();
            for (price, qty) in &depth.bids { *bids.entry(price.round_to_side_step(true, tick)).or_default() += qty.to_f64(); }
            for (price, qty) in &depth.asks { *asks.entry(price.round_to_side_step(false, tick)).or_default() += qty.to_f64(); }
            let mut keys = bids.keys().chain(asks.keys()).copied().collect::<Vec<_>>();
            keys.sort_unstable();
            keys.dedup();
            let mut visible_qty = Vec::new();
            for price in &keys {
                let Some((top, height)) = tick_row(scene, *price, tick) else { continue; };
                if top + height < book.y || top > book.y + book.height { continue; }
                for qty in [bids.get(price).copied().unwrap_or(0.0), asks.get(price).copied().unwrap_or(0.0)] {
                    if qty > 0.0 { visible_qty.push(qty); }
                }
            }
            let scale = visible_qty.into_iter().fold(0.0_f64, f64::max);
            if scale > 0.0 && book.width > 1.0 {
                frame.with_clip(book, |frame| {
                    frame.fill_rectangle(Point::new(book.x, book.y), Size::new(book.width, book.height), palette.background.weakest.color);
                    for price in keys {
                        let Some((top, height)) = tick_row(scene, price, tick) else { continue; };
                        if top + height < book.y || top > book.y + book.height { continue; }
                        let bid = bids.get(&price).copied().unwrap_or(0.0);
                        let ask = asks.get(&price).copied().unwrap_or(0.0);
                        let mut paint = |qty: f64, color: iced::Color| {
                            if qty <= 0.0 || height <= 0.0 { return; }
                            let width = ((qty / scale) as f32 * (book.width - 4.0)).max(1.0);
                            frame.fill_rectangle(Point::new(book.x + book.width - width, top), Size::new(width, height), color.scale_alpha(0.28));
                            if height >= 10.0 {
                                frame.fill_text(canvas::Text {
                                    content: config.quantity_format.format(qty),
                                    position: Point::new(book.x + book.width - 4.0, top + height / 2.0),
                                    align_x: iced::alignment::Horizontal::Right.into(),
                                    align_y: iced::alignment::Vertical::Center.into(),
                                    size: (height.min(12.0) - 2.0).into(),
                                    color,
                                    font: style::AZERET_MONO,
                                    ..Default::default()
                                });
                            }
                        };
                        if ask > 0.0 { paint(ask, palette.danger.base.color); }
                        if bid > 0.0 { paint(bid, palette.success.base.color); }
                    }
                });
            }
        }
        if !config.enabled.iter().any(|name| name == "VolumeProfile") { return; }
        let Some(series) = self.series.first() else { return; };
        let profile = config.volume_profile;
        let latest = series.latest_point().map_or(0, |point| point.kline.time.as_u64());
        let visible_end = scene.time_for_x_unit(scene.x.max_unit).map_or(latest, |time| time.as_u64());
        let end = match profile.past_duration_end { data::chart::kline::pro::VolumeProfilePastEnd::LatestData => latest, _ => visible_end };
        let start = match profile.range {
            data::chart::kline::pro::VolumeProfileRange::Visible => scene.time_for_x_unit(scene.x.min_unit).map_or(0, |time| time.as_u64()),
            data::chart::kline::pro::VolumeProfileRange::PastDuration => {
                let step = match self.basis { Basis::Time(timeframe) => timeframe.to_milliseconds(), _ => 60_000 };
                end.saturating_sub(step.saturating_mul(u64::from(profile.timeframe_multiplier.max(1))))
            }
        };
        let mut points = series.points().filter(|point| {
            let time = point.kline.time.as_u64();
            time >= start && time <= end
        }).collect::<Vec<_>>();
        if points.is_empty() {
            points = series.points().collect();
        }
        let mut buckets = std::collections::BTreeMap::<exchange::unit::Price, (f64, f64)>::new();
        for point in &points {
            for (price, group) in &point.footprint.trades {
                let entry = buckets.entry(price.floor_to_step(tick)).or_default();
                entry.0 += group.buy_qty.to_f64();
                entry.1 += group.sell_qty.to_f64();
            }
        }
        if buckets.values().all(|(buy, sell)| *buy + *sell <= 0.0) {
            for point in &points {
                let (buy, sell) = point.kline.volume.buy_sell().map(|(buy, sell)| (buy.to_f64(), sell.to_f64())).unwrap_or_else(|| {
                    let total = point.kline.volume.total().to_f64();
                    if point.kline.close >= point.kline.open { (total, 0.0) } else { (0.0, total) }
                });
                if buy + sell <= 0.0 { continue; }
                let mut price = exchange::unit::Price::from_f64(point.kline.low.to_f64()).floor_to_step(tick);
                let end = exchange::unit::Price::from_f64(point.kline.high.to_f64()).floor_to_step(tick);
                let mut count = 1u32;
                let mut cursor = price;
                while cursor < end {
                    let Some(next) = cursor.checked_add_steps(1, tick) else { break; };
                    cursor = next;
                    count += 1;
                    if count > 20_000 { break; }
                }
                let span = f64::from(count);
                for _ in 0..count {
                    let entry = buckets.entry(price).or_default();
                    entry.0 += buy / span;
                    entry.1 += sell / span;
                    let Some(next) = price.checked_add_steps(1, tick) else { break; };
                    price = next;
                }
            }
        }
        let delta = matches!(profile.display, data::chart::kline::pro::VolumeProfileDisplay::Delta);
        let value = |&(buy, sell): &(f64, f64)| if delta { buy - sell } else { buy + sell };
        let max = buckets.values().map(value).map(f64::abs).fold(0.0, f64::max);
        let width = if self.footprint.visible { main.x - bounds.x } else { bounds.width * f32::from(profile.max_width_percent.min(50)) / 100.0 };
        if width <= 0.0 { return; }
        let column = Rectangle { width, ..bounds };
        frame.with_clip(column, |frame| {
            frame.fill_rectangle(Point::new(column.x, column.y), Size::new(column.width, column.height), palette.background.weakest.color);
            frame.fill_rectangle(Point::new(column.x + column.width - 1.0, column.y), Size::new(1.0, column.height), palette.background.strong.color.scale_alpha(0.25));
            if max <= 0.0 { return; }
            for (price, bucket) in &buckets {
                let Some((top, height)) = tick_row(scene, *price, tick) else { continue; };
                let (buy, sell) = *bucket;
                if delta {
                    let signed = buy - sell;
                    if signed.abs() <= f64::EPSILON { continue; }
                    let color = if signed < 0.0 { palette.danger.base.color } else { palette.success.base.color };
                    frame.fill_rectangle(Point::new(column.x, top), Size::new((signed.abs() / max) as f32 * width, height), color.scale_alpha(0.75));
                    paint_profile_qty(frame, config, column.x, top, height, width, signed, palette.background.base.text);
                } else {
                    let mut x = column.x;
                    for (qty, color) in [(sell, palette.danger.base.color), (buy, palette.success.base.color)] {
                        let bar = (qty / max) as f32 * width;
                        if bar > 0.0 {
                            frame.fill_rectangle(Point::new(x, top), Size::new(bar, height), color.scale_alpha(0.75));
                            x += bar;
                        }
                    }
                    paint_profile_qty(frame, config, column.x, top, height, width, buy + sell, palette.background.base.text);
                }
            }
        });
    }

    pub(crate) fn with_pro_overlays(mut self, annotations: &'a [ChartAnnotation], trading: Option<&'a PositionOverlay>) -> Self {
        self.pro_annotations = annotations;
        self.pro_trading = trading;
        self
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

    pub(super) fn pro_line_hit(&self, scene: &Scene, position: Point) -> Option<(TradingLineId, f64)> {
        if !self.pro_main_bounds(scene).contains(position) { return None; }
        let overlay = self.pro_trading?;
        let mut candidates = vec![(TradingLineId::StopLoss, overlay.stop_loss), (TradingLineId::TakeProfit, overlay.take_profit)];
        candidates.extend(overlay.orders.iter().map(|o| (o.id,Some(o.price))));
        candidates.into_iter().filter_map(|(id,price)| {
            let price = price?;
            let distance = (price_y(scene, price)? - position.y).abs();
            (distance <= 7.0).then_some((id, price, distance))
        }).min_by(|a,b| a.2.total_cmp(&b.2)).map(|(id,price,_)| (id,price))
    }

    fn paint_pro_studies(&self, frame: &mut PaintInputs, scene: &Scene, palette: &Extended) {
        let Some((config, _)) = self.pro_market else { return; };
        let Some(series) = self.series.first() else { return; };
        let plot = self.pro_main_bounds(scene);
        let points = series.points().collect::<Vec<_>>();
        if points.is_empty() { return; }
        frame.with_clip(plot, |frame| {
            if config.is_enabled("SessionVwap") {
                let mode = config.vwap_mode.unwrap_or(VwapMode::Session { hour_utc: config.session_vwap.anchor_hour_utc });
                if !matches!(mode, VwapMode::Rolling { .. }) {
                    let vwap = vwap_for_series(series.points(), mode);
                    paint_vwap_line(frame, scene, self, &vwap, config.session_vwap.color, config.session_vwap.line_width, config.session_vwap.deviation_bands);
                }
            }
            if config.is_enabled("AnchoredVwap") {
                if let Some(anchor) = config.anchored_vwap.anchor.map(|time| time.as_u64()) {
                    let vwap = vwap_for_series(series.points(), VwapMode::Anchor { time_ms: anchor });
                    paint_vwap_line(frame, scene, self, &vwap, config.anchored_vwap.color, config.anchored_vwap.line_width, config.anchored_vwap.deviation_bands);
                }
            }
            if config.is_enabled("LiquidationHeatmap") && !self.pro_liquidations.is_empty() {
                let max = self.pro_liquidations.iter().map(|event| event.qty.to_f64()).fold(0.0, f64::max);
                let height = config.liquidation_heatmap.row_height.max(1.0);
                if max > 0.0 {
                    for event in self.pro_liquidations {
                        let Some(x) = self.pro_time_x(scene, event.time.as_u64()) else { continue; };
                        let Some(y) = price_y(scene, event.price.to_f64()) else { continue; };
                        let color = if event.is_sell { config.liquidation_heatmap.long_color } else { config.liquidation_heatmap.short_color };
                        let width = (event.qty.to_f64() / max).clamp(0.05, 1.0) as f32 * 28.0;
                        frame.fill_rectangle(Point::new(x - width / 2.0, y - height / 2.0), Size::new(width, height), color.scale_alpha(config.liquidation_heatmap.opacity));
                    }
                }
            }
            if config.show_footprint_summary && self.footprint.visible {
                for point in &points {
                    let Some(summary) = point.footprint.trade_summary() else { continue; };
                    let Some(x) = self.pro_time_x(scene, point.kline.time.as_u64()) else { continue; };
                    let Some(y) = price_y(scene, point.kline.low.to_f64()) else { continue; };
                    let delta = summary.buy_qty.to_f64() - summary.sell_qty.to_f64();
                    // Sit a full caption-gap outside the wick, not on the tip.
                    let outward = price_y(scene, point.kline.high.to_f64())
                        .map(|high_y| if y >= high_y { 1.0 } else { -1.0 })
                        .unwrap_or(1.0);
                    let align_y = if outward > 0.0 {
                        iced::alignment::Vertical::Top
                    } else {
                        iced::alignment::Vertical::Bottom
                    };
                    frame.fill_text(canvas::Text {
                        content: config.quantity_format.format(delta),
                        position: Point::new(x, y + outward * 16.0),
                        align_x: iced::alignment::Horizontal::Center.into(),
                        align_y: align_y.into(),
                        size: 9.0.into(),
                        color: if delta >= 0.0 { palette.success.base.color } else { palette.danger.base.color },
                        font: style::AZERET_MONO,
                        ..Default::default()
                    });
                }
            }
            if config.is_enabled("FixedVolumeProfile") {
                paint_fixed_profiles(frame, scene, self, config, &points, palette);
            }
        });
    }

    pub(super) fn paint_pro_overlays(&self, frame: &mut PaintInputs, scene: &Scene, palette: &Extended, drag: Option<(TradingLineId,f64)>) {
        self.paint_pro_market(frame,scene,palette);
        self.paint_pro_studies(frame, scene, palette);
        let bounds = self.pro_main_bounds(scene);
        frame.with_clip(bounds, |frame| {
            let line = |frame: &mut PaintInputs, price: f64, label: &str, color: iced::Color, width: f32| {
                let Some(y) = price_y(scene, price).filter(|y| *y >= bounds.y && *y <= bounds.y + bounds.height) else { return; };
                frame.stroke(&canvas::Path::line(Point::new(bounds.x,y),Point::new(bounds.x+bounds.width,y)), canvas::Stroke::default().with_color(color).with_width(width));
                frame.fill_text(canvas::Text { content: format!("{label} {}", scene.format_primary_cursor_label(price as f32)),
                    position: Point::new(bounds.x + bounds.width - 5.0,y-3.0), align_x: iced::alignment::Horizontal::Right.into(),
                    color, size: 11.0.into(), font: style::AZERET_MONO, ..Default::default() });
            };
            let annotations = self.pro_annotations.iter().filter(|a| a.is_valid()).collect::<Vec<_>>();
            if let Some(annotation) = annotations.last() {
                for level in &annotation.levels {
                    line(frame, level.price, if level.is_support { "AI Support" } else { "AI Resistance" },
                        if level.is_support { palette.success.base.color } else { palette.danger.base.color }, 1.0 + level.strength.clamp(0.0,1.0));
                }
                for (price,label) in [(annotation.entry_low,"AI Entry"),(annotation.entry_high,"AI Entry"),(annotation.stop_loss,"AI SL")] {
                    if let Some(price) = price { line(frame, price, label, palette.primary.base.color, 1.0); }
                }
                for target in &annotation.targets { line(frame,*target,"AI Target",palette.primary.base.color,1.0); }
            }
            for annotation in annotations {
                if !annotation.mark_time { continue; }
                let Some(x) = annotation.time_ms.and_then(|t| self.pro_time_x(scene,t)) else { continue; };
                let Some(price) = annotation.entry_low.or(annotation.entry_high) else { continue; };
                let Some(y) = price_y(scene,price) else { continue; };
                let color = match annotation.signal { ChartSignal::Long => palette.success.base.color, ChartSignal::Short => palette.danger.base.color, ChartSignal::Neutral => palette.primary.base.color };
                paint_marker(frame, Point::new(x,y), annotation.signal != ChartSignal::Short, color);
            }
            if let Some(overlay) = self.pro_trading {
                for mut item in trading_lines(overlay,palette) {
                    if let Some((id,price)) = drag && item.id == Some(id) { item.price = price; }
                    line(frame,item.price,&item.label,item.color,1.5);
                }
                for marker in &overlay.trade_history {
                    if let (Some(x),Some(y)) = (self.pro_time_x(scene,marker.time_ms),price_y(scene,marker.price)) {
                        let buy = marker.side == crate::trading::Side::Buy;
                        paint_marker(frame,Point::new(x,y),buy,if buy { palette.success.base.color } else { palette.danger.base.color });
                    }
                }
            }
        });
    }

    pub(super) fn handle_pro_pointer<M: Clone + From<KlineWidgetEvent>>(&self, event: &Event, state: &mut State, layout: Layout<'_>, cursor: mouse::Cursor, shell: &mut Shell<'_, M>) -> bool {
        if state.pro_drag.is_some() && matches!(event, Event::Keyboard(keyboard::Event::KeyPressed { key: keyboard::Key::Named(keyboard::key::Named::Escape), .. })) {
            state.pro_drag = None;
            state.scheduler.request(RenderRequest::Interaction);
            shell.capture_event();
            shell.request_redraw();
            return true;
        }
        let Event::Mouse(mouse_event) = event else { return false; };
        if let Some((id,last_price)) = state.pro_drag {
            if matches!(mouse_event, mouse::Event::CursorMoved { .. } | mouse::Event::ButtonReleased(mouse::Button::Left)) {
                let price = cursor.position().and_then(|p| {
                    let scene = self.cached_scene(state,layout,cursor)?;
                    price_at(&scene,Point::new(p.x-layout.bounds().x,p.y-layout.bounds().y))
                }).unwrap_or(last_price);
                let finished = matches!(mouse_event, mouse::Event::ButtonReleased(_));
                state.pro_drag = (!finished).then_some((id,price));
                shell.publish(M::from(KlineWidgetEvent::TradingLineDragged(id,price,finished)));
            }
            state.scheduler.request(RenderRequest::Interaction);
            shell.request_redraw();
            shell.capture_event();
            return true;
        }
        if !matches!(mouse_event, mouse::Event::ButtonPressed(mouse::Button::Left)) || state.interaction.motion().is_some() || self.drawing_snapshot.active_tool != DrawingTool::Cursor || self.drawing_snapshot.drawing_draft.is_some() { return false; }
        let Some(position) = cursor.position_in(layout.bounds()) else { return false; };
        let Some(scene) = self.cached_scene(state,layout,cursor) else { return false; };
        // Native drawings retain priority when their handles or bodies are hit.
        if !matches!(scene.interaction.target, CursorTarget::Plot(_)) { return false; }
        if let Some(hit) = self.pro_line_hit(&scene,position) {
            state.pro_drag = Some(hit);
            shell.capture_event();
            shell.request_redraw();
            return true;
        }
        false
    }
}

fn paint_profile_qty(frame: &mut PaintInputs, config: &data::chart::kline::pro::ProConfig, x: f32, top: f32, height: f32, width: f32, value: f64, color: iced::Color) {
    if height < 12.0 || value.abs() <= f64::EPSILON || width < 28.0 { return; }
    frame.fill_text(canvas::Text {
        content: config.quantity_format.format(value),
        position: Point::new(x + 3.0, top + height / 2.0),
        align_x: iced::alignment::Horizontal::Left.into(),
        align_y: iced::alignment::Vertical::Center.into(),
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
        let Some(x) = widget.pro_time_x(scene, point.time.as_u64()) else { continue; };
        let Some(y) = price_y(scene, mean) else { continue; };
        if let Some((px, prev_mean, prev_dev, pkey)) = previous && pkey == key {
            if let Some(py) = price_y(scene, prev_mean) {
                frame.stroke(&canvas::Path::line(Point::new(px, py), Point::new(x, y)), canvas::Stroke::default().with_color(color).with_width(width));
            }
            for band in 1..=i32::from(bands.min(6)) {
                let scale = f64::from(band);
                if let (Some(y0), Some(y1)) = (price_y(scene, prev_mean + prev_dev * scale), price_y(scene, mean + deviation * scale)) {
                    frame.stroke(&canvas::Path::line(Point::new(px, y0), Point::new(x, y1)), canvas::Stroke::default().with_color(band_color).with_width(1.0));
                }
                if let (Some(y0), Some(y1)) = (price_y(scene, prev_mean - prev_dev * scale), price_y(scene, mean - deviation * scale)) {
                    frame.stroke(&canvas::Path::line(Point::new(px, y0), Point::new(x, y1)), canvas::Stroke::default().with_color(band_color).with_width(1.0));
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
    let step_ms = match widget.basis { Basis::Time(timeframe) => timeframe.to_milliseconds(), _ => 60_000 };
    let duration = step_ms.saturating_mul(u64::from(config.fixed_volume_profile.timeframe_multiplier.max(1))).max(step_ms);
    let profile = config.fixed_volume_profile;
    let (Some(first), Some(last)) = (points.first(), points.last()) else { return; };
    let (first, last) = (first.kline.time.as_u64(), last.kline.time.as_u64());
    let visible = (
        scene.time_for_x_unit(scene.x.min_unit).map_or(first, |time| time.as_u64()),
        scene.time_for_x_unit(scene.x.max_unit).map_or(last, |time| time.as_u64()),
    );
    let Some((mut window, end)) = fixed_profile_windows(first, last, visible, duration) else { return; };
    while window <= end {
        let next = window.saturating_add(duration);
        let samples = points.iter().filter(|point| {
            let time = point.kline.time.as_u64();
            time >= window && time < next
        }).collect::<Vec<_>>();
        if samples.len() >= 2 {
            let low = samples.iter().map(|point| point.kline.low.to_f64()).fold(f64::MAX, f64::min);
            let high = samples.iter().map(|point| point.kline.high.to_f64()).fold(f64::MIN, f64::max);
            let rows = usize::from(profile.rows.clamp(8, 80));
            let step = ((high - low) / rows as f64).max(f64::EPSILON);
            let mut buckets = vec![(0.0, 0.0); rows];
            for point in &samples {
                let (buy, sell) = point.kline.volume.buy_sell().map(|(buy, sell)| (buy.to_f64(), sell.to_f64())).unwrap_or_else(|| {
                    let total = point.kline.volume.total().to_f64();
                    if point.kline.close >= point.kline.open { (total, 0.0) } else { (0.0, total) }
                });
                let index = (((point.kline.close.to_f64() - low) / step) as usize).min(rows - 1);
                buckets[index].0 += buy;
                buckets[index].1 += sell;
            }
            let max = buckets.iter().map(|(buy, sell)| buy + sell).fold(0.0, f64::max);
            if let (Some(x0), Some(x1)) = (widget.pro_time_x(scene, window), widget.pro_time_x(scene, next.saturating_sub(1))) && max > 0.0 {
                let left = x0.min(x1);
                let span = (x1 - x0).abs().max(8.0);
                for (index, (buy, sell)) in buckets.iter().enumerate() {
                    let price = low + index as f64 * step;
                    let Some(y) = price_y(scene, price) else { continue; };
                    let bar = ((*buy + *sell) / max) as f32 * span;
                    let color = if buy >= sell { palette.success.base.color } else { palette.danger.base.color };
                    frame.fill_rectangle(Point::new(left, y), Size::new(bar, 2.0), color.scale_alpha(0.35));
                }
            }
        }
        if next <= window { break; }
        window = next;
    }
}

const MAX_FIXED_PROFILES: u64 = 48;

/// First and last window starts to draw: the windows overlapping the visible
/// time range, keeping the most recent ones when more than the cap are visible.
fn fixed_profile_windows(first: u64, last: u64, visible: (u64, u64), duration: u64) -> Option<(u64, u64)> {
    let start = visible.0.min(visible.1).max(first);
    let end = visible.0.max(visible.1).min(last);
    if start > end { return None; }
    let last_window = end / duration * duration;
    let earliest = last_window.saturating_sub((MAX_FIXED_PROFILES - 1).saturating_mul(duration));
    Some(((start / duration * duration).max(earliest), last_window))
}

fn paint_marker(frame: &mut PaintInputs, point: Point, buy: bool, color: iced::Color) {
    let direction = if buy { 1.0 } else { -1.0 };
    let path = canvas::Path::new(|p| {
        p.move_to(point);
        p.line_to(Point::new(point.x-4.0,point.y+direction*8.0));
        p.line_to(Point::new(point.x+4.0,point.y+direction*8.0));
        p.close();
    });
    frame.fill(&path,color);
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::scene::{ChromeScene, SceneInteraction};
    use data::chart::kline::{ChartStepMs, pro::ProConfig};

    #[test]
    fn fixed_profiles_follow_the_visible_range() {
        const HOUR: u64 = 3_600_000;
        let (first, last) = (0, 1_000 * HOUR);
        assert_eq!(fixed_profile_windows(first, last, (900 * HOUR + 5, 910 * HOUR), HOUR), Some((900 * HOUR, 910 * HOUR)));
        assert_eq!(fixed_profile_windows(first, last, (0, 500 * HOUR), HOUR), Some((453 * HOUR, 500 * HOUR)));
        assert_eq!(fixed_profile_windows(first, last, (2_000 * HOUR, 2_010 * HOUR), HOUR), None);
    }


    fn scene(mode: PanelScaleMode, inverted: bool) -> Scene {
        let mut composition = ChartComposition::prototype_kline();
        let mut auxiliary = composition.panels[0].clone();
        auxiliary.id = data::chart::kline::composition::PanelId(2);
        auxiliary.role = data::chart::kline::composition::PanelRole::Auxiliary;
        auxiliary.base_layer = None;
        auxiliary.layers.clear();
        composition.panels.push(auxiliary);
        let stack = layout::build_panel_stack(composition.panels.len(),&[0.7],Size::new(600.0,300.0),4.0);
        let row = iced_layout::Node::with_children(Size::new(666.0,300.0),vec![stack,iced_layout::Node::new(Size::new(66.0,300.0)).move_to(Point::new(600.0,0.0))]);
        let axis = iced_layout::Node::with_children(Size::new(666.0,24.0),vec![iced_layout::Node::new(Size::new(600.0,24.0)),iced_layout::Node::new(Size::new(66.0,24.0)).move_to(Point::new(600.0,0.0))]);
        let root = iced_layout::Node::with_children(Size::new(686.0,354.0),vec![row.move_to(Point::new(10.0,20.0)),axis.move_to(Point::new(10.0,320.0))]);
        let layout = PanelLayoutTree::from_layout(Layout::new(&root),&composition.panels).unwrap();
        let scale = PanelValueScale::new(None,None);
        Scene {
            layout, x:XViewport { axis:XAxis::Time { timeframe:exchange::Timeframe::M1,anchor:UnixMs::new(1_500_000) },world:HorizontalWorld::new(20.0,0.0).axis_at_right_unit(0.0),fractional_offset:0.0,min_unit:-20,max_unit:0 },
            bar_units:Arc::from([]), primary:PrimaryScale { panel_index:0,domain:PanelDomainState::new(YUnitRange::new(scale.value_to_unit(100.0),scale.value_to_unit(200.0))),display_domain_override:None,value_scale:scale,mark:MarkKind::Candle,mode,inverted,anchor:Some(100.0),value_step:Some(0.1),value_decimals:Some(1),series_percent_anchors:vec![] },
            indicator_panels:vec![],chrome:ChromeScene::default(),interaction:SceneInteraction::default(),x_right_inset:0.0,
        }
    }

    #[test]
    fn pro_overlay_price_roundtrip_on_native_scales() {
        for mode in [PanelScaleMode::Absolute,PanelScaleMode::Logarithmic,PanelScaleMode::PercentFromBase] {
            for inverted in [false,true] {
                let scene = scene(mode,inverted);
                for price in [110.0,150.0,190.0] {
                    let y = price_y(&scene,price).unwrap();
                    let recovered = price_at(&scene,Point::new(100.0,y)).unwrap();
                    assert!((recovered-price).abs() < 0.001,"{mode:?} inverted={inverted}: {recovered} != {price}");
                }
            }
        }
    }

    #[test]
    fn pro_overlay_shared_clip_preserves_lower_panel() {
        let scene = scene(PanelScaleMode::Absolute,false);
        let composition = ChartComposition::prototype_kline();
        let host = IndicatorHost::default();
        let instances = IndicatorInstanceStore::default();
        let theme = ThemeState::new(data::Theme(Theme::Dark));
        let mut config = ProConfig::default();
        config.enabled.push("VolumeProfile".into());
        config.enabled.push("OrderBook".into());
        let depth = exchange::depth::Depth::default();
        let widget = KlineWidget::new(&[],&host,Basis::Time(exchange::Timeframe::M1),&composition,&instances,IndicatorPresentationContext::new(data::chart::kline::indicators::IndicatorInputCoverage::default(),Arc::from([])),&theme)
            .with_footprint_config(FootprintConfig { visible:true,..Default::default() }).with_pro_market(&config,&depth);
        let clips = widget.pro_panel_clips(&scene,false);
        let bounds = primary_bounds(&scene);
        assert!(clips[0].x > bounds.x);
        assert!(clips[0].x+clips[0].width < bounds.x+bounds.width);
        let lower = scene.layout.panels[1].plot;
        assert_eq!(clips[1].width,lower.width);
        assert_eq!(clips[1].x,scene.layout.regions.plot.x+lower.x);
        let _ = ChartStepMs::from_u64(60_000);
    }

    #[test]
    fn adjacent_order_rows_use_tick_height() {
        let scene = scene(PanelScaleMode::Absolute, false);
        let (top, height) = price_row_span(&scene, 120.0, 5.0).unwrap();
        let (next_top, next_height) = price_row_span(&scene, 125.0, 5.0).unwrap();
        assert!(height < 18.0, "row height {height} still uses the overlapping cap");
        let overlap_start = top.max(next_top);
        let overlap_end = (top + height).min(next_top + next_height);
        assert!(overlap_start + 0.5 >= overlap_end, "rows overlap by {}", overlap_end - overlap_start);
    }
}
