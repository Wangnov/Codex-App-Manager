use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use sha2::{Digest, Sha256};

use crate::limits::MAX_PACKAGE_BYTES;
use crate::process::{curl_exe, hidden_command};
use crate::EngineError;

static DOWNLOAD_ACTIVE: AtomicBool = AtomicBool::new(false);
static DOWNLOAD_CANCELLED: AtomicBool = AtomicBool::new(false);
static DOWNLOAD_DISCARD: AtomicBool = AtomicBool::new(false);

struct DownloadGuard;

impl DownloadGuard {
    fn acquire() -> Result<Self, String> {
        DOWNLOAD_CANCELLED.store(false, Ordering::SeqCst);
        DOWNLOAD_DISCARD.store(false, Ordering::SeqCst);
        DOWNLOAD_ACTIVE
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| Self)
            .map_err(|_| "another Windows package download is already active".to_string())
    }
}

impl Drop for DownloadGuard {
    fn drop(&mut self) {
        DOWNLOAD_ACTIVE.store(false, Ordering::SeqCst);
        DOWNLOAD_CANCELLED.store(false, Ordering::SeqCst);
        DOWNLOAD_DISCARD.store(false, Ordering::SeqCst);
    }
}

fn request_cancel(discard_partial: bool) -> bool {
    let active = DOWNLOAD_ACTIVE.load(Ordering::SeqCst);
    if active {
        DOWNLOAD_DISCARD.store(discard_partial, Ordering::SeqCst);
        DOWNLOAD_CANCELLED.store(true, Ordering::SeqCst);
    }
    active
}

pub fn pause_active_download() -> bool {
    request_cancel(false)
}

pub fn cancel_active_download() -> bool {
    request_cancel(true)
}

fn is_cancelled_error(err: &str) -> bool {
    err == "download cancelled"
}

fn partial_path(dest: &Path) -> PathBuf {
    let file_name = dest
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("download");
    dest.with_file_name(format!("{file_name}.part"))
}

fn url_host(url: &str) -> &str {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or("")
}

fn proxy_env_summary() -> String {
    let vars = ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY", "NO_PROXY"];
    let configured = vars
        .iter()
        .filter(|name| std::env::var_os(name).is_some())
        .copied()
        .collect::<Vec<_>>();
    if configured.is_empty() {
        "no curl proxy environment variables are set; Windows system proxy/PAC may not be used automatically".to_string()
    } else {
        format!(
            "curl proxy environment variables set: {}",
            configured.join(", ")
        )
    }
}

fn curl_failure_message(url: &str, exit_code: Option<i32>, stderr: &str) -> String {
    format!(
        "curl failed for host={} exit={}: stderr='{}'; {}",
        url_host(url),
        exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".to_string()),
        stderr.trim(),
        proxy_env_summary(),
    )
}

fn run_curl(
    url: &str,
    dest: &Path,
    resume: bool,
    max_bytes: u64,
    on_progress: &dyn Fn(u64),
) -> Result<(), String> {
    let source = url_host(url);
    let dest = dest.to_string_lossy().into_owned();
    let max_bytes = max_bytes.to_string();
    let mut args = vec![
        "-fL".to_string(),
        "--proto".to_string(),
        "=https".to_string(),
        "--proto-redir".to_string(),
        "=https".to_string(),
        "--no-progress-meter".to_string(),
        "--connect-timeout".to_string(),
        "20".to_string(),
        "--max-filesize".to_string(),
        max_bytes,
        "--retry".to_string(),
        "2".to_string(),
    ];
    if resume {
        args.extend(["-C".to_string(), "-".to_string()]);
    }
    args.extend(["-o".to_string(), dest.clone(), url.to_string()]);

    let mut child = hidden_command(curl_exe())
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn curl: {e}"))?;

    loop {
        if DOWNLOAD_CANCELLED.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            let downloaded = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
            log::info!("Windows download cancelled source={source} downloaded={downloaded}");
            return Err("download cancelled".to_string());
        }
        let downloaded = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
        on_progress(downloaded);
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) => thread::sleep(Duration::from_millis(200)),
            Err(err) => {
                let _ = child.kill();
                return Err(format!("wait for curl: {err}"));
            }
        }
    }

    let output = match child.wait_with_output() {
        Ok(output) => output,
        Err(err) => return Err(format!("collect curl output: {err}")),
    };

    if !output.status.success() {
        return Err(curl_failure_message(
            url,
            output.status.code(),
            &String::from_utf8_lossy(&output.stderr),
        ));
    }
    let downloaded = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    on_progress(downloaded);
    log::info!("Windows curl download completed source={source} bytes={downloaded}");
    Ok(())
}

pub fn download_to(url: &str, dest: &Path) -> Result<(), EngineError> {
    download_to_with_progress(url, dest, &|_| {})
}

