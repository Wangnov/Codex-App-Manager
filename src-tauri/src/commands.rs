use std::path::{Path, PathBuf};

use tauri::{Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::DialogExt;

use crate::app::mac_update::{
    cancel_macos_download, install_macos, pause_macos_download, perform_macos_update,
    plan_macos_update, stage_macos_update, uninstall_macos, MacInstallStatus, MacPerformReport,
    MacStageReport, MacUninstallReport, MacUpdateReport, PerformExpectation,
};
use crate::app::settings_store::AppSettings as PersistedAppSettings;
use crate::app::win_update::{
    auto_stage_windows_update_with_install_mode, cancel_windows_download, pause_windows_download,
    perform_windows_update_with_install_mode, plan_windows_update_with_install_mode,
    stage_windows_update_with_install_mode, uninstall_windows_codex,
    win_adopt as adopt_windows_install, win_install_status,
    DownloadProgress as WinDownloadProgress, WinAutoStageReport, WinInstallStatus,
    WinPerformExpectation, WinPerformReport, WinStageReport, WinUninstallReport, WinUpdateReport,
};
use crate::domain::settings::AppSettings as DomainAppSettings;
use crate::domain::target::OperatingSystem;
use crate::errors::{AppError, CommandError};
use crate::state::ManagerState;

fn normalize_windows_source_base(raw: &str) -> Option<String> {
    let mut base = raw.trim().trim_end_matches('/').to_string();
    if base.is_empty() {
        return None;
    }
    for suffix in [
        "/latest/manifest",
        "/latest/checksums",
        "/latest/win-unpacked",
        "/latest/win",
        "/latest",
    ] {
        if let Some(stripped) = base.strip_suffix(suffix) {
            base = stripped.trim_end_matches('/').to_string();
            break;
        }
    }
    (!base.is_empty()).then_some(base)
}

fn windows_endpoints_for_settings(
    state: &ManagerState,
) -> Result<crate::domain::manifest::MirrorEndpoints, AppError> {
    let mut saved = PersistedAppSettings::load();
    normalize_settings_for_target(&mut saved, &state.target);
    match saved.source.as_str() {
        "custom" => {
            let base = normalize_windows_source_base(&saved.custom_url)
                .unwrap_or_else(|| state.settings.mirror_base_url.clone());
            Ok(crate::domain::manifest::MirrorEndpoints::from_base_url(&base))
        }
        "official" => Err(AppError::Engine(
            "Windows official update source is not available yet; choose mirror, auto, or a custom source that serves latest/manifest, latest/checksums, and latest/win.".to_string(),
        )),
        // Windows currently depends on the mirror-style manifest/checksum/MSIX
        // endpoints. `auto` therefore resolves to the known-good mirror until an
        // official source exposes the same contract.
        "auto" | "mirror" => Ok(state.endpoints.clone()),
        _ => Ok(state.endpoints.clone()),
    }
}

fn normalize_settings_for_target(settings: &mut PersistedAppSettings, target: &crate::domain::target::Target) {
    if matches!(target.os, OperatingSystem::Windows) && settings.source == "official" {
        settings.source = "auto".to_string();
    }
}

fn windows_install_mode_for_settings() -> String {
    let saved = PersistedAppSettings::load();
    if saved.windows_install_mode == "portable" {
        "portable".to_string()
    } else {
        "msix".to_string()
    }
}

fn windows_domain_settings_for_persisted(state: &ManagerState) -> DomainAppSettings {
    let saved = PersistedAppSettings::load();
    let mut settings = state.settings.clone();
    settings.install_root = saved.install_root;
    settings
}

fn dialog_start_dir(path: &str) -> PathBuf {
    let path = PathBuf::from(path);
    if path.is_dir() {
        return path;
    }
    path.parent()
        .filter(|parent| parent.is_dir())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from(PersistedAppSettings::load().install_root))
}

const MIN_PORTABLE_FREE_SPACE_BYTES: u64 = 1_073_741_824;

fn path_key(path: &Path) -> String {
    path.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}

fn path_is_equal_or_child(path: &Path, root: &Path) -> bool {
    let path = path_key(path);
    let root = path_key(root);
    path == root || path.starts_with(&format!("{root}\\"))
}

