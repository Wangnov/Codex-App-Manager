//! HTTP range transport for the delta engine: redirect pinning, bounded
//! retries with `Retry-After`, and the curl-backed implementation.
//!
//! Layering (so every policy decision is unit-testable with no network and no
//! curl binary):
//!
//!   - [`RangeTransport`]: one HTTP attempt. `probe` resolves the package URL
//!     (following redirects, reporting curl's `url_effective` and the total
//!     size); `get_range` fetches one validated byte range into a temp file.
//!     Each attempt reports failure as an [`AttemptError`] classified by the
//!     transport (HTTP status vs curl transport failure vs fatal).
//!   - [`RangeSession`]: wraps a transport with the policy and implements
//!     [`RangeFetcher`]:
//!       * the redirect is resolved **once** by the size probe and every later
//!         range goes to that final URL, instead of bouncing through the
//!         mirror router's 302 (a presigned S3 URL with a 1-hour TTL) for each
//!         range -- fewer requests and no per-range load on the router;
//!       * a `403` on the pinned URL (expired presign) re-resolves at most
//!         [`RetryPolicy::max_reresolves`] times (default once);
//!       * HTTP 429/502/503/504 (and 408) and transient curl transport exits
//!         are retried a bounded number of times with exponential backoff,
//!         honouring a capped `Retry-After`, under a per-range attempt limit
//!         and a per-session retry budget; only the failed range is retried.
//!         When the budget runs out the error propagates and the caller falls
//!         back to the full download.
//!   - [`CurlRangeFetcher`]: [`RangeSession`] over [`CurlTransport`], the
//!     production combination (`download.rs`'s curl conventions, honours
//!     [`NetworkConfig`]).
//!
//! The pinned URL is a presigned, credential-bearing URL: it is only ever
//! passed to curl and never included in error messages (those name the
//! original, unsigned package URL).

use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use std::time::{Duration, SystemTime};

use crate::delta::executor::RangeFetcher;
use crate::network::{is_schannel_revocation_check_failure, NetworkConfig, SchannelRevocationCheck};
use crate::process::{curl_exe, hidden_command, run_capturing, run_with_progress, RunError, RunLimits};
use crate::EngineError;

/// curl exit codes that mean "the transfer broke, the same request may work
/// again": 6 could not resolve host, 7 failed to connect, 18 partial file,
/// 28 timeout, 35 TLS handshake / connection reset, 52 empty reply,
/// 55 send error, 56 receive error.
pub const TRANSIENT_CURL_EXITS: [i32; 8] = [6, 7, 18, 28, 35, 52, 55, 56];

/// Message of the error returned when the caller's cancel flag stops a delta
/// fetch. Use [`is_cancelled_error`] rather than matching the text.
pub const CANCELLED_MESSAGE: &str = "delta fetch cancelled";

/// `true` when `err` is the cancellation error of this module (the caller's
/// cancel flag was raised), as opposed to a network/verification failure. A
/// caller that wired a cancel flag should stop instead of falling back to a
/// full download when this is the outcome.
pub fn is_cancelled_error(err: &EngineError) -> bool {
    matches!(err, EngineError::Io(message) if message == CANCELLED_MESSAGE)
}

fn cancelled_error() -> EngineError {
    EngineError::Io(CANCELLED_MESSAGE.to_string())
}

/// Temp files (range bodies, header dumps) older than this that a killed
/// process left in the temp dir are removed when a fetcher is created. Longer
/// than the 30 minute per-request limit, so a live request is never touched.
const STALE_TMP_AFTER: Duration = Duration::from_secs(60 * 60);
const TMP_FILE_PREFIXES: [&str; 3] = ["delta-probe-body-", "delta-probe-headers-", "delta-range-"];

/// Best-effort removal of stale temp files from an earlier process that was
/// killed mid-request (normal paths always remove their own).
fn sweep_stale_tmp_files(tmp_dir: &Path, older_than: Duration) {
    let Ok(entries) = std::fs::read_dir(tmp_dir) else {
        return;
    };
    let now = SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !TMP_FILE_PREFIXES.iter().any(|prefix| name.starts_with(prefix)) {
            continue;
        }
        let stale = entry
            .metadata()
            .ok()
            .filter(|meta| meta.is_file())
            .and_then(|meta| meta.modified().ok())
            .and_then(|modified| now.duration_since(modified).ok())
            .is_some_and(|age| age >= older_than);
        if stale {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// HTTP statuses worth retrying after a pause.
const TRANSIENT_HTTP_STATUSES: [u16; 5] = [408, 429, 502, 503, 504];

/// Bounds for retry behaviour. All bounds are hard: exhausting any of them
/// surfaces the last error so the caller falls back to a full download.
#[derive(Debug, Clone)]
pub struct RetryPolicy {
    /// Attempts per request, first try included (`>= 1`).
    pub max_attempts: u32,
    /// Retries allowed across the whole session (every range and probe), so
    /// one flaky link cannot multiply into `ranges x max_attempts` requests.
    pub total_retry_budget: u32,
    /// Backoff before retry `n` is `base_backoff * 2^(n-1)`, capped at
    /// [`max_backoff`](Self::max_backoff).
    pub base_backoff: Duration,
    pub max_backoff: Duration,
    /// A server's `Retry-After` is honoured but never waited longer than this.
    pub max_retry_after: Duration,
    /// How many times an expired pinned URL (HTTP 403) is re-resolved from
    /// the original URL before giving up.
    pub max_reresolves: u32,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 4,
            total_retry_budget: 10,
            base_backoff: Duration::from_secs(2),
            max_backoff: Duration::from_secs(30),
            max_retry_after: Duration::from_secs(60),
            max_reresolves: 1,
        }
    }
}

impl RetryPolicy {
    /// No retries and no re-resolution: every failure is immediate.
    pub fn none() -> Self {
        Self {
            max_attempts: 1,
            total_retry_budget: 0,
            max_reresolves: 0,
            ..Self::default()
        }
    }

    /// Delay before the retry that follows failed attempt number `attempt`
    /// (1-based): exponential backoff, raised to the server's `Retry-After`
    /// when given, and never above the applicable cap.
    pub fn delay_for(&self, attempt: u32, retry_after: Option<Duration>) -> Duration {
        let exp = attempt.saturating_sub(1).min(20);
        let backoff = self
            .base_backoff
            .saturating_mul(1u32 << exp)
            .min(self.max_backoff);
        match retry_after {
            // Honour the server's ask, never waiting less than our own
            // backoff and never more than the cap.
            Some(after) => after.max(backoff).min(self.max_retry_after),
            None => backoff,
        }
    }
}

/// What the session did beyond the plan's own requests, for reporting.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RetryStats {
    /// Requests re-sent after a transient failure.
    pub retries: usize,
    /// Times the pinned URL was re-resolved after an HTTP 403.
    pub re_resolves: usize,
}

/// One failed HTTP attempt, classified by the transport.
#[derive(Debug)]
pub enum AttemptError {
    /// The server answered with an HTTP error status (curl exit 22).
    Http {
        status: u16,
        retry_after: Option<Duration>,
    },
    /// The transfer itself failed (curl exit code, or a run timeout/stall
    /// with `exit: None`).
    Transport { exit: Option<i32>, detail: String },
    /// Not worth retrying: the server misbehaved (ignored `Range`, wrong
    /// `Content-Range`), curl could not be spawned, or a non-transient curl
    /// exit.
    Fatal(EngineError),
}

