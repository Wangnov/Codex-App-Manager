//! Tracks real download/install progress for the Manager's own self-update,
//! independent of any single renderer view. `manager_install_update` drives
//! this runtime as it awaits the updater plugin's `download_and_install`
//! callbacks; `manager_get_update_runtime` lets the renderer reattach to the
//! current snapshot after a reload (or a crash-recovered relaunch) instead of
//! losing progress state, and `manager_ack_update_runtime` clears a terminal
//! (installed/error) snapshot once the renderer has shown it.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::app::op_phase::OperationPhase;
use crate::errors::ErrorKind;
use crate::app::oplock::{OperationManager, OperationToken};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ManagerUpdatePhase {
    Idle,
    Downloading,
    Installing,
    Installed,
    Error,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagerUpdateSnapshot {
    pub phase: ManagerUpdatePhase,
    pub version: Option<String>,
    pub downloaded: u64,
    pub total: Option<u64>,
    /// Stable failure category (`ErrorKind::as_code`, e.g. `network`,
    /// `signature`, `install`) while `phase` is `Error`. Deliberately not the
    /// raw updater/engine text: that can carry feed URLs and local paths and
    /// is never localized, so the renderer maps this code to localized copy.
    pub code: Option<String>,
    pub updated_at_ms: u64,
}

impl ManagerUpdateSnapshot {
    fn idle() -> Self {
        Self {
            phase: ManagerUpdatePhase::Idle,
            version: None,
            downloaded: 0,
            total: None,
            code: None,
            updated_at_ms: now_ms(),
        }
    }
}

impl Default for ManagerUpdateSnapshot {
    fn default() -> Self {
        Self::idle()
    }
}

/// Shared, process-wide progress state for the Manager's self-update. One
/// instance lives on `ManagerState`; `manager_install_update` is guarded by
/// `OperationKind::ManagerUpdate` so only one run can drive it at a time, but
/// the snapshot itself stays readable (for reattach) even across that guard.
#[derive(Default)]
pub struct ManagerUpdateRuntime {
    snapshot: Mutex<ManagerUpdateSnapshot>,
    /// Version whose bytes this process has already written to disk. Unlike
    /// `snapshot` it survives `ack()`: dismissing the "installed, awaiting
    /// relaunch" reminder must not make the next check re-offer that same
    /// version, because the running process still reports its old version
    /// until it is relaunched. Cleared only by the process restarting.
    installed_version: Mutex<Option<String>>,
    /// Single-claim guard for `manager_relaunch`: the first caller wins and
    /// actually triggers `AppHandle::request_restart`; later calls (e.g. a
    /// duplicate click while the restart is already in flight) are treated as
    /// idempotent no-ops rather than errors.
    relaunch_reserved: AtomicBool,
}

impl ManagerUpdateRuntime {
    pub fn snapshot(&self) -> ManagerUpdateSnapshot {
        self.snapshot.lock().unwrap().clone()
    }

    pub fn start_download(&self, version: &str) {
        let mut guard = self.snapshot.lock().unwrap();
        *guard = ManagerUpdateSnapshot {
            phase: ManagerUpdatePhase::Downloading,
            version: Some(version.to_string()),
            downloaded: 0,
            total: None,
            code: None,
            updated_at_ms: now_ms(),
        };
    }

    /// Accumulates a downloaded chunk. `chunk_len` is the size of the chunk
    /// just received (matching the updater plugin's `on_chunk` callback,
    /// which reports per-chunk length rather than a running total), so this
    /// method carries the running sum itself. Progress reported after the
    /// phase has already moved on (e.g. a stray callback racing the install
    /// step) is ignored so it cannot resurrect a stale byte count.
    pub fn add_progress(&self, chunk_len: u64, total: Option<u64>) {
        let mut guard = self.snapshot.lock().unwrap();
        if guard.phase != ManagerUpdatePhase::Downloading {
            return;
        }
        guard.downloaded = guard.downloaded.saturating_add(chunk_len);
        if let Some(total) = total {
            guard.total = Some(total);
        }
        guard.updated_at_ms = now_ms();
    }

    pub fn mark_installing(&self) {
        let mut guard = self.snapshot.lock().unwrap();
        guard.phase = ManagerUpdatePhase::Installing;
        guard.updated_at_ms = now_ms();
    }

    /// Records that `update.install()` returned `Ok(())`.
    ///
    /// On macOS and Linux, `tauri-plugin-updater`'s `install_inner` swaps the
    /// bundle in place and returns normally, so this fires and the renderer
    /// can show the reattach/"Relaunch Now" UI.
    ///
    /// On Windows, `install_inner` launches the NSIS/MSI installer via
    /// `ShellExecuteW` and then unconditionally calls `std::process::exit(0)`
    /// on success (tauri-plugin-updater 2.12.0, `src/updater.rs`); it never
    /// returns `Ok(())`. So on a normal successful Windows update this method
    /// is never reached, the runtime never observes `Installed`, and the
    /// reattach/relaunch UI this phase drives is unreachable there today —
    /// not merely "non-durable" against a crash. See the Windows NSIS
    /// handoff follow-up tracked in the PR that introduced this runtime.
    pub fn mark_installed(&self) {
        let mut guard = self.snapshot.lock().unwrap();
        *self.installed_version.lock().unwrap() = guard.version.clone();
        guard.phase = ManagerUpdatePhase::Installed;
        guard.code = None;
        guard.updated_at_ms = now_ms();
    }

    /// Whether `version` is the one this process already installed and is
    /// only waiting to relaunch into.
    pub fn is_installed_version(&self, version: &str) -> bool {
        self.installed_version.lock().unwrap().as_deref() == Some(version)
    }

    /// Drops an updater result that only re-describes the version this
    /// process already wrote to disk. Shared by `manager_check_update` and
    /// `manager_install_update` so neither can re-offer an installed version
    /// (the running binary keeps reporting its old version until relaunch).
    pub fn without_installed<T>(
        &self,
        update: Option<T>,
        version_of: impl Fn(&T) -> &str,
    ) -> Option<T> {
        update.filter(|update| !self.is_installed_version(version_of(update)))
    }

    pub fn mark_error(&self, code: impl Into<String>) {
        let mut guard = self.snapshot.lock().unwrap();
        guard.phase = ManagerUpdatePhase::Error;
        guard.code = Some(code.into());
        guard.updated_at_ms = now_ms();
    }

    /// Clears a terminal snapshot (Installed/Error) back to Idle. Returns
    /// whether anything changed; a no-op while a download/install is
    /// actually in flight so a stray/late ack can never erase live progress.
    pub fn ack(&self) -> bool {
        let mut guard = self.snapshot.lock().unwrap();
        if matches!(
            guard.phase,
            ManagerUpdatePhase::Installed | ManagerUpdatePhase::Error
        ) {
            *guard = ManagerUpdateSnapshot::idle();
            true
        } else {
            false
        }
    }

    /// Single-claim reservation for `manager_relaunch`. Returns `true` for
    /// the caller that wins the race and should actually restart the app.
    pub fn reserve_relaunch(&self) -> bool {
        self.relaunch_reserved
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_ok()
    }

    /// Releases a claimed reservation without restarting — used when the
    /// restart the caller was about to perform did not actually happen (an
    /// uninterruptible operation elsewhere blocked it). Without this, the
    /// single-claim guard above would permanently silence every later
    /// `manager_relaunch` call for the rest of the process's lifetime, even
    /// after the blocking operation finishes.
    pub fn release_relaunch_reservation(&self) {
        self.relaunch_reserved.store(false, Ordering::SeqCst);
    }
}

/// Minimum gap between two `manager://update-state` progress emissions. A
/// 50-100 MB installer arrives in thousands of 8-64 KB chunks; forwarding each
/// one as an IPC event would re-render every mounted consumer per chunk. The
/// runtime snapshot itself is still updated on every chunk, so anything that
/// reads it (reattach) and every phase change (which is always emitted) sees
/// the exact byte count.
pub const PROGRESS_EMIT_INTERVAL: Duration = Duration::from_millis(150);

/// Rate limiter for progress emissions: the first call always passes, later
/// calls pass once `interval` has elapsed since the last one that did.
pub struct EmitThrottle {
    interval: Duration,
    last: Option<Instant>,
}

impl EmitThrottle {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: None,
        }
    }

    pub fn should_emit(&mut self, now: Instant) -> bool {
        match self.last {
            Some(last) if now.saturating_duration_since(last) < self.interval => false,
            _ => {
                self.last = Some(now);
                true
            }
        }
    }
}