#[cfg(windows)]
fn protected_windows_roots() -> Vec<PathBuf> {
    [
        "ProgramFiles",
        "ProgramFiles(x86)",
        "ProgramW6432",
        "ProgramData",
        "SystemRoot",
        "WINDIR",
    ]
    .into_iter()
    .filter_map(std::env::var_os)
    .filter(|value| !value.is_empty())
    .map(PathBuf::from)
    .collect()
}

#[cfg(not(windows))]
fn protected_windows_roots() -> Vec<PathBuf> {
    Vec::new()
}

fn is_filesystem_root(path: &Path) -> bool {
    path.parent().is_none() || (path.has_root() && path.components().count() <= 2)
}

fn is_protected_install_root(path: &Path) -> bool {
    is_filesystem_root(path)
        || protected_windows_roots()
            .iter()
            .any(|root| path_is_equal_or_child(path, root))
}

fn is_existing_codex_portable_root(path: &Path) -> bool {
    path.join("Codex.exe").is_file() && path.join("AppxManifest.xml").is_file()
}

fn directory_is_empty(path: &Path) -> Result<bool, AppError> {
    let mut entries = std::fs::read_dir(path)
        .map_err(|e| AppError::Internal(format!("读取安装位置失败: {e}")))?;
    Ok(entries.next().is_none())
}

#[cfg(windows)]
fn available_space(path: &Path) -> Result<Option<u64>, AppError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    let mut free_to_caller = 0_u64;
    let mut total = 0_u64;
    let mut total_free = 0_u64;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_to_caller,
            &mut total,
            &mut total_free,
        )
    };
    if ok == 0 {
        return Err(AppError::Internal(format!(
            "读取磁盘剩余空间失败: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(Some(free_to_caller))
}

#[cfg(not(windows))]
fn available_space(_path: &Path) -> Result<Option<u64>, AppError> {
    Ok(None)
}

fn validate_install_root_path(raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::Internal("安装位置不能为空".to_string()));
    }
    let path = PathBuf::from(trimmed);
    if !path.is_absolute() {
        return Err(AppError::Internal("安装位置必须是绝对路径".to_string()));
    }
    if path.exists() && !path.is_dir() {
        return Err(AppError::Internal("安装位置必须是文件夹".to_string()));
    }
    if is_protected_install_root(&path) {
        return Err(AppError::Internal(
            "安装位置不能放在系统目录、管理员目录或磁盘根目录".to_string(),
        ));
    }
    if path.exists() && !directory_is_empty(&path)? && !is_existing_codex_portable_root(&path) {
        return Err(AppError::Internal(
            "安装位置必须是空文件夹，或已有的 Codex 免安装版目录".to_string(),
        ));
    }
    // Probe writability and free space WITHOUT creating the target directory:
    // merely validating or remembering a location must not leave folders on
    // disk. We probe the nearest existing ancestor — it shares the volume (so
    // the free-space figure matches) and a writable parent means the installer
    // can create the leaf later. The directory is created at install time by
    // install_portable_from_msix, not here.
    let probe_dir = nearest_existing_dir(&path);
    let probe = probe_dir.join(format!(".codex-manager-write-test-{}", std::process::id()));
    std::fs::write(&probe, b"ok")
        .map_err(|e| AppError::Internal(format!("安装位置不可写: {e}")))?;
    let _ = std::fs::remove_file(&probe);
    if let Some(free) = available_space(&probe_dir)? {
        if free < MIN_PORTABLE_FREE_SPACE_BYTES {
            return Err(AppError::Internal(
                "安装位置所在磁盘剩余空间不足".to_string(),
            ));
        }
    }
    Ok(path.to_string_lossy().into_owned())
}

/// Nearest existing directory at or above `path`. Lets us probe writability and
/// free space for a not-yet-created install root without creating it.
fn nearest_existing_dir(path: &Path) -> PathBuf {
    let mut cur = path;
    loop {
        if cur.is_dir() {
            return cur.to_path_buf();
        }
        match cur.parent() {
            Some(parent) => cur = parent,
            None => return cur.to_path_buf(),
        }
    }
}

fn install_root_from_picked_dir(raw: &str) -> Result<String, AppError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(AppError::Internal("安装位置不能为空".to_string()));
    }
    let selected = PathBuf::from(trimmed);
    if !selected.is_absolute() {
        return Err(AppError::Internal("安装位置必须是绝对路径".to_string()));
    }
    if selected.exists() && !selected.is_dir() {
        return Err(AppError::Internal("安装位置必须是文件夹".to_string()));
    }
    if is_filesystem_root(&selected) || is_protected_install_root(&selected) {
        return validate_install_root_path(trimmed);
    }
    if selected.exists()
        && selected.is_dir()
        && !directory_is_empty(&selected)?
        && !is_existing_codex_portable_root(&selected)
    {
        return validate_install_root_path(&selected.join("Codex").to_string_lossy());
    }
    validate_install_root_path(trimmed)
}

