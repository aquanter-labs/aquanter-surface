//! Process-wide ticker directory fed by the sidebar metadata, so a chart can
//! find the same asset on other venues for aggregated order flow.
use crate::{Ticker, TickerInfo, adapter::MarketKind, adapter::Venue};
use rustc_hash::FxHashMap;
use std::sync::{LazyLock, RwLock};

static DIRECTORY: LazyLock<RwLock<FxHashMap<Ticker, TickerInfo>>> =
    LazyLock::new(|| RwLock::new(FxHashMap::default()));

const QUOTES: [&str; 4] = ["FDUSD", "USDT", "USDC", "USD"];

pub fn ingest(infos: impl IntoIterator<Item = TickerInfo>) {
    if let Ok(mut map) = DIRECTORY.write() {
        map.extend(infos.into_iter().map(|info| (info.ticker, info)));
    }
}

/// True when both tickers quote the same base coin (spot and perps both count).
pub fn same_asset(a: Ticker, b: Ticker) -> bool {
    matches!((asset_key(a), asset_key(b)), (Some(a), Some(b)) if a.0 == b.0)
}

/// `(base asset, is_spot)`; `None` for dated futures and unknown quotes.
fn asset_key(ticker: Ticker) -> Option<(String, bool)> {
    let is_spot = ticker.market_type() == MarketKind::Spot;
    let symbol = ticker.display_symbol_and_type().0.to_uppercase();
    let symbol = symbol
        .strip_suffix("-SWAP")
        .or_else(|| symbol.strip_suffix("_PERP"))
        .unwrap_or(&symbol);
    let base = match symbol.split_once(['-', '_', '/']) {
        Some((base, quote)) => QUOTES.contains(&quote).then_some(base)?,
        None => QUOTES
            .iter()
            .find_map(|quote| symbol.strip_suffix(quote))
            // Hyperliquid perps are bare coin names quoted in USDC.
            .or_else(|| {
                (ticker.exchange.venue() == Venue::Hyperliquid && !is_spot).then_some(symbol)
            })?,
    };
    (!base.is_empty()).then(|| (base.to_owned(), is_spot))
}

#[cfg(test)]
mod tests {
    use super::asset_key;
    use crate::{Ticker, adapter::Exchange};

    fn key(symbol: &str, exchange: Exchange) -> Option<(String, bool)> {
        asset_key(Ticker::new(symbol, exchange))
    }

    #[test]
    fn same_coin_matches_across_venue_symbol_styles() {
        let btc_perps = Some(("BTC".to_owned(), false));
        assert_eq!(key("BTCUSDT", Exchange::BinanceLinear), btc_perps);
        assert_eq!(key("BTCUSD_PERP", Exchange::BinanceInverse), btc_perps);
        assert_eq!(key("BTC-USDT-SWAP", Exchange::OkexLinear), btc_perps);
        assert_eq!(key("BTC_USDT", Exchange::MexcLinear), btc_perps);
        assert_eq!(key("BTC", Exchange::HyperliquidLinear), btc_perps);
        assert_eq!(
            key("BTCUSDT", Exchange::BybitSpot),
            Some(("BTC".to_owned(), true))
        );
        assert_eq!(key("BTCUSD_270326", Exchange::BinanceInverse), None);
        assert_eq!(key("BTC-USD-261225", Exchange::OkexInverse), None);
    }
}
