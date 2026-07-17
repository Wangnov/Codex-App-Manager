use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use sha2::{Digest, Sha256};

use crate::limits::MAX_PACKAGE_BYTES;
use crate::network::{is_schannel_revocation_offline, NetworkConfig, SchannelRevocationCheck};
use crate::process::{
    curl_exe, hidden_command, run_with_progress, RunError, RunLimits, TimeoutKind,
};
use crate::EngineError;

static DOWNLOAD_ACTIVE: AtomicBool = AtomicBool::new(false);
static DOWNLOAD_CANCELLED: AtomicBool = AtomicBool::new(false);
static DOWNLOAD_DISCARD: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProgressMode {
    NoProgressMeter,
    SilentWithErrors,
}

#[derive(Debug, Eq, PartialEq)]
enum CurlAttemptError {
    Cancelled,
    Curl {
        exit_code: Option<i32>,
        stderr: String,
    },
    Other(String),
}

impl CurlAttemptError {
    fn into_message(self, url: &str) -> String {
        match self {
            Self::Cancelled => "download cancelled".to_string(),
            Self::Curl { exit_code, stderr } => curl_failure_message(url, exit_code, &stderr),
            Self::Other(message) => message,
        }
    }
}

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
    let base = format!(
        "curl failed for host={} exit={}: stderr='{}'",
        url_host(url),
        exit_code
            .map(|code| code.to_string())
            .unwrap_or_else(|| "signal".to_string()),
        stderr.trim(),
    );
    // Append the proxy diagnostic only for connectivity failures — pasting it
    // onto write / disk / HTTP errors (e.g. exit 23) only misleads.
    if is_connectivity_exit(exit_code) {
        format!("{base}; {}", proxy_env_summary())
    } else {
        base
    }
}

fn is_connectivity_exit(exit_code: Option<i32>) -> bool {
    matches!(
        exit_code,
        Some(
            5 | 6
                | 7
                | 28
                | 35
                | 52
                | 53
                | 54
                | 55
                | 56
                | 58
                | 59
                | 60
                | 67
                | 77
                | 80
                | 82
                | 83
                | 91
        )
    )
}

fn curl_args(
    url: &str,
    dest: &str,
    resume: bool,
    max_bytes: &str,
    progress_mode: ProgressMode,
    network: &NetworkConfig,
    revocation_check: SchannelRevocationCheck,
) -> Vec<String> {
    let mut args = network.curl_args_with_schannel_revocation(revocation_check);
    args.extend([
        "-fL".to_string(),
        "--proto".to_string(),
        "=https".to_string(),
        "--proto-redir".to_string(),
        "=https".to_string(),
    ]);
    match progress_mode {
        ProgressMode::NoProgressMeter => args.push("--no-progress-meter".to_string()),
        ProgressMode::SilentWithErrors => args.push("-sS".to_string()),
    }
    args.extend([
        "--connect-timeout".to_string(),
        "20".to_string(),
        "--max-filesize".to_string(),
        max_bytes.to_string(),
        "--retry".to_string(),
        "2".to_string(),
    ]);
    if resume {
        args.extend(["-C".to_string(), "-".to_string()]);
    }
    args.extend(["-o".to_string(), dest.to_string(), url.to_string()]);
    args
}

fn is_no_progress_meter_unsupported(exit_code: Option<i32>, stderr: &str) -> bool {
    exit_code == Some(2)
        && stderr.contains("--no-progress-meter")
        && (stderr.contains("is unknown") || stderr.contains("unknown option"))
}

