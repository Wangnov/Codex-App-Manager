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

use std::path::{Path, PathBuf};

use serde::Serialize;

use codex_mac_engine::{
    apply_delta, download, gate_reconstructed, parse_appcast, plan_update, quit_codex, relaunch,
    rollback, swap::codex_running, swap_in_place, sys, verify_sparkle, Appcast, UpdatePlan,
    UpdateStrategy,
};

use crate::app::provenance::ProvenanceStore;
use crate::errors::AppError;

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

fn fetch_one(url: String) -> Result<(String, String), AppError> {
    let xml = sys::fetch_text(&url).map_err(|e| AppError::Engine(e.to_string()))?;
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
fn fetch_appcast_for_arch(arch: &str) -> Result<(String, String), AppError> {
    let settings = crate::app::settings_store::AppSettings::load();
    match settings.source.as_str() {
        "official" => fetch_one(official_for_arch(arch).to_string()),
        "mirror" => fetch_one(appcast_for_arch(arch).to_string()),
        "custom" => {
            let u = settings.custom_url.trim();
            let url = if u.is_empty() {
                appcast_for_arch(arch).to_string()
            } else {
                u.to_string()
            };
            fetch_one(url)
        }
        _ => {
            // auto: pick the higher build between the CN-reachable mirror and
            // OpenAI official, among whichever sources are reachable. The mirror
            // can lag the official feed by a release; when it does and official
            // is reachable, we still surface the newer build instead of stranding
            // the user on the stale mirror version. Official is a best-effort
            // probe with a short timeout so users who can't reach it don't stall.
            // If only one is reachable, use it; if neither, error.
            let mirror_url = appcast_for_arch(arch).to_string();
            let official_url = official_for_arch(arch).to_string();
            let mirror = sys::fetch_text(&mirror_url).ok();
            let official = sys::fetch_text_timeout(&official_url, 8).ok();
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

fn detect_managed_installed() -> Option<InstalledCodex> {
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
    let installed = detect_managed_installed();
    let (appcast_url, xml) = fetch_appcast_for_arch(arch_of(&installed))?;
    let appcast = parse_appcast(&xml).map_err(|e| AppError::Engine(e.to_string()))?;
    let plan = plan_update(&appcast, effective_build(simulated_build, &installed));

    if let Some(latest) = appcast.latest() {
        require_os_supported(latest.minimum_system_version.as_deref())?;
    }

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

    Ok(MacUpdateReport {
        appcast_url,
        installed,
        simulated_build,
        plan,
        installed_pub_date,
    })
}

fn staging_dir() -> std::path::PathBuf {
    std::env::temp_dir()
        .join("codex-app-manager")
        .join("staging")
}

fn strategy_label(strategy: &UpdateStrategy) -> String {
    match strategy {
        UpdateStrategy::Delta { from_build } => format!("delta-from-{from_build}"),
        UpdateStrategy::Full => "full".to_string(),
    }
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
fn quit_codex_gracefully() -> Result<(), AppError> {
    quit_codex(30).map_err(|_| {
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
    signature: &str,
    progress: &dyn Fn(DownloadProgress),
) -> Result<PathBuf, AppError> {
    let file_name = url.rsplit('/').next().unwrap_or("payload.bin");
    let dest = staging_dir().join(file_name);
    let source = host_of(url);

    let already = std::fs::metadata(&dest).map(|m| m.len() == size).unwrap_or(false);
    if already {
        // Cached from a prior stage — report complete so the UI doesn't sit at 0.
        progress(DownloadProgress {
            downloaded: size,
            total: size,
            source,
        });
    } else {
        download::download_to_with_progress(url, &dest, &|downloaded| {
            progress(DownloadProgress {
                downloaded,
                total: size,
                source: source.clone(),
            });
        })
        .map_err(|e| AppError::Engine(e.to_string()))?;
    }

    let len = std::fs::metadata(&dest)
        .map_err(|e| AppError::Engine(e.to_string()))?
        .len();
    if len != size {
        return Err(AppError::Engine(format!("size mismatch: {len} != {size}")));
    }

    let bytes = download::read_file(&dest).map_err(|e| AppError::Engine(e.to_string()))?;
    if let Err(err) = verify_sparkle(&bytes, signature) {
        let _ = std::fs::remove_file(&dest);
        return Err(AppError::Engine(err.to_string()));
    }

    Ok(dest)
}

pub fn stage_macos_update(simulated_build: Option<u64>) -> Result<MacStageReport, AppError> {
    let installed = detect_managed_installed();
    let (_, xml) = fetch_appcast_for_arch(arch_of(&installed))?;
    let appcast = parse_appcast(&xml).map_err(|e| AppError::Engine(e.to_string()))?;
    let plan = plan_update(&appcast, effective_build(simulated_build, &installed))
        .ok_or_else(|| AppError::Engine("appcast had no items".to_string()))?;

    if plan.up_to_date {
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

    let signature = plan
        .ed_signature
        .clone()
        .ok_or_else(|| AppError::Engine("appcast enclosure missing edSignature".to_string()))?;
    let dest = download_and_verify(&plan.download_url, plan.download_size, &signature, &no_progress)?;

    Ok(MacStageReport {
        up_to_date: false,
        strategy: strategy_label(&plan.strategy),
        latest_build: plan.latest_build,
        latest_short_version: plan.latest_short_version,
        download_size: plan.download_size,
        full_size: plan.full_size,
        savings_pct: plan.savings_pct,
        staged_path: Some(dest.to_string_lossy().into_owned()),
        verified: true,
    })
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
    let extract = work.join("unzip");
    let _ = std::fs::remove_dir_all(&extract);
    std::fs::create_dir_all(&extract)
        .map_err(|e| AppError::Engine(format!("mkdir unzip: {e}")))?;

    let status = std::process::Command::new("ditto")
        .args(["-x", "-k"])
        .arg(zip)
        .arg(&extract)
        .status()
        .map_err(|e| AppError::Engine(format!("spawn ditto: {e}")))?;
    if !status.success() {
        return Err(AppError::Engine(format!("ditto unzip exited with {status}")));
    }

    let found = find_dot_app(&extract)
        .ok_or_else(|| AppError::Engine("no .app found inside the full-update zip".to_string()))?;
    if out_app.exists() {
        let _ = std::fs::remove_dir_all(out_app);
    }
    std::fs::rename(&found, out_app)
        .map_err(|e| AppError::Engine(format!("move unpacked app: {e}")))?;
    Ok(())
}

/// Locate a `.app` bundle inside an extracted directory: prefer a top-level
/// `Codex.app`, else the first `*.app` directory entry.
fn find_dot_app(dir: &Path) -> Option<PathBuf> {
    let direct = dir.join("Codex.app");
    if direct.exists() {
        return Some(direct);
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|entry| entry.path())
        .find(|p| p.is_dir() && p.extension().map(|x| x == "app").unwrap_or(false))
}

/// Download the appcast's full enclosure (size + EdDSA verified) and unpack it
/// into `out_app`. Needs no BinaryDelta — used both as the primary full path and
/// as the recovery when a delta is unavailable or fails to apply.
fn reconstruct_full(
    appcast: &Appcast,
    work: &Path,
    out_app: &Path,
    progress: &dyn Fn(DownloadProgress),
) -> Result<(), AppError> {
    let latest = appcast
        .latest()
        .ok_or_else(|| AppError::Engine("appcast had no items".to_string()))?;
    let sig = latest.full.ed_signature.clone().ok_or_else(|| {
        AppError::Engine("appcast full enclosure missing edSignature".to_string())
    })?;
    let staged = download_and_verify(&latest.full.url, latest.full.length, &sig, progress)?;
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
    // A vanished install is itself a stale snapshot: the user confirmed an
    // update against a Codex that is no longer there (deleted / moved between
    // confirm and execute). Route it through StaleExpectation so the UI
    // auto-re-checks (→ none/install) instead of looping on a dead error.
    let installed = detect_managed_installed().ok_or_else(|| {
        AppError::StaleExpectation(
            "未检测到 Codex（可能已被删除或移动）：请重新检查后再试".to_string(),
        )
    })?;

    // Consent integrity: the destructive swap must target exactly what the user
    // saw + confirmed. If Codex self-updated (Sparkle), moved, or staging is
    // stale, refuse rather than act on a stale consent.
    if installed.path != expected.install_path {
        return Err(AppError::StaleExpectation(format!(
            "安装位置已变化（确认时 {}，现在 {}）：请重新检查后再试",
            expected.install_path, installed.path
        )));
    }
    if installed.build != expected.from_build {
        return Err(AppError::StaleExpectation(format!(
            "已装版本已变化（确认时 build {}，现在 build {}）：请重新检查后再试",
            expected.from_build, installed.build
        )));
    }

    let install_path = PathBuf::from(&installed.path);

    let (_, xml) = fetch_appcast_for_arch(&installed.arch)?;
    let appcast = parse_appcast(&xml).map_err(|e| AppError::Engine(e.to_string()))?;
    let plan = plan_update(&appcast, installed.build)
        .ok_or_else(|| AppError::Engine("appcast had no items".to_string()))?;

    // The appcast must still point at the build the user confirmed.
    if plan.latest_build != expected.to_build {
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

    // 1) Set up same-volume staging for the reconstructed bundle + backup.
    let work = staging_dir();
    let out_app = work.join("Codex.app");
    let backup = work.join("backup-Codex.app");
    let _ = std::fs::remove_dir_all(&out_app);
    let _ = std::fs::remove_dir_all(&backup);

    // 2) Reconstruct the new bundle into out_app. Prefer a delta when the tool is
    //    present; fall back to the appcast's full package when the tool is missing
    //    OR the delta fails to apply (modified basis, tool/patch version mismatch,
    //    …). The full enclosure is always present in the same appcast entry and
    //    needs no tool, so a recoverable delta failure no longer fails the update.
    let want_delta = matches!(plan.strategy, UpdateStrategy::Delta { .. });
    let effective_strategy = if want_delta {
        if let Some(tool) = binary_delta.as_deref() {
            let sig = plan
                .ed_signature
                .clone()
                .ok_or_else(|| AppError::Engine("appcast delta missing edSignature".to_string()))?;
            let staged =
                download_and_verify(&plan.download_url, plan.download_size, &sig, progress)?;
            match apply_delta(tool, &install_path, &out_app, &staged) {
                Ok(()) => strategy_label(&plan.strategy),
                Err(delta_err) => {
                    reconstruct_full(&appcast, &work, &out_app, progress)?;
                    format!("full (delta 应用失败回退: {delta_err})")
                }
            }
        } else {
            reconstruct_full(&appcast, &work, &out_app, progress)?;
            "full (delta 工具缺失，回退全量)".to_string()
        }
    } else {
        reconstruct_full(&appcast, &work, &out_app, progress)?;
        "full".to_string()
    };

    // 3) gate the reconstructed bundle before it touches the install root.
    gate_reconstructed(&out_app)
        .map_err(|e| AppError::Engine(format!("codesign 闸失败（拒绝替换）: {e}")))?;

    // 4a) pre-flight the atomic-swap precondition BEFORE quitting Codex, so a
    //     cross-volume staging dir fails fast WITHOUT closing the user's app.
    let install_parent = install_path.parent().unwrap_or(install_path.as_path());
    if !codex_mac_engine::swap::same_volume(&out_app, install_parent) {
        return Err(AppError::Engine(
            "暂存目录与安装根不在同一卷，无法原子替换：请确保 TMPDIR 与安装根同卷".to_string(),
        ));
    }

    // 4b) graceful quit (never force-kill), then 5) atomic same-volume swap. If
    //     the swap fails after the quit, swap_in_place has restored the old
    //     bundle in place — bring the user's app back before surfacing the error.
    let was_running = codex_running();
    quit_codex_gracefully()?;
    if let Err(err) = swap_in_place(&install_path, &out_app, &backup) {
        if was_running {
            let _ = relaunch(&install_path);
        }
        return Err(AppError::Engine(err.to_string()));
    }

    // 6) filesystem health check on the installed root.
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
        store.record(installed.path.clone(), plan.latest_build, "manager-installed");
        let save_warning = match store.save() {
            Ok(()) => None,
            Err(e) => Some(format!("托管记录保存失败（{e}），安装暂仍会被识别为外部")),
        };

        if was_running {
            // Relaunch BEFORE discarding the backup: if `open` fails we keep the
            // backup as a recovery path instead of claiming a clean success. We
            // do NOT downgrade a healthy, gated install just because auto-launch
            // failed — the user can launch it manually.
            if let Err(err) = relaunch(&install_path) {
                return Ok(MacPerformReport {
                    up_to_date: false,
                    from_build: installed.build,
                    to_build: plan.latest_build,
                    strategy: effective_strategy.clone(),
                    installed_path: installed.path,
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
        Ok(MacPerformReport {
            up_to_date: false,
            from_build: installed.build,
            to_build: plan.latest_build,
            strategy: effective_strategy.clone(),
            installed_path: installed.path,
            verified: true,
            relaunched: was_running,
            relaunch_failed: false,
            rolled_back: false,
            warning: save_warning,
            message: format!("已更新 build {} → {}", installed.build, plan.latest_build),
        })
    } else {
        rollback(&install_path, &backup)
            .map_err(|e| AppError::Engine(format!("回滚失败（需人工介入）: {e}")))?;
        let relaunched = was_running && relaunch(&install_path).is_ok();
        Ok(MacPerformReport {
            up_to_date: false,
            from_build: installed.build,
            to_build: plan.latest_build,
            strategy: effective_strategy.clone(),
            installed_path: installed.path,
            verified: true,
            relaunched,
            relaunch_failed: false,
            rolled_back: true,
            warning: None,
            message: "新版本健康检查未通过，已回滚到旧版本".to_string(),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MacInstallStatus {
    pub installed: Option<InstalledCodex>,
    /// "managed" | "external" | "none"
    pub status: String,
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
    MacInstallStatus { installed, status }
}

/// Adopt the detected install — record provenance after explicit user consent.
pub fn mac_adopt() -> Result<MacInstallStatus, AppError> {
    let installed = detect_installed()
        .ok_or_else(|| AppError::Internal("no Codex detected to adopt".to_string()))?;
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
    let (_, xml) = fetch_appcast_for_arch(arch)?;
    let appcast = parse_appcast(&xml).map_err(|e| AppError::Engine(e.to_string()))?;
    if let Some(latest) = appcast.latest() {
        require_os_supported(latest.minimum_system_version.as_deref())?;
    }

    let work = staging_dir();
    let out_app = work.join("Codex.app");
    let _ = std::fs::remove_dir_all(&out_app);

    // Full package only — no basis bundle to delta against.
    reconstruct_full(&appcast, &work, &out_app, progress)?;
    gate_reconstructed(&out_app)
        .map_err(|e| AppError::Engine(format!("codesign 闸失败（拒绝安装）: {e}")))?;

    if !codex_mac_engine::swap::same_volume(&out_app, &install_dir) {
        return Err(AppError::Engine(format!(
            "暂存目录与 {} 不在同一卷,无法安装",
            install_dir.display()
        )));
    }
    std::fs::rename(&out_app, &install_path)
        .map_err(|e| AppError::Engine(format!("写入 {} 失败: {e}", install_dir.display())))?;

    if let Some((path, build)) = sys::installed_codex_build() {
        let mut store = ProvenanceStore::load();
        store.record(path, build, "manager-installed");
        // The app is on disk now; if we can't persist provenance the install
        // would be misclassified as external, so surface it (the install
        // succeeded — the user just needs to re-adopt from the main screen).
        store.save().map_err(|e| {
            AppError::Engine(format!(
                "已安装 Codex,但来源记录保存失败（{e}）;请在主界面「开始管理」纳入管理"
            ))
        })?;
    }
    // Do NOT auto-launch — the UI shows a completion state with an explicit
    // 〔打开 Codex〕 button so opening is the user's choice, not a surprise.
    Ok(mac_install_status())
}

/// Open the installed Codex.app — invoked by the UI's explicit 〔打开 Codex〕
/// action (we no longer auto-launch after a fresh install).
pub fn launch_codex() -> Result<(), AppError> {
    let installed =
        detect_managed_installed().ok_or_else(|| AppError::Engine("没有可打开的 Codex".to_string()))?;
    std::process::Command::new("open")
        .arg(&installed.path)
        .spawn()
        .map(|_| ())
        .map_err(|e| AppError::Engine(format!("打开 Codex 失败: {e}")))
}

pub fn pause_macos_download() -> bool {
    download::pause_active_download()
}

pub fn cancel_macos_download() -> bool {
    download::cancel_active_download()
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MacUninstallReport {
    pub removed: bool,
    pub kept_codex_home: bool,
    pub message: String,
}

/// Remove the installed Codex app. Only uninstalls an install THIS manager
/// manages — an external (official / manual) install must be explicitly adopted
/// first, so we never delete something we didn't create. `keep_codex_home` is
/// true by default at the UI: the user's `~/.codex` (sign-in, sessions, config)
/// survives unless they explicitly opt out. Quits Codex first (never force-kills).
pub fn uninstall_macos(keep_codex_home: bool) -> Result<MacUninstallReport, AppError> {
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

    quit_codex_gracefully()?;

    // Delete first: if we lack permission to remove the bundle (e.g. a root-owned
    // install), the managed record stays intact so the user can retry without
    // re-adopting.
    std::fs::remove_dir_all(&install_path)
        .map_err(|e| AppError::Engine(format!("remove app bundle: {e}")))?;

    // The app is gone — drop the provenance record. If the write fails, surface
    // it: a stale record could misclassify a same-path/same-build reinstall.
    store.managed.retain(|r| r.path != installed.path);
    let prov_saved = store.save().is_ok();

    // Only ever touch ~/.codex when the user explicitly opted out of keeping it.
    // If the removal fails (e.g. permissions), report honestly rather than
    // claiming the data was cleared.
    let (kept_codex_home, mut message) = if keep_codex_home {
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
            None => (false, "已卸载 Codex,并清除了 ~/.codex".to_string()),
            Some(err) => (
                true,
                format!("已卸载 Codex,但 ~/.codex 清除失败,数据仍保留: {err}"),
            ),
        }
    };
    if !prov_saved {
        message.push_str("；托管记录更新失败,请重新检查管理状态");
    }

    Ok(MacUninstallReport {
        removed: true,
        kept_codex_home,
        message,
    })
}

// The full-update unpack branch is new logic (the delta/gate/swap tail reuses
// engine primitives already proven on real /Applications). `ditto` is macOS-only,
// so these stay gated off the cross-compiled Windows build.
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    fn ditto(args: &[&str]) {
        let status = std::process::Command::new("ditto")
            .args(args)
            .status()
            .expect("spawn ditto");
        assert!(status.success(), "ditto {args:?} failed");
    }

    #[test]
    fn unpack_app_zip_surfaces_bundle() {
        let root = std::env::temp_dir().join(format!("codex-unpack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        // A synthetic Codex.app, zipped the way Sparkle ships a macOS full update
        // (`ditto -c -k --keepParent` → the `.app` is the top-level entry).
        let src_app = root.join("Codex.app");
        std::fs::create_dir_all(src_app.join("Contents")).unwrap();
        std::fs::write(src_app.join("Contents/marker"), "3575").unwrap();
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
            "3575"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn find_dot_app_prefers_codex() {
        let root = std::env::temp_dir().join(format!("codex-find-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("Other.app")).unwrap();
        std::fs::create_dir_all(root.join("Codex.app")).unwrap();

        let found = find_dot_app(&root).expect("an .app is found");
        assert!(found.ends_with("Codex.app"), "prefers Codex.app, got {found:?}");

        let _ = std::fs::remove_dir_all(&root);
    }
}