/// macOS-only: detect the installed Codex build, read the Sparkle appcast, and
/// return an update plan (delta vs full). Read-only — performs no install.
#[tauri::command]
pub async fn mac_plan_update(
    simulated_build: Option<u64>,
) -> Result<MacUpdateReport, CommandError> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::UnsupportedPlatform.into());
    }
    // Off the main thread: the appcast fetch (plus the auto-source official
    // probe) is network IO — running it inline froze the webview, so the
    // re-check spinner never animated ("卡一下没动画").
    tauri::async_runtime::spawn_blocking(move || plan_macos_update(simulated_build))
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))?
        .map_err(Into::into)
}

/// macOS-only: plan + download + size/EdDSA verify into staging. Non-destructive
/// (no apply/swap). Runs the blocking download off the main thread.
#[tauri::command]
pub async fn mac_stage_update(
    simulated_build: Option<u64>,
) -> Result<MacStageReport, CommandError> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::UnsupportedPlatform.into());
    }
    tauri::async_runtime::spawn_blocking(move || stage_macos_update(simulated_build))
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))?
        .map_err(Into::into)
}

/// Locate the vendored Sparkle `BinaryDelta` tool, if present: an explicit
/// `CODEX_BINARY_DELTA` override first (testing / a system Sparkle), then the app
/// bundle's resources. Returns `None` when it isn't found — a *full*-package
/// update doesn't need it, so resolution is best-effort and only the delta path
/// errors on a genuine miss.
fn resolve_binary_delta(app: &tauri::AppHandle) -> Option<std::path::PathBuf> {
    if let Ok(p) = std::env::var("CODEX_BINARY_DELTA") {
        let pb = std::path::PathBuf::from(p);
        if pb.exists() {
            return Some(pb);
        }
    }
    for rel in ["resources/BinaryDelta", "BinaryDelta"] {
        if let Ok(res) = app.path().resolve(rel, tauri::path::BaseDirectory::Resource) {
            if res.exists() {
                return Some(res);
            }
        }
    }
    None
}

/// macOS-only **destructive** update: download+verify → reconstruct → codesign
/// gate → graceful quit → atomic same-volume swap → health-check → relaunch (or
/// rollback). Requires an explicit `confirm: true` from a UI second confirmation;
/// runs the blocking work off the main thread.
#[tauri::command]
pub async fn mac_perform_update(
    app: tauri::AppHandle,
    confirm: bool,
    expected_from_build: u64,
    expected_to_build: u64,
    expected_path: String,
) -> Result<MacPerformReport, CommandError> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::UnsupportedPlatform.into());
    }
    if !confirm {
        return Err(AppError::Internal("拒绝执行：破坏性更新必须带显式 confirm".to_string()).into());
    }
    // Best-effort: a full-package update needs no delta tool, so don't reject the
    // whole operation when it's absent — only the delta branch requires it.
    let binary_delta = resolve_binary_delta(&app);
    // The user confirmed a specific target; the backend re-verifies reality still
    // matches before the destructive swap (guards a TOCTOU vs appcast refresh /
    // Codex self-update between confirm and execute).
    let expected = PerformExpectation {
        from_build: expected_from_build,
        to_build: expected_to_build,
        install_path: expected_path,
    };
    let progress_app = app.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let report = move |p: crate::app::mac_update::DownloadProgress| {
            let _ = progress_app.emit("mac://download-progress", p);
        };
        perform_macos_update(binary_delta, expected, &report)
    })
    .await
    .map_err(|e| AppError::Internal(format!("join: {e}")))?
    .map_err(Into::into)
}

/// macOS-only: classify the installed Codex (managed / external / none).
#[tauri::command]
pub fn mac_status(state: State<'_, ManagerState>) -> Result<MacInstallStatus, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Macos) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    Ok(crate::app::mac_update::mac_install_status())
}

/// macOS-only: adopt the detected external install (after explicit user consent).
#[tauri::command]
pub fn mac_adopt(state: State<'_, ManagerState>) -> Result<MacInstallStatus, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Macos) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    crate::app::mac_update::mac_adopt().map_err(Into::into)
}

