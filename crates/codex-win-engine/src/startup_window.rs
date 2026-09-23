//! Bounded, PID-scoped startup observation. A modal error is not a healthy app.
use std::time::Duration;

#[derive(Default)]
pub(crate) struct StartupWindow {
    pub ready: bool,
    pub failure: Option<String>,
}

/// Require a continuous settling period AFTER the main window appears. A slow
/// startup must not consume that period before its runtime has initialized.
#[derive(Default)]
pub(crate) struct StartupProgress {
    ready_since: Option<Duration>,
}

impl StartupProgress {
    pub(crate) fn observe(
        &mut self,
        elapsed: Duration,
        window: &StartupWindow,
        stable_for: Duration,
    ) -> Result<bool, String> {
        if let Some(failure) = &window.failure {
            return Err(format!("Codex startup dialog: {failure}"));
        }
        if !window.ready {
            self.ready_since = None;
            return Ok(false);
        }
        let since = *self.ready_since.get_or_insert(elapsed);
        Ok(elapsed.saturating_sub(since) >= stable_for)
    }
}

#[cfg(any(windows, test))]
fn is_startup_failure(text: &str) -> bool {
    let text = text.to_ascii_lowercase();
    text.contains("failed to start")
        || text.contains("unable to locate the codex cli binary")
        || text.contains("required runtime components")
        || text.contains("the process has no package identity")
        || text.contains("该进程没有程序包标识符")
}

#[cfg(windows)]
pub(crate) fn inspect(pid: u32) -> StartupWindow {
    inspect_pids(&[pid])
}

