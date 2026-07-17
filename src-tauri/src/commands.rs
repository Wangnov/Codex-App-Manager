use std::path::{Path, PathBuf};

use codex_win_engine::InstalledWindowsCodex;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State};
use tauri_plugin_autostart::ManagerExt;
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_updater::UpdaterExt;

use crate::app::atomic_file;
use crate::app::config_health::ConfigHealth;
use crate::app::diagnostics::Diagnostics;
use crate::app::disk::available_space;
use crate::app::logging::redact_url;
use crate::app::mac_update::{
    cancel_macos_download, detect_existing_install_at_path as detect_macos_install_at_path,
    discard_macos_download, install_macos_with_network_and_phase,
    mac_adopt_path as adopt_macos_path, pause_macos_download,
    perform_macos_update_with_network_and_phase,
    plan_macos_update_with_network, retry_macos_ancillary, stage_macos_update_with_network,
    uninstall_macos, InstalledCodex, MacInstallStatus, MacPerformReport, MacStageReport,
    MacUninstallReport, MacUpdateReport, PerformExpectation,
};
use crate::app::op_phase::{OperationPhase, QuitPolicy};
use crate::app::operation_outcome::{AncillaryRetryReport, AncillaryRetryRequest};
use crate::app::oplock::{
    OperationCompletion, OperationError, OperationGuard, OperationKind, OperationManager,
    OperationProgress, OperationSnapshot, OperationToken,
};
use crate::app::paths;
use crate::app::provenance::ProvenanceStore;
use crate::app::settings_store::AppSettings as PersistedAppSettings;
use crate::app::settings_store::{ProxyMode, UpdateSource};
use crate::app::url_guard::{validate_custom_proxy, validate_custom_source};
use crate::app::window_mode::{self, WindowMode, WindowModeReport};
use crate::app::win_update::{
    auto_stage_windows_update_with_install_mode_and_network, cancel_windows_download,
    detect_existing_windows_install_at_path as detect_windows_install_at_path,
    discard_windows_download, pause_windows_download,
    perform_windows_update_with_install_mode_network_and_phase,
    plan_windows_update_with_install_mode_and_network,
    retry_windows_ancillary, stage_windows_update_with_install_mode_and_network,
    uninstall_windows_codex, win_adopt as adopt_windows_install,
    win_adopt_path as adopt_windows_path, win_install_status,
    DownloadProgress as WinDownloadProgress, OperationEvidence, WinAutoStageReport, WinInstallStatus,
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
        "/latest/win-arm64",
        "/latest/win-x64",
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
    match saved.source {
        UpdateSource::Custom => {
            let base = if saved.custom_url.trim().is_empty() {
                state.settings.mirror_base_url.clone()
            } else {
                let normalized = validate_custom_source(&saved.custom_url).map_err(|e| {
                    let host = redact_url(&saved.custom_url);
                    log::warn!("url_guard rejected custom Windows source reason={e} host={host}");
                    AppError::Engine(e.to_string())
                })?;
                normalize_windows_source_base(&normalized)
                    .unwrap_or_else(|| state.settings.mirror_base_url.clone())
            };
            Ok(crate::domain::manifest::MirrorEndpoints::from_base_url(&base))
        }
        UpdateSource::Official => Err(AppError::Engine(
            "Windows official update source is not available yet; choose mirror, auto, or a custom source that serves latest/manifest, latest/checksums, and latest/win.".to_string(),
        )),
        // Windows currently depends on the mirror-style manifest/checksum/MSIX
        // endpoints. `auto` therefore resolves to the known-good mirror until an
        // official source exposes the same contract.
        UpdateSource::Auto | UpdateSource::Mirror => Ok(state.endpoints.clone()),
    }
}