fn run_curl_once(
    source: &str,
    dest: &str,
    args: Vec<String>,
    on_progress: &dyn Fn(u64),
) -> Result<(), CurlAttemptError> {
    let dest_path = PathBuf::from(dest);
    let mut command = hidden_command(curl_exe());
    command.args(args);

    // Stall on no byte growth; total bound stops an endlessly crawling transfer.
    // Cancel is cooperative via DOWNLOAD_CANCELLED — the runner kills the tree.
    let output = match run_with_progress(
        command,
        RunLimits::download(),
        Some(&DOWNLOAD_CANCELLED),
        &|| std::fs::metadata(&dest_path).map(|m| m.len()).unwrap_or(0),
        on_progress,
    ) {
        Ok(output) => output,
        Err(RunError::Cancelled) => {
            let downloaded = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
            log::info!("Windows download cancelled source={source} downloaded={downloaded}");
            return Err(CurlAttemptError::Cancelled);
        }
        Err(RunError::Timeout { kind, .. }) => {
            let downloaded = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
            return match kind {
                TimeoutKind::Stall => {
                    log::warn!(
                        "Windows download stalled source={source} downloaded={downloaded}"
                    );
                    Err(CurlAttemptError::Other(format!(
                        "curl download stalled (no progress) source={source} downloaded={downloaded}"
                    )))
                }
                TimeoutKind::Total => {
                    log::warn!(
                        "Windows download exceeded total deadline source={source} downloaded={downloaded}"
                    );
                    Err(CurlAttemptError::Other(format!(
                        "curl download exceeded total deadline source={source} downloaded={downloaded}"
                    )))
                }
            };
        }
        Err(err) => {
            return Err(CurlAttemptError::Other(format!("curl: {}", err.message())));
        }
    };

    if !output.status.success() {
        return Err(CurlAttemptError::Curl {
            exit_code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).to_string(),
        });
    }
    let downloaded = std::fs::metadata(dest).map(|m| m.len()).unwrap_or(0);
    on_progress(downloaded);
    log::info!("Windows curl download completed source={source} bytes={downloaded}");
    Ok(())
}

struct CurlRun<'a> {
    url: &'a str,
    dest: &'a str,
    resume: bool,
    max_bytes: &'a str,
    network: &'a NetworkConfig,
    on_progress: &'a dyn Fn(u64),
}

impl CurlRun<'_> {
    fn source(&self) -> &str {
        url_host(self.url)
    }
}

fn run_curl_with_mode(
    run: &CurlRun<'_>,
    progress_mode: ProgressMode,
    revocation_check: SchannelRevocationCheck,
) -> Result<(), CurlAttemptError> {
    let args = curl_args(
        run.url,
        run.dest,
        run.resume,
        run.max_bytes,
        progress_mode,
        run.network,
        revocation_check,
    );
    run_curl_once(run.source(), run.dest, args, run.on_progress)
}

fn retry_with_schannel_no_revoke(
    err: CurlAttemptError,
    run: &CurlRun<'_>,
    progress_mode: ProgressMode,
) -> Result<(), CurlAttemptError> {
    let should_retry = match &err {
        CurlAttemptError::Curl { exit_code, stderr } => {
            is_schannel_revocation_offline(*exit_code, stderr)
        }
        _ => false,
    };
    if !should_retry {
        return Err(err);
    }

    let source = run.source();
    log::warn!(
        "Windows curl Schannel revocation check failed; retrying with --ssl-no-revoke source={source}"
    );
    run_curl_with_mode(run, progress_mode, SchannelRevocationCheck::Disabled)
}

fn run_curl(
    url: &str,
    dest: &Path,
    resume: bool,
    max_bytes: u64,
    on_progress: &dyn Fn(u64),
    network: &NetworkConfig,
) -> Result<(), String> {
    let source = url_host(url);
    let dest = dest.to_string_lossy().into_owned();
    let max_bytes = max_bytes.to_string();
    let run = CurlRun {
        url,
        dest: &dest,
        resume,
        max_bytes: &max_bytes,
        network,
        on_progress,
    };

    if let Err(first_err) = run_curl_with_mode(
        &run,
        ProgressMode::NoProgressMeter,
        SchannelRevocationCheck::Strict,
    ) {
        if let CurlAttemptError::Curl { exit_code, stderr } = &first_err {
            if is_no_progress_meter_unsupported(*exit_code, stderr) {
                log::warn!(
                    "Windows curl does not support --no-progress-meter; retrying with -sS source={source}"
                );
                let compat_result = run_curl_with_mode(
                    &run,
                    ProgressMode::SilentWithErrors,
                    SchannelRevocationCheck::Strict,
                );
                return compat_result
                    .or_else(|err| {
                        retry_with_schannel_no_revoke(err, &run, ProgressMode::SilentWithErrors)
                    })
                    .map_err(|err| err.into_message(url));
            }
        }
        return retry_with_schannel_no_revoke(first_err, &run, ProgressMode::NoProgressMeter)
            .map_err(|err| err.into_message(url));
    }
    Ok(())
}

