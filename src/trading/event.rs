//! Simulated Binance-style event contracts: bet Up or Down over a fixed window;
//! a win pays the stake back plus a fixed rate, a loss forfeits the stake.

use serde::{Deserialize, Serialize};

pub const MIN_STAKE: f64 = 5.0;
pub const MAX_STAKE: f64 = 250.0;
pub const MAX_OPEN: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
pub enum EventDuration {
    #[default]
    M10,
    M30,
    H1,
}

impl EventDuration {
    pub const ALL: [Self; 3] = [Self::M10, Self::M30, Self::H1];

    pub fn ms(self) -> u64 {
        match self {
            Self::M10 => 10 * 60_000,
            Self::M30 => 30 * 60_000,
            Self::H1 => 60 * 60_000,
        }
    }

    /// Profit on a win as a fraction of the stake.
    pub fn payout_rate(self) -> f64 {
        match self {
            Self::M10 => 0.80,
            Self::M30 | Self::H1 => 0.85,
        }
    }
}

impl std::fmt::Display for EventDuration {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::M10 => "10m",
            Self::M30 => "30m",
            Self::H1 => "1h",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct EventContract {
    pub id: u64,
    pub symbol: String,
    pub up: bool,
    pub duration: EventDuration,
    pub stake: f64,
    /// Locked at open so a later rate change never rewrites history.
    pub payout_rate: f64,
    pub open_time_ms: u64,
    pub open_price: f64,
    pub expiry_ms: u64,
    pub settle_price: Option<f64>,
}

impl EventContract {
    pub fn is_open(&self) -> bool {
        self.settle_price.is_none()
    }

    /// Only a strict move in the chosen direction wins; an unchanged price loses.
    pub fn wins_at(&self, price: f64) -> bool {
        if self.up {
            price > self.open_price
        } else {
            price < self.open_price
        }
    }