fn normalize_settings_for_target(
    settings: &mut PersistedAppSettings,
    target: &crate::domain::target::Target,
) {
    if matches!(target.os, OperatingSystem::Windows) && settings.source == UpdateSource::Official {
        settings.source = UpdateSource::Auto;
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

fn validated_custom_proxy_for_settings(raw: &str, context: &str) -> Result<String, AppError> {
    validate_custom_proxy(raw).map_err(|e| {
        log::warn!("url_guard rejected {context} proxy reason={e}");
        AppError::Engine(e.to_string())
    })
}

fn mac_network_config_for_settings() -> Result<codex_mac_engine::NetworkConfig, AppError> {
    let saved = PersistedAppSettings::load();
    match saved.proxy_mode {
        ProxyMode::System => Ok(codex_mac_engine::NetworkConfig::system()),
        ProxyMode::Direct => Ok(codex_mac_engine::NetworkConfig::direct()),
        ProxyMode::Custom => {
            let proxy = validated_custom_proxy_for_settings(&saved.custom_proxy_url, "mac update")?;
            Ok(codex_mac_engine::NetworkConfig::custom(proxy))
        }
    }
}

fn win_network_config_for_settings() -> Result<codex_win_engine::NetworkConfig, AppError> {
    let saved = PersistedAppSettings::load();
    match saved.proxy_mode {
        ProxyMode::System => Ok(codex_win_engine::NetworkConfig::system()),
        ProxyMode::Direct => Ok(codex_win_engine::NetworkConfig::direct()),
        ProxyMode::Custom => {
            let proxy =
                validated_custom_proxy_for_settings(&saved.custom_proxy_url, "Windows update")?;
            Ok(codex_win_engine::NetworkConfig::custom(proxy))
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagerUpdateMetadata {
    pub version: String,
    pub current_version: String,
    pub body: Option<String>,
}

fn manager_updater_builder(
    app: &AppHandle,
) -> Result<tauri_plugin_updater::UpdaterBuilder, AppError> {
    let saved = PersistedAppSettings::load();
    let mut builder = app.updater_builder();
    match saved.proxy_mode {
        ProxyMode::System => {}
        ProxyMode::Direct => {
            builder = builder.no_proxy();
        }
        ProxyMode::Custom => {
            let normalized =
                validated_custom_proxy_for_settings(&saved.custom_proxy_url, "manager updater")?;
            let proxy = url::Url::parse(&normalized)
                .map_err(|e| AppError::Engine(format!("invalid proxy URL: {e}")))?;
            builder = builder.proxy(proxy);
        }
    }
    Ok(builder)
}

fn manager_update_matches_confirmation(
    latest_version: &str,
    current_version: &str,
    expected_version: &str,
    expected_current_version: &str,
) -> bool {
    latest_version == expected_version.trim() && current_version == expected_current_version.trim()
}

#[tauri::command]
pub async fn manager_check_update(
    app: AppHandle,
) -> Result<Option<ManagerUpdateMetadata>, CommandError> {
    let updater = manager_updater_builder(&app)?
        .build()
        .map_err(|e| AppError::Engine(format!("build manager updater: {e}")))?;
    let update = updater
        .check()
        .await
        .map_err(|e| AppError::Engine(format!("check manager update: {e}")))?;
    Ok(update.map(|update| ManagerUpdateMetadata {
        version: update.version,
        current_version: update.current_version,
        body: update.body,
    }))
}

#[tauri::command]
pub async fn manager_install_update(
    app: AppHandle,
    expected_version: String,
    expected_current_version: String,
) -> Result<(), CommandError> {
    let updater = manager_updater_builder(&app)?
        .build()
        .map_err(|e| AppError::Engine(format!("build manager updater: {e}")))?;
    let update = updater
        .check()
        .await
        .map_err(|e| AppError::Engine(format!("check manager update before install: {e}")))?
        .ok_or_else(|| AppError::Engine("No manager update is available.".to_string()))?;
    if !manager_update_matches_confirmation(
        &update.version,
        &update.current_version,
        &expected_version,
        &expected_current_version,
    ) {
        return Err(AppError::StaleExpectation(
            "管理器更新内容已变化，请重新检查后再确认。".to_string(),
        )
        .into());
    }
    update
        .download_and_install(|_, _| {}, || {})
        .await
        .map_err(|e| AppError::Engine(format!("install manager update: {e}")))?;
    Ok(())
}

fn windows_domain_settings_for_persisted(state: &ManagerState) -> DomainAppSettings {
    let saved = PersistedAppSettings::load();
    let mut settings = state.settings.clone();
    settings.install_root = saved.install_root;
    settings.disable_codex_self_updates = saved.disable_codex_self_updates;
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

fn mac_existing_install_start_dir() -> PathBuf {
    let system = PathBuf::from("/Applications");
    if system.is_dir() {
        return system;
    }
    if let Some(home) = std::env::var_os("HOME") {
        let user_apps = PathBuf::from(home).join("Applications");
        if user_apps.is_dir() {
            return user_apps;
        }
    }
    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("/"))
}

const MIN_PORTABLE_FREE_SPACE_BYTES: u64 = 1_073_741_824;
const INSTALL_LOCATION_PROBE_PREFIX: &str = ".codex-manager-install-test-";

fn begin_guard(state: &ManagerState, kind: OperationKind) -> Result<OperationGuard, CommandError> {
    state
        .operations
        .begin(kind)
        .map_err(|err| {
            log::warn!(
                "failed to acquire operation guard kind={} error={err}",
                kind.as_str()
            );
            AppError::from(err)
        })
        .map_err(Into::into)
}

struct DetachedGuard {
    completion_tracking: bool,
    operations: OperationManager,
    succeeded: bool,
    token: Option<OperationToken>,
}

impl DetachedGuard {
    fn validate(state: &ManagerState, token: OperationToken) -> Result<Self, CommandError> {
        Self::validate_inner(state, token, false)
    }

    fn validate_tracked(state: &ManagerState, token: OperationToken) -> Result<Self, CommandError> {
        Self::validate_inner(state, token, true)
    }

    fn validate_inner(
        state: &ManagerState,
        token: OperationToken,
        completion_tracking: bool,
    ) -> Result<Self, CommandError> {
        let operations = state.operations.clone();
        operations
            .validate(&token)
            .map_err(destructive_token_error)?;
        Ok(Self {
            completion_tracking,
            operations,
            succeeded: false,
            token: Some(token),
        })
    }

    fn validate_with_phase(
        state: &ManagerState,
        token: OperationToken,
        phase: OperationPhase,
    ) -> Result<Self, CommandError> {
        let operations = state.operations.clone();
        operations
            .validate_with_phase(&token, phase)
            .map_err(destructive_token_error)?;
        Ok(Self {
            completion_tracking: false,
            operations,
            succeeded: false,
            token: Some(token),
        })
    }

    fn mark_succeeded(&mut self) {
        self.succeeded = true;
    }

    fn set_phase(&self, phase: OperationPhase) {
        if let Some(token) = self.token.as_ref() {
            let _ = self.operations.set_phase(token, phase);
        }
    }

    fn token_clone(&self) -> Option<OperationToken> {
        self.token.clone()
    }

    fn operations(&self) -> OperationManager {
        self.operations.clone()
    }
}

/// Progress payload emitted on the download event bus. Includes the operation
/// id so a reloaded frontend can reject late events from a previous op.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DownloadProgressEvent {
    downloaded: u64,
    total: u64,
    source: String,
    operation_id: String,
}

fn emit_op_download_progress(
    app: &AppHandle,
    operations: &OperationManager,
    token: &OperationToken,
    channel: &str,
    downloaded: u64,
    total: u64,
    source: String,
) {
    let progress = OperationProgress {
        downloaded,
        total,
        source: source.clone(),
    };
    let _ = operations.set_progress(token, progress);
    // Bytes in flight ⇒ downloading phase (unless already past it).
    if total > 0 && downloaded < total {
        let _ = operations.set_phase(token, OperationPhase::Downloading);
    }
    let _ = app.emit(
        channel,
        DownloadProgressEvent {
            downloaded,
            total,
            source,
            operation_id: token.0.clone(),
        },
    );
}

impl Drop for DetachedGuard {
    fn drop(&mut self) {
        if let Some(token) = self.token.take() {
            if self.completion_tracking {
                if let Err(error) = self.operations.record_completion(&token, self.succeeded) {
                    log::error!("failed to record terminal operation outcome: {error}");
                }
            }
            let _ = self.operations.end(token);
        }
    }
}

fn destructive_token_error(err: OperationError) -> CommandError {
    match err {
        OperationError::InvalidToken => {
            log::warn!("destructive token validation failed");
            AppError::StaleExpectation("操作令牌无效或已过期，请重新检查后再确认".to_string())
                .into()
        }
        other => {
            log::warn!("destructive token rejected error={other}");
            AppError::from(other).into()
        }
    }
}

fn refresh_config_health(state: &ManagerState) -> ConfigHealth {
    let (_, settings_health) = PersistedAppSettings::load_with_health();
    let (_, provenance_health) = ProvenanceStore::load_with_health();
    let health = ConfigHealth::from_parts(settings_health, provenance_health).with_live_backup_flags();
    let mut slot = state
        .config_health
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    *slot = health.clone();
    health
}

fn config_path(which: &str) -> Result<PathBuf, AppError> {
    match which {
        "settings" => paths::settings_path()
            .ok_or_else(|| AppError::Internal("无法定位 settings.json 数据目录".to_string())),
        "provenance" => paths::provenance_path()
            .ok_or_else(|| AppError::Internal("无法定位 provenance.json 数据目录".to_string())),
        _ => Err(AppError::Internal(
            "配置类型必须是 settings 或 provenance".to_string(),
        )),
    }
}

fn auto_stage_busy_report(enabled: bool, allow_metered: bool) -> WinAutoStageReport {
    WinAutoStageReport {
        enabled,
        allow_metered,
        attempted: false,
        skipped: true,
        reason: "operation-busy".to_string(),
        stage: None,
        capabilities: None,
        notes: vec![
            "Automatic Windows pre-download was skipped because another operation is running."
                .to_string(),
        ],
    }
}

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

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FrontendErrorPayload {
    kind: String,
    message: String,
    stack: Option<String>,
    component_stack: Option<String>,
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
    // Identity-gated (entry exe + AppxManifest Identity == OpenAI.Codex): a
    // directory that merely contains a ChatGPT.exe (e.g. an unpacked ChatGPT
    // Classic) must not be treated as a replaceable Codex install.
    codex_win_engine::detect_portable_install(path).is_some()
}

fn directory_is_empty(path: &Path) -> Result<bool, AppError> {
    let mut entries = std::fs::read_dir(path)
        .map_err(|e| AppError::Internal(format!("读取安装位置失败: {e}")))?;
    Ok(entries.next().is_none())
}

/// Probe the operation the portable installer ultimately needs: create a
/// non-empty child directory and atomically rename it beside the install root.
/// A plain file write inside an existing install only proves file creation; it
/// does not prove the parent grants directory replacement/delete-child rights.
fn probe_install_parent_replace(path: &Path) -> Result<PathBuf, AppError> {
    let requested_parent = path.parent().unwrap_or(path);
    let probe_dir = nearest_existing_dir(requested_parent);
    let probe_id = uuid::Uuid::new_v4();
    let source = probe_dir.join(format!("{INSTALL_LOCATION_PROBE_PREFIX}{probe_id}-source"));
    let destination =
        probe_dir.join(format!("{INSTALL_LOCATION_PROBE_PREFIX}{probe_id}-destination"));

    let probe_result = (|| -> std::io::Result<()> {
        std::fs::create_dir(&source)?;
        std::fs::write(source.join("probe"), b"ok")?;
        codex_win_engine::rename_directory_with_retry(
            "probe install parent directory rename",
            &source,
            &destination,
        )?;
        Ok(())
    })();

    // Validation must remain side-effect free. Clean both names because the
    // probe can fail before or after the rename boundary.
    let source_cleanup = if source.exists() {
        codex_win_engine::remove_directory_all_with_retry(
            "clean install parent source probe",
            &source,
        )
    } else {
        Ok(())
    };
    let destination_cleanup = if destination.exists() {
        codex_win_engine::remove_directory_all_with_retry(
            "clean install parent destination probe",
            &destination,
        )
    } else {
        Ok(())
    };

    let cleanup_result = source_cleanup.and(destination_cleanup);
    match (probe_result, cleanup_result) {
        (Ok(()), Ok(())) => {}
        (Err(probe_err), Ok(())) => {
            return Err(AppError::Internal(format!(
                "安装位置父目录不可写或不支持目录替换: {probe_err}"
            )));
        }
        (Ok(()), Err(cleanup_err)) => {
            return Err(AppError::Internal(format!(
                "安装位置探测目录清理失败: {cleanup_err}"
            )));
        }
        (Err(probe_err), Err(cleanup_err)) => {
            return Err(AppError::Internal(format!(
                "安装位置父目录不可写或不支持目录替换: {probe_err}; 探测目录清理也失败: {cleanup_err}"
            )));
        }
    }
    Ok(probe_dir)
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
    // Probe parent-level directory creation and rename WITHOUT creating the
    // requested target itself. This mirrors the install swap and also gives us
    // the same-volume ancestor for the free-space check.
    let probe_dir = probe_install_parent_replace(&path)?;
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
    let network = mac_network_config_for_settings()?;
    tauri::async_runtime::spawn_blocking(move || {
        plan_macos_update_with_network(simulated_build, &network)
    })
    .await
    .map_err(|e| AppError::Internal(format!("join: {e}")))?
    .map_err(Into::into)
}

/// macOS-only: plan + download + size/EdDSA verify into staging. Non-destructive
/// (no apply/swap). Runs the blocking download off the main thread.
#[tauri::command]
pub async fn mac_stage_update(
    state: State<'_, ManagerState>,
    simulated_build: Option<u64>,
) -> Result<MacStageReport, CommandError> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let _op = begin_guard(&state, OperationKind::Update)?;
    let network = mac_network_config_for_settings()?;
    tauri::async_runtime::spawn_blocking(move || {
        stage_macos_update_with_network(simulated_build, &network)
    })
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
        if let Ok(res) = app
            .path()
            .resolve(rel, tauri::path::BaseDirectory::Resource)
        {
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
    state: State<'_, ManagerState>,
    confirm: bool,
    token: OperationToken,
    expected_from_build: u64,
    expected_to_build: u64,
    expected_path: String,
) -> Result<MacPerformReport, CommandError> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::UnsupportedPlatform.into());
    }
    if !confirm {
        return Err(
            AppError::Internal("拒绝执行：破坏性更新必须带显式 confirm".to_string()).into(),
        );
    }
    let op = DetachedGuard::validate(&state, token)?;
    op.set_phase(OperationPhase::Preparing);
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
    let network = mac_network_config_for_settings()?;
    let ops = op.operations();
    let phase_token = op.token_clone();
    let progress_token = phase_token.clone();
    let progress_ops = ops.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let report = move |p: crate::app::mac_update::DownloadProgress| {
            if let Some(token) = progress_token.as_ref() {
                emit_op_download_progress(
                    &progress_app,
                    &progress_ops,
                    token,
                    "mac://download-progress",
                    p.downloaded,
                    p.total,
                    p.source,
                );
            } else {
                let _ = progress_app.emit("mac://download-progress", p);
            }
        };
        let phase_hook = |phase: OperationPhase| {
            if let Some(token) = phase_token.as_ref() {
                let _ = ops.set_phase(token, phase);
            }
        };
        perform_macos_update_with_network_and_phase(
            binary_delta,
            expected,
            &report,
            &network,
            Some(&phase_hook),
        )
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
    let _op = begin_guard(&state, OperationKind::Adopt)?;
    crate::app::mac_update::mac_adopt().map_err(Into::into)
}

/// macOS-only: let the user pick an existing Codex install and validate it.
/// The bundle may be named Codex.app or (post-rebrand) ChatGPT.app — validation
/// is by CFBundleIdentifier, so ChatGPT Classic is rejected with a clear error.
#[tauri::command]
pub async fn mac_pick_existing_install(
    app: tauri::AppHandle,
    state: State<'_, ManagerState>,
) -> Result<Option<InstalledCodex>, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Macos) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let start_dir = mac_existing_install_start_dir();
    let selected = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("选择 Codex 应用（Codex.app 或 ChatGPT.app）")
            .set_directory(start_dir)
            .blocking_pick_file()
            .map(|path| {
                path.into_path()
                    .map(|p| p.to_string_lossy().into_owned())
                    .map_err(|e| AppError::Internal(format!("读取选择的应用失败: {e}")))
            })
            .transpose()
    })
    .await
    .map_err(|e| AppError::Internal(format!("join: {e}")))??;
    selected
        .as_deref()
        .map(|path| detect_macos_install_at_path(Path::new(path)))
        .transpose()
        .map_err(Into::into)
}

