//! Native clipboard writes avoid WKWebView losing user activation while
//! asynchronous diagnostics are collected.

pub fn write_text(app: &tauri::AppHandle, text: &str) -> Result<(), String> {
    if text.len() > 256 * 1024 || text.contains('\0') {
        return Err("Clipboard text is too large or contains a NUL byte".into());
    }
    platform_write(app, text)
}

#[cfg(target_os = "macos")]
fn platform_write(_app: &tauri::AppHandle, text: &str) -> Result<(), String> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new("/usr/bin/pbcopy")
        .env("LC_CTYPE", "UTF-8")
        .stdin(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let result = child
        .stdin
        .take()
        .ok_or("pbcopy stdin unavailable")
        .and_then(|mut stdin| {
            stdin
                .write_all(text.as_bytes())
                .map_err(|_| "pbcopy write failed")
        });
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait().map_err(|error| error.to_string())?;
    result.map_err(str::to_string)?;
    if !status.success() {
        return Err(format!("pbcopy failed: {status}"));
    }
    Ok(())
}

#[cfg(target_os = "windows")]
fn platform_write(app: &tauri::AppHandle, text: &str) -> Result<(), String> {
    use tauri::Manager;
    use windows_sys::Win32::Foundation::GlobalFree;
    use windows_sys::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows_sys::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
    };

    let window = app
        .get_webview_window("main")
        .ok_or("Main window unavailable")?;
    let hwnd = window.hwnd().map_err(|error| error.to_string())?.0;
    let utf16: Vec<u16> = text.encode_utf16().chain([0]).collect();
    unsafe {
        let memory = GlobalAlloc(GMEM_MOVEABLE, utf16.len() * 2);
        if memory.is_null() {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let destination = GlobalLock(memory) as *mut u16;
        if destination.is_null() {
            let error = std::io::Error::last_os_error().to_string();
            GlobalFree(memory);
            return Err(error);
        }
        std::ptr::copy_nonoverlapping(utf16.as_ptr(), destination, utf16.len());
        GlobalUnlock(memory);
        if OpenClipboard(hwnd) == 0 {
            let error = std::io::Error::last_os_error().to_string();
            GlobalFree(memory);
            return Err(error);
        }
        let result = if EmptyClipboard() == 0 || SetClipboardData(13, memory).is_null() {
            let error = std::io::Error::last_os_error().to_string();
            GlobalFree(memory);
            Err(error)
        } else {
            // Successful SetClipboardData transfers the allocation to Windows.
            Ok(())
        };
        CloseClipboard();
        result
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn platform_write(_app: &tauri::AppHandle, _text: &str) -> Result<(), String> {
    Err("Native clipboard is unavailable on this platform".into())
}