    /// Realized PnL once settled.
    pub fn pnl(&self) -> Option<f64> {
        self.settle_price.map(|price| {
            if self.wins_at(price) {
                self.stake * self.payout_rate
            } else {
                -self.stake
            }
        })
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct EventStats {
    pub settled: usize,
    pub wins: usize,
    pub net: f64,
    pub gross_profit: f64,
    pub gross_loss: f64,
    pub open: usize,
    pub open_stake: f64,
}

impl EventStats {
    pub fn win_rate(&self) -> f64 {
        if self.settled == 0 {
            0.0
        } else {
            self.wins as f64 / self.settled as f64 * 100.0
        }
    }
}

pub fn stats(contracts: &[EventContract]) -> EventStats {
    let mut stats = EventStats::default();
    for contract in contracts {
        match contract.pnl() {
            None => {
                stats.open += 1;
                stats.open_stake += contract.stake;
            }
            Some(pnl) => {
                stats.settled += 1;
                stats.net += pnl;
                if pnl > 0.0 {
                    stats.wins += 1;
                    stats.gross_profit += pnl;
                } else {
                    stats.gross_loss -= pnl;
                }
            }
        }
    }
    stats
}

/// Validates a new contract against Binance's limits and the free balance.
pub fn open(
    contracts: &[EventContract],
    symbol: &str,
    up: bool,
    duration: EventDuration,
    stake: f64,
    price: f64,
    now_ms: u64,
    available: f64,
) -> Result<EventContract, String> {
    if !(MIN_STAKE..=MAX_STAKE).contains(&stake) {
        return Err(format!("Stake must be {MIN_STAKE}-{MAX_STAKE} USDT."));
    }
    if contracts.iter().filter(|c| c.is_open()).count() >= MAX_OPEN {
        return Err(format!("At most {MAX_OPEN} event contracts can be open."));
    }
    if stake > available {
        return Err("Insufficient available USDT.".to_string());
    }
    if !(price.is_finite() && price > 0.0) {
        return Err("No live price for this chart yet.".to_string());
    }
    Ok(EventContract {
        id: now_ms.saturating_mul(10) + contracts.len() as u64 % 10,
        symbol: symbol.to_string(),
        up,
        duration,
        stake,
        payout_rate: duration.payout_rate(),
        open_time_ms: now_ms,
        open_price: price,
        expiry_ms: now_ms + duration.ms(),
        settle_price: None,
    })
}

/// Settles `symbol`'s expired contracts at `price`; returns what goes back to the
/// wallet (stake plus profit for each win), or `None` when nothing settled.
// ponytail: settles at the first price seen after expiry; if the app was closed at
// expiry that is the price on restart. Fetch the expiry-minute kline if that matters.
pub fn settle_due(
    contracts: &mut [EventContract],
    symbol: &str,
    price: f64,
    now_ms: u64,
) -> Option<f64> {
    let mut credit = None;
    for contract in contracts
        .iter_mut()
        .filter(|c| c.is_open() && c.symbol == symbol && c.expiry_ms <= now_ms)
    {
        contract.settle_price = Some(price);
        let back = contract
            .pnl()
            .map_or(0.0, |pnl| (contract.stake + pnl).max(0.0));
        *credit.get_or_insert(0.0) += back;
    }
    credit
}

#[cfg(test)]
mod tests {
    use super::*;

    fn opened(contracts: &[EventContract], up: bool, duration: EventDuration) -> EventContract {
        open(
            contracts, "BTCUSDT", up, duration, 100.0, 50_000.0, 1_000, 10_000.0,
        )
        .unwrap()
    }

    #[test]
    fn win_pays_rate_loss_and_tie_forfeit_stake() {
        let mut contracts = vec![
            opened(&[], true, EventDuration::M10),
            opened(&[], false, EventDuration::M30),
            opened(&[], true, EventDuration::H1),
        ];
        // Not yet expired: nothing settles.
        assert_eq!(settle_due(&mut contracts, "BTCUSDT", 50_100.0, 1_000), None);
        // At 10m only the 10m contract settles: up and price rose -> 100 + 80.
        let ten = 1_000 + EventDuration::M10.ms();
        assert_eq!(
            settle_due(&mut contracts, "BTCUSDT", 50_100.0, ten),
            Some(180.0)
        );
        // At 1h: down loses on a rise (0 back), up ties at the open price and loses too.
        let hour = 1_000 + EventDuration::H1.ms();
        assert_eq!(
            settle_due(&mut contracts, "BTCUSDT", 50_000.0, hour),
            Some(0.0)
        );
        assert_eq!(
            contracts
                .iter()
                .map(|c| c.pnl().unwrap())
                .collect::<Vec<_>>(),
            [80.0, -100.0, -100.0]
        );

        contracts.push(opened(&[], true, EventDuration::M10));
        let s = stats(&contracts);
        assert_eq!((s.settled, s.wins, s.open, s.open_stake), (3, 1, 1, 100.0));
        assert_eq!((s.net, s.gross_profit, s.gross_loss), (-120.0, 80.0, 200.0));
    }

    #[test]
    fn open_enforces_limits() {
        let ok = |stake, available| {
            open(
                &[],
                "BTCUSDT",
                true,
                EventDuration::M10,
                stake,
                1.0,
                0,
                available,
            )
        };
        assert!(ok(250.0, 1_000.0).is_ok());
        assert!(ok(251.0, 1_000.0).is_err());
        assert!(ok(4.0, 1_000.0).is_err());
        assert!(ok(100.0, 50.0).is_err());
        let five = (0..MAX_OPEN)
            .map(|_| opened(&[], true, EventDuration::M10))
            .collect::<Vec<_>>();
        assert!(
            open(
                &five,
                "BTCUSDT",
                true,
                EventDuration::M10,
                10.0,
                1.0,
                0,
                1_000.0
            )
            .is_err()
        );
    }
}
