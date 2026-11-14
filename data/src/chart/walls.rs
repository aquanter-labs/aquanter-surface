//! Pro: spots resting-order walls that get pulled (spoof) or absorb flow as price nears them.

use exchange::depth::Depth;
use exchange::unit::{Price, PriceStep, Qty};
use exchange::{Trade, UnixMs};
use std::collections::{BTreeMap, VecDeque};

// ponytail: fixed thresholds; expose as heatmap settings if they need tuning per market.
/// A level is a wall when it holds this many times the median nearby level.
const WALL_MULT: f64 = 4.0;
/// Start watching a wall once mid price is this many steps away or closer.
const WATCH_STEPS: i64 = 10;
/// Median is taken over this many steps on the wall's side.
const AVG_STEPS: i64 = 50;
/// Stop watching once price drifts this far away, or after `MAX_AGE_MS`.
const DROP_STEPS: i64 = 30;
const MAX_AGE_MS: u64 = 5 * 60_000;
const MAX_MARKS: usize = 500;
/// A wall must rest this long before a pull counts; quote flicker is noise.
const MIN_LIFE_MS: u64 = 3_000;
/// Same-side, same-kind events this close in time form one burst.
const MERGE_MS: u64 = 30_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WallEvent {
    /// Wall vanished with little traded at its price.
    Pulled,
    /// Wall kept most of its size while a large amount traded into it.
    Absorbed,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WallMark {
    pub time: UnixMs,
    pub price: Price,
    pub is_bid: bool,
    pub event: WallEvent,
    /// Peak resting size of the wall.
    pub qty: Qty,
    /// Peak size as a multiple of the median nearby level.
    pub strength: f32,
    /// Size of the same-side, same-kind burst this mark belongs to (a wall walking with price).
    pub count: u32,
}

#[derive(Debug, Clone, Copy)]
struct Watch {
    is_bid: bool,
    peak: f64,
    strength: f64,
    traded: f64,
    since: u64,
}

#[derive(Debug, Default)]
pub struct WallTracker {
    watches: BTreeMap<Price, Watch>,
    pub marks: VecDeque<WallMark>,
}

fn grouped(side: &BTreeMap<Price, Qty>, step: PriceStep) -> BTreeMap<Price, f64> {
    let mut out = BTreeMap::new();
    for (price, qty) in side {
        *out.entry(price.round_to_step(step)).or_insert(0.0) += qty.to_f64();
    }
    out
}

fn steps(a: Price, b: Price, step: PriceStep) -> i64 {
    (a.units - b.units).abs() / step.units.max(1)
}

impl WallTracker {
    pub fn on_trades(&mut self, trades: &[Trade], step: PriceStep) {
        for trade in trades {
            if let Some(watch) = self.watches.get_mut(&trade.price.round_to_step(step)) {
                watch.traded += trade.qty.to_f64();
            }
        }
    }

    pub fn on_depth(&mut self, depth: &Depth, time: UnixMs, step: PriceStep) {
        let Some(mid) = depth.mid_price() else { return };
        let now = time.as_u64();
        let bids = grouped(&depth.bids, step);
        let asks = grouped(&depth.asks, step);

        for (levels, is_bid) in [(&bids, true), (&asks, false)] {
            let mut near = levels
                .iter()
                .filter(|(price, _)| steps(**price, mid, step) <= AVG_STEPS)
                .map(|(_, qty)| *qty)
                .collect::<Vec<_>>();
            if near.is_empty() {
                continue;
            }
            near.sort_by(f64::total_cmp);
            let median = near[near.len() / 2];
            for (price, qty) in levels {
                if steps(*price, mid, step) <= WATCH_STEPS && *qty >= median * WALL_MULT {
                    let watch = self.watches.entry(*price).or_insert(Watch {
                        is_bid,
                        peak: *qty,
                        strength: 0.0,
                        traded: 0.0,
                        since: now,
                    });
                    watch.peak = watch.peak.max(*qty);
                    watch.strength = watch.strength.max(*qty / median.max(f64::EPSILON));
                }
            }
        }

        let mut marks = Vec::new();
        self.watches.retain(|price, watch| {
            let side = if watch.is_bid { &bids } else { &asks };
            let current = side.get(price).copied().unwrap_or(0.0);
            let crossed = if watch.is_bid {
                mid < *price
            } else {
                mid > *price
            };
            if crossed
                || steps(*price, mid, step) > DROP_STEPS
                || now.saturating_sub(watch.since) > MAX_AGE_MS
            {
                return false;
            }
            let event = if current <= watch.peak * 0.3 {
                if now.saturating_sub(watch.since) < MIN_LIFE_MS {
                    return false;
                }
                // Gone: pulled if trades explain under 30% of the drop, else it was eaten.
                (watch.traded < (watch.peak - current) * 0.3).then_some(WallEvent::Pulled)
            } else if watch.traded >= watch.peak * 0.5 && current >= watch.peak * 0.7 {
                Some(WallEvent::Absorbed)
            } else {
                return true;
            };
            if let Some(event) = event {
                marks.push(WallMark {
                    time,
                    price: *price,
                    is_bid: watch.is_bid,
                    event,
                    qty: Qty::from_f64(watch.peak),
                    strength: watch.strength as f32,
                    count: 1,
                });
            }
            false
        });
        for mut mark in marks {
            // Marks stay where they happened; the whole burst shares its size.
            let mut burst = self
                .marks
                .iter_mut()
                .rev()
                .take_while(|m| now.saturating_sub(m.time.as_u64()) <= MERGE_MS)
                .filter(|m| m.is_bid == mark.is_bid && m.event == mark.event)
                .collect::<Vec<_>>();
            mark.count = burst.len() as u32 + 1;
            for m in &mut burst {
                m.count = m.count.max(mark.count);
            }
            self.marks.push_back(mark);
        }
        let extra = self.marks.len().saturating_sub(MAX_MARKS);
        self.marks.drain(..extra);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(v: f64) -> Price {
        Price::from_f64(v)
    }

    fn book(bids: &[(f64, f64)], asks: &[(f64, f64)]) -> Depth {
        let side = |levels: &[(f64, f64)]| {
            levels
                .iter()
                .map(|(price, qty)| (p(*price), Qty::from_f64(*qty)))
                .collect()
        };
        Depth {
            bids: side(bids),
            asks: side(asks),
        }
    }

    fn trade(price: f64, qty: f64) -> Trade {
        Trade {
            time: UnixMs(0),
            is_sell: true,
            price: p(price),
            qty: Qty::from_f64(qty),
        }
    }

    const ASKS: &[(f64, f64)] = &[(101.0, 1.0), (102.0, 1.0)];
    const THIN: &[(f64, f64)] = &[(100.0, 1.0), (99.0, 1.0), (98.0, 1.0)];

    #[test]
    fn pulled_wall_without_trades_is_marked_pulled() {
        let step = PriceStep {
            units: Price::from_f64(1.0).units,
        };
        let mut tracker = WallTracker::default();
        let walled = book(&[(100.0, 1.0), (99.0, 1.0), (97.0, 50.0)], ASKS);
        tracker.on_depth(&walled, UnixMs(1), step);
        tracker.on_depth(&book(THIN, ASKS), UnixMs(2), step);
        assert!(
            tracker.marks.is_empty(),
            "flicker under MIN_LIFE_MS is ignored"
        );
        tracker.on_depth(&walled, UnixMs(10), step);
        tracker.on_depth(&book(THIN, ASKS), UnixMs(5_000), step);
        assert_eq!(tracker.marks.len(), 1);
        assert_eq!(tracker.marks[0].event, WallEvent::Pulled);
        assert_eq!(tracker.marks[0].price, p(97.0));
        assert!(tracker.marks[0].is_bid);
        assert_eq!(tracker.marks[0].strength, 50.0);
        // A second pull within MERGE_MS keeps its own spot; both share the burst size.
        tracker.on_depth(&walled, UnixMs(6_000), step);
        tracker.on_depth(&book(THIN, ASKS), UnixMs(10_000), step);
        assert_eq!(tracker.marks.len(), 2);
        assert!(tracker.marks.iter().all(|m| m.count == 2));
    }

    #[test]
    fn wall_that_holds_under_selling_is_marked_absorbed() {
        let step = PriceStep {
            units: Price::from_f64(1.0).units,
        };
        let mut tracker = WallTracker::default();
        let walled = book(&[(100.0, 1.0), (99.0, 1.0), (97.0, 50.0)], ASKS);
        tracker.on_depth(&walled, UnixMs(1), step);
        tracker.on_trades(&[trade(97.0, 30.0)], step);
        tracker.on_depth(&walled, UnixMs(2), step);
        assert_eq!(tracker.marks.len(), 1);
        assert_eq!(tracker.marks[0].event, WallEvent::Absorbed);
    }

    #[test]
    fn wall_eaten_by_trades_is_not_marked() {
        let step = PriceStep {
            units: Price::from_f64(1.0).units,
        };
        let mut tracker = WallTracker::default();
        tracker.on_depth(
            &book(&[(100.0, 1.0), (99.0, 1.0), (97.0, 50.0)], ASKS),
            UnixMs(1),
            step,
        );
        tracker.on_trades(&[trade(97.0, 45.0)], step);
        tracker.on_depth(&book(THIN, ASKS), UnixMs(2), step);
        assert!(tracker.marks.is_empty());
    }
}
