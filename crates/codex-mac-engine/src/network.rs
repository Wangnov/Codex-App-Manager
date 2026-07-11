use std::process::Command;

use url::Url;

/// Return only a URL's parsed origin for diagnostics. The original URL still
/// goes to curl, but credentials, path, query, and fragment must never cross
/// into persisted errors or logs.
pub(crate) fn safe_url_origin(raw: &str) -> String {
    let Ok(url) = Url::parse(raw.trim()) else {
        return "<invalid-url>".to_string();
    };
    let origin = url.origin().ascii_serialization();
    if origin == "null" {
        "<invalid-url>".to_string()
    } else {
        origin
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyMode {
    System,
    Direct,
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkConfig {
    proxy_mode: ProxyMode,
}

impl NetworkConfig {
    pub fn system() -> Self {
        Self {
            proxy_mode: ProxyMode::System,
        }
    }

    pub fn direct() -> Self {
        Self {
            proxy_mode: ProxyMode::Direct,
        }
    }

    pub fn custom(proxy_url: impl Into<String>) -> Self {
        Self {
            proxy_mode: ProxyMode::Custom(proxy_url.into()),
        }
    }

    pub(crate) fn curl_args(&self) -> Vec<String> {
        match &self.proxy_mode {
            ProxyMode::System => Vec::new(),
            ProxyMode::Direct => vec![
                "--proxy".to_string(),
                String::new(),
                "--noproxy".to_string(),
                "*".to_string(),
            ],
            ProxyMode::Custom(proxy_url) => vec![
                "--proxy".to_string(),
                proxy_url.clone(),
                "--noproxy".to_string(),
                String::new(),
            ],
        }
    }

    pub(crate) fn apply_to_command(&self, command: &mut Command) {
        let args = self.curl_args();
        if !args.is_empty() {
            command.args(args);
        }
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self::system()
    }
}

#[cfg(test)]
mod tests {
    use super::{safe_url_origin, NetworkConfig};

    #[test]
    fn diagnostic_origin_strips_all_sensitive_url_components() {
        assert_eq!(
            safe_url_origin(
                "https://basic-user:basic-pass@downloads.example:8443/private/file.zip?X-Amz-Credential=secret#fragment-secret",
            ),
            "https://downloads.example:8443"
        );
        assert_eq!(
            safe_url_origin("https://downloads.example/public/file.zip"),
            "https://downloads.example"
        );
        assert_eq!(safe_url_origin("not a URL secret-token"), "<invalid-url>");
    }

    #[test]
    fn direct_proxy_mode_disables_curl_proxy_resolution() {
        assert_eq!(
            NetworkConfig::direct().curl_args(),
            vec!["--proxy", "", "--noproxy", "*"]
        );
    }

    #[test]
    fn custom_proxy_mode_preserves_socks5h_scheme() {
        assert_eq!(
            NetworkConfig::custom("socks5h://127.0.0.1:7890").curl_args(),
            vec!["--proxy", "socks5h://127.0.0.1:7890", "--noproxy", ""]
        );
    }
}