/// macOS-only: adopt the user-selected Codex.app path.
#[tauri::command]
pub fn mac_adopt_path(
    state: State<'_, ManagerState>,
    path: String,
) -> Result<MacInstallStatus, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Macos) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let _op = begin_guard(&state, OperationKind::Adopt)?;
    adopt_macos_path(Path::new(&path)).map_err(Into::into)
}

/// macOS-only: open the installed Codex.app (explicit 〔打开 Codex〕 action).
/// With a persisted theme selection this transparently becomes "launch
/// debuggable + inject" — unless an operation holds the lock, in which case
/// the launch stays plain rather than racing the updater for the process.
#[tauri::command]
pub async fn mac_launch_codex(state: State<'_, ManagerState>) -> Result<(), CommandError> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let settings = PersistedAppSettings::load();
    if settings.codex_theme.is_some() && state.operations.snapshot().is_none() {
        let themed =
            crate::app::codex_theme::launch_with_active_theme(&state.codex_theme, &settings)
                .await?;
        if themed {
            return Ok(());
        }
    }
    crate::app::mac_update::launch_codex().map_err(Into::into)
}

/// macOS-only: fresh-install the latest Codex (full package) into /Applications.
/// Runs the blocking download/verify/install off the main thread.
#[tauri::command]
pub async fn mac_install(
    app: tauri::AppHandle,
    state: State<'_, ManagerState>,
) -> Result<MacInstallStatus, CommandError> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let op = begin_guard(&state, OperationKind::Install)?;
    let token = op.token().clone();
    let ops = state.operations.clone();
    let _ = ops.set_phase(&token, OperationPhase::Preparing);
    let network = mac_network_config_for_settings()?;
    let progress_token = token.clone();
    let progress_ops = ops.clone();
    let phase_token = token.clone();
    let phase_ops = ops.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let report = move |p: crate::app::mac_update::DownloadProgress| {
            emit_op_download_progress(
                &app,
                &progress_ops,
                &progress_token,
                "mac://download-progress",
                p.downloaded,
                p.total,
                p.source,
            );
        };
        let phase_hook = |phase: OperationPhase| {
            let _ = phase_ops.set_phase(&phase_token, phase);
        };
        install_macos_with_network_and_phase(&report, &network, Some(&phase_hook))
    })
    .await
    .map_err(|e| AppError::Internal(format!("join: {e}")))?
    .map_err(Into::into)
}

/// macOS-only: request pausing an active package download.
/// Partial bytes are left in place for the next resume-capable run.
#[tauri::command]
pub fn mac_pause_download(
    state: State<'_, ManagerState>,
    operation_id: String,
) -> Result<bool, CommandError> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::UnsupportedPlatform.into());
    }
    Ok(state
        .operations
        .request_pause(&OperationToken(operation_id), pause_macos_download))
}

/// macOS-only: request cancellation of an active package download.
/// Partial bytes are discarded.
#[tauri::command]
pub fn mac_cancel_download(
    state: State<'_, ManagerState>,
    operation_id: String,
) -> Result<bool, CommandError> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::UnsupportedPlatform.into());
    }
    Ok(state
        .operations
        .request_cancellation(&OperationToken(operation_id), cancel_macos_download))
}

