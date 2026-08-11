//! macOS update planning + staging service.
//!
//! Bridges the pure `codex-mac-engine` (detect + appcast + plan + download +
//! verify) to the Tauri command surface.
//!
//! - `plan_macos_update`  — read-only: what would we download? (delta vs full)
//! - `stage_macos_update` — download the artifact + size + EdDSA verify into a
//!   staging dir. Non-destructive: it does NOT apply/swap. The destructive tail
//!   (BinaryDelta apply → codesign gate → atomic swap → relaunch) comes next.
//!
//! `simulated_build` lets the UI preview the delta path even when the machine is
//! already on the latest build (the user's case during development).

use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use serde::Serialize;

use codex_mac_engine::{
    apply_delta, download, gate_reconstructed, parse_appcast, plan_update, quit_codex_at, relaunch,
    rollback, swap::codex_running_at, swap_in_place_with_observer, sys, verify_sparkle, Appcast,
    NetworkConfig, SwapBoundary, UpdatePlan, UpdateStrategy,
};

use crate::app::disk;
use crate::app::install_tx::{ActiveInstallTx, InstallTxKind, SelfUpdatePolicyTransition};
use crate::app::op_phase::OperationPhase;
use crate::app::operation_outcome::{recovery, OperationOutcome, StepOutcome};
use crate::app::provenance::ProvenanceStore;
use crate::app::release_install::{
    resolved_local_asset, verify_file_digest, ReleaseArchitecture, ReleasePackageFormat,
    ResolvedReleaseAsset,
};
use crate::app::settings_store::UpdateSource;
use crate::app::staging::{self, StagingDir};
use crate::app::url_guard::validate_custom_source;
use crate::errors::AppError;

/// Optional hook for the command layer to publish operation phases (quit policy).
pub type PhaseHook<'a> = dyn Fn(OperationPhase) + Send + Sync + 'a;

/// Completion evidence for historical installs. This keeps renderer recovery
/// honest if the invoke is lost across the destructive rename window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HistoricalMacEvidence {
    MutationStarted,
    MutationRolledBack,
}

pub type HistoricalEvidenceHook<'a> = dyn Fn(HistoricalMacEvidence) + Send + Sync + 'a;
pub type BeforeHistoricalCommitHook<'a> = dyn Fn() -> Result<(), AppError> + Send + Sync + 'a;
pub type AfterHistoricalRollbackHook<'a> = dyn Fn() -> Result<(), AppError> + Send + Sync + 'a;

const DITTO: &str = "/usr/bin/ditto";
const OPEN: &str = "/usr/bin/open";
const MAX_APP_PACKAGE_ENTRIES: usize = 200_000;
const MAX_APP_EXPANDED_BYTES: u64 = 8 * 1024 * 1024 * 1024;
const APP_EXPANSION_HEADROOM_BYTES: u64 = 512 * 1024 * 1024;
const MAX_ZIP_SYMLINK_TARGET_BYTES: u64 = 16 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledCodex {
    pub path: String,
    pub build: u64,
    /// Human-facing version (`CFBundleShortVersionString`, e.g. 26.602.40724) —
    /// what we display. `build` (CFBundleVersion) is the Sparkle comparison key.
    pub short_version: String,
    /// `arm64` / `x86_64` of the installed bundle (drives appcast selection).
    pub arch: String,
    /// Bundle file mtime as Unix seconds — when this build landed on disk
    /// (install or in-place update). A reliable date when the feed lacks one.
    pub installed_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MacUpdateReport {
    pub appcast_url: String,
    pub installed: Option<InstalledCodex>,
    pub simulated_build: Option<u64>,
    pub plan: Option<UpdatePlan>,
    /// `<pubDate>` of the appcast item matching the INSTALLED build — the true
    /// release date of the running version, when the feed publishes it.
    pub installed_pub_date: Option<String>,
    /// `<pubDate>` of the latest appcast item — the release date of the update
    /// target, when the feed publishes it.
    pub latest_pub_date: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MacStageReport {
    pub up_to_date: bool,
    pub strategy: String,
    pub latest_build: u64,
    pub latest_short_version: String,
    pub download_size: u64,
    pub full_size: u64,
    pub savings_pct: f64,
    pub staged_path: Option<String>,
    /// EdDSA signature verified against the pinned Sparkle key.
    pub verified: bool,
}

/// arm64 / x64 Sparkle appcasts, served by the mirror (CN-reachable; enclosure
/// URLs rewritten to R2/S3). EdDSA signatures are preserved (they sign bytes,
/// not URLs), so the pinned-key verification still passes against mirrored files.
pub const PROD_ARM64_APPCAST: &str = "https://codexapp.agentsmirror.com/latest/appcast.xml";
pub const PROD_X64_APPCAST: &str = "https://codexapp.agentsmirror.com/latest/appcast-x64.xml";

/// OpenAI's own Sparkle appcast (for users who can reach OpenAI directly — e.g.
/// overseas users whose only blocker is that Windows can't use the Store).
pub const OFFICIAL_ARM64_APPCAST: &str =
    "https://persistent.oaistatic.com/codex-app-prod/appcast.xml";
pub const OFFICIAL_X64_APPCAST: &str =
    "https://persistent.oaistatic.com/codex-app-prod/appcast-x64.xml";

fn appcast_for_arch(arch: &str) -> &'static str {
    match arch {
        "x86_64" | "x64" => PROD_X64_APPCAST,
        _ => PROD_ARM64_APPCAST, // arm64 / aarch64 / default
    }
}

fn official_for_arch(arch: &str) -> &'static str {
    match arch {
        "x86_64" | "x64" => OFFICIAL_X64_APPCAST,
        _ => OFFICIAL_ARM64_APPCAST,
    }
}

/// Architecture to plan against: the INSTALLED Codex's (so a delta applies to
/// the right base bundle), falling back to the host arch when nothing's installed.
fn arch_of(installed: &Option<InstalledCodex>) -> &str {
    installed
        .as_ref()
        .map(|i| i.arch.as_str())
        .unwrap_or(std::env::consts::ARCH)
}

fn fetch_one(url: String, network: &NetworkConfig) -> Result<(String, String), AppError> {
    let xml =
        sys::fetch_text_with_network(&url, network).map_err(|e| AppError::Engine(e.to_string()))?;
    Ok((url, xml))
}

/// Latest build number advertised by an appcast XML, or 0 if it can't be parsed.
fn latest_build_of(xml: &str) -> u64 {
    parse_appcast(xml)
        .ok()
        .and_then(|a| a.latest().map(|i| i.build))
        .unwrap_or(0)
}

/// Fetch the appcast XML honoring the configured source — returns (url, xml).
/// `auto` tries the CN-reachable mirror first and falls back to OpenAI official
/// when the mirror is unreachable; `mirror` / `official` / `custom` use exactly
/// that source (custom falls back to the mirror when its URL is blank).
fn fetch_appcast_for_arch(
    arch: &str,
    network: &NetworkConfig,
) -> Result<(String, String), AppError> {
    let settings = crate::app::settings_store::AppSettings::load();
    match settings.source {
        UpdateSource::Official => fetch_one(official_for_arch(arch).to_string(), network),
        UpdateSource::Mirror => fetch_one(appcast_for_arch(arch).to_string(), network),
        UpdateSource::Custom => {
            let u = settings.custom_url.trim();
            let url = if u.is_empty() {
                appcast_for_arch(arch).to_string()
            } else {
                validate_custom_source(u).map_err(|e| AppError::Engine(e.to_string()))?
            };
            fetch_one(url, network)
        }
        UpdateSource::Auto => {
            // auto: pick the higher build between the CN-reachable mirror and
            // OpenAI official, among whichever sources are reachable. The mirror
            // can lag the official feed by a release; when it does and official
            // is reachable, we still surface the newer build instead of stranding
            // the user on the stale mirror version. Official is a best-effort
            // probe with a short timeout so users who can't reach it don't stall.
            // If only one is reachable, use it; if neither, error.
            let mirror_url = appcast_for_arch(arch).to_string();
            let official_url = official_for_arch(arch).to_string();
            let mirror = sys::fetch_text_with_network(&mirror_url, network).ok();
            let official = sys::fetch_text_timeout_with_network(&official_url, 8, network).ok();
            match (mirror, official) {
                (Some(m), Some(o)) => {
                    if latest_build_of(&o) > latest_build_of(&m) {
                        Ok((official_url, o))
                    } else {
                        Ok((mirror_url, m))
                    }
                }
                (Some(m), None) => Ok((mirror_url, m)),
                (None, Some(o)) => Ok((official_url, o)),
                (None, None) => Err(AppError::Engine(
                    "both the mirror and OpenAI official appcast are unreachable".to_string(),
                )),
            }
        }
    }
}

fn parse_macos_version(s: &str) -> Option<(u32, u32)> {
    let mut parts = s.trim().split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().and_then(|m| m.parse().ok()).unwrap_or(0);
    Some((major, minor))
}

fn host_macos_version() -> Option<(u32, u32)> {
    let out = std::process::Command::new("sw_vers")
        .arg("-productVersion")
        .output()
        .ok()?;
    if !out.status.success() {
        return None;
    }
    parse_macos_version(&String::from_utf8_lossy(&out.stdout))
}

/// Reject staging/installing an update the host macOS is too old to run. If the
/// requirement or host version can't be parsed, we don't block.
fn require_os_supported(required: Option<&str>) -> Result<(), AppError> {
    let (Some(req), Some(host)) = (required.and_then(parse_macos_version), host_macos_version())
    else {
        return Ok(());
    };
    if host >= req {
        Ok(())
    } else {
        Err(AppError::Engine(format!(
            "this macOS ({}.{}) is older than the latest Codex requires ({}.{}+)",
            host.0, host.1, req.0, req.1
        )))
    }
}

fn effective_build(simulated_build: Option<u64>, installed: &Option<InstalledCodex>) -> u64 {
    simulated_build
        .or_else(|| installed.as_ref().map(|i| i.build))
        .unwrap_or(0)
}

fn installed_from_path_build(path: String, build: u64) -> InstalledCodex {
    let arch = sys::app_arch(&path).unwrap_or_else(|| std::env::consts::ARCH.to_string());
    let short_version = sys::read_bundle_short_version(&path).unwrap_or_default();
    // Bundle mtime -> when this build landed on disk (install / in-place swap).
    let installed_at = std::fs::metadata(Path::new(&path))
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs());
    InstalledCodex {
        path,
        build,
        short_version,
        arch,
        installed_at,
    }
}

fn detect_installed() -> Option<InstalledCodex> {
    sys::installed_codex_build().map(|(path, build)| installed_from_path_build(path, build))
}

pub fn detect_existing_install_at_path(path: &Path) -> Result<InstalledCodex, AppError> {
    if !path.exists() {
        return Err(AppError::Internal(
            "所选位置不存在，请选择已安装的 Codex 应用".to_string(),
        ));
    }
    if !path.is_dir() {
        return Err(AppError::Internal(
            "所选位置必须是应用包（.app）".to_string(),
        ));
    }
    // Identity, not name: after the upstream ChatGPT-brand merge the Codex
    // bundle may be named ChatGPT.app, while /Applications/ChatGPT.app can just
    // as well be ChatGPT Classic. Only CFBundleIdentifier can tell them apart.
    if path.extension().and_then(|ext| ext.to_str()) != Some("app") {
        return Err(AppError::Internal(
            "请选择 Codex 应用本体（.app），而不是它的上级文件夹".to_string(),
        ));
    }
    let raw = path.to_string_lossy();
    match sys::read_bundle_identifier(&raw).as_deref() {
        Some(sys::CODEX_BUNDLE_ID) => {}
        Some("com.openai.chat") => {
            return Err(AppError::Internal(
                "所选应用是 ChatGPT Classic（com.openai.chat），不是 Codex；本工具只管理 Codex"
                    .to_string(),
            ));
        }
        Some(other) => {
            return Err(AppError::Internal(format!(
                "所选应用不是 Codex（CFBundleIdentifier 为 {other}，期望 {}）",
                sys::CODEX_BUNDLE_ID
            )));
        }
        None => {
            return Err(AppError::Internal(
                "无法读取所选应用的 CFBundleIdentifier，请选择已安装的 Codex 应用".to_string(),
            ));
        }
    }
    require_not_translocation_risk(&raw)?;
    let (detected_path, build) = sys::installed_codex_build_at_path(&raw)
        .ok_or_else(|| AppError::Internal("无法读取所选 Codex 应用的版本信息".to_string()))?;
    Ok(installed_from_path_build(detected_path, build))
}

/// Refuse paths macOS would run through App Translocation: the live process
/// then sits on a randomized mount, every path-scoped run/quit protection is
/// blind to it, and a swap/uninstall could act while the app is running.
/// Checked at adoption AND re-checked right before each destructive tail
/// (the attribute can appear later, e.g. after a re-download over the path).
fn require_not_translocation_risk(app: &str) -> Result<(), AppError> {
    if sys::is_translocation_risk(app) {
        return Err(AppError::Internal(
            "该应用带有会触发 App Translocation 的 macOS 隔离属性：系统会在随机路径运行它，\
             无法安全地检测或退出运行中的实例。请先退出该应用，再用 Finder 将它「移动」到\
             其他位置再移回（移动会写入豁免标记），或运行 xattr -d com.apple.quarantine \
             移除隔离属性后重试"
                .to_string(),
        ));
    }
    // De-quarantining does not migrate an ALREADY-RUNNING translocated
    // instance back — it stays on its randomized mount, invisible to the
    // path-scoped quit check. Refuse until it exits. (Bundle-name matching may
    // also catch a translocated ChatGPT Classic; quitting it too is a harmless
    // ask compared to swapping under a live process.)
    if codex_mac_engine::swap::translocated_instance_running(Path::new(app)) {
        return Err(AppError::Internal(
            "检测到该应用（或同名应用）仍有一个经 App Translocation 启动的实例在运行：\
             它不会随隔离属性的清除而迁回原路径，无法被安全退出。请先退出该应用后重试"
                .to_string(),
        ));
    }
    Ok(())
}

pub(crate) fn detect_managed_installed() -> Option<InstalledCodex> {
    let store = ProvenanceStore::load();
    for record in store.managed.iter().rev() {
        if let Some((path, build)) = sys::installed_codex_build_at_path(&record.path) {
            if store.is_managed_build(&path, build) {
                return Some(installed_from_path_build(path, build));
            }
        }
    }
    detect_installed()
}

pub fn plan_macos_update(simulated_build: Option<u64>) -> Result<MacUpdateReport, AppError> {
    plan_macos_update_with_network(simulated_build, &NetworkConfig::system())
}

pub fn plan_macos_update_with_network(
    simulated_build: Option<u64>,
    network: &NetworkConfig,
) -> Result<MacUpdateReport, AppError> {
    log::info!("macOS plan start simulated_build={simulated_build:?}");
    let installed = detect_managed_installed();
    let (appcast_url, xml) = fetch_appcast_for_arch(arch_of(&installed), network)?;
    let appcast = parse_appcast(&xml).map_err(|e| AppError::Engine(e.to_string()))?;
    let plan = plan_update(&appcast, effective_build(simulated_build, &installed));

    let latest = appcast.latest();
    if let Some(latest) = latest {
        require_os_supported(latest.minimum_system_version.as_deref())?;
    }
    let latest_pub_date = latest.and_then(|latest| latest.pub_date.clone());

    // Release date of the *installed* build (its own appcast item), so it stays
    // correct even when a newer version is available. None when the feed omits
    // pubDate or the installed build has aged out of the feed.
    let installed_pub_date = installed.as_ref().and_then(|inst| {
        appcast
            .items
            .iter()
            .find(|it| it.build == inst.build)
            .and_then(|it| it.pub_date.clone())
    });

    let report = MacUpdateReport {
        appcast_url,
        installed,
        simulated_build,
        plan,
        installed_pub_date,
        latest_pub_date,
    };
    let installed_build = report.installed.as_ref().map(|installed| installed.build);
    let latest_build = report.plan.as_ref().map(|plan| plan.latest_build);
    let strategy = report
        .plan
        .as_ref()
        .map(|plan| strategy_label(&plan.strategy))
        .unwrap_or_else(|| "none".to_string());
    log::info!(
        "macOS plan complete installed_build={installed_build:?} latest_build={latest_build:?} strategy={strategy}"
    );
    Ok(report)
}

