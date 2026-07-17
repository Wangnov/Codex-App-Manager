//! Codex UI theme orchestration. Thin coordination over `codex-theme-engine`:
//! resolve theme packages from local roots, keep the in-process daemon fed,
//! and drive the restart path (graceful quit → native config.toml appearance
//! sections → relaunch with a loopback CDP port → inject).
//!
//! Everything CDP-related is macOS-first (launching with
//! `--remote-debugging-port` needs `open -n`); the commands report
//! `supported: false` elsewhere so the UI can say so instead of erroring.

use std::path::{Path, PathBuf};
use std::time::Duration;

use codex_theme_engine::daemon::{run_daemon, DaemonStatus, Directive};
use codex_theme_engine::native::{has_backup, NativeThemePaths};
use codex_theme_engine::theme::{list_themes, load_theme, ThemeSummary};
use serde::Serialize;
use tokio::sync::{watch, Mutex};

use crate::app::paths;
use crate::app::settings_store::AppSettings;
use crate::errors::AppError;

/// Fixed loopback CDP port. Deliberately not scanned-for (yet): the port only
/// exists while a themed Codex runs, and a collision surfaces as an explicit
/// error rather than a silent bind elsewhere the daemon wouldn't find.
pub const THEME_CDP_PORT: u16 = 9345;

/// Wait after Codex's PIDs disappear before touching config.toml — Codex
/// persists its in-memory config on exit *after* the process count reaches
/// zero (measured in the studio; writing earlier gets clobbered).
const CONFIG_SETTLE: Duration = Duration::from_secs(2);
#[cfg(target_os = "macos")]
const QUIT_TIMEOUT_SECS: u64 = 30;
const CDP_WAIT: Duration = Duration::from_secs(45);

#[derive(Default)]
pub struct ThemeService {
    inner: Mutex<ServiceInner>,
}

#[derive(Default)]
struct ServiceInner {
    directive_tx: Option<watch::Sender<Directive>>,
    status_rx: Option<watch::Receiver<DaemonStatus>>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeStatusReport {
    /// Whether this platform can theme Codex at all (macOS for now).
    pub supported: bool,
    /// The persisted selection (settings), i.e. what launches will apply.
    pub active_theme: Option<String>,
    /// Live daemon snapshot, when one is running.
    pub daemon: Option<DaemonStatus>,
    /// A CDP endpoint answers on the theme port right now.
    pub cdp_ready: bool,
    /// Codex processes are running (regardless of CDP).
    pub codex_running: bool,
    /// A pristine config.toml appearance backup exists (full restore possible).
    pub native_backup_present: bool,
    /// Where managed skins currently live (downloads/imports land here).
    pub store_dir: Option<PathBuf>,
}

fn theme_supported() -> bool {
    cfg!(target_os = "macos")
}

/// Where managed skins live: the user-chosen store from settings, else the
/// platform default (macOS: Application Support; Windows: LOCALAPPDATA —
/// megabytes of re-downloadable content must not roam with a domain profile).
pub fn store_dir(settings: &AppSettings) -> Result<PathBuf, AppError> {
    if let Some(dir) = settings
        .codex_theme_store_dir
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        return Ok(PathBuf::from(dir));
    }
    paths::default_skins_store_dir()
        .ok_or_else(|| AppError::Internal("无法定位主题存储目录".to_string()))
}

/// Managed skin store + optional dev root from settings. Dev packages shadow
/// managed ones by id.
fn theme_roots(settings: &AppSettings) -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Some(dir) = settings
        .codex_theme_dir
        .as_deref()
        .filter(|d| !d.trim().is_empty())
    {
        roots.push(PathBuf::from(dir));
    }
    if let Ok(store) = store_dir(settings) {
        roots.push(store);
    }
    roots
}