/// macOS-only: discard a PAUSED download. After a pause the curl process is gone
/// but its `.part` is still cached for resume; this drops it when the user
/// cancels from the paused state instead of resuming.
#[tauri::command]
pub fn mac_discard_download() -> Result<(), CommandError> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::UnsupportedPlatform.into());
    }
    discard_macos_download().map_err(Into::into)
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
    let network = win_network_config_for_settings()?;
    tauri::async_runtime::spawn_blocking(move || {
        plan_windows_update_with_install_mode_and_network(
            &endpoints,
            &settings,
            &install_mode,
            &network,
        )
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
    let _op = begin_guard(&state, OperationKind::Update)?;
    let endpoints = windows_endpoints_for_settings(&state)?;
    let settings = windows_domain_settings_for_persisted(&state);
    let install_mode = windows_install_mode_for_settings();
    let network = win_network_config_for_settings()?;
    tauri::async_runtime::spawn_blocking(move || {
        stage_windows_update_with_install_mode_and_network(
            &endpoints,
            &settings,
            &install_mode,
            &|_| {},
            &network,
        )
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
    // After normalize(), empty custom source/proxy is coerced away. Any remaining
    // Custom mode must carry a non-empty, validated URL so disk matches runtime.
    if s.source == UpdateSource::Custom {
        s.custom_url = validate_custom_source(&s.custom_url).map_err(|e| {
            let host = redact_url(&s.custom_url);
            log::warn!("url_guard rejected custom source reason={e} host={host}");
            AppError::Engine(e.to_string())
        })?;
    }
    if s.proxy_mode == ProxyMode::Custom {
        s.custom_proxy_url = validated_custom_proxy_for_settings(&s.custom_proxy_url, "settings")?;
    }
    let previous = PersistedAppSettings::load();
    let _op = begin_guard(&state, OperationKind::SetInstallRoot)?;
    if previous.disable_codex_self_updates != s.disable_codex_self_updates {
        crate::app::codex_self_update::sync_setting(s.disable_codex_self_updates)?;
    }
    s.save()?;
    refresh_config_health(&state);
    log::info!(
        "saved settings source={} windows_install_mode={} proxy_mode={} disable_codex_self_updates={}",
        s.source.as_str(),
        s.windows_install_mode,
        s.proxy_mode.as_str(),
        s.disable_codex_self_updates
    );
    Ok(s)
}

#[tauri::command]
pub fn get_config_health(state: State<'_, ManagerState>) -> ConfigHealth {
    // Always re-read from disk so the UI sees post-restore/reset truth, not a
    // stale snapshot taken at process start.
    refresh_config_health(&state)
}

#[tauri::command]
pub fn restore_config_backup(
    state: State<'_, ManagerState>,
    which: String,
) -> Result<ConfigHealth, CommandError> {
    let _op = begin_guard(&state, OperationKind::SetInstallRoot)?;
    let path = config_path(which.as_str())?;
    let backup = atomic_file::backup_path(&path);
    if !backup.exists() {
        return Err(AppError::Internal(format!("找不到 {which} 的 .bak 备份")).into());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| AppError::Internal(format!("create data dir: {e}")))?;
    }
    let current_tmp = path.with_extension(format!("restore-current-{}", std::process::id()));
    if path.exists() {
        std::fs::rename(&path, &current_tmp)
            .map_err(|e| AppError::Internal(format!("move current config aside: {e}")))?;
    }
    if let Err(err) = std::fs::rename(&backup, &path) {
        if current_tmp.exists() {
            let _ = std::fs::rename(&current_tmp, &path);
        }
        return Err(AppError::Internal(format!("restore config backup: {err}")).into());
    }
    if current_tmp.exists() {
        let _ = std::fs::remove_file(current_tmp);
    }
    log::info!("restored config backup which={which}");
    // Re-read + re-verify; never claim success without a fresh health probe.
    let health = refresh_config_health(&state);
    let status = match which.as_str() {
        "settings" => health.settings_status.as_str(),
        "provenance" => health.provenance_status.as_str(),
        _ => "ok",
    };
    if status == "corrupt" {
        return Err(AppError::Internal(format!(
            "已从 .bak 还原 {which}，但重新读取仍判定为损坏"
        ))
        .into());
    }
    Ok(health)
}

#[tauri::command]
pub fn reset_config(
    state: State<'_, ManagerState>,
    which: String,
) -> Result<ConfigHealth, CommandError> {
    let _op = begin_guard(&state, OperationKind::SetInstallRoot)?;
    match which.as_str() {
        "settings" => {
            let mut settings = PersistedAppSettings::default();
            normalize_settings_for_target(&mut settings, &state.target);
            crate::app::codex_self_update::sync_setting(settings.disable_codex_self_updates)?;
            settings.save()?;
        }
        "provenance" => {
            ProvenanceStore::default().save()?;
        }
        _ => {
            return Err(
                AppError::Internal("配置类型必须是 settings 或 provenance".to_string()).into(),
            )
        }
    }
    log::info!("reset config which={which}");
    let health = refresh_config_health(&state);
    let status = match which.as_str() {
        "settings" => health.settings_status.as_str(),
        "provenance" => health.provenance_status.as_str(),
        _ => "ok",
    };
    if status == "corrupt" {
        return Err(AppError::Internal(format!(
            "已重置 {which}，但重新读取仍判定为损坏"
        ))
        .into());
    }
    Ok(health)
}

/// Retry only failed ancillary steps after a partial install/uninstall.
/// Never re-runs full install or uninstall of the app itself.
///
/// `purge_user_data` is destructive (deletes `~/.codex`): it requires the same
/// explicit confirm + armed uninstall token as a full uninstall, so it cannot
/// be one-clicked from a recovery CTA.
#[tauri::command]
pub fn retry_ancillary(
    state: State<'_, ManagerState>,
    request: AncillaryRetryRequest,
    confirm: Option<bool>,
    token: Option<OperationToken>,
) -> Result<AncillaryRetryReport, CommandError> {
    let actions = &request.actions;
    let path = request.path.as_deref();
    let purge = request.purge_user_data
        && actions
            .iter()
            .any(|a| a == crate::app::operation_outcome::recovery::PURGE_USER_DATA);
    // Hold either a scoped adopt lock or a validated destructive uninstall token
    // for the duration of the retry. Drop ends the token/lock (fields unread on purpose).
    #[allow(dead_code)]
    enum RetryGuard {
        Scoped(OperationGuard),
        Detached(DetachedGuard),
    }
    let _guard: RetryGuard = if purge {
        if confirm != Some(true) {
            return Err(AppError::Internal(
                "清除用户数据需要二次确认（confirm=true）".to_string(),
            )
            .into());
        }
        let token = token.ok_or_else(|| {
            AppError::Internal(
                "清除用户数据需要破坏性令牌（先 arm_destructive uninstall）".to_string(),
            )
        })?;
        let guard =
            DetachedGuard::validate_with_phase(&state, token, OperationPhase::Committing)?;
        RetryGuard::Detached(guard)
    } else {
        RetryGuard::Scoped(begin_guard(&state, OperationKind::Adopt)?)
    };
    match state.target.os {
        OperatingSystem::Macos => {
            retry_macos_ancillary(actions, path, purge).map_err(Into::into)
        }
        OperatingSystem::Windows => {
            let settings = windows_domain_settings_for_persisted(&state);
            retry_windows_ancillary(&settings, actions, path, purge).map_err(Into::into)
        }
        _ => Err(AppError::UnsupportedPlatform.into()),
    }
}

#[tauri::command]
pub fn begin_operation(
    state: State<'_, ManagerState>,
    kind: OperationKind,
) -> Result<OperationToken, CommandError> {
    state
        .operations
        .begin_detached(kind)
        .map_err(|err| {
            log::warn!(
                "begin_operation rejected kind={} error={err}",
                kind.as_str()
            );
            AppError::from(err)
        })
        .map_err(Into::into)
}

#[tauri::command]
pub fn arm_destructive(
    state: State<'_, ManagerState>,
    kind: OperationKind,
) -> Result<OperationToken, CommandError> {
    if !matches!(kind, OperationKind::Update | OperationKind::Uninstall) {
        return Err(
            AppError::Internal("仅 update/uninstall 操作可使用破坏性令牌".to_string()).into(),
        );
    }
    state
        .operations
        .begin_detached(kind)
        .map_err(|err| {
            log::warn!(
                "arm_destructive rejected kind={} error={err}",
                kind.as_str()
            );
            AppError::from(err)
        })
        .map_err(Into::into)
}

#[tauri::command]
pub fn end_operation(
    state: State<'_, ManagerState>,
    token: OperationToken,
) -> Result<(), CommandError> {
    state
        .operations
        .end(token)
        .map_err(AppError::from)
        .map_err(Into::into)
}

/// Current same-process operation lease, if any. Used by the frontend on mount
/// to reattach progress/phase UI after a renderer reload without re-arming work.
#[tauri::command]
pub fn get_operation_snapshot(
    state: State<'_, ManagerState>,
) -> Result<Option<OperationSnapshot>, CommandError> {
    Ok(state.operations.snapshot())
}

/// Token-keyed terminal evidence for a renderer that lost the original invoke
/// promise. `failed-before-commit` and `rolled-back` prove that retrying a fresh
/// install is safe; unresolved committing/finishing failures remain outcome-unknown.
#[tauri::command]
pub fn get_operation_completion(
    state: State<'_, ManagerState>,
    token: OperationToken,
) -> Result<Option<OperationCompletion>, CommandError> {
    Ok(state.operations.completion(&token))
}

/// The user confirmed quitting from the close dialog — flag it and exit so the
/// CloseRequested / ExitRequested guards stop intercepting and let it go.
/// Still refuses when the backend is in a non-interruptible install phase.
#[tauri::command]
pub fn confirm_quit(app: tauri::AppHandle, state: State<'_, ManagerState>) -> Result<(), CommandError> {
    let confirm_close = crate::app::settings_store::AppSettings::load().confirm_close;
    // Decide and arm exit under the SAME operation mutex used by phase changes.
    // If the worker already reached commit this returns Block. Otherwise both app
    // abort latches and force_quit are set before it can cross that boundary, so
    // the final pre-commit checkpoint observes the cancellation.
    let policy = state.operations.prepare_quit(confirm_close, true, || {
        let _ = cancel_macos_download();
        let _ = cancel_windows_download();
        state
            .force_quit
            .store(true, std::sync::atomic::Ordering::SeqCst);
    });
    if let QuitPolicy::Block {
        phase,
        reason_code,
        reason,
        kind,
    } = &policy
    {
        log::warn!(
            "confirm_quit blocked phase={} reason_code={reason_code} kind={:?} reason={reason}",
            phase.as_str(),
            kind
        );
        crate::dispatch_shell_event(
            &app,
            crate::app::shell::ShellEvent::QuitBlocked(policy.clone()),
        );
        return Err(AppError::Busy(reason.clone()).into());
    }
    crate::exit_after_confirm(&app);
    Ok(())
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
    match selected {
        Some(path) => tauri::async_runtime::spawn_blocking(move || {
            install_root_from_picked_dir(&path).map(Some)
        })
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))?
        .map_err(Into::into),
        None => Ok(None),
    }
}

