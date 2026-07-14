//! Native Windows process discovery and shutdown for Codex installs.
//!
//! The pre-replacement close gate must not add a PowerShell dependency: policy
//! can block a later PowerShell launch even after an artifact was staged and
//! verified. Keep the close path in process and pin every target to its
//! executable path so the post-rebrand `ChatGPT.exe` never causes us to close
//! the separate ChatGPT product.

use std::path::Path;

use crate::EngineError;

#[cfg(windows)]
const TARGET_EXE_NAMES: [&str; 2] = ["Codex.exe", "ChatGPT.exe"];

#[cfg(any(windows, test))]
fn normalize_windows_path_text(value: &str) -> String {
    let mut normalized = value.replace('/', "\\").to_lowercase();
    if let Some(rest) = normalized.strip_prefix(r"\\?\unc\") {
        normalized = format!(r"\\{rest}");
    } else if let Some(rest) = normalized.strip_prefix(r"\\?\") {
        normalized = rest.to_string();
    }
    normalized.trim_end_matches('\\').to_string()
}

#[cfg(any(windows, test))]
fn normalized_windows_path(path: &Path) -> String {
    let resolved = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    normalize_windows_path_text(&resolved.to_string_lossy())
}

#[cfg(any(windows, test))]
fn path_is_within_root(candidate: &Path, root: &Path) -> bool {
    let candidate = normalized_windows_path(candidate);
    let root = normalized_windows_path(root);
    if candidate.is_empty() || root.is_empty() {
        return false;
    }
    candidate == root
        || candidate
            .strip_prefix(&root)
            .is_some_and(|suffix| suffix.starts_with('\\'))
}

#[cfg(windows)]
mod imp {
    use std::ffi::OsString;
    use std::mem::size_of;
    use std::os::windows::ffi::OsStringExt;
    use std::path::{Path, PathBuf};
    use std::thread;
    use std::time::{Duration, Instant};

