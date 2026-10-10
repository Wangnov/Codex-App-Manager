//! Child-process helpers for the Windows engine.
//!
//! Every external probe (PowerShell, curl, portable launch checks) goes through
//! a shared deadline + optional stall timeout + cleanup path so hung AppX /
//! enterprise-policy machines cannot freeze the manager indefinitely.

use std::ffi::OsStr;
use std::io::{self, Read};
use std::path::PathBuf;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Default wall-clock budget for a short PowerShell / curl-text probe.
///
/// Only real production callers are Windows-only PowerShell/AppX probes, but
/// this crate's own cross-platform test suite (`RunLimits::probe`) also
/// exercises the shared `run_capturing` runner with it, so it must stay
/// available under `cfg(test)` on every platform, not just `cfg(windows)`.
#[cfg(any(windows, test))]
pub const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(45);
/// Longer budget for Add-AppxPackage / Remove-AppxPackage. Windows-only: every
/// caller lives behind `cfg(windows)` PowerShell/AppX code paths.
#[cfg(windows)]
pub const INSTALL_TIMEOUT: Duration = Duration::from_secs(180);
/// Upper bound for the one-shot UAC recovery used only when Windows Update owns
/// an active deployment of the exact package we already downloaded locally.
/// Windows-only: every caller lives behind `cfg(windows)` AppX recovery code.
#[cfg(windows)]
pub const APPX_RECOVERY_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Default no-progress budget for streaming package downloads.
pub const DEFAULT_STALL_TIMEOUT: Duration = Duration::from_secs(120);
/// Absolute upper bound for a package download (2 hours). Stall timeout is the
/// primary hang defense; this only stops an endlessly crawling transfer.
pub const DEFAULT_DOWNLOAD_TOTAL_TIMEOUT: Duration = Duration::from_secs(2 * 60 * 60);
/// Minimum survival window after spawning a portable binary for "launchable".
pub const PORTABLE_LIVENESS_WINDOW: Duration = Duration::from_secs(3);
/// Continuous survival required after MSIX shell activation — aligned with the
/// portable liveness window so both routes reject the same class of crash-loops.
/// MSIX shell activation only runs on Windows, but a cross-platform test
/// (`msix_liveness_window_matches_portable`) pins this invariant on every
/// platform, so it must also stay available under `cfg(test)`.
#[cfg(any(windows, test))]
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

    #[cfg(any(windows, test))]
    pub fn probe() -> Self {
        Self::total(DEFAULT_PROBE_TIMEOUT)
    }

    #[cfg(windows)]
    pub fn install() -> Self {
        Self::total(INSTALL_TIMEOUT)
    }

    #[cfg(windows)]
    pub fn appx_recovery() -> Self {
        Self::total(APPX_RECOVERY_TIMEOUT)
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
    OutputTooLarge {
        stream: &'static str,
        limit: usize,
    },
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
            Self::OutputTooLarge { stream, limit } => {
                format!("process {stream} exceeded capture limit of {limit} bytes")
            }
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
    #[cfg(windows)]
    {
        let pid = child.id();
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
    let _ = child.kill();
    let _ = child.wait();
}

fn cancelled(flag: Option<&AtomicBool>) -> bool {
    flag.map(|f| f.load(Ordering::SeqCst)).unwrap_or(false)
}

/// Run `command` to completion with a total deadline (and optional cancel flag).
/// Captures stdout/stderr. Does not interpret exit codes — callers do.
pub fn run_capturing(
    command: Command,
    limits: RunLimits,
    cancel: Option<&AtomicBool>,
) -> Result<Output, RunError> {
    capture(command, limits, cancel, None, None)
}

/// Like [`run_capturing`], but tracks a progress signal for stall detection.
/// `progress` is polled each loop; `on_progress` is notified when the value grows.
pub fn run_with_progress(
    command: Command,
    limits: RunLimits,
    cancel: Option<&AtomicBool>,
    progress: &dyn Fn() -> u64,
    on_progress: &dyn Fn(u64),
) -> Result<Output, RunError> {
    capture(command, limits, cancel, Some(progress), Some(on_progress))
}

// Poll both pipes without blocking, including after the direct child exits:
// descendants may still hold a write handle. Do not spawn blocking reader
// threads that outlive a timeout/cancellation or join them beyond the deadline.
#[cfg(windows)]
trait CapturePipe: Read + std::os::windows::io::AsRawHandle {}
#[cfg(windows)]
impl<T: Read + std::os::windows::io::AsRawHandle> CapturePipe for T {}
#[cfg(unix)]
trait CapturePipe: Read + std::os::fd::AsRawFd {}
#[cfg(unix)]
impl<T: Read + std::os::fd::AsRawFd> CapturePipe for T {}

fn prepare_pipe(pipe: &impl CapturePipe) -> io::Result<()> {
    #[cfg(unix)]
    {
        let fd = pipe.as_raw_fd();
        // The read descriptor belongs exclusively to this capture operation.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
        {
            return Err(io::Error::last_os_error());
        }
    }
    #[cfg(windows)]
    let _ = pipe; // PeekNamedPipe makes each subsequent read nonblocking.
    Ok(())
}

fn read_available(pipe: &mut impl CapturePipe, buffer: &mut [u8]) -> io::Result<usize> {
    #[cfg(windows)]
    {
        use windows_sys::Win32::{Foundation::ERROR_BROKEN_PIPE, System::Pipes::PeekNamedPipe};
        let mut available = 0;
        // The pipe handle remains owned by the caller; this does not consume bytes.
        let ok = unsafe {
            PeekNamedPipe(
                pipe.as_raw_handle(),
                std::ptr::null_mut(),
                0,
                std::ptr::null_mut(),
                &mut available,
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            let error = io::Error::last_os_error();
            return if error.raw_os_error() == Some(ERROR_BROKEN_PIPE as i32) {
                Ok(0)
            } else {
                Err(error)
            };
        }
        if available == 0 {
            return Err(io::ErrorKind::WouldBlock.into());
        }
        let count = buffer.len().min(available as usize);
        pipe.read(&mut buffer[..count])
    }
    #[cfg(unix)]
    pipe.read(buffer)
}

fn drain_pipe(
    pipe: &mut impl CapturePipe,
    bytes: &mut Vec<u8>,
    closed: &mut bool,
    stream: &'static str,
    limit: usize,
) -> Result<(), RunError> {
    if *closed {
        return Ok(());
    }
    let mut buffer = [0; 16 * 1024];
    // Fairness: even an endless stdout writer cannot starve stderr or cancel.
    for _ in 0..16 {
        match read_available(pipe, &mut buffer) {
            Ok(0) => {
                *closed = true;
                break;
            }
            Ok(count) => {
                if bytes.len().saturating_add(count) > limit {
                    return Err(RunError::OutputTooLarge { stream, limit });
                }
                bytes.extend_from_slice(&buffer[..count]);
            }
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(RunError::Wait(format!("read {stream}: {error}"))),
        }
    }
    Ok(())
}

fn capture(
    mut command: Command,
    limits: RunLimits,
    cancel: Option<&AtomicBool>,
    progress: Option<&dyn Fn() -> u64>,
    on_progress: Option<&dyn Fn(u64)>,
) -> Result<Output, RunError> {
    let started = Instant::now();
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = command
        .spawn()
        .map_err(|e| RunError::Spawn(e.to_string()))?;
    let mut stdout = child.stdout.take().expect("piped stdout");
    let mut stderr = child.stderr.take().expect("piped stderr");
    let mut out = Vec::new();
    let mut err = Vec::new();
    let (mut out_closed, mut err_closed) = (false, false);
    let result = (|| {
        prepare_pipe(&stdout)
            .and_then(|()| prepare_pipe(&stderr))
            .map_err(|e| RunError::Wait(e.to_string()))?;
        let mut last_progress = progress.map(|p| p()).unwrap_or(0);
        let mut last_progress_at = Instant::now();
        if let (Some(p), Some(cb)) = (progress, on_progress) {
            cb(p());
        }

        loop {
            if cancelled(cancel) {
                return Err(RunError::Cancelled);
            }

            let previous_bytes = out.len() + err.len();
            drain_pipe(
                &mut stdout,
                &mut out,
                &mut out_closed,
                "stdout",
                crate::limits::MAX_TEXT_BYTES as usize,
            )?;
            drain_pipe(
                &mut stderr,
                &mut err,
                &mut err_closed,
                "stderr",
                1024 * 1024,
            )?;

            match child.try_wait() {
                Ok(Some(status)) if out_closed && err_closed => return Ok(status),
                Ok(_) => {
                    if started.elapsed() >= limits.total {
                        return Err(RunError::Timeout {
                            kind: TimeoutKind::Total,
                            partial_stderr: String::from_utf8_lossy(&err).into_owned(),
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
                                return Err(RunError::Timeout {
                                    kind: TimeoutKind::Stall,
                                    partial_stderr: String::from_utf8_lossy(&err).into_owned(),
                                });
                            }
                        }
                    }
                    if out.len() + err.len() == previous_bytes {
                        thread::sleep(POLL_INTERVAL);
                    }
                }
                Err(err) => {
                    return Err(RunError::Wait(err.to_string()));
                }
            }
        }
    })();
    match result {
        Ok(status) => Ok(Output {
            status,
            stdout: out,
            stderr: err,
        }),
        Err(error) => {
            // Avoid acting on a recycled PID after a child has already exited.
            if !matches!(child.try_wait(), Ok(Some(_))) {
                terminate_tree(&mut child);
            }
            Err(error)
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
#[cfg(test)]
pub fn spawn_and_require_liveness(
    command: Command,
    window: Duration,
) -> Result<LivenessResult, RunError> {
    spawn_and_check_startup(command, window, false)
}

pub(crate) fn spawn_and_check_startup(
    mut command: Command,
    window: Duration,
    require_window: bool,
) -> Result<LivenessResult, RunError> {
    // Detach stdio so a chatty broken payload cannot fill pipes and block, and
    // so unit tests that use console tools (e.g. whoami) do not pollute output.
    command.stdout(Stdio::null());
    command.stderr(Stdio::null());
    let mut child = command
        .spawn()
        .map_err(|e| RunError::Spawn(e.to_string()))?;
    let started = Instant::now();
    let deadline = started + window;
    let startup_deadline = started + Duration::from_secs(MSIX_ACTIVATION_WINDOW_SECS);
    let mut progress = crate::startup_window::StartupProgress::default();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Ok(LivenessResult::ExitedEarly {
                    code: status.code(),
                });
            }
            Ok(None) => {
                let state = crate::startup_window::inspect(child.id());
                let ready = match progress.observe(started.elapsed(), &state, window) {
                    Ok(ready) => ready,
                    Err(failure) => {
                        terminate_tree(&mut child);
                        return Err(RunError::Wait(failure));
                    }
                };
                if (require_window && ready) || (!require_window && Instant::now() >= deadline) {
                    return Ok(LivenessResult::Survived { child });
                }
                if require_window && Instant::now() >= startup_deadline {
                    terminate_tree(&mut child);
                    return Err(RunError::Wait(
                        "Codex did not open its main window within 30 seconds".into(),
                    ));
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

    fn output_fixture(mode: &str) -> Command {
        let mut command = hidden_command(std::env::current_exe().unwrap());
        command.args([
            "--exact",
            "process::tests::capture_output_fixture",
            "--ignored",
            "--nocapture",
        ]);
        command.env("CODEX_CAPTURE_FIXTURE", mode);
        command
    }

    #[test]
    #[ignore = "subprocess fixture, invoked by capture tests"]
    #[allow(clippy::zombie_processes)] // Fixture deliberately exits before its short-lived child.
    fn capture_output_fixture() {
        use std::io::Write;
        match std::env::var("CODEX_CAPTURE_FIXTURE").unwrap().as_str() {
            "large" => {
                std::io::stdout()
                    .write_all(&vec![b'x'; 5 * 1024 * 1024])
                    .unwrap();
                std::io::stderr()
                    .write_all(&vec![b'y'; 512 * 1024])
                    .unwrap();
                std::process::exit(7);
            }
            "overflow" => std::io::stdout()
                .write_all(&vec![b'x'; 9 * 1024 * 1024])
                .unwrap(),
            "hang" => {
                std::io::stderr()
                    .write_all(b"diagnostic before timeout\n")
                    .unwrap();
                std::thread::sleep(Duration::from_secs(60));
            }
            "inherit" => {
                output_fixture("hold-pipes").spawn().unwrap();
            }
            "hold-pipes" => std::thread::sleep(Duration::from_secs(2)),
            _ => panic!("unknown fixture"),
        }
        std::process::exit(0);
    }

    #[test]
    fn captures_large_stdout_and_stderr_without_blocking_for_both_runners() {
        for progress in [false, true] {
            let command = output_fixture("large");
            let limits = RunLimits::total(Duration::from_secs(15));
            let output = if progress {
                run_with_progress(command, limits, None, &|| 0, &|_| {})
            } else {
                run_capturing(command, limits, None)
            }
            .unwrap();
            assert_eq!(output.status.code(), Some(7));
            assert_eq!(
                output.stdout.iter().filter(|&&b| b == b'x').count(),
                5 * 1024 * 1024
            );
            assert_eq!(output.stderr, vec![b'y'; 512 * 1024]);
        }
    }

    #[test]
    fn rejects_output_over_limit_instead_of_returning_truncated_data() {
        let error =
            run_capturing(output_fixture("overflow"), RunLimits::probe(), None).unwrap_err();
        assert!(matches!(
            error,
            RunError::OutputTooLarge {
                stream: "stdout",
                ..
            }
        ));
    }

    #[test]
    fn timeout_keeps_stderr_and_does_not_wait_for_inherited_pipes() {
        let error = run_capturing(
            output_fixture("hang"),
            RunLimits::total(Duration::from_secs(1)),
            None,
        )
        .unwrap_err();
        assert!(
            matches!(error, RunError::Timeout { partial_stderr, .. } if partial_stderr.contains("diagnostic before timeout"))
        );
        let started = Instant::now();
        let error = run_capturing(
            output_fixture("inherit"),
            RunLimits::total(Duration::from_millis(600)),
            None,
        )
        .unwrap_err();
        assert!(error.is_timeout());
        assert!(started.elapsed() < Duration::from_millis(1800));
    }
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
        let result = spawn_and_require_liveness(immediate_exit_command(7), Duration::from_secs(2))
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
        let result = spawn_and_require_liveness(sleep_command(30), Duration::from_millis(400))
            .expect("spawn");
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