impl AttemptError {
    fn is_transient(&self) -> bool {
        match self {
            Self::Http { status, .. } => TRANSIENT_HTTP_STATUSES.contains(status),
            Self::Transport { .. } => true,
            Self::Fatal(_) => false,
        }
    }

    fn retry_after(&self) -> Option<Duration> {
        match self {
            Self::Http { retry_after, .. } => *retry_after,
            _ => None,
        }
    }

    fn describe(&self) -> String {
        match self {
            Self::Http { status, .. } => format!("HTTP {status}"),
            Self::Transport { exit: Some(code), detail } => format!("curl exit {code}: {detail}"),
            Self::Transport { exit: None, detail } => detail.clone(),
            Self::Fatal(err) => err.to_string(),
        }
    }

    fn into_engine_error(self, what: &str) -> EngineError {
        match self {
            Self::Fatal(err) => err,
            other => EngineError::Io(format!("{what}: {}", other.describe())),
        }
    }
}

/// Result of resolving the package URL.
#[derive(Debug, Clone)]
pub struct Probe {
    /// The URL curl ended up at after following redirects (`url_effective`),
    /// when it could be read. `None` keeps requests on the original URL.
    pub effective_url: Option<String>,
    pub total_len: u64,
}

/// One HTTP attempt. Implementations do no retrying of their own.
pub trait RangeTransport {
    /// Follow redirects from `url`, learn the total size (a ranged GET of
    /// `bytes=0-0`: presigned mirror URLs reject `HEAD`).
    fn probe(&self, url: &str) -> Result<Probe, AttemptError>;
    /// Fetch exactly `len` bytes at `offset` from `url` into a new temp file
    /// and return its path; the caller removes it. Must verify the response
    /// really is the `206` for that range. On error, leaves no file behind.
    fn get_range(&self, url: &str, offset: u64, len: u64) -> Result<PathBuf, AttemptError>;
    /// `true` once the caller asked to stop. Checked between attempts (and
    /// before every backoff wait); an in-flight request is stopped by the
    /// transport itself. Defaults to never cancelled.
    fn is_cancelled(&self) -> bool {
        false
    }
}

struct SessionState {
    /// The resolved final URL, once known and different from the original.
    pinned_url: Option<String>,
    retries_used: u32,
    reresolves_used: u32,
}

/// A [`RangeTransport`] plus redirect pinning and the retry policy; see the
/// module docs.
pub struct RangeSession<T: RangeTransport> {
    transport: T,
    original_url: String,
    policy: RetryPolicy,
    state: Mutex<SessionState>,
    sleeper: Box<dyn Fn(Duration) + Send + Sync>,
}

impl<T: RangeTransport> RangeSession<T> {
    pub fn new(transport: T, url: impl Into<String>, policy: RetryPolicy) -> Self {
        Self::with_sleeper(transport, url, policy, Box::new(std::thread::sleep))
    }

    /// Like [`new`](Self::new) with an injectable sleep, so tests observe
    /// backoff delays instead of waiting them out.
    pub fn with_sleeper(
        transport: T,
        url: impl Into<String>,
        policy: RetryPolicy,
        sleeper: Box<dyn Fn(Duration) + Send + Sync>,
    ) -> Self {
        Self {
            transport,
            original_url: url.into(),
            policy,
            state: Mutex::new(SessionState {
                pinned_url: None,
                retries_used: 0,
                reresolves_used: 0,
            }),
            sleeper,
        }
    }

    /// URL the next range request will use (pinned final URL if resolved).
    fn current_url(&self) -> String {
        self.state
            .lock()
            .unwrap()
            .pinned_url
            .clone()
            .unwrap_or_else(|| self.original_url.clone())
    }

    fn take_retry(&self) -> bool {
        let mut state = self.state.lock().unwrap();
        if state.retries_used >= self.policy.total_retry_budget {
            return false;
        }
        state.retries_used += 1;
        true
    }

    /// Resolve the original URL and pin the result. Returns the total size.
    fn resolve(&self) -> Result<Probe, AttemptError> {
        let probe = self.transport.probe(&self.original_url)?;
        let pinned = probe
            .effective_url
            .as_deref()
            .filter(|url| url.starts_with("https://") && *url != self.original_url)
            .map(str::to_string);
        self.state.lock().unwrap().pinned_url = pinned;
        Ok(probe)
    }

    /// [`resolve`](Self::resolve) under the retry policy. A separate method
    /// (not an inline closure in `run`) so `run` does not instantiate itself
    /// recursively with an ever-nested closure type.
    fn resolve_with_retry(&self, what: &str) -> Result<Probe, EngineError> {
        let mut probe = None;
        self.run(what, true, |_| {
            probe = Some(self.resolve()?);
            Ok(())
        })?;
        Ok(probe.expect("probe succeeded"))
    }

