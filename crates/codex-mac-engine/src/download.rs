//! Resumable downloader for update artifacts.
//!
//! Scaffold transport: shells out to `curl -C -` (HTTP range resume), which
//! both R2/S3 and oaistatic support. Production will swap in a proper async
//! HTTP client behind the same function signature; keeping it here lets the
//! verify/plan flow be exercised end-to-end today.

use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::EngineError;

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
    let _guard = DownloadGuard::acquire()?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| EngineError::Io(e.to_string()))?;
    }

    let mut child = Command::new("curl")
        .args([
            "-fL",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--no-progress-meter",
            "-C",
            "-",
            "--retry",
            "5",
            "--retry-delay",
            "2",
            "--retry-all-errors",
            "--connect-timeout",
            "20",
            "-o",
            &dest.to_string_lossy(),
            url,
        ])
        .spawn()
        .map_err(|e| EngineError::Io(format!("spawn curl: {e}")))?;

    loop {
        if DOWNLOAD_CANCELLED.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            if DOWNLOAD_DISCARD.load(Ordering::SeqCst) {
                let _ = std::fs::remove_file(dest);
            }
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
                on_progress(downloaded);
                std::thread::sleep(std::time::Duration::from_millis(300));
            }
        }
    }

    let meta = std::fs::metadata(dest).map_err(|e| EngineError::Io(e.to_string()))?;
    on_progress(meta.len());
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
