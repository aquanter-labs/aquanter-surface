use serde::{Deserialize, Serialize};
use std::fmt;

const KEYCHAIN_SERVICE: &str = "flowsurface.ai";

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize)]
pub enum AiProvider {
    #[default]
    OpenAi,
    OpenRouter,
    DeepSeek,
    XAi,
    Groq,
    Mistral,
    Anthropic,
    Gemini,
    Custom,
}

impl AiProvider {
    pub const ALL: [Self; 9] = [
        Self::OpenAi,
        Self::OpenRouter,
        Self::DeepSeek,
        Self::XAi,
        Self::Groq,
        Self::Mistral,
        Self::Anthropic,
        Self::Gemini,
        Self::Custom,
    ];
}

impl fmt::Display for AiProvider {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OpenAi => write!(f, "OpenAI"),
            Self::OpenRouter => write!(f, "OpenRouter"),
            Self::DeepSeek => write!(f, "DeepSeek"),
            Self::XAi => write!(f, "xAI (Grok)"),
            Self::Groq => write!(f, "Groq"),
            Self::Mistral => write!(f, "Mistral"),
            Self::Anthropic => write!(f, "Anthropic"),
            Self::Gemini => write!(f, "Gemini"),
            Self::Custom => write!(f, "Custom (OpenAI-compatible)"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(default)]
pub struct AiConfig {
    pub provider: AiProvider,
    pub base_url: String,
    pub model: String,
    pub short_bars: u16,
    pub long_bars: u16,
    pub timeout_secs: u16,
}

impl Default for AiConfig {
    fn default() -> Self {
        Self::for_provider(AiProvider::OpenAi)
    }
}

impl AiConfig {
    pub fn for_provider(provider: AiProvider) -> Self {
        let (base_url, model) = match provider {
            AiProvider::OpenAi => ("https://api.openai.com/v1", "gpt-4.1-mini"),
            AiProvider::OpenRouter => ("https://openrouter.ai/api/v1", "openai/gpt-4.1-mini"),
            AiProvider::DeepSeek => ("https://api.deepseek.com/v1", "deepseek-chat"),
            AiProvider::XAi => ("https://api.x.ai/v1", "grok-4"),
            AiProvider::Groq => ("https://api.groq.com/openai/v1", "llama-3.3-70b-versatile"),
            AiProvider::Mistral => ("https://api.mistral.ai/v1", "mistral-small-latest"),
            AiProvider::Anthropic => ("https://api.anthropic.com/v1", "claude-sonnet-4-20250514"),
            AiProvider::Gemini => (
                "https://generativelanguage.googleapis.com/v1beta",
                "gemini-2.5-flash",
            ),
            AiProvider::Custom => ("http://127.0.0.1:11434/v1", ""),
        };
        Self {
            provider,
            base_url: base_url.to_string(),
            model: model.to_string(),
            short_bars: 30,
            long_bars: 120,
            timeout_secs: 180,
        }
    }
}

fn entry_for(config: &AiConfig) -> Result<keyring::Entry, keyring::Error> {
    let account = format!(
        "{}:{}",
        config.provider,
        config.base_url.trim_end_matches('/')
    );
    keyring::Entry::new(KEYCHAIN_SERVICE, &account)
}

pub fn load_api_key(config: &AiConfig) -> Option<String> {
    entry_for(config).ok()?.get_password().ok()
}

pub fn save_api_key(config: &AiConfig, api_key: &str) -> Result<(), String> {
    if api_key.trim().is_empty() {
        return Err("API key cannot be empty.".to_string());
    }
    entry_for(config)
        .map_err(|error| format!("Failed to access the system keychain: {error}"))?
        .set_password(api_key)
        .map_err(|error| format!("Failed to store the API key: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_saved_state_uses_ai_defaults() {
        let config: AiConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config, AiConfig::default());
        assert_eq!(config.timeout_secs, 180);
    }

    #[test]
    fn provider_presets_have_endpoints_and_models() {
        for provider in AiProvider::ALL {
            let config = AiConfig::for_provider(provider);
            assert!(!config.base_url.is_empty());
            if provider != AiProvider::Custom {
                assert!(!config.model.is_empty());
            }
        }
    }
}
