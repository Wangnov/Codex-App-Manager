//! Child-process helpers for the Windows engine.
//!
//! Every external probe (PowerShell, curl, portable launch checks) goes through
//! a shared deadline + optional stall timeout + cleanup path so hung AppX /
//! enterprise-policy machines cannot freeze the manager indefinitely.

use std::ffi::OsStr;
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Default wall-clock budget for a short PowerShell / curl-text probe.
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(45);
/// Longer budget for Add-AppxPackage / Remove-AppxPackage.
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(180);
/// Default no-progress budget for streaming package downloads.
pub const DEFAULT_STALL_TIMEOUT: Duration = Duration::from_secs(120);
/// Absolute upper bound for a package download (2 hours). Stall timeout is the
/// primary hang defense; this only stops an endlessly crawling transfer.
pub const DEFAULT_DOWNLOAD_TOTAL_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
/// Minimum survival window after spawning a portable binary for "launchable".
pub const PORTABLE_LIVENESS_WINDOW: Duration = Duration::from_secs(3);
/// Continuous survival required after MSIX shell activation — aligned with the
/// portable liveness window so both routes reject the same class of crash-loops.
pub const MSIX_LIVENESS_WINDOW_SECS: u64 = PORTABLE_LIVENESS_WINDOW.as_secs();
/// Outer budget to wait for a cold-started MSIX process to *appear* after
/// `Start-Process shell:AppsFolder\…`. Cold machines / AppX service warm-up can
/// take well over 10s; too short a window causes false portable fallbacks.
pub const MSIX_ACTIVATION_WINDOW_SECS: u64 = 30;

/// Poll interval while waiting on a child.
const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy)]
pub struct RunLimits {
    /// Hard wall-clock deadline from spawn.
    pub total: Duration,
    /// Optional: kill when a progress signal has not advanced for this long.
    pub stall: Option<Duration>,
}

impl RunLimits {
    pub fn total(total: Duration) -> Self {
        Self { total, stall: None }
    }

    pub fn with_stall(total: Duration, stall: Duration) -> Self {
        Self {
            total,
            stall: Some(stall),
        }
    }

    pub fn probe() -> Self {
        Self::total(DEFAULT_PROBE_TIMEOUT)
    }

    pub fn install() -> Self {
        Self::total(INSTALL_TIMEOUT)
    }

    pub fn download() -> Self {
        Self::with_stall(DEFAULT_DOWNLOAD_TOTAL_TIMEOUT, DEFAULT_STALL_TIMEOUT)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutKind {
    Total,
    Stall,
}

#[derive(Debug)]
pub enum RunError {
    Spawn(String),
    Timeout {
        kind: TimeoutKind,
        /// Best-effort stderr collected before kill (often empty).
        #[allow(dead_code)]
        partial_stderr: String,
    },
    Cancelled,
    Wait(String),
}

impl RunError {
    #[allow(dead_code)] // used by unit tests and available to callers
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout { .. })
    }

    #[allow(dead_code)] // used by unit tests and available to callers
    pub fn is_cancelled(&self) -> bool {
        matches!(self, Self::Cancelled)
    }

    pub fn message(&self) -> String {
        match self {
            Self::Spawn(msg) => format!("spawn failed: {msg}"),
            Self::Timeout { kind, .. } => match kind {
                TimeoutKind::Total => "process exceeded total deadline".to_string(),
                TimeoutKind::Stall => "process made no progress within stall timeout".to_string(),
            },
            Self::Cancelled => "process cancelled".to_string(),
            Self::Wait(msg) => format!("wait failed: {msg}"),
        }
    }
}

pub(crate) fn hidden_command(program: impl AsRef<OsStr>) -> Command {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;

        let mut command = Command::new(program);
        command.creation_flags(CREATE_NO_WINDOW);
        command
    }
    #[cfg(not(windows))]
    {
        Command::new(program)
    }
}

pub(crate) fn curl_exe() -> PathBuf {
    std::env::var_os("SystemRoot")
        .or_else(|| std::env::var_os("WINDIR"))
        .map(PathBuf::from)
        .map(|root| root.join("System32").join("curl.exe"))
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from("curl"))
}

/// Terminate a child and, on Windows, its process tree (PowerShell nests work).
fn terminate_tree(child: &mut Child) {
    let pid = child.id();
    let _ = child.kill();
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // Best-effort: kill may race with natural exit. `/T` covers grandchildren
        // that PowerShell or curl may have spawned under the same tree.
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    let _ = child.wait();
}

fn cancelled(flag: Option<&AtomicBool>) -> bool {
    flag.map(|f| f.load(Ordering::SeqCst)).unwrap_or(false)
}