/// Windows-only: let the user pick an existing portable/self-extracted Codex directory.
#[tauri::command]
pub async fn win_pick_existing_install(
    app: tauri::AppHandle,
    state: State<'_, ManagerState>,
) -> Result<Option<InstalledWindowsCodex>, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let start_dir = dialog_start_dir(&PersistedAppSettings::load().install_root);
    let selected = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("选择已安装的 Codex 位置")
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
        .map(|path| detect_windows_install_at_path(Path::new(path)))
        .transpose()
        .map_err(Into::into)
}

/// Windows-only: adopt the user-selected Codex directory.
#[tauri::command]
pub fn win_adopt_path(
    state: State<'_, ManagerState>,
    path: String,
) -> Result<WinInstallStatus, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let _op = begin_guard(&state, OperationKind::Adopt)?;
    let settings = windows_domain_settings_for_persisted(&state);
    adopt_windows_path(&settings, Path::new(&path)).map_err(Into::into)
}

/// Windows-only: persist a validated portable install root.
#[tauri::command]
pub async fn win_set_install_root(
    state: State<'_, ManagerState>,
    path: String,
) -> Result<PersistedAppSettings, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let _op = begin_guard(&state, OperationKind::SetInstallRoot)?;
    let install_root =
        tauri::async_runtime::spawn_blocking(move || validate_install_root_path(&path))
            .await
            .map_err(|e| AppError::Internal(format!("join: {e}")))??;
    let mut settings = PersistedAppSettings::load();
    settings.install_root = install_root;
    settings.normalize();
    settings.save()?;
    let path = &settings.install_root;
    log::info!("set Windows install root path={path}");
    Ok(settings)
}

/// Windows-only: reset the remembered portable install root to the per-user default.
#[tauri::command]
pub async fn win_reset_install_root(
    state: State<'_, ManagerState>,
) -> Result<PersistedAppSettings, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let _op = begin_guard(&state, OperationKind::SetInstallRoot)?;
    let default_install_root = PersistedAppSettings::default().install_root;
    let install_root = tauri::async_runtime::spawn_blocking(move || {
        validate_install_root_path(&default_install_root)
    })
    .await
    .map_err(|e| AppError::Internal(format!("join: {e}")))??;
    let mut settings = PersistedAppSettings::load();
    settings.install_root = install_root;
    settings.normalize();
    settings.save()?;
    let path = &settings.install_root;
    log::info!("reset Windows install root path={path}");
    Ok(settings)
}

/// macOS-only **destructive**: uninstall Codex. Requires explicit `confirm`.
/// `keep_codex_home` defaults true at the UI — `~/.codex` survives unless the
/// user opts out. Runs the blocking work off the main thread.
#[tauri::command]
pub async fn mac_uninstall(
    state: State<'_, ManagerState>,
    confirm: bool,
    token: OperationToken,
    keep_codex_home: bool,
) -> Result<MacUninstallReport, CommandError> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::UnsupportedPlatform.into());
    }
    if !confirm {
        return Err(AppError::Internal("拒绝执行：卸载必须带显式 confirm".to_string()).into());
    }
    let _op = DetachedGuard::validate_with_phase(&state, token, OperationPhase::Committing)?;
    // Uninstall has no resumable cancellation protocol. Treat the whole worker
    // as point-of-no-return so every native/window quit path blocks until the
    // removal and ancillary bookkeeping have settled.
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
    let _op = if enabled {
        match state.operations.begin(OperationKind::Update) {
            Ok(guard) => Some(guard),
            Err(OperationError::BusySameProcess(_) | OperationError::BusyOtherProcess) => {
                return Ok(auto_stage_busy_report(enabled, allow_metered));
            }
            Err(err) => return Err(AppError::from(err).into()),
        }
    } else {
        None
    };
    let endpoints = windows_endpoints_for_settings(&state)?;
    let settings = windows_domain_settings_for_persisted(&state);
    let install_mode = windows_install_mode_for_settings();
    let network = win_network_config_for_settings()?;
    tauri::async_runtime::spawn_blocking(move || {
        auto_stage_windows_update_with_install_mode_and_network(
            &endpoints,
            &settings,
            enabled,
            allow_metered,
            &install_mode,
            &network,
        )
    })
    .await
    .map_err(|e| AppError::Internal(format!("join: {e}")))?
    .map_err(Into::into)
}

/// Windows-only: request pausing an active background/manual download.
/// Partial bytes are left in place for the next resume-capable staging run.
#[tauri::command]
pub fn win_pause_download(
    state: State<'_, ManagerState>,
    operation_id: String,
) -> Result<bool, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    Ok(state
        .operations
        .request_pause(&OperationToken(operation_id), pause_windows_download))
}

/// Windows-only: request cancellation of an active background/manual download.
/// Partial bytes are discarded.
#[tauri::command]
pub fn win_cancel_download(
    state: State<'_, ManagerState>,
    operation_id: String,
) -> Result<bool, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    Ok(state
        .operations
        .request_cancellation(&OperationToken(operation_id), cancel_windows_download))
}