fn strategy_label(strategy: &UpdateStrategy) -> String {
    match strategy {
        UpdateStrategy::Delta { from_build } => format!("delta-from-{from_build}"),
        UpdateStrategy::Full => "full".to_string(),
    }
}

/// Worst-case temporary disk budget for one macOS update. Even a delta plan may
/// fall back to the full package, and `ditto` extraction can briefly occupy
/// several times the zip size, so the budget uses the full archive as the
/// conservative basis.
fn mac_space_budget(plan: &UpdatePlan) -> u64 {
    const HEADROOM: u64 = 512 * 1024 * 1024;
    plan.download_size
        .saturating_add(plan.full_size.saturating_mul(3))
        .saturating_add(HEADROOM)
}

fn human_bytes(bytes: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    const MIB: f64 = 1024.0 * 1024.0;
    if bytes >= 1024 * 1024 * 1024 {
        format!("{:.1} GiB", bytes as f64 / GIB)
    } else {
        format!("{:.0} MiB", bytes as f64 / MIB)
    }
}

fn preflight_mac_disk(plan: &UpdatePlan) -> Result<(), AppError> {
    preflight_mac_disk_with_available(plan, disk::available_space)
}

fn preflight_mac_disk_with_available<F>(plan: &UpdatePlan, available: F) -> Result<(), AppError>
where
    F: Fn(&Path) -> Result<Option<u64>, AppError>,
{
    let staging = staging::staging_root();
    let need = mac_space_budget(plan);
    if let Some(free) = available(&staging)? {
        if free < need {
            return Err(AppError::Engine(format!(
                "磁盘可用空间不足：本次更新约需 {}，当前可用 {}。请清理后重试",
                human_bytes(need),
                human_bytes(free)
            )));
        }
    }
    Ok(())
}

/// Download an artifact `(url, size, signature)` into staging, size-gate it, and
/// verify its EdDSA signature against the pinned Sparkle key. Idempotent: reuses
/// an already-staged file of the right size. On EdDSA failure the staged file is
/// dropped so a retry re-downloads instead of trusting a corrupt same-size
/// cache. Returns the verified staged path. Shared by `stage` (non-destructive)
/// and `perform` (the full install), so the destructive path can never skip the
/// pinned-key check. Taking explicit fields (rather than a plan) lets `perform`
/// download the full enclosure when it falls back from a delta.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgress {
    pub downloaded: u64,
    pub total: u64,
    /// Host the bytes are coming from, e.g. `codexapp.agentsmirror.com`.
    pub source: String,
}

/// Host portion of a URL, for showing the user which source is downloading.
fn host_of(url: &str) -> String {
    url.split("://")
        .nth(1)
        .and_then(|rest| rest.split('/').next())
        .unwrap_or("")
        .to_string()
}

/// No-op progress sink for downloads whose progress isn't surfaced (e.g. stage).
fn no_progress(_p: DownloadProgress) {}

/// Graceful quit with a user-actionable failure message. Codex often answers
/// the quit event with its own confirmation dialog ("Quit Codex?" when
/// automations are enabled); the engine brings that dialog frontmost after a
/// grace period, and if the user still hasn't confirmed by the timeout we say
/// exactly what to click instead of a bare timeout. Never force-kills.
fn quit_codex_gracefully(install_app: &Path) -> Result<(), AppError> {
    quit_codex_at(install_app, 30).map_err(|_| {
        AppError::Engine(
            "Codex 未在限时内退出——它可能正在等待退出确认（如「Quit Codex?」对话框，已尝试将其带到前台）。\
             请在 Codex 中确认退出后重试；为保护进行中的会话，不会强制结束 Codex"
                .to_string(),
        )
    })
}

fn download_and_verify(
    url: &str,
    size: u64,
    max_size: u64,
    signature: &str,
    progress: &dyn Fn(DownloadProgress),
    network: &NetworkConfig,
) -> Result<PathBuf, AppError> {
    let source = host_of(url);
    log::info!("macOS download and verify start source={source}");
    if size > max_size {
        return Err(AppError::Engine(format!(
            "artifact size {size} exceeds {max_size} byte limit"
        )));
    }
    let file_name = url.rsplit('/').next().unwrap_or("payload.bin");
    // Download into the PERSISTENT cache (not a per-run staging dir): a paused
    // `.part` survives here, so the next perform/install resumes it instead of
    // restarting at 0. The artifact is consumed (verified → unpacked/applied)
    // from here; success clears the cache (see perform/install tails).
    let dest = staging::download_cache_path(url, file_name)?;
    let source = host_of(url);

    let already = std::fs::metadata(&dest)
        .map(|m| m.len() == size)
        .unwrap_or(false);
    if already {
        // Cached from a prior stage — report complete so the UI doesn't sit at 0.
        progress(DownloadProgress {
            downloaded: size,
            total: size,
            source,
        });
    } else {
        download::download_to_with_progress_bounded_with_network(
            url,
            &dest,
            max_size,
            &|downloaded| {
                progress(DownloadProgress {
                    downloaded,
                    total: size,
                    source: source.clone(),
                });
            },
            network,
        )
        .map_err(|e| AppError::Engine(e.to_string()))?;
    }

    let len = std::fs::metadata(&dest)
        .map_err(|e| AppError::Engine(e.to_string()))?
        .len();
    if len != size {
        let err = AppError::Engine(format!("size mismatch: {len} != {size}"));
        log::error!("macOS download and verify failed error={err}");
        return Err(err);
    }
    if len > max_size {
        let err = AppError::Engine(format!("artifact size {len} exceeds {max_size} byte limit"));
        log::error!("macOS download and verify failed error={err}");
        return Err(err);
    }

    let bytes = download::read_file(&dest).map_err(|e| AppError::Engine(e.to_string()))?;
    if let Err(err) = verify_sparkle(&bytes, signature) {
        let _ = std::fs::remove_file(&dest);
        let err = AppError::Engine(err.to_string());
        log::error!("macOS download and verify failed error={err}");
        return Err(err);
    }

    let path = dest.display();
    log::info!("macOS download and verify complete path={path}");
    Ok(dest)
}

fn max_bytes_for_strategy(strategy: &UpdateStrategy) -> u64 {
    match strategy {
        UpdateStrategy::Delta { .. } => codex_mac_engine::limits::MAX_DELTA_BYTES,
        UpdateStrategy::Full => codex_mac_engine::limits::MAX_PACKAGE_BYTES,
    }
}

pub fn stage_macos_update(simulated_build: Option<u64>) -> Result<MacStageReport, AppError> {
    stage_macos_update_with_network(simulated_build, &NetworkConfig::system())
}

pub fn stage_macos_update_with_network(
    simulated_build: Option<u64>,
    network: &NetworkConfig,
) -> Result<MacStageReport, AppError> {
    log::info!("macOS stage start simulated_build={simulated_build:?}");
    // A standalone stage can also be cancelled (its download sets the abort
    // latch). Own a guard so a cancelled stage can't leave the latch set and
    // make the NEXT perform/install abort itself at its first checkpoint.
    let _abort_guard = AbortGuard::new();
    let installed = detect_managed_installed();
    let (_, xml) = fetch_appcast_for_arch(arch_of(&installed), network)?;
    let appcast = parse_appcast(&xml).map_err(|e| AppError::Engine(e.to_string()))?;
    let plan = plan_update(&appcast, effective_build(simulated_build, &installed))
        .ok_or_else(|| AppError::Engine("appcast had no items".to_string()))?;

    if plan.up_to_date {
        log::info!(
            "macOS stage complete build={} verified=false",
            plan.latest_build
        );
        return Ok(MacStageReport {
            up_to_date: true,
            strategy: "none".to_string(),
            latest_build: plan.latest_build,
            latest_short_version: plan.latest_short_version,
            download_size: 0,
            full_size: plan.full_size,
            savings_pct: 0.0,
            staged_path: None,
            verified: false,
        });
    }

    if let Some(latest) = appcast.latest() {
        require_os_supported(latest.minimum_system_version.as_deref())?;
    }
    preflight_mac_disk(&plan)?;

    let signature = plan
        .ed_signature
        .clone()
        .ok_or_else(|| AppError::Engine("appcast enclosure missing edSignature".to_string()))?;
    // Stages straight into the persistent download cache — no per-run staging dir
    // to discard, so a paused partial survives for the next resume.
    let dest = match download_and_verify(
        &plan.download_url,
        plan.download_size,
        max_bytes_for_strategy(&plan.strategy),
        &signature,
        &no_progress,
        network,
    ) {
        Ok(dest) => dest,
        Err(err) => {
            log::error!("macOS stage failed error={err}");
            return Err(err);
        }
    };

    let report = MacStageReport {
        up_to_date: false,
        strategy: strategy_label(&plan.strategy),
        latest_build: plan.latest_build,
        latest_short_version: plan.latest_short_version,
        download_size: plan.download_size,
        full_size: plan.full_size,
        savings_pct: plan.savings_pct,
        staged_path: Some(dest.to_string_lossy().into_owned()),
        verified: true,
    };
    let build = report.latest_build;
    let verified = report.verified;
    log::info!("macOS stage complete build={build} verified={verified}");
    Ok(report)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MacPerformReport {
    pub up_to_date: bool,
    pub from_build: u64,
    pub to_build: u64,
    pub strategy: String,
    pub installed_path: String,
    /// EdDSA (download) + codesign/Team/Gatekeeper (reconstructed bundle) passed.
    pub verified: bool,
    /// The new version was relaunched (only when Codex had been running).
    pub relaunched: bool,
    /// Codex WAS running but the relaunch failed — the user must start it
    /// manually. Distinct from a plain `!relaunched`, which also covers the
    /// clean case where Codex simply wasn't running (no action needed).
    pub relaunch_failed: bool,
    /// The post-swap health check failed and we restored the previous bundle.
    pub rolled_back: bool,
    /// A non-fatal warning to surface alongside an otherwise *successful* update
    /// — e.g. the provenance record couldn't be saved (the install will keep
    /// being seen as external), or where the previous bundle's backup was kept
    /// after a relaunch failure. None on a fully clean update.
    pub warning: Option<String>,
    pub message: String,
}

/// What the user saw + consented to at stage/confirm time. `perform` re-checks
/// reality against this BEFORE the destructive swap, so a Codex self-update, a
/// moved install, or an appcast that advanced between confirm and execute can't
/// silently redirect the swap to a target the user never approved.
#[derive(Debug, Clone)]
pub struct PerformExpectation {
    pub from_build: u64,
    pub to_build: u64,
    pub install_path: String,
}

/// Unpack a Sparkle macOS full-update `.zip` and surface the `.app` inside it.
/// `ditto -x -k` is used (not `unzip`) because it preserves the extended
/// attributes and resource metadata a code signature depends on, so the
/// extracted bundle still passes `codesign`/Gatekeeper. The found bundle is
/// moved to `out_app` (both live under the same-volume `work` dir).
fn unpack_app_zip(zip: &Path, work: &Path, out_app: &Path) -> Result<(), AppError> {
    preflight_app_zip(zip, work)?;

    let extract = work.join("unzip");
    let _ = std::fs::remove_dir_all(&extract);
    std::fs::create_dir_all(&extract).map_err(|e| AppError::Engine(format!("mkdir unzip: {e}")))?;

    let status = std::process::Command::new(DITTO)
        .args(["-x", "-k"])
        .arg(zip)
        .arg(&extract)
        .status()
        .map_err(|e| AppError::Engine(format!("spawn ditto: {e}")))?;
    if !status.success() {
        return Err(AppError::Engine(format!(
            "ditto unzip exited with {status}"
        )));
    }

    // The upstream archive may ship the bundle under either name (Codex.app
    // pre-merge, ChatGPT.app post-merge); identity is checked, and the rename
    // below normalizes whatever we accepted to the caller's canonical out_app.
    let found = require_single_codex_app(&extract)?;
    // Defense in depth against parser differentials between the Rust ZIP reader
    // and ditto: the actual extracted bundle must still fit the same hard tree
    // budget before it can become the install candidate.
    extracted_app_logical_size(&found)?;
    if out_app.exists() {
        let _ = std::fs::remove_dir_all(out_app);
    }
    std::fs::rename(&found, out_app)
        .map_err(|e| AppError::Engine(format!("move unpacked app: {e}")))?;
    Ok(())
}

/// Validate archive paths and stream every entry through the real decompressor
/// before `ditto` writes anything. Central-directory sizes are attacker input:
/// they are used only as a consistency check, never as the expansion budget.
/// Native extraction is still required to preserve signatures, but only after
/// the measured output bytes fit the hard limit and available-space budget.
fn preflight_app_zip(path: &Path, work: &Path) -> Result<(), AppError> {
    preflight_app_zip_with_available(path, work, disk::available_space)
}

fn preflight_app_zip_with_available<F>(
    path: &Path,
    work: &Path,
    available: F,
) -> Result<(), AppError>
where
    F: Fn(&Path) -> Result<Option<u64>, AppError>,
{
    let metadata =
        std::fs::metadata(path).map_err(|e| AppError::Engine(format!("读取 ZIP 信息失败: {e}")))?;
    if !metadata.is_file()
        || metadata.len() == 0
        || metadata.len() > codex_mac_engine::limits::MAX_PACKAGE_BYTES
    {
        return Err(AppError::Engine(format!(
            "ZIP 大小 {} 超出允许范围",
            metadata.len()
        )));
    }

    let file =
        std::fs::File::open(path).map_err(|e| AppError::Engine(format!("打开 ZIP 失败: {e}")))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| AppError::Engine(format!("读取 ZIP 目录失败: {e}")))?;
    if archive.is_empty() || archive.len() > MAX_APP_PACKAGE_ENTRIES {
        return Err(AppError::Engine(format!(
            "ZIP 条目数 {} 超出允许范围",
            archive.len()
        )));
    }

    let mut expanded_bytes = 0_u64;
    for index in 0..archive.len() {
        let mut entry = archive
            .by_index(index)
            .map_err(|e| AppError::Engine(format!("读取 ZIP 条目失败: {e}")))?;
        let Some(enclosed) = entry.enclosed_name() else {
            return Err(AppError::Engine(format!(
                "ZIP 包含不安全路径：{}",
                entry.name()
            )));
        };
        let declared_size = entry.size();
        // Oversized declarations are safe to reject early. A forged *small*
        // declaration is caught below by counting actual decompressor output.
        let remaining = MAX_APP_EXPANDED_BYTES.saturating_sub(expanded_bytes);
        if declared_size > remaining {
            return Err(AppError::Engine(format!(
                "ZIP 声明解压后大小超出允许范围：{}",
                entry.name()
            )));
        }

        let actual_size = if entry.is_symlink() {
            if declared_size == 0 || declared_size > MAX_ZIP_SYMLINK_TARGET_BYTES {
                return Err(AppError::Engine(format!(
                    "ZIP 包含无效符号链接：{}",
                    entry.name()
                )));
            }
            let mut target = Vec::new();
            let actual = (&mut entry)
                .take(MAX_ZIP_SYMLINK_TARGET_BYTES + 1)
                .read_to_end(&mut target)
                .map_err(|e| AppError::Engine(format!("读取 ZIP 符号链接失败: {e}")))?;
            let actual = actual as u64;
            if actual == 0 || actual > MAX_ZIP_SYMLINK_TARGET_BYTES {
                return Err(AppError::Engine(format!(
                    "ZIP 符号链接实际展开大小超出允许范围：{}",
                    entry.name()
                )));
            }
            let target = String::from_utf8(target).map_err(|_| {
                AppError::Engine(format!("ZIP 符号链接目标不是 UTF-8：{}", entry.name()))
            })?;
            validate_zip_symlink_target(&enclosed, &target)?;
            actual
        } else {
            std::io::copy(
                &mut (&mut entry).take(remaining.saturating_add(1)),
                &mut std::io::sink(),
            )
            .map_err(|e| AppError::Engine(format!("展开 ZIP 条目失败: {e}")))?
        };
        if actual_size > remaining {
            return Err(AppError::Engine(format!(
                "ZIP 实际解压后大小超出允许范围：{}",
                entry.name()
            )));
        }
        if actual_size != declared_size {
            return Err(AppError::Engine(format!(
                "ZIP 条目声明大小与实际展开大小不一致：{}（{} != {}），拒绝安装",
                entry.name(),
                declared_size,
                actual_size
            )));
        }
        expanded_bytes = checked_zip_expanded_bytes(expanded_bytes, actual_size)?;
    }
    require_zip_expansion_space(expanded_bytes, work, available)?;
    Ok(())
}

