use std::process::Command;

#[cfg(windows)]
use windows::core::{w, PCWSTR};
#[cfg(windows)]
use windows::Win32::Foundation::ERROR_SUCCESS;
#[cfg(windows)]
use windows::Win32::System::Registry::{
    RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SchannelRevocationCheck {
    Strict,
    Disabled,
}

/// WinINET per-user proxy settings live under this HKCU key — the same values
/// Windows shows in Settings > Network & Internet > Proxy.
#[cfg(windows)]
const INTERNET_SETTINGS_KEY: PCWSTR =
    w!("Software\\Microsoft\\Windows\\CurrentVersion\\Internet Settings");

#[cfg(windows)]
fn reg_read_dword(value: PCWSTR) -> Option<u32> {
    let mut data = 0u32;
    let mut size = std::mem::size_of::<u32>() as u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            INTERNET_SETTINGS_KEY,
            value,
            RRF_RT_REG_DWORD,
            None,
            Some(&mut data as *mut u32 as *mut _),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS).then_some(data)
}

#[cfg(windows)]
fn reg_read_string(value: PCWSTR) -> Option<String> {
    let mut size = 0u32;
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            INTERNET_SETTINGS_KEY,
            value,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS || size < 2 {
        return None;
    }
    let mut buffer = vec![0u16; (size / 2) as usize + 1];
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            INTERNET_SETTINGS_KEY,
            value,
            RRF_RT_REG_SZ,
            None,
            Some(buffer.as_mut_ptr() as *mut _),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS {
        return None;
    }
    let end = buffer
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(buffer.len());
    Some(String::from_utf16_lossy(&buffer[..end]))
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

    /// The proxy arguments alone, for app-layer curl calls that do not go
    /// through this engine (e.g. the theme catalog).
    pub fn curl_proxy_args(&self) -> Vec<String> {
        match &self.proxy_mode {
            ProxyMode::System => match resolved_system_proxy() {
                Some(proxy) => vec![
                    "--proxy".to_string(),
                    proxy.url,
                    "--noproxy".to_string(),
                    proxy.bypass,
                ],
                None => Vec::new(),
            },
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

    pub(crate) fn curl_args(&self) -> Vec<String> {
        self.curl_proxy_args()
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

    /// One-line proxy state appended to connectivity-failure diagnostics. The
    /// custom URL is never included — it may embed credentials.
    pub(crate) fn proxy_summary(&self) -> String {
        match &self.proxy_mode {
            ProxyMode::System => match resolved_system_proxy() {
                Some(proxy) => format!("system (resolved to {})", redact_userinfo(&proxy.url)),
                None => {
                    "system (no manual proxy; PAC/WPAD cannot be evaluated)".to_string()
                }
            },
            ProxyMode::Direct => "direct".to_string(),
            ProxyMode::Custom(_) => "custom".to_string(),
        }
    }
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self::system()
    }
}

/// The manual WinINET proxy translated for curl: `url` feeds `--proxy` and
/// `bypass` feeds `--noproxy`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SystemProxy {
    pub url: String,
    pub bypass: String,
}

/// The system-proxy read for "system" mode. On Windows this resolves the
/// WinINET per-user settings; every other platform has nothing to resolve.
pub(crate) fn resolved_system_proxy() -> Option<SystemProxy> {
    let (server, overrides) = system_proxy_settings()?;
    resolve_system_proxy(&server, &overrides)
}

#[cfg(windows)]
fn system_proxy_settings() -> Option<(String, String)> {
    if reg_read_dword(w!("ProxyEnable")).unwrap_or(0) == 0 {
        return None;
    }
    let server = reg_read_string(w!("ProxyServer"))?;
    let overrides = reg_read_string(w!("ProxyOverride")).unwrap_or_default();
    Some((server, overrides))
}

#[cfg(not(windows))]
fn system_proxy_settings() -> Option<(String, String)> {
    None
}

fn resolve_system_proxy(server: &str, overrides: &str) -> Option<SystemProxy> {
    Some(SystemProxy {
        url: system_proxy_url(server)?,
        bypass: system_proxy_bypass(overrides),
    })
}

/// ProxyServer is either a single `host:port` applied to every scheme or a
/// per-scheme map (`http=h:p;https=h:p;socks=h:p`). Downloads are https-only,
/// so the https entry wins, then http, then a SOCKS fallback.
fn system_proxy_url(server: &str) -> Option<String> {
    let server = server.trim();
    if server.is_empty() {
        return None;
    }
    if !server.contains('=') {
        return Some(qualify_proxy_url(server, "http"));
    }
    let (mut https, mut http, mut socks) = (None, None, None);
    for entry in server.split(';') {
        let Some((scheme, target)) = entry.trim().split_once('=') else {
            continue;
        };
        let target = target.trim();
        if target.is_empty() {
            continue;
        }
        match scheme.trim().to_ascii_lowercase().as_str() {
            "https" => https = Some(target),
            "http" => http = Some(target),
            "socks" => socks = Some(target),
            _ => {}
        }
    }
    https
        .or(http)
        .map(|target| qualify_proxy_url(target, "http"))
        .or_else(|| socks.map(|target| qualify_proxy_url(target, "socks5h")))
}

fn qualify_proxy_url(target: &str, default_scheme: &str) -> String {
    if target.contains("://") {
        target.to_string()
    } else {
        format!("{default_scheme}://{target}")
    }
}

/// ProxyOverride is a `;`-separated bypass list that may carry the `<local>`
/// token for dotless intranet names; curl's `--noproxy` takes a `,`-separated
/// list and cannot express `<local>`, so it is dropped.
fn system_proxy_bypass(overrides: &str) -> String {
    overrides
        .split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty() && !entry.eq_ignore_ascii_case("<local>"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Strips embedded credentials from a proxy URL before it is logged.
fn redact_userinfo(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    match rest.rsplit_once('@') {
        Some((_, host)) => format!("{scheme}://***@{host}"),
        None => url.to_string(),
    }
}

pub(crate) fn is_schannel_revocation_check_failure(exit_code: Option<i32>, stderr: &str) -> bool {
    let lower = stderr.to_ascii_lowercase();
    exit_code == Some(35)
        && lower.contains("schannel")
        && (lower.contains("crypt_e_revocation_offline")
            || lower.contains("0x80092013")
            || lower.contains("crypt_e_no_revocation_check")
            || lower.contains("0x80092012"))
}

#[cfg(windows)]
fn push_schannel_no_revoke(args: &mut Vec<String>) {
    args.push("--ssl-no-revoke".to_string());
}

#[cfg(not(windows))]
fn push_schannel_no_revoke(_args: &mut Vec<String>) {}

#[cfg(test)]
mod tests {
    use super::{
        is_schannel_revocation_check_failure, redact_userinfo, resolve_system_proxy,
        system_proxy_bypass, system_proxy_url, NetworkConfig, SchannelRevocationCheck,
        SystemProxy,
    };

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
    fn curl_args_reuses_curl_proxy_args() {
        let config = NetworkConfig::custom("http://127.0.0.1:8080");
        assert_eq!(config.curl_args(), config.curl_proxy_args());

        let direct = NetworkConfig::direct();
        assert_eq!(direct.curl_args(), direct.curl_proxy_args());
    }

    #[test]
    fn redact_userinfo_strips_credentials_but_keeps_host() {
        assert_eq!(
            redact_userinfo("http://user:secret@127.0.0.1:7890"),
            "http://***@127.0.0.1:7890"
        );
        assert_eq!(
            redact_userinfo("http://127.0.0.1:7890"),
            "http://127.0.0.1:7890"
        );
        assert_eq!(redact_userinfo("not-a-url"), "not-a-url");
    }

    #[test]
    fn system_proxy_url_accepts_single_server_for_all_protocols() {
        assert_eq!(
            system_proxy_url("127.0.0.1:7890"),
            Some("http://127.0.0.1:7890".to_string())
        );
        assert_eq!(
            system_proxy_url(" https://proxy.example.com:443 "),
            Some("https://proxy.example.com:443".to_string())
        );
        assert_eq!(system_proxy_url(""), None);
        assert_eq!(system_proxy_url("   "), None);
    }

    #[test]
    fn system_proxy_url_prefers_https_entry_in_per_scheme_map() {
        assert_eq!(
            system_proxy_url("http=10.0.0.1:8080;https=10.0.0.2:8443;socks=10.0.0.3:1080"),
            Some("http://10.0.0.2:8443".to_string())
        );
        assert_eq!(
            system_proxy_url("http=10.0.0.1:8080;ftp=10.0.0.4:21"),
            Some("http://10.0.0.1:8080".to_string())
        );
        assert_eq!(
            system_proxy_url("ftp=10.0.0.4:21;socks=10.0.0.3:1080"),
            Some("socks5h://10.0.0.3:1080".to_string())
        );
        // Only unusable schemes -> nothing to hand to curl.
        assert_eq!(system_proxy_url("ftp=10.0.0.4:21"), None);
    }

    #[test]
    fn system_proxy_bypass_converts_semicolons_and_drops_local_token() {
        assert_eq!(
            system_proxy_bypass("localhost;127.*;*.internal;<local>"),
            "localhost,127.*,*.internal"
        );
        assert_eq!(system_proxy_bypass(""), "");
        assert_eq!(system_proxy_bypass("<LOCAL>"), "");
    }

    #[test]
    fn resolve_system_proxy_combines_url_and_bypass() {
        assert_eq!(
            resolve_system_proxy("127.0.0.1:7890", "localhost;<local>"),
            Some(SystemProxy {
                url: "http://127.0.0.1:7890".to_string(),
                bypass: "localhost".to_string(),
            })
        );
        assert_eq!(resolve_system_proxy("", ""), None);
    }

    #[test]
    fn detects_schannel_revocation_offline_failure() {
        for reason in ["CRYPT_E_REVOCATION_OFFLINE", "0x80092013"] {
            let stderr = format!(
                "curl: (35) schannel: next InitializeSecurityContext failed: {reason}"
            );
            assert!(is_schannel_revocation_check_failure(Some(35), &stderr));
            assert!(!is_schannel_revocation_check_failure(Some(6), &stderr));
        }
        assert!(!is_schannel_revocation_check_failure(
            Some(35),
            "curl: (35) OpenSSL SSL_connect: connection reset"
        ));
    }

    #[test]
    fn detects_schannel_no_revocation_check_failure() {
        for reason in ["CRYPT_E_NO_REVOCATION_CHECK", "0x80092012"] {
            let stderr = format!(
                "curl: (35) schannel: next InitializeSecurityContext failed: {reason}"
            );
            assert!(is_schannel_revocation_check_failure(Some(35), &stderr));
            assert!(!is_schannel_revocation_check_failure(Some(6), &stderr));
            assert!(!is_schannel_revocation_check_failure(
                Some(35),
                &stderr.replace("schannel", "OpenSSL")
            ));
        }
        assert!(!is_schannel_revocation_check_failure(
            Some(35),
            "curl: (35) schannel: unrelated TLS handshake failure"
        ));
        // A certificate that is actually revoked must never trigger the retry.
        assert!(!is_schannel_revocation_check_failure(
            Some(35),
            "curl: (35) schannel: next InitializeSecurityContext failed: CRYPT_E_REVOKED (0x80092010)"
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
