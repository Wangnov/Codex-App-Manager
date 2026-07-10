//! Windows update planning + staging service.
//!
//! Mirrors the macOS command shape while keeping the Windows-specific logic in
//! `codex-win-engine`:
//!   - `plan_windows_update`  — read-only capability + manifest/checksum plan.
//!   - `stage_windows_update` — download MSIX + SHA256 + Authenticode + identity
//!     verification into staging. Non-destructive; it does not install yet.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};

use codex_win_engine::{
    cancel_active_download, close_codex_gracefully_for_root, close_msix_codex_processes,
    detect_installed_codex,
    detect_portable_install, download_to_with_progress_bounded_with_network,
    fetch_text_with_network, find_msix_sha256, install_msix_sideload, install_portable_from_msix,
    limits::MAX_PACKAGE_BYTES, parse_manifest, pause_active_download, plan_update,
    precheck_msix_dependencies, probe_capabilities, purge_codex_user_data, read_msix_identity,
    remove_msix_package, sha256_file, uninstall_portable, validate_codex_identity,
    verify_msix_health, verify_openai_authenticode, version_key, AuthenticodeReport,
    CapabilityState, InstalledWindowsCodex, MsixHealthReport, MsixIdentity, MsixRemoveReport,
    MsixSideloadReport, NetworkConfig, PortableInstallReport, PortableUninstallReport,
    WinCapabilityReport, WinInstallRoute, WindowsRelease, WindowsUpdatePlan,
};

