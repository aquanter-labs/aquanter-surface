use data::chart::heatmap::GroupedTrade;
use exchange::unit::{Price, PriceStep, Qty, qty};

use crate::widget::chart::heatmap::scene::depth_grid::HeatmapPalette;
use crate::widget::chart::heatmap::view::ViewWindow;

pub const CIRCLE_VERTICES: &[[f32; 2]] = &[[-1.0, -1.0], [1.0, -1.0], [1.0, 1.0], [-1.0, 1.0]];
pub const CIRCLE_INDICES: &[u16] = &[0, 1, 2, 2, 3, 0];

#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct CircleInstance {
    pub y_world: f32,
    pub x_bin_rel: i32,
    pub x_frac: f32,
    pub radius_px: f32,
    /// Stroke width of a hollow ring; 0 draws a filled disc.
    ring_px: f32,
    pub color: [f32; 4],
}

impl CircleInstance {
    /// Pro: amber = wall pulled (spoof), violet = wall absorbing flow.
    pub fn wall_marker(mark: &data::chart::walls::WallMark, y_world: f32, x_bin_rel: i64) -> Self {
        // Color = side: orange bid, yellow ask. Shape = event: hollow pulled, filled absorbed.
        let color = if mark.is_bid {
            [1.0, 0.5, 0.1, 0.95]
        } else {
            [1.0, 0.95, 0.35, 0.95]
        };
        let ring_px = match mark.event {
            data::chart::walls::WallEvent::Pulled => 1.5,
            data::chart::walls::WallEvent::Absorbed => 0.0,
        };
        Self {
            y_world,
            x_bin_rel: x_bin_rel.clamp(i32::MIN as i64, i32::MAX as i64) as i32,
            x_frac: 0.0,
            // 4px at the 4x detection floor, growing with log strength.
            // 4px..9px with log strength; small so the book underneath stays readable.
            radius_px: (4.0 + 2.0 * (mark.strength / 4.0).max(1.0).ln()).min(9.0),
            ring_px,
            color,
        }
    }

    pub const R_MIN_PX: f32 = 1.5;
    const R_MAX_PX: f32 = 25.0;
    const ALPHA: f32 = 0.8;

    pub fn from_trade(
        trade: &GroupedTrade,
        bucket: i64,
        ref_bucket: i64,
        base_price: Price,
        step: PriceStep,
        y_anchor: Option<Price>,
        w: &ViewWindow,
        palette: &HeatmapPalette,
        max_trade_qty: Qty,
        trade_size_scale: Option<i32>,
        fallback_radius_px: f32,
    ) -> Self {
        let x_bin_rel = (bucket - ref_bucket).clamp(i32::MIN as i64, i32::MAX as i64) as i32;
        let x_frac = 0.0;

        let y_world = w.y_center_for_price_texture_aligned(trade.price, base_price, step, y_anchor);

        let q = trade.qty.max(qty::Qty::ZERO).to_f64();
        let t = (q / max_trade_qty.to_scale_or_one()).clamp(0.0, 1.0) as f32;
        let radius_px = if let Some(scale_pct) = trade_size_scale {
            let scale_factor = (scale_pct as f32 / 100.0).max(0.0);
            Self::R_MIN_PX + t * (Self::R_MAX_PX - Self::R_MIN_PX) * scale_factor
        } else {
            fallback_radius_px.max(Self::R_MIN_PX)
        };

        let rgba = if trade.is_sell {
            [
                palette.sell_rgb[0],
                palette.sell_rgb[1],
                palette.sell_rgb[2],
                Self::ALPHA,
            ]
        } else {
            [
                palette.buy_rgb[0],
                palette.buy_rgb[1],
                palette.buy_rgb[2],
                Self::ALPHA,
            ]
        };

        Self {
            y_world,
            x_bin_rel,
            x_frac,
            radius_px,
            ring_px: 0.0,
            color: rgba,
        }
    }
}
