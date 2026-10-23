//! GitHub API throttling is distinct from a broken package or installation.

use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

pub(crate) fn is_github_api(url: &str) -> bool {
    url.starts_with("https://api.github.com/")
}

pub(crate) struct Response<'a> {
    pub body: &'a [u8],
    pub status: u16,
    remaining: Option<u64>,
    reset: Option<u64>,
    retry_after: Option<u64>,
}

/// curl --include emits each redirect/proxy header block before the final body.
pub(crate) fn response(mut bytes: &[u8]) -> Option<Response<'_>> {
    loop {
        let (end, delimiter) = bytes
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|end| (end, 4))
            .or_else(|| {
                bytes
                    .windows(2)
                    .position(|w| w == b"\n\n")
                    .map(|end| (end, 2))
            })?;
        let headers = std::str::from_utf8(&bytes[..end]).ok()?;
        let mut lines = headers.lines();
        let mut status = lines.next()?.split_whitespace();
        if !status.next()?.starts_with("HTTP/") {
            return None;
        }
        let status = status.next()?.parse().ok()?;
        let mut result = Response {
            body: &bytes[end + delimiter..],
            status,
            remaining: None,
            reset: None,
            retry_after: None,
        };
        for line in lines {
            let Some((key, value)) = line.split_once(':') else {
                continue;
            };
            let value = value.trim().parse().ok();
            if key.eq_ignore_ascii_case("x-ratelimit-remaining") {
                result.remaining = value;
            }
            if key.eq_ignore_ascii_case("x-ratelimit-reset") {
                result.reset = value;
            }
            if key.eq_ignore_ascii_case("retry-after") {
                result.retry_after = value;
            }
        }
        if result.body.starts_with(b"HTTP/") {
            bytes = result.body;
        } else {
            return Some(result);
        }
    }
}

impl Response<'_> {
    pub fn retry_at(&self, now: u64) -> Option<u64> {
        if self.status != 429
            && !(self.status == 403 && (self.remaining == Some(0) || self.retry_after.is_some()))
        {
            return None;
        }
        let reset = self.reset.filter(|&reset| reset > now).unwrap_or(now);
        let retry = self
            .retry_after
            .map(|seconds| now.saturating_add(seconds))
            .unwrap_or(now);
        let deadline = reset.max(retry);
        Some(if deadline > now {
            deadline
        } else {
            now.saturating_add(60)
        })
    }
}

pub(crate) fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

pub(crate) fn message(retry_at: u64) -> String {
    format!(
        "GitHub API rate limit exceeded; retry_after_unix={retry_at}; retry in {} seconds",
        retry_at.saturating_sub(unix_now())
    )
}

// Scope backoff to the effective proxy route. No tokens or proxy credentials
// are logged or persisted, and no request is made while that route is limited.
type Cooldowns = Vec<(Vec<String>, u64)>;
static COOLDOWNS: OnceLock<Mutex<Cooldowns>> = OnceLock::new();

pub(crate) fn blocked_until(proxy_args: &[String]) -> Option<u64> {
    let now = unix_now();
    let mut entries = COOLDOWNS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    entries.retain(|(_, deadline)| *deadline > now);
    entries
        .iter()
        .find(|(route, _)| route == proxy_args)
        .map(|(_, deadline)| *deadline)
}

pub(crate) fn remember(proxy_args: Vec<String>, deadline: u64) {
    let now = unix_now();
    let mut entries = COOLDOWNS
        .get_or_init(Mutex::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    entries.retain(|(route, until)| *until > now && route != &proxy_args);
    if entries.len() >= 16 {
        entries.remove(0);
    }
    entries.push((proxy_args, deadline));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn final_headers_and_body_survive_redirects_and_proxy_responses() {
        let raw = b"HTTP/1.1 200 Connection established\r\n\r\nHTTP/2 302\r\nlocation: https://api.github.com/next\r\n\r\nHTTP/2 200\r\nX-RateLimit-Remaining: 12\r\n\r\n{\"ok\":true}";
        let parsed = response(raw).unwrap();
        assert_eq!(parsed.status, 200);
        assert_eq!(parsed.body, br#"{"ok":true}"#);
        assert!(parsed.retry_at(100).is_none());
    }

    #[test]
    fn exhausted_primary_limit_preserves_the_reset_deadline() {
        let parsed =
            response(b"HTTP/2 403\r\nx-ratelimit-remaining: 0\r\nx-ratelimit-reset: 900\r\n\r\n")
                .unwrap();
        assert_eq!(parsed.retry_at(100), Some(900));
    }

    #[test]
    fn secondary_limit_respects_retry_after_and_plain_forbidden_stays_forbidden() {
        let parsed = response(b"HTTP/2 403\nRetry-After: 120\n\n").unwrap();
        assert_eq!(parsed.retry_at(100), Some(220));
        assert!(response(b"HTTP/2 403\r\nx-ratelimit-remaining: 10\r\n\r\n")
            .unwrap()
            .retry_at(100)
            .is_none());
        assert!(response(b"HTTP/2 404\r\nx-ratelimit-remaining: 0\r\n\r\n")
            .unwrap()
            .retry_at(100)
            .is_none());
        assert_eq!(
            response(b"HTTP/2 429\r\n\r\n").unwrap().retry_at(100),
            Some(160)
        );
    }

    #[test]
    fn backoff_is_scoped_to_the_same_proxy_route() {
        let route = vec![
            "--proxy".into(),
            "http://test-rate-limit.invalid:18441".into(),
        ];
        let deadline = unix_now() + 60;
        remember(route.clone(), deadline);
        assert_eq!(blocked_until(&route), Some(deadline));
        assert_eq!(
            blocked_until(&["--proxy".into(), "http://other.invalid:18442".into()]),
            None
        );
        remember(route.clone(), unix_now().saturating_sub(1));
        assert_eq!(blocked_until(&route), None);
        assert!(!is_github_api(
            "https://api.github.com.evil.invalid/repos/foo"
        ));
    }
}
