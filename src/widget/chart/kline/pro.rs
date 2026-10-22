use super::*;
use crate::ai::{ChartAnnotation, ChartSignal};
use crate::trading::{PositionOverlay, TradingLineId};
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

fn price_y(scene: &Scene, price: f64) -> Option<f32> {
    scene.map_primary_plot_with_anchor(price, scene.primary.anchor).map(|y| scene.layout.regions.plot.y + y)
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

    pub(super) fn pro_main_bounds(&self, scene: &Scene) -> Rectangle {
        let bounds = primary_bounds(scene);
        let Some((config,_)) = self.pro_market else { return bounds; };
        if !self.footprint.visible { return bounds; }
        let left = if config.enabled.iter().any(|s| s == "VolumeProfile") {
            bounds.width * f32::from(config.volume_profile.max_width_percent.min(50)) / 100.0
        } else { 0.0 };
        let right = if config.show_footprint_orderbook { (bounds.width * 0.2).min(130.0) } else { 0.0 };
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
        if self.footprint.visible && config.show_footprint_orderbook {
            let book = Rectangle { x: main.x + main.width, width: bounds.x+bounds.width-main.x-main.width, ..bounds };
            let max = depth.bids.values().chain(depth.asks.values()).map(|q| q.to_f64()).fold(0.0,f64::max);
            if max > 0.0 {
                frame.with_clip(book, |frame| {
                    for (orders,color) in [(&depth.bids,palette.success.base.color),(&depth.asks,palette.danger.base.color)] {
                        for (price,qty) in orders {
                            let Some(y) = price_y(scene,price.to_f64()).filter(|y| *y >= bounds.y && *y <= bounds.y+bounds.height) else { continue; };
                            let height = (price_y(scene,price.to_f64()+self.series.first().map_or(0.0,|s| s.tick_size().to_f64_lossy())).unwrap_or(y-3.0)-y).abs().clamp(2.0,18.0);
                            let width = (qty.to_f64()/max) as f32 * book.width;
                            frame.fill_rectangle(Point::new(book.x+book.width-width,y-height/2.0),Size::new(width,height),color.scale_alpha(0.4));
                            if height >= 10.0 { frame.fill_text(canvas::Text { content: config.quantity_format.format(qty.to_f64()), position:Point::new(book.x+book.width-3.0,y),align_x:iced::alignment::Horizontal::Right.into(),align_y:iced::alignment::Vertical::Center.into(),size:10.0.into(),color:palette.background.base.text,..Default::default() }); }
                        }
                    }
                });
            }
        }
        if !config.enabled.iter().any(|s| s == "VolumeProfile") { return; }
        let Some(series) = self.series.first() else { return; };
        let profile = config.volume_profile;
        let latest = series.latest_point().map_or(0,|p| p.kline.time.as_u64());
        let visible_end = scene.time_for_x_unit(scene.x.max_unit).map_or(latest,|t| t.as_u64());
        let end = match profile.past_duration_end { data::chart::kline::pro::VolumeProfilePastEnd::LatestData => latest, _ => visible_end };
        let start = match profile.range {
            data::chart::kline::pro::VolumeProfileRange::Visible => scene.time_for_x_unit(scene.x.min_unit).map_or(0,|t| t.as_u64()),
            data::chart::kline::pro::VolumeProfileRange::PastDuration => {
                let step = match self.basis { Basis::Time(t) => t.to_milliseconds(), _ => 60_000 };
                end.saturating_sub(step.saturating_mul(u64::from(profile.timeframe_multiplier.max(1))))
            }
        };
        let (low,high) = scene.primary_domain_value_range();
        let rows = usize::from(profile.rows.clamp(1,500));
        let step = ((high.to_f64()-low.to_f64())/rows as f64).max(series.tick_size().to_f64_lossy());
        let mut buckets = vec![(0.0,0.0);rows];
        for point in series.points().filter(|p| p.kline.time.as_u64() >= start && p.kline.time.as_u64() <= end) {
            for (price,group) in &point.footprint.trades {
                let price = price.to_f64();
                if price < low.to_f64() || price > high.to_f64() { continue; }
                let i = (((price-low.to_f64())/step) as usize).min(rows-1);
                buckets[i].0 += group.buy_qty.to_f64();
                buckets[i].1 += group.sell_qty.to_f64();
            }
        }
        let delta = matches!(profile.display,data::chart::kline::pro::VolumeProfileDisplay::Delta);
        let value = |&(buy,sell): &(f64,f64)| if delta { buy-sell } else { buy+sell };
        let max = buckets.iter().map(|b| value(b).abs()).fold(0.0,f64::max);
        if max <= 0.0 { return; }
        let width = if self.footprint.visible { main.x-bounds.x } else { bounds.width*f32::from(profile.max_width_percent.min(50))/100.0 };
        frame.with_clip(Rectangle { width, ..bounds },|frame| {
            for (i,bucket) in buckets.iter().enumerate() {
                let price = low.to_f64()+i as f64*step;
                let (Some(y1),Some(y2)) = (price_y(scene,price),price_y(scene,price+step)) else { continue; };
                let v = value(bucket);
                let color = if v < 0.0 { palette.danger.base.color } else { palette.success.base.color };
                frame.fill_rectangle(Point::new(bounds.x,y1.min(y2)),Size::new((v.abs()/max) as f32*width,(y2-y1).abs().max(1.0)),color.scale_alpha(0.5));
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

    pub(super) fn paint_pro_overlays(&self, frame: &mut PaintInputs, scene: &Scene, palette: &Extended, drag: Option<(TradingLineId,f64)>) {
        self.paint_pro_market(frame,scene,palette);
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
            indicator_panels:vec![],chrome:ChromeScene::default(),interaction:SceneInteraction::default(),
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
        config.show_footprint_orderbook = true;
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
}
