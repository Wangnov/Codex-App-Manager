use std::process::Command;

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
    use super::NetworkConfig;

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