pub fn download_to_with_progress(
    url: &str,
    dest: &Path,
    on_progress: &dyn Fn(u64),
) -> Result<(), EngineError> {
    download_to_with_progress_bounded(url, dest, MAX_PACKAGE_BYTES, on_progress)
}

pub fn download_to_with_progress_bounded(
    url: &str,
    dest: &Path,
    max_bytes: u64,
    on_progress: &dyn Fn(u64),
) -> Result<(), EngineError> {
    // The manager has one Windows package staging slot. Serialize downloads so
    // auto-stage and manual-stage cannot reset each other's cancel flag.
    let _guard = DownloadGuard::acquire().map_err(EngineError::Io)?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| EngineError::Io(format!("create staging dir: {e}")))?;
    }

    let part = partial_path(dest);
    let should_resume = part.metadata().map(|m| m.len() > 0).unwrap_or(false);
    let source = url_host(url);
    let dest_name = dest
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("download");
    log::info!(
        "Windows download start source={source} dest={dest_name} resume={should_resume} max_bytes={max_bytes}"
    );
    let download_result = run_curl(url, &part, should_resume, max_bytes, on_progress);
    if let Err(first_err) = download_result {
        if is_cancelled_error(&first_err) {
            if DOWNLOAD_DISCARD.load(Ordering::SeqCst) {
                let _ = std::fs::remove_file(&part);
            }
            return Err(EngineError::Io(first_err));
        }
        if should_resume {
            let _ = std::fs::remove_file(&part);
            log::warn!("Windows resume failed; retrying fresh source={source} first_err={first_err}");
            run_curl(url, &part, false, max_bytes, on_progress).map_err(|second_err| {
                if is_cancelled_error(&second_err) {
                    if DOWNLOAD_DISCARD.load(Ordering::SeqCst) {
                        let _ = std::fs::remove_file(&part);
                    }
                    return EngineError::Io(second_err);
                }
                EngineError::Io(format!(
                    "resume failed ({first_err}); fresh download failed ({second_err})"
                ))
            })?;
        } else {
            return Err(EngineError::Io(first_err));
        }
    }

    if dest.exists() {
        std::fs::remove_file(dest)
            .map_err(|e| EngineError::Io(format!("remove previous download: {e}")))?;
    }
    std::fs::rename(&part, dest).map_err(|e| EngineError::Io(format!("publish download: {e}")))?;
    let bytes = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    log::info!("Windows download finished source={source} bytes={bytes}");
    Ok(())
}

pub fn read_file(path: &Path) -> Result<Vec<u8>, EngineError> {
    std::fs::read(path).map_err(|e| EngineError::Io(format!("read {}: {e}", path.display())))
}

pub fn sha256_file(path: &Path) -> Result<String, EngineError> {
    let mut file = std::fs::File::open(path)
        .map_err(|e| EngineError::Io(format!("open {}: {e}", path.display())))?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 128 * 1024];
    loop {
        let read = file
            .read(&mut buf)
            .map_err(|e| EngineError::Io(format!("read {}: {e}", path.display())))?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    let sha256 = format!("{:x}", hasher.finalize());
    let sha256_prefix = sha256.get(..12).unwrap_or(&sha256);
    log::info!("SHA256 calculation completed sha256_prefix={sha256_prefix}");
    Ok(sha256)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_url(path: &Path) -> String {
        let mut path = path.to_string_lossy().replace('\\', "/");
        if !path.starts_with('/') {
            path = format!("/{path}");
        }
        format!("file://{}", path.replace(' ', "%20"))
    }

    #[test]
    fn download_guard_rejects_concurrent_downloads() {
        let guard = DownloadGuard::acquire().unwrap();
        assert!(cancel_active_download());
        assert!(DownloadGuard::acquire().is_err());
        drop(guard);
        assert!(!cancel_active_download());
    }

    #[test]
    fn download_with_progress_reports_final_size() {
        if hidden_command(curl_exe())
            .arg("--version")
            .output()
            .is_err()
        {
            return;
        }

        let root =
            std::env::temp_dir().join(format!("codex-win-engine-progress-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let source = root.join("source.bin");
        let dest = root.join("dest.bin");
        let bytes = vec![0x4d; 1024 * 1024];
        std::fs::write(&source, &bytes).unwrap();

        let seen = std::sync::Mutex::new(Vec::new());
        let result = download_to_with_progress(&file_url(&source), &dest, &|downloaded| {
            seen.lock().unwrap().push(downloaded);
        });
        if let Err(EngineError::Io(message)) = &result {
            if message.contains("Protocol \"file\" disabled") {
                let _ = std::fs::remove_dir_all(&root);
                return;
            }
        }
        result.unwrap();

        assert_eq!(std::fs::read(&dest).unwrap(), bytes);
        assert!(seen.lock().unwrap().contains(&(1024 * 1024)));
        let _ = std::fs::remove_dir_all(&root);
    }
}
