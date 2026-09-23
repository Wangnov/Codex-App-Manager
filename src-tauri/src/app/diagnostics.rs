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
    pub windows_runtime: Vec<String>,
    pub generated_at_unix: u64,
}

pub fn collect_diagnostics(app: &tauri::AppHandle, state: &ManagerState) -> Diagnostics {
    let settings = PersistedAppSettings::load();
    let update_source = settings.source.as_str().to_string();
    let custom_source_host =
        (!settings.custom_url.trim().is_empty()).then(|| redact_url(&settings.custom_url));
    let windows_install_mode = matches!(state.target.os, OperatingSystem::Windows)
        .then(|| settings.windows_install_mode.clone());
    let (install_status, installed_windows) = install_status_summary(state, &settings);
    let windows_runtime = if matches!(state.target.os, OperatingSystem::Windows) {
        windows_runtime_summary(installed_windows.as_ref())
    } else {
        Vec::new()
    };
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
        arch: state.target.arch.as_str().to_string(),
        locale: None,
        update_source,
        custom_source_host,
        windows_install_mode,
        install_status,
        config_health,
        logs_dir: log_dir.map(|path| path.to_string_lossy().into_owned()),
        recent_errors,
        log_tail,
        windows_runtime,
        generated_at_unix: now_unix(),
    }
}

