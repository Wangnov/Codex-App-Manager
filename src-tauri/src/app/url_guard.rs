use std::net::{Ipv4Addr, Ipv6Addr};

use url::{Host, Url};

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum UrlRejectReason {
    #[error("自定义源必须是 https:// 链接")]
    NotHttps,
    #[error("自定义源不能使用本机 / 内网地址")]
    PrivateOrLoopback,
    #[error("自定义源不能直接使用 IP 地址，请用域名")]
    BareIp,
    #[error("自定义源不能包含用户名/密码")]
    HasUserinfo,
    #[error("自定义源必须是基础 URL，不能包含查询参数或片段")]
    HasQueryOrFragment,
    #[error("自定义源缺少有效主机名")]
    MissingHost,
    #[error("无法解析该 URL")]
    Unparsable,
    #[error("自定义源不能为空")]
    Empty,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProxyRejectReason {
    #[error("代理不能为空")]
    Empty,
    #[error("代理地址无法解析")]
    Unparsable,
    #[error("代理协议必须是 http、https、socks5 或 socks5h")]
    UnsupportedScheme,
    #[error("代理地址缺少有效主机名")]
    MissingHost,
    #[error("代理地址不能包含用户名/密码")]
    HasUserinfo,
}

pub fn validate_custom_source(raw: &str) -> Result<String, UrlRejectReason> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(UrlRejectReason::Empty);
    }
    if raw
        .chars()
        .any(|ch| ch.is_control() || ch.is_whitespace() || ch == '\\')
    {
        return Err(UrlRejectReason::Unparsable);
    }
    let lower_raw = raw.to_ascii_lowercase();
    if let Some(rest) = lower_raw.strip_prefix("https://") {
        if rest.is_empty() || rest.starts_with('/') {
            return Err(UrlRejectReason::MissingHost);
        }
    }

    let mut url = Url::parse(raw).map_err(|_| UrlRejectReason::Unparsable)?;
    if url.scheme() != "https" {
        return Err(UrlRejectReason::NotHttps);
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(UrlRejectReason::HasUserinfo);
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(UrlRejectReason::HasQueryOrFragment);
    }

    match url.host() {
        None => return Err(UrlRejectReason::MissingHost),
        Some(Host::Ipv4(ip)) => {
            if is_blocked_ipv4(ip) {
                return Err(UrlRejectReason::PrivateOrLoopback);
            }
            return Err(UrlRejectReason::BareIp);
        }
        Some(Host::Ipv6(ip)) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                if is_blocked_ipv4(v4) {
                    return Err(UrlRejectReason::PrivateOrLoopback);
                }
            }
            if is_blocked_ipv6(ip) {
                return Err(UrlRejectReason::PrivateOrLoopback);
            }
            return Err(UrlRejectReason::BareIp);
        }
        Some(Host::Domain(domain)) => validate_domain(domain)?,
    }

    if url.port() == Some(443) {
        let _ = url.set_port(None);
    }
    Ok(url.to_string())
}

pub fn validate_custom_proxy(raw: &str) -> Result<String, ProxyRejectReason> {
    let raw = raw.trim();
    if raw.is_empty() {
        return Err(ProxyRejectReason::Empty);
    }
    if raw
        .chars()
        .any(|ch| ch.is_control() || ch.is_whitespace() || ch == '\\')
    {
        return Err(ProxyRejectReason::Unparsable);
    }
    let url = Url::parse(raw).map_err(|_| ProxyRejectReason::Unparsable)?;
    match url.scheme().to_ascii_lowercase().as_str() {
        "http" | "https" | "socks5" | "socks5h" => {}
        _ => return Err(ProxyRejectReason::UnsupportedScheme),
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(ProxyRejectReason::HasUserinfo);
    }
    if url.host().is_none() {
        return Err(ProxyRejectReason::MissingHost);
    }
    Ok(url.to_string())
}

fn validate_domain(domain: &str) -> Result<(), UrlRejectReason> {
    let domain = domain.trim_end_matches('.').to_ascii_lowercase();
    if domain.is_empty() {
        return Err(UrlRejectReason::MissingHost);
    }
    if domain == "localhost"
        || domain.ends_with(".localhost")
        || domain.ends_with(".local")
        || domain.ends_with(".internal")
        || domain.ends_with(".home.arpa")
    {
        return Err(UrlRejectReason::PrivateOrLoopback);
    }
    Ok(())
}

fn is_blocked_ipv4(ip: Ipv4Addr) -> bool {
    let [a, b, _, _] = ip.octets();
    ip.is_loopback()
        || ip.is_private()
        || ip.is_link_local()
        || ip.is_unspecified()
        || (a == 100 && (64..=127).contains(&b))
        || (a == 198 && matches!(b, 18 | 19))
}

fn is_blocked_ipv6(ip: Ipv6Addr) -> bool {
    let segments = ip.segments();
    ip.is_loopback()
        || ip.is_unspecified()
        || (segments[0] & 0xffc0) == 0xfe80
        || (segments[0] & 0xfe00) == 0xfc00
        || (segments[0] == 0x2001 && segments[1] == 0x0db8)
}