/// Move one directory across an arbitrary boundary: fast rename first,
/// recursive copy + delete when the rename crosses filesystems.
fn move_dir(src: &Path, dst: &Path) -> Result<(), AppError> {
    if std::fs::rename(src, dst).is_ok() {
        return Ok(());
    }
    fn copy_tree(src: &Path, dst: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(dst)?;
        for entry in std::fs::read_dir(src)? {
            let entry = entry?;
            let target = dst.join(entry.file_name());
            if entry.file_type()?.is_dir() {
                copy_tree(&entry.path(), &target)?;
            } else {
                std::fs::copy(entry.path(), &target)?;
            }
        }
        Ok(())
    }
    copy_tree(src, dst).map_err(|e| AppError::Internal(format!("迁移 {} 失败: {e}", src.display())))?;
    std::fs::remove_dir_all(src)
        .map_err(|e| AppError::Internal(format!("清理旧目录失败: {e}")))?;
    Ok(())
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct StoreMigrationReport {
    pub from: PathBuf,
    pub to: PathBuf,
    pub moved: Vec<String>,
    /// Ids skipped because the destination already had them.
    pub skipped: Vec<String>,
}

/// Migrate every valid skin package from `old` to `new`. Conflicting ids are
/// left in place (destination wins — never destroy what the user already has
/// at the target). Leftover staging debris is not migrated.
pub fn migrate_store(old: &Path, new: &Path) -> Result<StoreMigrationReport, AppError> {
    std::fs::create_dir_all(new)
        .map_err(|e| AppError::Internal(format!("创建主题目录失败: {e}")))?;
    let mut report = StoreMigrationReport {
        from: old.to_path_buf(),
        to: new.to_path_buf(),
        moved: Vec::new(),
        skipped: Vec::new(),
    };
    let Ok(entries) = std::fs::read_dir(old) else {
        return Ok(report); // old store never materialized — nothing to move
    };
    for entry in entries.flatten() {
        let src = entry.path();
        if !src.is_dir() || load_theme(&src).is_err() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().into_owned();
        let dst = new.join(&name);
        if dst.exists() {
            report.skipped.push(name);
            continue;
        }
        move_dir(&src, &dst)?;
        report.moved.push(name);
    }
    Ok(report)
}

fn native_paths() -> Result<NativeThemePaths, AppError> {
    let config = paths::codex_home_dir()
        .ok_or_else(|| AppError::Internal("无法定位 ~/.codex".to_string()))?
        .join("config.toml");
    let backup = paths::data_dir()
        .ok_or_else(|| AppError::Internal("无法定位数据目录".to_string()))?
        .join("codex-theme-native-backup.json");
    Ok(NativeThemePaths { config, backup })
}

/// Which root a gallery entry came from. Dev packages shadow store packages
/// per id at *resolution* time; the gallery still needs to see both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeOrigin {
    Dev,
    Store,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeListEntry {
    #[serde(flatten)]
    pub summary: ThemeSummary,
    pub origin: ThemeOrigin,
}

/// Every package from every root, dev root first (same precedence as
/// `resolve_theme`), annotated with its origin. Deliberately NOT deduplicated:
/// a dev checkout masks the store copy for resolution, but the store tab
/// compares catalog versions against the STORE copy — after an online update
/// the shadowed store version must still be observable, or the update button
/// never flips to "installed".
pub fn merged_theme_list(settings: &AppSettings) -> Vec<ThemeListEntry> {
    let mut entries = Vec::new();
    if let Some(dir) = settings
        .codex_theme_dir
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
    {
        entries.extend(list_themes(Path::new(dir)).into_iter().map(|summary| ThemeListEntry {
            summary,
            origin: ThemeOrigin::Dev,
        }));
    }
    if let Ok(store) = store_dir(settings) {
        entries.extend(list_themes(&store).into_iter().map(|summary| ThemeListEntry {
            summary,
            origin: ThemeOrigin::Store,
        }));
    }
    entries
}

fn resolve_theme(settings: &AppSettings, theme_ref: &str) -> Result<PathBuf, AppError> {
    for root in theme_roots(settings) {
        if let Ok(dir) = codex_theme_engine::theme::resolve_theme_dir(&root, theme_ref) {
            return Ok(dir);
        }
    }
    Err(AppError::Engine(format!("未找到主题: {theme_ref}")))
}

/// Canonical id for persisting a selection: a `theme_ref` may be a dev path,
/// but settings always store the package's own id.
pub fn resolve_theme_for_keep(settings: &AppSettings, theme_ref: &str) -> Result<String, AppError> {
    let dir = resolve_theme(settings, theme_ref)?;
    let theme = load_theme(&dir).map_err(|e| AppError::Engine(e.to_string()))?;
    Ok(theme.config.id)
}

// ── Online catalog (skins.agentsmirror.com) ────────────────────────────────
// The catalog is published by awesome-codex-skins' CI; URLs inside it are
// relative and resolved ONLY against this fixed base — a hostile catalog
// cannot redirect downloads elsewhere. All transfers go through the system
// curl (the repo's networking idiom; Windows 10+ ships curl.exe) with https
// pinned, size caps, and a sha256 gate before anything reaches the importer.

const SKINS_BASE: &str = "https://skins.agentsmirror.com";
const CATALOG_MAX_BYTES: &str = "1048576"; // 1 MB index.json cap
const PACK_MAX_BYTES: &str = "52428800"; // 50 MB archive cap (importer re-checks)

#[derive(Debug, Clone, serde::Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogSkin {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub author: String,
    #[serde(default)]
    pub appearance: Option<String>,
    #[serde(default)]
    pub license: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub codex_verified: Option<String>,
    #[serde(default)]
    pub bytes: u64,
    #[serde(default)]
    pub sha256: String,
    #[serde(default)]
    pub pack: String,
    #[serde(default)]
    pub preview: String,
}

