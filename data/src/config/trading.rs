use serde::{Deserialize, Serialize};

const KEYCHAIN_SERVICE: &str = "flowsurface.trading";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct TradingConfig {
    pub testnet: bool,
    pub symbol: String,
    pub poll_interval_secs: u16,
    pub recv_window_ms: u16,
    pub confirm_orders: bool,
}

impl Default for TradingConfig {
    fn default() -> Self {
        Self {
            testnet: true,
            symbol: "BTCUSDT".to_string(),
            poll_interval_secs: 1,
            recv_window_ms: 5_000,
            confirm_orders: true,
        }
    }
}

#[derive(Clone)]
pub struct TradingCredentials {
    pub api_key: String,
    pub secret_key: String,
}

fn entry_for(testnet: bool) -> Result<keyring::Entry, keyring::Error> {
    keyring::Entry::new(
        KEYCHAIN_SERVICE,
        if testnet {
            "binance-usdm-testnet"
        } else {
            "binance-usdm"
        },
    )
}

pub fn load_credentials(testnet: bool) -> Option<TradingCredentials> {
    let value = entry_for(testnet).ok()?.get_password().ok()?;
    let (api_key, secret_key) = value.split_once('\n')?;
    (!api_key.is_empty() && !secret_key.is_empty()).then(|| TradingCredentials {
        api_key: api_key.to_string(),
        secret_key: secret_key.to_string(),
    })
}

pub fn save_credentials(testnet: bool, api_key: &str, secret_key: &str) -> Result<(), String> {
    let api_key = api_key.trim();
    let secret_key = secret_key.trim();
    if api_key.is_empty() || secret_key.is_empty() {
        return Err("API key and secret key are required.".to_string());
    }
    if api_key.contains('\n') || secret_key.contains('\n') {
        return Err("API credentials contain an invalid newline.".to_string());
    }
    entry_for(testnet)
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
    }
}
