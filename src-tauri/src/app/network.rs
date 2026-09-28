//! The saved proxy setting translated into each engine's network config, plus
//! bare curl proxy arguments for the app layer's own curl calls (the theme
//! catalog), so every request honours the same Settings > Network choice.

use crate::app::settings_store::{AppSettings, ProxyMode};
use crate::app::url_guard::validate_custom_proxy;
use crate::errors::AppError;

pub fn validated_custom_proxy(raw: &str, context: &str) -> Result<String, AppError> {
    validate_custom_proxy(raw).map_err(|e| {
        log::warn!("url_guard rejected {context} proxy reason={e}");
        AppError::Engine(e.to_string())
    })
}

pub fn mac_network_config(
    settings: &AppSettings,
) -> Result<codex_mac_engine::NetworkConfig, AppError> {
    match settings.proxy_mode {
        ProxyMode::System => Ok(codex_mac_engine::NetworkConfig::system()),
        ProxyMode::Direct => Ok(codex_mac_engine::NetworkConfig::direct()),
        ProxyMode::Custom => {
            let proxy = validated_custom_proxy(&settings.custom_proxy_url, "mac update")?;
            Ok(codex_mac_engine::NetworkConfig::custom(proxy))
        }
    }
}

pub fn win_network_config(
    settings: &AppSettings,
) -> Result<codex_win_engine::NetworkConfig, AppError> {
    match settings.proxy_mode {
        ProxyMode::System => Ok(codex_win_engine::NetworkConfig::system()),
        ProxyMode::Direct => Ok(codex_win_engine::NetworkConfig::direct()),
        ProxyMode::Custom => {
            let proxy = validated_custom_proxy(&settings.custom_proxy_url, "Windows update")?;
            Ok(codex_win_engine::NetworkConfig::custom(proxy))
        }
    }
}

/// curl proxy arguments for the saved setting on this platform.
pub fn curl_proxy_args(settings: &AppSettings) -> Result<Vec<String>, AppError> {
    #[cfg(target_os = "windows")]
    {
        Ok(win_network_config(settings)?.curl_proxy_args())
    }
    #[cfg(not(target_os = "windows"))]
    {
        Ok(mac_network_config(settings)?.curl_proxy_args())
    }
}

#[cfg(test)]
mod tests {
    use super::curl_proxy_args;
    use crate::app::settings_store::{AppSettings, ProxyMode};

    fn settings(mode: ProxyMode, custom: &str) -> AppSettings {
        AppSettings {
            proxy_mode: mode,
            custom_proxy_url: custom.to_string(),
            ..AppSettings::default()
        }
    }

    #[test]
    fn direct_mode_disables_every_proxy() {
        assert_eq!(
            curl_proxy_args(&settings(ProxyMode::Direct, "")).unwrap(),
            vec!["--proxy", "", "--noproxy", "*"]
        );
    }

    #[test]
    fn custom_mode_uses_the_validated_proxy() {
        assert_eq!(
            curl_proxy_args(&settings(ProxyMode::Custom, "socks5h://127.0.0.1:7890")).unwrap(),
            vec!["--proxy", "socks5h://127.0.0.1:7890", "--noproxy", ""]
        );
    }

    #[test]
    fn custom_mode_rejects_an_invalid_proxy() {
        assert!(curl_proxy_args(&settings(ProxyMode::Custom, "ftp://proxy")).is_err());
    }
}