/// macOS-only: open the installed Codex.app (explicit 〔打开 Codex〕 action).
#[tauri::command]
pub fn mac_launch_codex() -> Result<(), CommandError> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::UnsupportedPlatform.into());
    }
    crate::app::mac_update::launch_codex().map_err(Into::into)
}

/// macOS-only: fresh-install the latest Codex (full package) into /Applications.
/// Runs the blocking download/verify/install off the main thread.
#[tauri::command]
pub async fn mac_install(app: tauri::AppHandle) -> Result<MacInstallStatus, CommandError> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::UnsupportedPlatform.into());
    }
    tauri::async_runtime::spawn_blocking(move || {
        let report = move |p: crate::app::mac_update::DownloadProgress| {
            let _ = app.emit("mac://download-progress", p);
        };
        install_macos(&report)
    })
    .await
    .map_err(|e| AppError::Internal(format!("join: {e}")))?
    .map_err(Into::into)
}

/// macOS-only: request pausing an active package download.
/// Partial bytes are left in place for the next resume-capable run.
#[tauri::command]
pub fn mac_pause_download() -> Result<bool, CommandError> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::UnsupportedPlatform.into());
    }
    Ok(pause_macos_download())
}

/// macOS-only: request cancellation of an active package download.
/// Partial bytes are discarded.
#[tauri::command]
pub fn mac_cancel_download() -> Result<bool, CommandError> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::UnsupportedPlatform.into());
    }
    Ok(cancel_macos_download())
}

/// Windows-only: detect installed Codex, read mirror manifest/checksums, probe
/// sideload capabilities, and return the preferred update path. Read-only.
#[tauri::command]
pub async fn win_plan_update(
    state: State<'_, ManagerState>,
) -> Result<WinUpdateReport, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let endpoints = windows_endpoints_for_settings(&state)?;
    let settings = windows_domain_settings_for_persisted(&state);
    let install_mode = windows_install_mode_for_settings();
    tauri::async_runtime::spawn_blocking(move || {
        plan_windows_update_with_install_mode(&endpoints, &settings, &install_mode)
    })
    .await
    .map_err(|e| AppError::Internal(format!("join: {e}")))?
    .map_err(Into::into)
}

/// Windows-only: plan + download + size/SHA256/AuthentiCode/AppxManifest gates
/// into staging. Non-destructive (no install yet).
#[tauri::command]
pub async fn win_stage_update(
    state: State<'_, ManagerState>,
) -> Result<WinStageReport, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let endpoints = windows_endpoints_for_settings(&state)?;
    let settings = windows_domain_settings_for_persisted(&state);
    let install_mode = windows_install_mode_for_settings();
    tauri::async_runtime::spawn_blocking(move || {
        stage_windows_update_with_install_mode(&endpoints, &settings, &install_mode, &|_| {})
    })
    .await
    .map_err(|e| AppError::Internal(format!("join: {e}")))?
    .map_err(Into::into)
}

/// Read persisted app settings (update source + general).
#[tauri::command]
pub fn get_settings(state: State<'_, ManagerState>) -> Result<PersistedAppSettings, CommandError> {
    let mut settings = PersistedAppSettings::load();
    normalize_settings_for_target(&mut settings, &state.target);
    Ok(settings)
}

/// Persist app settings. `signed_only` is forced on regardless of input.
#[tauri::command]
pub fn set_settings(
    state: State<'_, ManagerState>,
    settings: PersistedAppSettings,
) -> Result<PersistedAppSettings, CommandError> {
    let mut s = settings;
    s.normalize();
    normalize_settings_for_target(&mut s, &state.target);
    s.save()?;
    Ok(s)
}

/// The user confirmed quitting from the close dialog — flag it and exit so the
/// CloseRequested / ExitRequested guards stop intercepting and let it go.
#[tauri::command]
pub fn confirm_quit(app: tauri::AppHandle, state: State<'_, ManagerState>) {
    state
        .force_quit
        .store(true, std::sync::atomic::Ordering::SeqCst);
    app.exit(0);
}

/// Windows-only: return the current user's default portable install root.
#[tauri::command]
pub fn win_default_install_root(state: State<'_, ManagerState>) -> Result<String, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    Ok(PersistedAppSettings::default().install_root)
}