#[derive(Debug, Clone, serde::Deserialize)]
struct CatalogIndex {
    #[serde(default)]
    skins: Vec<CatalogSkin>,
}

/// A catalog-relative path is plain (`packs/x.codexskin`) — no scheme, no
/// authority, no parent hops. Everything else is rejected before URL joining.
fn safe_catalog_path(rel: &str) -> Result<String, AppError> {
    let ok = !rel.is_empty()
        && !rel.contains("://")
        && !rel.starts_with('/')
        && !rel.contains("..")
        && rel
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"/-_.".contains(&b));
    if ok {
        Ok(format!("{SKINS_BASE}/{rel}"))
    } else {
        Err(AppError::Engine(format!("目录条目路径非法: {rel}")))
    }
}

fn curl_bin() -> &'static str {
    if cfg!(target_os = "windows") {
        "curl.exe"
    } else {
        "/usr/bin/curl"
    }
}

/// Fetch a URL to stdout via system curl: https only, no retries into other
/// protocols, hard timeout and size cap. `--retry` covers the transient
/// connection resets this route sees in the wild (CDN + long-haul links).
fn curl_fetch(url: &str, max_bytes: &str, timeout_secs: &str) -> Result<Vec<u8>, AppError> {
    let output = std::process::Command::new(curl_bin())
        .args([
            "-sfL",
            "--proto",
            "=https",
            "--retry",
            "2",
            "--max-time",
            timeout_secs,
            "--max-filesize",
            max_bytes,
            url,
        ])
        .output()
        .map_err(|e| AppError::Engine(format!("curl 不可用: {e}")))?;
    if !output.status.success() {
        return Err(AppError::Engine(format!(
            "下载失败 ({}): {}",
            output.status,
            crate::app::logging::redact_url(url)
        )));
    }
    Ok(output.stdout)
}

pub fn fetch_catalog() -> Result<Vec<CatalogSkin>, AppError> {
    let bytes = curl_fetch(&format!("{SKINS_BASE}/index.json"), CATALOG_MAX_BYTES, "15")?;
    let index: CatalogIndex = serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Engine(format!("皮肤目录解析失败: {e}")))?;
    let mut skins: Vec<CatalogSkin> = index
        .skins
        .into_iter()
        .filter(|s| !s.id.is_empty() && !s.pack.is_empty() && s.sha256.len() == 64)
        .collect();
    skins.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(skins)
}