pub fn download_to(url: &str, dest: &Path) -> Result<(), EngineError> {
    download_to_with_progress(url, dest, &|_| {})
}

pub fn download_to_with_network(
    url: &str,
    dest: &Path,
    network: &NetworkConfig,
) -> Result<(), EngineError> {
    download_to_with_progress_with_network(url, dest, &|_| {}, network)
}

pub fn download_to_with_progress(
    url: &str,
    dest: &Path,
    on_progress: &dyn Fn(u64),
) -> Result<(), EngineError> {
    download_to_with_progress_bounded_with_network(
        url,
        dest,
        MAX_PACKAGE_BYTES,
        on_progress,
        &NetworkConfig::system(),
    )
}

pub fn download_to_with_progress_with_network(
    url: &str,
    dest: &Path,
    on_progress: &dyn Fn(u64),
    network: &NetworkConfig,
) -> Result<(), EngineError> {
    download_to_with_progress_bounded_with_network(
        url,
        dest,
        MAX_PACKAGE_BYTES,
        on_progress,
        network,
    )
}

pub fn download_to_with_progress_bounded(
    url: &str,
    dest: &Path,
    max_bytes: u64,
    on_progress: &dyn Fn(u64),
) -> Result<(), EngineError> {
    download_to_with_progress_bounded_with_network(
        url,
        dest,
        max_bytes,
        on_progress,
        &NetworkConfig::system(),
    )
}

pub fn download_to_with_progress_bounded_with_network(
    url: &str,
    dest: &Path,
    max_bytes: u64,
    on_progress: &dyn Fn(u64),
    network: &NetworkConfig,
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
    let download_result = run_curl(url, &part, should_resume, max_bytes, on_progress, network);
    if let Err(first_err) = download_result {
        if is_cancelled_error(&first_err) {
            if DOWNLOAD_DISCARD.load(Ordering::SeqCst) {
                let _ = std::fs::remove_file(&part);
            }
            return Err(EngineError::Io(first_err));
        }
        if should_resume {
            let _ = std::fs::remove_file(&part);
            log::warn!(
                "Windows resume failed; retrying fresh source={source} first_err={first_err}"
            );
            run_curl(url, &part, false, max_bytes, on_progress, network).map_err(|second_err| {
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
    let sha256 = hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
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
    fn curl_args_keep_modern_progress_flag_by_default() {
        let args = curl_args(
            "https://example.test/Codex.msix",
            "Codex.msix.part",
            false,
            "123",
            ProgressMode::NoProgressMeter,
            &NetworkConfig::system(),
            SchannelRevocationCheck::Strict,
        );

        assert!(args.contains(&"--no-progress-meter".to_string()));
        assert!(!args.contains(&"-sS".to_string()));
    }

    #[test]
    fn curl_args_can_fall_back_to_legacy_silent_mode() {
        let args = curl_args(
            "https://example.test/Codex.msix",
            "Codex.msix.part",
            true,
            "123",
            ProgressMode::SilentWithErrors,
            &NetworkConfig::system(),
            SchannelRevocationCheck::Strict,
        );

        assert!(args.contains(&"-sS".to_string()));
        assert!(!args.contains(&"--no-progress-meter".to_string()));
        assert!(args.windows(2).any(|pair| pair == ["-C", "-"]));
    }

    #[test]
    fn curl_args_include_custom_proxy_before_transfer_options() {
        let args = curl_args(
            "https://example.test/Codex.msix",
            "Codex.msix.part",
            false,
            "123",
            ProgressMode::NoProgressMeter,
            &NetworkConfig::custom("socks5h://127.0.0.1:7890"),
            SchannelRevocationCheck::Strict,
        );

        assert_eq!(args.first().map(String::as_str), Some("--proxy"));
        assert_eq!(
            args.get(1).map(String::as_str),
            Some("socks5h://127.0.0.1:7890")
        );
    }

    #[test]
    fn detects_old_curl_without_no_progress_meter() {
        let stderr = "curl: option --no-progress-meter: is unknown\ncurl: try 'curl --help' for more information";

        assert!(is_no_progress_meter_unsupported(Some(2), stderr));
        assert!(!is_no_progress_meter_unsupported(Some(22), stderr));
        assert!(!is_no_progress_meter_unsupported(
            Some(2),
            "curl: (6) Could not resolve host"
        ));
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