/// Windows-only: open a system folder picker for the portable install root.
#[tauri::command]
pub async fn win_pick_install_dir(
    app: tauri::AppHandle,
    state: State<'_, ManagerState>,
) -> Result<Option<String>, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let start_dir = dialog_start_dir(&PersistedAppSettings::load().install_root);
    let selected = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("选择 Codex 安装位置")
            .set_directory(start_dir)
            .blocking_pick_folder()
            .map(|path| {
                path.into_path()
                    .map(|p| p.to_string_lossy().into_owned())
                    .map_err(|e| AppError::Internal(format!("读取选择的文件夹失败: {e}")))
            })
            .transpose()
    })
    .await
    .map_err(|e| AppError::Internal(format!("join: {e}")))??;
    selected
        .as_deref()
        .map(install_root_from_picked_dir)
        .transpose()
        .map_err(Into::into)
}

/// Windows-only: persist a validated portable install root.
#[tauri::command]
pub fn win_set_install_root(
    state: State<'_, ManagerState>,
    path: String,
) -> Result<PersistedAppSettings, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let install_root = validate_install_root_path(&path)?;
    let mut settings = PersistedAppSettings::load();
    settings.install_root = install_root;
    settings.normalize();
    settings.save()?;
    Ok(settings)
}

/// Windows-only: reset the remembered portable install root to the per-user default.
#[tauri::command]
pub fn win_reset_install_root(
    state: State<'_, ManagerState>,
) -> Result<PersistedAppSettings, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let install_root = validate_install_root_path(&PersistedAppSettings::default().install_root)?;
    let mut settings = PersistedAppSettings::load();
    settings.install_root = install_root;
    settings.normalize();
    settings.save()?;
    Ok(settings)
}

/// macOS-only **destructive**: uninstall Codex. Requires explicit `confirm`.
/// `keep_codex_home` defaults true at the UI — `~/.codex` survives unless the
/// user opts out. Runs the blocking work off the main thread.
#[tauri::command]
pub async fn mac_uninstall(
    confirm: bool,
    keep_codex_home: bool,
) -> Result<MacUninstallReport, CommandError> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::UnsupportedPlatform.into());
    }
    if !confirm {
        return Err(AppError::Internal("拒绝执行：卸载必须带显式 confirm".to_string()).into());
    }
    tauri::async_runtime::spawn_blocking(move || uninstall_macos(keep_codex_home))
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))?
        .map_err(Into::into)
}

/// Windows-only: background pre-download guard. It stages only when the user
/// enabled auto download and the current network passes the metered policy.
#[tauri::command]
pub async fn win_auto_stage_update(
    state: State<'_, ManagerState>,
    enabled: bool,
    allow_metered: bool,
) -> Result<WinAutoStageReport, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let endpoints = windows_endpoints_for_settings(&state)?;
    let settings = windows_domain_settings_for_persisted(&state);
    let install_mode = windows_install_mode_for_settings();
    tauri::async_runtime::spawn_blocking(move || {
        auto_stage_windows_update_with_install_mode(
            &endpoints,
            &settings,
            enabled,
            allow_metered,
            &install_mode,
        )
    })
    .await
    .map_err(|e| AppError::Internal(format!("join: {e}")))?
    .map_err(Into::into)
}

/// Windows-only: request pausing an active background/manual download.
/// Partial bytes are left in place for the next resume-capable staging run.
#[tauri::command]
pub fn win_pause_download(state: State<'_, ManagerState>) -> Result<bool, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    Ok(pause_windows_download())
}

/// Windows-only: request cancellation of an active background/manual download.
/// Partial bytes are discarded.
#[tauri::command]
pub fn win_cancel_download(state: State<'_, ManagerState>) -> Result<bool, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    Ok(cancel_windows_download())
}

/// Whether "launch at login" is currently enabled (off by default).
#[tauri::command]
pub fn get_autostart(app: tauri::AppHandle) -> Result<bool, CommandError> {
    app.autolaunch()
        .is_enabled()
        .map_err(|e| AppError::Internal(format!("autostart: {e}")).into())
}

/// Enable/disable launch at login. The user opts in explicitly from Settings.
#[tauri::command]
pub fn set_autostart(app: tauri::AppHandle, enabled: bool) -> Result<(), CommandError> {
    let mgr = app.autolaunch();
    let result = if enabled { mgr.enable() } else { mgr.disable() };
    result.map_err(|e| AppError::Internal(format!("autostart: {e}")).into())
}

