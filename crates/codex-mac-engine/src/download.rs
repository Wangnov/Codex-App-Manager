//! Resumable downloader for update artifacts.
//!
//! Scaffold transport: shells out to `curl -C -` (HTTP range resume), which
//! both R2/S3 and oaistatic support. Production will swap in a proper async
//! HTTP client behind the same function signature; keeping it here lets the
//! verify/plan flow be exercised end-to-end today.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crate::limits::MAX_PACKAGE_BYTES;
use crate::EngineError;

const CURL: &str = "/usr/bin/curl";
const STALL_TIMEOUT: Duration = Duration::from_secs(120);

static DOWNLOAD_ACTIVE: AtomicBool = AtomicBool::new(false);
static DOWNLOAD_CANCELLED: AtomicBool = AtomicBool::new(false);
static DOWNLOAD_DISCARD: AtomicBool = AtomicBool::new(false);

struct DownloadGuard;

impl DownloadGuard {
    fn acquire() -> Result<Self, EngineError> {
        DOWNLOAD_CANCELLED.store(false, Ordering::SeqCst);
        DOWNLOAD_DISCARD.store(false, Ordering::SeqCst);
        DOWNLOAD_ACTIVE
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .map(|_| Self)
            .map_err(|_| EngineError::Io("another macOS package download is already active".into()))
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

/// Download `url` into `dest`, resuming a partial file if present.
/// Returns the final file size in bytes.
pub fn download_to(url: &str, dest: &Path) -> Result<u64, EngineError> {
    download_to_with_progress(url, dest, &|_| {})
}

/// Download with periodic progress callbacks. Spawns `curl` and polls the
/// destination file size (curl writes incrementally) every ~300ms, invoking
/// `on_progress(downloaded_bytes)`; resumes a partial file like `download_to`.
pub fn download_to_with_progress(
    url: &str,
    dest: &Path,
    on_progress: &dyn Fn(u64),
) -> Result<u64, EngineError> {
    download_to_with_progress_bounded(url, dest, MAX_PACKAGE_BYTES, on_progress)
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

fn is_cancelled_error(err: &EngineError) -> bool {
    matches!(err, EngineError::Io(msg) if msg == "download cancelled")
}

fn run_curl(
    url: &str,
    dest: &Path,
    resume: bool,
    max_bytes: u64,
    on_progress: &dyn Fn(u64),
) -> Result<(), EngineError> {
    let source = url_host(url);
    let dest_arg = dest.to_string_lossy().into_owned();
    let max_bytes = max_bytes.to_string();
    let mut args = vec![
        "-fL".to_string(),
        "--proto".to_string(),
        "=https".to_string(),
        "--proto-redir".to_string(),
        "=https".to_string(),
        "--no-progress-meter".to_string(),
    ];
    if resume {
        args.extend(["-C".to_string(), "-".to_string()]);
    }
    args.extend([
        "--retry".to_string(),
        "5".to_string(),
        "--retry-delay".to_string(),
        "2".to_string(),
        "--retry-all-errors".to_string(),
        "--connect-timeout".to_string(),
        "20".to_string(),
        "--max-filesize".to_string(),
        max_bytes,
        "-o".to_string(),
        dest_arg,
        url.to_string(),
    ]);

    let mut child = Command::new(CURL)
        .args(args)
        .spawn()
        .map_err(|e| EngineError::Io(format!("spawn curl: {e}")))?;

    let mut last_downloaded = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    let mut last_progress = Instant::now();
    on_progress(last_downloaded);

    loop {
        if DOWNLOAD_CANCELLED.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            let downloaded = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
            log::info!("macOS download cancelled source={source} downloaded={downloaded}");
            return Err(EngineError::Io("download cancelled".to_string()));
        }
        match child
            .try_wait()
            .map_err(|e| EngineError::Io(e.to_string()))?
        {
            Some(status) => {
                if !status.success() {
                    return Err(EngineError::Io(format!("curl download failed: {url}")));
                }
                break;
            }
            None => {
                let downloaded = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
                if downloaded > last_downloaded {
                    last_downloaded = downloaded;
                    last_progress = Instant::now();
                } else if last_progress.elapsed() >= STALL_TIMEOUT {
                    let _ = child.kill();
                    let _ = child.wait();
                    let stall_secs = STALL_TIMEOUT.as_secs();
                    log::warn!(
                        "macOS download stalled source={source} downloaded={downloaded} stall_secs={stall_secs}"
                    );
                    return Err(EngineError::Io(format!(
                        "curl download stalled for {} seconds: {url}",
                        STALL_TIMEOUT.as_secs()
                    )));
                }
                on_progress(downloaded);
                std::thread::sleep(Duration::from_millis(300));
            }
        }
    }

    let downloaded = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    on_progress(downloaded);
    log::info!("macOS curl download completed source={source} bytes={downloaded}");
    Ok(())
}

pub fn download_to_with_progress_bounded(
    url: &str,
    dest: &Path,
    max_bytes: u64,
    on_progress: &dyn Fn(u64),
) -> Result<u64, EngineError> {
    let _guard = DownloadGuard::acquire()?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| EngineError::Io(e.to_string()))?;
    }