    use windows_sys::Win32::Foundation::{
        CloseHandle, ERROR_NO_MORE_FILES, HANDLE, HWND, INVALID_HANDLE_VALUE, LPARAM, WAIT_FAILED,
        WAIT_OBJECT_0, WAIT_TIMEOUT,
    };
    use windows_sys::Win32::System::Diagnostics::ToolHelp::{
        CreateToolhelp32Snapshot, Process32FirstW, Process32NextW, PROCESSENTRY32W,
        TH32CS_SNAPPROCESS,
    };
    use windows_sys::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
        PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumWindows, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
    };

    use super::{path_is_within_root, EngineError, TARGET_EXE_NAMES};

    const PROCESS_SYNCHRONIZE: u32 = 0x0010_0000;
    const FORCE_CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
    const POLL_INTERVAL: Duration = Duration::from_millis(250);
    const MAX_PROCESS_PATH_UTF16: usize = 32_768;

    struct OwnedHandle(HANDLE);

    impl OwnedHandle {
        fn new(handle: HANDLE) -> Option<Self> {
            (!handle.is_null() && handle != INVALID_HANDLE_VALUE).then_some(Self(handle))
        }

        fn raw(&self) -> HANDLE {
            self.0
        }
    }

    impl Drop for OwnedHandle {
        fn drop(&mut self) {
            // SAFETY: `OwnedHandle` is only constructed from a successful Win32
            // handle-returning call and owns that handle exactly once.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    struct TargetProcess {
        pid: u32,
        handle: OwnedHandle,
        can_terminate: bool,
    }

    impl TargetProcess {
        fn is_running(&self) -> bool {
            // SAFETY: the process handle remains owned for the lifetime of self.
            match unsafe { WaitForSingleObject(self.handle.raw(), 0) } {
                WAIT_OBJECT_0 => false,
                WAIT_TIMEOUT => true,
                WAIT_FAILED => {
                    log::warn!("wait for target Codex process failed pid={}", self.pid);
                    true
                }
                status => {
                    log::warn!(
                        "wait for target Codex process returned unexpected status pid={} status={status}",
                        self.pid
                    );
                    true
                }
            }
        }
    }

    fn last_error(context: &str) -> EngineError {
        EngineError::Install(format!("{context}: {}", std::io::Error::last_os_error()))
    }

    fn process_name(entry: &PROCESSENTRY32W) -> String {
        let end = entry
            .szExeFile
            .iter()
            .position(|unit| *unit == 0)
            .unwrap_or(entry.szExeFile.len());
        String::from_utf16_lossy(&entry.szExeFile[..end])
    }

    fn open_target_process(pid: u32, root: &Path) -> Option<TargetProcess> {
        let full_access =
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | PROCESS_SYNCHRONIZE;
        // SAFETY: access flags and PID come from the process snapshot.
        let mut handle = unsafe { OpenProcess(full_access, 0, pid) };
        let can_terminate = !handle.is_null();
        if handle.is_null() {
            // Query-only access still lets us identify and gracefully close a
            // process when policy denies PROCESS_TERMINATE.
            handle = unsafe {
                OpenProcess(
                    PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_SYNCHRONIZE,
                    0,
                    pid,
                )
            };
        }
        let handle = match OwnedHandle::new(handle) {
            Some(handle) => handle,
            None => {
                log::debug!(
                    "skip Codex-name process whose image path cannot be queried pid={pid} error={}",
                    std::io::Error::last_os_error()
                );
                return None;
            }
        };

        let mut buffer = vec![0u16; MAX_PROCESS_PATH_UTF16];
        let mut length = buffer.len() as u32;
        // SAFETY: buffer is writable for `length` UTF-16 units and the process
        // handle has PROCESS_QUERY_LIMITED_INFORMATION access.
        if unsafe { QueryFullProcessImageNameW(handle.raw(), 0, buffer.as_mut_ptr(), &mut length) }
            == 0
        {
            log::debug!(
                "skip Codex-name process whose image path query failed pid={pid} error={}",
                std::io::Error::last_os_error()
            );
            return None;
        }
        let image_path = PathBuf::from(OsString::from_wide(&buffer[..length as usize]));
        if !path_is_within_root(&image_path, root) {
            return None;
        }

        Some(TargetProcess {
            pid,
            handle,
            can_terminate,
        })
    }

    fn target_processes_under_root(root: &Path) -> Result<Vec<TargetProcess>, EngineError> {
        // SAFETY: TH32CS_SNAPPROCESS ignores the process-id argument.
        let snapshot = OwnedHandle::new(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) })
            .ok_or_else(|| last_error("create process snapshot"))?;
        let mut entry = PROCESSENTRY32W {
            dwSize: size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };

        // SAFETY: `entry` has the documented size and remains writable.
        if unsafe { Process32FirstW(snapshot.raw(), &mut entry) } == 0 {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                return Ok(Vec::new());
            }
            return Err(EngineError::Install(format!(
                "read first process snapshot entry: {err}"
            )));
        }

        let mut targets = Vec::new();
        loop {
            let name = process_name(&entry);
            if TARGET_EXE_NAMES
                .iter()
                .any(|candidate| name.eq_ignore_ascii_case(candidate))
            {
                if let Some(target) = open_target_process(entry.th32ProcessID, root) {
                    targets.push(target);
                }
            }

            // SAFETY: same valid snapshot and entry buffer as Process32FirstW.
            if unsafe { Process32NextW(snapshot.raw(), &mut entry) } == 0 {
                let err = std::io::Error::last_os_error();
                if err.raw_os_error() == Some(ERROR_NO_MORE_FILES as i32) {
                    break;
                }
                return Err(EngineError::Install(format!(
                    "read next process snapshot entry: {err}"
                )));
            }
        }
        Ok(targets)
    }

    unsafe extern "system" fn post_close_to_pid(hwnd: HWND, target_pid: LPARAM) -> i32 {
        let mut window_pid = 0u32;
        // SAFETY: EnumWindows supplied a live top-level HWND; `window_pid` is a
        // valid out pointer for the duration of the call.
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut window_pid);
        }
        if window_pid == target_pid as u32 {
            // Best-effort graceful close. A process without a responsive window
            // is handled by the bounded force-close phase below.
            unsafe {
                PostMessageW(hwnd, WM_CLOSE, 0, 0);
            }
        }
        1
    }

    fn request_graceful_close(targets: &[TargetProcess]) {
        for target in targets.iter().filter(|target| target.is_running()) {
            // SAFETY: callback is valid for the synchronous enumeration call and
            // the PID fits losslessly in LPARAM on supported Windows targets.
            unsafe {
                EnumWindows(Some(post_close_to_pid), target.pid as LPARAM);
            }
        }
    }

    fn wait_until_exited(targets: &[TargetProcess], timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if targets.iter().all(|target| !target.is_running()) {
                return true;
            }
            if Instant::now() >= deadline {
                return false;
            }
            thread::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
        }
    }

    pub(crate) fn close_codex_processes_for_root(
        timeout_secs: u64,
        root: &Path,
    ) -> Result<(), EngineError> {
        let targets = target_processes_under_root(root)?;
        if targets.is_empty() {
            return Ok(());
        }

        request_graceful_close(&targets);
        if wait_until_exited(&targets, Duration::from_secs(timeout_secs)) {
            return Ok(());
        }

        let force_ids: Vec<u32> = targets
            .iter()
            .filter(|target| target.is_running())
            .map(|target| target.pid)
            .collect();
        for target in targets.iter().filter(|target| target.is_running()) {
            if !target.can_terminate {
                log::warn!(
                    "target Codex process cannot be force-closed without PROCESS_TERMINATE access pid={}",
                    target.pid
                );
                continue;
            }
            // SAFETY: handle was opened with PROCESS_TERMINATE and is still owned.
            if unsafe { TerminateProcess(target.handle.raw(), 1) } == 0 {
                log::warn!(
                    "force-close target Codex process failed pid={} error={}",
                    target.pid,
                    std::io::Error::last_os_error()
                );
            }
        }

        if wait_until_exited(&targets, FORCE_CLOSE_TIMEOUT) {
            log::warn!(
                "target Codex processes required native force-close pids={:?}",
                force_ids
            );
            return Ok(());
        }

        let remaining: Vec<u32> = targets
            .iter()
            .filter(|target| target.is_running())
            .map(|target| target.pid)
            .collect();
        Err(EngineError::Install(format!(
            "target Codex process is still running after native close request (pids={remaining:?}); no files were replaced"
        )))
    }

    #[cfg(test)]
    mod windows_tests {
        use std::process::Command;

        use super::*;

        const HELPER_ENV: &str = "CODEX_APP_MANAGER_PROCESS_HELPER";

        #[test]
        fn closes_matching_process_without_powershell() {
            if std::env::var_os(HELPER_ENV).is_some() {
                thread::sleep(Duration::from_secs(30));
                return;
            }

            let root = std::env::temp_dir().join(format!(
                "codex-native-close-test-{}-{}",
                std::process::id(),
                uuid::Uuid::new_v4()
            ));
            std::fs::create_dir_all(&root).unwrap();
            let helper = root.join("Codex.exe");
            std::fs::copy(std::env::current_exe().unwrap(), &helper).unwrap();
            let mut child = Command::new(&helper)
                .args([
                    "--exact",
                    "windows_process::imp::windows_tests::closes_matching_process_without_powershell",
                    "--nocapture",
                ])
                .env(HELPER_ENV, "1")
                .spawn()
                .unwrap();

            let discover_deadline = Instant::now() + Duration::from_secs(10);
            while target_processes_under_root(&root).unwrap().is_empty() {
                if Instant::now() >= discover_deadline {
                    let _ = child.kill();
                    panic!("helper process was not discovered under its install root");
                }
                thread::sleep(Duration::from_millis(50));
            }

            let result = close_codex_processes_for_root(0, &root);
            if result.is_err() {
                let _ = child.kill();
            }
            result.unwrap();
            assert!(child.wait().unwrap().code().is_some());
            let _ = std::fs::remove_dir_all(root);
        }
    }
}

#[cfg(windows)]
pub(crate) use imp::close_codex_processes_for_root;

#[cfg(not(windows))]
pub(crate) fn close_codex_processes_for_root(
    _timeout_secs: u64,
    _root: &Path,
) -> Result<(), EngineError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_windows_paths_case_insensitively_and_on_component_boundaries() {
        let root = Path::new(r"C:\Users\Alice\Apps\Codex");
        assert!(path_is_within_root(
            Path::new(r"c:/users/ALICE/apps/codex/ChatGPT.exe"),
            root
        ));
        assert!(path_is_within_root(root, root));
        assert!(!path_is_within_root(
            Path::new(r"C:\Users\Alice\Apps\Codex-old\ChatGPT.exe"),
            root
        ));
    }

    #[test]
    fn normalizes_extended_drive_and_unc_prefixes() {
        assert_eq!(
            normalize_windows_path_text(r"\\?\C:\Users\Alice\Codex\"),
            r"c:\users\alice\codex"
        );
        assert_eq!(
            normalize_windows_path_text(r"\\?\UNC\server\share\Codex\"),
            r"\\server\share\codex"
        );
    }
}