    /// Run `op` against the current URL with the retry policy. `for_probe`
    /// runs it against the original URL (a probe always re-resolves from the
    /// source of truth) and disables the expired-presign handling.
    fn run<R>(
        &self,
        what: &str,
        for_probe: bool,
        mut op: impl FnMut(&str) -> Result<R, AttemptError>,
    ) -> Result<R, EngineError> {
        let mut attempt: u32 = 0;
        loop {
            attempt += 1;
            if self.transport.is_cancelled() {
                return Err(cancelled_error());
            }
            let url = if for_probe {
                self.original_url.clone()
            } else {
                self.current_url()
            };
            let err = match op(&url) {
                Ok(value) => return Ok(value),
                Err(err) => err,
            };

            let used_pinned = !for_probe && url != self.original_url;
            if used_pinned && matches!(err, AttemptError::Http { status: 403, .. }) {
                // The presigned URL most likely expired: go back to the
                // original URL once (bounded) for a fresh redirect.
                let may_reresolve = {
                    let mut state = self.state.lock().unwrap();
                    if state.reresolves_used < self.policy.max_reresolves {
                        state.reresolves_used += 1;
                        true
                    } else {
                        false
                    }
                };
                if !may_reresolve {
                    return Err(err.into_engine_error(&format!(
                        "{what} of {}: pinned URL rejected (HTTP 403) and re-resolve limit reached",
                        self.original_url
                    )));
                }
                // The refresh gets the same bounded retries as the initial
                // probe: a brief 429/503 from the router must not turn a
                // recoverable expiry into a full-download fallback.
                self.resolve_with_retry("re-resolve after HTTP 403")?;
                // A re-resolve is not a failed attempt of the range itself.
                attempt -= 1;
                continue;
            }

            if !err.is_transient() {
                return Err(err.into_engine_error(&format!("{what} of {}", self.original_url)));
            }
            if attempt >= self.policy.max_attempts || !self.take_retry() {
                return Err(EngineError::Io(format!(
                    "{what} of {} failed after {attempt} attempt(s): {}",
                    self.original_url,
                    err.describe()
                )));
            }
            (self.sleeper)(self.policy.delay_for(attempt, err.retry_after()));
            if self.transport.is_cancelled() {
                return Err(cancelled_error());
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn transport(&self) -> &T {
        &self.transport
    }

    pub fn stats(&self) -> RetryStats {
        let state = self.state.lock().unwrap();
        RetryStats {
            retries: state.retries_used as usize,
            re_resolves: state.reresolves_used as usize,
        }
    }

    /// The URL range requests currently go to (`None` before resolution or
    /// when the original URL never redirected). Exposed for tests; callers
    /// must treat the value as a secret (presigned).
    #[cfg(test)]
    pub(crate) fn pinned_url(&self) -> Option<String> {
        self.state.lock().unwrap().pinned_url.clone()
    }

    fn fetch_to_temp_file(&self, offset: u64, len: u64) -> Result<PathBuf, EngineError> {
        // Offsets and lengths derive from untrusted package metadata: refuse
        // a range whose end overflows before it can become a request.
        if offset.checked_add(len).is_none() {
            return Err(EngineError::Msix(format!(
                "range fetch bytes {offset}+{len} overflows the address space"
            )));
        }
        let what = format!("range fetch bytes {offset}+{len}");
        self.run(&what, false, |url| self.transport.get_range(url, offset, len))
    }
}

impl<T: RangeTransport> RangeFetcher for RangeSession<T> {
    fn total_len(&self) -> Result<u64, EngineError> {
        Ok(self.resolve_with_retry("length probe")?.total_len)
    }

    fn fetch_range(&self, offset: u64, len: u64) -> Result<Vec<u8>, EngineError> {
        if len == 0 {
            return Ok(Vec::new());
        }
        let body = self.fetch_to_temp_file(offset, len)?;
        // Remove the temp file on every path.
        let read_result =
            std::fs::read(&body).map_err(|err| EngineError::Io(format!("read fetched range: {err}")));
        let _ = std::fs::remove_file(&body);
        read_result
    }

    fn fetch_range_into(&self, offset: u64, len: u64, dest: &mut File) -> Result<u64, EngineError> {
        if len == 0 {
            return Ok(0);
        }
        let body = self.fetch_to_temp_file(offset, len)?;
        // Stream the curl output file into `dest` in bounded chunks so a
        // large coalesced range (up to several hundred MB) is never resident
        // in memory at once.
        let copy_result = (|| -> Result<u64, EngineError> {
            let mut reader = File::open(&body)
                .map_err(|err| EngineError::Io(format!("open fetched range: {err}")))?;
            std::io::copy(&mut reader, dest)
                .map_err(|err| EngineError::Io(format!("copy fetched range: {err}")))
        })();
        let _ = std::fs::remove_file(&body);
        copy_result
    }

    fn retry_stats(&self) -> RetryStats {
        self.stats()
    }
}

/// How one curl invocation failed, before classification.
enum CurlFailure {
    Exit { code: Option<i32>, stderr: String },
    Run(RunError),
}

/// curl-backed [`RangeTransport`] matching `download.rs`'s conventions
/// (`-fL`, HTTPS-only, `NetworkConfig`-aware proxy args, the same
/// Schannel-revocation-offline retry).
pub struct CurlTransport<'a> {
    network: &'a NetworkConfig,
    tmp_dir: PathBuf,
    cancel: Option<&'a AtomicBool>,
}

impl<'a> CurlTransport<'a> {
    pub fn new(network: &'a NetworkConfig, tmp_dir: impl Into<PathBuf>) -> Self {
        Self {
            network,
            tmp_dir: tmp_dir.into(),
            cancel: None,
        }
    }

    /// Stop in-flight curl processes (and further attempts) once `cancel`
    /// becomes `true`.
    pub fn with_cancel(mut self, cancel: &'a AtomicBool) -> Self {
        self.cancel = Some(cancel);
        self
    }

    /// `progress_path`, when given, is polled for its file size to detect a
    /// stalled transfer (`limits.stall`, if set, is otherwise never enforced
    /// -- `run_capturing`'s no-progress-callback form only ever checks the
    /// total deadline). It should be the path curl is writing its `-o`
    /// output to; growth in that file's size is curl making progress.
    fn run_curl(
        &self,
        url: &str,
        extra_args: &[String],
        limits: RunLimits,
        progress_path: Option<&Path>,
    ) -> Result<std::process::Output, CurlFailure> {
        let attempt = |revocation: SchannelRevocationCheck| -> Result<std::process::Output, RunError> {
            let mut command = hidden_command(curl_exe());
            let mut args = self.network.curl_args_with_schannel_revocation(revocation);
            args.extend([
                "-fL".to_string(),
                "--proto".to_string(),
                "=https".to_string(),
                "--proto-redir".to_string(),
                "=https".to_string(),
                "-sS".to_string(),
                "--connect-timeout".to_string(),
                "20".to_string(),
            ]);
            args.extend_from_slice(extra_args);
            args.push(url.to_string());
            command.args(args);
            match progress_path {
                Some(path) => run_with_progress(
                    command,
                    limits,
                    self.cancel,
                    &|| std::fs::metadata(path).map(|m| m.len()).unwrap_or(0),
                    &|_| {},
                ),
                None => run_capturing(command, limits, self.cancel),
            }
        };

        let failed = |output: &std::process::Output| CurlFailure::Exit {
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        };
        match attempt(SchannelRevocationCheck::Strict) {
            Ok(output) if output.status.success() => Ok(output),
            Ok(output) => {
                let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
                if is_schannel_revocation_check_failure(output.status.code(), &stderr) {
                    let retried =
                        attempt(SchannelRevocationCheck::Disabled).map_err(CurlFailure::Run)?;
                    if retried.status.success() {
                        Ok(retried)
                    } else {
                        Err(failed(&retried))
                    }
                } else {
                    Err(failed(&output))
                }
            }
            Err(err) => Err(CurlFailure::Run(err)),
        }
    }

    fn unique_tmp_path(&self, prefix: &str) -> PathBuf {
        self.tmp_dir.join(format!(
            "{prefix}-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ))
    }

    fn ensure_tmp_dir(&self) -> Result<(), AttemptError> {
        std::fs::create_dir_all(&self.tmp_dir).map_err(|err| {
            AttemptError::Fatal(EngineError::Io(format!("create tmp dir: {err}")))
        })
    }
}

/// Turn a curl failure (plus the header dump it wrote, which `-f` still
/// leaves behind) into a retry-classified [`AttemptError`].
fn classify_curl_failure(failure: CurlFailure, header_text: &str) -> AttemptError {
    match failure {
        CurlFailure::Exit { code: Some(22), stderr } => match parse_last_status_code(header_text) {
            Some(status) => AttemptError::Http {
                status,
                retry_after: parse_retry_after(header_text),
            },
            None => AttemptError::Fatal(EngineError::Io(format!(
                "curl failed (exit=22, no HTTP status): {stderr}"
            ))),
        },
        CurlFailure::Exit { code: Some(code), stderr } if TRANSIENT_CURL_EXITS.contains(&code) => {
            AttemptError::Transport {
                exit: Some(code),
                detail: stderr,
            }
        }
        CurlFailure::Exit { code, stderr } => AttemptError::Fatal(EngineError::Io(format!(
            "curl failed (exit={code:?}): {stderr}"
        ))),
        CurlFailure::Run(RunError::Cancelled) => AttemptError::Fatal(cancelled_error()),
        // A stalled or over-long transfer is the same class of problem as
        // curl's own timeout (exit 28).
        CurlFailure::Run(err @ RunError::Timeout { .. }) => AttemptError::Transport {
            exit: None,
            detail: format!("curl: {}", err.message()),
        },
        CurlFailure::Run(err) => {
            AttemptError::Fatal(EngineError::Io(format!("curl: {}", err.message())))
        }
    }
}

impl RangeTransport for CurlTransport<'_> {
    fn is_cancelled(&self) -> bool {
        self.cancel
            .is_some_and(|flag| flag.load(std::sync::atomic::Ordering::SeqCst))
    }

    fn probe(&self, url: &str) -> Result<Probe, AttemptError> {
        self.ensure_tmp_dir()?;
        let body = self.unique_tmp_path("delta-probe-body");
        let headers = self.unique_tmp_path("delta-probe-headers");
        let body_str = body.to_string_lossy().into_owned();
        let headers_str = headers.to_string_lossy().into_owned();

        // "bytes=0-0": the server's first byte. A short, cheap probe whose
        // response header carries the resource's total size, without
        // relying on `HEAD` (rejected by the presigned mirror URLs this is
        // built for, and by GitHub's own release-asset redirect target) or
        // downloading the resource itself. `--max-filesize` bounds an origin
        // or proxy that ignores `Range` and answers `200` with the whole
        // package to one probe-sized body (curl aborts with a non-zero exit)
        // instead of writing ~900 MB to disk before the `Content-Range`
        // check below ever ran. A *suffix* range (`bytes=-1`) is avoided:
        // GitHub Releases' backend (Azure Blob Storage) answers it with
        // `501`, while a plain forward range starting at 0 is accepted
        // everywhere observed. `-w %{url_effective}` reports where the
        // redirect chain ended (curl's stdout), which the session pins.
        const PROBE_MAX_BODY_BYTES: u64 = 64 * 1024;
        let result = self.run_curl(
            url,
            &[
                "-r".to_string(),
                "0-0".to_string(),
                "--max-filesize".to_string(),
                PROBE_MAX_BODY_BYTES.to_string(),
                "-D".to_string(),
                headers_str,
                "-o".to_string(),
                body_str,
                "-w".to_string(),
                "%{url_effective}".to_string(),
            ],
            RunLimits::total(Duration::from_secs(30)),
            None,
        );
        let header_text = std::fs::read_to_string(&headers).unwrap_or_default();
        let _ = std::fs::remove_file(&body);
        let _ = std::fs::remove_file(&headers);
        let output = result.map_err(|failure| classify_curl_failure(failure, &header_text))?;

        // Require an actual `206 Partial Content` for exactly `bytes 0-0`,
        // not just *a* Content-Range-shaped header.
        validate_range_response(&header_text, 0, 0).map_err(|err| {
            AttemptError::Fatal(EngineError::Io(format!(
                "length probe did not behave like a Range-capable server: {err}"
            )))
        })?;
        let total_len = parse_content_range_total(&header_text).ok_or_else(|| {
            AttemptError::Fatal(EngineError::Io(
                "no Content-Range header in length probe response -- does the server support HTTP Range requests?"
                    .to_string(),
            ))
        })?;
        let effective = String::from_utf8_lossy(&output.stdout).trim().to_string();
        Ok(Probe {
            effective_url: (!effective.is_empty()).then_some(effective),
            total_len,
        })
    }

    fn get_range(&self, url: &str, offset: u64, len: u64) -> Result<PathBuf, AttemptError> {
        self.ensure_tmp_dir()?;
        let body = self.unique_tmp_path("delta-range-body");
        let headers = self.unique_tmp_path("delta-range-headers");
        let body_str = body.to_string_lossy().into_owned();
        let headers_str = headers.to_string_lossy().into_owned();
        let end_inclusive = offset
            .checked_add(len)
            .and_then(|end| end.checked_sub(1))
            .ok_or_else(|| {
                AttemptError::Fatal(EngineError::Msix(format!(
                    "range bytes {offset}+{len} is empty or overflows the address space"
                )))
            })?;

        let result = self.run_curl(
            url,
            &[
                "-r".to_string(),
                format!("{offset}-{end_inclusive}"),
                // If an origin or proxy ignores the Range request and
                // answers `200` with the entire package, curl's exit status
                // alone would still look like success -- `-fL` only treats
                // HTTP error *statuses* (>=400) as failure. `--max-filesize`
                // bounds the waste: curl aborts (non-zero exit) once the
                // body exceeds `len` bytes.
                "--max-filesize".to_string(),
                len.to_string(),
                "-D".to_string(),
                headers_str,
                "-o".to_string(),
                body_str,
            ],
            RunLimits::with_stall(Duration::from_secs(30 * 60), Duration::from_secs(90)),
            Some(&body),
        );
        let header_text = std::fs::read_to_string(&headers).unwrap_or_default();
        let _ = std::fs::remove_file(&headers);
        let validated = match result {
            Ok(_) => validate_range_response(&header_text, offset, end_inclusive).map_err(|err| {
                AttemptError::Fatal(EngineError::Msix(format!(
                    "range bytes {offset}-{end_inclusive}: {err}"
                )))
            }),
            Err(failure) => Err(classify_curl_failure(failure, &header_text)),
        };
        match validated {
            Ok(()) => Ok(body),
            Err(err) => {
                // A failed or timed-out curl can still have written a
                // partial body; repeated attempts must not accumulate them.
                let _ = std::fs::remove_file(&body);
                Err(err)
            }
        }
    }
}

/// The production fetcher: [`RangeSession`] over [`CurlTransport`].
pub struct CurlRangeFetcher<'a> {
    session: RangeSession<CurlTransport<'a>>,
}

impl<'a> CurlRangeFetcher<'a> {
    /// Default [`RetryPolicy`].
    pub fn new(url: &str, network: &'a NetworkConfig, tmp_dir: impl Into<PathBuf>) -> Self {
        Self::with_policy(url, network, tmp_dir, RetryPolicy::default())
    }

    pub fn with_policy(
        url: &str,
        network: &'a NetworkConfig,
        tmp_dir: impl Into<PathBuf>,
        policy: RetryPolicy,
    ) -> Self {
        let tmp_dir = tmp_dir.into();
        sweep_stale_tmp_files(&tmp_dir, STALE_TMP_AFTER);
        Self {
            session: RangeSession::new(CurlTransport::new(network, tmp_dir), url, policy),
        }
    }

    /// Let the caller cancel: raising `cancel` kills the in-flight curl
    /// request and stops retries; the fetch then fails with an error for
    /// which [`is_cancelled_error`] is `true`. Without it a single range
    /// request can run for up to 30 minutes uninterruptibly.
    pub fn with_cancel(mut self, cancel: &'a AtomicBool) -> Self {
        self.session.transport.cancel = Some(cancel);
        self
    }
}

impl RangeFetcher for CurlRangeFetcher<'_> {
    fn total_len(&self) -> Result<u64, EngineError> {
        self.session.total_len()
    }

    fn fetch_range(&self, offset: u64, len: u64) -> Result<Vec<u8>, EngineError> {
        self.session.fetch_range(offset, len)
    }

    fn fetch_range_into(&self, offset: u64, len: u64, dest: &mut File) -> Result<u64, EngineError> {
        self.session.fetch_range_into(offset, len, dest)
    }

    fn retry_stats(&self) -> RetryStats {
        self.session.stats()
    }
}

/// Parse `Retry-After` given as delta-seconds from the final response in a
/// `-D` dump (a redirect hop's headers are ignored: each `HTTP/` status line
/// starts a new block). The HTTP-date form is ignored; the caller falls back
/// to its exponential backoff.
fn parse_retry_after(headers: &str) -> Option<Duration> {
    let mut found = None;
    for line in headers.lines() {
        let line = line.trim();
        if line.starts_with("HTTP/") {
            found = None;
        } else if let Some((name, value)) = line.split_once(':') {
            if name.trim().eq_ignore_ascii_case("retry-after") {
                found = value.trim().parse::<u64>().ok().map(Duration::from_secs);
            }
        }
    }
    found
}

/// Require the response curl just wrote `body` from to actually be the
/// `206 Partial Content` response for exactly `bytes {offset}-{end_inclusive}`
/// that was requested -- not, say, a `200` with the full resource because an
/// origin or proxy silently ignored the `Range` header. Checked against the
/// *last* status/`Content-Range` header block in a `-fL` header dump so a
/// redirect chain's final response is what gets validated.
fn validate_range_response(headers: &str, offset: u64, end_inclusive: u64) -> Result<(), String> {
    match parse_last_status_code(headers) {
        Some(206) => {}
        Some(other) => return Err(format!("expected HTTP 206 Partial Content, got {other}")),
        None => return Err("no HTTP status line in response headers".to_string()),
    }
    match parse_content_range_start_end(headers) {
        Some((start, end)) if start == offset && end == end_inclusive => Ok(()),
        Some((start, end)) => Err(format!(
            "Content-Range bytes {start}-{end} does not match the requested {offset}-{end_inclusive}"
        )),
        None => Err("no Content-Range header in response".to_string()),
    }
}

/// Parse curl's `-D` header dump for the last `HTTP/<version> <code> ...`
/// status line (may contain one per redirect hop -- the final hop's status
/// is what matters).
fn parse_last_status_code(headers: &str) -> Option<u16> {
    headers.lines().rev().find_map(|line| {
        let line = line.trim();
        line.strip_prefix("HTTP/")?
            .split_whitespace()
            .nth(1)?
            .parse::<u16>()
            .ok()
    })
}

/// Like [`parse_content_range_total`], but returns the response's declared
/// `(start, end)` byte range instead of the total resource size.
fn parse_content_range_start_end(headers: &str) -> Option<(u64, u64)> {
    headers.lines().rev().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if !name.trim().eq_ignore_ascii_case("content-range") {
            return None;
        }
        let range = value.trim().strip_prefix("bytes ")?.split_once('/')?.0;
        let (start, end) = range.split_once('-')?;
        Some((start.trim().parse().ok()?, end.trim().parse().ok()?))
    })
}

