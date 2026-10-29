use serde::Serialize;
use thiserror::Error;

use crate::app::oplock::OperationError;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("the current platform is not supported yet")]
    UnsupportedPlatform,
    #[error("update engine error: {0}")]
    Engine(String),
    /// Reality (installed bundle / feed target) no longer matches the snapshot
    /// the user confirmed — the TOCTOU guard before a destructive step. The
    /// message is already user-facing; the UI reacts to the code by silently
    /// re-checking and asking the user to confirm the fresh plan.
    #[error("{0}")]
    StaleExpectation(String),
    #[error("{0}")]
    Busy(String),
    #[error("{0}")]
    Internal(String),
}

/// Stable failure category. Drives which localized title + hint the UI shows;
/// the raw engine message stays available behind a "details" disclosure.
///
/// Engine failures (download / install / verify / OS io) all reach the boundary
/// as opaque strings — many are built ad-hoc, not from a typed `EngineError`
/// variant — so we infer the kind here from the curl exit code (a hard,
/// structured signal) and message markers. Keeping this in one backend function
/// is the point: the frontend selects copy by `code` and never string-matches.
#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub enum ErrorKind {
    Network,
    RateLimit,
    Timeout,
    DiskSpace,
    DiskWrite,
    Permission,
    InstallDirectoryUnavailable,
    Signature,
    Artifact,
    Incompatible,
    Install,
    Cancelled,
    Generic,
}

impl ErrorKind {
    /// Stable machine code sent to the frontend. `Generic` keeps the legacy
    /// `engine_error` so existing fallbacks (and `stale_expectation` etc. on the
    /// other AppError variants) stay valid.
    pub fn as_code(self) -> &'static str {
        match self {
            Self::Network => "network",
            Self::RateLimit => "rate_limit",
            Self::Timeout => "timeout",
            Self::DiskSpace => "disk_space",
            Self::DiskWrite => "disk_write",
            Self::Permission => "permission",
            Self::InstallDirectoryUnavailable => "install_directory_unavailable",
            Self::Signature => "signature",
            Self::Artifact => "artifact",
            Self::Incompatible => "incompatible",
            Self::Install => "install",
            Self::Cancelled => "cancelled",
            Self::Generic => "engine_error",
        }
    }
}