/// Open an external http(s) URL in the user's default browser. Restricted to
/// http(s) so it can't be coerced into launching arbitrary local handlers.
#[tauri::command]
pub fn open_url(url: String) -> Result<(), CommandError> {
    validate_external_http_url(&url)?;
    open_external_url(&url).map_err(|e| AppError::Internal(format!("打开链接失败: {e}")).into())
}

fn validate_external_http_url(url: &str) -> Result<(), AppError> {
    if url
        .chars()
        .any(|ch| ch.is_control() || ch.is_whitespace() || ch == '\\')
    {
        return Err(AppError::Internal("链接包含非法字符".to_string()));
    }
    let Some((scheme, rest)) = url.split_once("://") else {
        return Err(AppError::Internal("仅支持 http(s) 链接".to_string()));
    };
    if !(scheme.eq_ignore_ascii_case("https") || scheme.eq_ignore_ascii_case("http")) {
        return Err(AppError::Internal("仅支持 http(s) 链接".to_string()));
    }
    let host = rest
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .trim_matches('.');
    if host.is_empty() || host.contains('@') {
        return Err(AppError::Internal("链接缺少有效主机名".to_string()));
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn open_external_url(url: &str) -> Result<(), String> {
    std::process::Command::new("open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "windows")]
fn open_external_url(url: &str) -> Result<(), String> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let operation: Vec<u16> = OsStr::new("open").encode_wide().chain([0]).collect();
    let target: Vec<u16> = OsStr::new(url).encode_wide().chain([0]).collect();
    let result = unsafe {
        ShellExecuteW(
            null_mut(),
            operation.as_ptr(),
            target.as_ptr(),
            null(),
            null(),
            SW_SHOWNORMAL,
        )
    } as isize;
    if result <= 32 {
        Err(format!("ShellExecuteW failed with code {result}"))
    } else {
        Ok(())
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn open_external_url(url: &str) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(url)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod open_url_tests {
    use super::validate_external_http_url;

    #[test]
    fn accepts_http_urls_with_query_delimiters() {
        assert!(validate_external_http_url("https://example.com/a?x=1&y=2").is_ok());
    }

    #[test]
    fn rejects_non_http_and_shell_sensitive_url_shapes() {
        for url in [
            "file:///C:/Windows/notepad.exe",
            "https://example.com/a b",
            "https://example.com\\evil",
            "https://user@example.com/",
            "https://",
        ] {
            assert!(validate_external_http_url(url).is_err(), "{url} should be rejected");
        }
    }
}

/// Windows-only: classify the installed Codex (managed / external / none).
#[tauri::command]
pub fn win_status(state: State<'_, ManagerState>) -> Result<WinInstallStatus, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let settings = windows_domain_settings_for_persisted(&state);
    Ok(win_install_status(&settings))
}

/// Windows-only: adopt the detected external install (after explicit consent).
#[tauri::command]
pub fn win_adopt(state: State<'_, ManagerState>) -> Result<WinInstallStatus, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let settings = windows_domain_settings_for_persisted(&state);
    adopt_windows_install(&settings).map_err(Into::into)
}

/// Windows-only: open the installed Codex.
#[tauri::command]
pub fn win_launch_codex(state: State<'_, ManagerState>) -> Result<(), CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let settings = windows_domain_settings_for_persisted(&state);
    crate::app::win_update::launch_codex(&settings).map_err(Into::into)
}

/// Windows-only: guarded execution. Requires explicit confirmation, stages and
/// verifies the MSIX first, then attempts Add-AppxPackage without elevation or
/// policy changes. Reports portable fallback need transparently.
#[tauri::command]
pub async fn win_perform_update(
    app: tauri::AppHandle,
    state: State<'_, ManagerState>,
    confirm: bool,
    install_root: Option<String>,
    expected: Option<WinPerformExpectation>,
) -> Result<WinPerformReport, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let endpoints = windows_endpoints_for_settings(&state)?;
    let mut settings = windows_domain_settings_for_persisted(&state);
    // Validate (but don't yet persist) an explicitly chosen install root: it
    // only becomes the remembered default after the install actually succeeds,
    // so a failed or cancelled attempt never changes the user's saved location.
    let pending_install_root = match install_root {
        Some(raw) => {
            let validated = validate_install_root_path(&raw)?;
            settings.install_root = validated.clone();
            Some(validated)
        }
        None => None,
    };
    let install_mode = windows_install_mode_for_settings();
    let progress_app = app.clone();
    let report = tauri::async_runtime::spawn_blocking(move || {
        let report = move |p: WinDownloadProgress| {
            let _ = progress_app.emit("win://download-progress", p);
        };
        perform_windows_update_with_install_mode(
            &endpoints,
            &settings,
            confirm,
            &install_mode,
            expected,
            &report,
        )
    })
    .await
    .map_err(|e| AppError::Internal(format!("join: {e}")))??;
    if let Some(root) = pending_install_root {
        if report.success {
            let mut saved = PersistedAppSettings::load();
            saved.install_root = root;
            saved.normalize();
            saved.save()?;
        }
    }
    Ok(report)
}

/// Windows-only: guarded uninstall. Only removes installs recorded as managed
/// by this app. User data is preserved unless `purge_user_data` is true.
#[tauri::command]
pub async fn win_uninstall(
    state: State<'_, ManagerState>,
    confirm: bool,
    purge_user_data: bool,
) -> Result<WinUninstallReport, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let settings = windows_domain_settings_for_persisted(&state);
    tauri::async_runtime::spawn_blocking(move || {
        uninstall_windows_codex(&settings, confirm, purge_user_data)
    })
    .await
    .map_err(|e| AppError::Internal(format!("join: {e}")))?
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::{
        install_root_from_picked_dir, normalize_windows_source_base, validate_install_root_path,
    };
    use std::fs;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("{name}-{}", std::process::id()))
    }

    #[test]
    fn normalizes_windows_source_base_urls() {
        assert_eq!(
            normalize_windows_source_base("https://example.test/latest/manifest").as_deref(),
            Some("https://example.test")
        );
        assert_eq!(
            normalize_windows_source_base("https://example.test/latest/win/").as_deref(),
            Some("https://example.test")
        );
        assert_eq!(
            normalize_windows_source_base("https://example.test/custom").as_deref(),
            Some("https://example.test/custom")
        );
        assert!(normalize_windows_source_base("   ").is_none());
    }

    #[test]
    fn rejects_non_empty_non_codex_install_root() {
        let root = temp_path("codex-manager-non-empty-root");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("notes.txt"), b"not codex").unwrap();

        let err = validate_install_root_path(&root.to_string_lossy()).unwrap_err();
        assert!(
            err.to_string()
                .contains("安装位置必须是空文件夹，或已有的 Codex 免安装版目录")
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn accepts_existing_codex_portable_install_root() {
        let root = temp_path("codex-manager-existing-portable-root");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("Codex.exe"), b"codex").unwrap();
        fs::write(root.join("AppxManifest.xml"), b"<Package />").unwrap();

        let validated = validate_install_root_path(&root.to_string_lossy()).unwrap();
        assert_eq!(validated, root.to_string_lossy());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn picked_non_empty_folder_installs_into_codex_child() {
        let parent = temp_path("codex-manager-picked-parent");
        let _ = fs::remove_dir_all(&parent);
        fs::create_dir_all(&parent).unwrap();
        fs::write(parent.join("existing.txt"), b"user file").unwrap();

        let validated = install_root_from_picked_dir(&parent.to_string_lossy()).unwrap();
        assert_eq!(validated, parent.join("Codex").to_string_lossy());
        // Validation maps to the Codex child but must not create it — the
        // directory only appears at install time.
        assert!(!parent.join("Codex").exists());

        let _ = fs::remove_dir_all(parent);
    }

    #[test]
    fn validation_does_not_create_the_target_directory() {
        let root = temp_path("codex-manager-validate-no-create").join("Codex");
        let _ = fs::remove_dir_all(root.parent().unwrap());

        let validated = validate_install_root_path(&root.to_string_lossy()).unwrap();
        assert_eq!(validated, root.to_string_lossy());
        // Validating a fresh location must leave the filesystem untouched.
        assert!(!root.exists(), "validation must not create the install root");

        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[cfg(windows)]
    #[test]
    fn rejects_windows_protected_install_root() {
        let Some(program_files) = std::env::var_os("ProgramFiles") else {
            return;
        };
        let root = std::path::PathBuf::from(program_files).join("Codex");

        let err = validate_install_root_path(&root.to_string_lossy()).unwrap_err();
        assert!(
            err.to_string()
                .contains("安装位置不能放在系统目录、管理员目录或磁盘根目录")
        );
    }
}