/// Parse `Content-Range: bytes X-Y/TOTAL` out of a raw curl `-D` header dump
/// (which, with `-fL`, may contain one header block per redirect hop) and
/// return `TOTAL`. The *last* occurrence in the text is used so a redirect
/// chain's final response wins over an intermediate hop's headers.
fn parse_content_range_total(headers: &str) -> Option<u64> {
    headers.lines().rev().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        if !name.trim().eq_ignore_ascii_case("content-range") {
            return None;
        }
        value.trim().rsplit('/').next()?.trim().parse::<u64>().ok()
    })
}


/// Fake, scriptable [`RangeTransport`] shared by this module's tests and the
/// executor's end-to-end tests. Serves `data`, "redirects" the original URL
/// to a numbered presigned-looking URL, and injects scripted failures.
#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Arc;

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub enum Call {
        Probe(String),
        Range { url: String, offset: u64, len: u64 },
    }

    pub struct FakeTransport {
        pub data: Vec<u8>,
        /// `true`: probes report `https://cdn.example/obj?sig=<generation>`.
        pub redirects: bool,
        /// Failures returned (front first) by successive `probe` calls
        /// before it starts succeeding.
        pub probe_failures: Mutex<VecDeque<AttemptError>>,
        /// Failures returned by successive `get_range` calls before it
        /// starts succeeding (consulted after the URL validity check).
        pub range_failures: Mutex<VecDeque<AttemptError>>,
        generation: Mutex<u32>,
        min_valid_generation: Mutex<u32>,
        /// Expire every issued URL just before the Nth `get_range` call
        /// (1-based), simulating a presign expiring mid-download.
        pub expire_before_range_call: Mutex<Option<usize>>,
        pub calls: Mutex<Vec<Call>>,
        /// Shared cancel flag reported through `is_cancelled`.
        pub cancel: Arc<std::sync::atomic::AtomicBool>,
    }

    impl FakeTransport {
        pub fn new(data: Vec<u8>, redirects: bool) -> Self {
            Self {
                data,
                redirects,
                probe_failures: Mutex::new(VecDeque::new()),
                range_failures: Mutex::new(VecDeque::new()),
                generation: Mutex::new(0),
                min_valid_generation: Mutex::new(0),
                expire_before_range_call: Mutex::new(None),
                calls: Mutex::new(Vec::new()),
                cancel: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            }
        }

        pub fn fail_ranges(&self, failures: Vec<AttemptError>) {
            self.range_failures.lock().unwrap().extend(failures);
        }

        pub fn fail_probes(&self, failures: Vec<AttemptError>) {
            self.probe_failures.lock().unwrap().extend(failures);
        }

        /// Invalidate every URL handed out so far (the presign expired).
        pub fn expire_issued_urls(&self) {
            *self.min_valid_generation.lock().unwrap() = *self.generation.lock().unwrap() + 1;
        }

        fn url_for(generation: u32) -> String {
            format!("https://cdn.example/obj?sig={generation}")
        }

        pub fn calls(&self) -> Vec<Call> {
            self.calls.lock().unwrap().clone()
        }

        pub fn probe_count(&self) -> usize {
            self.calls().iter().filter(|c| matches!(c, Call::Probe(_))).count()
        }

        pub fn range_calls(&self) -> Vec<(String, u64, u64)> {
            self.calls()
                .into_iter()
                .filter_map(|c| match c {
                    Call::Range { url, offset, len } => Some((url, offset, len)),
                    Call::Probe(_) => None,
                })
                .collect()
        }
    }

    impl RangeTransport for FakeTransport {
        fn is_cancelled(&self) -> bool {
            self.cancel.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn probe(&self, url: &str) -> Result<Probe, AttemptError> {
            self.calls.lock().unwrap().push(Call::Probe(url.to_string()));
            if let Some(err) = self.probe_failures.lock().unwrap().pop_front() {
                return Err(err);
            }
            let effective_url = self.redirects.then(|| {
                let mut generation = self.generation.lock().unwrap();
                *generation += 1;
                Self::url_for(*generation)
            });
            Ok(Probe {
                effective_url,
                total_len: self.data.len() as u64,
            })
        }

        fn get_range(&self, url: &str, offset: u64, len: u64) -> Result<PathBuf, AttemptError> {
            let range_call_number = {
                let mut calls = self.calls.lock().unwrap();
                calls.push(Call::Range {
                    url: url.to_string(),
                    offset,
                    len,
                });
                calls.iter().filter(|c| matches!(c, Call::Range { .. })).count()
            };
            if *self.expire_before_range_call.lock().unwrap() == Some(range_call_number) {
                self.expire_issued_urls();
            }
            if self.redirects {
                let current = *self.generation.lock().unwrap();
                let min_valid = *self.min_valid_generation.lock().unwrap();
                let issued_valid = (min_valid..=current)
                    .filter(|g| *g > 0)
                    .any(|g| Self::url_for(g) == url);
                if !issued_valid {
                    return Err(AttemptError::Http {
                        status: 403,
                        retry_after: None,
                    });
                }
            }
            if let Some(err) = self.range_failures.lock().unwrap().pop_front() {
                return Err(err);
            }
            let start = offset as usize;
            let bytes = self
                .data
                .get(start..start + len as usize)
                .ok_or_else(|| {
                    AttemptError::Fatal(EngineError::Msix("fake range out of bounds".to_string()))
                })?;
            let path = std::env::temp_dir().join(format!(
                "codex-win-delta-fake-range-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            std::fs::write(&path, bytes)
                .map_err(|err| AttemptError::Fatal(EngineError::Io(err.to_string())))?;
            Ok(path)
        }
    }

    /// A session over `transport` whose sleeps are recorded instead of
    /// waited. Returns the recorded delays alongside.
    pub fn new_session(
        transport: FakeTransport,
        policy: RetryPolicy,
    ) -> (RangeSession<FakeTransport>, Arc<Mutex<Vec<Duration>>>) {
        let sleeps = Arc::new(Mutex::new(Vec::new()));
        let recorder = Arc::clone(&sleeps);
        let session = RangeSession::with_sleeper(
            transport,
            "https://mirror.example/latest/win-x64",
            policy,
            Box::new(move |d| recorder.lock().unwrap().push(d)),
        );
        (session, sleeps)
    }

    pub fn transport_failure(exit: i32) -> AttemptError {
        AttemptError::Transport {
            exit: Some(exit),
            detail: "simulated".to_string(),
        }
    }

    pub fn http_failure(status: u16, retry_after_secs: Option<u64>) -> AttemptError {
        AttemptError::Http {
            status,
            retry_after: retry_after_secs.map(Duration::from_secs),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_content_range_total_from_a_header_dump() {
        let headers = "HTTP/2 206\r\ncontent-range: bytes 876623360-876623360/876623361\r\naccept-ranges: bytes\r\n\r\n";
        assert_eq!(parse_content_range_total(headers), Some(876623361));
    }

    #[test]
    fn parses_content_range_total_preferring_the_final_redirect_hop() {
        let headers = "\
HTTP/2 302\r\nlocation: https://mirror.example/final\r\n\r\n\
HTTP/2 206\r\nContent-Range: bytes 0-0/123456\r\n\r\n";
        assert_eq!(parse_content_range_total(headers), Some(123456));
    }

    #[test]
    fn missing_content_range_header_is_none() {
        let headers = "HTTP/2 200\r\ncontent-length: 42\r\n\r\n";
        assert_eq!(parse_content_range_total(headers), None);
    }

    #[test]
    fn validate_range_response_accepts_a_matching_206() {
        let headers = "HTTP/2 206\r\ncontent-range: bytes 100-199/876623361\r\n\r\n";
        assert!(validate_range_response(headers, 100, 199).is_ok());
    }

    #[test]
    fn validate_range_response_rejects_an_ignored_range_answered_with_200() {
        // An origin/proxy that ignores `Range` and returns the whole
        // resource -- exactly the failure mode a `--max-filesize` cap and
        // this status check exist to catch quickly instead of trusting a
        // merely-successful curl exit.
        let headers = "HTTP/2 200\r\ncontent-length: 876623361\r\n\r\n";
        let err = validate_range_response(headers, 100, 199).unwrap_err();
        assert!(err.contains("206"), "{err}");
    }

    #[test]
    fn validate_range_response_rejects_a_content_range_for_the_wrong_bytes() {
        let headers = "HTTP/2 206\r\ncontent-range: bytes 0-99/876623361\r\n\r\n";
        let err = validate_range_response(headers, 100, 199).unwrap_err();
        assert!(err.contains("does not match"), "{err}");
    }

    #[test]
    fn validate_range_response_prefers_the_final_redirect_hops_status() {
        let headers = "\
HTTP/2 302\r\nlocation: https://mirror.example/final\r\n\r\n\
HTTP/2 206\r\ncontent-range: bytes 100-199/876623361\r\n\r\n";
        assert!(validate_range_response(headers, 100, 199).is_ok());
    }


    // ---- Session policy tests over a scripted fake transport (no network).

    use super::test_support::*;
    use std::sync::Arc;

    fn payload() -> Vec<u8> {
        (0..4096u32).map(|i| (i % 251) as u8).collect()
    }

    #[test]
    fn probe_pins_the_final_url_and_every_range_uses_it() {
        let (session, sleeps) = new_session(FakeTransport::new(payload(), true), RetryPolicy::default());
        assert_eq!(session.total_len().unwrap(), 4096);
        let pinned = session.pinned_url().expect("redirect pinned");
        assert_eq!(pinned, "https://cdn.example/obj?sig=1");

        assert_eq!(session.fetch_range(10, 100).unwrap(), payload()[10..110].to_vec());
        assert_eq!(session.fetch_range(500, 50).unwrap(), payload()[500..550].to_vec());

        let transport = session.transport();
        assert_eq!(transport.probe_count(), 1, "the router is hit once, by the probe");
        assert_eq!(transport.calls()[0], Call::Probe("https://mirror.example/latest/win-x64".to_string()));
        for (url, _, _) in transport.range_calls() {
            assert_eq!(url, pinned, "range requests bypass the router");
        }
        assert!(sleeps.lock().unwrap().is_empty());
        assert_eq!(session.stats(), RetryStats::default());
    }

    #[test]
    fn without_a_redirect_ranges_use_the_original_url() {
        let (session, _) = new_session(FakeTransport::new(payload(), false), RetryPolicy::default());
        session.total_len().unwrap();
        assert_eq!(session.pinned_url(), None);
        session.fetch_range(0, 10).unwrap();
        assert_eq!(
            session.transport().range_calls()[0].0,
            "https://mirror.example/latest/win-x64"
        );
    }

    #[test]
    fn http_429_honours_retry_after_but_caps_it() {
        let transport = FakeTransport::new(payload(), false);
        transport.fail_ranges(vec![http_failure(429, Some(5)), http_failure(429, Some(3600))]);
        let (session, sleeps) = new_session(transport, RetryPolicy::default());
        assert_eq!(session.fetch_range(0, 16).unwrap(), payload()[..16].to_vec());
        // 1st: server asks 5s (> backoff 2s). 2nd: server asks an hour,
        // capped at max_retry_after (60s).
        assert_eq!(
            *sleeps.lock().unwrap(),
            vec![Duration::from_secs(5), Duration::from_secs(60)]
        );
        assert_eq!(session.stats().retries, 2);
        assert_eq!(session.transport().range_calls().len(), 3);
    }

    #[test]
    fn http_503_without_retry_after_backs_off_exponentially() {
        let transport = FakeTransport::new(payload(), false);
        transport.fail_ranges(vec![http_failure(503, None), http_failure(503, None), http_failure(503, None)]);
        let (session, sleeps) = new_session(transport, RetryPolicy::default());
        session.fetch_range(0, 16).unwrap();
        assert_eq!(
            *sleeps.lock().unwrap(),
            vec![Duration::from_secs(2), Duration::from_secs(4), Duration::from_secs(8)]
        );
    }

    #[test]
    fn transient_curl_exits_retry_only_the_failed_range() {
        for exit in TRANSIENT_CURL_EXITS {
            let transport = FakeTransport::new(payload(), true);
            let (session, _) = new_session(transport, RetryPolicy::default());
            session.total_len().unwrap();
            session.fetch_range(0, 100).unwrap();
            session.transport().fail_ranges(vec![transport_failure(exit), transport_failure(exit)]);
            session.fetch_range(1000, 100).unwrap();
            session.fetch_range(2000, 100).unwrap();

            let ranges = session.transport().range_calls();
            let offsets: Vec<u64> = ranges.iter().map(|(_, o, _)| *o).collect();
            assert_eq!(offsets, vec![0, 1000, 1000, 1000, 2000], "exit {exit}");
            assert_eq!(session.stats().retries, 2, "exit {exit}");
        }
    }

    #[test]
    fn a_range_that_keeps_failing_gives_up_after_max_attempts() {
        let transport = FakeTransport::new(payload(), true);
        let (session, _) = new_session(transport, RetryPolicy::default());
        session.total_len().unwrap();
        session
            .transport()
            .fail_ranges((0..10).map(|_| transport_failure(56)).collect());
        let err = session.fetch_range(0, 100).unwrap_err().to_string();
        assert!(err.contains("after 4 attempt"), "{err}");
        assert_eq!(session.transport().range_calls().len(), 4);
        // The presigned URL is a credential: it never appears in errors.
        assert!(!err.contains("sig="), "{err}");
        assert!(err.contains("mirror.example"), "{err}");
    }

    #[test]
    fn the_session_wide_retry_budget_bounds_total_retries() {
        let transport = FakeTransport::new(payload(), false);
        transport.fail_ranges((0..10).map(|_| http_failure(503, None)).collect());
        let policy = RetryPolicy {
            total_retry_budget: 3,
            ..RetryPolicy::default()
        };
        let (session, _) = new_session(transport, policy);
        // First range burns all 3 budgeted retries (4th attempt would still
        // be allowed per-range, but the budget is gone) and fails.
        assert!(session.fetch_range(0, 10).is_err());
        assert_eq!(session.transport().range_calls().len(), 4);
        assert_eq!(session.stats().retries, 3);
        // Budget spent: the next failure is immediate, no more retries.
        assert!(session.fetch_range(100, 10).is_err());
        assert_eq!(session.transport().range_calls().len(), 5);
    }

    #[test]
    fn non_transient_errors_are_not_retried() {
        let transport = FakeTransport::new(payload(), false);
        transport.fail_ranges(vec![http_failure(404, None)]);
        let (session, sleeps) = new_session(transport, RetryPolicy::default());
        assert!(session.fetch_range(0, 10).is_err());
        assert_eq!(session.transport().range_calls().len(), 1);
        assert!(sleeps.lock().unwrap().is_empty());

        let transport = FakeTransport::new(payload(), false);
        transport.fail_ranges(vec![AttemptError::Fatal(EngineError::Msix("ignored Range".to_string()))]);
        let (session, _) = new_session(transport, RetryPolicy::default());
        let err = session.fetch_range(0, 10).unwrap_err().to_string();
        assert!(err.contains("ignored Range"), "{err}");
        assert_eq!(session.transport().range_calls().len(), 1);
    }

    #[test]
    fn a_failing_length_probe_is_retried_too() {
        let transport = FakeTransport::new(payload(), true);
        transport.fail_probes(vec![http_failure(429, Some(1))]);
        let (session, sleeps) = new_session(transport, RetryPolicy::default());
        assert_eq!(session.total_len().unwrap(), 4096);
        assert_eq!(session.transport().probe_count(), 2);
        assert_eq!(*sleeps.lock().unwrap(), vec![Duration::from_secs(2)]);
        assert!(session.pinned_url().is_some());
    }

    #[test]
    fn an_expired_pinned_url_is_re_resolved_once_then_the_range_is_retried() {
        let (session, _) = new_session(FakeTransport::new(payload(), true), RetryPolicy::default());
        session.total_len().unwrap();
        session.fetch_range(0, 10).unwrap();
        session.transport().expire_issued_urls();

        assert_eq!(session.fetch_range(10, 10).unwrap(), payload()[10..20].to_vec());
        assert_eq!(session.stats().re_resolves, 1);
        assert_eq!(session.stats().retries, 0, "a re-resolve is not a transient retry");
        assert_eq!(session.pinned_url().unwrap(), "https://cdn.example/obj?sig=2");
        // The re-resolve probed the original URL, not the dead presigned one.
        let calls = session.transport().calls();
        assert_eq!(
            calls.iter().filter(|c| matches!(c, Call::Probe(u) if u == "https://mirror.example/latest/win-x64")).count(),
            2
        );
        // Later ranges use the fresh URL directly.
        session.fetch_range(20, 10).unwrap();
        assert_eq!(session.transport().range_calls().last().unwrap().0, "https://cdn.example/obj?sig=2");
    }

    #[test]
    fn the_re_resolve_probe_itself_is_retried_on_transient_errors() {
        let (session, sleeps) = new_session(FakeTransport::new(payload(), true), RetryPolicy::default());
        session.total_len().unwrap();
        session.transport().expire_issued_urls();
        session.transport().fail_probes(vec![http_failure(429, Some(3)), transport_failure(35)]);

        assert_eq!(session.fetch_range(0, 10).unwrap(), payload()[..10].to_vec());
        assert_eq!(session.stats().re_resolves, 1);
        assert_eq!(session.stats().retries, 2);
        assert_eq!(session.transport().probe_count(), 4, "initial + failed + failed + succeeded");
        assert_eq!(sleeps.lock().unwrap().len(), 2);
    }

    #[test]
    fn a_second_expiry_beyond_the_re_resolve_limit_fails_closed() {
        let (session, _) = new_session(FakeTransport::new(payload(), true), RetryPolicy::default());
        session.total_len().unwrap();
        session.transport().expire_issued_urls();
        session.fetch_range(0, 10).unwrap(); // re-resolves (1 of 1)
        session.transport().expire_issued_urls();
        let err = session.fetch_range(10, 10).unwrap_err().to_string();
        assert!(err.contains("re-resolve limit"), "{err}");
        assert!(!err.contains("sig="), "{err}");
        assert_eq!(session.stats().re_resolves, 1);
    }

    #[test]
    fn a_403_on_a_url_that_never_redirected_is_fatal_without_re_resolving() {
        let transport = FakeTransport::new(payload(), false);
        transport.fail_ranges(vec![http_failure(403, None)]);
        let (session, _) = new_session(transport, RetryPolicy::default());
        session.total_len().unwrap();
        assert!(session.fetch_range(0, 10).is_err());
        assert_eq!(session.transport().probe_count(), 1);
        assert_eq!(session.stats().re_resolves, 0);
    }

    #[test]
    fn retry_policy_none_fails_on_the_first_error() {
        let transport = FakeTransport::new(payload(), true);
        let (session, _) = new_session(transport, RetryPolicy::none());
        session.total_len().unwrap();
        session.transport().fail_ranges(vec![http_failure(429, Some(1))]);
        assert!(session.fetch_range(0, 10).is_err());
        assert_eq!(session.transport().range_calls().len(), 1);
        session.transport().expire_issued_urls();
        assert!(session.fetch_range(0, 10).is_err(), "no re-resolve either");
    }

    #[test]
    fn fetch_range_into_streams_the_bytes_into_the_destination() {
        let (session, _) = new_session(FakeTransport::new(payload(), false), RetryPolicy::default());
        let mut dest = tempfile_in_temp();
        session.fetch_range_into(0, 64, &mut dest.0).unwrap();
        assert_eq!(std::fs::read(&dest.1).unwrap(), payload()[..64].to_vec());
        let _ = std::fs::remove_file(&dest.1);
    }

    fn tempfile_in_temp() -> (File, PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "codex-win-delta-http-test-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        (File::create(&path).unwrap(), path)
    }

    #[test]
    fn backoff_delay_schedule() {
        let policy = RetryPolicy::default();
        assert_eq!(policy.delay_for(1, None), Duration::from_secs(2));
        assert_eq!(policy.delay_for(2, None), Duration::from_secs(4));
        assert_eq!(policy.delay_for(4, None), Duration::from_secs(16));
        assert_eq!(policy.delay_for(9, None), Duration::from_secs(30), "capped by max_backoff");
        assert_eq!(policy.delay_for(1, Some(Duration::from_secs(1))), Duration::from_secs(2), "never below our backoff");
        assert_eq!(policy.delay_for(1, Some(Duration::from_secs(45))), Duration::from_secs(45));
        assert_eq!(policy.delay_for(1, Some(Duration::from_secs(9999))), Duration::from_secs(60));
    }

    #[test]
    fn parses_retry_after_seconds_from_the_final_response_only() {
        let headers = "HTTP/2 302\r\nretry-after: 99\r\nlocation: https://x\r\n\r\nHTTP/2 429\r\nRetry-After: 7\r\n\r\n";
        assert_eq!(parse_retry_after(headers), Some(Duration::from_secs(7)));
        let redirect_only = "HTTP/2 302\r\nretry-after: 99\r\n\r\nHTTP/2 429\r\n\r\n";
        assert_eq!(parse_retry_after(redirect_only), None);
        let http_date = "HTTP/2 503\r\nretry-after: Wed, 21 Oct 2026 07:28:00 GMT\r\n\r\n";
        assert_eq!(parse_retry_after(http_date), None);
    }

    fn exit(code: i32) -> CurlFailure {
        CurlFailure::Exit { code: Some(code), stderr: "boom".to_string() }
    }

    #[test]
    fn classifies_curl_failures() {
        // HTTP status via exit 22 and the header dump curl left behind.
        let headers = "HTTP/2 302\r\nlocation: https://cdn\r\n\r\nHTTP/2 429\r\nretry-after: 12\r\n\r\n";
        match classify_curl_failure(exit(22), headers) {
            AttemptError::Http { status: 429, retry_after } => {
                assert_eq!(retry_after, Some(Duration::from_secs(12)))
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            classify_curl_failure(exit(22), "HTTP/2 403\r\n\r\n"),
            AttemptError::Http { status: 403, .. }
        ));
        assert!(matches!(classify_curl_failure(exit(22), ""), AttemptError::Fatal(_)));
        for code in TRANSIENT_CURL_EXITS {
            let err = classify_curl_failure(exit(code), "");
            assert!(matches!(err, AttemptError::Transport { exit: Some(c), .. } if c == code));
            assert!(err.is_transient());
        }
        // 63 = --max-filesize exceeded (ignored Range): never retried.
        assert!(!classify_curl_failure(exit(63), "").is_transient());
        assert!(!classify_curl_failure(exit(60), "").is_transient(), "TLS cert problem is not transient");
        assert!(classify_curl_failure(CurlFailure::Run(RunError::Timeout {
            kind: crate::process::TimeoutKind::Stall,
            partial_stderr: String::new(),
        }), "").is_transient());
        assert!(!classify_curl_failure(CurlFailure::Run(RunError::Spawn("no curl".into())), "").is_transient());
        // Only these statuses are transient.
        for (status, transient) in [(408, true), (429, true), (502, true), (503, true), (504, true), (403, false), (404, false), (416, false), (500, false)] {
            assert_eq!(AttemptError::Http { status, retry_after: None }.is_transient(), transient, "{status}");
        }
    }

    #[test]
    fn a_cancel_raised_before_a_request_stops_it_without_any_network_call() {
        let transport = FakeTransport::new(payload(), false);
        transport.cancel.store(true, std::sync::atomic::Ordering::SeqCst);
        let (session, _) = new_session(transport, RetryPolicy::default());
        let err = session.fetch_range(0, 10).unwrap_err();
        assert!(is_cancelled_error(&err), "{err}");
        assert!(session.transport().calls().is_empty());
        assert!(is_cancelled_error(&session.total_len().unwrap_err()));
    }

    #[test]
    fn a_cancel_raised_during_a_backoff_wait_stops_the_retry_loop() {
        let transport = FakeTransport::new(payload(), false);
        transport.fail_ranges((0..10).map(|_| http_failure(503, Some(30))).collect());
        let cancel = Arc::clone(&transport.cancel);
        // The "sleep" of the first backoff is where the user hits cancel.
        let session = RangeSession::with_sleeper(
            transport,
            "https://mirror.example/latest/win-x64",
            RetryPolicy::default(),
            Box::new(move |_| cancel.store(true, std::sync::atomic::Ordering::SeqCst)),
        );
        let err = session.fetch_range(0, 10).unwrap_err();
        assert!(is_cancelled_error(&err), "{err}");
        assert_eq!(session.transport().range_calls().len(), 1, "no attempt after the cancel");
    }

    #[test]
    fn a_curl_run_cancellation_is_classified_as_the_cancel_error_and_never_retried() {
        let err = classify_curl_failure(CurlFailure::Run(RunError::Cancelled), "");
        assert!(!err.is_transient());
        assert!(is_cancelled_error(&err.into_engine_error("range")));
    }

    #[test]
    fn ranges_that_overflow_are_rejected_before_any_request() {
        let (session, _) = new_session(FakeTransport::new(payload(), false), RetryPolicy::default());
        assert!(session.fetch_range(u64::MAX, 2).is_err());
        assert!(session.fetch_range(u64::MAX - 1, u64::MAX).is_err());
        let tmp = std::env::temp_dir().join(format!("codex-win-delta-overflow-{}", uuid::Uuid::new_v4()));
        let mut dest = File::create(tmp.with_extension("out")).unwrap();
        assert!(session.fetch_range_into(u64::MAX, 2, &mut dest).is_err());
        assert!(session.transport().calls().is_empty());
        let _ = std::fs::remove_file(tmp.with_extension("out"));

        // The curl transport rejects it too, before spawning anything.
        let network = NetworkConfig::default();
        let curl = CurlTransport::new(&network, &tmp);
        match curl.get_range("https://mirror.example/x", u64::MAX, 2) {
            Err(AttemptError::Fatal(_)) => {}
            other => panic!("{other:?}"),
        }
        match curl.get_range("https://mirror.example/x", 0, 0) {
            Err(AttemptError::Fatal(_)) => {}
            other => panic!("{other:?}"),
        }
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn stale_temp_files_from_a_killed_process_are_swept_but_others_are_kept() {
        let dir = std::env::temp_dir().join(format!("codex-win-delta-sweep-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for name in ["delta-range-body-1-a", "delta-range-headers-1-a", "delta-probe-body-1-a", "delta-probe-headers-1-a", "unrelated.txt"] {
            std::fs::write(dir.join(name), b"x").unwrap();
        }
        // Fresh files are never touched by the production threshold.
        sweep_stale_tmp_files(&dir, STALE_TMP_AFTER);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 5);
        // With a zero threshold every delta-* file is stale; foreign files stay.
        sweep_stale_tmp_files(&dir, Duration::ZERO);
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(left, vec![std::ffi::OsString::from("unrelated.txt")]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