/// Official signed app bundles contain relative framework/node symlinks, so a
/// blanket symlink ban would reject real GitHub ZIP assets. Allow only nested,
/// relative links whose lexical destination remains inside the same top-level
/// `.app`; in particular the `.app` itself can never be a link to a machine-local
/// signed installation.
fn validate_zip_symlink_target(link: &Path, target: &str) -> Result<(), AppError> {
    let components = link.components().collect::<Vec<_>>();
    let app_root = match components.as_slice() {
        [Component::Normal(root), _, ..]
            if Path::new(root).extension().is_some_and(|x| x == "app") =>
        {
            root.to_os_string()
        }
        _ => {
            return Err(AppError::Engine(format!(
                "ZIP 符号链接不在物理 .app 目录内：{}",
                link.display()
            )))
        }
    };
    if target.is_empty() || target.contains('\0') {
        return Err(AppError::Engine(format!(
            "ZIP 符号链接目标无效：{}",
            link.display()
        )));
    }

    let mut resolved = link
        .parent()
        .into_iter()
        .flat_map(Path::components)
        .filter_map(|component| match component {
            Component::Normal(value) => Some(value.to_os_string()),
            _ => None,
        })
        .collect::<Vec<_>>();
    for component in Path::new(target).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(value) => resolved.push(value.to_os_string()),
            Component::ParentDir if resolved.len() > 1 => {
                resolved.pop();
            }
            Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                return Err(AppError::Engine(format!(
                    "ZIP 符号链接会逃出应用目录：{} -> {target}",
                    link.display()
                )))
            }
        }
    }
    if resolved.first() != Some(&app_root) {
        return Err(AppError::Engine(format!(
            "ZIP 符号链接会逃出应用目录：{} -> {target}",
            link.display()
        )));
    }
    Ok(())
}

fn require_zip_expansion_space<F>(
    expanded_bytes: u64,
    work: &Path,
    available: F,
) -> Result<(), AppError>
where
    F: Fn(&Path) -> Result<Option<u64>, AppError>,
{
    let need = expanded_bytes.saturating_add(APP_EXPANSION_HEADROOM_BYTES);
    if let Some(free) = available(work)? {
        if free < need {
            return Err(AppError::Engine(format!(
                "磁盘可用空间不足：ZIP 解压约需 {}，当前可用 {}。请清理后重试",
                human_bytes(need),
                human_bytes(free)
            )));
        }
    }
    Ok(())
}

fn checked_zip_expanded_bytes(current: u64, entry_size: u64) -> Result<u64, AppError> {
    let total = current
        .checked_add(entry_size)
        .ok_or_else(|| AppError::Engine("ZIP 解压大小溢出，拒绝安装".to_string()))?;
    if total > MAX_APP_EXPANDED_BYTES {
        return Err(AppError::Engine(format!(
            "ZIP 解压后大小 {total} 超出允许范围"
        )));
    }
    Ok(total)
}

fn preflight_app_dmg_with_available<F>(
    dmg: &Path,
    work: &Path,
    available: F,
) -> Result<(), AppError>
where
    F: Fn(&Path) -> Result<Option<u64>, AppError>,
{
    const HEADROOM: u64 = 512 * 1024 * 1024;
    let metadata =
        std::fs::metadata(dmg).map_err(|e| AppError::Engine(format!("读取 DMG 信息失败: {e}")))?;
    let size = metadata.len();
    if !metadata.is_file() || size == 0 || size > codex_mac_engine::limits::MAX_PACKAGE_BYTES {
        return Err(AppError::Engine(format!("DMG 大小 {size} 超出允许范围")));
    }
    let need = size.saturating_mul(4).saturating_add(HEADROOM);
    if let Some(free) = available(work)? {
        if free < need {
            return Err(AppError::Engine(format!(
                "磁盘可用空间不足：检查 DMG 约需 {}，当前可用 {}。请清理后重试",
                human_bytes(need),
                human_bytes(free)
            )));
        }
    }
    Ok(())
}

fn preflight_app_dmg(dmg: &Path, work: &Path) -> Result<(), AppError> {
    preflight_app_dmg_with_available(dmg, work, disk::available_space)
}

/// Measure the mounted bundle itself before `ditto` copies it. A compressed DMG
/// has no trustworthy fixed expansion ratio, so the image-file preflight above
/// is only an early rejection; this walk is the authoritative copy budget.
/// Symlinks are counted as entries but never followed into the host filesystem.
fn mounted_app_logical_size(app: &Path) -> Result<u64, AppError> {
    physical_app_logical_size(app, "DMG 应用")
}

fn extracted_app_logical_size(app: &Path) -> Result<u64, AppError> {
    physical_app_logical_size(app, "ZIP 应用")
}

fn physical_app_logical_size(app: &Path, label: &str) -> Result<u64, AppError> {
    let metadata = std::fs::symlink_metadata(app)
        .map_err(|e| AppError::Engine(format!("读取{label}信息失败: {e}")))?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(AppError::Engine(format!("{label}不是物理目录，拒绝复制")));
    }

    let mut pending = vec![app.to_path_buf()];
    let mut entries = 0_usize;
    let mut logical_bytes = 0_u64;
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir)
            .map_err(|e| AppError::Engine(format!("读取{label}目录失败: {e}")))?
        {
            let entry = entry.map_err(|e| AppError::Engine(format!("读取{label}条目失败: {e}")))?;
            entries = entries
                .checked_add(1)
                .ok_or_else(|| AppError::Engine("DMG 应用条目数溢出，拒绝安装".to_string()))?;
            if entries > MAX_APP_PACKAGE_ENTRIES {
                return Err(AppError::Engine(format!(
                    "{label}条目数 {entries} 超出允许范围"
                )));
            }

            let file_type = entry
                .file_type()
                .map_err(|e| AppError::Engine(format!("读取{label}条目类型失败: {e}")))?;
            if file_type.is_symlink() {
                continue;
            }
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                let size = entry
                    .metadata()
                    .map_err(|e| AppError::Engine(format!("读取{label}文件大小失败: {e}")))?
                    .len();
                logical_bytes = checked_app_expanded_bytes(logical_bytes, size, label)?;
            } else {
                return Err(AppError::Engine(format!(
                    "{label}包含不支持的特殊文件：{}",
                    entry.path().display()
                )));
            }
        }
    }
    if entries == 0 || logical_bytes == 0 {
        return Err(AppError::Engine(format!("{label}内容为空，拒绝安装")));
    }
    Ok(logical_bytes)
}

fn checked_app_expanded_bytes(current: u64, entry_size: u64, label: &str) -> Result<u64, AppError> {
    let total = current
        .checked_add(entry_size)
        .ok_or_else(|| AppError::Engine(format!("{label}大小溢出，拒绝安装")))?;
    if total > MAX_APP_EXPANDED_BYTES {
        return Err(AppError::Engine(format!(
            "{label}大小 {total} 超出允许范围"
        )));
    }
    Ok(total)
}

fn preflight_mounted_app_copy_with_available<F>(
    app: &Path,
    work: &Path,
    available: F,
) -> Result<(), AppError>
where
    F: Fn(&Path) -> Result<Option<u64>, AppError>,
{
    let logical_bytes = mounted_app_logical_size(app)?;
    let need = logical_bytes.saturating_add(APP_EXPANSION_HEADROOM_BYTES);
    if let Some(free) = available(work)? {
        if free < need {
            return Err(AppError::Engine(format!(
                "磁盘可用空间不足：复制 DMG 应用约需 {}，当前可用 {}。请清理后重试",
                human_bytes(need),
                human_bytes(free)
            )));
        }
    }
    Ok(())
}

fn preflight_mounted_app_copy(app: &Path, work: &Path) -> Result<(), AppError> {
    preflight_mounted_app_copy_with_available(app, work, disk::available_space)
}

/// Mount a GitHub Release DMG read-only, copy its single Codex-lineage app with
/// `ditto` (preserving signature metadata), then detach before installation.
fn unpack_app_dmg(dmg: &Path, work: &Path, out_app: &Path) -> Result<(), AppError> {
    const HDIUTIL: &str = "/usr/bin/hdiutil";
    // Fail before hdiutil can mount or ditto can expand an oversized local
    // image. This is also used by the offline picker inspection path.
    preflight_app_dmg(dmg, work)?;
    let mount = work.join("dmg-mount");
    let _ = std::fs::remove_dir_all(&mount);
    std::fs::create_dir_all(&mount)
        .map_err(|e| AppError::Engine(format!("创建 DMG 挂载目录失败: {e}")))?;
    let attach = std::process::Command::new(HDIUTIL)
        .args([
            "attach",
            "-readonly",
            "-nobrowse",
            "-noautoopen",
            "-mountpoint",
        ])
        .arg(&mount)
        .arg(dmg)
        .status()
        .map_err(|e| AppError::Engine(format!("启动 hdiutil 失败: {e}")))?;
    if !attach.success() {
        return Err(AppError::Engine(format!(
            "hdiutil 挂载 DMG 失败（{attach}）"
        )));
    }

    let copy_result = (|| -> Result<(), AppError> {
        let found = require_single_codex_app(&mount)?;
        preflight_mounted_app_copy(&found, work)?;
        if out_app.exists() {
            std::fs::remove_dir_all(out_app)
                .map_err(|e| AppError::Engine(format!("清理旧暂存应用失败: {e}")))?;
        }
        let status = std::process::Command::new(DITTO)
            .arg(&found)
            .arg(out_app)
            .status()
            .map_err(|e| AppError::Engine(format!("从 DMG 复制应用失败: {e}")))?;
        if !status.success() {
            return Err(AppError::Engine(format!(
                "ditto 从 DMG 复制应用失败（{status}）"
            )));
        }
        Ok(())
    })();

    let detach = std::process::Command::new(HDIUTIL)
        .args(["detach"])
        .arg(&mount)
        .status()
        .map_err(|e| AppError::Engine(format!("启动 hdiutil detach 失败: {e}")))?;
    if !detach.success() {
        return Err(AppError::Engine(format!(
            "DMG 已读取但无法安全卸载（{detach}），拒绝继续安装"
        )));
    }
    copy_result
}

fn require_single_codex_app(dir: &Path) -> Result<PathBuf, AppError> {
    let mut apps = Vec::new();
    for entry in std::fs::read_dir(dir).map_err(|e| AppError::Engine(format!("read unzip: {e}")))? {
        let entry = entry.map_err(|e| AppError::Engine(format!("read unzip entry: {e}")))?;
        let path = entry.path();
        if path.extension().is_some_and(|x| x == "app")
            && entry
                .file_type()
                .map_err(|e| AppError::Engine(format!("read unzip entry type: {e}")))?
                .is_symlink()
        {
            return Err(AppError::Engine(
                "安装包的顶层 .app 是符号链接，拒绝跟随本机路径".to_string(),
            ));
        }
        if entry
            .file_type()
            .map_err(|e| AppError::Engine(format!("read unzip entry type: {e}")))?
            .is_dir()
            && path.extension().is_some_and(|x| x == "app")
        {
            apps.push(path);
        }
    }
    if apps.is_empty() {
        return Err(AppError::Engine(
            "full-update zip did not contain an .app bundle".to_string(),
        ));
    }
    if apps.len() > 1 {
        return Err(AppError::Engine(
            "full-update zip contained multiple .app bundles; refusing to guess".to_string(),
        ));
    }
    let app = apps.remove(0);
    let root = std::fs::canonicalize(dir)
        .map_err(|e| AppError::Engine(format!("resolve extraction root: {e}")))?;
    let physical = std::fs::canonicalize(&app)
        .map_err(|e| AppError::Engine(format!("resolve extracted app: {e}")))?;
    if physical.parent() != Some(root.as_path()) {
        return Err(AppError::Engine(
            "安装包的 .app 不在解压根目录内，拒绝安装".to_string(),
        ));
    }
    // Accept any top-level bundle name but require the Codex product identity —
    // the codesign gate re-asserts this against the sealed plist right after.
    match sys::read_bundle_identifier(&app.to_string_lossy()).as_deref() {
        Some(sys::CODEX_BUNDLE_ID) => Ok(app),
        Some(other) => Err(AppError::Engine(format!(
            "full-update zip contained a non-Codex .app bundle (CFBundleIdentifier {other}, expected {})",
            sys::CODEX_BUNDLE_ID
        ))),
        None => Err(AppError::Engine(
            "full-update zip's .app bundle had no readable CFBundleIdentifier".to_string(),
        )),
    }
}

/// Download the appcast's full enclosure (size + EdDSA verified) and unpack it
/// into `out_app`. Needs no BinaryDelta — used both as the primary full path and
/// as the recovery when a delta is unavailable or fails to apply.
fn reconstruct_full(
    appcast: &Appcast,
    work: &Path,
    out_app: &Path,
    progress: &dyn Fn(DownloadProgress),
    network: &NetworkConfig,
) -> Result<(), AppError> {
    let latest = appcast
        .latest()
        .ok_or_else(|| AppError::Engine("appcast had no items".to_string()))?;
    let sig = latest.full.ed_signature.clone().ok_or_else(|| {
        AppError::Engine("appcast full enclosure missing edSignature".to_string())
    })?;
    let staged = download_and_verify(
        &latest.full.url,
        latest.full.length,
        codex_mac_engine::limits::MAX_PACKAGE_BYTES,
        &sig,
        progress,
        network,
    )?;
    unpack_app_zip(&staged, work, out_app)
}

/// **Destructive**: download → verify → reconstruct → gate → quit → atomic swap
/// → health-check → relaunch (or rollback). Always operates on the REAL installed
/// build (no `simulated_build` — a delta only applies to the true basis bundle).
///
/// Ordering minimizes downtime and maximizes safety:
///   1. download the artifact and verify its EdDSA signature (pinned key);
///   2. reconstruct the new bundle in same-volume staging (delta apply, or unzip
///      a full update) — Codex may stay running; the basis is only read;
///   3. gate the reconstructed bundle (codesign/Team=OpenAI/Gatekeeper) BEFORE it
///      goes anywhere near the install root — a compromised mirror cannot forge
///      Apple's Developer ID signature;
///   4. ask Codex to quit gracefully (never force-kill — protects in-flight work);
///      if it refuses we abort with the install root untouched;
///   5. same-volume atomic `rename` swap, keeping the old bundle as a backup;
///   6. filesystem health check (installed build == target && still gated). On
///      success drop the backup, record managed provenance, relaunch the new
///      version (only if Codex had been running). On failure roll back to the
///      preserved old bundle and relaunch it.
///
/// `binary_delta` is the optional path to the vendored Sparkle `BinaryDelta`
/// tool, resolved by the command layer (it owns the Tauri resource resolver).
/// It is only required for the delta branch; a full-package update ignores it,
/// so `None` is fine unless an actual delta needs reconstructing.
pub fn perform_macos_update(
    binary_delta: Option<PathBuf>,
    expected: PerformExpectation,
    progress: &dyn Fn(DownloadProgress),
) -> Result<MacPerformReport, AppError> {
    perform_macos_update_with_network(binary_delta, expected, progress, &NetworkConfig::system())
}