    let part = partial_path(dest);
    let should_resume = part.metadata().map(|m| m.len() > 0).unwrap_or(false);
    let source = url_host(url);
    let dest_name = dest
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("download");
    log::info!(
        "macOS download start source={source} dest={dest_name} resume={should_resume} max_bytes={max_bytes}"
    );
    let download_result = run_curl(url, &part, should_resume, max_bytes, on_progress);
    if let Err(first_err) = download_result {
        if is_cancelled_error(&first_err) {
            if DOWNLOAD_DISCARD.load(Ordering::SeqCst) {
                let _ = std::fs::remove_file(&part);
            }
            return Err(first_err);
        }
        if should_resume {
            let _ = std::fs::remove_file(&part);
            log::warn!("macOS resume failed; retrying fresh source={source} first_err={first_err}");
            run_curl(url, &part, false, max_bytes, on_progress).map_err(|second_err| {
                if is_cancelled_error(&second_err) {
                    if DOWNLOAD_DISCARD.load(Ordering::SeqCst) {
                        let _ = std::fs::remove_file(&part);
                    }
                    return second_err;
                }
                let _ = std::fs::remove_file(&part);
                EngineError::Io(format!(
                    "resume failed ({first_err}); fresh download failed ({second_err})"
                ))
            })?;
        } else {
            let _ = std::fs::remove_file(&part);
            return Err(first_err);
        }
    }

    std::fs::rename(&part, dest).map_err(|e| EngineError::Io(format!("publish download: {e}")))?;
    let meta = std::fs::metadata(dest).map_err(|e| EngineError::Io(e.to_string()))?;
    on_progress(meta.len());
    let bytes = meta.len();
    log::info!("macOS download finished source={source} bytes={bytes}");
    Ok(meta.len())
}

/// Read a downloaded artifact into memory for verification.
pub fn read_file(path: &Path) -> Result<Vec<u8>, EngineError> {
    std::fs::read(path).map_err(|e| EngineError::Io(e.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    // Mirrors the Windows engine's guard test: the guard serializes downloads,
    // pause vs cancel set the discard flag correctly, and dropping the guard
    // clears every flag so the next download starts clean. No network/curl is
    // touched, and no other test in this crate mutates these process globals.
    #[test]
    fn guard_serializes_and_pause_cancel_set_flags() {
        let guard = DownloadGuard::acquire().unwrap();

        // PAUSE: reports the download active, keeps the partial (discard = false).
        assert!(pause_active_download());
        assert!(DOWNLOAD_CANCELLED.load(Ordering::SeqCst));
        assert!(!DOWNLOAD_DISCARD.load(Ordering::SeqCst));

        // CANCEL: reports active, discards the partial (discard = true).
        assert!(cancel_active_download());
        assert!(DOWNLOAD_DISCARD.load(Ordering::SeqCst));

        // A second concurrent download is rejected while one is active.
        assert!(DownloadGuard::acquire().is_err());

        // Dropping the guard clears every flag…
        drop(guard);
        assert!(!DOWNLOAD_CANCELLED.load(Ordering::SeqCst));
        assert!(!DOWNLOAD_DISCARD.load(Ordering::SeqCst));

        // …and with nothing active, a stop request reports nothing to stop.
        assert!(!cancel_active_download());
    }
}