/// Which half of the self-update an updater error came from; decides the
/// fallback category for errors with no more specific meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateStage {
    /// Reading the signed update feed (`check()`), before anything is
    /// downloaded.
    Check,
    Download,
    Install,
}

/// Upper bound for one manifest check. The updater plugin applies no timeout
/// unless the builder sets one, and the check runs while the shared
/// `ManagerUpdate` operation lease is held, so a stalled feed connection would
/// otherwise block every Codex install/update/uninstall/adopt (and the
/// relaunch) until the app is quit. The manifest is a few KiB, so a total
/// request timeout is appropriate here.
pub const CHECK_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the artifact download may go without receiving a single byte
/// (from the request being sent until the first chunk, and between chunks)
/// before it is abandoned. Deliberately an inactivity limit rather than a
/// total one: the package is large, and a slow but progressing download must
/// not be killed. Applies while the `ManagerUpdate` lease is held, for the same
/// reason as [`CHECK_TIMEOUT`].
pub const DOWNLOAD_STALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Marks the last moment a download made progress, for
/// [`with_stall_timeout`].
pub struct ActivityClock {
    base: Instant,
    last_ms: std::sync::atomic::AtomicU64,
}

impl ActivityClock {
    pub fn new() -> Self {
        Self {
            base: Instant::now(),
            last_ms: std::sync::atomic::AtomicU64::new(0),
        }
    }

