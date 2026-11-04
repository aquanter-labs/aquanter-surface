pub mod binance;
pub mod bybit;
pub mod hyperliquid;
pub mod mexc;
pub mod okex;

use super::AdapterError;

/// Downloads `url` to `path` once; later calls reuse the file. Writes through a
/// `.part` file so an interrupted download is never cached as complete.
pub(crate) async fn cached_download(
    client: &reqwest::Client,
    url: &str,
    path: &std::path::Path,
) -> Result<(), AdapterError> {
    if std::fs::metadata(path).is_ok() {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)
            .map_err(|e| AdapterError::ParseError(format!("Failed to create directories: {e}")))?;
    }
    log::info!("Downloading from {url}");
    // Bybit's CDN answers 403 to requests without a User-Agent.
    let resp = client
        .get(url)
        .header(reqwest::header::USER_AGENT, "flowsurface")
        .send()
        .await
        .map_err(AdapterError::from)?;
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
        .map_err(|e| AdapterError::ParseError(format!("Failed to write {path:?}: {e}")))
}

use super::http::spawn_fetch_worker;
use super::http::{FetchCommand, FetchCommandHandler, HttpHub, RequestPort};
use super::http::{TickerMetadataMap, TickerStatsMap};

use super::ws::TradeBuffer;
use super::ws::{HeartbeatPolicy, WsAdapter, WsSession, WsTransport};

/// Live network checks: `cargo test -p flowsurface-exchange trade_history -- --ignored`.
#[cfg(test)]
mod trade_history_tests {
    use crate::{Ticker, TickerInfo, Timeframe, Trade, UnixMs, adapter::Exchange};

    fn assert_sane(trades: &[Trade], from: UnixMs, until: UnixMs) {
        assert!(!trades.is_empty(), "no trades in range");
        assert!(trades.windows(2).all(|w| w[0].time <= w[1].time));
        assert!(trades.iter().all(|t| t.time >= from && t.time < until && t.qty.to_f64() > 0.0));
    }

    #[tokio::test]
    #[ignore]
    async fn okx_rest_and_archive_trades() {
        let handle = super::okex::OkexHandle::new(reqwest::Client::new(), None).unwrap();
        let info = TickerInfo::new(Ticker::new("BTC-USDT-SWAP", Exchange::OkexLinear), 0.1, 0.01, Some(0.01));
        let until = UnixMs::new(UnixMs::now().as_u64() - 60_000);
        let from = UnixMs::new(until.as_u64() - 5_000);
        assert_sane(&handle.fetch_trades(info, from, until, None).await.unwrap(), from, until);

        let day = UnixMs::now().floor_to(Timeframe::D1).as_u64() - 2 * 86_400_000;
        let (from, until) = (UnixMs::new(day + 15 * 3_600_000), UnixMs::new(day + 17 * 3_600_000));
        let dir = std::env::temp_dir().join("flowsurface-okx-test");
        assert_sane(&handle.fetch_trades(info, from, until, Some(dir)).await.unwrap(), from, until);
    }

    #[tokio::test]
    #[ignore]
    async fn bybit_archive_trades() {
        let handle = super::bybit::BybitHandle::new(reqwest::Client::new(), None).unwrap();
        let info = TickerInfo::new(Ticker::new("ETCUSDT", Exchange::BybitLinear), 0.001, 0.1, None);
        let day = UnixMs::now().floor_to(Timeframe::D1).as_u64() - 86_400_000;
        let (from, until) = (UnixMs::new(day), UnixMs::new(day + 86_400_000));
        let dir = std::env::temp_dir().join("flowsurface-bybit-test");
        assert_sane(&handle.fetch_trades(info, from, until, Some(dir)).await.unwrap(), from, until);
    }
}