/// Pull the curl exit code out of an engine message such as
/// `"curl failed for host=… exit=23: stderr='…'"`. Uses the LAST `exit=` so a
/// combined `resume failed (…exit=A…); fresh download failed (…exit=B…)` message
/// classifies on the final (fresh) attempt the user must act on — not the resume,
/// whose range/partial failure is incidental.
fn curl_exit_code(message: &str) -> Option<i32> {
    let idx = message.rfind("exit=")?;
    let digits: String = message[idx + "exit=".len()..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// Infer a category from an opaque engine error string. Order matters: explicit
/// cancel and concrete OS/disk/permission/signature markers come first (more
/// specific than a transport code), then the curl exit code, then softer
/// keyword markers, finally `Generic` as the catch-all.
pub fn classify(message: &str) -> ErrorKind {
    let m = message.to_lowercase();

    if m.contains("github api rate limit exceeded") {
        return ErrorKind::RateLimit;
    }

    if m.contains("download cancelled") || m.contains("cancelled") {
        return ErrorKind::Cancelled;
    }
    // Disk-full beats a bare write error or a transport code.
    if m.contains("no space left")
        || m.contains("not enough space")
        || m.contains("enospc")
        || m.contains("disk is full")
        || m.contains("磁盘空间")
    {
        return ErrorKind::DiskSpace;
    }
    if m.contains("portable install directory unavailable") {
        return ErrorKind::InstallDirectoryUnavailable;
    }
    if m.contains("access is denied")
        || m.contains("permission denied")
        || m.contains("operation not permitted")
        || m.contains("administrator approval is required")
        || m.contains("operation was canceled by the user")
        || m.contains("eacces")
        || m.contains("拒绝访问")
    {
        return ErrorKind::Permission;
    }
    if m.contains("authenticode")
        || m.contains("codesign")
        || m.contains("teamidentifier")
        || m.contains("signature verification")
        || m.contains("gatekeeper")
        || m.contains("is not openai")
        || m.contains("eddsa")
    {
        return ErrorKind::Signature;
    }
    if m.contains("capability probe") || m.contains("sideload policy") || m.contains("developer mode")
    {
        return ErrorKind::Incompatible;
    }

    if let Some(code) = curl_exit_code(&m) {
        match code {
            // resolve / connect / send / recv / empty-reply / proxy resolution
            5 | 6 | 7 | 52 | 55 | 56 | 67 => return ErrorKind::Network,
            28 => return ErrorKind::Timeout,
            // TLS / certificate handshake failures → connectivity bucket
            35 | 53 | 54 | 58 | 59 | 60 | 77 | 80 | 82 | 83 | 91 => return ErrorKind::Network,
            // write to local destination failed (disk-full handled above)
            23 => return ErrorKind::DiskWrite,
            // HTTP ≥400 or max-filesize exceeded → bad/stale artifact ("try later")
            22 | 63 => return ErrorKind::Artifact,
            _ => {}
        }
    }

    if m.contains("could not resolve")
        || m.contains("failed to connect")
        || m.contains("connection refused")
        || m.contains("connection reset")
        || m.contains("network is unreachable")
        || m.contains("empty reply")
        || m.contains("schannel")
        || m.contains("ssl/tls")
    {
        return ErrorKind::Network;
    }
    if m.contains("timed out")
        || m.contains("timeout")
        || m.contains("stalled")
        || m.contains("exceeded total deadline")
    {
        return ErrorKind::Timeout;
    }
    if m.contains("failure writing output") || m.contains("write error") {
        return ErrorKind::DiskWrite;
    }
    if m.contains("add-appxpackage")
        || m.contains("binarydelta")
        || m.contains("install error")
        || m.contains("install failed")
        || m.contains("rollback failed")
    {
        return ErrorKind::Install;
    }
    if m.contains("manifest")
        || m.contains("checksum")
        || m.contains("hash mismatch")
        || m.contains("parse appcast")
        || m.contains("no usable items")
        || m.contains("invalid line")
    {
        return ErrorKind::Artifact;
    }

    ErrorKind::Generic
}

impl AppError {
    fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedPlatform => "unsupported_platform",
            // Engine errors carry the real category — inferred from the opaque
            // message — instead of collapsing everything to `engine_error`.
            Self::Engine(message) => classify(message).as_code(),
            Self::StaleExpectation(_) => "stale_expectation",
            Self::Busy(_) => "operation_busy",
            Self::Internal(_) => "internal_error",
        }
    }
}

