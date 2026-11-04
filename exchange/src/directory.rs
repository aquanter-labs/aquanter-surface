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

/// Same base asset and market class (spot vs perps) on every known venue,
/// across USDT/USDC/USD quotes, excluding `base` itself. Sorted for stable
/// stream specs.
pub fn aggregate_peers(base: TickerInfo) -> Vec<TickerInfo> {
    let Some(key) = asset_key(base.ticker) else {
        return Vec::new();
    };
    let Ok(map) = DIRECTORY.read() else {
        return Vec::new();
    };
    let mut peers = map
        .values()
        .filter(|info| info.ticker != base.ticker)
        .filter(|info| asset_key(info.ticker).as_ref() == Some(&key))
        .copied()
        .collect::<Vec<_>>();
    peers.sort_by(|a, b| {
        (a.ticker.exchange.venue() as u8, a.ticker.as_str())
            .cmp(&(b.ticker.exchange.venue() as u8, b.ticker.as_str()))
    });
    peers
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
        assert_eq!(key("BTCUSDT", Exchange::BybitSpot), Some(("BTC".to_owned(), true)));
        assert_eq!(key("BTCUSD_270326", Exchange::BinanceInverse), None);
        assert_eq!(key("BTC-USD-261225", Exchange::OkexInverse), None);
    }
}