/// Windows-only: discard a PAUSED download. Drops the cached `.part` left for
/// resume when the user cancels from the paused state instead of resuming.
#[tauri::command]
pub fn win_discard_download(state: State<'_, ManagerState>) -> Result<(), CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    discard_windows_download().map_err(Into::into)
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

// ── Codex UI themes ──────────────────────────────────────────────────────────
// CDP-injected theme packages (see crates/codex-theme-engine). Live try-on
// needs a debuggable Codex; the apply path restarts Codex with the loopback
// debug port and writes the native config.toml appearance sections.

/// Refuse theme operations that restart Codex while an install/update holds
/// the operation lock — a themed relaunch racing an updater's quit/swap would
/// interleave two owners of the Codex process lifecycle.
fn ensure_theme_may_restart_codex(state: &ManagerState) -> Result<(), CommandError> {
    if state.operations.snapshot().is_some() {
        return Err(AppError::Engine("有正在进行的操作，请稍后再试".to_string()).into());
    }
    Ok(())
}

#[tauri::command]
pub fn codex_theme_list() -> Vec<codex_theme_engine::theme::ThemeSummary> {
    let settings = PersistedAppSettings::load();
    crate::app::codex_theme::merged_theme_list(&settings)
}

#[tauri::command]
pub async fn codex_theme_status(
    state: State<'_, ManagerState>,
) -> Result<crate::app::codex_theme::ThemeStatusReport, CommandError> {
    let settings = PersistedAppSettings::load();
    Ok(state.codex_theme.status(&settings).await)
}

/// Live try-on against an already-debuggable Codex. Does not persist.
#[tauri::command]
pub async fn codex_theme_try_on(
    state: State<'_, ManagerState>,
    theme_ref: String,
) -> Result<crate::app::codex_theme::ThemeStatusReport, CommandError> {
    let settings = PersistedAppSettings::load();
    state.codex_theme.try_on(&settings, &theme_ref).await?;
    Ok(state.codex_theme.status(&settings).await)
}

/// Try-on that restarts Codex into debug mode first. Persists nothing —
/// the 保留 action on the try-on banner is what makes it stick.
#[tauri::command]
pub async fn codex_theme_try_on_restart(
    state: State<'_, ManagerState>,
    theme_ref: String,
) -> Result<crate::app::codex_theme::ThemeStatusReport, CommandError> {
    ensure_theme_may_restart_codex(&state)?;
    let settings = PersistedAppSettings::load();
    state
        .codex_theme
        .try_on_with_restart(&settings, &theme_ref)
        .await?;
    Ok(state.codex_theme.status(&settings).await)
}

/// Persist the selection (the daemon keeps whatever is currently injected;
/// future launches through the manager apply it automatically).
#[tauri::command]
pub fn codex_theme_keep(theme_ref: String) -> Result<(), CommandError> {
    let mut settings = PersistedAppSettings::load();
    let dir = crate::app::codex_theme::resolve_theme_for_keep(&settings, &theme_ref)?;
    settings.codex_theme = Some(dir);
    settings.save().map_err(Into::into)
}

/// Full apply: quiesce Codex → native appearance sections → relaunch with the
/// debug port → inject → persist the selection.
#[tauri::command]
pub async fn codex_theme_apply(
    state: State<'_, ManagerState>,
    theme_ref: String,
) -> Result<crate::app::codex_theme::ThemeStatusReport, CommandError> {
    ensure_theme_may_restart_codex(&state)?;
    let settings = PersistedAppSettings::load();
    state
        .codex_theme
        .apply_with_restart(&settings, &theme_ref)
        .await?;
    let id = crate::app::codex_theme::resolve_theme_for_keep(&settings, &theme_ref)?;
    let mut updated = PersistedAppSettings::load();
    updated.codex_theme = Some(id);
    updated.save()?;
    Ok(state.codex_theme.status(&updated).await)
}

/// Pick a `.codexskin` archive and install it into the managed theme root.
/// Returns None when the user cancels the picker.
#[tauri::command]
pub async fn codex_theme_import(
    app: tauri::AppHandle,
) -> Result<Option<codex_theme_engine::theme::ThemeSummary>, CommandError> {
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("导入 Codex 主题包")
            .add_filter("Codex Skin", &["codexskin", "zip"])
            .blocking_pick_file()
            .and_then(|path| path.into_path().ok())
    })
    .await
    .map_err(|e| AppError::Internal(format!("join: {e}")))?;
    let Some(archive) = picked else {
        return Ok(None);
    };
    import_codexskin_at(&archive).map(Some)
}

/// Install a `.codexskin` from an explicit path (drag-and-drop delivery).
#[tauri::command]
pub async fn codex_theme_import_path(
    path: String,
) -> Result<codex_theme_engine::theme::ThemeSummary, CommandError> {
    tauri::async_runtime::spawn_blocking(move || import_codexskin_at(Path::new(&path)))
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))?
}

fn import_codexskin_at(
    archive: &Path,
) -> Result<codex_theme_engine::theme::ThemeSummary, CommandError> {
    let themes_root = crate::app::codex_theme::store_dir(&PersistedAppSettings::load())?;
    let summary = codex_theme_engine::import::import_codexskin(archive, &themes_root)
        .map_err(|e| AppError::Engine(e.to_string()))?;
    log::info!(
        "imported codexskin id={} version={:?} from={}",
        summary.id,
        summary.meta.version,
        archive.display()
    );
    Ok(summary)
}

/// Cover preview of a theme as a data URL (None when the package ships no
/// preview). Served through invoke rather than the asset protocol so dev-dir
/// themes — whose location is user-chosen — need no protocol scope.
#[tauri::command]
pub fn codex_theme_preview(theme_ref: String) -> Result<Option<String>, CommandError> {
    let settings = PersistedAppSettings::load();
    Ok(crate::app::codex_theme::preview_data_url(&settings, &theme_ref))
}

/// The online skin catalog (skins.agentsmirror.com, published by
/// awesome-codex-skins CI).
#[tauri::command]
pub async fn codex_theme_catalog(
) -> Result<Vec<crate::app::codex_theme::CatalogSkin>, CommandError> {
    tauri::async_runtime::spawn_blocking(crate::app::codex_theme::fetch_catalog)
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))?
        .map_err(Into::into)
}

/// Catalog cover preview as a data URL (relative path resolved only against
/// the pinned catalog origin).
#[tauri::command]
pub async fn codex_theme_catalog_preview(preview: String) -> Result<String, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::app::codex_theme::catalog_preview_data_url(&preview)
    })
    .await
    .map_err(|e| AppError::Internal(format!("join: {e}")))?
    .map_err(Into::into)
}

/// Download, sha256-verify and install one catalog skin.
#[tauri::command]
pub async fn codex_theme_install_online(
    skin_id: String,
) -> Result<codex_theme_engine::theme::ThemeSummary, CommandError> {
    tauri::async_runtime::spawn_blocking(move || {
        crate::app::codex_theme::install_from_catalog(&skin_id)
    })
    .await
    .map_err(|e| AppError::Internal(format!("join: {e}")))?
    .map_err(Into::into)
}

