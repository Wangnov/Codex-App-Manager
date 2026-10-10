use std::net::IpAddr;
use std::process::Command;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProxyMode {
    /// Follow the macOS system proxy. It is resolved once, when the config is
    /// built, so every request of one operation takes the same route.
    System(SystemProxyState),
    Direct,
    Custom(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NetworkConfig {
    proxy_mode: ProxyMode,
    destination_pin: Option<DestinationPin>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DestinationPin {
    host: String,
    port: u16,
    address: IpAddr,
}

/// The manual system proxy translated for curl: `url` feeds `--proxy` and
/// `bypass` feeds `--noproxy`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemProxy {
    pub url: String,
    pub bypass: String,
}

/// What "system" mode resolved to. curl never reads the macOS proxy settings
/// on its own, so without this every "system" request would connect directly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SystemProxyState {
    proxy: Option<SystemProxy>,
    /// PAC or auto-discovery is on but no manual proxy is: curl cannot evaluate
    /// either, so requests stay direct and diagnostics say so.
    auto_config_only: bool,
}

impl SystemProxyState {
    /// A resolved manual proxy (already a curl `--proxy` URL and `--noproxy`
    /// list).
    pub fn manual(url: impl Into<String>, bypass: impl Into<String>) -> Self {
        Self {
            proxy: Some(SystemProxy {
                url: url.into(),
                bypass: bypass.into(),
            }),
            auto_config_only: false,
        }
    }
}

impl NetworkConfig {
    pub fn system() -> Self {
        Self::with_system_proxy(system_proxy::resolve())
    }

    /// "System" mode with an already-resolved state — lets callers and tests
    /// pin the route instead of reading the host's live settings.
    pub fn with_system_proxy(state: SystemProxyState) -> Self {
        Self {
            proxy_mode: ProxyMode::System(state),
            destination_pin: None,
        }
    }

    pub fn direct() -> Self {
        Self {
            proxy_mode: ProxyMode::Direct,
            destination_pin: None,
        }
    }

    pub fn custom(proxy_url: impl Into<String>) -> Self {
        Self {
            proxy_mode: ProxyMode::Custom(proxy_url.into()),
            destination_pin: None,
        }
    }

    /// Custom HTTP(S)/SOCKS proxies can resolve a public DoH endpoint even when
    /// the local resolver cannot see the requested update host.
    pub fn is_custom_proxy(&self) -> bool {
        matches!(&self.proxy_mode, ProxyMode::Custom(_))
    }

    /// Whether requests leave through a proxy — a custom one, or a resolved
    /// system proxy. Such a proxy can reach hosts the local resolver cannot
    /// see, exactly like a custom proxy.
    pub fn routes_through_proxy(&self) -> bool {
        match &self.proxy_mode {
            ProxyMode::System(state) => state.proxy.is_some(),
            ProxyMode::Direct => false,
            ProxyMode::Custom(_) => true,
        }
    }

    /// Pin a custom HTTPS request to an address that the app layer has already
    /// classified as public. `curl --connect-to` preserves the original URL
    /// hostname for TLS SNI/certificate checks while also forcing an HTTP/SOCKS
    /// proxy to connect to this exact IP instead of resolving the hostname
    /// again. Redirects are disabled so a second, unvalidated host cannot escape
    /// the pin.
    pub fn with_https_destination_pin(
        &self,
        host: impl Into<String>,
        port: u16,
        address: IpAddr,
    ) -> Self {
        let mut pinned = self.clone();
        pinned.destination_pin = Some(DestinationPin {
            host: host.into(),
            port,
            address,
        });
        pinned
    }

    /// The proxy arguments alone, for app-layer curl calls that do not go
    /// through this engine (e.g. the theme catalog).
    pub fn curl_proxy_args(&self) -> Vec<String> {
        match &self.proxy_mode {
            ProxyMode::System(state) => match &state.proxy {
                Some(proxy) => vec![
                    "--proxy".to_string(),
                    proxy.url.clone(),
                    "--noproxy".to_string(),
                    proxy.bypass.clone(),
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
        let mut args = self.curl_proxy_args();
        if let Some(pin) = &self.destination_pin {
            let address = match pin.address {
                IpAddr::V4(address) => address.to_string(),
                IpAddr::V6(address) => format!("[{address}]"),
            };
            args.extend([
                "--connect-to".to_string(),
                format!("{}:{}:{}:{}", pin.host, pin.port, address, pin.port),
                "--max-redirs".to_string(),
                "0".to_string(),
            ]);
        }
        args
    }

    pub(crate) fn apply_to_command(&self, command: &mut Command) {
        let args = self.curl_args();
        if !args.is_empty() {
            command.args(args);
        }
    }

    /// One-line proxy state for connectivity-failure diagnostics. Custom URLs
    /// are never included and credentials are stripped from system ones.
    pub fn proxy_summary(&self) -> String {
        match &self.proxy_mode {
            ProxyMode::System(state) => match &state.proxy {
                Some(proxy) => format!("system (resolved to {})", redact_userinfo(&proxy.url)),
                None if state.auto_config_only => {
                    "system (PAC/auto-discovery only; curl cannot evaluate it, so requests go direct)"
                        .to_string()
                }
                None => "system (no manual proxy configured)".to_string(),
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

/// curl exit codes for failures to reach the peer at all — the ones where the
/// proxy route is the first thing worth knowing.
pub(crate) fn is_connectivity_exit(exit_code: Option<i32>) -> bool {
    matches!(
        exit_code,
        Some(5 | 6 | 7 | 28 | 35 | 52 | 53 | 54 | 55 | 56 | 58 | 59 | 60 | 67 | 77 | 80 | 82 | 83 | 91 | 97)
    )
}

fn redact_userinfo(url: &str) -> String {
    let Some((scheme, rest)) = url.split_once("://") else {
        return url.to_string();
    };
    match rest.rsplit_once('@') {
        Some((_, host)) => format!("{scheme}://***@{host}"),
        None => url.to_string(),
    }
}

#[cfg(any(target_os = "macos", test))]
/// The macOS proxy settings that matter to curl, as plain data so the
/// translation is testable on any host.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SystemProxySettings {
    pub https: Option<ProxyEndpoint>,
    pub http: Option<ProxyEndpoint>,
    pub socks: Option<ProxyEndpoint>,
    pub exceptions: Vec<String>,
    pub auto_config: bool,
}

#[cfg(any(target_os = "macos", test))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProxyEndpoint {
    pub host: String,
    pub port: Option<u16>,
}

#[cfg(any(target_os = "macos", test))]
/// Downloads are HTTPS-only, so the HTTPS proxy wins, then the HTTP proxy
/// (both are plain HTTP proxies reached with CONNECT), then SOCKS. SOCKS uses
/// `socks5h` so the proxy resolves names, as browsers do.
pub(crate) fn translate_system_proxy(settings: &SystemProxySettings) -> SystemProxyState {
    let proxy = settings
        .https
        .as_ref()
        .or(settings.http.as_ref())
        .and_then(|endpoint| proxy_url("http", endpoint))
        .or_else(|| {
            settings
                .socks
                .as_ref()
                .and_then(|endpoint| proxy_url("socks5h", endpoint))
        })
        .map(|url| SystemProxy {
            url,
            bypass: curl_noproxy_list(&settings.exceptions),
        });
    SystemProxyState {
        auto_config_only: proxy.is_none() && settings.auto_config,
        proxy,
    }
}

#[cfg(any(target_os = "macos", test))]
/// macOS stores a bare host plus a separate port; tolerate hosts that already
/// carry a scheme or a port, and bracket bare IPv6 literals for curl.
fn proxy_url(scheme: &str, endpoint: &ProxyEndpoint) -> Option<String> {
    let host = endpoint.host.trim();
    let host = host
        .split_once("://")
        .map_or(host, |(_, rest)| rest)
        .trim_end_matches('/');
    if host.is_empty() {
        return None;
    }
    let (host, has_port) = if host.parse::<std::net::Ipv6Addr>().is_ok() {
        (format!("[{host}]"), false)
    } else if let Some(bracketed) = host.strip_prefix('[') {
        (host.to_string(), bracketed.contains("]:"))
    } else {
        (host.to_string(), host.contains(':'))
    };
    Some(match endpoint.port {
        Some(port) if !has_port => format!("{scheme}://{host}:{port}"),
        _ => format!("{scheme}://{host}"),
    })
}

#[cfg(any(target_os = "macos", test))]
/// macOS bypass entries use `*.domain`, `a.b.*` and short CIDR (`169.254/16`)
/// forms; curl's `--noproxy` takes a comma-separated list of domain suffixes,
/// hosts and full CIDR blocks. Entries curl cannot express are dropped.
fn curl_noproxy_list(exceptions: &[String]) -> String {
    exceptions
        .iter()
        .filter_map(|entry| curl_noproxy_entry(entry.trim()))
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(any(target_os = "macos", test))]
fn curl_noproxy_entry(entry: &str) -> Option<String> {
    if entry.is_empty() {
        return None;
    }
    if entry == "*" {
        return Some("*".to_string());
    }
    if let Some(domain) = entry.strip_prefix("*.") {
        return (!domain.is_empty() && !domain.contains('*')).then(|| format!(".{domain}"));
    }
    if let Some(prefix) = entry.strip_suffix(".*") {
        return ipv4_prefix_cidr(prefix);
    }
    if let Some((address, bits)) = entry.split_once('/') {
        return ipv4_short_cidr(address, bits).or_else(|| Some(entry.to_string()));
    }
    (!entry.contains('*')).then(|| entry.to_string())
}

#[cfg(any(target_os = "macos", test))]
/// `10` / `172.16` / `192.168.1` (from `a.b.*`) → the covering IPv4 CIDR.
fn ipv4_prefix_cidr(prefix: &str) -> Option<String> {
    let octets = parse_ipv4_octets(prefix)?;
    if octets.is_empty() || octets.len() > 3 {
        return None;
    }
    let bits = octets.len() * 8;
    Some(format!("{}/{bits}", pad_ipv4(&octets)))
}

#[cfg(any(target_os = "macos", test))]
/// `169.254/16` → `169.254.0.0/16`.
fn ipv4_short_cidr(address: &str, bits: &str) -> Option<String> {
    let octets = parse_ipv4_octets(address)?;
    let bits: u8 = bits.parse().ok()?;
    if octets.is_empty() || octets.len() > 4 || bits > 32 {
        return None;
    }
    Some(format!("{}/{bits}", pad_ipv4(&octets)))
}

#[cfg(any(target_os = "macos", test))]
fn parse_ipv4_octets(text: &str) -> Option<Vec<u8>> {
    text.split('.').map(|part| part.parse::<u8>().ok()).collect()
}

#[cfg(any(target_os = "macos", test))]
fn pad_ipv4(octets: &[u8]) -> String {
    let mut padded = octets.to_vec();
    padded.resize(4, 0);
    padded
        .iter()
        .map(u8::to_string)
        .collect::<Vec<_>>()
        .join(".")
}

#[cfg(target_os = "macos")]
mod system_proxy {
    use system_configuration::core_foundation::array::CFArray;
    use system_configuration::core_foundation::base::{CFType, TCFType};
    use system_configuration::core_foundation::dictionary::CFDictionary;
    use system_configuration::core_foundation::number::CFNumber;
    use system_configuration::core_foundation::string::{CFString, CFStringRef};
    use system_configuration::dynamic_store::SCDynamicStoreBuilder;
    use system_configuration::sys::schema_definitions::{
        kSCPropNetProxiesExceptionsList, kSCPropNetProxiesHTTPEnable, kSCPropNetProxiesHTTPPort,
        kSCPropNetProxiesHTTPProxy, kSCPropNetProxiesHTTPSEnable, kSCPropNetProxiesHTTPSPort,
        kSCPropNetProxiesHTTPSProxy, kSCPropNetProxiesProxyAutoConfigEnable,
        kSCPropNetProxiesProxyAutoDiscoveryEnable, kSCPropNetProxiesSOCKSEnable,
        kSCPropNetProxiesSOCKSPort, kSCPropNetProxiesSOCKSProxy,
    };

    use super::{translate_system_proxy, ProxyEndpoint, SystemProxySettings, SystemProxyState};

    type Proxies = CFDictionary<CFString, CFType>;

    pub(super) fn resolve() -> SystemProxyState {
        let Some(proxies) = SCDynamicStoreBuilder::new("codex-app-manager")
            .build()
            .and_then(|store| store.get_proxies())
        else {
            return SystemProxyState::default();
        };
        // SAFETY: the kSCPropNetProxies* keys are immutable CFString constants
        // exported by SystemConfiguration.framework.
        let settings = unsafe {
            SystemProxySettings {
                https: endpoint(
                    &proxies,
                    kSCPropNetProxiesHTTPSEnable,
                    kSCPropNetProxiesHTTPSProxy,
                    kSCPropNetProxiesHTTPSPort,
                ),
                http: endpoint(
                    &proxies,
                    kSCPropNetProxiesHTTPEnable,
                    kSCPropNetProxiesHTTPProxy,
                    kSCPropNetProxiesHTTPPort,
                ),
                socks: endpoint(
                    &proxies,
                    kSCPropNetProxiesSOCKSEnable,
                    kSCPropNetProxiesSOCKSProxy,
                    kSCPropNetProxiesSOCKSPort,
                ),
                exceptions: strings(&proxies, kSCPropNetProxiesExceptionsList),
                auto_config: flag(&proxies, kSCPropNetProxiesProxyAutoConfigEnable)
                    || flag(&proxies, kSCPropNetProxiesProxyAutoDiscoveryEnable),
            }
        };
        translate_system_proxy(&settings)
    }

    fn flag(proxies: &Proxies, key: CFStringRef) -> bool {
        number(proxies, key) == Some(1)
    }

    fn number(proxies: &Proxies, key: CFStringRef) -> Option<i64> {
        proxies
            .find(key)
            .and_then(|value| value.downcast::<CFNumber>())
            .and_then(|value| value.to_i64())
    }

    fn endpoint(
        proxies: &Proxies,
        enable: CFStringRef,
        host: CFStringRef,
        port: CFStringRef,
    ) -> Option<ProxyEndpoint> {
        if !flag(proxies, enable) {
            return None;
        }
        let host = proxies
            .find(host)
            .and_then(|value| value.downcast::<CFString>())
            .map(|value| value.to_string())?;
        let port = number(proxies, port).and_then(|port| u16::try_from(port).ok());
        Some(ProxyEndpoint { host, port })
    }

    fn strings(proxies: &Proxies, key: CFStringRef) -> Vec<String> {
        let Some(array) = proxies
            .find(key)
            .and_then(|value| value.downcast::<CFArray>())
        else {
            return Vec::new();
        };
        array
            .iter()
            .filter_map(|item| {
                // SAFETY: CFArray items are retained CF objects owned by the
                // array; wrapping under the get rule adds our own reference.
                let value = unsafe { CFType::wrap_under_get_rule(*item) };
                value.downcast::<CFString>().map(|value| value.to_string())
            })
            .collect()
    }
}

#[cfg(not(target_os = "macos"))]
mod system_proxy {
    use super::SystemProxyState;

    pub(super) fn resolve() -> SystemProxyState {
        SystemProxyState::default()
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    use super::{
        curl_noproxy_list, is_connectivity_exit, translate_system_proxy, NetworkConfig,
        ProxyEndpoint, SystemProxy, SystemProxySettings, SystemProxyState,
    };

    fn endpoint(host: &str, port: Option<u16>) -> Option<ProxyEndpoint> {
        Some(ProxyEndpoint {
            host: host.to_string(),
            port,
        })
    }

    fn resolved(host: &str, port: u16, bypass: &str) -> SystemProxyState {
        translate_system_proxy(&SystemProxySettings {
            https: endpoint(host, Some(port)),
            exceptions: bypass.split(',').map(str::to_string).collect(),
            ..SystemProxySettings::default()
        })
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
        let network = NetworkConfig::custom("socks5h://127.0.0.1:7890");
        assert!(network.is_custom_proxy());
        assert!(network.routes_through_proxy());
        assert_eq!(
            network.curl_args(),
            vec!["--proxy", "socks5h://127.0.0.1:7890", "--noproxy", ""]
        );
        let no_system_proxy = NetworkConfig::with_system_proxy(SystemProxyState::default());
        assert!(!no_system_proxy.is_custom_proxy());
        assert!(!no_system_proxy.routes_through_proxy());
        assert!(!NetworkConfig::direct().is_custom_proxy());
        assert!(!NetworkConfig::direct().routes_through_proxy());
    }

    #[test]
    fn system_mode_without_a_manual_proxy_adds_no_curl_arguments() {
        let network = NetworkConfig::with_system_proxy(SystemProxyState::default());
        assert!(network.curl_args().is_empty());
        assert_eq!(network.proxy_summary(), "system (no manual proxy configured)");
    }

    #[test]
    fn system_mode_passes_the_resolved_proxy_to_curl() {
        let network = NetworkConfig::with_system_proxy(resolved("127.0.0.1", 7890, "localhost"));
        assert!(network.routes_through_proxy());
        assert!(!network.is_custom_proxy());
        assert_eq!(
            network.curl_args(),
            vec!["--proxy", "http://127.0.0.1:7890", "--noproxy", "localhost"]
        );
        assert_eq!(
            network.proxy_summary(),
            "system (resolved to http://127.0.0.1:7890)"
        );
    }

    #[test]
    fn system_proxy_tolerates_hosts_with_scheme_or_port() {
        let with_scheme = SystemProxySettings {
            http: endpoint("http://proxy.example.com/", Some(3128)),
            ..SystemProxySettings::default()
        };
        assert_eq!(
            translate_system_proxy(&with_scheme).proxy.map(|proxy| proxy.url),
            Some("http://proxy.example.com:3128".to_string())
        );
        let with_port = SystemProxySettings {
            http: endpoint("127.0.0.1:7890", Some(7890)),
            ..SystemProxySettings::default()
        };
        assert_eq!(
            translate_system_proxy(&with_port).proxy.map(|proxy| proxy.url),
            Some("http://127.0.0.1:7890".to_string())
        );
        let bracketed = SystemProxySettings {
            http: endpoint("[::1]:7890", None),
            ..SystemProxySettings::default()
        };
        assert_eq!(
            translate_system_proxy(&bracketed).proxy.map(|proxy| proxy.url),
            Some("http://[::1]:7890".to_string())
        );
    }

    #[test]
    fn system_proxy_prefers_https_then_http_then_socks() {
        let all = SystemProxySettings {
            https: endpoint("10.0.0.2", Some(8443)),
            http: endpoint("10.0.0.1", Some(8080)),
            socks: endpoint("10.0.0.3", Some(1080)),
            ..SystemProxySettings::default()
        };
        assert_eq!(
            translate_system_proxy(&all).proxy.map(|proxy| proxy.url),
            Some("http://10.0.0.2:8443".to_string())
        );
        let http_only = SystemProxySettings {
            https: None,
            ..all.clone()
        };
        assert_eq!(
            translate_system_proxy(&http_only).proxy.map(|proxy| proxy.url),
            Some("http://10.0.0.1:8080".to_string())
        );
        let socks_only = SystemProxySettings {
            https: None,
            http: None,
            ..all
        };
        assert_eq!(
            translate_system_proxy(&socks_only).proxy.map(|proxy| proxy.url),
            Some("socks5h://10.0.0.3:1080".to_string())
        );
    }

    #[test]
    fn system_proxy_brackets_ipv6_hosts_and_skips_blank_hosts() {
        let ipv6 = SystemProxySettings {
            http: endpoint("::1", Some(7890)),
            ..SystemProxySettings::default()
        };
        assert_eq!(
            translate_system_proxy(&ipv6).proxy.map(|proxy| proxy.url),
            Some("http://[::1]:7890".to_string())
        );
        let blank = SystemProxySettings {
            https: endpoint("  ", Some(7890)),
            socks: endpoint("127.0.0.1", Some(7891)),
            ..SystemProxySettings::default()
        };
        assert_eq!(
            translate_system_proxy(&blank).proxy.map(|proxy| proxy.url),
            Some("socks5h://127.0.0.1:7891".to_string())
        );
    }

    #[test]
    fn pac_only_system_proxy_stays_direct_and_says_why() {
        let state = translate_system_proxy(&SystemProxySettings {
            auto_config: true,
            ..SystemProxySettings::default()
        });
        assert_eq!(state.proxy, None);
        let network = NetworkConfig::with_system_proxy(state);
        assert!(network.curl_args().is_empty());
        assert!(!network.routes_through_proxy());
        assert!(network.proxy_summary().contains("PAC"));
    }

    #[test]
    fn manual_proxy_wins_over_pac() {
        let state = translate_system_proxy(&SystemProxySettings {
            http: endpoint("127.0.0.1", Some(7890)),
            auto_config: true,
            ..SystemProxySettings::default()
        });
        assert_eq!(
            state.proxy,
            Some(SystemProxy {
                url: "http://127.0.0.1:7890".to_string(),
                bypass: String::new(),
            })
        );
    }

    #[test]
    fn macos_bypass_entries_translate_to_curl_noproxy() {
        let exceptions = [
            "*.local",
            "169.254/16",
            "10.*",
            "192.168.1.*",
            "localhost",
            "127.0.0.1",
            "fe80::/10",
            "*.*.example",
            "*",
            "",
        ]
        .map(str::to_string);
        assert_eq!(
            curl_noproxy_list(&exceptions),
            ".local,169.254.0.0/16,10.0.0.0/8,192.168.1.0/24,localhost,127.0.0.1,fe80::/10,*"
        );
    }

    #[test]
    fn proxy_summary_never_leaks_credentials() {
        let network = NetworkConfig::with_system_proxy(SystemProxyState {
            proxy: Some(SystemProxy {
                url: "http://user:secret@127.0.0.1:7890".to_string(),
                bypass: String::new(),
            }),
            auto_config_only: false,
        });
        let summary = network.proxy_summary();
        assert!(!summary.contains("secret"), "{summary}");
        assert!(summary.contains("127.0.0.1:7890"), "{summary}");
        assert_eq!(
            NetworkConfig::custom("http://user:secret@proxy:1").proxy_summary(),
            "custom"
        );
    }

    #[test]
    fn connectivity_exits_cover_connect_timeout_and_tls() {
        for code in [5, 6, 7, 28, 35, 56, 60] {
            assert!(is_connectivity_exit(Some(code)), "{code}");
        }
        for code in [22, 23, 63] {
            assert!(!is_connectivity_exit(Some(code)), "{code}");
        }
        assert!(!is_connectivity_exit(None));
    }

    #[test]
    fn destination_pin_preserves_tls_host_and_disables_redirects() {
        let args = NetworkConfig::with_system_proxy(SystemProxyState::default())
            .with_https_destination_pin(
                "updates.example.com",
                443,
                IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)),
            )
            .curl_args();
        assert_eq!(
            args,
            vec![
                "--connect-to",
                "updates.example.com:443:93.184.216.34:443",
                "--max-redirs",
                "0",
            ]
        );
    }

    #[test]
    fn destination_pin_keeps_the_system_proxy() {
        let args = NetworkConfig::with_system_proxy(resolved("127.0.0.1", 7890, ""))
            .with_https_destination_pin(
                "updates.example.com",
                443,
                IpAddr::V4(Ipv4Addr::new(93, 184, 216, 34)),
            )
            .curl_args();
        assert_eq!(
            args,
            vec![
                "--proxy",
                "http://127.0.0.1:7890",
                "--noproxy",
                "",
                "--connect-to",
                "updates.example.com:443:93.184.216.34:443",
                "--max-redirs",
                "0",
            ]
        );
    }

    #[test]
    fn destination_pin_formats_ipv6_for_curl() {
        let args = NetworkConfig::direct()
            .with_https_destination_pin(
                "updates.example.com",
                8443,
                IpAddr::V6("2606:4700:4700::1111".parse::<Ipv6Addr>().unwrap()),
            )
            .curl_args();
        assert_eq!(
            args,
            vec![
                "--proxy",
                "",
                "--noproxy",
                "*",
                "--connect-to",
                "updates.example.com:8443:[2606:4700:4700::1111]:8443",
                "--max-redirs",
                "0",
            ]
        );
    }
}
