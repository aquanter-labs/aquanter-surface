pub mod binance;
pub mod bybit;
pub mod hyperliquid;
pub mod mexc;
pub mod okex;

use super::AdapterError;

pub(crate) const HOUR_MS: u64 = 3_600_000;
/// Bytes per cached trade: time, price, qty, is_sell.
const CACHED_TRADE_LEN: usize = 25;

/// A trade as the venue reported it (qty not yet normalized), as kept in the hour cache.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct RawTrade {
    pub time: u64,
    pub price: f64,
    pub qty: f64,
    pub is_sell: bool,
}

/// Start of the newest UTC hour that is not yet safe to cache; a minute of slack
/// lets the hour's last trades be published first.
pub(crate) fn open_hour_start() -> u64 {
    crate::UnixMs::now().as_u64().saturating_sub(60_000) / HOUR_MS * HOUR_MS
}

/// Cache file of one closed UTC hour of `symbol`'s REST trade history.
// ponytail: raw 25-byte records, no compression (~4-8 MB per busy perp hour); gzip if disk use matters.
pub(crate) fn hour_cache_path(
    dir: &std::path::Path,
    symbol: &str,
    hour: u64,
) -> Option<std::path::PathBuf> {
    let stamp = chrono::DateTime::from_timestamp_millis(hour as i64)?.format("%Y-%m-%d-%H");
    Some(dir.join(format!("{}-intraday-{stamp}.bin", symbol.to_uppercase())))
}

pub(crate) fn read_hour_cache(path: &std::path::Path) -> Option<Vec<RawTrade>> {
    decode_trades(&std::fs::read(path).ok()?)
}

/// Best effort: a failed write only means the hour is fetched again next time.
pub(crate) fn write_hour_cache(path: &std::path::Path, trades: &[RawTrade]) {
    let part = path.with_extension("part");
    let written = path
        .parent()
        .map_or(Ok(()), std::fs::create_dir_all)
        .and_then(|()| std::fs::write(&part, encode_trades(trades)))
        .and_then(|()| std::fs::rename(&part, path));
    if let Err(e) = written {
        log::warn!("Failed to cache {path:?}: {e}");
    }
}

fn encode_trades(trades: &[RawTrade]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(trades.len() * CACHED_TRADE_LEN);
    for trade in trades {
        bytes.extend(trade.time.to_le_bytes());
        bytes.extend(trade.price.to_le_bytes());
        bytes.extend(trade.qty.to_le_bytes());
        bytes.push(u8::from(trade.is_sell));
    }
    bytes
}

fn decode_trades(bytes: &[u8]) -> Option<Vec<RawTrade>> {
    if !bytes.len().is_multiple_of(CACHED_TRADE_LEN) {
        return None;
    }
    let word = |c: &[u8], i: usize| <[u8; 8]>::try_from(&c[i..i + 8]).ok();
    bytes
        .chunks_exact(CACHED_TRADE_LEN)
        .map(|c| {
            Some(RawTrade {
                time: u64::from_le_bytes(word(c, 0)?),
                price: f64::from_le_bytes(word(c, 8)?),
                qty: f64::from_le_bytes(word(c, 16)?),
                is_sell: c[24] != 0,
            })
        })
        .collect()
}

#[cfg(test)]
mod hour_cache_tests {
    use super::{RawTrade, decode_trades, encode_trades};

    #[test]
    fn hour_cache_round_trips_trades() {
        let trades = vec![
            RawTrade {
                time: 42,
                price: 10.0,
                qty: 1.0,
                is_sell: false,
            },
            RawTrade {
                time: 43,
                price: 1.5,
                qty: 0.25,
                is_sell: true,
            },
        ];
        assert_eq!(decode_trades(&encode_trades(&trades)), Some(trades));
        assert!(decode_trades(&[0; 5]).is_none());
    }
}

