//! Bounded, PID-scoped startup observation. A modal error is not a healthy app.
#[derive(Default)]
pub(crate) struct StartupWindow {
    pub ready: bool,
    pub failure: Option<String>,
}

#[cfg(windows)]
pub(crate) fn inspect(pid: u32) -> StartupWindow {
    use windows_sys::Win32::Foundation::{HWND, LPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        EnumChildWindows, EnumWindows, GetClassNameW, GetWindowThreadProcessId, IsWindowVisible,
        SendMessageTimeoutW, SMTO_ABORTIFHUNG, WM_GETTEXT,
    };
    struct Probe {
        pid: u32,
        state: StartupWindow,
    }
    unsafe extern "system" fn text(hwnd: HWND, param: LPARAM) -> i32 {
        let texts = unsafe { &mut *(param as *mut Vec<String>) };
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
            texts.push(String::from_utf16_lossy(
                &buffer[..len.min(buffer.len() - 1)],
            ));
        }
        1
    }
    unsafe extern "system" fn window(hwnd: HWND, param: LPARAM) -> i32 {
        let probe = unsafe { &mut *(param as *mut Probe) };
        let mut pid = 0;
        unsafe {
            GetWindowThreadProcessId(hwnd, &mut pid);
        }
        if pid != probe.pid || unsafe { IsWindowVisible(hwnd) } == 0 {
            return 1;
        }
        let mut buffer = [0_u16; 128];
        let len = unsafe { GetClassNameW(hwnd, buffer.as_mut_ptr(), buffer.len() as i32) };
        let class = String::from_utf16_lossy(&buffer[..len.max(0) as usize]);
        if class == "Chrome_WidgetWin_1" {
            probe.state.ready = true;
        }
        if class == "#32770" {
            let mut texts: Vec<String> = Vec::new();
            unsafe {
                EnumChildWindows(hwnd, Some(text), &mut texts as *mut _ as LPARAM);
            }
            if texts.iter().any(|t| t.contains("failed to start")) {
                probe.state.failure = Some(texts.join(" "));
            }
        }
        1
    }
    let mut probe = Probe {
        pid,
        state: StartupWindow::default(),
    };
    // SAFETY: EnumWindows is synchronous; the stack context and buffers remain alive.
    unsafe {
        EnumWindows(Some(window), &mut probe as *mut _ as LPARAM);
    }
    probe.state
}

#[cfg(not(windows))]
pub(crate) fn inspect(_pid: u32) -> StartupWindow {
    StartupWindow {
        ready: true,
        failure: None,
    }
}
