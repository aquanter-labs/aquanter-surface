use serde::{Deserialize, Serialize};

const KEYCHAIN_SERVICE: &str = "flowsurface.trading";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum TradingExchange {
    #[default]
    BinanceTestnet,
    BinanceLive,
    Simulation,
}

impl TradingExchange {
    pub const ALL: [Self; 3] = [Self::Simulation, Self::BinanceTestnet, Self::BinanceLive];
}

impl std::fmt::Display for TradingExchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::BinanceTestnet => "Binance Testnet",
            Self::BinanceLive => "Binance Live",
            Self::Simulation => "Local Simulation",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(default)]
pub struct TradingConfig {
    #[serde(default = "new_account_id")]
    pub account_id: String,
    pub account_label: String,
    pub exchange: TradingExchange,
    pub testnet: bool,
    pub symbol: String,
    pub poll_interval_secs: u16,
    pub recv_window_ms: u16,
    pub confirm_orders: bool,
    pub allocation_percent: u8,
    pub simulated_balance: u64,
    /// Simulated fee rates in percent of notional (Binance VIP 0 defaults).
    pub simulated_maker_fee_pct: f64,
    pub simulated_taker_fee_pct: f64,
    /// Chart markers for open / settled simulated event contracts.
    pub event_show_open: bool,
    pub event_show_history: bool,
    /// The panel shows the event contract tab instead of futures trading.
    pub event_tab: bool,
    /// Draw trades / event contracts on every active chart, not just the selected one.
    pub overlay_all_charts: bool,
    /// The chart position line shows the break-even price (fees included)
    /// instead of the plain entry price.
    pub chart_price_with_fees: bool,
    /// Chart shows the open position, its SL/TP and pending orders.
    pub chart_show_position: bool,
    /// Chart shows filled trades.
    pub chart_show_fills: bool,
    /// Fills as a B/S badge under / over the candle instead of an arrow at the fill price.
    pub chart_fill_badges: bool,
    /// Only mark fills that open or add to a position (no realized PnL).
    pub chart_opening_fills_only: bool,
    /// TP / SL and limit-order lines can be dragged on the chart.
    pub chart_drag_orders: bool,
    /// A chart drag waits for confirmation before it is sent.
    pub chart_drag_confirm: bool,
    /// Last order-ticket inputs (price, qty, SL/TP, event stake...), owned by the app crate.
    pub ticket: serde_json::Value,
}

impl Default for TradingConfig {
    fn default() -> Self {
        Self {
            account_id: new_account_id(),
            account_label: "Binance USD-M".to_string(),
            exchange: TradingExchange::BinanceTestnet,
            testnet: true,
            symbol: "BTCUSDT".to_string(),
            poll_interval_secs: 1,
            recv_window_ms: 5_000,
            confirm_orders: true,
            allocation_percent: 10,
            simulated_balance: 10_000,
            simulated_maker_fee_pct: 0.02,
            simulated_taker_fee_pct: 0.04,
            event_show_open: true,
            event_show_history: true,
            event_tab: false,
            overlay_all_charts: false,
            chart_price_with_fees: false,
            chart_show_position: true,
            chart_show_fills: true,
            chart_fill_badges: true,
            chart_opening_fills_only: false,
            chart_drag_orders: true,
            chart_drag_confirm: true,
            ticket: serde_json::Value::Null,
        }
    }
}

/// One account shared by every trading panel in every layout, so switching
/// layouts never shows a different (empty) account.
pub const SHARED_ACCOUNT_ID: &str = "shared";

fn new_account_id() -> String {
    SHARED_ACCOUNT_ID.to_string()
}

/// Copies the API keys saved under a per-panel account (older versions) to the
/// shared account, unless the shared account already has its own.
pub fn adopt_credentials(old_account_id: &str) {
    for testnet in [true, false] {
        let Ok(target) = entry_for(SHARED_ACCOUNT_ID, testnet) else {
            continue;
        };
        if target.get_password().is_ok() {
            continue;
        }
        if let Some(value) = entry_for(old_account_id, testnet)
            .ok()
            .and_then(|entry| entry.get_password().ok())
            && let Err(error) = target.set_password(&value)
        {
            log::warn!("Failed to move trading credentials to the shared account: {error}");
        }
    }
}

#[derive(Clone)]
pub struct TradingCredentials {
    pub api_key: String,
    pub secret_key: String,
}

fn entry_for(account_id: &str, testnet: bool) -> Result<keyring::Entry, keyring::Error> {
    keyring::Entry::new(
        KEYCHAIN_SERVICE,
        &format!(
            "{}-{}",
            account_id,
            if testnet { "testnet" } else { "live" }
        ),
    )
}

pub fn load_credentials(account_id: &str, testnet: bool) -> Option<TradingCredentials> {
    let value = entry_for(account_id, testnet).ok()?.get_password().ok()?;
    let (api_key, secret_key) = value.split_once('\n')?;
    (!api_key.is_empty() && !secret_key.is_empty()).then(|| TradingCredentials {
        api_key: api_key.to_string(),
        secret_key: secret_key.to_string(),
    })
}

pub fn migrate_legacy_credentials(account_id: &str, testnet: bool) -> Result<(), String> {
    let target = entry_for(account_id, testnet)
        .map_err(|error| format!("Failed to access the system keychain: {error}"))?;
    if target.get_password().is_ok() {
        return Ok(());
    }
    let legacy = keyring::Entry::new(
        KEYCHAIN_SERVICE,
        if testnet {
            "binance-usdm-testnet"
        } else {
            "binance-usdm"
        },
    )
    .map_err(|error| format!("Failed to access legacy trading credentials: {error}"))?;
    let Ok(value) = legacy.get_password() else {
        return Ok(());
    };
    target
        .set_password(&value)
        .map_err(|error| format!("Failed to migrate trading credentials: {error}"))
}

pub fn save_credentials(
    account_id: &str,
    testnet: bool,
    api_key: &str,
    secret_key: &str,
) -> Result<(), String> {
    let api_key = api_key.trim();
    let secret_key = secret_key.trim();
    if api_key.is_empty() || secret_key.is_empty() {
        return Err("API key and secret key are required.".to_string());
    }
    if api_key.contains('\n') || secret_key.contains('\n') {
        return Err("API credentials contain an invalid newline.".to_string());
    }
    entry_for(account_id, testnet)
        .map_err(|error| format!("Failed to access the system keychain: {error}"))?
        .set_password(&format!("{api_key}\n{secret_key}"))
        .map_err(|error| format!("Failed to store trading credentials: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_state_uses_safe_trading_defaults() {
        let config: TradingConfig = serde_json::from_str("{}").unwrap();
        assert!(config.testnet);
        assert!(config.confirm_orders);
        assert_eq!(config.poll_interval_secs, 1);
        assert_eq!(config.account_id, SHARED_ACCOUNT_ID);
    }
}