/// Pick a new skin-store directory and migrate existing skins into it.
/// Returns None when the user cancels the picker. Conflicting ids stay at
/// the destination (never clobbered); a live daemon directive pointing into
/// the old store is re-based onto the new one.
#[tauri::command]
pub async fn codex_theme_pick_store_dir(
    app: tauri::AppHandle,
    state: State<'_, ManagerState>,
) -> Result<Option<crate::app::codex_theme::StoreMigrationReport>, CommandError> {
    let settings = PersistedAppSettings::load();
    let current = crate::app::codex_theme::store_dir(&settings)?;
    let start_dir = current.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app.dialog()
            .file()
            .set_title("选择主题存储目录")
            .set_directory(start_dir)
            .blocking_pick_folder()
            .and_then(|path| path.into_path().ok())
    })
    .await
    .map_err(|e| AppError::Internal(format!("join: {e}")))?;
    let Some(new_dir) = picked else {
        return Ok(None);
    };
    if new_dir == current {
        return Ok(None);
    }
    if new_dir.starts_with(&current) || current.starts_with(&new_dir) {
        return Err(AppError::Engine("新目录不能嵌套在当前主题目录内".to_string()).into());
    }

    let report = {
        let current = current.clone();
        let new_dir = new_dir.clone();
        tauri::async_runtime::spawn_blocking(move || {
            crate::app::codex_theme::migrate_store(&current, &new_dir)
        })
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))??
    };

    let mut updated = PersistedAppSettings::load();
    updated.codex_theme_store_dir = Some(new_dir.to_string_lossy().into_owned());
    updated.save()?;
    state.codex_theme.rebase_directive(&current, &new_dir).await;
    log::info!(
        "skin store migrated from={} to={} moved={} skipped={}",
        current.display(),
        new_dir.display(),
        report.moved.len(),
        report.skipped.len()
    );
    Ok(Some(report))
}

/// Reveal the current skin store in Finder/Explorer.
#[tauri::command]
pub fn codex_theme_open_store() -> Result<(), CommandError> {
    let dir = crate::app::codex_theme::store_dir(&PersistedAppSettings::load())?;
    let _ = std::fs::create_dir_all(&dir);
    open_dir_platform(&dir).map_err(|e| AppError::Internal(format!("打开主题目录失败: {e}")).into())
}

/// Turn the theme off. `full` additionally restores the original config.toml
/// appearance sections (restarting Codex plainly if it was running).
#[tauri::command]
pub async fn codex_theme_off(
    state: State<'_, ManagerState>,
    full: bool,
) -> Result<crate::app::codex_theme::ThemeStatusReport, CommandError> {
    if full {
        ensure_theme_may_restart_codex(&state)?;
        state.codex_theme.off_full().await?;
    } else {
        state.codex_theme.turn_off_live().await?;
    }
    let mut settings = PersistedAppSettings::load();
    settings.codex_theme = None;
    settings.save()?;
    Ok(state.codex_theme.status(&settings).await)
}

/// Switch the main window between the compact dashboard and the expanded
/// workbench. `width`/`height` are the frontend-remembered expanded size
/// (logical px); the echoed report carries what was actually applied after
/// clamping, so the frontend persists reality rather than the request.
#[tauri::command]
pub fn set_window_mode(
    app: tauri::AppHandle,
    mode: WindowMode,
    width: Option<f64>,
    height: Option<f64>,
) -> Result<WindowModeReport, CommandError> {
    window_mode::apply_window_mode(&app, mode, width, height).map_err(Into::into)
}

/// Open an external http(s) URL in the user's default browser. Restricted to
/// http(s) so it can't be coerced into launching arbitrary local handlers.
#[tauri::command]
pub fn open_url(url: String) -> Result<(), CommandError> {
    validate_external_http_url(&url)?;
    let host = redact_url(&url);
    log::info!("open external URL host={host}");
    open_external_url(&url).map_err(|e| AppError::Internal(format!("打开链接失败: {e}")).into())
}

#[tauri::command]
pub fn get_diagnostics(app: tauri::AppHandle, state: State<'_, ManagerState>) -> Diagnostics {
    log::info!("collecting diagnostics");
    crate::app::diagnostics::collect_diagnostics(&app, &state)
}

#[tauri::command]
pub fn open_logs_dir(app: tauri::AppHandle) -> Result<(), CommandError> {
    let dir = crate::app::logging::logs_dir(&app)
        .ok_or_else(|| AppError::Internal("无法定位日志目录".to_string()))?;
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.display();
    log::info!("open logs dir path={path}");
    open_dir_platform(&dir).map_err(|e| AppError::Internal(format!("打开日志目录失败: {e}")).into())
}

#[cfg(target_os = "macos")]
fn open_dir_platform(dir: &Path) -> Result<(), String> {
    std::process::Command::new("/usr/bin/open")
        .arg(dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[cfg(target_os = "windows")]
fn open_dir_platform(dir: &Path) -> Result<(), String> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let operation: Vec<u16> = OsStr::new("open").encode_wide().chain([0]).collect();
    let target: Vec<u16> = dir.as_os_str().encode_wide().chain([0]).collect();
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
fn open_dir_platform(dir: &Path) -> Result<(), String> {
    std::process::Command::new("xdg-open")
        .arg(dir)
        .spawn()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_codex_home() -> Result<(), CommandError> {
    let dir = paths::codex_home_dir()
        .ok_or_else(|| AppError::Internal("无法定位 Codex 数据目录".to_string()))?;
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.display();
    log::info!("open Codex home path={path}");
    open_dir_platform(&dir)
        .map_err(|e| AppError::Internal(format!("打开 Codex 数据目录失败: {e}")).into())
}

#[tauri::command]
pub fn log_frontend_error(payload: FrontendErrorPayload) {
    let kind = single_line(&payload.kind);
    let message = single_line(&payload.message);
    let stack = payload
        .stack
        .as_deref()
        .map(single_line)
        .unwrap_or_else(|| "none".to_string());
    let component_stack = payload
        .component_stack
        .as_deref()
        .map(single_line)
        .unwrap_or_else(|| "none".to_string());
    log::error!(
        "frontend error kind={kind} message={message} stack={stack} component_stack={component_stack}"
    );
}

fn single_line(value: &str) -> String {
    value.replace('\r', "\\r").replace('\n', "\\n")
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
    std::process::Command::new("/usr/bin/open")
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
            assert!(
                validate_external_http_url(url).is_err(),
                "{url} should be rejected"
            );
        }
    }
}

/// Windows-only: classify the installed Codex (managed / external / none).
///
/// Async + `spawn_blocking` so AppX / PowerShell status probes cannot freeze the
/// WebView when enterprise policy or a hung AppX service stalls detection.
#[tauri::command]
pub async fn win_status(state: State<'_, ManagerState>) -> Result<WinInstallStatus, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let settings = windows_domain_settings_for_persisted(&state);
    tauri::async_runtime::spawn_blocking(move || win_install_status(&settings))
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))
        .map_err(Into::into)
}

/// Windows-only: adopt the detected external install (after explicit consent).
///
/// Async + `spawn_blocking` so filesystem / provenance work stays off the UI thread.
#[tauri::command]
pub async fn win_adopt(state: State<'_, ManagerState>) -> Result<WinInstallStatus, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let _op = begin_guard(&state, OperationKind::Adopt)?;
    let settings = windows_domain_settings_for_persisted(&state);
    tauri::async_runtime::spawn_blocking(move || adopt_windows_install(&settings))
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))?
        .map_err(Into::into)
}

/// Windows-only: open the installed Codex.
///
/// Async + `spawn_blocking` so PowerShell AUMID activation (or portable spawn)
/// cannot freeze the WebView while the OS is cold-starting Codex. Errors still
/// surface to the UI after the blocking work finishes.
#[tauri::command]
pub async fn win_launch_codex(state: State<'_, ManagerState>) -> Result<(), CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let settings = windows_domain_settings_for_persisted(&state);
    tauri::async_runtime::spawn_blocking(move || crate::app::win_update::launch_codex(&settings))
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))?
        .map_err(Into::into)
}