/// Run `command` to completion with a total deadline (and optional cancel flag).
/// Captures stdout/stderr. Does not interpret exit codes — callers do.
pub fn run_capturing(
    mut command: Command,
    limits: RunLimits,
    cancel: Option<&AtomicBool>,
) -> Result<Output, RunError> {
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| RunError::Spawn(e.to_string()))?;
    wait_child(
        &mut child,
        limits,
        cancel,
        /*progress*/ None,
        /*on_progress*/ None,
    )?;
    child
        .wait_with_output()
        .map_err(|e| RunError::Wait(e.to_string()))
}

/// Like [`run_capturing`], but tracks a progress signal for stall detection.
/// `progress` is polled each loop; `on_progress` is notified when the value grows.
pub fn run_with_progress(
    mut command: Command,
    limits: RunLimits,
    cancel: Option<&AtomicBool>,
    progress: &dyn Fn() -> u64,
    on_progress: &dyn Fn(u64),
) -> Result<Output, RunError> {
    command.stdout(Stdio::piped());
    command.stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| RunError::Spawn(e.to_string()))?;
    wait_child(
        &mut child,
        limits,
        cancel,
        Some(progress),
        Some(on_progress),
    )?;
    child
        .wait_with_output()
        .map_err(|e| RunError::Wait(e.to_string()))
}