fn install_status_summary(
    state: &ManagerState,
    settings: &PersistedAppSettings,
) -> (String, Option<codex_win_engine::InstalledWindowsCodex>) {
    match state.target.os {
        OperatingSystem::Macos => {
            let status = mac_install_status();
            (
                match status.installed {
                    Some(installed) => format!(
                        "macos status={} build={} version={} path={}",
                        status.status, installed.build, installed.short_version, installed.path
                    ),
                    None => format!("macos status={}", status.status),
                },
                None,
            )
        }
        OperatingSystem::Windows => {
            let domain_settings = DomainAppSettings::new(
                state.settings.mirror_base_url.clone(),
                settings.install_root.clone(),
            );
            let status = win_install_status(&domain_settings);
            let summary = match status.installed.as_ref() {
                Some(installed) => format!(
                    "windows status={} source={} version={} path={}",
                    status.status, installed.source, installed.version, installed.path
                ),
                None => format!("windows status={}", status.status),
            };
            (summary, status.installed)
        }
        _ => ("unsupported platform".to_string(), None),
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
    if file.take(max_bytes).read_to_end(&mut bytes).is_err() {
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

pub(crate) fn record_windows_runtime_failure(
    installed: Option<&codex_win_engine::InstalledWindowsCodex>,
) {
    for line in windows_runtime_summary(installed) {
        log::warn!("Windows runtime diagnostic: {line}");
    }
}

fn windows_runtime_summary(
    installed: Option<&codex_win_engine::InstalledWindowsCodex>,
) -> Vec<String> {
    let mut lines = Vec::new();
    if let Some(installed) = installed {
        let root = Path::new(&installed.path);
        let resources = [
            "app/resources",
            "resources",
            "VFS/ProgramFilesX64/Codex/resources",
            "VFS/ProgramFilesArm64/Codex/resources",
        ]
        .iter()
        .map(|p| root.join(p))
        .find(|p| p.is_dir());
        if let Some(resources) = resources {
            for component in [
                "codex.exe",
                "codex-command-runner.exe",
                "codex-windows-sandbox-setup.exe",
                "cua_node/bin/node.exe",
                "cua_node/bin/node_repl.exe",
            ] {
                let status = match std::fs::File::open(resources.join(component)) {
                    Ok(file) => match file.metadata() {
                        Ok(meta) if meta.is_file() && meta.len() > 0 => "readable".to_string(),
                        _ => "empty-or-not-file".to_string(),
                    },
                    Err(err) => format!("{:?} (os={:?})", err.kind(), err.raw_os_error()),
                };
                lines.push(format!("Bundled {component}: {status}"));
            }
        } else {
            lines.push("Bundled runtime: resources directory not found or inaccessible".into());
        }
    }
    // Shell-activated MSIX apps may inherit a different environment. Record
    // presence only; never export user values, commands, credentials or paths.
    for name in [
        "CODEX_CLI_PATH",
        "CODEX_HOME",
        "CODEX_WINDOWS_REGISTERED_CORE",
        "CODEX_ELECTRON_USER_DATA_PATH",
    ] {
        let present =
            std::env::var_os(name).is_some_and(|v| !v.to_string_lossy().trim().is_empty());
        lines.push(format!(
            "Manager environment {name}: {}",
            if present { "set" } else { "unset" }
        ));
    }
    if let Some(local) = std::env::var_os("LOCALAPPDATA") {
        let local = PathBuf::from(local);
        let mut roots = vec![local.join("Codex/Logs")];
        if let Some(family) = installed.and_then(|i| i.package_family_name.as_ref()) {
            // The registered family is a single filename, never a path.
            if !family.is_empty()
                && family
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
            {
                roots.push(
                    local
                        .join("Packages")
                        .join(family)
                        .join("LocalCache/Local/Codex/Logs"),
                );
            }
        }
        let mut files = Vec::new();
        let mut budget = 2048;
        for root in roots {
            recent_codex_logs(&root, 0, &mut budget, &mut files);
        }
        files.sort_by_key(|entry| std::cmp::Reverse(entry.0));
        files.truncate(16);
        lines.push(format!("Codex startup log scan: {} recent file(s), up to 128 KiB per file; selected signals only", files.len()));
        for (modified, path) in files {
            let modified = modified
                .duration_since(UNIX_EPOCH)
                .map(|v| v.as_secs())
                .unwrap_or(0);
            for signal in startup_log_signals(&read_tail(&path, 128 * 1024)) {
                let line = format!("App log modifiedUnix={modified}: {signal}");
                if !lines.contains(&line) {
                    lines.push(line);
                }
            }
        }
    }
    lines
}

fn recent_codex_logs(
    dir: &Path,
    depth: usize,
    budget: &mut usize,
    out: &mut Vec<(SystemTime, PathBuf)>,
) {
    if depth > 3 || *budget == 0 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.take(*budget).filter_map(Result::ok).collect();
    *budget = budget.saturating_sub(entries.len());
    entries.sort_by_key(|e| std::cmp::Reverse(e.file_name()));
    for entry in entries {
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if kind.is_dir() && name.chars().all(|c| c.is_ascii_digit()) {
            recent_codex_logs(&entry.path(), depth + 1, budget, out);
        } else if kind.is_file() && name.starts_with("codex-desktop-") && name.ends_with(".log") {
            if let Ok(modified) = entry.metadata().and_then(|m| m.modified()) {
                if SystemTime::now()
                    .duration_since(modified)
                    .unwrap_or_default()
                    .as_secs()
                    <= 48 * 3600
                {
                    out.push((modified, entry.path()));
                }
            }
        }
    }
}

/// Extract only known event names and enumerated safe fields, never entire app
/// log lines: those can contain prompts, server URLs and authentication data.
fn startup_log_signals(text: &str) -> Vec<String> {
    const EVENTS: &[&str] = &[
        "bundled_executable_relocation_failed",
        "windows_core_runtime_launch_selected",
        "windows_core_runtime_component_resolved",
        "windows_runtime_framework_gate_hydration_failed",
        "windows_primary_runtime_framework_activation_failed",
        "Desktop bootstrap failed to start the main app",
        "app_server_connection.state_changed",
        "Unable to locate the Codex CLI binary",
        "[wsl] error retrieving eligible distro",
        "WSL bash availability check failed",
        "WSL command error",
        "WSL command returned empty output",
    ];
    let mut signals = Vec::new();
    for line in text.lines() {
        let Some(event) = EVENTS.iter().find(|event| line.contains(**event)) else {
            continue;
        };
        let mut signal = event.to_string();
        let tokens: Vec<_> = line
            .split(|c: char| c.is_whitespace() || ['=', ':', ',', '"', '{', '}'].contains(&c))
            .filter(|s| !s.is_empty())
            .collect();
        for (key, allowed) in [
            (
                "errorCode",
                &["ENOENT", "EACCES", "EPERM", "EBUSY", "ENOSPC", "UNKNOWN"][..],
            ),
            ("selectedRuntimeSource", &["app-package", "bundled"][..]),
            (
                "source",
                &["override", "bundled", "registered-core", "app-package"][..],
            ),
            ("appPackageCoreActive", &["true", "false"][..]),
            ("copiedRuntimeLocation", &["true", "false"][..]),
            ("next", &["connecting", "connected", "disconnected"][..]),
            ("initialized", &["true", "false"][..]),
            (
                "operation",
                &["copy", "rename", "mkdir", "stat", "hash", "unknown"][..],
            ),
            (
                "executableName",
                &[
                    "codex.exe",
                    "node.exe",
                    "node_repl.exe",
                    "codex-command-runner.exe",
                    "codex-windows-sandbox-setup.exe",
                ][..],
            ),
        ] {
            if let Some(pair) = tokens
                .windows(2)
                .find(|pair| pair[0] == key && allowed.contains(&pair[1]))
            {
                signal.push_str(&format!(" {key}={}", pair[1]));
            }
        }
        if !signals.contains(&signal) {
            signals.push(signal);
        }
    }
    signals
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

    #[test]
    fn startup_signals_keep_only_known_events_and_safe_enum_values() {
        let text = r#"ordinary chat prompt secret-sk-123
2026-09-23 [warning] bundled_executable_relocation_failed errorCode=EACCES operation=copy executableName=codex.exe sourcePath=C:\Users\private error=secret-sk-456
{"message":"windows_core_runtime_launch_selected","selectedRuntimeSource":"app-package","token":"secret-sk-789"}
[error] [wsl] error retrieving eligible distro error=private-distro
bundled_executable_relocation_failed errorCode=secret-sk-abc operation=private-command
"#;
        let signals = super::startup_log_signals(text);
        assert_eq!(signals, vec![
            "bundled_executable_relocation_failed errorCode=EACCES operation=copy executableName=codex.exe",
            "windows_core_runtime_launch_selected selectedRuntimeSource=app-package",
            "[wsl] error retrieving eligible distro",
            "bundled_executable_relocation_failed",
        ]);
        assert!(!signals.join("\n").contains("private"));
        assert!(!signals.join("\n").contains("secret"));
    }

    #[test]
    fn runtime_log_discovery_is_bounded_and_ignores_other_files() {
        let root =
            std::env::temp_dir().join(format!("codex-runtime-logs-{}", uuid::Uuid::new_v4()));
        let day = root.join("2026/09/23");
        std::fs::create_dir_all(&day).unwrap();
        std::fs::write(day.join("codex-desktop-fixture.log"), b"fixture").unwrap();
        std::fs::write(day.join("auth.json"), b"private").unwrap();
        let mut files = Vec::new();
        super::recent_codex_logs(&root, 0, &mut 0, &mut files);
        assert!(files.is_empty());
        super::recent_codex_logs(&root, 0, &mut 20, &mut files);
        assert_eq!(files.len(), 1);
        assert!(files[0].1.ends_with("codex-desktop-fixture.log"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