/// Windows-only: guarded execution. Requires explicit confirmation, stages and
/// verifies the MSIX first, then attempts Add-AppxPackage without elevation or
/// policy changes. Reports portable fallback need transparently.
#[tauri::command]
pub async fn win_perform_update(
    app: tauri::AppHandle,
    state: State<'_, ManagerState>,
    confirm: bool,
    token: OperationToken,
    install_root: Option<String>,
    expected: Option<WinPerformExpectation>,
) -> Result<WinPerformReport, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    if !confirm {
        return Err(
            AppError::Internal("拒绝执行：Windows 更新必须带显式 confirm".to_string()).into(),
        );
    }
    let mut op = DetachedGuard::validate_tracked(&state, token)?;
    op.set_phase(OperationPhase::Preparing);
    let endpoints = windows_endpoints_for_settings(&state)?;
    let mut settings = windows_domain_settings_for_persisted(&state);
    // Validate (but don't yet persist) an explicitly chosen install root: it
    // only becomes the remembered default after the install actually succeeds,
    // so a failed or cancelled attempt never changes the user's saved location.
    let pending_install_root = match install_root {
        Some(raw) => {
            let validated =
                tauri::async_runtime::spawn_blocking(move || validate_install_root_path(&raw))
                    .await
                    .map_err(|e| AppError::Internal(format!("join: {e}")))??;
            settings.install_root = validated.clone();
            Some(validated)
        }
        None => None,
    };
    let install_mode = windows_install_mode_for_settings();
    let network = win_network_config_for_settings()?;
    let progress_app = app.clone();
    let ops = op.operations();
    let phase_token = op.token_clone();
    let progress_token = phase_token.clone();
    let progress_ops = ops.clone();
    let report = tauri::async_runtime::spawn_blocking(move || {
        let report = move |p: WinDownloadProgress| {
            if let Some(token) = progress_token.as_ref() {
                emit_op_download_progress(
                    &progress_app,
                    &progress_ops,
                    token,
                    "win://download-progress",
                    p.downloaded,
                    p.total,
                    p.source,
                );
            } else {
                let _ = progress_app.emit("win://download-progress", p);
            }
        };
        let phase_hook = |phase: OperationPhase| {
            if let Some(token) = phase_token.as_ref() {
                let _ = ops.set_phase(token, phase);
            }
        };
        let evidence_hook = |evidence: OperationEvidence| {
            if let Some(token) = phase_token.as_ref() {
                let result = match evidence {
                    OperationEvidence::MutationStarted => ops.mark_mutation_started(token),
                    OperationEvidence::MutationRolledBack => {
                        ops.mark_mutation_rolled_back(token)
                    }
                    OperationEvidence::OutcomeAmbiguous => ops.mark_outcome_ambiguous(token),
                };
                if let Err(error) = result {
                    log::error!("failed to record Windows operation evidence: {error}");
                }
            }
        };
        perform_windows_update_with_install_mode_network_and_phase(
            &endpoints,
            &settings,
            confirm,
            &install_mode,
            expected,
            &report,
            &network,
            Some(&phase_hook),
            Some(&evidence_hook),
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
    if report.success {
        // The staged MSIX was consumed by the install — drop the cache. A failed
        // or cancelled perform leaves it so the next run (or a resume) reuses the
        // partial/full artifact instead of re-downloading. `stage`/`auto_stage`
        // never clear it: they're pre-downloads whose whole point is to be reused.
        // Best-effort: the stale sweep reclaims a leftover, so a cleanup failure
        // must not turn a successful install into an error.
        let _ = crate::app::staging::clear_download_cache();
    }
    op.mark_succeeded();
    Ok(report)
}

/// Windows-only: guarded uninstall. Only removes installs recorded as managed
/// by this app. User data is preserved unless `purge_user_data` is true.
#[tauri::command]
pub async fn win_uninstall(
    state: State<'_, ManagerState>,
    confirm: bool,
    token: OperationToken,
    purge_user_data: bool,
) -> Result<WinUninstallReport, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    if !confirm {
        return Err(
            AppError::Internal("拒绝执行：Windows 卸载必须带显式 confirm".to_string()).into(),
        );
    }
    let _op = DetachedGuard::validate_with_phase(&state, token, OperationPhase::Committing)?;
    // Remove-AppxPackage / portable tree removal must not be killed mid-call.
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
        install_root_from_picked_dir, manager_update_matches_confirmation,
        normalize_windows_source_base, validate_install_root_path, INSTALL_LOCATION_PROBE_PREFIX,
        validated_custom_proxy_for_settings,
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
            normalize_windows_source_base("https://example.test/latest/win-arm64").as_deref(),
            Some("https://example.test")
        );
        assert_eq!(
            normalize_windows_source_base("https://example.test/latest/win-x64").as_deref(),
            Some("https://example.test")
        );
        assert_eq!(
            normalize_windows_source_base("https://example.test/custom").as_deref(),
            Some("https://example.test/custom")
        );
        assert!(normalize_windows_source_base("   ").is_none());
    }

    #[test]
    fn settings_custom_proxy_requires_a_url() {
        let err = validated_custom_proxy_for_settings("  ", "settings").unwrap_err();
        assert!(err.to_string().contains("代理不能为空"));
        assert_eq!(
            validated_custom_proxy_for_settings("socks5h://127.0.0.1:1080", "settings").unwrap(),
            "socks5h://127.0.0.1:1080"
        );
    }

    #[test]
    fn manager_self_update_confirmation_must_match_latest_check() {
        assert!(manager_update_matches_confirmation(
            "0.2.1", "0.2.0", "0.2.1", "0.2.0"
        ));
        assert!(!manager_update_matches_confirmation(
            "0.2.2", "0.2.0", "0.2.1", "0.2.0"
        ));
        assert!(!manager_update_matches_confirmation(
            "0.2.1", "0.2.1", "0.2.1", "0.2.0"
        ));
    }

    #[test]
    fn rejects_non_empty_non_codex_install_root() {
        let root = temp_path("codex-manager-non-empty-root");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("notes.txt"), b"not codex").unwrap();

        let err = validate_install_root_path(&root.to_string_lossy()).unwrap_err();
        assert!(err
            .to_string()
            .contains("安装位置必须是空文件夹，或已有的 Codex 免安装版目录"));

        let _ = fs::remove_dir_all(root);
    }

    fn write_portable_manifest(root: &std::path::Path, identity_name: &str) {
        fs::write(
            root.join("AppxManifest.xml"),
            format!(
                r#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Identity Name="{identity_name}" Publisher="CN=OpenAI OpCo, LLC" Version="26.707.3748.0" ProcessorArchitecture="x64" />
</Package>"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn accepts_existing_codex_portable_install_root() {
        let root = temp_path("codex-manager-existing-portable-root");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("Codex.exe"), b"codex").unwrap();
        write_portable_manifest(&root, "OpenAI.Codex");

        let validated = validate_install_root_path(&root.to_string_lossy()).unwrap();
        assert_eq!(validated, root.to_string_lossy());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_portable_root_with_foreign_identity() {
        // An unpacked non-Codex payload (e.g. ChatGPT Classic) also carries a
        // root-level ChatGPT.exe + AppxManifest.xml. Its identity fails the
        // gate, so the non-empty directory must NOT be treated as replaceable.
        let root = temp_path("codex-manager-foreign-portable-root");
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("ChatGPT.exe"), b"classic").unwrap();
        write_portable_manifest(&root, "OpenAI.ChatGPT");

        let err = validate_install_root_path(&root.to_string_lossy()).unwrap_err();
        assert!(err
            .to_string()
            .contains("安装位置必须是空文件夹，或已有的 Codex 免安装版目录"));

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
        assert!(
            !root.exists(),
            "validation must not create the install root"
        );

        let _ = fs::remove_dir_all(root.parent().unwrap());
    }

    #[test]
    fn validation_removes_parent_replace_probe() {
        let parent = temp_path("codex-manager-validate-probe-cleanup");
        let root = parent.join("Codex");
        let _ = fs::remove_dir_all(&parent);
        fs::create_dir_all(&parent).unwrap();

        validate_install_root_path(&root.to_string_lossy()).unwrap();

        assert!(!fs::read_dir(&parent).unwrap().any(|entry| {
            entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(INSTALL_LOCATION_PROBE_PREFIX)
        }));
        assert!(!root.exists());

        let _ = fs::remove_dir_all(parent);
    }

    #[cfg(windows)]
    #[test]
    fn rejects_windows_protected_install_root() {
        let Some(program_files) = std::env::var_os("ProgramFiles") else {
            return;
        };
        let root = std::path::PathBuf::from(program_files).join("Codex");

        let err = validate_install_root_path(&root.to_string_lossy()).unwrap_err();
        assert!(err
            .to_string()
            .contains("安装位置不能放在系统目录、管理员目录或磁盘根目录"));
    }
}
