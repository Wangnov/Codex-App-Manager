use tauri::{Manager, State};

use crate::app::health_service::HealthService;
use crate::app::mac_update::{
    install_macos, perform_macos_update, plan_macos_update, stage_macos_update, uninstall_macos,
    MacInstallStatus, MacPerformReport, MacStageReport, MacUninstallReport, MacUpdateReport,
    PerformExpectation,
};
use crate::app::settings_store::AppSettings as PersistedAppSettings;
use crate::app::snapshot::ManagerSnapshot;
use crate::app::update_check::PayloadUpdateCheck;
use crate::app::win_update::{
    auto_stage_windows_update, cancel_windows_download, perform_windows_update,
    plan_windows_update, stage_windows_update, uninstall_windows_codex,
    win_adopt as adopt_windows_install, win_install_status, WinAutoStageReport, WinInstallStatus,
    WinPerformReport, WinStageReport, WinUninstallReport, WinUpdateReport,
};
use crate::domain::health::HealthReport;
use crate::domain::operations::{OperationKind, OperationPlan};
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
    let saved = PersistedAppSettings::load();
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

#[tauri::command]
pub fn get_app_snapshot(state: State<'_, ManagerState>) -> Result<ManagerSnapshot, CommandError> {
    Ok(state.snapshot())
}

#[tauri::command]
pub fn plan_install(state: State<'_, ManagerState>) -> Result<OperationPlan, CommandError> {
    Ok(state.planner.plan(
        OperationKind::Install,
        &state.target,
        &state.settings,
        &state.endpoints,
    ))
}

#[tauri::command]
pub fn plan_uninstall(state: State<'_, ManagerState>) -> Result<OperationPlan, CommandError> {
    Ok(state.planner.plan(
        OperationKind::Uninstall,
        &state.target,
        &state.settings,
        &state.endpoints,
    ))
}

#[tauri::command]
pub fn check_payload_updates(
    state: State<'_, ManagerState>,
) -> Result<PayloadUpdateCheck, CommandError> {
    Ok(PayloadUpdateCheck::pending(&state.endpoints))
}

#[tauri::command]
pub fn run_health_check(state: State<'_, ManagerState>) -> Result<HealthReport, CommandError> {
    Ok(HealthService::run(
        &state.target,
        &state.settings,
        &state.endpoints,
    ))
}

/// macOS-only: detect the installed Codex build, read the Sparkle appcast, and
/// return an update plan (delta vs full). Read-only — performs no install.
#[tauri::command]
pub fn mac_plan_update(
    state: State<'_, ManagerState>,
    simulated_build: Option<u64>,
) -> Result<MacUpdateReport, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Macos) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    plan_macos_update(simulated_build).map_err(Into::into)
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
    tauri::async_runtime::spawn_blocking(move || perform_macos_update(binary_delta, expected))
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

/// macOS-only: fresh-install the latest Codex (full package) into /Applications.
/// Runs the blocking download/verify/install off the main thread.
#[tauri::command]
pub async fn mac_install() -> Result<MacInstallStatus, CommandError> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::UnsupportedPlatform.into());
    }
    tauri::async_runtime::spawn_blocking(install_macos)
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))?
        .map_err(Into::into)
}

/// Windows-only: detect installed Codex, read mirror manifest/checksums, probe
/// sideload capabilities, and return the preferred update path. Read-only.
#[tauri::command]
pub fn win_plan_update(state: State<'_, ManagerState>) -> Result<WinUpdateReport, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let endpoints = windows_endpoints_for_settings(&state)?;
    plan_windows_update(&endpoints, &state.settings).map_err(Into::into)
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
    let settings = state.settings.clone();
    tauri::async_runtime::spawn_blocking(move || stage_windows_update(&endpoints, &settings))
        .await
        .map_err(|e| AppError::Internal(format!("join: {e}")))?
        .map_err(Into::into)
}

/// Read persisted app settings (update source + general).
#[tauri::command]
pub fn get_settings() -> Result<PersistedAppSettings, CommandError> {
    Ok(PersistedAppSettings::load())
}

/// Persist app settings. `signed_only` is forced on regardless of input.
#[tauri::command]
pub fn set_settings(settings: PersistedAppSettings) -> Result<PersistedAppSettings, CommandError> {
    let mut s = settings;
    s.signed_only = true;
    s.save()?;
    Ok(s)
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
    let settings = state.settings.clone();
    tauri::async_runtime::spawn_blocking(move || {
        auto_stage_windows_update(&endpoints, &settings, enabled, allow_metered)
    })
    .await
    .map_err(|e| AppError::Internal(format!("join: {e}")))?
    .map_err(Into::into)
}

/// Windows-only: request cancellation of an active background/manual download.
/// Partial bytes are left in place for the next resume-capable staging run.
#[tauri::command]
pub fn win_cancel_download(state: State<'_, ManagerState>) -> Result<bool, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    Ok(cancel_windows_download())
}

/// Windows-only: classify the installed Codex (managed / external / none).
#[tauri::command]
pub fn win_status(state: State<'_, ManagerState>) -> Result<WinInstallStatus, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    Ok(win_install_status(&state.settings))
}

/// Windows-only: adopt the detected external install (after explicit consent).
#[tauri::command]
pub fn win_adopt(state: State<'_, ManagerState>) -> Result<WinInstallStatus, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    adopt_windows_install(&state.settings).map_err(Into::into)
}

/// Windows-only: guarded execution. Requires explicit confirmation, stages and
/// verifies the MSIX first, then attempts Add-AppxPackage without elevation or
/// policy changes. Reports portable fallback need transparently.
#[tauri::command]
pub async fn win_perform_update(
    state: State<'_, ManagerState>,
    confirm: bool,
) -> Result<WinPerformReport, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let endpoints = windows_endpoints_for_settings(&state)?;
    let settings = state.settings.clone();
    tauri::async_runtime::spawn_blocking(move || {
        perform_windows_update(&endpoints, &settings, confirm)
    })
    .await
    .map_err(|e| AppError::Internal(format!("join: {e}")))?
    .map_err(Into::into)
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
    let settings = state.settings.clone();
    tauri::async_runtime::spawn_blocking(move || {
        uninstall_windows_codex(&settings, confirm, purge_user_data)
    })
    .await
    .map_err(|e| AppError::Internal(format!("join: {e}")))?
    .map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::normalize_windows_source_base;

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
}