/// Downloads `url` to `path` once; later calls reuse the file. Writes through a
/// `.part` file so an interrupted download is never cached as complete.
/// Returns `false` on 404: the archive day isn't published yet (venues lag a day or so).
pub(crate) async fn cached_download(
    client: &reqwest::Client,
    url: &str,
    path: &std::path::Path,
) -> Result<bool, AdapterError> {
    if std::fs::metadata(path).is_ok() {
        return Ok(true);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| AdapterError::ParseError(format!("Failed to create directories: {e}")))?;
    }
    log::info!("Downloading from {url}");
    // Bybit's CDN answers 403 to requests without a User-Agent.
    let resp = client
        .get(url)
        .header(reqwest::header::USER_AGENT, "aquanter-surface")
        .send()
        .await
        .map_err(AdapterError::from)?;
    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(false);
    }
    if !resp.status().is_success() {
        return Err(AdapterError::InvalidRequest(format!(
            "Failed to fetch from {url}: {}",
            resp.status()
        )));
    }
    let body = resp.bytes().await.map_err(AdapterError::from)?;
    let part = path.with_extension("part");
    std::fs::write(&part, &body)
        .and_then(|()| std::fs::rename(&part, path))
        .map(|()| true)
        .map_err(|e| AdapterError::ParseError(format!("Failed to write {path:?}: {e}")))
}

use super::http::spawn_fetch_worker;
use super::http::{FetchCommand, FetchCommandHandler, HttpHub, RequestPort};
use super::http::{TickerMetadataMap, TickerStatsMap};

use super::ws::TradeBuffer;
use super::ws::{HeartbeatPolicy, WsAdapter, WsSession, WsTransport};

/// Live network checks: `cargo test -p aquanter-surface-exchange trade_history -- --ignored`.
#[cfg(test)]
mod trade_history_tests {
    use crate::{Ticker, TickerInfo, Timeframe, Trade, UnixMs, adapter::Exchange};

    fn assert_sane(trades: &[Trade], from: UnixMs, until: UnixMs) {
        assert!(!trades.is_empty(), "no trades in range");
        assert!(trades.windows(2).all(|w| w[0].time <= w[1].time));
        assert!(
            trades
                .iter()
                .all(|t| t.time >= from && t.time < until && t.qty.to_f64() > 0.0)
        );
    }

    #[tokio::test]
    #[ignore]
    async fn okx_rest_and_archive_trades() {
        let handle = super::okex::OkexHandle::new(reqwest::Client::new(), None).unwrap();
        let info = TickerInfo::new(
            Ticker::new("BTC-USDT-SWAP", Exchange::OkexLinear),
            0.1,
            0.01,
            Some(0.01),
        );
        let until = UnixMs::new(UnixMs::now().as_u64() - 60_000);
        let from = UnixMs::new(until.as_u64() - 5_000);
        assert_sane(
            &handle.fetch_trades(info, from, until, None).await.unwrap(),
            from,
            until,
        );

        let day = UnixMs::now().floor_to(Timeframe::D1).as_u64() - 2 * 86_400_000;
        let (from, until) = (
            UnixMs::new(day + 15 * 3_600_000),
            UnixMs::new(day + 17 * 3_600_000),
        );
        let dir = std::env::temp_dir().join("aquanter-surface-okx-test");
        assert_sane(
            &handle
                .fetch_trades(info, from, until, Some(dir))
                .await
                .unwrap(),
            from,
            until,
        );
    }

    #[tokio::test]
    #[ignore]
    async fn bybit_archive_trades() {
        let handle = super::bybit::BybitHandle::new(reqwest::Client::new(), None).unwrap();
        let info = TickerInfo::new(
            Ticker::new("ETCUSDT", Exchange::BybitLinear),
            0.001,
            0.1,
            None,
        );
        let day = UnixMs::now().floor_to(Timeframe::D1).as_u64() - 86_400_000;
        let (from, until) = (UnixMs::new(day), UnixMs::new(day + 86_400_000));
        let dir = std::env::temp_dir().join("aquanter-surface-bybit-test");
        assert_sane(
            &handle
                .fetch_trades(info, from, until, Some(dir))
                .await
                .unwrap(),
            from,
            until,
        );
    }
}