pub fn perform_macos_update_with_network(
    binary_delta: Option<PathBuf>,
    expected: PerformExpectation,
    progress: &dyn Fn(DownloadProgress),
    network: &NetworkConfig,
) -> Result<MacPerformReport, AppError> {
    perform_macos_update_with_network_and_phase(binary_delta, expected, progress, network, None)
}

pub fn perform_macos_update_with_network_and_phase(
    binary_delta: Option<PathBuf>,
    expected: PerformExpectation,
    progress: &dyn Fn(DownloadProgress),
    network: &NetworkConfig,
    phase: Option<&PhaseHook<'_>>,
) -> Result<MacPerformReport, AppError> {
    let set_phase = |p: OperationPhase| {
        if let Some(hook) = phase {
            hook(p);
        }
    };
    // Reset the latch when THIS op ends (not at entry) so a cancel racing the
    // op's startup isn't wiped. See AbortGuard.
    let _abort_guard = AbortGuard::new();
    set_phase(OperationPhase::Preparing);
    // A vanished install is itself a stale snapshot: the user confirmed an
    // update against a Codex that is no longer there (deleted / moved between
    // confirm and execute). Route it through StaleExpectation so the UI
    // auto-re-checks (→ none/install) instead of looping on a dead error.
    let installed = detect_managed_installed().ok_or_else(|| {
        AppError::StaleExpectation(
            "未检测到 Codex（可能已被删除或移动）：请重新检查后再试".to_string(),
        )
    })?;
    log::info!(
        "macOS perform start from_build={} to_build={} path={}",
        expected.from_build,
        expected.to_build,
        expected.install_path
    );

    // Consent integrity: the destructive swap must target exactly what the user
    // saw + confirmed. If Codex self-updated (Sparkle), moved, or staging is
    // stale, refuse rather than act on a stale consent.
    if installed.path != expected.install_path {
        let actual = &installed.path;
        log::warn!(
            "macOS perform stale expectation expected_from={} expected_to={} actual_path={actual}",
            expected.from_build,
            expected.to_build
        );
        return Err(AppError::StaleExpectation(format!(
            "安装位置已变化（确认时 {}，现在 {}）：请重新检查后再试",
            expected.install_path, installed.path
        )));
    }
    if installed.build != expected.from_build {
        let actual = installed.build;
        log::warn!(
            "macOS perform stale expectation expected_from={} expected_to={} actual_build={actual}",
            expected.from_build,
            expected.to_build
        );
        return Err(AppError::StaleExpectation(format!(
            "已装版本已变化（确认时 build {}，现在 build {}）：请重新检查后再试",
            expected.from_build, installed.build
        )));
    }

    let install_path = PathBuf::from(&installed.path);

    let (_, xml) = fetch_appcast_for_arch(&installed.arch, network)?;
    let appcast = parse_appcast(&xml).map_err(|e| AppError::Engine(e.to_string()))?;
    let plan = plan_update(&appcast, installed.build)
        .ok_or_else(|| AppError::Engine("appcast had no items".to_string()))?;
    // The appcast fetch is the slow part of "正在准备" — honor a cancel here,
    // before the up-to-date / stale early-returns and the destructive work below.
    check_update_abort()?;

    // The appcast must still point at the build the user confirmed.
    if plan.latest_build != expected.to_build {
        let actual = plan.latest_build;
        log::warn!(
            "macOS perform stale expectation expected_from={} expected_to={} actual_target={actual}",
            expected.from_build,
            expected.to_build
        );
        return Err(AppError::StaleExpectation(format!(
            "更新目标已变化（确认时 build {}，现在 build {}）：请重新检查后再试",
            expected.to_build, plan.latest_build
        )));
    }

    if plan.up_to_date {
        return Ok(MacPerformReport {
            up_to_date: true,
            from_build: installed.build,
            to_build: plan.latest_build,
            strategy: "none".to_string(),
            installed_path: installed.path,
            verified: false,
            relaunched: false,
            relaunch_failed: false,
            rolled_back: false,
            warning: None,
            message: format!("已是最新 (build {})", plan.latest_build),
        });
    }

    if let Some(latest) = appcast.latest() {
        require_os_supported(latest.minimum_system_version.as_deref())?;
    }
    preflight_mac_disk(&plan)?;
    // Last cancel checkpoint before the download begins: the preparing phase
    // (appcast fetch → plan → preflight) is done; once curl starts, the download
    // loop's own cancel flag takes over. A cancel pressed during "正在准备"
    // lands here and bails before any destructive prep.
    check_update_abort()?;
    set_phase(OperationPhase::Downloading);

    // 1) Set up same-volume staging for the reconstructed bundle + backup.
    let staging = staging::create_unique_staging("update")?;
    let work = staging.path().to_path_buf();
    let out_app = work.join("Codex.app");
    let backup = work.join("backup-Codex.app");
    let _ = std::fs::remove_dir_all(&out_app);
    let _ = std::fs::remove_dir_all(&backup);
    let mut keep_staging = false;
    let perform_result = (|| -> Result<MacPerformReport, AppError> {
        // 2) Reconstruct the new bundle into out_app. Prefer a delta when the tool is
        //    present; fall back to the appcast's full package when the tool is missing
        //    OR the delta fails to apply (modified basis, tool/patch version mismatch,
        //    …). The full enclosure is always present in the same appcast entry and
        //    needs no tool, so a recoverable delta failure no longer fails the update.
        let want_delta = matches!(plan.strategy, UpdateStrategy::Delta { .. });
        let effective_strategy = if want_delta {
            if let Some(tool) = binary_delta.as_deref() {
                let sig = plan.ed_signature.clone().ok_or_else(|| {
                    AppError::Engine("appcast delta missing edSignature".to_string())
                })?;
                let staged = download_and_verify(
                    &plan.download_url,
                    plan.download_size,
                    codex_mac_engine::limits::MAX_DELTA_BYTES,
                    &sig,
                    progress,
                    network,
                )?;
                set_phase(OperationPhase::Applying);
                match apply_delta(tool, &install_path, &out_app, &staged) {
                    Ok(()) => strategy_label(&plan.strategy),
                    Err(delta_err) => {
                        reconstruct_full(&appcast, &work, &out_app, progress, network)?;
                        format!("full (delta 应用失败回退: {delta_err})")
                    }
                }
            } else {
                set_phase(OperationPhase::Applying);
                reconstruct_full(&appcast, &work, &out_app, progress, network)?;
                "full (delta 工具缺失，回退全量)".to_string()
            }
        } else {
            set_phase(OperationPhase::Applying);
            reconstruct_full(&appcast, &work, &out_app, progress, network)?;
            "full".to_string()
        };

        // 3) gate the reconstructed bundle before it touches the install root.
        set_phase(OperationPhase::Verifying);
        log::info!("macOS perform step=gate");
        gate_reconstructed(&out_app)
            .map_err(|e| AppError::Engine(format!("codesign 闸失败（拒绝替换）: {e}")))?;

        // 4a) pre-flight the atomic-swap precondition BEFORE quitting Codex, so a
        //     cross-volume staging dir fails fast WITHOUT closing the user's app.
        let install_parent = install_path.parent().unwrap_or(install_path.as_path());
        log::info!("macOS perform step=same-volume-preflight");
        if !codex_mac_engine::swap::same_volume(&out_app, install_parent) {
            log::warn!("macOS perform same-volume preflight failed");
            return Err(AppError::Engine(
                "暂存目录与安装根不在同一卷，无法原子替换：请确保 TMPDIR 与安装根同卷".to_string(),
            ));
        }

        // Honor a cancel before touching the user's running Codex. A second,
        // phase-linearized checkpoint immediately before the swap closes the
        // later quit/cancel window.
        check_update_abort()?;

        // Re-verify the TARGET right before the destructive tail: the early
        // consent check ran before download, and the bundle at this path may
        // have been swapped in the meantime — e.g. replaced with a ChatGPT
        // Classic, which the identity gate inside installed_codex_build_at_path
        // rejects. Quitting/swapping whatever sits here now would destroy
        // something the user never confirmed.
        match sys::installed_codex_build_at_path(&installed.path) {
            Some((_, build)) if build == expected.from_build => {}
            Some((_, build)) => {
                return Err(AppError::StaleExpectation(format!(
                    "安装目标在确认后发生了变化（当前 build {build}，确认时为 {}）：请重新检查后再试",
                    expected.from_build
                )));
            }
            None => {
                return Err(AppError::StaleExpectation(
                    "安装目标在确认后被移除或替换为非 Codex 应用：请重新检查后再试".to_string(),
                ));
            }
        }
        // The attribute can appear after adoption (e.g. the path was replaced
        // by a fresh download) — a translocated live instance would be
        // invisible to the quit check below.
        require_not_translocation_risk(&installed.path)?;

        // 4b) graceful quit (never force-kill), then 5) atomic same-volume swap. If
        //     the swap fails after the quit, swap_in_place has restored the old
        //     bundle in place — bring the user's app back before surfacing the error.
        let was_running = codex_running_at(&install_path);
        log::info!("macOS perform step=quit");
        quit_codex_gracefully(&install_path)?;
        log::info!("macOS perform step=swap");
        set_phase(OperationPhase::Committing);
        // The phase transition and the app quit preparation share the operation
        // mutex. If quit won first its latch is now visible; if commit won first,
        // quit was blocked. No destructive rename occurs between those outcomes.
        if let Err(err) = check_update_abort() {
            if was_running {
                let _ = relaunch(&install_path);
            }
            return Err(err);
        }
        let had_previous = install_path.exists();
        let mut tx = ActiveInstallTx::begin(
            InstallTxKind::MacosSwap,
            &install_path,
            &out_app,
            &backup,
            had_previous,
            Some(was_running),
        )?;
        let mut observer = |boundary: SwapBoundary| -> Result<(), codex_mac_engine::EngineError> {
            match boundary {
                SwapBoundary::BeforeMoveOld => Ok(()),
                SwapBoundary::AfterMoveOld => tx
                    .mark_old_moved()
                    .map_err(|e| codex_mac_engine::EngineError::Io(e.to_string())),
                SwapBoundary::BeforeMoveNew => Ok(()),
                SwapBoundary::AfterMoveNew => tx
                    .mark_new_installed()
                    .map_err(|e| codex_mac_engine::EngineError::Io(e.to_string())),
                SwapBoundary::BeforeRollback => tx
                    .mark_rollback_started()
                    .map_err(|e| codex_mac_engine::EngineError::Io(e.to_string())),
            }
        };
        if let Err(err) =
            swap_in_place_with_observer(&install_path, &out_app, &backup, &mut observer)
        {
            // In-process failure may have rolled back; if still mid-window the
            // Drop of ActiveInstallTx keeps the log for startup recovery.
            if was_running {
                let _ = relaunch(&install_path);
            }
            log::error!("macOS perform swap failed error={err}");
            return Err(AppError::Engine(err.to_string()));
        }
        // Keep the transaction log through post-swap health / rollback — never
        // complete() until that tail settles.
        set_phase(OperationPhase::Finishing);

        // 6) filesystem health check on the installed root.
        log::info!("macOS perform step=health");
        let healthy = sys::installed_codex_build_at_path(&installed.path)
            .map(|(_, build)| build == plan.latest_build)
            .unwrap_or(false)
            && gate_reconstructed(&install_path).is_ok();

        if healthy {
            // The new bundle is authentic + correct-version; record provenance. If
            // the store can't be written (disk full / unwritable data dir), the
            // update still succeeded — but surface it, since status would otherwise
            // keep classifying this now-manager install as "external".
            let mut store = ProvenanceStore::load();
            store.record(
                installed.path.clone(),
                plan.latest_build,
                "manager-installed",
            );
            let save_warning = match store.save() {
                Ok(()) => None,
                Err(e) => Some(format!("托管记录保存失败（{e}），安装暂仍会被识别为外部")),
            };

            if was_running {
                // Relaunch BEFORE discarding the backup: if `open` fails we keep the
                // backup as a recovery path instead of claiming a clean success. We
                // do NOT downgrade a healthy, gated install just because auto-launch
                // failed — the user can launch it manually.
                log::info!("macOS perform step=relaunch");
                if let Err(err) = relaunch(&install_path) {
                    keep_staging = true;
                    // Leave NewInstalled log + backup for recovery / manual use.
                    tx.leave_pending();
                    return Ok(MacPerformReport {
                        up_to_date: false,
                        from_build: installed.build,
                        to_build: plan.latest_build,
                        strategy: effective_strategy.clone(),
                        installed_path: installed.path.clone(),
                        verified: true,
                        relaunched: false,
                        relaunch_failed: true,
                        rolled_back: false,
                        warning: Some(match &save_warning {
                            Some(note) => format!("旧版本备份暂留于 {}；{note}", backup.display()),
                            None => format!("旧版本备份暂留于 {}", backup.display()),
                        }),
                        message: format!(
                            "已替换为 build {}，但自动重启失败（{err}）：请手动启动 Codex",
                            plan.latest_build
                        ),
                    });
                }
            }

            // Not running, or relaunched cleanly → the backup is no longer needed.
            let _ = std::fs::remove_dir_all(&backup);
            let tx_warnings = tx.succeed();
            let final_warning = match (save_warning, tx_warnings.is_empty()) {
                (Some(existing), true) => Some(existing),
                (Some(existing), false) => Some(format!("{existing}；{}", tx_warnings.join("；"))),
                (None, true) => None,
                (None, false) => Some(tx_warnings.join("；")),
            };
            let report = MacPerformReport {
                up_to_date: false,
                from_build: installed.build,
                to_build: plan.latest_build,
                strategy: effective_strategy.clone(),
                installed_path: installed.path.clone(),
                verified: true,
                relaunched: was_running,
                relaunch_failed: false,
                rolled_back: false,
                warning: final_warning,
                message: format!("已更新 build {} → {}", installed.build, plan.latest_build),
            };
            log::info!(
                "macOS perform success relaunched={} to_build={}",
                report.relaunched,
                report.to_build
            );
            Ok(report)
        } else {
            log::warn!("macOS perform step=rollback");
            tx.mark_rollback_started()?;
            match rollback(&install_path, &backup) {
                Ok(()) => {
                    tx.mark_rolled_back()?;
                }
                Err(e) => {
                    // Leave NewInstalled log + materials for manual recovery.
                    tx.leave_pending();
                    return Err(AppError::Engine(format!("回滚失败（需人工介入）: {e}")));
                }
            }
            let relaunched = was_running && relaunch(&install_path).is_ok();
            Ok(MacPerformReport {
                up_to_date: false,
                from_build: installed.build,
                to_build: plan.latest_build,
                strategy: effective_strategy.clone(),
                installed_path: installed.path.clone(),
                verified: true,
                relaunched,
                relaunch_failed: false,
                rolled_back: true,
                warning: None,
                message: "新版本健康检查未通过，已回滚到旧版本".to_string(),
            })
        }
    })();
    match perform_result {
        Ok(report) => {
            if keep_staging {
                let _ = staging.keep();
            } else {
                staging.discard();
            }
            // The artifact was downloaded, verified, and consumed — drop it so a
            // later run re-downloads fresh. A FAILED run leaves the cache intact
            // (the Err arm) so a paused/interrupted download can still resume.
            // Best-effort: a stale artifact left behind is reclaimed by the stale
            // cache sweep, so a cleanup failure must not fail a successful update.
            let _ = staging::clear_download_cache();
            Ok(report)
        }
        Err(err) => {
            log::error!("macOS perform failed error={err} rolled_back=false");
            staging.discard();
            Err(err)
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MacInstallStatus {
    pub installed: Option<InstalledCodex>,
    /// "managed" | "external" | "none"
    pub status: String,
    /// All Codex-lineage installs when more than one exists (e.g. an old
    /// `Codex.app` plus a hand-dragged post-rebrand `ChatGPT.app`). The UI
    /// should surface this and have the user adopt one explicitly; destructive
    /// operations stay safe regardless (they re-verify path + build + identity).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ambiguous_paths: Option<Vec<String>>,
    /// Present after install (or other mutators) that can partial-succeed.
    /// Absent on plain status probes so the contract stays light.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outcome: Option<OperationOutcome>,
}

/// Classify the installed Codex against our provenance store.
pub fn mac_install_status() -> MacInstallStatus {
    let installed = detect_managed_installed();
    let store = ProvenanceStore::load();
    let status = match &installed {
        None => "none",
        // Build-aware: a self-updated or path-reused install no longer matches
        // its record and falls back to "external" (prompting re-adoption).
        Some(codex) if store.is_managed_build(&codex.path, codex.build) => "managed",
        Some(_) => "external",
    }
    .to_string();
    // Report ambient ambiguity (multiple lineage installs) unless provenance
    // already pins which install the user manages.
    let ambiguous_paths = match &installed {
        Some(codex) if !store.is_managed_build(&codex.path, codex.build) => {
            let candidates = sys::installed_codex_candidates();
            (candidates.len() > 1).then(|| candidates.into_iter().map(|(path, _)| path).collect())
        }
        _ => None,
    };
    MacInstallStatus {
        installed,
        status,
        ambiguous_paths,
        outcome: None,
    }
}

/// Adopt the detected install — record provenance after explicit user consent.
pub fn mac_adopt() -> Result<MacInstallStatus, AppError> {
    // Ambient adoption picks the canonical-order first install; with several
    // lineage installs coexisting that silently chooses for the user and every
    // later update/uninstall would act on a target they never confirmed.
    // Force the explicit path-picking flow instead.
    let mut candidates = sys::installed_codex_candidates();
    if candidates.len() > 1 {
        let paths: Vec<String> = candidates.into_iter().map(|(path, _)| path).collect();
        return Err(AppError::Internal(format!(
            "检测到多个 Codex 安装（{}）。请使用「选择已安装的 Codex」手动指定要管理的那一个",
            paths.join("、")
        )));
    }
    // Adopt exactly the install the ambiguity check saw — a second scan could
    // pick a DIFFERENT path if another install appeared in between, recording
    // provenance for something the user never looked at.
    let (path, build) = candidates
        .pop()
        .ok_or_else(|| AppError::Internal("no Codex detected to adopt".to_string()))?;
    let installed = installed_from_path_build(path, build);
    // Same gate as the manual picker — ambient adoption must not manage a
    // bundle whose running instance cannot be located.
    require_not_translocation_risk(&installed.path)?;
    let path = &installed.path;
    log::info!("macOS adopt external install path={path}");
    let mut store = ProvenanceStore::load();
    store.record(installed.path.clone(), installed.build, "adopted-external");
    store.save()?;
    Ok(mac_install_status())
}

pub fn mac_adopt_path(path: &Path) -> Result<MacInstallStatus, AppError> {
    let installed = detect_existing_install_at_path(path)?;
    let install_path = &installed.path;
    log::info!("macOS adopt selected install path={install_path}");
    let mut store = ProvenanceStore::load();
    store.record(installed.path.clone(), installed.build, "adopted-external");
    store.save()?;
    Ok(mac_install_status())
}

/// Can we create (and remove) a file in `dir`? Used to decide whether the
/// system `/Applications` is writable before attempting an install there.
fn dir_writable(dir: &Path) -> bool {
    let probe = dir.join(".cam-write-probe");
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Choose the install directory: `/Applications` when writable, otherwise the
/// no-admin `~/Applications` (created if needed). Lets non-admin / managed Macs
/// install without elevation.
fn choose_install_dir() -> Result<PathBuf, AppError> {
    let system = PathBuf::from("/Applications");
    if dir_writable(&system) {
        return Ok(system);
    }
    let home =
        std::env::var("HOME").map_err(|_| AppError::Engine("找不到用户主目录".to_string()))?;
    let user_apps = PathBuf::from(home).join("Applications");
    std::fs::create_dir_all(&user_apps)
        .map_err(|e| AppError::Engine(format!("创建 ~/Applications 失败: {e}")))?;
    Ok(user_apps)
}

/// Fresh install: download the appcast's full package, verify + gate it, and
/// place it under `/Applications` (or `~/Applications` when the system folder
/// isn't writable). No delta, no quit (nothing running), no backup (nothing to
/// replace). Records `manager-installed` provenance and launches the app.
pub fn install_macos(progress: &dyn Fn(DownloadProgress)) -> Result<MacInstallStatus, AppError> {
    install_macos_with_network(progress, &NetworkConfig::system())
}

pub fn install_macos_with_network(
    progress: &dyn Fn(DownloadProgress),
    network: &NetworkConfig,
) -> Result<MacInstallStatus, AppError> {
    install_macos_with_network_and_phase(progress, network, None)
}

pub fn install_macos_with_network_and_phase(
    progress: &dyn Fn(DownloadProgress),
    network: &NetworkConfig,
    phase_hook: Option<&PhaseHook<'_>>,
) -> Result<MacInstallStatus, AppError> {
    install_macos_with_network_and_phase_for_target(progress, network, phase_hook, None)
}

/// Same fresh-install path with an optional latest-build consent guard. The UI
/// supplies it for resumable operations so an appcast refresh cannot redirect a
/// paused download to a newer build the user never started.
pub fn install_macos_with_network_and_phase_for_target(
    progress: &dyn Fn(DownloadProgress),
    network: &NetworkConfig,
    phase_hook: Option<&PhaseHook<'_>>,
    expected_target_build: Option<u64>,
) -> Result<MacInstallStatus, AppError> {
    log::info!("macOS install start");
    // Reset on op end, not entry — race-free cancel (see AbortGuard).
    let _abort_guard = AbortGuard::new();
    if detect_installed().is_some() {
        return Err(AppError::Engine(
            "已检测到 Codex,请使用更新而非安装".to_string(),
        ));
    }

    let install_dir = choose_install_dir()?;
    let install_path = install_dir.join("Codex.app");

    let arch = if std::env::consts::ARCH == "aarch64" {
        "arm64"
    } else {
        "x86_64"
    };
    let (_, xml) = fetch_appcast_for_arch(arch, network)?;
    let appcast = parse_appcast(&xml).map_err(|e| AppError::Engine(e.to_string()))?;
    if let Some(latest) = appcast.latest() {
        require_os_supported(latest.minimum_system_version.as_deref())?;
    }
    let plan = plan_update(&appcast, 0)
        .ok_or_else(|| AppError::Engine("appcast had no items".to_string()))?;
    require_expected_latest_build(expected_target_build, plan.latest_build)?;
    preflight_mac_disk(&plan)?;
    // Cancel checkpoint before bytes flow (mirrors perform) — makes a fresh
    // install's "正在准备" cancellable too.
    check_update_abort()?;

    let staging = staging::create_unique_staging("update")?;
    let install_result = install_macos_in_staging(
        &appcast,
        &install_dir,
        &install_path,
        &staging,
        progress,
        network,
        phase_hook,
    );
    match install_result {
        Ok(status) => {
            staging.discard();
            // Consumed on success — clear it (best-effort; the stale sweep
            // reclaims any leftover). A failed install keeps the cached partial
            // (Err arm) so the next attempt resumes instead of restarting.
            let _ = staging::clear_download_cache();
            let build = status.installed.as_ref().map(|installed| installed.build);
            log::info!("macOS install complete build={build:?}");
            Ok(status)
        }
        Err(err) => {
            staging.discard();
            log::error!("macOS install failed error={err}");
            Err(err)
        }
    }
}

fn require_expected_latest_build(expected: Option<u64>, actual: u64) -> Result<(), AppError> {
    if let Some(expected) = expected {
        if expected != actual {
            return Err(AppError::StaleExpectation(format!(
                "macOS latest release changed before install (expected build {expected}, found {actual}); please re-check and confirm again."
            )));
        }
    }
    Ok(())
}

fn install_macos_in_staging(
    appcast: &Appcast,
    install_dir: &Path,
    install_path: &Path,
    staging: &StagingDir,
    progress: &dyn Fn(DownloadProgress),
    network: &NetworkConfig,
    phase_hook: Option<&PhaseHook<'_>>,
) -> Result<MacInstallStatus, AppError> {
    let work = staging.path();
    let out_app = staging.join("Codex.app");
    let _ = std::fs::remove_dir_all(&out_app);

    // Full package only — no basis bundle to delta against.
    reconstruct_full(appcast, work, &out_app, progress, network)?;
    if let Some(hook) = phase_hook {
        hook(OperationPhase::Verifying);
    }
    gate_reconstructed(&out_app)
        .map_err(|e| AppError::Engine(format!("codesign 闸失败（拒绝安装）: {e}")))?;

    if !codex_mac_engine::swap::same_volume(&out_app, install_dir) {
        return Err(AppError::Engine(format!(
            "暂存目录与 {} 不在同一卷,无法安装",
            install_dir.display()
        )));
    }
    // Publish the point-of-no-return while the same operation token is still
    // held, then perform one final cancel checkpoint. A cancel that won the
    // operation mutex first leaves its latch for this checkpoint; a later cancel
    // sees Committing and is rejected instead of racing the rename.
    if let Some(hook) = phase_hook {
        hook(OperationPhase::Committing);
    }
    check_update_abort()?;
    std::fs::rename(&out_app, install_path)
        .map_err(|e| AppError::Engine(format!("写入 {} 失败: {e}", install_dir.display())))?;
    if let Some(hook) = phase_hook {
        hook(OperationPhase::Finishing);
    }

    let mut outcome = OperationOutcome::full_success("present", Some("managed"));
    outcome.cleanup = StepOutcome::not_applicable();
    if let Some((path, build)) = sys::installed_codex_build() {
        outcome.path = Some(path.clone());
        let mut store = ProvenanceStore::load();
        store.record(path, build, "manager-installed");
        // The app is on disk now. Provenance save is ancillary: never turn a
        // landed install into a hard failure (that would invite a re-install).
        if let Err(e) = store.save() {
            let detail = format!("托管记录保存失败（{e}）");
            outcome.provenance = StepOutcome::failed(detail.clone());
            outcome.push_warning(format!(
                "{detail}；应用已在磁盘上，请用「开始管理」重试写入托管记录，勿重复安装"
            ));
            outcome.push_recovery(recovery::RECORD_PROVENANCE);
            outcome.install_class = Some("external".to_string());
        }
    } else {
        outcome.path = Some(install_path.to_string_lossy().into_owned());
        outcome.provenance = StepOutcome::failed("安装后未能读取 bundle 版本，托管记录未写入");
        outcome
            .push_warning("已写入应用，但未能确认版本；请重新检查状态或「开始管理」".to_string());
        outcome.push_recovery(recovery::RECORD_PROVENANCE);
        outcome.install_class = Some("external".to_string());
        // Honest: app may be present but we couldn't classify it as managed.
        outcome.app_state = "present".to_string();
    }
    // Do NOT auto-launch — the UI shows a completion state with an explicit
    // 〔打开 Codex〕 button so opening is the user's choice, not a surprise.
    let mut status = mac_install_status();
    // Prefer the live classification after the attempted record write.
    if outcome.provenance.is_failed() {
        status.status = "external".to_string();
    }
    status.outcome = Some(outcome);
    Ok(status)
}

#[derive(Debug, Clone)]
pub struct HistoricalMacExpectation {
    pub current_path: Option<String>,
    pub current_build: Option<u64>,
}

fn validate_historical_mac_expectation(
    expected: &HistoricalMacExpectation,
    current: Option<&InstalledCodex>,
) -> Result<(), AppError> {
    let actual_path = current.map(|installed| installed.path.as_str());
    let actual_build = current.map(|installed| installed.build);
    if actual_path != expected.current_path.as_deref() || actual_build != expected.current_build {
        return Err(AppError::StaleExpectation(format!(
            "Codex 在确认后发生变化（确认时 {:?} build {:?}，现在 {:?} build {:?}），请重新检查后再试",
            expected.current_path, expected.current_build, actual_path, actual_build
        )));
    }
    Ok(())
}

fn preflight_historical_mac_disk(package_size: u64) -> Result<(), AppError> {
    const HEADROOM: u64 = 512 * 1024 * 1024;
    let need = package_size.saturating_mul(4).saturating_add(HEADROOM);
    if let Some(free) = disk::available_space(&staging::staging_root())? {
        if free < need {
            return Err(AppError::Engine(format!(
                "磁盘可用空间不足：历史版本安装约需 {}，当前可用 {}。请清理后重试",
                human_bytes(need),
                human_bytes(free)
            )));
        }
    }
    Ok(())
}

/// A fresh install uses one atomic rename. A failed rename is definitive
/// no-mutation evidence, so publish `MutationStarted` only after it succeeds.
/// A worker panic after the syscall is still handled conservatively by the
/// command layer's join-error path.
fn commit_fresh_historical_app(
    staged_app: &Path,
    install_path: &Path,
    evidence: Option<&HistoricalEvidenceHook<'_>>,
    tx: Option<&mut ActiveInstallTx>,
) -> Result<(), AppError> {
    std::fs::rename(staged_app, install_path)
        .map_err(|e| AppError::Engine(format!("写入 {} 失败: {e}", install_path.display())))?;
    if let Some(hook) = evidence {
        hook(HistoricalMacEvidence::MutationStarted);
    }
    if let Some(tx) = tx {
        // If this durable mark fails, the Prepared journal plus the fresh-install
        // disk matrix (target exists, payload absent) still proves the rename
        // landed and startup recovery will finalize the requested policy.
        tx.mark_new_installed()?;
    }
    Ok(())
}

/// Finish a confirmed rollback in the only safe order: restore the policy that
/// the old app should inherit, then relaunch it. If policy restoration fails,
/// leave the old app stopped instead of launching it with the temporary setting;
/// the command layer will make one final restore attempt before returning.
fn finish_historical_rollback(
    primary: AppError,
    was_running: bool,
    after_rollback: Option<&AfterHistoricalRollbackHook<'_>>,
    relaunch_old: impl FnOnce() -> Result<(), codex_mac_engine::EngineError>,
) -> AppError {
    if let Some(hook) = after_rollback {
        if let Err(restore) = hook() {
            return AppError::Internal(format!(
                "{primary}；回滚后恢复安装前的 Codex 自更新策略失败：{restore}"
            ));
        }
    }
    if was_running {
        if let Err(error) = relaunch_old() {
            log::warn!("relaunch rolled-back Codex failed error={error}");
        }
    }
    primary
}

fn normalize_release_arch(value: &str) -> Option<ReleaseArchitecture> {
    ReleaseArchitecture::from_runtime(value)
}

fn read_bundle_minimum_system_version(app: &Path) -> Option<String> {
    let plist = app.join("Contents/Info.plist");
    let output = std::process::Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print :LSMinimumSystemVersion"])
        .arg(plist)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Inspect a local DMG/ZIP without any network access. The mounted/extracted
/// bundle must pass the same OpenAI code-signing, Gatekeeper/notarization,
/// identity, architecture and OS gates as the real installer.
pub fn resolve_macos_local_package(
    path: &Path,
    architecture: ReleaseArchitecture,
) -> Result<ResolvedReleaseAsset, AppError> {
    let format = match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("dmg") => ReleasePackageFormat::Dmg,
        Some("zip") => ReleasePackageFormat::Zip,
        _ => {
            return Err(AppError::Internal(
                "请选择从 GitHub Release 下载的 DMG 或 ZIP 文件".to_string(),
            ))
        }
    };
    let staging = staging::create_unique_staging("inspect")?;
    let out_app = staging.join("Codex.app");
    let result = (|| -> Result<ResolvedReleaseAsset, AppError> {
        match format {
            ReleasePackageFormat::Dmg => {
                unpack_app_dmg(path, staging.path(), &out_app)?;
            }
            ReleasePackageFormat::Zip => {
                unpack_app_zip(path, staging.path(), &out_app)?;
            }
            ReleasePackageFormat::Msix => unreachable!(),
        }
        gate_reconstructed(&out_app)
            .map_err(|e| AppError::Engine(format!("本地安装包代码签名/公证验证失败: {e}")))?;
        let version = sys::read_bundle_short_version(&out_app.to_string_lossy())
            .ok_or_else(|| AppError::Engine("无法读取本地 Codex 安装包版本".to_string()))?;
        let actual_arch = sys::app_arch(&out_app.to_string_lossy())
            .and_then(|value| normalize_release_arch(&value))
            .ok_or_else(|| AppError::Engine("无法识别本地 Codex 安装包架构".to_string()))?;
        if actual_arch != architecture {
            return Err(AppError::Engine(format!(
                "本地安装包架构 {} 与所选设备架构 {} 不一致",
                actual_arch.as_str(),
                architecture.as_str()
            )));
        }
        require_os_supported(read_bundle_minimum_system_version(&out_app).as_deref())?;
        resolved_local_asset(path, &version, actual_arch, format, None)
    })();
    staging.discard();
    result
}

/// Install a specific DMG/ZIP from the mirror repository's GitHub Releases.
/// The caller has already rebound the selected tag/name to a fresh API response
/// and checked that GitHub's asset digest agrees with SHA256SUMS. This function
/// still hashes the actual local/downloaded bytes, gates the extracted app, and
/// re-checks both the source package and current install immediately before the
/// atomic swap.
#[allow(clippy::too_many_arguments)]
pub fn install_macos_historical_release(
    resolved: ResolvedReleaseAsset,
    local_path: Option<PathBuf>,
    expected: HistoricalMacExpectation,
    progress: &dyn Fn(DownloadProgress),
    network: &NetworkConfig,
    phase: Option<&PhaseHook<'_>>,
    policy_transition: Option<SelfUpdatePolicyTransition>,
    before_commit: Option<&BeforeHistoricalCommitHook<'_>>,
    evidence: Option<&HistoricalEvidenceHook<'_>>,
    after_rollback: Option<&AfterHistoricalRollbackHook<'_>>,
) -> Result<MacInstallStatus, AppError> {
    let set_phase = |value: OperationPhase| {
        if let Some(hook) = phase {
            hook(value);
        }
    };
    let _abort_guard = AbortGuard::new();
    set_phase(OperationPhase::Preparing);

    if !matches!(
        resolved.format,
        ReleasePackageFormat::Dmg | ReleasePackageFormat::Zip
    ) {
        return Err(AppError::Internal(
            "macOS 历史安装仅支持 GitHub Release 的 DMG/ZIP".to_string(),
        ));
    }
    let initial_status = mac_install_status();
    if initial_status.status == "external" {
        return Err(AppError::StaleExpectation(
            "当前 Codex 尚未由管理器托管，请先“开始管理”再更换版本".to_string(),
        ));
    }
    let initial = initial_status.installed;
    validate_historical_mac_expectation(&expected, initial.as_ref())?;
    preflight_historical_mac_disk(resolved.size)?;
    check_update_abort()?;

    let is_local = local_path.is_some();
    let package = if let Some(path) = local_path {
        let metadata = std::fs::metadata(&path)
            .map_err(|e| AppError::Engine(format!("读取本地安装包失败: {e}")))?;
        if metadata.len() != resolved.size {
            return Err(AppError::StaleExpectation(format!(
                "本地安装包在确认后发生变化（大小 {} → {}），请重新选择",
                resolved.size,
                metadata.len()
            )));
        }
        verify_file_digest(&path, &resolved.digest)?;
        path
    } else {
        set_phase(OperationPhase::Downloading);
        let dest = staging::download_cache_path(&resolved.url, &resolved.name)?;
        let cached_ok = std::fs::metadata(&dest)
            .map(|metadata| metadata.len() == resolved.size)
            .unwrap_or(false)
            && verify_file_digest(&dest, &resolved.digest).is_ok();
        if !cached_ok {
            if dest.exists() {
                let _ = std::fs::remove_file(&dest);
            }
            let source = host_of(&resolved.url);
            download::download_to_with_progress_bounded_with_network(
                &resolved.url,
                &dest,
                codex_mac_engine::limits::MAX_PACKAGE_BYTES,
                &|downloaded| {
                    progress(DownloadProgress {
                        downloaded,
                        total: resolved.size,
                        source: source.clone(),
                    });
                },
                network,
            )
            .map_err(|e| AppError::Engine(e.to_string()))?;
        } else {
            progress(DownloadProgress {
                downloaded: resolved.size,
                total: resolved.size,
                source: host_of(&resolved.url),
            });
        }
        let actual_size = std::fs::metadata(&dest)
            .map_err(|e| AppError::Engine(format!("读取下载包信息失败: {e}")))?
            .len();
        if actual_size != resolved.size {
            return Err(AppError::Engine(format!(
                "下载包大小不匹配（{actual_size} != {}）",
                resolved.size
            )));
        }
        verify_file_digest(&dest, &resolved.digest)?;
        dest
    };

    check_update_abort()?;
    let staging = staging::create_unique_staging("update")?;
    let out_app = staging.join("Codex.app");
    let backup = staging.join("backup-Codex.app");
    let work = staging.path().to_path_buf();
    let mut keep_staging = false;
    let install_result = (|| -> Result<MacInstallStatus, AppError> {
        set_phase(OperationPhase::Applying);
        match resolved.format {
            ReleasePackageFormat::Dmg => unpack_app_dmg(&package, &work, &out_app)?,
            ReleasePackageFormat::Zip => unpack_app_zip(&package, &work, &out_app)?,
            ReleasePackageFormat::Msix => unreachable!("validated above"),
        }

        set_phase(OperationPhase::Verifying);
        gate_reconstructed(&out_app).map_err(|e| {
            AppError::Engine(format!("codesign/Gatekeeper 闸失败（拒绝安装）: {e}"))
        })?;
        let (_, target_build) = sys::installed_codex_build_at_path(&out_app.to_string_lossy())
            .ok_or_else(|| AppError::Engine("无法读取待安装 Codex 的 bundle build".to_string()))?;
        let target_version = sys::read_bundle_short_version(&out_app.to_string_lossy())
            .ok_or_else(|| AppError::Engine("无法读取待安装 Codex 的版本号".to_string()))?;
        if target_version != resolved.version {
            return Err(AppError::Engine(format!(
                "安装包内版本 {target_version} 与 Release {} 不一致，拒绝安装",
                resolved.version
            )));
        }
        let target_arch = sys::app_arch(&out_app.to_string_lossy())
            .and_then(|value| normalize_release_arch(&value))
            .ok_or_else(|| AppError::Engine("无法识别待安装 Codex 的架构".to_string()))?;
        if target_arch != resolved.architecture {
            return Err(AppError::Engine(format!(
                "安装包内架构 {} 与 Release 资产架构 {} 不一致，拒绝安装",
                target_arch.as_str(),
                resolved.architecture.as_str()
            )));
        }
        let minimum = read_bundle_minimum_system_version(&out_app);
        require_os_supported(minimum.as_deref())?;

        // Re-detect after the potentially long download/extract/gate window.
        let current_status = mac_install_status();
        if current_status.status == "external" {
            return Err(AppError::StaleExpectation(
                "当前安装在确认后变为外部安装，请重新检查并先开始管理".to_string(),
            ));
        }
        validate_historical_mac_expectation(&expected, current_status.installed.as_ref())?;
        let install_path = match current_status.installed.as_ref() {
            Some(installed) => PathBuf::from(&installed.path),
            None => choose_install_dir()?.join("Codex.app"),
        };
        let install_parent = install_path.parent().unwrap_or(install_path.as_path());
        if !codex_mac_engine::swap::same_volume(&out_app, install_parent) {
            return Err(AppError::Engine(
                "暂存目录与安装根不在同一卷，无法原子安装".to_string(),
            ));
        }
        if let Some(installed) = current_status.installed.as_ref() {
            match sys::installed_codex_build_at_path(&installed.path) {
                Some((_, build)) if Some(build) == expected.current_build => {}
                _ => {
                    return Err(AppError::StaleExpectation(
                        "安装目标在确认后被移动、替换或更新，请重新检查后再试".to_string(),
                    ));
                }
            }
            require_not_translocation_risk(&installed.path)?;
        } else if detect_installed().is_some() {
            return Err(AppError::StaleExpectation(
                "确认后检测到新的 Codex 安装，请重新检查后再试".to_string(),
            ));
        }

        // No cancel is accepted after this phase. Persist crash-recovery
        // ownership (including the old/requested update policy) before changing
        // launchctl/settings or stopping/moving the installed app.
        set_phase(OperationPhase::Committing);
        check_update_abort()?;
        let had_previous = current_status.installed.is_some();
        let was_running = had_previous && codex_running_at(&install_path);
        let mut tx = ActiveInstallTx::begin(
            InstallTxKind::MacosSwap,
            &install_path,
            &out_app,
            &backup,
            had_previous,
            Some(was_running),
        )?;
        if let Some(transition) = policy_transition {
            tx.set_self_update_policy_transition(transition)?;
        }
        if let Some(hook) = before_commit {
            hook()?;
        }
        if had_previous {
            // Establish crash-recovery ownership before stopping a running app.
            // If journal creation fails, the user's existing Codex stays open.
            quit_codex_gracefully(&install_path)?;
            let swap_result = {
                let mut observer =
                    |boundary: SwapBoundary| -> Result<(), codex_mac_engine::EngineError> {
                        match boundary {
                            SwapBoundary::BeforeMoveOld | SwapBoundary::BeforeMoveNew => Ok(()),
                            SwapBoundary::AfterMoveOld => {
                                if let Some(hook) = evidence {
                                    hook(HistoricalMacEvidence::MutationStarted);
                                }
                                tx.mark_old_moved()
                                    .map_err(|e| codex_mac_engine::EngineError::Io(e.to_string()))
                            }
                            SwapBoundary::AfterMoveNew => tx
                                .mark_new_installed()
                                .map_err(|e| codex_mac_engine::EngineError::Io(e.to_string())),
                            SwapBoundary::BeforeRollback => tx
                                .mark_rollback_started()
                                .map_err(|e| codex_mac_engine::EngineError::Io(e.to_string())),
                        }
                    };
                swap_in_place_with_observer(&install_path, &out_app, &backup, &mut observer)
            };
            if let Err(err) = swap_result {
                let primary = AppError::Engine(format!("历史版本原子替换失败: {err}"));
                if historical_swap_needs_recovery_materials(&install_path, &backup) {
                    // The backup may be the only remaining runnable copy. Keep
                    // both it and the pending journal; startup recovery owns the
                    // next move and staging cleanup must not delete either.
                    tx.leave_pending();
                    keep_staging = true;
                } else {
                    // The engine restored the old install in place and consumed
                    // the backup. Record that terminal truth so no stale pending
                    // journal survives a clean in-process rollback.
                    tx.mark_rolled_back()?;
                    if let Some(hook) = evidence {
                        hook(HistoricalMacEvidence::MutationRolledBack);
                    }
                    return Err(finish_historical_rollback(
                        primary,
                        was_running,
                        after_rollback,
                        || relaunch(&install_path),
                    ));
                }
                return Err(primary);
            }
            set_phase(OperationPhase::Finishing);
            let healthy = sys::installed_codex_build_at_path(&install_path.to_string_lossy())
                .map(|(_, build)| build == target_build)
                .unwrap_or(false)
                && sys::read_bundle_short_version(&install_path.to_string_lossy()).as_deref()
                    == Some(resolved.version.as_str())
                && gate_reconstructed(&install_path).is_ok();
            if !healthy {
                tx.mark_rollback_started()?;
                match rollback(&install_path, &backup) {
                    Ok(()) => {
                        tx.mark_rolled_back()?;
                        if let Some(hook) = evidence {
                            hook(HistoricalMacEvidence::MutationRolledBack);
                        }
                        return Err(finish_historical_rollback(
                            AppError::Engine(
                                "历史版本安装后健康检查失败，已回滚到原版本".to_string(),
                            ),
                            was_running,
                            after_rollback,
                            || relaunch(&install_path),
                        ));
                    }
                    Err(err) => {
                        tx.leave_pending();
                        keep_staging = true;
                        return Err(AppError::Engine(format!(
                            "历史版本安装后健康检查失败且回滚失败（需人工介入）: {err}"
                        )));
                    }
                }
            }

            let mut outcome = OperationOutcome::full_success("present", Some("managed"))
                .with_path(install_path.to_string_lossy().into_owned());
            outcome.cleanup = StepOutcome::not_applicable();
            let mut store = ProvenanceStore::load();
            store.record(
                install_path.to_string_lossy().into_owned(),
                target_build,
                if is_local {
                    "manager-installed-local-signed-package"
                } else {
                    "manager-installed-github-release"
                },
            );
            if let Err(err) = store.save() {
                let detail = format!("托管记录保存失败（{err}）");
                outcome.provenance = StepOutcome::failed(detail.clone());
                outcome.install_class = Some("external".to_string());
                outcome.push_warning(format!(
                    "{detail}；版本已安装，请用“开始管理”重试写入托管记录，勿重复安装"
                ));
                outcome.push_recovery(recovery::RECORD_PROVENANCE);
            }
            if was_running {
                if let Err(err) = relaunch(&install_path) {
                    keep_staging = true;
                    tx.leave_pending();
                    outcome.push_warning(format!(
                        "已安装 {}，但自动重新打开失败（{err}）；旧版本备份暂留于 {}",
                        resolved.version,
                        backup.display()
                    ));
                } else {
                    let _ = std::fs::remove_dir_all(&backup);
                    for warning in tx.succeed() {
                        outcome.push_warning(warning);
                    }
                }
            } else {
                let _ = std::fs::remove_dir_all(&backup);
                for warning in tx.succeed() {
                    outcome.push_warning(warning);
                }
            }
            let mut status = mac_install_status();
            if outcome.provenance.is_failed() {
                status.status = "external".to_string();
            }
            status.outcome = Some(outcome);
            Ok(status)
        } else {
            // A fresh install is one atomic rename, but still carries a durable
            // journal so a crash after changing the requested update policy (or
            // between rename and NewInstalled) can be resolved from disk truth.
            commit_fresh_historical_app(&out_app, &install_path, evidence, Some(&mut tx))?;
            set_phase(OperationPhase::Finishing);
            let healthy = sys::installed_codex_build_at_path(&install_path.to_string_lossy())
                .map(|(_, build)| build == target_build)
                .unwrap_or(false)
                && gate_reconstructed(&install_path).is_ok();
            if !healthy {
                tx.mark_rollback_started()?;
                std::fs::remove_dir_all(&install_path).map_err(|e| {
                    AppError::Engine(format!("新安装健康检查失败，且清理失败（需人工介入）: {e}"))
                })?;
                if let Some(hook) = evidence {
                    hook(HistoricalMacEvidence::MutationRolledBack);
                }
                tx.mark_rolled_back()?;
                return Err(AppError::Engine(
                    "新安装健康检查失败，已移除无效安装".to_string(),
                ));
            }
            let mut outcome = OperationOutcome::full_success("present", Some("managed"))
                .with_path(install_path.to_string_lossy().into_owned());
            outcome.cleanup = StepOutcome::not_applicable();
            let mut store = ProvenanceStore::load();
            store.record(
                install_path.to_string_lossy().into_owned(),
                target_build,
                if is_local {
                    "manager-installed-local-signed-package"
                } else {
                    "manager-installed-github-release"
                },
            );
            if let Err(err) = store.save() {
                let detail = format!("托管记录保存失败（{err}）");
                outcome.provenance = StepOutcome::failed(detail.clone());
                outcome.install_class = Some("external".to_string());
                outcome.push_warning(format!(
                    "{detail}；版本已安装，请用“开始管理”重试写入托管记录，勿重复安装"
                ));
                outcome.push_recovery(recovery::RECORD_PROVENANCE);
            }
            let mut status = mac_install_status();
            if outcome.provenance.is_failed() {
                status.status = "external".to_string();
            }
            for warning in tx.succeed() {
                outcome.push_warning(warning);
            }
            status.outcome = Some(outcome);
            Ok(status)
        }
    })();

    match install_result {
        Ok(status) => {
            if keep_staging {
                let _ = staging.keep();
            } else {
                staging.discard();
            }
            if !is_local {
                let _ = staging::clear_download_cache();
            }
            Ok(status)
        }
        Err(err) => {
            if keep_staging {
                let _ = staging.keep();
            } else {
                staging.discard();
            }
            Err(err)
        }
    }
}

fn historical_swap_needs_recovery_materials(install_path: &Path, backup: &Path) -> bool {
    backup.exists() || !install_path.exists()
}

/// Open the installed Codex.app — invoked by the UI's explicit 〔打开 Codex〕
/// action (we no longer auto-launch after a fresh install).
pub fn launch_codex() -> Result<(), AppError> {
    let installed = detect_managed_installed()
        .ok_or_else(|| AppError::Engine("没有可打开的 Codex".to_string()))?;
    let settings = crate::app::settings_store::AppSettings::load();
    if settings.disable_codex_self_updates {
        crate::app::codex_self_update::sync_setting(true)?;
    }
    let path = &installed.path;
    log::info!("macOS launch Codex path={path}");
    let mut command = std::process::Command::new(OPEN);
    crate::app::codex_self_update::apply_to_command(
        &mut command,
        settings.disable_codex_self_updates,
    );
    command
        .arg(&installed.path)
        .spawn()
        .map(|_| ())
        .map_err(|e| {
            log::warn!("macOS launch Codex failed error={e}");
            AppError::Engine(format!("打开 Codex 失败: {e}"))
        })
}

/// Preparing-phase abort latch. The download loop has its own cancel flag once
/// curl is running, but everything BEFORE the first byte — appcast fetch,
/// planning, disk preflight — used to be an uncancellable wait. This latch lets
/// a cancel pressed during "正在准备" be honored at the next checkpoint.
static UPDATE_ABORT: AtomicBool = AtomicBool::new(false);

pub(crate) fn clear_update_abort() {
    UPDATE_ABORT.store(false, Ordering::SeqCst);
}

/// Resets the abort latch when the operation that owns it ends — on EVERY path
/// (success, error, early return, panic). Clearing on DROP (not at entry) is what
/// makes the latch race-free: a cancel that lands in the window between the UI
/// showing its cancel button and this operation reaching its first checkpoint is
/// NOT wiped by an entry-clear, so the checkpoint still observes it; the latch is
/// reset only once this operation is done, leaving the next one clean. The cancel
/// command now binds the latch to the owning operation under the op lock, but the
/// worker may still be between entry and its first checkpoint — hence the guard
/// instead of an entry reset.
pub(crate) struct AbortGuard;

static UPDATE_ABORT_GUARD_DEPTH: AtomicUsize = AtomicUsize::new(0);

impl AbortGuard {
    pub(crate) fn new() -> Self {
        UPDATE_ABORT_GUARD_DEPTH.fetch_add(1, Ordering::SeqCst);
        Self
    }
}

impl Drop for AbortGuard {
    fn drop(&mut self) {
        let previous = UPDATE_ABORT_GUARD_DEPTH.fetch_sub(1, Ordering::SeqCst);
        debug_assert!(previous > 0, "AbortGuard depth underflow");
        if previous == 1 {
            clear_update_abort();
        }
    }
}

/// Bail out of the preparing phase when the user cancelled. Surfaces the same
/// "download cancelled" marker the curl-cancel path uses, so the UI treats it
/// as a cancel (routes home + cancelled notice) uniformly.
fn check_update_abort() -> Result<(), AppError> {
    if UPDATE_ABORT.load(Ordering::SeqCst) {
        Err(AppError::Engine("download cancelled".to_string()))
    } else {
        Ok(())
    }
}

pub fn pause_macos_download() -> bool {
    // Pause is only offered once bytes are flowing (UI disables it during
    // preparing), so it stays a pure download-loop operation — keep the `.part`.
    let requested = download::pause_active_download();
    log::info!("macOS pause download requested={requested}");
    requested
}

pub fn cancel_macos_download() -> bool {
    // Latch the preparing-phase abort too: a cancel pressed before the first
    // byte (or mid appcast-fetch) is honored at the next checkpoint, not just an
    // already-running curl. Report actionable unconditionally — during preparing
    // the latch IS the cancel mechanism, so the UI must not say "不能取消".
    UPDATE_ABORT.store(true, Ordering::SeqCst);
    let requested = download::cancel_active_download();
    log::info!("macOS cancel download requested={requested}");
    true
}

/// Paused-state cancel: the download already stopped and its `.part` is on disk.
/// Clear the cache so "继续" can't resume, and drop the abort latch. Surfaces a
/// removal failure (vs. silently reporting a cancel that left the partial behind,
/// which a later run would then resume).
pub fn discard_macos_download() -> Result<(), AppError> {
    clear_update_abort();
    staging::clear_download_cache()
        .map_err(|e| AppError::Internal(format!("清理下载缓存失败: {e}")))?;
    log::info!("macOS discard download cache");
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MacUninstallReport {
    pub removed: bool,
    pub kept_codex_home: bool,
    pub message: String,
    /// Structured primary/ancillary result for partial-success recovery.
    pub outcome: OperationOutcome,
}

/// Remove the installed Codex app. Only uninstalls an install THIS manager
/// manages — an external (official / manual) install must be explicitly adopted
/// first, so we never delete something we didn't create. `keep_codex_home` is
/// true by default at the UI: the user's `~/.codex` (sign-in, sessions, config)
/// survives unless they explicitly opt out. Quits Codex first (never force-kills).
pub fn uninstall_macos(keep_codex_home: bool) -> Result<MacUninstallReport, AppError> {
    log::info!("macOS uninstall start keep_codex_home={keep_codex_home}");
    let installed = detect_managed_installed()
        .ok_or_else(|| AppError::Engine("no Codex detected to uninstall".to_string()))?;
    let install_path = PathBuf::from(&installed.path);

    // Boundary: refuse to delete anything that isn't an install we manage at this
    // exact build (path-only matching could delete a path-reused external install
    // or one left by a stale record).
    let mut store = ProvenanceStore::load();
    if !store.is_managed_build(&installed.path, installed.build) {
        return Err(AppError::Engine(
            "这是外部安装的 Codex,或版本与托管记录不一致。请先在主界面「开始管理」纳入管理后再卸载。"
                .to_string(),
        ));
    }

    // A translocated live instance would be invisible to the quit check —
    // re-verify before the destructive delete, same as perform.
    require_not_translocation_risk(&installed.path)?;
    quit_codex_gracefully(&install_path)?;

    // Delete first: if we lack permission to remove the bundle (e.g. a root-owned
    // install), the managed record stays intact so the user can retry without
    // re-adopting.
    std::fs::remove_dir_all(&install_path)
        .map_err(|e| AppError::Engine(format!("remove app bundle: {e}")))?;

    let mut outcome =
        OperationOutcome::full_success("absent", Some("none")).with_path(installed.path.clone());

    // The app is gone — drop the provenance record. If the write fails, surface
    // it: a stale record could misclassify a same-path/same-build reinstall.
    store.managed.retain(|r| r.path != installed.path);
    if let Err(e) = store.save() {
        let detail = format!("托管记录清除失败（{e}）");
        outcome.provenance = StepOutcome::failed(detail.clone());
        outcome.push_warning(detail);
        outcome.push_recovery(recovery::CLEAR_PROVENANCE);
    }

    // Only ever touch ~/.codex when the user explicitly opted out of keeping it.
    // If the removal fails (e.g. permissions), report honestly rather than
    // claiming the data was cleared.
    let (kept_codex_home, mut message) = if keep_codex_home {
        outcome.cleanup = StepOutcome::skipped("保留用户数据 ~/.codex");
        (true, "已卸载 Codex,保留了 ~/.codex".to_string())
    } else {
        // Best-effort, non-fatal: a purge failure must not abort the uninstall
        // report. But like the Windows path (purge_codex_user_data), surface the
        // real underlying io/fs error instead of a generic "purge failed" so the
        // user can diagnose it (permissions, busy file, …) rather than guess.
        let mut purge_err: Option<String> = None;
        if let Ok(home) = std::env::var("HOME") {
            let codex_home = PathBuf::from(home).join(".codex");
            if codex_home.exists() {
                if let Err(e) = std::fs::remove_dir_all(&codex_home) {
                    purge_err = Some(e.to_string());
                }
            }
        }
        match purge_err {
            None => {
                outcome.cleanup = StepOutcome::ok_detail("已清除 ~/.codex");
                (false, "已卸载 Codex,并清除了 ~/.codex".to_string())
            }
            Some(err) => {
                let detail = format!("~/.codex 清除失败,数据仍保留: {err}");
                outcome.cleanup = StepOutcome::failed(detail.clone());
                outcome.push_warning(detail.clone());
                outcome.push_recovery(recovery::PURGE_USER_DATA);
                (true, format!("已卸载 Codex,但 {detail}"))
            }
        }
    };
    if outcome.provenance.is_failed() {
        message.push_str("；托管记录更新失败,可仅重试清除记录（无需再卸载）");
    }

    let report = MacUninstallReport {
        removed: true,
        kept_codex_home,
        message,
        outcome,
    };
    log::info!(
        "macOS uninstall complete keep_codex_home={}",
        report.kept_codex_home
    );
    Ok(report)
}

/// Retry only failed ancillary steps after a macOS uninstall (or install
/// provenance write). Never re-deletes the app bundle.
pub fn retry_macos_ancillary(
    actions: &[String],
    path: Option<&str>,
    purge_user_data: bool,
) -> Result<crate::app::operation_outcome::AncillaryRetryReport, AppError> {
    retry_macos_ancillary_with_detector(actions, path, purge_user_data, || {
        detect_managed_installed().or_else(detect_installed)
    })
}

fn retry_macos_ancillary_with_detector<F>(
    actions: &[String],
    path: Option<&str>,
    purge_user_data: bool,
    detect_install: F,
) -> Result<crate::app::operation_outcome::AncillaryRetryReport, AppError>
where
    F: FnOnce() -> Option<InstalledCodex>,
{
    use crate::app::operation_outcome::AncillaryRetryReport;

    let mut outcome = OperationOutcome {
        primary_ok: true,
        app_state: "unknown".to_string(),
        install_class: None,
        path: path.map(str::to_string),
        provenance: StepOutcome::not_applicable(),
        cleanup: StepOutcome::not_applicable(),
        warnings: Vec::new(),
        recovery_actions: Vec::new(),
    };
    let mut messages = Vec::new();

    if actions.iter().any(|a| a == recovery::RECORD_PROVENANCE) {
        // Re-record currently detected install as managed — same as adopt, but
        // as an explicit recovery path for "installed, record failed".
        match detect_install() {
            Some(installed) => {
                let mut store = ProvenanceStore::load();
                store.record(installed.path.clone(), installed.build, "manager-installed");
                match store.save() {
                    Ok(()) => {
                        outcome.provenance = StepOutcome::ok();
                        outcome.app_state = "present".to_string();
                        outcome.install_class = Some("managed".to_string());
                        messages.push("托管记录已重新写入".to_string());
                    }
                    Err(e) => {
                        outcome.provenance = StepOutcome::failed(e.to_string());
                        outcome.push_recovery(recovery::RECORD_PROVENANCE);
                        messages.push(format!("托管记录写入仍失败: {e}"));
                    }
                }
            }
            None => {
                outcome.provenance = StepOutcome::failed("未检测到可记录的 Codex 安装");
                outcome.app_state = "absent".to_string();
                messages.push("未检测到 Codex，无法写入托管记录".to_string());
            }
        }
        // Every failed record attempt remains retryable, including the
        // no-install-detected branch above. Keep this invariant outside the
        // individual match arms so future failure paths cannot omit the CTA.
        outcome.retain_failed_provenance_recovery(recovery::RECORD_PROVENANCE);
    }

    if actions.iter().any(|a| a == recovery::CLEAR_PROVENANCE) {
        let mut store = ProvenanceStore::load();
        if let Some(path) = path {
            store.remove(path);
        } else {
            // Drop records whose path no longer exists (stale after uninstall).
            store.managed.retain(|r| Path::new(&r.path).exists());
        }
        match store.save() {
            Ok(()) => {
                outcome.provenance = StepOutcome::ok();
                messages.push("陈旧托管记录已清除".to_string());
            }
            Err(e) => {
                outcome.provenance = StepOutcome::failed(e.to_string());
                outcome.push_recovery(recovery::CLEAR_PROVENANCE);
                messages.push(format!("清除托管记录仍失败: {e}"));
            }
        }
    }

    if actions.iter().any(|a| a == recovery::PURGE_USER_DATA) && purge_user_data {
        let mut purge_err: Option<String> = None;
        if let Ok(home) = std::env::var("HOME") {
            let codex_home = PathBuf::from(home).join(".codex");
            if codex_home.exists() {
                if let Err(e) = std::fs::remove_dir_all(&codex_home) {
                    purge_err = Some(e.to_string());
                }
            }
        }
        match purge_err {
            None => {
                outcome.cleanup = StepOutcome::ok_detail("已清除 ~/.codex");
                messages.push("用户数据已清除".to_string());
            }
            Some(err) => {
                outcome.cleanup = StepOutcome::failed(err.clone());
                outcome.push_recovery(recovery::PURGE_USER_DATA);
                messages.push(format!("清除用户数据仍失败: {err}"));
            }
        }
    }

    if messages.is_empty() {
        return Err(AppError::Internal(
            "没有可执行的恢复步骤（请传入 record_provenance / clear_provenance / purge_user_data）"
                .to_string(),
        ));
    }

    Ok(AncillaryRetryReport {
        message: messages.join("；"),
        outcome,
    })
}

#[cfg(test)]
mod disk_preflight_tests {
    use super::*;

    fn plan(download_size: u64, full_size: u64, strategy: UpdateStrategy) -> UpdatePlan {
        UpdatePlan {
            up_to_date: false,
            current_build: 1,
            latest_build: 2,
            latest_short_version: "2.0.0".to_string(),
            strategy,
            download_url: "https://example.com/Codex.zip".to_string(),
            download_size,
            ed_signature: Some("sig".to_string()),
            full_size,
            savings_pct: 0.0,
        }
    }

    #[test]
    fn mac_space_budget_accounts_for_delta_full_fallback_and_headroom() {
        let p = plan(10, 100, UpdateStrategy::Delta { from_build: 1 });
        assert_eq!(mac_space_budget(&p), 10 + 300 + 512 * 1024 * 1024);

        let p = plan(u64::MAX, u64::MAX, UpdateStrategy::Full);
        assert_eq!(mac_space_budget(&p), u64::MAX);
    }

    #[test]
    fn fresh_rename_reports_mutation_only_after_success() {
        let root = std::env::temp_dir().join(format!(
            "cam-fresh-historical-rename-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let staged = root.join("staged.app");
        let install = root.join("Codex.app");
        std::fs::create_dir_all(&staged).unwrap();
        std::fs::write(staged.join("payload"), b"test").unwrap();
        // A directory cannot replace an existing file. The failed atomic call
        // must leave policy/completion evidence in the pre-commit state.
        std::fs::write(&install, b"occupied").unwrap();
        let evidence = std::sync::Mutex::new(Vec::new());
        let hook = |value| evidence.lock().unwrap().push(value);
        assert!(commit_fresh_historical_app(&staged, &install, Some(&hook), None).is_err());
        assert!(evidence.lock().unwrap().is_empty());
        assert!(staged.exists());

        std::fs::remove_file(&install).unwrap();
        commit_fresh_historical_app(&staged, &install, Some(&hook), None).unwrap();
        assert_eq!(
            evidence.lock().unwrap().as_slice(),
            &[HistoricalMacEvidence::MutationStarted]
        );
        assert!(install.join("payload").is_file());
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn confirmed_rollback_restores_policy_before_relaunch() {
        let order = std::sync::Mutex::new(Vec::new());
        let restore = || {
            order.lock().unwrap().push("policy");
            Ok(())
        };
        let error = finish_historical_rollback(
            AppError::Engine("primary failure".to_string()),
            true,
            Some(&restore),
            || {
                order.lock().unwrap().push("relaunch");
                Ok::<(), codex_mac_engine::EngineError>(())
            },
        );
        assert_eq!(order.lock().unwrap().as_slice(), &["policy", "relaunch"]);
        assert!(error.to_string().contains("primary failure"));

        let relaunched = std::sync::atomic::AtomicBool::new(false);
        let failed_restore = || Err(AppError::Internal("restore failed".to_string()));
        let error = finish_historical_rollback(
            AppError::Engine("primary failure".to_string()),
            true,
            Some(&failed_restore),
            || {
                relaunched.store(true, Ordering::SeqCst);
                Ok::<(), codex_mac_engine::EngineError>(())
            },
        );
        assert!(!relaunched.load(Ordering::SeqCst));
        assert!(error.to_string().contains("restore failed"));
    }

    #[test]
    fn zip_expansion_budget_rejects_oversized_or_overflowing_archives() {
        assert_eq!(checked_zip_expanded_bytes(1, 2).unwrap(), 3);
        assert!(checked_zip_expanded_bytes(MAX_APP_EXPANDED_BYTES, 1)
            .unwrap_err()
            .to_string()
            .contains("超出允许范围"));
        assert!(checked_zip_expanded_bytes(u64::MAX, 1)
            .unwrap_err()
            .to_string()
            .contains("溢出"));
    }

    #[test]
    fn zip_expansion_preflight_uses_measured_output_before_ditto() {
        let work = Path::new("/staging");
        let expanded = 2 * 1024 * 1024 * 1024;
        let need = expanded + APP_EXPANSION_HEADROOM_BYTES;

        let err = require_zip_expansion_space(expanded, work, |_| Ok(Some(need - 1))).unwrap_err();
        assert!(err.to_string().contains("磁盘可用空间不足"));
        assert!(require_zip_expansion_space(expanded, work, |_| Ok(Some(need))).is_ok());
        assert!(require_zip_expansion_space(expanded, work, |_| Ok(None)).is_ok());
    }

    #[test]
    fn zip_preflight_rejects_forged_small_sizes_before_ditto_can_expand_them() {
        use std::io::Write as _;

        let tmp = std::env::temp_dir().join(format!(
            "cam-zip-forged-size-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&tmp).unwrap();
        let package = tmp.join("Codex.zip");
        let file = std::fs::File::create(&package).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .start_file(
                "Codex.app/Contents/large.bin",
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Deflated),
            )
            .unwrap();
        writer.write_all(&vec![b'0'; 1024 * 1024]).unwrap();
        writer.finish().unwrap();

        // Forge both local and central-directory uncompressed sizes to one byte,
        // matching the bypass reproduced against ditto during review.
        let mut bytes = std::fs::read(&package).unwrap();
        let local = bytes
            .windows(4)
            .position(|window| window == b"PK\x03\x04")
            .unwrap();
        bytes[local + 22..local + 26].copy_from_slice(&1_u32.to_le_bytes());
        let central = bytes
            .windows(4)
            .position(|window| window == b"PK\x01\x02")
            .unwrap();
        bytes[central + 24..central + 28].copy_from_slice(&1_u32.to_le_bytes());
        std::fs::write(&package, bytes).unwrap();

        let error =
            preflight_app_zip_with_available(&package, &tmp, |_| Ok(Some(u64::MAX))).unwrap_err();
        assert!(
            error.to_string().contains("ZIP"),
            "forged expansion must be rejected: {error}"
        );

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn zip_symlinks_must_stay_inside_a_physical_top_level_app() {
        assert!(validate_zip_symlink_target(
            Path::new("Codex.app/Contents/Frameworks/Codex.framework/Versions/Current"),
            "A",
        )
        .is_ok());
        assert!(validate_zip_symlink_target(
            Path::new("Codex.app/Contents/Frameworks/Codex.framework/Codex"),
            "Versions/Current/Codex",
        )
        .is_ok());

        let top_level =
            validate_zip_symlink_target(Path::new("Codex.app"), "/Applications/Codex.app")
                .unwrap_err();
        assert!(top_level.to_string().contains("物理 .app"));

        let escaping = validate_zip_symlink_target(
            Path::new("Codex.app/Contents/escape"),
            "../../Applications/Codex.app",
        )
        .unwrap_err();
        assert!(escaping.to_string().contains("逃出应用目录"));
    }

    #[test]
    fn zip_preflight_rejects_a_top_level_app_symlink_entry() {
        let tmp = std::env::temp_dir().join(format!(
            "cam-zip-symlink-preflight-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&tmp).unwrap();
        let package = tmp.join("Codex.zip");
        let file = std::fs::File::create(&package).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        writer
            .add_symlink(
                "Codex.app",
                "/Applications/Codex.app",
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        writer.finish().unwrap();

        let error =
            preflight_app_zip_with_available(&package, &tmp, |_| Ok(Some(u64::MAX))).unwrap_err();
        assert!(error.to_string().contains("物理 .app"));

        std::fs::remove_file(&package).unwrap();
        std::fs::remove_dir(&tmp).unwrap();
    }

    #[test]
    fn dmg_preflight_rejects_oversized_and_low_space_before_mounting() {
        let tmp = std::env::temp_dir().join(format!(
            "cam-dmg-preflight-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&tmp).unwrap();
        let dmg = tmp.join("Codex.dmg");
        let file = std::fs::File::create(&dmg).unwrap();
        file.set_len(codex_mac_engine::limits::MAX_PACKAGE_BYTES + 1)
            .unwrap();
        assert!(
            preflight_app_dmg_with_available(&dmg, &tmp, |_| Ok(Some(u64::MAX)))
                .unwrap_err()
                .to_string()
                .contains("超出允许范围")
        );

        file.set_len(1).unwrap();
        assert!(
            preflight_app_dmg_with_available(&dmg, &tmp, |_| Ok(Some(0)))
                .unwrap_err()
                .to_string()
                .contains("磁盘可用空间不足")
        );
        drop(file);
        std::fs::remove_file(&dmg).unwrap();
        std::fs::remove_dir(&tmp).unwrap();
    }

    #[test]
    fn mounted_dmg_app_preflight_uses_actual_logical_size_before_ditto() {
        let tmp = std::env::temp_dir().join(format!(
            "cam-mounted-app-preflight-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let app = tmp.join("Codex.app");
        let contents = app.join("Contents");
        std::fs::create_dir_all(&contents).unwrap();
        std::fs::write(contents.join("Info.plist"), vec![0_u8; 1024]).unwrap();

        let logical = mounted_app_logical_size(&app).unwrap();
        assert_eq!(logical, 1024);
        let need = logical + APP_EXPANSION_HEADROOM_BYTES;
        assert!(
            preflight_mounted_app_copy_with_available(&app, &tmp, |_| Ok(Some(need - 1)))
                .unwrap_err()
                .to_string()
                .contains("磁盘可用空间不足")
        );
        assert!(preflight_mounted_app_copy_with_available(&app, &tmp, |_| Ok(Some(need))).is_ok());

        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn mounted_dmg_app_preflight_rejects_actual_oversized_bundle() {
        let tmp = std::env::temp_dir().join(format!(
            "cam-mounted-app-oversized-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let app = tmp.join("Codex.app");
        let contents = app.join("Contents");
        std::fs::create_dir_all(&contents).unwrap();
        let payload = std::fs::File::create(contents.join("payload.bin")).unwrap();
        payload.set_len(MAX_APP_EXPANDED_BYTES + 1).unwrap();

        assert!(mounted_app_logical_size(&app)
            .unwrap_err()
            .to_string()
            .contains("超出允许范围"));

        drop(payload);
        std::fs::remove_dir_all(&tmp).unwrap();
    }

    #[test]
    fn preflight_mac_disk_rejects_low_space_and_allows_unknown_space() {
        let p = plan(100, 200, UpdateStrategy::Full);
        let err = preflight_mac_disk_with_available(&p, |_| Ok(Some(10))).unwrap_err();
        assert!(err.to_string().contains("磁盘可用空间不足"));

        assert!(preflight_mac_disk_with_available(&p, |_| Ok(None)).is_ok());
        assert!(preflight_mac_disk_with_available(&p, |_| Ok(Some(mac_space_budget(&p)))).is_ok());
    }

    #[test]
    fn retry_record_provenance_without_detected_install_keeps_retry_action() {
        let report = retry_macos_ancillary_with_detector(
            &[recovery::RECORD_PROVENANCE.to_string()],
            None,
            false,
            || None,
        )
        .expect("a missing install is a retryable ancillary result");

        assert!(report.outcome.provenance.is_failed());
        assert!(report
            .outcome
            .recovery_actions
            .iter()
            .any(|action| action == recovery::RECORD_PROVENANCE));
    }

    #[test]
    fn historical_swap_failure_preserves_staging_until_disk_is_restored() {
        let tmp = std::env::temp_dir().join(format!(
            "cam-historical-swap-recovery-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let install = tmp.join("Codex.app");
        let backup = tmp.join("backup-Codex.app");
        std::fs::create_dir_all(&install).unwrap();

        assert!(
            !historical_swap_needs_recovery_materials(&install, &backup),
            "an intact restored install with no backup needs no retained staging"
        );
        std::fs::create_dir_all(&backup).unwrap();
        assert!(
            historical_swap_needs_recovery_materials(&install, &backup),
            "a backup must remain available to the pending recovery journal"
        );
        std::fs::remove_dir_all(&install).unwrap();
        std::fs::remove_dir_all(&backup).unwrap();
        assert!(
            historical_swap_needs_recovery_materials(&install, &backup),
            "a missing install is outcome-ambiguous even if the backup probe failed"
        );

        std::fs::remove_dir_all(&tmp).unwrap();
    }
}

// The full-update unpack branch is new logic (the delta/gate/swap tail reuses
// engine primitives already proven on real /Applications). `ditto` is macOS-only,
// so these stay gated off the cross-compiled Windows build.
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    #[test]
    fn abort_guard_preserves_a_startup_race_cancel_and_resets_on_drop() {
        let _serial = crate::app::oplock::CANCEL_LATCH_TEST_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        // The race a reviewer flagged: a cancel can land BEFORE perform reaches
        // its first checkpoint even though the command now validates the owning
        // token under the op lock. Clearing the latch at op ENTRY would wipe that
        // cancel; AbortGuard clears on DROP instead, so a pending cancel survives
        // the guard's creation and is still observed, while the next op starts
        // clean. OperationManager cleanup shares the same test-only serial lock.
        UPDATE_ABORT.store(true, Ordering::SeqCst); // a cancel that beat the op
        {
            let outer = AbortGuard::new();
            let inner = AbortGuard::new();
            assert!(
                check_update_abort().is_err(),
                "guard creation must not wipe a pending cancel"
            );
            drop(inner);
            assert!(
                check_update_abort().is_err(),
                "a nested guard must not clear the owning operation's cancel"
            );
            drop(outer);
        }
        assert!(
            check_update_abort().is_ok(),
            "guard drop must reset the latch for the next op"
        );
    }

    #[test]
    fn fresh_install_refuses_a_latest_build_that_changed_after_confirmation() {
        assert!(require_expected_latest_build(Some(5813), 5813).is_ok());
        assert!(require_expected_latest_build(None, 6000).is_ok());
        assert!(matches!(
            require_expected_latest_build(Some(5813), 6000),
            Err(AppError::StaleExpectation(_))
        ));
    }

    fn ditto(args: &[&str]) {
        let status = std::process::Command::new(DITTO)
            .args(args)
            .status()
            .expect("spawn ditto");
        assert!(status.success(), "ditto {args:?} failed");
    }

    fn write_bundle_plist(app: &Path, bundle_id: &str) {
        std::fs::create_dir_all(app.join("Contents")).unwrap();
        std::fs::write(
            app.join("Contents/Info.plist"),
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>{bundle_id}</string>
</dict>
</plist>
"#
            ),
        )
        .unwrap();
    }

    #[test]
    fn unpack_app_zip_surfaces_bundle() {
        let root = std::env::temp_dir().join(format!("codex-unpack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        // A synthetic post-rebrand bundle (named ChatGPT.app, Codex identity),
        // zipped the way Sparkle ships a macOS full update (`ditto -c -k
        // --keepParent` → the `.app` is the top-level entry). unpack must accept
        // it by identity and normalize it to the caller's out_app name.
        let src_app = root.join("ChatGPT.app");
        write_bundle_plist(&src_app, sys::CODEX_BUNDLE_ID);
        std::fs::write(src_app.join("Contents/marker"), "5059").unwrap();
        let zip = root.join("Codex.zip");
        ditto(&[
            "-c",
            "-k",
            "--keepParent",
            &src_app.to_string_lossy(),
            &zip.to_string_lossy(),
        ]);

        let work = root.join("work");
        std::fs::create_dir_all(&work).unwrap();
        let out_app = work.join("Codex.app");
        unpack_app_zip(&zip, &work, &out_app).unwrap();

        assert!(out_app.join("Contents/marker").exists(), "bundle surfaced");
        assert_eq!(
            std::fs::read_to_string(out_app.join("Contents/marker")).unwrap(),
            "5059"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn require_single_codex_app_rejects_multiple_apps() {
        let root = std::env::temp_dir().join(format!("codex-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Other.app")).unwrap();
        std::fs::create_dir_all(root.join("Codex.app")).unwrap();

        let err = require_single_codex_app(&root).unwrap_err();
        assert!(err.to_string().contains("multiple .app"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn require_single_codex_app_gates_on_bundle_identity() {
        let root = std::env::temp_dir().join(format!("codex-identity-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        // ChatGPT Classic in the archive: right name shape, wrong product.
        let classic = root.join("classic");
        write_bundle_plist(&classic.join("ChatGPT.app"), "com.openai.chat");
        let err = require_single_codex_app(&classic).unwrap_err();
        assert!(err.to_string().contains("com.openai.chat"));

        // Rebranded Codex: accepted regardless of the bundle's file name.
        let rebranded = root.join("rebranded");
        write_bundle_plist(&rebranded.join("ChatGPT.app"), sys::CODEX_BUNDLE_ID);
        let found = require_single_codex_app(&rebranded).unwrap();
        assert!(found.ends_with("ChatGPT.app"));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn require_single_codex_app_rejects_a_top_level_symlink() {
        use std::os::unix::fs::symlink;

        let root = std::env::temp_dir().join(format!(
            "codex-top-level-symlink-{}-{}",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        let extract = root.join("extract");
        let outside = root.join("outside/Codex.app");
        std::fs::create_dir_all(&extract).unwrap();
        write_bundle_plist(&outside, sys::CODEX_BUNDLE_ID);
        symlink(&outside, extract.join("Codex.app")).unwrap();

        let error = require_single_codex_app(&extract).unwrap_err();
        assert!(error.to_string().contains("顶层 .app 是符号链接"));

        std::fs::remove_dir_all(&root).unwrap();
    }
}