fn wait_child(
    child: &mut Child,
    limits: RunLimits,
    cancel: Option<&AtomicBool>,
    progress: Option<&dyn Fn() -> u64>,
    on_progress: Option<&dyn Fn(u64)>,
) -> Result<(), RunError> {
    let started = Instant::now();
    let mut last_progress = progress.map(|p| p()).unwrap_or(0);
    let mut last_progress_at = Instant::now();
    if let (Some(p), Some(cb)) = (progress, on_progress) {
        cb(p());
    }

    loop {
        if cancelled(cancel) {
            terminate_tree(child);
            return Err(RunError::Cancelled);
        }

        match child.try_wait() {
            Ok(Some(_)) => return Ok(()),
            Ok(None) => {
                if started.elapsed() >= limits.total {
                    terminate_tree(child);
                    return Err(RunError::Timeout {
                        kind: TimeoutKind::Total,
                        partial_stderr: String::new(),
                    });
                }
                if let Some(p) = progress {
                    let current = p();
                    if current > last_progress {
                        last_progress = current;
                        last_progress_at = Instant::now();
                        if let Some(cb) = on_progress {
                            cb(current);
                        }
                    } else if let Some(stall) = limits.stall {
                        if last_progress_at.elapsed() >= stall {
                            terminate_tree(child);
                            return Err(RunError::Timeout {
                                kind: TimeoutKind::Stall,
                                partial_stderr: String::new(),
                            });
                        }
                    }
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(err) => {
                terminate_tree(child);
                return Err(RunError::Wait(err.to_string()));
            }
        }
    }
}

/// Outcome of a liveness probe on a freshly spawned process.
#[derive(Debug)]
pub enum LivenessResult {
    /// Still running after the survival window. Caller owns the child handle
    /// (keep it for relaunch, or drop/kill when only verifying).
    Survived { child: Child },
    /// Exited before the window elapsed.
    ExitedEarly { code: Option<i32> },
}

/// Spawn `command` and require it to stay alive for `window`.
///
/// Used by portable post-install health checks: spawn success alone does not
/// mean the binary is launchable — an immediate crash must fail the install.
pub fn spawn_and_require_liveness(
    mut command: Command,
    window: Duration,
) -> Result<LivenessResult, RunError> {
    // Detach stdio so a chatty broken payload cannot fill pipes and block, and
    // so unit tests that use console tools (e.g. whoami) do not pollute output.
    command.stdout(Stdio::null());
    command.stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|e| RunError::Spawn(e.to_string()))?;
    let deadline = Instant::now() + window;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ok(LivenessResult::ExitedEarly {
                    code: status.code(),
                });
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    return Ok(LivenessResult::Survived { child });
                }
                thread::sleep(POLL_INTERVAL);
            }
            Err(err) => {
                terminate_tree(&mut child);
                return Err(RunError::Wait(err.to_string()));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn sleep_command(secs: u64) -> Command {
        #[cfg(windows)]
        {
            let mut cmd = hidden_command("powershell.exe");
            cmd.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!("Start-Sleep -Seconds {secs}"),
            ]);
            cmd
        }
        #[cfg(not(windows))]
        {
            let mut cmd = Command::new("sleep");
            cmd.arg(secs.to_string());
            cmd
        }
    }

    fn immediate_exit_command(code: i32) -> Command {
        #[cfg(windows)]
        {
            let mut cmd = hidden_command("cmd.exe");
            cmd.args(["/C", &format!("exit {code}")]);
            cmd
        }
        #[cfg(not(windows))]
        {
            let mut cmd = Command::new("sh");
            cmd.args(["-c", &format!("exit {code}")]);
            cmd
        }
    }

    fn slow_echo_command(delay_secs: u64, message: &str) -> Command {
        #[cfg(windows)]
        {
            let mut cmd = hidden_command("powershell.exe");
            cmd.args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &format!("Start-Sleep -Seconds {delay_secs}; Write-Output '{message}'"),
            ]);
            cmd
        }
        #[cfg(not(windows))]
        {
            let mut cmd = Command::new("sh");
            cmd.args([
                "-c",
                &format!("sleep {delay_secs}; printf '%s' '{message}'"),
            ]);
            cmd
        }
    }

    #[test]
    fn total_timeout_kills_hung_child() {
        let err = run_capturing(
            sleep_command(60),
            RunLimits::total(Duration::from_millis(400)),
            None,
        )
        .expect_err("hung child must time out");
        match err {
            RunError::Timeout {
                kind: TimeoutKind::Total,
                ..
            } => {}
            other => panic!("expected total timeout, got {other:?}"),
        }
    }

    #[test]
    fn slow_child_within_deadline_succeeds() {
        let output = run_capturing(
            slow_echo_command(1, "alive"),
            RunLimits::total(Duration::from_secs(15)),
            None,
        )
        .expect("slow child within deadline");
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("alive"), "stdout={stdout}");
    }

    #[test]
    fn stall_timeout_when_progress_frozen() {
        let err = run_with_progress(
            sleep_command(60),
            RunLimits::with_stall(Duration::from_secs(30), Duration::from_millis(300)),
            None,
            &|| 0u64,
            &|_| {},
        )
        .expect_err("frozen progress must stall-timeout");
        match err {
            RunError::Timeout {
                kind: TimeoutKind::Stall,
                ..
            } => {}
            other => panic!("expected stall timeout, got {other:?}"),
        }
    }

    #[test]
    fn progress_growth_resets_stall_clock() {
        // A short sleep finishes well under total; monotonically growing progress
        // keeps the stall clock from firing.
        let counter = std::sync::atomic::AtomicU64::new(0);
        let output = run_with_progress(
            sleep_command(1),
            RunLimits::with_stall(Duration::from_secs(15), Duration::from_millis(400)),
            None,
            &|| counter.fetch_add(1, Ordering::SeqCst),
            &|_| {},
        )
        .expect("progressing child should finish");
        assert!(output.status.success());
    }

    #[test]
    fn cancellation_kills_child() {
        let flag = Arc::new(AtomicBool::new(false));
        let cancel = Arc::clone(&flag);
        let handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(200));
            cancel.store(true, Ordering::SeqCst);
        });
        let err = run_capturing(
            sleep_command(60),
            RunLimits::total(Duration::from_secs(30)),
            Some(&flag),
        )
        .expect_err("cancel must abort child");
        assert!(err.is_cancelled(), "got {err:?}");
        assert!(!err.is_timeout());
        handle.join().unwrap();
    }

    #[test]
    fn timeout_error_reports_kind_helpers() {
        let err = run_capturing(
            sleep_command(60),
            RunLimits::total(Duration::from_millis(200)),
            None,
        )
        .expect_err("must timeout");
        assert!(err.is_timeout());
        assert!(!err.is_cancelled());
        assert!(err.message().contains("deadline"));
    }

    #[test]
    fn immediate_exit_liveness_detected() {
        let result = spawn_and_require_liveness(
            immediate_exit_command(7),
            Duration::from_secs(2),
        )
        .expect("spawn");
        match result {
            LivenessResult::ExitedEarly { code } => {
                assert_eq!(code, Some(7));
            }
            LivenessResult::Survived { mut child } => {
                let _ = child.kill();
                panic!("immediate-exit binary must not be reported as survived");
            }
        }
    }

    #[test]
    fn surviving_child_reported_alive() {
        let result =
            spawn_and_require_liveness(sleep_command(30), Duration::from_millis(400)).expect("spawn");
        match result {
            LivenessResult::Survived { mut child } => {
                terminate_tree(&mut child);
            }
            LivenessResult::ExitedEarly { code } => {
                panic!("sleep should still be running, exit={code:?}");
            }
        }
    }
}