#[cfg(windows)]
pub(crate) fn inspect_pids(pids: &[u32]) -> StartupWindow {
    use std::time::Instant;
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible,
        SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_GETTEXT,
    };
    struct Probe<'a> {
        pids: &'a [u32],
        state: StartupWindow,
        deadline: Instant,
    }
    struct TextProbe {
        texts: Vec<String>,
        deadline: Instant,
        visited: usize,
    }
    unsafe extern "system" fn text(hwnd: HWND, param: LPARAM) -> i32 {
        let probe = unsafe { &mut *(param as *mut TextProbe) };
        if Instant::now() >= probe.deadline || probe.visited >= 64 {
            return 0;
        }
        probe.visited += 1;
        let mut buffer = [0_u16; 2048];
        let mut len = 0usize;
        // GetWindowText cannot read another process's child-control text.
        // Bound WM_GETTEXT so a hung dialog cannot hang installation itself.
        let ok = unsafe {
            SendMessageTimeoutW(
                hwnd,
                WM_GETTEXT,
                buffer.len(),
                buffer.as_mut_ptr() as LPARAM,
                SMTO_ABORTIFHUNG,
                50,
                &mut len,
            )
        };
        if ok != 0 && len > 0 {
            probe.texts.push(String::from_utf16_lossy(
                &buffer[..len.min(buffer.len() - 1)],
            ));
        }
        1
    }
    unsafe extern "system" fn window(hwnd: HWND, param: LPARAM) -> i32 {
        let probe = unsafe { &mut *(param as *mut Probe) };
        if Instant::now() >= probe.deadline {
            return 0;
        }
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
        }
        if !probe.pids.contains(&pid) || unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }
        let mut buffer = [0_u16; 128];
        let len = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
        let class = String::from_utf16_lossy(&buffer[..len.max(0) as usize]);
        if class == "Chrome_WidgetWin_1" {
            probe.state.ready = true;
        }
        if class == "#32770" {
            let mut texts = TextProbe {
                texts: Vec::new(),
                deadline: probe.deadline,
                visited: 0,
            };
            unsafe {
                // Electron can put "ChatGPT failed to start" only in the title.
                // Inspect both the title and body, including rebranded builds.
                text(hwnd, &mut texts as *mut _ as LPARAM);
                EnumChildWindows(hwnd, Some(text), &mut texts as *mut _ as LPARAM);
            }
            if texts.texts.iter().any(|t| is_startup_failure(t)) {
                probe.state.failure = Some(texts.texts.join(" "));
            }
        }
        1
    }
    let mut probe = Probe {
        pids,
        state: StartupWindow::default(),
        deadline: Instant::now() + Duration::from_millis(300),
    };
    // SAFETY: EnumWindows is synchronous; the stack context and buffers remain alive.
    unsafe {
        EnumWindows(Some(window), &mut probe as *mut _ as LPARAM);
    }
    probe.state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_error_body_or_title_is_a_failure_but_ordinary_dialog_is_not() {
        for text in [
            "ChatGPT failed to start",
            "CODEX FAILED TO START",
            "Unable to locate the Codex CLI binary or required runtime components. Check the installation or explicit runtime overrides.",
            "该进程没有程序包标识符。",
        ] {
            assert!(is_startup_failure(text), "{text}");
        }
        assert!(!is_startup_failure("Do you want to quit Codex?"));
    }

    #[test]
    fn late_window_must_settle_and_error_takes_priority_over_main_window() {
        let mut progress = StartupProgress::default();
        let stable = Duration::from_secs(3);
        let ready = StartupWindow {
            ready: true,
            failure: None,
        };
        assert!(!progress
            .observe(Duration::from_secs(15), &ready, stable)
            .unwrap());
        assert!(!progress
            .observe(Duration::from_secs(17), &ready, stable)
            .unwrap());
        let error = StartupWindow {
            ready: true,
            failure: Some("runtime missing".into()),
        };
        assert!(progress
            .observe(Duration::from_secs(18), &error, stable)
            .is_err());
        assert!(progress
            .observe(Duration::from_secs(18), &ready, stable)
            .unwrap());
        assert!(!progress
            .observe(Duration::from_secs(19), &StartupWindow::default(), stable)
            .unwrap());
        assert!(!progress
            .observe(Duration::from_secs(20), &ready, stable)
            .unwrap());
        assert!(progress
            .observe(Duration::from_secs(23), &ready, stable)
            .unwrap());
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "child process fixture; exercised by native_startup_windows_are_scoped_and_reject_errors"]
    fn native_window_fixture() {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let Ok(mode) = std::env::var("CODEX_STARTUP_WINDOW_FIXTURE") else {
            return;
        };
        let wide = |text: &str| text.encode_utf16().chain(Some(0)).collect::<Vec<_>>();
        // Own off-screen test windows; never interact with an installed Codex.
        let main_class = wide("Chrome_WidgetWin_1");
        let class = WNDCLASSW {
            lpfnWndProc: Some(DefWindowProcW),
            lpszClassName: main_class.as_ptr(),
            ..Default::default()
        };
        unsafe {
            assert_ne!(RegisterClassW(&class), 0);
            if mode == "ready" || mode == "late-error" {
                assert!(!CreateWindowExW(
                    0,
                    main_class.as_ptr(),
                    wide("startup fixture").as_ptr(),
                    WS_POPUP | WS_VISIBLE,
                    -32000,
                    -32000,
                    200,
                    100,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null()
                )
                .is_null());
            }
        }
        let started = std::time::Instant::now();
        let mut dialog_created = false;
        while started.elapsed() < Duration::from_secs(12) {
            if mode != "ready"
                && !dialog_created
                && (mode != "late-error" || started.elapsed() >= Duration::from_secs(1))
            {
                let title = if mode == "title" {
                    "ChatGPT FAILED TO START"
                } else {
                    "ChatGPT"
                };
                unsafe {
                    let dialog = CreateWindowExW(
                        0,
                        wide("#32770").as_ptr(),
                        wide(title).as_ptr(),
                        WS_POPUP | WS_VISIBLE,
                        -32000,
                        -32000,
                        200,
                        100,
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null_mut(),
                        std::ptr::null(),
                    );
                    assert!(!dialog.is_null());
                    if mode != "title" {
                        assert!(!CreateWindowExW(0, wide("STATIC").as_ptr(), wide("Unable to locate the Codex CLI binary or required runtime components.").as_ptr(),
                            WS_CHILD | WS_VISIBLE, 0, 0, 200, 100, dialog, std::ptr::null_mut(), std::ptr::null_mut(), std::ptr::null()).is_null());
                    }
                }
                dialog_created = true;
            }
            let mut msg = MSG::default();
            unsafe {
                while PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(not(windows))]
pub(crate) fn inspect(_pid: u32) -> StartupWindow {
    StartupWindow {
        ready: true,
        failure: None,
    }
}