use crate::app::provenance::ProvenanceStore;
use crate::app::staging;
use crate::domain::manifest::MirrorEndpoints;
use crate::domain::settings::AppSettings;
use crate::errors::AppError;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WinUpdateReport {
    pub manifest_url: String,
    pub checksums_url: String,
    pub package_url: String,
    pub release: WindowsRelease,
    pub installed: Option<InstalledWindowsCodex>,
    pub capabilities: WinCapabilityReport,
    pub plan: WindowsUpdatePlan,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WinStageReport {
    pub up_to_date: bool,
    pub route: String,
    pub latest_version: String,
    pub package_moniker: String,
    pub download_size: u64,
    pub staged_path: Option<String>,
    pub sha256: String,
    pub hash_verified: bool,
    pub authenticode: Option<AuthenticodeReport>,
    pub identity: Option<MsixIdentity>,
    pub identity_verified: bool,
    pub install_ready: bool,
    pub portable_fallback_ready: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WinAutoStageReport {
    pub enabled: bool,
    pub allow_metered: bool,
    pub attempted: bool,
    pub skipped: bool,
    /// "disabled" | "up-to-date" | "metered-network" | "metered-unknown" | "staged"
    pub reason: String,
    pub stage: Option<WinStageReport>,
    pub capabilities: Option<WinCapabilityReport>,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WinPerformReport {
    pub success: bool,
    pub action: WinPerformAction,
    pub message: String,
    pub stage: WinStageReport,
    pub sideload: Option<MsixSideloadReport>,
    pub portable: Option<PortableInstallReport>,
    pub msix_health: Option<MsixHealthReport>,
    pub installed: Option<InstalledWindowsCodex>,
    pub fallback_available: bool,
    pub fallback_attempted: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum WinPerformAction {
    None,
    MsixSideload,
    PortableFallback,
    PortableFallbackAfterMsixFailure,
    PortableFallbackAfterMsixUnhealthy,
    PortableFallbackMissingFramework,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WinPerformExpectation {
    pub current_version: Option<String>,
    pub latest_version: String,
    pub package_moniker: String,
    pub route: String,
}

impl WinPerformAction {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::None => "none",
            Self::MsixSideload => "msix-sideload",
            Self::PortableFallback => "portable-fallback",
            Self::PortableFallbackAfterMsixFailure => "portable-fallback-after-msix-failure",
            Self::PortableFallbackAfterMsixUnhealthy => "portable-fallback-after-msix-unhealthy",
            Self::PortableFallbackMissingFramework => "portable-fallback-missing-framework",
        }
    }
}

impl std::fmt::Display for WinPerformAction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WinUninstallReport {
    pub success: bool,
    pub action: String,
    pub message: String,
    pub installed_before: Option<InstalledWindowsCodex>,
    pub msix: Option<MsixRemoveReport>,
    pub portable: Option<PortableUninstallReport>,
    pub purged_user_data: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WinInstallStatus {
    pub installed: Option<InstalledWindowsCodex>,
    /// "managed" | "external" | "none"
    pub status: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: u64,
    /// Host the bytes are coming from, e.g. `codexapp.agentsmirror.com`.
    pub source: String,
}

fn engine_err(err: impl ToString) -> AppError {
    AppError::Engine(err.to_string())
}

fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or("")
        .to_string()
}

fn no_progress(_p: DownloadProgress) {}

fn portable_fallback_ready(_endpoints: &MirrorEndpoints) -> bool {
    true
}

fn msix_stem(file_name: &str) -> Option<&str> {
    let base = file_name.rsplit('/').find(|segment| !segment.is_empty())?;
    let suffix_start = base.len().checked_sub(5)?;
    let suffix = base.get(suffix_start..)?;
    if suffix.eq_ignore_ascii_case(".msix") {
        base.get(..suffix_start)
    } else {
        None
    }
}

fn package_file_name_from_url(package_url: &str) -> Result<String, AppError> {
    let parsed = url::Url::parse(package_url)
        .map_err(|e| AppError::Engine(format!("invalid Windows package URL: {e}")))?;
    parsed
        .path()
        .rsplit('/')
        .find(|segment| !segment.is_empty())
        .map(|segment| segment.to_string())
        .ok_or_else(|| AppError::Engine("Windows package URL has no file name".to_string()))
}

fn bind_manifest_checksums(
    release: &WindowsRelease,
    checksums_text: &str,
    package_url: &str,
) -> Result<String, AppError> {
    let package_file = package_file_name_from_url(package_url)?;
    if let Some(url_moniker) = msix_stem(&package_file) {
        if !url_moniker.eq_ignore_ascii_case(&release.package_moniker) {
            let err = AppError::Engine(format!(
                "Windows manifest package moniker {} does not match URL artifact {}",
                release.package_moniker, package_file
            ));
            log::error!("Windows manifest checksums binding mismatch error={err}");
            return Err(err);
        }
    } else {
        log::debug!(
            "Windows package URL has no direct MSIX artifact name; relying on manifest/checksums binding package_file={package_file}"
        );
    }
    if let Some(identity) = release.package_identity.as_deref() {
        if identity != codex_win_engine::OPENAI_PACKAGE_IDENTITY {
            let err = AppError::Engine(format!(
                "Windows manifest package identity {identity} does not match {}",
                codex_win_engine::OPENAI_PACKAGE_IDENTITY
            ));
            log::error!("Windows manifest checksums binding mismatch error={err}");
            return Err(err);
        }
    }
    find_msix_sha256(checksums_text, &release.package_moniker).map_err(|err| {
        log::error!("Windows manifest checksums binding mismatch error={err}");
        engine_err(err)
    })
}

fn read_windows_release(
    endpoints: &MirrorEndpoints,
    network: &NetworkConfig,
) -> Result<(WindowsRelease, String, String), AppError> {
    let manifest_text =
        fetch_text_with_network(&endpoints.manifest_url, network).map_err(engine_err)?;
    let checksums_text =
        fetch_text_with_network(&endpoints.checksums_url, network).map_err(engine_err)?;
    let release = parse_manifest(&manifest_text).map_err(engine_err)?;
    let package_url = endpoints
        .windows_msix_url_for_arch(release.download_architecture.as_deref())
        .to_string();
    let sha256 = bind_manifest_checksums(&release, &checksums_text, &package_url)?;
    Ok((release, sha256, package_url))
}

fn route_label(plan: &WindowsUpdatePlan) -> String {
    match plan.route {
        codex_win_engine::WinInstallRoute::MsixSideload => "msix-sideload",
        codex_win_engine::WinInstallRoute::PortableFallback => "portable-fallback",
    }
    .to_string()
}

fn validate_perform_expectation(
    expected: &WinPerformExpectation,
    previous_installed: Option<&InstalledWindowsCodex>,
    stage: &WinStageReport,
) -> Result<(), AppError> {
    let actual_current = previous_installed.map(|installed| installed.version.as_str());
    if actual_current != expected.current_version.as_deref() {
        return Err(AppError::StaleExpectation(format!(
            "Windows Codex changed before install (expected current {:?}, found {:?}); please re-check and confirm again.",
            expected.current_version, actual_current
        )));
    }
    if stage.latest_version != expected.latest_version {
        return Err(AppError::StaleExpectation(format!(
            "Windows update target changed from {} to {}; please re-check and confirm again.",
            expected.latest_version, stage.latest_version
        )));
    }
    if stage.package_moniker != expected.package_moniker {
        return Err(AppError::StaleExpectation(format!(
            "Windows package changed from {} to {}; please re-check and confirm again.",
            expected.package_moniker, stage.package_moniker
        )));
    }
    if stage.route != expected.route {
        return Err(AppError::StaleExpectation(format!(
            "Windows install route changed from {} to {}; please re-check and confirm again.",
            expected.route, stage.route
        )));
    }
    Ok(())
}

fn close_existing_codex_before_portable_fallback(
    settings: &AppSettings,
    previous_installed: Option<&InstalledWindowsCodex>,
) -> Result<(), AppError> {
    log::info!("Windows portable fallback close existing source=portable-fallback");
    if let Some(installed) = detect_installed_codex(PathBuf::from(&settings.install_root).as_path())
    {
        if installed.source == "msix" {
            close_codex_gracefully_for_root(30, PathBuf::from(&installed.path).as_path())
                .map_err(engine_err)?;
        }
    }
    if let Some(previous) = previous_installed {
        if previous.source == "portable" {
            close_codex_gracefully_for_root(30, PathBuf::from(&previous.path).as_path())
                .map_err(engine_err)?;
        }
    }
    Ok(())
}

/// Detect the installed Codex, preferring a manager-managed PORTABLE build over
/// a still-present (possibly stale) MSIX package.
///
/// `detect_installed_codex` is MSIX-first, which is correct for a clean machine.
/// But after a portable fallback an older MSIX can linger — e.g. sideload was
/// blocked by policy and the package couldn't be removed — and it would shadow
/// the portable build we just installed and recorded, leaving status, planning
/// and uninstall all resolving to the stale package (shown as external, planned
/// against the old version, and impossible to uninstall). When a managed portable
/// build is present it wins; otherwise fall back to normal MSIX-first detection.
fn detect_managed_codex(
    settings: &AppSettings,
    store: &ProvenanceStore,
) -> Option<InstalledWindowsCodex> {
    let root = PathBuf::from(&settings.install_root);
    if let Some(portable) = detect_portable_install(root.as_path()) {
        if store.is_managed(&portable.path) {
            return Some(portable);
        }
    }
    for record in store.managed.iter().rev() {
        if let Some(portable) = detect_portable_install(PathBuf::from(&record.path).as_path()) {
            if store.is_managed(&portable.path) {
                return Some(portable);
            }
        }
    }
    detect_installed_codex(root.as_path())
}

pub fn plan_windows_update(
    endpoints: &MirrorEndpoints,
    settings: &AppSettings,
) -> Result<WinUpdateReport, AppError> {
    plan_windows_update_with_install_mode(endpoints, settings, "msix")
}

pub fn plan_windows_update_with_install_mode(
    endpoints: &MirrorEndpoints,
    settings: &AppSettings,
    install_mode: &str,
) -> Result<WinUpdateReport, AppError> {
    plan_windows_update_with_install_mode_and_network(
        endpoints,
        settings,
        install_mode,
        &NetworkConfig::system(),
    )
}

pub fn plan_windows_update_with_install_mode_and_network(
    endpoints: &MirrorEndpoints,
    settings: &AppSettings,
    install_mode: &str,
    network: &NetworkConfig,
) -> Result<WinUpdateReport, AppError> {
    log::info!("Windows plan start install_mode={install_mode}");
    let (release, sha256, package_url) = read_windows_release(endpoints, network)?;
    let installed = detect_managed_codex(settings, &ProvenanceStore::load());
    let capabilities = probe_capabilities();
    let mut plan = plan_update(
        &release,
        &sha256,
        &package_url,
        &installed,
        &capabilities,
        portable_fallback_ready(endpoints),
    );
    if install_mode == "portable" {
        log::info!("Windows user selected portable; skipping MSIX sideload");
        plan.route = WinInstallRoute::PortableFallback;
        plan.warnings.push(
            "User selected the portable Windows install mode; MSIX sideload will be skipped."
                .to_string(),
        );
    }

    let report = WinUpdateReport {
        manifest_url: endpoints.manifest_url.clone(),
        checksums_url: endpoints.checksums_url.clone(),
        package_url,
        release,
        installed,
        capabilities,
        plan,
    };
    let route = route_label(&report.plan);
    let recommendation = match report.capabilities.recommendation {
        codex_win_engine::SideloadRecommendation::MsixPreferred => "msix-preferred",
        codex_win_engine::SideloadRecommendation::PortableFallback => "portable-fallback",
    };
    log::info!("Windows plan complete route={route} capabilities={recommendation}");
    Ok(report)
}

pub fn stage_windows_update(
    endpoints: &MirrorEndpoints,
    settings: &AppSettings,
) -> Result<WinStageReport, AppError> {
    stage_windows_update_with_install_mode(endpoints, settings, "msix", &no_progress)
}

pub fn stage_windows_update_with_install_mode(
    endpoints: &MirrorEndpoints,
    settings: &AppSettings,
    install_mode: &str,
    progress: &dyn Fn(DownloadProgress),
) -> Result<WinStageReport, AppError> {
    stage_windows_update_with_install_mode_and_network(
        endpoints,
        settings,
        install_mode,
        progress,
        &NetworkConfig::system(),
    )
}

pub fn stage_windows_update_with_install_mode_and_network(
    endpoints: &MirrorEndpoints,
    settings: &AppSettings,
    install_mode: &str,
    progress: &dyn Fn(DownloadProgress),
    network: &NetworkConfig,
) -> Result<WinStageReport, AppError> {
    log::info!("Windows stage start install_mode={install_mode}");
    // Own a guard so EVERY stage caller — `perform` (nested, harmless), background
    // `auto_stage`, and the standalone `win_stage_update` command — resets the
    // latch on exit. Without it, a cancelled background/standalone stage would
    // leave the latch set and make the next user perform abort at its first check.
    // (Nesting under perform's guard can't lose a reachable cancel: once the
    // download completes the UI is in the "finishing" state with cancel disabled,
    // so no cancel lands during stage's post-download verify.)
    let _abort_guard = WinAbortGuard;
    let report = plan_windows_update_with_install_mode_and_network(
        endpoints,
        settings,
        install_mode,
        network,
    )?;
    // The plan above did the manifest/checksums fetch (the Windows "正在准备"
    // phase). Honor a cancel here — before the up-to-date early-return and the
    // download below; once curl runs, its own cancel flag takes over.
    check_win_update_abort()?;
    let route = route_label(&report.plan);
    if report.plan.up_to_date {
        log::info!(
            "Windows stage complete route={route} verified=false portable_fallback_ready={}",
            report.plan.portable_fallback_ready
        );
        return Ok(WinStageReport {
            up_to_date: true,
            route,
            latest_version: report.plan.latest_version,
            package_moniker: report.plan.package_moniker,
            download_size: 0,
            staged_path: None,
            sha256: report.plan.sha256,
            hash_verified: false,
            authenticode: None,
            identity: None,
            identity_verified: false,
            install_ready: false,
            portable_fallback_ready: report.plan.portable_fallback_ready,
            notes: vec!["Installed Windows Codex is already current.".to_string()],
        });
    }

    let stage_result = (|| -> Result<WinStageReport, AppError> {
        // Downloads into the PERSISTENT cache so a paused `.part` survives for the
        // next resume instead of dying with a per-run staging dir. (A preparing-
        // phase cancel was already checked right after the plan above; the
        // transfer itself is interruptible via the download loop's cancel flag.)
        let dest = staging::download_cache_path(
            &report.package_url,
            &format!("{}.msix", report.release.package_moniker),
        )?;
        let expected_size = report.release.content_length.unwrap_or(0);
        if expected_size > MAX_PACKAGE_BYTES {
            return Err(AppError::Engine(format!(
                "MSIX content length {expected_size} exceeds {MAX_PACKAGE_BYTES} byte limit"
            )));
        }
        let expected_sha = report.plan.sha256.clone();
        let source = host_of(&report.package_url);

        let cached_ok = dest.exists()
            && sha256_file(&dest)
                .map(|actual| actual.eq_ignore_ascii_case(&expected_sha))
                .unwrap_or(false);
        if !cached_ok {
            if dest.exists() {
                let _ = std::fs::remove_file(&dest);
            }
            download_to_with_progress_bounded_with_network(
                &report.package_url,
                &dest,
                MAX_PACKAGE_BYTES,
                &|downloaded| {
                    progress(DownloadProgress {
                        downloaded,
                        total: expected_size,
                        source: source.clone(),
                    });
                },
                network,
            )
            .map_err(engine_err)?;
        }

        // A fully-cached MSIX is hash-verified above WITHOUT firing a progress
        // event, so the UI is still in "正在准备" (cancel enabled) during that
        // hash — unlike the download path, which fires progress and flips the UI
        // to the cancel-disabled "finishing" state. Honor a cancel that landed
        // during the cache hash here, before we commit to the artifact: otherwise
        // the stage guard (nested under perform) would clear the latch on success
        // and perform's later checkpoint would never see it.
        check_win_update_abort()?;

        let actual_size = std::fs::metadata(&dest)
            .map_err(|e| AppError::Engine(format!("read staged MSIX metadata: {e}")))?
            .len();
        if expected_size > 0 && actual_size != expected_size {
            return Err(AppError::Engine(format!(
                "MSIX size mismatch: {actual_size} != {expected_size}"
            )));
        }
        if actual_size > MAX_PACKAGE_BYTES {
            return Err(AppError::Engine(format!(
                "MSIX size {actual_size} exceeds {MAX_PACKAGE_BYTES} byte limit"
            )));
        }
        progress(DownloadProgress {
            downloaded: actual_size,
            total: if expected_size > 0 {
                expected_size
            } else {
                actual_size
            },
            source,
        });

        let actual_sha = sha256_file(&dest).map_err(engine_err)?;
        if !actual_sha.eq_ignore_ascii_case(&expected_sha) {
            return Err(AppError::Engine(format!(
                "MSIX sha256 mismatch: {actual_sha} != {expected_sha}"
            )));
        }

        let authenticode = verify_openai_authenticode(&dest).map_err(engine_err)?;
        if !authenticode.is_valid_openai() {
            let err = AppError::Engine(format!(
                "MSIX Authenticode verification failed: status={}, subject={}",
                authenticode.status, authenticode.subject
            ));
            log::error!("Windows stage failed error={err}");
            return Err(err);
        }

        let identity = read_msix_identity(&dest).map_err(engine_err)?;
        validate_codex_identity(
            &identity,
            &report.release.package_version,
            report.release.architecture.as_deref(),
        )
        .map_err(engine_err)?;
        if let Some(expected_identity) = report.release.package_identity.as_deref() {
            if identity.name != expected_identity {
                let err = AppError::Engine(format!(
                    "MSIX identity {} does not match manifest package identity {}",
                    identity.name, expected_identity
                ));
                log::error!("Windows stage failed error={err}");
                return Err(err);
            }
        }

        let mut notes = report.plan.warnings.clone();
        notes.push(
            "MSIX is staged and verified; install execution will sideload first and fall back transparently to the portable path if sideloading fails."
                .to_string(),
        );

        Ok(WinStageReport {
            up_to_date: false,
            route,
            latest_version: report.plan.latest_version,
            package_moniker: report.plan.package_moniker,
            download_size: actual_size,
            staged_path: Some(dest.to_string_lossy().into_owned()),
            sha256: actual_sha,
            hash_verified: true,
            authenticode: Some(authenticode),
            identity: Some(identity),
            identity_verified: true,
            install_ready: true,
            portable_fallback_ready: report.plan.portable_fallback_ready,
            notes,
        })
    })();
    match stage_result {
        Ok(report) => {
            let route = &report.route;
            let verified = report.hash_verified && report.identity_verified;
            let portable_fallback_ready = report.portable_fallback_ready;
            log::info!(
                "Windows stage complete route={route} verified={verified} portable_fallback_ready={portable_fallback_ready}"
            );
            Ok(report)
        }
        Err(err) => {
            log::error!("Windows stage failed error={err}");
            Err(err)
        }
    }
}

pub fn auto_stage_windows_update(
    endpoints: &MirrorEndpoints,
    settings: &AppSettings,
    enabled: bool,
    allow_metered: bool,
) -> Result<WinAutoStageReport, AppError> {
    auto_stage_windows_update_with_install_mode(endpoints, settings, enabled, allow_metered, "msix")
}

pub fn auto_stage_windows_update_with_install_mode(
    endpoints: &MirrorEndpoints,
    settings: &AppSettings,
    enabled: bool,
    allow_metered: bool,
    install_mode: &str,
) -> Result<WinAutoStageReport, AppError> {
    auto_stage_windows_update_with_install_mode_and_network(
        endpoints,
        settings,
        enabled,
        allow_metered,
        install_mode,
        &NetworkConfig::system(),
    )
}

pub fn auto_stage_windows_update_with_install_mode_and_network(
    endpoints: &MirrorEndpoints,
    settings: &AppSettings,
    enabled: bool,
    allow_metered: bool,
    install_mode: &str,
    network: &NetworkConfig,
) -> Result<WinAutoStageReport, AppError> {
    log::info!("Windows auto-stage decision enabled={enabled} allow_metered={allow_metered}");
    if !enabled {
        return Ok(WinAutoStageReport {
            enabled,
            allow_metered,
            attempted: false,
            skipped: true,
            reason: "disabled".to_string(),
            stage: None,
            capabilities: None,
            notes: vec!["Automatic Windows pre-download is disabled.".to_string()],
        });
    }

    let report = plan_windows_update_with_install_mode_and_network(
        endpoints,
        settings,
        install_mode,
        network,
    )?;
    let capabilities = report.capabilities.clone();
    if report.plan.up_to_date {
        return Ok(WinAutoStageReport {
            enabled,
            allow_metered,
            attempted: false,
            skipped: true,
            reason: "up-to-date".to_string(),
            stage: None,
            capabilities: Some(capabilities),
            notes: vec![
                "Windows Codex is already current; no background download needed.".to_string(),
            ],
        });
    }

    if !allow_metered {
        match report.capabilities.metered_network.state {
            CapabilityState::Available => {}
            CapabilityState::Unavailable => {
                let metered_state = report.capabilities.metered_network.state.as_str();
                log::warn!(
                    "Windows auto-stage skipped metered network metered_state={metered_state}"
                );
                return Ok(WinAutoStageReport {
                    enabled,
                    allow_metered,
                    attempted: false,
                    skipped: true,
                    reason: "metered-network".to_string(),
                    stage: None,
                    capabilities: Some(capabilities),
                    notes: vec![
                        "Automatic Windows pre-download was skipped because the current network is metered."
                            .to_string(),
                    ],
                });
            }
            CapabilityState::Unknown => {
                let metered_state = report.capabilities.metered_network.state.as_str();
                log::warn!(
                    "Windows auto-stage skipped metered network metered_state={metered_state}"
                );
                return Ok(WinAutoStageReport {
                    enabled,
                    allow_metered,
                    attempted: false,
                    skipped: true,
                    reason: "metered-unknown".to_string(),
                    stage: None,
                    capabilities: Some(capabilities),
                    notes: vec![
                        "Automatic Windows pre-download was skipped because metered-network status could not be determined."
                            .to_string(),
                    ],
                });
            }
        }
    }

    let stage = stage_windows_update_with_install_mode_and_network(
        endpoints,
        settings,
        install_mode,
        &no_progress,
        network,
    )?;
    let notes = if stage.install_ready {
        vec!["Windows package is staged and ready for user-confirmed installation.".to_string()]
    } else {
        stage.notes.clone()
    };

    Ok(WinAutoStageReport {
        enabled,
        allow_metered,
        attempted: true,
        skipped: false,
        reason: "staged".to_string(),
        stage: Some(stage),
        capabilities: Some(capabilities),
        notes,
    })
}

/// Preparing-phase abort latch (mirrors the macOS one). Covers the gap before
/// the first byte — manifest/checksums fetch, planning — that the curl-level
/// cancel flag can't reach. Reset on op end via `WinAbortGuard`, not at entry.
static WIN_UPDATE_ABORT: AtomicBool = AtomicBool::new(false);

fn clear_win_update_abort() {
    WIN_UPDATE_ABORT.store(false, Ordering::SeqCst);
}

/// Resets the latch when the owning operation ends — on every path. Clearing on
/// DROP (not at entry) keeps the cancel race-free: a cancel landing between the
/// UI showing its button and the op reaching its first checkpoint isn't wiped, so
/// the checkpoint observes it; the next op still starts clean. The cancel command
/// doesn't hold the op lock, so this startup window is real. Owned by both
/// `perform` and `stage` (so background `auto_stage` and the standalone
/// `win_stage_update` can't leak a set latch into the next op). The perform→stage
/// nesting is harmless: clears are idempotent, and the only window the inner clear
/// could touch (stage's post-download verify) has the UI cancel already disabled.
struct WinAbortGuard;

impl Drop for WinAbortGuard {
    fn drop(&mut self) {
        clear_win_update_abort();
    }
}

/// Bail out of the Windows preparing phase on a user cancel. Surfaces the same
/// "download cancelled" marker the curl-cancel path uses so the UI treats it as
/// a cancel uniformly.
fn check_win_update_abort() -> Result<(), AppError> {
    if WIN_UPDATE_ABORT.load(Ordering::SeqCst) {
        Err(AppError::Engine("download cancelled".to_string()))
    } else {
        Ok(())
    }
}

pub fn cancel_windows_download() -> bool {
    // Latch the preparing-phase abort too, so a cancel pressed before the first
    // byte (mid manifest-fetch) is honored at the next checkpoint. Report
    // actionable unconditionally — during preparing the latch IS the cancel.
    WIN_UPDATE_ABORT.store(true, Ordering::SeqCst);
    let requested = cancel_active_download();
    log::info!("Windows cancel download requested={requested}");
    true
}

pub fn pause_windows_download() -> bool {
    // Pause is only offered once bytes flow (UI disables it during preparing),
    // so it stays a pure download-loop operation — keep the `.part`.
    let requested = pause_active_download();
    log::info!("Windows pause download requested={requested}");
    requested
}

/// Paused-state cancel: the download already stopped and its `.part` is on disk.
/// Clear the cache so "继续" can't resume, and drop the abort latch. Surfaces a
/// removal failure rather than silently reporting a cancel that left the partial
/// behind (which a later run would resume).
pub fn discard_windows_download() -> Result<(), AppError> {
    clear_win_update_abort();
    staging::clear_download_cache()
        .map_err(|e| AppError::Engine(format!("清理下载缓存失败: {e}")))?;
    log::info!("Windows discard download cache");
    Ok(())
}

pub fn perform_windows_update(
    endpoints: &MirrorEndpoints,
    settings: &AppSettings,
    confirm: bool,
) -> Result<WinPerformReport, AppError> {
    perform_windows_update_with_install_mode(
        endpoints,
        settings,
        confirm,
        "msix",
        None,
        &no_progress,
    )
}

pub fn perform_windows_update_with_install_mode(
    endpoints: &MirrorEndpoints,
    settings: &AppSettings,
    confirm: bool,
    install_mode: &str,
    expected: Option<WinPerformExpectation>,
    progress: &dyn Fn(DownloadProgress),
) -> Result<WinPerformReport, AppError> {
    perform_windows_update_with_install_mode_and_network(
        endpoints,
        settings,
        confirm,
        install_mode,
        expected,
        progress,
        &NetworkConfig::system(),
    )
}

pub fn perform_windows_update_with_install_mode_and_network(
    endpoints: &MirrorEndpoints,
    settings: &AppSettings,
    confirm: bool,
    install_mode: &str,
    expected: Option<WinPerformExpectation>,
    progress: &dyn Fn(DownloadProgress),
    network: &NetworkConfig,
) -> Result<WinPerformReport, AppError> {
    log::info!("Windows perform start install_mode={install_mode}");
    // Reset the latch when THIS perform ends (not at stage entry) so a cancel
    // racing the op's startup isn't wiped, and `auto_stage` never clears it. See
    // WinAbortGuard.
    let _abort_guard = WinAbortGuard;
    if !confirm {
        return Err(AppError::Internal(
            "explicit confirmation is required before installing Windows Codex".to_string(),
        ));
    }

    let stage = stage_windows_update_with_install_mode_and_network(
        endpoints,
        settings,
        install_mode,
        progress,
        network,
    )?;
    // Staging can take long enough for Codex to self-update, be uninstalled, or
    // move between the user's confirmation and our destructive work. Re-detect
    // after staging and use this fresh snapshot for both consent validation and
    // every close/provenance/install-root decision below.
    let store = ProvenanceStore::load();
    let current_installed = detect_managed_codex(settings, &store)
        .or_else(|| detect_installed_codex(PathBuf::from(&settings.install_root).as_path()));
    if let Some(expected) = &expected {
        validate_perform_expectation(expected, current_installed.as_ref(), &stage)?;
    }
    if stage.up_to_date {
        return Ok(WinPerformReport {
            success: true,
            action: WinPerformAction::None,
            message: "Windows Codex is already current.".to_string(),
            installed: win_install_status(settings).installed,
            sideload: None,
            portable: None,
            msix_health: None,
            fallback_available: stage.portable_fallback_ready,
            fallback_attempted: false,
            notes: stage.notes.clone(),
            stage,
        });
    }

    // Point of no return. Honor a cancel one last time BEFORE closing Codex or
    // sideloading — closes the gap after staging where a fully-cached MSIX skips
    // the download loop (so its cancel flag never arms) yet still reaches here.
    check_win_update_abort()?;

    if stage.route == "portable-fallback" {
        log::warn!("Windows route changed to portable fallback from_route=msix-sideload to_route=portable-fallback");
        close_existing_codex_before_portable_fallback(settings, current_installed.as_ref())?;
        return install_portable_after_stage(settings, stage, None, None, current_installed);
    }

    let staged_path = stage
        .staged_path
        .as_ref()
        .ok_or_else(|| AppError::Engine("staged MSIX path missing".to_string()))?
        .clone();

    // PRE-check the staged MSIX's declared framework dependencies BEFORE touching
    // the running install or attempting the sideload. On a stripped / China /
    // Store-disabled Windows, `Add-AppxPackage` cannot auto-acquire a missing
    // framework (VCLibs / WindowsAppRuntime / UI.Xaml / NET.Native), so the
    // sideload is doomed — it either errors or registers a package that won't
    // launch. When a required framework is positively missing we route straight
    // to the portable build instead of burning a failed attempt. The probe is
    // conservative: if the manifest can't be read or the check can't run it
    // returns `checked = false` and we proceed to the sideload as before, where
    // the post-install health check + transparent fallback remain the backstop.
    let precheck = precheck_msix_dependencies(PathBuf::from(&staged_path).as_path());
    if precheck.should_route_portable() {
        log::warn!("Windows route changed to portable fallback from_route=msix-sideload to_route=portable-fallback");
        // We're switching to portable, but the running build must be stopped first.
        close_existing_codex_before_portable_fallback(settings, current_installed.as_ref())?;
        let mut stage = stage;
        stage.notes.push(format!(
            "Skipped MSIX sideload before attempting it: {}. Routed to the portable build, which carries its own runtime and does not need these framework packages.",
            precheck.reason
        ));
        let mut report =
            install_portable_after_stage(settings, stage, None, None, current_installed)?;
        report.action = WinPerformAction::PortableFallbackMissingFramework;
        return Ok(report);
    }

    if let Some(installed) = detect_installed_codex(PathBuf::from(&settings.install_root).as_path())
    {
        if installed.source == "msix" {
            close_codex_gracefully_for_root(30, PathBuf::from(&installed.path).as_path())
                .map_err(engine_err)?;
        }
    }
    // A managed portable build (possibly under a previous install root) is not
    // stopped by the MSIX sideload below, so close it first — otherwise it keeps
    // running after we switch the user over to the MSIX package. `current_installed`
    // comes from the provenance-aware detect_managed_codex above.
    if let Some(previous) = &current_installed {
        if previous.source == "portable" {
            close_codex_gracefully_for_root(30, PathBuf::from(&previous.path).as_path())
                .map_err(engine_err)?;
        }
    }
    let sideload =
        install_msix_sideload(PathBuf::from(&staged_path).as_path()).map_err(engine_err)?;

    if sideload.success {
        // Add-AppxPackage returning success only means the cmdlet didn't throw.
        // Verify the package is actually runnable before committing to it — on a
        // stripped Windows it can register yet fail to launch. If it's unhealthy,
        // first install the portable fallback so the user is never left without
        // a runnable build, then clean up the bad MSIX best-effort.
        let health = verify_msix_health();
        if !health.healthy {
            log::warn!("Windows route changed to portable fallback from_route=msix-sideload to_route=portable-fallback");
            // Activation probe may have started Codex (or left a half-started
            // process). Close it before portable install and Remove-AppxPackage
            // so package files unlock cleanly. Prefer the sideload install path
            // when known; always also sweep via the registered package location.
            if let Some(installed) = sideload.installed.as_ref() {
                if let Err(err) =
                    close_codex_gracefully_for_root(20, PathBuf::from(&installed.path).as_path())
                {
                    log::warn!(
                        "close MSIX Codex after unhealthy health check (install path) error={err}"
                    );
                }
            }
            if let Err(err) = close_msix_codex_processes(20) {
                log::warn!(
                    "close MSIX Codex after unhealthy health check (package sweep) error={err}"
                );
            }
            let mut report = install_portable_after_stage(
                settings,
                stage,
                Some(sideload),
                Some(health),
                current_installed,
            )?;
            match remove_msix_package() {
                Ok(remove) if remove.success => {
                    report.notes.push(
                        "Unhealthy MSIX package was removed after portable fallback succeeded."
                            .to_string(),
                    );
                    report.notes.extend(remove.notes);
                }
                Ok(remove) => {
                    report.notes.push(format!(
                        "Portable fallback succeeded, but removing the unhealthy MSIX package failed: {}",
                        remove.message
                    ));
                    report.notes.extend(remove.notes);
                }
                Err(err) => {
                    report.notes.push(format!(
                        "Portable fallback succeeded, but removing the unhealthy MSIX package could not run: {err}"
                    ));
                }
            }
            return Ok(report);
        }

        let installed = sideload
            .installed
            .clone()
            .or_else(|| win_install_status(settings).installed);
        if let Some(installed) = &installed {
            let mut store = ProvenanceStore::load();
            if let Some(previous) = &current_installed {
                store.remove(&previous.path);
            }
            store.record(
                installed.path.clone(),
                version_key(&installed.version),
                "manager-installed-msix",
            );
            store.save()?;
        }

        let report = WinPerformReport {
            success: true,
            action: WinPerformAction::MsixSideload,
            message: sideload.message.clone(),
            installed,
            sideload: Some(sideload),
            portable: None,
            msix_health: Some(health),
            fallback_available: stage.portable_fallback_ready,
            fallback_attempted: false,
            notes: stage.notes.clone(),
            stage,
        };
        let action = report.action.as_str();
        let installed_version = report
            .installed
            .as_ref()
            .map(|installed| installed.version.as_str())
            .unwrap_or("none");
        log::info!("Windows perform success action={action} installed_version={installed_version}");
        return Ok(report);
    }

    install_portable_after_stage(settings, stage, Some(sideload), None, current_installed)
}

fn install_portable_after_stage(
    settings: &AppSettings,
    stage: WinStageReport,
    sideload: Option<MsixSideloadReport>,
    health: Option<MsixHealthReport>,
    previous_installed: Option<InstalledWindowsCodex>,
) -> Result<WinPerformReport, AppError> {
    let staged_path = stage
        .staged_path
        .as_ref()
        .ok_or_else(|| AppError::Engine("staged MSIX path missing".to_string()))?;
    let install_root = previous_installed
        .as_ref()
        .filter(|installed| installed.source == "portable")
        .map(|installed| installed.path.clone())
        .unwrap_or_else(|| settings.install_root.clone());
    let portable = install_portable_from_msix(
        PathBuf::from(staged_path).as_path(),
        PathBuf::from(&install_root).as_path(),
        true,
    )
    .map_err(engine_err)?;

    // Detect the PORTABLE install we just wrote — not detect_installed_codex,
    // which prefers MSIX and would return a still-present older MSIX package
    // (e.g. when sideload was blocked by policy), recording the wrong target so
    // the user keeps seeing the same update and the portable build goes unmanaged.
    let installed = detect_portable_install(PathBuf::from(&install_root).as_path());
    if let Some(installed) = &installed {
        let mut store = ProvenanceStore::load();
        if let Some(previous) = &previous_installed {
            store.remove(&previous.path);
        }
        store.record(
            installed.path.clone(),
            version_key(&installed.version),
            "manager-installed-portable",
        );
        store.save()?;
    }

    let mut notes = stage.notes.clone();
    let msix_unhealthy = health.as_ref().is_some_and(|h| !h.healthy);
    if let Some(health) = &health {
        if !health.healthy {
            notes.push(format!(
                "MSIX installed but failed its post-install health check ({}); switched to the portable build.",
                health.reason
            ));
        }
    } else if let Some(sideload) = &sideload {
        notes.push(format!(
            "MSIX sideload failed without elevation or policy changes: {}",
            sideload.message
        ));
    }
    notes.extend(portable.notes.clone());

    let action = if msix_unhealthy {
        WinPerformAction::PortableFallbackAfterMsixUnhealthy
    } else if sideload.is_some() {
        WinPerformAction::PortableFallbackAfterMsixFailure
    } else {
        WinPerformAction::PortableFallback
    };

    let report = WinPerformReport {
        success: true,
        action,
        message: portable.message.clone(),
        installed,
        sideload,
        portable: Some(portable),
        msix_health: health,
        fallback_available: true,
        fallback_attempted: true,
        notes,
        stage,
    };
    let action = report.action.as_str();
    let installed_version = report
        .installed
        .as_ref()
        .map(|installed| installed.version.as_str())
        .unwrap_or("none");
    log::info!("Windows perform success action={action} installed_version={installed_version}");
    Ok(report)
}

pub fn win_install_status(settings: &AppSettings) -> WinInstallStatus {
    let store = ProvenanceStore::load();
    let installed = detect_managed_codex(settings, &store);
    let status = match &installed {
        None => "none",
        // Build-aware (matching the macOS path): a self-updated or path-reused
        // install no longer matches its record and reads as "external" so the
        // user is prompted to re-adopt rather than silently treated as managed.
        Some(codex) if store.is_managed_build(&codex.path, version_key(&codex.version)) => {
            "managed"
        }
        Some(_) => "external",
    }
    .to_string();
    WinInstallStatus { installed, status }
}

pub fn win_adopt(settings: &AppSettings) -> Result<WinInstallStatus, AppError> {
    let installed = detect_installed_codex(PathBuf::from(&settings.install_root).as_path())
        .ok_or_else(|| AppError::Internal("no Windows Codex detected to adopt".to_string()))?;
    let path = &installed.path;
    log::info!("Windows adopt external install path={path}");
    let mut store = ProvenanceStore::load();
    store.record(
        installed.path.clone(),
        version_key(&installed.version),
        "adopted-external",
    );
    store.save()?;
    Ok(win_install_status(settings))
}

pub fn detect_existing_windows_install_at_path(
    path: &Path,
) -> Result<InstalledWindowsCodex, AppError> {
    if !path.exists() {
        return Err(AppError::Internal(
            "所选位置不存在，请选择已安装的 Codex 文件夹".to_string(),
        ));
    }
    if !path.is_dir() {
        return Err(AppError::Internal(
            "所选位置必须是 Codex 安装文件夹".to_string(),
        ));
    }
    let installed = detect_portable_install(path).ok_or_else(|| {
        AppError::Internal(
            "未在所选位置识别到 Codex 安装：需要 ChatGPT.exe / Codex.exe 入口，且 AppxManifest.xml \
             声明的包身份必须是 OpenAI.Codex（其他产品如 ChatGPT Classic 不受本工具管理）"
                .to_string(),
        )
    })?;
    if installed.version.trim().is_empty() || installed.version == "0.0.0.0" {
        return Err(AppError::Internal(
            "无法读取所选 Codex 的版本信息，请确认这是完整的 Codex 安装目录".to_string(),
        ));
    }
    Ok(installed)
}

pub fn win_adopt_path(settings: &AppSettings, path: &Path) -> Result<WinInstallStatus, AppError> {
    let installed = detect_existing_windows_install_at_path(path)?;
    let install_path = &installed.path;
    log::info!("Windows adopt selected install path={install_path}");
    let mut store = ProvenanceStore::load();
    store.record(
        installed.path.clone(),
        version_key(&installed.version),
        "adopted-external",
    );
    store.save()?;
    Ok(win_install_status(settings))
}

/// Open the installed Codex (MSIX or portable). Uses the SAME managed-aware
/// detection as status/planning (`detect_managed_codex`) — not raw MSIX-first
/// `detect_installed_codex` — so we launch exactly the build the UI is showing,
/// never a stale MSIX that lingers behind a managed portable install.
/// Fully-qualified engine call to avoid shadowing this function's name.
pub fn launch_codex(settings: &AppSettings) -> Result<(), AppError> {
    let store = ProvenanceStore::load();
    let installed = detect_managed_codex(settings, &store)
        .ok_or_else(|| AppError::Engine("没有可打开的 Codex".to_string()))?;
    if settings.disable_codex_self_updates {
        crate::app::codex_self_update::sync_setting(true)?;
    }
    let path = &installed.path;
    log::info!("Windows launch Codex path={path}");
    codex_win_engine::launch_codex_with_options(
        &installed,
        codex_win_engine::LaunchOptions {
            disable_codex_self_updates: settings.disable_codex_self_updates,
        },
    )
    .map_err(|e| AppError::Engine(e.to_string()))
}

pub fn uninstall_windows_codex(
    settings: &AppSettings,
    confirm: bool,
    purge_user_data: bool,
) -> Result<WinUninstallReport, AppError> {
    log::info!("Windows uninstall start purge_user_data={purge_user_data}");
    if !confirm {
        return Err(AppError::Internal(
            "explicit confirmation is required before uninstalling Windows Codex".to_string(),
        ));
    }

    let installed = detect_managed_codex(settings, &ProvenanceStore::load());
    let Some(installed_before) = installed else {
        return Ok(WinUninstallReport {
            success: true,
            action: "none".to_string(),
            message: "Windows Codex is not installed.".to_string(),
            installed_before: None,
            msix: None,
            portable: None,
            purged_user_data: false,
            notes: vec![],
        });
    };

    let mut store = ProvenanceStore::load();
    // Boundary (matching the macOS uninstall): refuse to delete anything that
    // isn't an install we manage at this exact build. Path-only matching could
    // delete a path-reused external install or one left by a stale record —
    // more likely now that the install root is user-configurable.
    if !store.is_managed_build(
        &installed_before.path,
        version_key(&installed_before.version),
    ) {
        log::warn!("Windows uninstall rejected external install");
        return Ok(WinUninstallReport {
            success: false,
            action: "external-not-managed".to_string(),
            message:
                "Detected Windows Codex is external. Adopt it before uninstalling via manager."
                    .to_string(),
            installed_before: Some(installed_before),
            msix: None,
            portable: None,
            purged_user_data: false,
            notes: vec!["No files or packages were removed.".to_string()],
        });
    }

    if installed_before.source == "msix" {
        close_codex_gracefully_for_root(30, PathBuf::from(&installed_before.path).as_path())
            .map_err(engine_err)?;
        let msix = remove_msix_package().map_err(engine_err)?;
        let mut notes = Vec::new();
        let mut purged_user_data = false;
        if msix.success {
            store.remove(&installed_before.path);
            store.save()?;
            notes.extend(msix.notes.clone());
            // Honor the user's "don't keep my data" choice on the MSIX path too,
            // exactly like the portable path — remove ~/.codex when asked.
            if purge_user_data {
                purged_user_data = purge_codex_user_data(&mut notes).map_err(engine_err)?;
                if purged_user_data {
                    notes.push("User data was removed.".to_string());
                }
            } else {
                notes.push("User data was preserved.".to_string());
            }
        }
        let report = WinUninstallReport {
            success: msix.success,
            action: "remove-msix".to_string(),
            message: msix.message.clone(),
            installed_before: Some(installed_before),
            msix: Some(msix),
            portable: None,
            purged_user_data,
            notes,
        };
        log::info!("Windows uninstall complete purge_user_data={purge_user_data}");
        return Ok(report);
    }

    let portable = uninstall_portable(
        PathBuf::from(&installed_before.path).as_path(),
        purge_user_data,
    )
    .map_err(engine_err)?;
    if portable.success {
        store.remove(&installed_before.path);
        store.save()?;
    }
    let report = WinUninstallReport {
        success: portable.success,
        action: "remove-portable".to_string(),
        message: portable.message.clone(),
        installed_before: Some(installed_before),
        msix: None,
        purged_user_data: portable.purged_user_data,
        notes: portable.notes.clone(),
        portable: Some(portable),
    };
    log::info!("Windows uninstall complete purge_user_data={purge_user_data}");
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::{
        bind_manifest_checksums, check_win_update_abort, detect_existing_windows_install_at_path,
        detect_managed_codex, WinAbortGuard, WinPerformAction, WIN_UPDATE_ABORT,
    };
    use crate::app::provenance::ProvenanceStore;
    use crate::domain::settings::AppSettings;
    use codex_win_engine::WindowsRelease;
    use std::path::PathBuf;
    use std::sync::atomic::Ordering;

    fn temp_test_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("codex-manager-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn write_fake_portable_install(dir: &std::path::Path, version: &str) {
        std::fs::write(dir.join("Codex.exe"), b"").unwrap();
        std::fs::write(
            dir.join("AppxManifest.xml"),
            format!(
                r#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Identity Name="OpenAI.Codex" Publisher="CN=OpenAI OpCo, LLC" Version="{version}" ProcessorArchitecture="x64" />
</Package>"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn win_abort_guard_preserves_a_startup_race_cancel_and_resets_on_drop() {
        // Mirrors the macOS guard test: a cancel landing before `perform` reaches
        // its first checkpoint must survive the guard's creation (no entry-clear)
        // and still be observed; the guard resets the latch on drop so the next
        // op — and background auto_stage — start clean.
        WIN_UPDATE_ABORT.store(true, Ordering::SeqCst);
        {
            let _guard = WinAbortGuard;
            assert!(
                check_win_update_abort().is_err(),
                "guard creation must not wipe a pending cancel"
            );
        }
        assert!(
            check_win_update_abort().is_ok(),
            "guard drop must reset the latch for the next op"
        );
    }

    #[test]
    fn serializes_win_perform_actions_as_frontend_contract() {
        let cases = [
            (WinPerformAction::None, "\"none\""),
            (WinPerformAction::MsixSideload, "\"msix-sideload\""),
            (WinPerformAction::PortableFallback, "\"portable-fallback\""),
            (
                WinPerformAction::PortableFallbackAfterMsixFailure,
                "\"portable-fallback-after-msix-failure\"",
            ),
            (
                WinPerformAction::PortableFallbackAfterMsixUnhealthy,
                "\"portable-fallback-after-msix-unhealthy\"",
            ),
            (
                WinPerformAction::PortableFallbackMissingFramework,
                "\"portable-fallback-missing-framework\"",
            ),
        ];

        for (action, expected) in cases {
            assert_eq!(serde_json::to_string(&action).unwrap(), expected);
        }
    }

    #[test]
    fn detects_user_selected_portable_install() {
        let dir = temp_test_dir("manual-existing-portable");
        write_fake_portable_install(&dir, "26.623.31921.0");

        let installed = detect_existing_windows_install_at_path(&dir).unwrap();
        assert_eq!(installed.path, dir.to_string_lossy());
        assert_eq!(installed.version, "26.623.31921.0");
        assert_eq!(installed.arch.as_deref(), Some("x64"));
        assert_eq!(installed.source, "portable");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_user_selected_non_codex_folder() {
        let dir = temp_test_dir("manual-existing-empty");
        let err = detect_existing_windows_install_at_path(&dir).unwrap_err();
        assert!(err.to_string().contains("Codex.exe"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn managed_detection_prefers_latest_provenance_record() {
        let old_dir = temp_test_dir("managed-old-portable");
        let new_dir = temp_test_dir("managed-new-portable");
        write_fake_portable_install(&old_dir, "26.623.31921.0");
        write_fake_portable_install(&new_dir, "26.623.42026.0");

        let mut store = ProvenanceStore::default();
        store.record(
            old_dir.to_string_lossy().into_owned(),
            codex_win_engine::version_key("26.623.31921.0"),
            "adopted-external",
        );
        store.record(
            new_dir.to_string_lossy().into_owned(),
            codex_win_engine::version_key("26.623.42026.0"),
            "adopted-external",
        );
        let settings = AppSettings::new(
            "https://codexapp.agentsmirror.com".to_string(),
            temp_test_dir("managed-missing-root")
                .join("missing")
                .to_string_lossy()
                .into_owned(),
        );

        let installed = detect_managed_codex(&settings, &store).unwrap();
        assert_eq!(installed.path, new_dir.to_string_lossy());
        assert_eq!(installed.version, "26.623.42026.0");

        let _ = std::fs::remove_dir_all(&old_dir);
        let _ = std::fs::remove_dir_all(&new_dir);
    }

    fn release() -> WindowsRelease {
        WindowsRelease {
            version: "26.602.3474.0".to_string(),
            package_version: "26.602.3474.0".to_string(),
            released_at: None,
            package_moniker: "OpenAI.Codex_26.602.3474.0_x64__2p2nqsd0c76g0".to_string(),
            architecture: Some("x64".to_string()),
            download_architecture: None,
            content_length: Some(566_504_666),
            etag: None,
            store_product_id: Some("9PLM9XGG6VKS".to_string()),
            package_identity: Some("OpenAI.Codex".to_string()),
        }
    }

    #[test]
    fn binds_manifest_checksums_to_url_moniker() {
        let checksums = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  OpenAI.Codex_26.602.3474.0_x64__2p2nqsd0c76g0.Msix
";
        let sha = bind_manifest_checksums(
            &release(),
            checksums,
            "https://example.com/OpenAI.Codex_26.602.3474.0_x64__2p2nqsd0c76g0.Msix",
        )
        .unwrap();
        assert_eq!(
            sha,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
    }

    #[test]
    fn accepts_stable_windows_short_url_without_msix_suffix() {
        let checksums = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  OpenAI.Codex_26.602.3474.0_x64__2p2nqsd0c76g0.Msix
";
        let sha = bind_manifest_checksums(&release(), checksums, "https://example.com/latest/win")
            .unwrap();
        assert_eq!(
            sha,
            "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"
        );
    }

    #[test]
    fn rejects_manifest_url_moniker_mismatch() {
        let checksums = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  OpenAI.Codex_26.602.3474.0_x64__2p2nqsd0c76g0.Msix
";
        let err = bind_manifest_checksums(
            &release(),
            checksums,
            "https://example.com/OpenAI.Codex_26.602.3474.0_arm64__2p2nqsd0c76g0.Msix",
        )
        .unwrap_err();
        assert!(err.to_string().contains("does not match URL artifact"));
    }

    #[test]
    fn rejects_manifest_identity_mismatch() {
        let mut release = release();
        release.package_identity = Some("OpenAI.Other".to_string());
        let checksums = "\
aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  OpenAI.Codex_26.602.3474.0_x64__2p2nqsd0c76g0.Msix
";
        assert!(bind_manifest_checksums(
            &release,
            checksums,
            "https://example.com/OpenAI.Codex_26.602.3474.0_x64__2p2nqsd0c76g0.Msix",
        )
        .is_err());
    }
}
