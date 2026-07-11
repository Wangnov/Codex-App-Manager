use std::process::Command;

use url::Url;

/// Parsed host (plus explicit port) for diagnostics. The original URL remains
/// available to curl, but credentials, path, query, and fragment never cross
/// into progress events, errors, or persisted logs.
pub(crate) fn safe_url_host(raw: &str) -> String {
    let Ok(url) = Url::parse(raw.trim()) else {
        return "<invalid-url>".to_string();
    };
    let origin = url.origin().ascii_serialization();
    if origin == "null" {
        return "<invalid-url>".to_string();
    }
    origin
        .split_once("://")
        .map(|(_, host)| host.to_string())
        .unwrap_or_else(|| "<invalid-url>".to_string())
}

/// Convert curl stderr into a fixed diagnostic category. Raw stderr is used
/// transiently for retry decisions, but must not be persisted because curl can
/// repeat or normalize a credentialed/presigned URL in its message.
pub(crate) fn safe_curl_failure_message(url: &str, exit_code: Option<i32>, stderr: &str) -> String {
    let host = safe_url_host(url);
    let exit = exit_code
        .map(|code| code.to_string())
        .unwrap_or_else(|| "signal".to_string());
    let lower = stderr.to_ascii_lowercase();
    let reason = if lower.contains("no space left")
        || lower.contains("not enough space")
        || lower.contains("disk is full")
    {
        Some("disk is full")
    } else if lower.contains("access is denied") || lower.contains("permission denied") {
        Some("permission denied")
    } else if lower.contains("failure writing output") || lower.contains("write error") {
        Some("write error")
    } else if lower.contains("protocol") && lower.contains("disabled") {
        Some("protocol disabled")
    } else if lower.contains("could not resolve") {
        Some("DNS resolution failed")
    } else if lower.contains("failed to connect") || lower.contains("connection refused") {
        Some("connection failed")
    } else if lower.contains("timed out") || lower.contains("timeout") {
        Some("timeout")
    } else if lower.contains("schannel")
        || lower.contains("ssl")
        || lower.contains("tls")
        || lower.contains("certificate")
    {
        Some("TLS failure")
    } else {
        None
    };

    match reason {
        Some(reason) => format!("curl failed for host={host} exit={exit} reason='{reason}'"),
        None => format!("curl failed for host={host} exit={exit}"),
    }
}

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
    #[cfg(windows)]
    use super::SchannelRevocationCheck;
    use super::{
        is_schannel_revocation_offline, safe_curl_failure_message, safe_url_host, NetworkConfig,
    };

    #[test]
    fn diagnostic_host_and_curl_error_strip_sensitive_url_components() {
        let raw = "https://basic-user:basic-pass@downloads.example:8443/private/Codex.msix?X-Amz-Signature=presigned-secret#fragment-secret";
        assert_eq!(safe_url_host(raw), "downloads.example:8443");

        let stderr = format!("curl: (23) No space left while requesting {raw}");
        let message = safe_curl_failure_message(raw, Some(23), &stderr);
        assert_eq!(
            message,
            "curl failed for host=downloads.example:8443 exit=23 reason='disk is full'"
        );
        for secret in [
            "basic-user",
            "basic-pass",
            "/private/Codex.msix",
            "X-Amz-Signature",
            "presigned-secret",
            "fragment-secret",
        ] {
            assert!(!message.contains(secret), "diagnostic leaked {secret}");
        }
        assert_eq!(safe_url_host("not a URL with-secret"), "<invalid-url>");
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
