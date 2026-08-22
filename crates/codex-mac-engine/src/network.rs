use std::net::IpAddr;
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
    destination_pin: Option<DestinationPin>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DestinationPin {
    host: String,
    port: u16,
    address: IpAddr,
}

impl NetworkConfig {
    pub fn system() -> Self {
        Self {
            proxy_mode: ProxyMode::System,
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

    pub(crate) fn curl_args(&self) -> Vec<String> {
        let mut args = match &self.proxy_mode {
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
        };
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
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self::system()
    }
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

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
        let network = NetworkConfig::custom("socks5h://127.0.0.1:7890");
        assert!(network.is_custom_proxy());
        assert_eq!(
            network.curl_args(),
            vec!["--proxy", "socks5h://127.0.0.1:7890", "--noproxy", ""]
        );
        assert!(!NetworkConfig::system().is_custom_proxy());
        assert!(!NetworkConfig::direct().is_custom_proxy());
    }

    #[test]
    fn destination_pin_preserves_tls_host_and_disables_redirects() {
        let args = NetworkConfig::system()
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