impl From<OperationError> for AppError {
    fn from(value: OperationError) -> Self {
        Self::Busy(value.to_string())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_unix: Option<u64>,
}

impl From<AppError> for CommandError {
    fn from(value: AppError) -> Self {
        let message = value.to_string();
        let retry_after_unix = if value.code() == "rate_limit" {
            message.split("retry_after_unix=").nth(1)
                .and_then(|tail| tail.split(';').next())
                .and_then(|number| number.parse().ok())
        } else { None };
        Self {
            code: value.code().to_string(),
            message,
            retry_after_unix,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code_of(message: &str) -> &'static str {
        AppError::Engine(message.to_string()).code()
    }

    #[test]
    fn github_rate_limit_is_distinct_and_preserves_reset_time() {
        let error = CommandError::from(AppError::Engine(
            "io error: GitHub API rate limit exceeded; retry_after_unix=1800000000; retry in 60 seconds".into()));
        assert_eq!(error.code, "rate_limit");
        assert_eq!(error.retry_after_unix, Some(1800000000));
        assert_eq!(serde_json::to_value(&error).unwrap()["retryAfterUnix"], 1800000000u64);
        assert_eq!(code_of("curl failed for host=api.github.com exit=22: stderr='403 Forbidden'"), "artifact");
    }

    #[test]
    fn curl_write_error_is_disk_write_not_proxy() {
        // The exact failure from the v0.2.2 Windows bug report.
        let msg = "io error: curl failed for host=codexapp.agentsmirror.com exit=23: \
                   stderr='curl: (23) Failure writing output to destination, passed 16384 returned 640'";
        assert_eq!(classify(msg), ErrorKind::DiskWrite);
        assert_eq!(code_of(msg), "disk_write");
    }

    #[test]
    fn disk_full_beats_write_error() {
        let msg = "curl failed for host=x exit=23: stderr='... No space left on device'";
        assert_eq!(classify(msg), ErrorKind::DiskSpace);
    }

    #[test]
    fn curl_connect_and_timeout_codes() {
        assert_eq!(classify("curl failed exit=7: stderr='Failed to connect'"), ErrorKind::Network);
        assert_eq!(classify("curl failed exit=6: stderr='Could not resolve host'"), ErrorKind::Network);
        assert_eq!(classify("curl failed exit=28: stderr='Operation timed out'"), ErrorKind::Timeout);
        assert_eq!(classify("curl failed exit=35: stderr='SSL connect error'"), ErrorKind::Network);
        assert_eq!(classify("curl failed exit=55: stderr='Failed sending network data'"), ErrorKind::Network);
        assert_eq!(classify("curl failed exit=22: stderr='The requested URL returned error: 404'"), ErrorKind::Artifact);
    }

    #[test]
    fn marker_based_classification_without_exit_code() {
        assert_eq!(classify("Authenticode verification failed"), ErrorKind::Signature);
        assert_eq!(classify("codesign verification failed: invalid signature"), ErrorKind::Signature);
        assert_eq!(classify("Access is denied. (os error 5)"), ErrorKind::Permission);
        assert_eq!(classify("hash mismatch for staged package"), ErrorKind::Artifact);
        assert_eq!(classify("run Add-AppxPackage: deployment failed"), ErrorKind::Install);
        assert_eq!(classify("capability probe error: sideloading is disabled"), ErrorKind::Incompatible);
        assert_eq!(classify("download cancelled"), ErrorKind::Cancelled);
        assert_eq!(
            classify(
                "install error: run Add-AppxPackage: powershell: process exceeded total deadline"
            ),
            ErrorKind::Timeout
        );
        assert_eq!(
            classify(
                "install error: administrator approval is required to release only that transaction"
            ),
            ErrorKind::Permission
        );
    }

    #[test]
    fn portable_directory_failure_has_specific_recovery_even_for_access_denied() {
        for detail in ["Access is denied. (os error 5)", "拒绝访问。 (os error 5)", "os error 32", "os error 33"] {
            let message = format!(
                "io error: portable install directory unavailable: move current install to rollback; \
                 source=\"C:\\Codex\"; destination=\"C:\\Codex.rollback\"; {detail}"
            );
            assert_eq!(code_of(&message), "install_directory_unavailable");
        }
        assert_eq!(code_of("Access is denied. (os error 5)"), "permission");
    }

    #[test]
    fn combined_retry_message_classifies_on_the_fresh_attempt() {
        // resume's incidental range failure (33) must not mask the fresh
        // attempt's real connect failure (7).
        let msg = "resume failed (curl failed exit=33: stderr='Requested range not satisfiable'); \
                   fresh download failed (curl failed exit=7: stderr='Failed to connect to host')";
        assert_eq!(classify(msg), ErrorKind::Network);
    }

    #[test]
    fn unknown_engine_message_falls_back_to_generic() {
        assert_eq!(classify("something unexpected happened"), ErrorKind::Generic);
        assert_eq!(code_of("something unexpected happened"), "engine_error");
    }

    #[test]
    fn non_engine_variants_keep_their_codes() {
        assert_eq!(AppError::UnsupportedPlatform.code(), "unsupported_platform");
        assert_eq!(AppError::StaleExpectation("x".into()).code(), "stale_expectation");
        assert_eq!(AppError::Busy("x".into()).code(), "operation_busy");
        assert_eq!(AppError::Internal("x".into()).code(), "internal_error");
    }
}