    /// Records activity now (called per received chunk).
    pub fn touch(&self) {
        let ms = self.base.elapsed().as_millis() as u64;
        self.last_ms.fetch_max(ms, Ordering::SeqCst);
    }

    /// Time elapsed since the last recorded activity (or since creation).
    pub fn idle(&self) -> Duration {
        let last = Duration::from_millis(self.last_ms.load(Ordering::SeqCst));
        self.base.elapsed().saturating_sub(last)
    }
}

impl Default for ActivityClock {
    fn default() -> Self {
        Self::new()
    }
}

/// The wrapped future made no progress for the whole inactivity limit and was
/// dropped (which aborts its in-flight request).
#[derive(Debug, PartialEq, Eq)]
pub struct Stalled;

/// Drives `fut` to completion unless `clock` reports no activity for `limit`,
/// in which case the future is dropped and `Err(Stalled)` returned. The future
/// is expected to call `clock.touch()` whenever it makes progress.
pub async fn with_stall_timeout<F: std::future::Future>(
    fut: F,
    clock: &ActivityClock,
    limit: Duration,
) -> Result<F::Output, Stalled> {
    let mut fut = std::pin::pin!(fut);
    loop {
        let idle = clock.idle();
        if idle >= limit {
            return Err(Stalled);
        }
        if let Ok(output) = tokio::time::timeout(limit - idle, fut.as_mut()).await {
            return Ok(output);
        }
    }
}

