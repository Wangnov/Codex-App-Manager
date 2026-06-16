use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::Serialize;

use crate::app::config_health::ConfigHealth;
use crate::app::logging::{logs_dir, redact_url};
use crate::app::mac_update::mac_install_status;
use crate::app::settings_store::AppSettings as PersistedAppSettings;
use crate::app::win_update::win_install_status;
use crate::domain::settings::AppSettings as DomainAppSettings;
use crate::domain::target::OperatingSystem;
use crate::state::ManagerState;

const LOG_TAIL_BYTES: u64 = 16 * 1024;
const RECENT_ERRORS_MAX: usize = 30;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Diagnostics {
    pub app_version: String,
    pub os: String,
    pub arch: String,
    pub locale: Option<String>,
    pub update_source: String,
    pub custom_source_host: Option<String>,
    pub windows_install_mode: Option<String>,
    pub install_status: String,
    pub config_health: ConfigHealth,
    pub logs_dir: Option<String>,
    pub recent_errors: Vec<String>,
    pub log_tail: String,
    pub generated_at_unix: u64,
}

pub fn collect_diagnostics(app: &tauri::AppHandle, state: &ManagerState) -> Diagnostics {
    let settings = PersistedAppSettings::load();
    let update_source = settings.source.as_str().to_string();
    let custom_source_host =
        (!settings.custom_url.trim().is_empty()).then(|| redact_url(&settings.custom_url));
    let windows_install_mode = matches!(state.target.os, OperatingSystem::Windows)
        .then(|| settings.windows_install_mode.clone());
    let install_status = install_status_summary(state, &settings);
    let config_health = state
        .config_health
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .clone();
    let log_dir = logs_dir(app);
    let log_tail = log_dir
        .as_deref()
        .and_then(newest_log_file)
        .map(|path| read_tail(&path, LOG_TAIL_BYTES))
        .unwrap_or_default();
    let recent_errors = recent_warning_error_lines(&log_tail);

    Diagnostics {
        app_version: app.package_info().version.to_string(),
        os: std::env::consts::OS.to_string(),
        arch: std::env::consts::ARCH.to_string(),
        locale: None,
        update_source,
        custom_source_host,
        windows_install_mode,
        install_status,
        config_health,
        logs_dir: log_dir.map(|path| path.to_string_lossy().into_owned()),
        recent_errors,
        log_tail,
        generated_at_unix: now_unix(),
    }
}

fn install_status_summary(state: &ManagerState, settings: &PersistedAppSettings) -> String {
    match state.target.os {
        OperatingSystem::Macos => {
            let status = mac_install_status();
            match status.installed {
                Some(installed) => format!(
                    "macos status={} build={} version={} path={}",
                    status.status, installed.build, installed.short_version, installed.path
                ),
                None => format!("macos status={}", status.status),
            }
        }
        OperatingSystem::Windows => {
            let domain_settings = DomainAppSettings::new(
                state.settings.mirror_base_url.clone(),
                settings.install_root.clone(),
            );
            let status = win_install_status(&domain_settings);
            match status.installed {
                Some(installed) => format!(
                    "windows status={} source={} version={} path={}",
                    status.status, installed.source, installed.version, installed.path
                ),
                None => format!("windows status={}", status.status),
            }
        }
        _ => "unsupported platform".to_string(),
    }
}

fn newest_log_file(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("codex-app-manager") && name.contains(".log"))
        })
        .filter_map(|path| {
            let modified = std::fs::metadata(&path).ok()?.modified().ok()?;
            Some((modified, path))
        })
        .max_by(|(mtime_a, path_a), (mtime_b, path_b)| {
            mtime_a
                .cmp(mtime_b)
                .then_with(|| path_a.file_name().cmp(&path_b.file_name()))
        })
        .map(|(_, path)| path)
}

fn read_tail(path: &Path, max_bytes: u64) -> String {
    let Ok(mut file) = std::fs::File::open(path) else {
        return String::new();
    };
    let Ok(len) = file.metadata().map(|metadata| metadata.len()) else {
        return String::new();
    };
    let start = len.saturating_sub(max_bytes);
    if file.seek(SeekFrom::Start(start)).is_err() {
        return String::new();
    }
    let mut bytes = Vec::new();
    if file.read_to_end(&mut bytes).is_err() {
        return String::new();
    }
    let text = String::from_utf8_lossy(&bytes);
    if start == 0 {
        text.into_owned()
    } else {
        text.split_once('\n')
            .map(|(_, rest)| rest.to_string())
            .unwrap_or_default()
    }
}

fn recent_warning_error_lines(log_tail: &str) -> Vec<String> {
    let mut lines = log_tail
        .lines()
        .filter(|line| line.contains("[ERROR]") || line.contains("[WARN]"))
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    if lines.len() > RECENT_ERRORS_MAX {
        lines.drain(..lines.len() - RECENT_ERRORS_MAX);
    }
    lines
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{read_tail, recent_warning_error_lines};

    #[test]
    fn read_tail_limits_and_starts_on_line_boundary() {
        let path =
            std::env::temp_dir().join(format!("codex-manager-tail-{}.log", std::process::id()));
        let body = format!("{}\nlast-one\nlast-two\n", "x".repeat(20_000));
        std::fs::write(&path, body).unwrap();

        let tail = read_tail(&path, 32);

        assert!(tail.len() <= 32);
        assert!(tail.starts_with("last-"));
        assert!(tail.ends_with("last-two\n"));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn recent_warning_error_lines_keeps_last_30() {
        let mut log = String::new();
        for idx in 0..35 {
            log.push_str(&format!("[WARN] warning {idx}\n"));
        }
        log.push_str("[INFO] ignored\n");
        let lines = recent_warning_error_lines(&log);
        assert_eq!(lines.len(), 30);
        assert_eq!(lines.first().unwrap(), "[WARN] warning 5");
        assert_eq!(lines.last().unwrap(), "[WARN] warning 34");
    }
}
