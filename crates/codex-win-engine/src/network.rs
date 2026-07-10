use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SchannelRevocationCheck {
    Strict,
    Disabled,
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

    pub(crate) fn curl_args_with_schannel_revocation(
        &self,
        revocation_check: SchannelRevocationCheck,
    ) -> Vec<String> {
        let mut args = self.curl_args();
        if revocation_check == SchannelRevocationCheck::Disabled {
            push_schannel_no_revoke(&mut args);
        }
        args
    }

    pub(crate) fn apply_to_command_with_schannel_revocation(
        &self,
        command: &mut Command,
        revocation_check: SchannelRevocationCheck,
    ) {
        let args = self.curl_args_with_schannel_revocation(revocation_check);
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

pub(crate) fn is_schannel_revocation_offline(exit_code: Option<i32>, stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    exit_code == Some(35)
        && lower.contains("schannel")
        && (stderr.contains("CRYPT_E_REVOCATION_OFFLINE") || lower.contains("0x80092013"))
}

#[cfg(windows)]
fn push_schannel_no_revoke(args: &mut Vec<String>) {
    args.push("--ssl-no-revoke".to_string());
}

#[cfg(not(windows))]
fn push_schannel_no_revoke(_args: &mut Vec<String>) {}

#[cfg(test)]
mod tests {
    use super::{is_schannel_revocation_offline, NetworkConfig, SchannelRevocationCheck};

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

    #[test]
    fn detects_schannel_revocation_offline_failure() {
        let stderr = "curl: (35) schannel: next InitializeSecurityContext failed: CRYPT_E_REVOCATION_OFFLINE (0x80092013)";

        assert!(is_schannel_revocation_offline(Some(35), stderr));
        assert!(!is_schannel_revocation_offline(Some(6), stderr));
        assert!(!is_schannel_revocation_offline(
            Some(35),
            "curl: (35) OpenSSL SSL_connect: connection reset"
        ));
    }

    #[cfg(windows)]
    #[test]
    fn disabled_schannel_revocation_adds_windows_curl_flag_after_proxy_args() {
        assert_eq!(
            NetworkConfig::custom("http://127.0.0.1:7890")
                .curl_args_with_schannel_revocation(SchannelRevocationCheck::Disabled),
            vec![
                "--proxy",
                "http://127.0.0.1:7890",
                "--noproxy",
                "",
                "--ssl-no-revoke"
            ]
        );
    }
}