/// Maps a `tauri-plugin-updater` error to the stable failure category the
/// renderer localizes (`ErrorKind::as_code`). Uses the typed variants rather
/// than string-matching the message, which for transport failures is an
/// opaque `reqwest` string that the generic engine classifier cannot read.
pub fn classify_updater_error(
    stage: UpdateStage,
    error: &tauri_plugin_updater::Error,
) -> ErrorKind {
    use tauri_plugin_updater::Error as E;
    match error {
        E::Reqwest(error) if error.is_timeout() => ErrorKind::Timeout,
        E::Reqwest(error) if error.is_status() => ErrorKind::Artifact,
        E::Reqwest(_) => ErrorKind::Network,
        // The plugin reports a non-success HTTP status of the artifact as this.
        E::Network(_) | E::ReleaseNotFound => ErrorKind::Artifact,
        E::Minisign(_)
        | E::Base64(_)
        | E::SignatureUtf8(_)
        | E::SignedVersionMismatch { .. }
        | E::MissingSignedVersion => ErrorKind::Signature,
        E::AuthenticationFailed => ErrorKind::Permission,
        E::Io(io) => match io.kind() {
            std::io::ErrorKind::PermissionDenied => ErrorKind::Permission,
            std::io::ErrorKind::StorageFull => ErrorKind::DiskSpace,
            _ if stage == UpdateStage::Install => ErrorKind::Install,
            _ => ErrorKind::Generic,
        },
        _ if stage == UpdateStage::Install => ErrorKind::Install,
        _ => ErrorKind::Generic,
    }
}