#[cfg(test)]
mod tests {
    use super::{
        validate_custom_proxy, validate_custom_source, ProxyRejectReason, UrlRejectReason,
    };

    #[test]
    fn accepts_https_domain_sources() {
        assert_eq!(
            validate_custom_source("https://codexapp.agentsmirror.com/latest/appcast.xml").unwrap(),
            "https://codexapp.agentsmirror.com/latest/appcast.xml"
        );
        assert_eq!(
            validate_custom_source("https://mirror.example.com:8443/feed").unwrap(),
            "https://mirror.example.com:8443/feed"
        );
        assert!(validate_custom_source("https://my-mirror.internal-name.com/x").is_ok());
        assert!(validate_custom_source("https://例え.テスト/").is_ok());
        assert_eq!(
            validate_custom_source("https://EXAMPLE.com:443/feed").unwrap(),
            "https://example.com/feed"
        );
    }

    #[test]
    fn rejects_non_https_schemes() {
        for raw in [
            "http://example.com/feed",
            "ftp://example.com",
            "file:///etc/passwd",
            "data:text/plain,x",
            "gopher://x",
            "htps://example.com",
        ] {
            assert_eq!(validate_custom_source(raw), Err(UrlRejectReason::NotHttps));
        }
    }

    #[test]
    fn rejects_empty_userinfo_and_bad_host_shapes() {
        for raw in ["", "   "] {
            assert_eq!(validate_custom_source(raw), Err(UrlRejectReason::Empty));
        }
        for raw in [
            "https://user@example.com/",
            "https://user:pw@example.com/",
            "https://trusted.com@evil-internal/",
        ] {
            assert_eq!(
                validate_custom_source(raw),
                Err(UrlRejectReason::HasUserinfo)
            );
        }
        for raw in ["https://", "https:///path"] {
            assert!(matches!(
                validate_custom_source(raw),
                Err(UrlRejectReason::MissingHost | UrlRejectReason::Unparsable)
            ));
        }
        assert_eq!(
            validate_custom_source("https://example.com\t/feed"),
            Err(UrlRejectReason::Unparsable)
        );
    }

    #[test]
    fn rejects_query_and_fragment_on_custom_source_base_urls() {
        for raw in [
            "https://mirror.example.com?token=secret",
            "https://mirror.example.com/feed?token=secret",
            "https://mirror.example.com/#private",
            "https://mirror.example.com/feed#private",
        ] {
            assert_eq!(
                validate_custom_source(raw),
                Err(UrlRejectReason::HasQueryOrFragment),
                "{raw}"
            );
        }
    }

    #[test]
    fn rejects_loopback_private_and_local_domains() {
        for raw in [
            "https://localhost/feed",
            "https://LOCALHOST/feed",
            "https://foo.local/feed",
            "https://x.localhost/",
            "https://h.home.arpa/",
            "https://svc.internal/",
        ] {
            assert_eq!(
                validate_custom_source(raw),
                Err(UrlRejectReason::PrivateOrLoopback),
                "{raw}"
            );
        }
    }

    #[test]
    fn rejects_loopback_private_and_metadata_ips() {
        for raw in [
            "https://127.0.0.1/",
            "https://127.0.0.1:8443/",
            "https://[::1]/",
            "https://10.0.0.5/",
            "https://172.16.3.4/",
            "https://192.168.1.1/",
            "https://169.254.169.254/latest/meta-data/",
            "https://[fe80::1]/",
            "https://[fc00::1]/",
            "https://0.0.0.0/",
            "https://[::ffff:127.0.0.1]/",
        ] {
            assert_eq!(
                validate_custom_source(raw),
                Err(UrlRejectReason::PrivateOrLoopback),
                "{raw}"
            );
        }
    }

    #[test]
    fn rejects_public_bare_ips() {
        for raw in ["https://8.8.8.8/", "https://[2001:4860:4860::8888]/"] {
            assert_eq!(validate_custom_source(raw), Err(UrlRejectReason::BareIp));
        }
    }

    #[test]
    fn accepts_common_proxy_schemes_and_local_hosts() {
        for raw in [
            "http://127.0.0.1:7890",
            "https://proxy.example.com:8443",
            "socks5://127.0.0.1:1080",
            "socks5h://127.0.0.1:1080",
        ] {
            assert!(validate_custom_proxy(raw).is_ok(), "{raw}");
        }
    }

    #[test]
    fn rejects_unsupported_or_credentialed_proxy_urls() {
        assert_eq!(validate_custom_proxy(""), Err(ProxyRejectReason::Empty));
        assert_eq!(
            validate_custom_proxy("ftp://proxy.example.com"),
            Err(ProxyRejectReason::UnsupportedScheme)
        );
        assert_eq!(
            validate_custom_proxy("socks5://user:pw@127.0.0.1:1080"),
            Err(ProxyRejectReason::HasUserinfo)
        );
        assert!(matches!(
            validate_custom_proxy("socks5://"),
            Err(ProxyRejectReason::Unparsable | ProxyRejectReason::MissingHost)
        ));
    }
}