/// Catalog cover preview as a data URL (WebP, ≤ 2 MB by convention).
pub fn catalog_preview_data_url(preview_rel: &str) -> Result<String, AppError> {
    use base64::Engine as _;
    let url = safe_catalog_path(preview_rel)?;
    let bytes = curl_fetch(&url, "2097152", "15")?;
    Ok(format!(
        "data:image/webp;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

/// Download + sha256-gate + install one catalog skin. Returns the installed
/// summary (the importer re-validates everything structurally).
pub fn install_from_catalog(skin_id: &str) -> Result<codex_theme_engine::theme::ThemeSummary, AppError> {
    use sha2::Digest as _;
    let skin = fetch_catalog()?
        .into_iter()
        .find(|s| s.id == skin_id)
        .ok_or_else(|| AppError::Engine(format!("目录中没有该皮肤: {skin_id}")))?;
    let url = safe_catalog_path(&skin.pack)?;
    let bytes = curl_fetch(&url, PACK_MAX_BYTES, "120")?;

    let digest = sha2::Sha256::digest(&bytes);
    let hex = digest
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    if !hex.eq_ignore_ascii_case(&skin.sha256) {
        return Err(AppError::Engine(format!(
            "校验失败：{skin_id} 的 sha256 与目录不符"
        )));
    }

    let staging = std::env::temp_dir().join(format!(
        "codexskin-online-{}-{}.codexskin",
        std::process::id(),
        skin_id
    ));
    std::fs::write(&staging, &bytes)
        .map_err(|e| AppError::Internal(format!("写入临时包失败: {e}")))?;
    let themes_root = store_dir(&AppSettings::load())?;
    let outcome = codex_theme_engine::import::import_codexskin(&staging, &themes_root)
        .map_err(|e| AppError::Engine(e.to_string()));
    let _ = std::fs::remove_file(&staging);
    log::info!(
        "online skin install id={skin_id} version={} ok={}",
        skin.version,
        outcome.is_ok()
    );
    outcome
}

/// Cover preview as a data URL; None when the theme has no preview, can't be
/// resolved, or the image fails to read (gallery falls back to swatch art).
pub fn preview_data_url(settings: &AppSettings, theme_ref: &str) -> Option<String> {
    use base64::Engine as _;
    let dir = resolve_theme(settings, theme_ref).ok()?;
    let theme = load_theme(&dir).ok()?;
    let rel = theme.meta.previews.first()?;
    let path = dir.join(rel);
    let mime = match path.extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => return None,
    };
    let bytes = std::fs::read(&path).ok()?;
    Some(format!(
        "data:{mime};base64,{}",
        base64::engine::general_purpose::STANDARD.encode(bytes)
    ))
}

impl ThemeService {
    /// Ensure the reconciliation daemon is running and return its directive
    /// handle. One daemon per manager process, lazily started.
    async fn directive_handle(&self) -> watch::Sender<Directive> {
        let mut inner = self.inner.lock().await;
        if let Some(tx) = &inner.directive_tx {
            if !tx.is_closed() {
                return tx.clone();
            }
        }
        let (directive_tx, directive_rx) = watch::channel::<Directive>(None);
        let (status_tx, status_rx) = watch::channel(DaemonStatus::default());
        tauri::async_runtime::spawn(run_daemon(THEME_CDP_PORT, directive_rx, status_tx));
        inner.directive_tx = Some(directive_tx.clone());
        inner.status_rx = Some(status_rx);
        directive_tx
    }

    async fn daemon_status(&self) -> Option<DaemonStatus> {
        let inner = self.inner.lock().await;
        inner.status_rx.as_ref().map(|rx| rx.borrow().clone())
    }

    pub async fn status(&self, settings: &AppSettings) -> ThemeStatusReport {
        let cdp_ready = codex_theme_engine::cdp::cdp_http_ready(THEME_CDP_PORT).await;
        let native_backup = native_paths().map(|p| has_backup(&p)).unwrap_or(false);
        ThemeStatusReport {
            supported: theme_supported(),
            active_theme: settings.codex_theme.clone(),
            daemon: self.daemon_status().await,
            cdp_ready,
            codex_running: codex_running(),
            native_backup_present: native_backup,
            store_dir: store_dir(settings).ok(),
        }
    }

    /// Live try-on: requires a debuggable Codex on the theme port. Does not
    /// touch persisted settings — the caller decides whether to keep it.
    pub async fn try_on(&self, settings: &AppSettings, theme_ref: &str) -> Result<(), AppError> {
        if !theme_supported() {
            return Err(AppError::UnsupportedPlatform);
        }
        let dir = resolve_theme(settings, theme_ref)?;
        // Validate eagerly so a broken package fails the command, not the
        // daemon's next tick.
        load_theme(&dir).map_err(|e| AppError::Engine(e.to_string()))?;
        if !codex_theme_engine::cdp::cdp_http_ready(THEME_CDP_PORT).await {
            return Err(AppError::Engine(
                "codex-not-debuggable: Codex 未以调试模式运行".to_string(),
            ));
        }
        let handle = self.directive_handle().await;
        handle
            .send(Some(dir))
            .map_err(|_| AppError::Internal("主题守护未运行".to_string()))
    }

    /// Live removal (renderers back to stock). Persisted selection is cleared
    /// by the command layer; config.toml is only touched by `off_full`.
    pub async fn turn_off_live(&self) -> Result<(), AppError> {
        let inner = self.inner.lock().await;
        if let Some(tx) = &inner.directive_tx {
            let _ = tx.send(None);
        }
        Ok(())
    }

    /// After a store migration, re-point a live directive whose theme dir
    /// moved with the store (the daemon rebuilds its payload from the new
    /// path on the next tick; injected renderers are untouched either way).
    pub async fn rebase_directive(&self, old_root: &Path, new_root: &Path) {
        let inner = self.inner.lock().await;
        let Some(tx) = &inner.directive_tx else {
            return;
        };
        let current = tx.borrow().clone();
        if let Some(dir) = current {
            if let Ok(rel) = dir.strip_prefix(old_root) {
                let rebased = new_root.join(rel);
                if rebased.join("theme.json").is_file() {
                    let _ = tx.send(Some(rebased));
                }
            }
        }
    }

    /// Try-on that first puts Codex into debug mode: graceful quit → relaunch
    /// with the loopback CDP port → inject. Unlike the full apply it writes
    /// NOTHING — no config.toml sections, no persisted selection; the top
    /// banner's 保留 is what makes a try-on stick.
    pub async fn try_on_with_restart(
        &self,
        settings: &AppSettings,
        theme_ref: &str,
    ) -> Result<(), AppError> {
        self.restart_debuggable_and_inject(settings, theme_ref, false)
            .await
    }

    /// Full apply: quiesce Codex, write the native appearance sections, then
    /// relaunch with the loopback CDP port and inject. The only path that
    /// writes config.toml, honoring "only while Codex is stopped".
    pub async fn apply_with_restart(
        &self,
        settings: &AppSettings,
        theme_ref: &str,
    ) -> Result<(), AppError> {
        self.restart_debuggable_and_inject(settings, theme_ref, true)
            .await
    }

    async fn restart_debuggable_and_inject(
        &self,
        settings: &AppSettings,
        theme_ref: &str,
        write_native: bool,
    ) -> Result<(), AppError> {
        if !theme_supported() {
            return Err(AppError::UnsupportedPlatform);
        }
        let dir = resolve_theme(settings, theme_ref)?;
        let theme = load_theme(&dir).map_err(|e| AppError::Engine(e.to_string()))?;

        let disable_self_updates = settings.disable_codex_self_updates;
        let codex_theme_block = theme.codex_theme.clone().filter(|_| write_native);
        tauri::async_runtime::spawn_blocking(move || -> Result<(), AppError> {
            let installed = installed_codex_path()?;
            quit_codex(&installed)?;
            if let Some(block) = &codex_theme_block {
                std::thread::sleep(CONFIG_SETTLE);
                let paths = native_paths()?;
                codex_theme_engine::native::apply_native_theme(&paths, block)
                    .map_err(|e| AppError::Engine(e.to_string()))?;
            }
            launch_codex_with_cdp(&installed, THEME_CDP_PORT, disable_self_updates)
        })
        .await
        .map_err(|e| AppError::Internal(format!("主题应用任务失败: {e}")))??;

        let deadline = tokio::time::Instant::now() + CDP_WAIT;
        while !codex_theme_engine::cdp::cdp_http_ready(THEME_CDP_PORT).await {
            if tokio::time::Instant::now() >= deadline {
                return Err(AppError::Engine(
                    "Codex 已启动但调试端口未就绪".to_string(),
                ));
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        let handle = self.directive_handle().await;
        handle
            .send(Some(dir))
            .map_err(|_| AppError::Internal("主题守护未运行".to_string()))
    }

    /// Full restore: live removal + quiesce Codex, put the user's original
    /// appearance sections back, relaunch plainly (no CDP port).
    pub async fn off_full(&self) -> Result<(), AppError> {
        if !theme_supported() {
            return Err(AppError::UnsupportedPlatform);
        }
        self.turn_off_live().await?;
        tauri::async_runtime::spawn_blocking(move || -> Result<(), AppError> {
            let installed = installed_codex_path()?;
            let was_running = codex_running();
            quit_codex(&installed)?;
            std::thread::sleep(CONFIG_SETTLE);
            let paths = native_paths()?;
            codex_theme_engine::native::restore_native_theme(&paths)
                .map_err(|e| AppError::Engine(e.to_string()))?;
            if was_running {
                crate::app::mac_update::launch_codex()?;
            }
            Ok(())
        })
        .await
        .map_err(|e| AppError::Internal(format!("主题还原任务失败: {e}")))?
    }
}

#[cfg(target_os = "macos")]
fn installed_codex_path() -> Result<PathBuf, AppError> {
    crate::app::mac_update::detect_managed_installed()
        .map(|installed| PathBuf::from(installed.path))
        .ok_or_else(|| AppError::Engine("没有可用的 Codex 安装".to_string()))
}

#[cfg(target_os = "macos")]
fn codex_running() -> bool {
    installed_codex_path()
        .map(|path| codex_mac_engine::swap::codex_running_at(&path))
        .unwrap_or(false)
}

#[cfg(target_os = "macos")]
fn quit_codex(installed: &std::path::Path) -> Result<(), AppError> {
    codex_mac_engine::swap::quit_codex_at(installed, QUIT_TIMEOUT_SECS)
        .map_err(|e| AppError::Engine(format!("退出 Codex 失败: {e}")))
}

/// `open -n -a <bundle> --args --remote-debugging-…`: `-n` is required for
/// argument delivery (without a new instance, `open` merely activates the
/// running app and drops the args) — callers must have quiesced Codex first.
#[cfg(target_os = "macos")]
fn launch_codex_with_cdp(    installed: &std::path::Path,
    port: u16,
    disable_self_updates: bool,
) -> Result<(), AppError> {
    if disable_self_updates {
        crate::app::codex_self_update::sync_setting(true)?;
    }
    log::info!(
        "launching Codex with CDP port={port} path={}",
        installed.display()
    );
    let mut command = std::process::Command::new("/usr/bin/open");
    crate::app::codex_self_update::apply_to_command(&mut command, disable_self_updates);
    command
        .arg("-n")
        .arg("-a")
        .arg(installed)
        .arg("--args")
        .arg("--remote-debugging-address=127.0.0.1")
        .arg(format!("--remote-debugging-port={port}"))
        .spawn()
        .map(|_| ())
        .map_err(|e| AppError::Engine(format!("以调试模式打开 Codex 失败: {e}")))
}

#[cfg(not(target_os = "macos"))]
fn installed_codex_path() -> Result<PathBuf, AppError> {
    Err(AppError::UnsupportedPlatform)
}

#[cfg(not(target_os = "macos"))]
fn codex_running() -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
fn quit_codex(_installed: &std::path::Path) -> Result<(), AppError> {
    Err(AppError::UnsupportedPlatform)
}

#[cfg(not(target_os = "macos"))]
fn launch_codex_with_cdp(    _installed: &std::path::Path,
    _port: u16,
    _disable_self_updates: bool,
) -> Result<(), AppError> {
    Err(AppError::UnsupportedPlatform)
}

/// Launch hook for the ordinary 〔打开 Codex〕 action: when a theme is the
/// persisted selection, launching through the manager transparently becomes
/// "launch debuggable + keep themed".
pub async fn launch_with_active_theme(
    service: &ThemeService,
    settings: &AppSettings,
) -> Result<bool, AppError> {
    let Some(theme_ref) = settings.codex_theme.clone() else {
        return Ok(false);
    };
    if !theme_supported() {
        return Ok(false);
    }
    match service.apply_with_restart(settings, &theme_ref).await {
        Ok(()) => Ok(true),
        Err(error) => {
            // A broken/missing theme must never brick the launch button:
            // surface in logs, launch plainly.
            log::warn!("themed launch failed, falling back to plain launch: {error}");
            Ok(false)
        }
    }
}