/// Final pre-commit checkpoint of the self-update install. Advances the lease
/// to `Committing` and reports whether it is still safe to touch the
/// Manager's own files (`false` when a confirmed quit already armed
/// `force_quit`). `set_phase` and a confirmed quit's `prepare_quit` take the
/// SAME operation-lock mutex, so whichever reaches it first is what the other
/// observes: if the quit won, `force_quit` is visible here and the caller must
/// bail out; if this won, the quit that follows sees `Committing` and is
/// blocked, so no `app.exit()` can race the install.
pub fn enter_commit_checkpoint(
    operations: &OperationManager,
    token: &OperationToken,
    force_quit: &AtomicBool,
) -> bool {
    let _ = operations.set_phase(token, OperationPhase::Committing);
    !force_quit.load(Ordering::SeqCst)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remembers_the_installed_version_across_ack() {
        let runtime = ManagerUpdateRuntime::default();
        assert!(!runtime.is_installed_version("1.2.3"));
        runtime.start_download("1.2.3");
        assert!(!runtime.is_installed_version("1.2.3"));
        runtime.mark_installing();
        runtime.mark_installed();
        assert!(runtime.is_installed_version("1.2.3"));
        assert!(runtime.ack());
        assert_eq!(runtime.snapshot().phase, ManagerUpdatePhase::Idle);
        assert!(runtime.is_installed_version("1.2.3"));
        assert!(!runtime.is_installed_version("1.2.4"));
    }

    #[test]
    fn a_failed_install_does_not_mark_the_version_installed() {
        let runtime = ManagerUpdateRuntime::default();
        runtime.start_download("1.2.3");
        runtime.mark_installing();
        runtime.mark_error("install");
        assert!(runtime.ack());
        assert!(!runtime.is_installed_version("1.2.3"));
    }

    #[test]
    fn starts_idle() {
        let runtime = ManagerUpdateRuntime::default();
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.phase, ManagerUpdatePhase::Idle);
        assert_eq!(snapshot.downloaded, 0);
        assert!(snapshot.total.is_none());
        assert!(snapshot.code.is_none());
    }

    #[test]
    fn tracks_the_full_download_then_install_then_installed_lifecycle() {
        let runtime = ManagerUpdateRuntime::default();
        runtime.start_download("1.2.3");
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.phase, ManagerUpdatePhase::Downloading);
        assert_eq!(snapshot.version.as_deref(), Some("1.2.3"));
        assert_eq!(snapshot.downloaded, 0);

        runtime.add_progress(1_000, Some(10_000));
        runtime.add_progress(2_000, Some(10_000));
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.downloaded, 3_000);
        assert_eq!(snapshot.total, Some(10_000));

        runtime.mark_installing();
        assert_eq!(runtime.snapshot().phase, ManagerUpdatePhase::Installing);

        // Progress arriving after the phase has moved on must not resurrect
        // a stale byte count or flip the phase back.
        runtime.add_progress(500, Some(10_000));
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.downloaded, 3_000);
        assert_eq!(snapshot.phase, ManagerUpdatePhase::Installing);

        runtime.mark_installed();
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.phase, ManagerUpdatePhase::Installed);
        assert!(snapshot.code.is_none());
    }

    #[test]
    fn error_carries_a_stable_code_and_acks_back_to_idle() {
        let runtime = ManagerUpdateRuntime::default();
        runtime.start_download("1.2.3");
        runtime.mark_error("network");
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.phase, ManagerUpdatePhase::Error);
        assert_eq!(snapshot.code.as_deref(), Some("network"));

        assert!(runtime.ack());
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.phase, ManagerUpdatePhase::Idle);
        assert!(snapshot.code.is_none());
    }

    #[test]
    fn ack_is_a_no_op_while_a_download_or_install_is_in_flight() {
        let runtime = ManagerUpdateRuntime::default();
        runtime.start_download("1.2.3");
        assert!(!runtime.ack());
        assert_eq!(runtime.snapshot().phase, ManagerUpdatePhase::Downloading);

        runtime.mark_installing();
        assert!(!runtime.ack());
        assert_eq!(runtime.snapshot().phase, ManagerUpdatePhase::Installing);
    }

    #[test]
    fn ack_on_an_idle_runtime_reports_no_change() {
        let runtime = ManagerUpdateRuntime::default();
        assert!(!runtime.ack());
    }

    #[test]
    fn relaunch_reservation_is_single_claim() {
        let runtime = ManagerUpdateRuntime::default();
        assert!(runtime.reserve_relaunch());
        assert!(!runtime.reserve_relaunch());
        runtime.release_relaunch_reservation();
        assert!(runtime.reserve_relaunch());
    }

    #[test]
    fn a_released_reservation_can_be_reclaimed_after_a_blocked_restart() {
        // Mirrors `manager_relaunch` finding out the process cannot actually
        // exit (an uninterruptible operation is active elsewhere) and giving
        // the reservation back so a later retry is not silenced forever.
        let runtime = ManagerUpdateRuntime::default();
        assert!(runtime.reserve_relaunch());
        runtime.release_relaunch_reservation();
        assert!(runtime.reserve_relaunch());
        assert!(!runtime.reserve_relaunch());
    }

    #[test]
    fn a_second_download_run_overwrites_a_previous_terminal_snapshot() {
        let runtime = ManagerUpdateRuntime::default();
        runtime.start_download("1.2.3");
        runtime.mark_error("network");
        // A fresh check-and-install cycle must not require an explicit ack
        // first; starting a new download always wins.
        runtime.start_download("1.2.4");
        let snapshot = runtime.snapshot();
        assert_eq!(snapshot.phase, ManagerUpdatePhase::Downloading);
        assert_eq!(snapshot.version.as_deref(), Some("1.2.4"));
        assert_eq!(snapshot.downloaded, 0);
        assert!(snapshot.code.is_none());
    }

    #[test]
    fn without_installed_only_drops_the_installed_version() {
        let runtime = ManagerUpdateRuntime::default();
        let offer = |v: &'static str| Some((v, "notes"));
        // Nothing installed yet: every result passes through untouched.
        assert!(runtime.without_installed(offer("1.2.3"), |u| u.0).is_some());
        assert!(runtime.without_installed(None::<(&str, &str)>, |u| u.0).is_none());

        runtime.start_download("1.2.3");
        runtime.mark_installing();
        runtime.mark_installed();
        // Still suppressed after the reminder was dismissed (ack)...
        assert!(runtime.ack());
        assert!(runtime.without_installed(offer("1.2.3"), |u| u.0).is_none());
        // ...but a newer version is still offered.
        assert!(runtime.without_installed(offer("1.2.4"), |u| u.0).is_some());
    }

    #[test]
    fn stall_timeout_abandons_a_future_that_never_progresses() {
        let clock = ActivityClock::new();
        let started = Instant::now();
        let result = tauri::async_runtime::block_on(with_stall_timeout(
            std::future::pending::<()>(),
            &clock,
            Duration::from_millis(80),
        ));
        assert_eq!(result, Err(Stalled));
        assert!(started.elapsed() >= Duration::from_millis(80));
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn stall_timeout_lets_a_slow_but_progressing_future_finish() {
        let clock = ActivityClock::new();
        // Total run time (~240ms) exceeds the 100ms limit, but activity is
        // recorded every 40ms, so it must not be cut off.
        let work = async {
            for _ in 0..6 {
                tokio::time::sleep(Duration::from_millis(40)).await;
                clock.touch();
            }
            7
        };
        let result = tauri::async_runtime::block_on(with_stall_timeout(
            work,
            &clock,
            Duration::from_millis(100),
        ));
        assert_eq!(result, Ok(7));
    }

    #[test]
    fn stall_timeout_fires_when_progress_stops_midway() {
        let clock = ActivityClock::new();
        let work = async {
            tokio::time::sleep(Duration::from_millis(30)).await;
            clock.touch();
            std::future::pending::<()>().await;
        };
        let result = tauri::async_runtime::block_on(with_stall_timeout(
            work,
            &clock,
            Duration::from_millis(80),
        ));
        assert_eq!(result, Err(Stalled));
    }

    #[test]
    fn emit_throttle_limits_progress_events_but_lets_the_first_through() {
        let start = Instant::now();
        let mut throttle = EmitThrottle::new(Duration::from_millis(100));
        assert!(throttle.should_emit(start));
        // A burst of chunks inside the interval is coalesced.
        for ms in [1, 10, 50, 99] {
            assert!(!throttle.should_emit(start + Duration::from_millis(ms)));
        }
        assert!(throttle.should_emit(start + Duration::from_millis(100)));
        // The window restarts from the last emission, not from the first.
        assert!(!throttle.should_emit(start + Duration::from_millis(150)));
        assert!(throttle.should_emit(start + Duration::from_millis(200)));
    }

    #[test]
    fn updater_errors_map_to_stable_localizable_codes() {
        use tauri_plugin_updater::Error as E;
        let code = |stage, error: E| classify_updater_error(stage, &error).as_code();
        let d = UpdateStage::Download;
        let i = UpdateStage::Install;

        assert_eq!(code(d, E::Network("status 404".into())), "artifact");
        assert_eq!(code(d, E::ReleaseNotFound), "artifact");
        assert_eq!(code(i, E::MissingSignedVersion), "signature");
        assert_eq!(
            code(
                i,
                E::SignedVersionMismatch {
                    signed: "1.0.0".into(),
                    announced: "1.0.1".into()
                }
            ),
            "signature"
        );
        assert_eq!(code(i, E::SignatureUtf8("x".into())), "signature");
        assert_eq!(code(i, E::AuthenticationFailed), "permission");
        let io = |kind| E::Io(std::io::Error::new(kind, "x"));
        assert_eq!(code(i, io(std::io::ErrorKind::PermissionDenied)), "permission");
        assert_eq!(code(i, io(std::io::ErrorKind::StorageFull)), "disk_space");
        // No specific meaning: the fallback depends on which half failed.
        assert_eq!(code(i, io(std::io::ErrorKind::Other)), "install");
        assert_eq!(code(i, E::InvalidUpdaterFormat), "install");
        assert_eq!(code(d, io(std::io::ErrorKind::Other)), "engine_error");
        assert_eq!(code(d, E::EmptyEndpoints), "engine_error");
        // The manifest check has no "install" fallback: an unclassifiable
        // failure there stays generic.
        assert_eq!(code(UpdateStage::Check, E::EmptyEndpoints), "engine_error");
        assert_eq!(code(UpdateStage::Check, io(std::io::ErrorKind::Other)), "engine_error");
    }

    /// `reqwest::Error` has no public constructor, so the transport-level
    /// branches are exercised with real requests against a loopback listener.
    fn reqwest_error_from(
        respond: impl FnOnce(std::net::TcpStream) + Send + 'static,
        timeout: Duration,
        error_for_status: bool,
    ) -> reqwest::Error {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                respond(stream);
            }
        });
        // Same provider the updater plugin installs before building a client.
        let _ = rustls::crypto::ring::default_provider().install_default();
        let error = tauri::async_runtime::block_on(async move {
            let client = reqwest::Client::builder()
                .no_proxy()
                .timeout(timeout)
                .build()
                .unwrap();
            let response = client.get(format!("http://{addr}/")).send().await;
            if error_for_status {
                response.unwrap().error_for_status().unwrap_err()
            } else {
                response.unwrap_err()
            }
        });
        let _ = server.join();
        error
    }

    #[test]
    fn reqwest_timeout_status_and_transport_errors_are_classified() {
        use std::io::{Read, Write};
        use tauri_plugin_updater::Error as E;

        // Accepts the connection but never answers (captive portal / dead
        // proxy): the request timeout fires.
        let stalled = reqwest_error_from(
            |mut stream| {
                let mut buf = [0u8; 256];
                let _ = stream.read(&mut buf);
                std::thread::sleep(Duration::from_millis(400));
            },
            Duration::from_millis(100),
            false,
        );
        assert!(stalled.is_timeout());
        assert_eq!(
            classify_updater_error(UpdateStage::Check, &E::Reqwest(stalled)),
            ErrorKind::Timeout
        );

        let status = reqwest_error_from(
            |mut stream| {
                let mut buf = [0u8; 256];
                let _ = stream.read(&mut buf);
                let _ = stream.write_all(
                    b"HTTP/1.1 500 Internal Server Error\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                );
            },
            Duration::from_secs(5),
            true,
        );
        assert!(status.is_status());
        assert_eq!(
            classify_updater_error(UpdateStage::Download, &E::Reqwest(status)),
            ErrorKind::Artifact
        );

        // Connection dropped before any response: a plain transport failure.
        let dropped = reqwest_error_from(drop, Duration::from_secs(5), false);
        assert!(!dropped.is_timeout() && !dropped.is_status());
        assert_eq!(
            classify_updater_error(UpdateStage::Check, &E::Reqwest(dropped)),
            ErrorKind::Network
        );
    }

    fn checkpoint_lock_path(name: &str) -> std::path::PathBuf {
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-data")
            .join(format!("manager-update-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("operation.lock")
    }

    #[test]
    fn commit_checkpoint_proceeds_and_blocks_a_later_quit_when_it_wins() {
        use crate::app::op_phase::QuitPolicy;
        use crate::app::oplock::OperationKind;

        let path = checkpoint_lock_path("wins");
        let operations = OperationManager::new(path.clone());
        let guard = operations.begin(OperationKind::ManagerUpdate).unwrap();
        let force_quit = AtomicBool::new(false);

        assert!(enter_commit_checkpoint(&operations, guard.token(), &force_quit));
        assert_eq!(operations.phase(), OperationPhase::Committing);
        // A quit request arriving after the checkpoint is refused, even when
        // confirmed, so it cannot exit mid-install.
        let prepared = AtomicBool::new(false);
        let policy = operations.prepare_quit(true, true, || {
            prepared.store(true, Ordering::SeqCst);
        });
        assert!(matches!(policy, QuitPolicy::Block { .. }));
        assert!(!prepared.load(Ordering::SeqCst));

        drop(guard);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }

    #[test]
    fn commit_checkpoint_bails_out_when_a_confirmed_quit_won_first() {
        use crate::app::oplock::OperationKind;

        let path = checkpoint_lock_path("loses");
        let operations = OperationManager::new(path.clone());
        let guard = operations.begin(OperationKind::ManagerUpdate).unwrap();
        operations
            .set_phase(guard.token(), OperationPhase::Downloading)
            .unwrap();
        let force_quit = AtomicBool::new(false);

        // Confirmed quit during the (interruptible) download arms force_quit.
        operations.prepare_quit(true, true, || force_quit.store(true, Ordering::SeqCst));
        assert!(force_quit.load(Ordering::SeqCst));
        assert!(!enter_commit_checkpoint(&operations, guard.token(), &force_quit));

        drop(guard);
        let _ = std::fs::remove_dir_all(path.parent().unwrap());
    }
}
