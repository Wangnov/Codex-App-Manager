use std::{
    collections::HashMap,
    future::Future,
    path::{Path, PathBuf},
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

use codex_win_engine::InstalledWindowsCodex;
use futures_util::StreamExt;
use reqwest::header::CONTENT_LENGTH;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
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
    discard_macos_download, install_macos_with_network, mac_adopt_path as adopt_macos_path,
    pause_macos_download, perform_macos_update_with_network_and_phase,
    plan_macos_update_with_network, retry_macos_ancillary, stage_macos_update_with_network,
    uninstall_macos, InstalledCodex, MacInstallStatus, MacPerformReport, MacStageReport,
    MacUninstallReport, MacUpdateReport, PerformExpectation,
};
use crate::app::manager_update_handoff::clear_for_platform as clear_manager_update_handoff;
#[cfg(target_os = "windows")]
use crate::app::manager_update_handoff::{
    now_unix_ms, persist_for_platform as persist_manager_update_handoff, ManagerUpdateHandoff,
};
use crate::app::manager_update_handoff::{
    status_for_platform as manager_update_handoff_status, ManagerUpdateHandoffStatus,
};
use crate::app::manager_update_runtime::ManagerUpdateRuntimeSnapshot;
use crate::app::op_phase::{OperationPhase, QuitPolicy};
use crate::app::operation_outcome::{AncillaryRetryReport, AncillaryRetryRequest};
use crate::app::oplock::{
    OperationError, OperationGuard, OperationKind, OperationManager, OperationProgress,
    OperationSnapshot, OperationToken,
};
use crate::app::paths;
use crate::app::provenance::ProvenanceStore;
use crate::app::settings_store::AppSettings as PersistedAppSettings;
use crate::app::settings_store::{ProxyMode, UpdateSource};
use crate::app::url_guard::{validate_custom_proxy, validate_custom_source};
use crate::app::win_update::{
    auto_stage_windows_update_with_install_mode_and_network, cancel_windows_download,
    detect_existing_windows_install_at_path as detect_windows_install_at_path,
    discard_windows_download, pause_windows_download,
    perform_windows_update_with_install_mode_network_and_phase,
    plan_windows_update_with_install_mode_and_network, retry_windows_ancillary,
    stage_windows_update_with_install_mode_and_network, uninstall_windows_codex,
    win_adopt as adopt_windows_install, win_adopt_path as adopt_windows_path, win_install_status,
    DownloadProgress as WinDownloadProgress, WinAutoStageReport, WinInstallStatus,
    WinPerformExpectation, WinPerformReport, WinStageReport, WinUninstallReport, WinUpdateReport,
};
use crate::domain::settings::AppSettings as DomainAppSettings;
use crate::domain::target::OperatingSystem;
use crate::errors::{AppError, CommandError};
use crate::state::{manager_update_handoff_timeout, ManagerState};

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

const MANAGER_UPDATE_STATE_EVENT: &str = "manager://update-state";
const MANAGER_UPDATE_CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const MANAGER_UPDATE_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const MANAGER_UPDATE_READ_TIMEOUT: Duration = Duration::from_secs(30);
const MANAGER_UPDATE_ARTIFACT_TIMEOUT: Duration = Duration::from_secs(15 * 60);
const MANAGER_UPDATE_MANIFEST_MAX_BYTES: u64 = 256 * 1024;
const MANAGER_UPDATE_IDENTITY_MAX_BYTES: u64 = 256 * 1024;
const MANAGER_UPDATE_IDENTITY_SIGNATURE_MAX_BYTES: u64 = 16 * 1024;
const MANAGER_UPDATE_ARTIFACT_MAX_BYTES: u64 = 64 * 1024 * 1024;
const MANAGER_UPDATE_IDENTITY_SCHEMA: u32 = 1;
const MANAGER_UPDATE_IDENTITY_FILE: &str = "release-identity.json";
const MANAGER_UPDATE_IDENTITY_SIGNATURE_FILE: &str = "release-identity.json.sig";

#[derive(Debug, Deserialize)]
struct ManagerReleaseIdentity {
    schema: u32,
    version: String,
    channel: ManagerReleaseChannel,
    notes_sha256: String,
    platforms: HashMap<String, ManagerReleaseIdentityPlatform>,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum ManagerReleaseChannel {
    Stable,
    Prerelease,
}

#[derive(Debug, Deserialize)]
struct ManagerReleaseIdentityPlatform {
    artifact: String,
    // The patched updater verifies the manifest-provided Minisign signature;
    // the signed authority independently binds the exact artifact bytes.
    sha256: String,
}

struct AuthenticatedManagerUpdate {
    update: tauri_plugin_updater::Update,
    artifact_sha256: String,
}

fn emit_manager_update_state(app: &AppHandle, snapshot: ManagerUpdateRuntimeSnapshot) {
    let _ = app.emit(MANAGER_UPDATE_STATE_EVENT, snapshot);
}

fn persist_manager_update_handoff_before_install(
    snapshot: &ManagerUpdateRuntimeSnapshot,
) -> Result<Option<u64>, AppError> {
    #[cfg(target_os = "windows")]
    {
        let started_at_unix_ms = now_unix_ms();
        persist_manager_update_handoff(&ManagerUpdateHandoff {
            version: snapshot.version.clone(),
            current_version: snapshot.current_version.clone(),
            body: snapshot.body.clone(),
            started_at_unix_ms,
        })
        .map_err(|error| AppError::Internal(format!("persist manager updater handoff: {error}")))?;
        Ok(Some(started_at_unix_ms))
    }
    #[cfg(not(target_os = "windows"))]
    {
        let _ = snapshot;
        Ok(None)
    }
}

fn release_manager_update_handoff_guard(state: &ManagerState) {
    state
        .manager_update_handoff_guard
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
}

fn manager_updater_builder_for_endpoints(
    app: &AppHandle,
    endpoints: Option<Vec<url::Url>>,
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
    if let Some(endpoints) = endpoints {
        builder = builder
            .endpoints(endpoints)
            .map_err(|e| AppError::Engine(format!("configure manager updater endpoints: {e}")))?;
    }
    // The builder timeout applies to manifest checks. The shared client hook
    // intentionally sets only connection/read stall limits; the authenticated
    // Update gets its independent artifact total timeout below.
    Ok(builder
        .timeout(MANAGER_UPDATE_CHECK_TIMEOUT)
        .max_manifest_size(MANAGER_UPDATE_MANIFEST_MAX_BYTES)
        .max_download_size(MANAGER_UPDATE_ARTIFACT_MAX_BYTES)
        .configure_client(|client| {
            client
                .connect_timeout(MANAGER_UPDATE_CONNECT_TIMEOUT)
                .read_timeout(MANAGER_UPDATE_READ_TIMEOUT)
        }))
}

fn configured_manager_update_config(
    app: &AppHandle,
) -> Result<tauri_plugin_updater::Config, AppError> {
    let raw = app
        .config()
        .plugins
        .0
        .get("updater")
        .cloned()
        .ok_or_else(|| AppError::Internal("missing updater plugin configuration".to_string()))?;
    serde_json::from_value(raw)
        .map_err(|e| AppError::Internal(format!("read updater plugin configuration: {e}")))
}

fn manager_update_identity_client() -> Result<reqwest::Client, AppError> {
    let saved = PersistedAppSettings::load();
    let mut builder = reqwest::Client::builder()
        .connect_timeout(MANAGER_UPDATE_CONNECT_TIMEOUT)
        .read_timeout(MANAGER_UPDATE_READ_TIMEOUT)
        .timeout(MANAGER_UPDATE_CHECK_TIMEOUT);
    match saved.proxy_mode {
        ProxyMode::System => {}
        ProxyMode::Direct => builder = builder.no_proxy(),
        ProxyMode::Custom => {
            let normalized =
                validated_custom_proxy_for_settings(&saved.custom_proxy_url, "manager updater")?;
            let proxy = reqwest::Proxy::all(&normalized).map_err(|error| {
                manager_update_reqwest_error("configure", "manager updater proxy", error)
            })?;
            builder = builder.proxy(proxy);
        }
    }
    builder.build().map_err(|error| {
        manager_update_reqwest_error("build", "manager updater identity client", error)
    })
}

fn manager_update_reqwest_error(
    operation: &'static str,
    resource: &'static str,
    error: reqwest::Error,
) -> AppError {
    // reqwest associates the final URL with request, redirect, status, and
    // response-body errors. That URL can be an object-store presigned URL, so
    // never let its path or query cross the AppError/CommandError boundary.
    let error = error.without_url();
    let category = if error.is_timeout() {
        "timeout"
    } else if error.is_connect() {
        "connect"
    } else if error.is_redirect() {
        "redirect"
    } else if error.is_status() {
        "status"
    } else if error.is_body() {
        "body"
    } else if error.is_decode() {
        "decode"
    } else if error.is_request() {
        "request"
    } else if error.is_builder() {
        "builder"
    } else {
        "transport"
    };
    let status = error
        .status()
        .map(|status| format!("; HTTP {status}"))
        .unwrap_or_default();
    let diagnostic = if error.is_timeout() {
        format!("network timeout (timed out; {category}{status})")
    } else {
        format!("network error ({category}{status})")
    };
    AppError::Engine(format!("{operation} {resource}: {diagnostic}"))
}

async fn fetch_manager_update_file_limited(
    client: &reqwest::Client,
    url: url::Url,
    max_bytes: u64,
    resource: &'static str,
) -> Result<Vec<u8>, AppError> {
    let response = client
        .get(url)
        .send()
        .await
        .map_err(|error| manager_update_reqwest_error("fetch", resource, error))?;
    if !response.status().is_success() {
        return Err(AppError::Engine(format!(
            "fetch {resource}: HTTP {}",
            response.status()
        )));
    }

    let content_length = response
        .headers()
        .get(CONTENT_LENGTH)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse::<u64>().ok());
    if let Some(announced) = content_length.filter(|length| *length > max_bytes) {
        return Err(AppError::Engine(format!(
            "{resource} exceeds {max_bytes}-byte limit (announced {announced} bytes)"
        )));
    }

    let mut bytes = Vec::new();
    let mut observed = 0_u64;
    let mut stream = response.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| manager_update_reqwest_error("read", resource, error))?;
        observed = observed.checked_add(chunk.len() as u64).ok_or_else(|| {
            AppError::Engine(format!("{resource} exceeds {max_bytes}-byte limit"))
        })?;
        if observed > max_bytes {
            return Err(AppError::Engine(format!(
                "{resource} exceeds {max_bytes}-byte limit (observed {observed} bytes)"
            )));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn is_safe_manager_update_filename(filename: &str) -> bool {
    !filename.is_empty()
        && filename != "."
        && filename != ".."
        && filename
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

fn manager_update_root_url(
    manifest_endpoint: &url::Url,
    filename: &str,
) -> Result<url::Url, AppError> {
    if !is_safe_manager_update_filename(filename) {
        return Err(AppError::Engine(
            "signed manager update identity contains an unsafe artifact name".to_string(),
        ));
    }

    let mut url = manifest_endpoint.clone();
    url.set_query(None);
    url.set_fragment(None);
    let base_segments = manifest_endpoint
        .path_segments()
        .ok_or_else(|| AppError::Engine("manager updater endpoint has no path".to_string()))?
        .collect::<Vec<_>>();
    let Some((manifest_name, directories)) = base_segments.split_last() else {
        return Err(AppError::Engine(
            "manager updater endpoint has no manifest filename".to_string(),
        ));
    };
    if *manifest_name != "latest.json" {
        return Err(AppError::Engine(
            "manager updater endpoint must end in latest.json".to_string(),
        ));
    }
    url.set_path("");
    {
        let mut segments = url.path_segments_mut().map_err(|_| {
            AppError::Engine("manager updater endpoint cannot be a base URL".to_string())
        })?;
        for segment in directories {
            if !segment.is_empty() {
                segments.push(segment);
            }
        }
        segments.push(filename);
    }
    Ok(url)
}

fn manager_update_versioned_url(
    manifest_endpoint: &url::Url,
    version: &str,
    filename: &str,
) -> Result<url::Url, AppError> {
    if !is_safe_manager_update_filename(filename) {
        return Err(AppError::Engine(
            "signed manager update identity contains an unsafe artifact name".to_string(),
        ));
    }

    let mut url = manifest_endpoint.clone();
    url.set_query(None);
    url.set_fragment(None);
    let is_github = url.scheme() == "https"
        && url.host_str() == Some("github.com")
        && url.path() == "/Wangnov/Codex-App-Manager/releases/latest/download/latest.json";
    url.set_path("");
    {
        let mut segments = url.path_segments_mut().map_err(|_| {
            AppError::Engine("manager updater endpoint cannot be a base URL".to_string())
        })?;
        if is_github {
            for segment in ["Wangnov", "Codex-App-Manager", "releases", "download"] {
                segments.push(segment);
            }
            segments.push(&format!("v{version}"));
        } else {
            let base_segments = manifest_endpoint
                .path_segments()
                .ok_or_else(|| {
                    AppError::Engine("manager updater endpoint has no path".to_string())
                })?
                .collect::<Vec<_>>();
            let Some((manifest_name, directories)) = base_segments.split_last() else {
                return Err(AppError::Engine(
                    "manager updater endpoint has no manifest filename".to_string(),
                ));
            };
            if *manifest_name != "latest.json" {
                return Err(AppError::Engine(
                    "manager updater endpoint must end in latest.json".to_string(),
                ));
            }
            for segment in directories {
                if !segment.is_empty() {
                    segments.push(segment);
                }
            }
            segments.push(version);
        }
        segments.push(filename);
    }
    Ok(url)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn validate_manager_release_identity_authority(
    identity: &ManagerReleaseIdentity,
) -> Result<(), AppError> {
    if identity.schema != MANAGER_UPDATE_IDENTITY_SCHEMA {
        return Err(AppError::Engine(
            "signed manager update identity has an unsupported schema".to_string(),
        ));
    }
    if identity.channel != ManagerReleaseChannel::Stable
        || identity
            .version
            .split_once('+')
            .map_or(identity.version.as_str(), |(version, _)| version)
            .contains('-')
    {
        return Err(AppError::Engine(
            "stable manager updater rejects prerelease release identity".to_string(),
        ));
    }
    Ok(())
}

fn validate_manager_update_identity_claim(
    manifest_version: &str,
    manifest_channel: &str,
    notes: Option<&str>,
    platform_key: &str,
    manifest_artifact: &str,
    manifest_sha256: &str,
    identity: &ManagerReleaseIdentity,
) -> Result<String, AppError> {
    validate_manager_release_identity_authority(identity)?;
    if identity.version != manifest_version || manifest_channel != "stable" {
        return Err(AppError::Engine(
            "manager update manifest does not match the signed release identity".to_string(),
        ));
    }
    if identity.notes_sha256 != sha256_hex(notes.unwrap_or_default().as_bytes()) {
        return Err(AppError::Engine(
            "manager update notes do not match the signed release identity".to_string(),
        ));
    }
    let platform = identity.platforms.get(platform_key).ok_or_else(|| {
        AppError::Engine(
            "signed manager update identity has no entry for this platform".to_string(),
        )
    })?;
    if platform.artifact != manifest_artifact || platform.sha256 != manifest_sha256 {
        return Err(AppError::Engine(
            "manager update artifact does not match the signed release identity".to_string(),
        ));
    }
    let valid_sha256 = platform.sha256.len() == 64
        && platform
            .sha256
            .bytes()
            .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase());
    if !valid_sha256 {
        return Err(AppError::Engine(
            "signed manager update identity has an invalid artifact digest".to_string(),
        ));
    }
    Ok(platform.sha256.clone())
}

async fn fetch_authenticated_manager_release_identity(
    client: &reqwest::Client,
    manifest_endpoint: &url::Url,
    pubkey: &str,
) -> Result<ManagerReleaseIdentity, AppError> {
    // This root authority is deliberately independent of unsigned
    // `latest.json`; the feed cannot select which signed identity is fetched.
    let identity_url = manager_update_root_url(manifest_endpoint, MANAGER_UPDATE_IDENTITY_FILE)?;
    let signature_url =
        manager_update_root_url(manifest_endpoint, MANAGER_UPDATE_IDENTITY_SIGNATURE_FILE)?;
    let identity_bytes = fetch_manager_update_file_limited(
        client,
        identity_url,
        MANAGER_UPDATE_IDENTITY_MAX_BYTES,
        "manager release identity",
    )
    .await?;
    let signature_bytes = fetch_manager_update_file_limited(
        client,
        signature_url,
        MANAGER_UPDATE_IDENTITY_SIGNATURE_MAX_BYTES,
        "manager release identity signature",
    )
    .await?;
    let signature = std::str::from_utf8(&signature_bytes)
        .map(str::trim)
        .map_err(|_| {
            AppError::Engine("manager release identity signature is not UTF-8".to_string())
        })?;
    tauri_plugin_updater::verify_signature(&identity_bytes, signature, pubkey)
        .map_err(|e| AppError::Engine(format!("verify manager release identity: {e}")))?;
    let identity: ManagerReleaseIdentity = serde_json::from_slice(&identity_bytes)
        .map_err(|e| AppError::Engine(format!("parse manager release identity: {e}")))?;
    validate_manager_release_identity_authority(&identity)?;
    Ok(identity)
}

fn manager_update_manifest_claim(
    update: &tauri_plugin_updater::Update,
) -> Result<(String, String, String), AppError> {
    let version = update
        .raw_json
        .get("version")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| AppError::Engine("manager update manifest has no version".to_string()))?;
    let channel = update
        .raw_json
        .get("channel")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| AppError::Engine("manager update manifest has no channel".to_string()))?;
    let sha256 = update
        .raw_json
        .get("platforms")
        .and_then(|platforms| platforms.get(&update.platform))
        .and_then(|platform| platform.get("sha256"))
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            AppError::Engine(
                "manager update manifest has no artifact digest for this platform".to_string(),
            )
        })?;
    Ok((version.to_string(), channel.to_string(), sha256.to_string()))
}

fn authenticate_manager_update(
    manifest_endpoint: &url::Url,
    mut update: tauri_plugin_updater::Update,
    identity: &ManagerReleaseIdentity,
) -> Result<AuthenticatedManagerUpdate, AppError> {
    let (manifest_version, manifest_channel, manifest_sha256) =
        manager_update_manifest_claim(&update)?;
    if update.version != manifest_version {
        return Err(AppError::Engine(
            "manager update manifest version was not parsed canonically".to_string(),
        ));
    }
    let manifest_artifact = update
        .download_url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .unwrap_or_default();
    let artifact_sha256 = validate_manager_update_identity_claim(
        &manifest_version,
        &manifest_channel,
        update.body.as_deref(),
        &update.platform,
        manifest_artifact,
        &manifest_sha256,
        identity,
    )?;
    let artifact = identity
        .platforms
        .get(&update.platform)
        .expect("validated identity platform")
        .artifact
        .clone();
    update.download_url =
        manager_update_versioned_url(manifest_endpoint, &identity.version, &artifact)?;
    // Only the artifact selected by the signed identity receives the longer
    // total transfer window. Manifest checks retain the builder's 30s bound.
    update.timeout = Some(MANAGER_UPDATE_ARTIFACT_TIMEOUT);
    Ok(AuthenticatedManagerUpdate {
        update,
        artifact_sha256,
    })
}

async fn download_with_manager_fallback<T, Attempt, AttemptFuture>(
    endpoints: Vec<url::Url>,
    mut attempt: Attempt,
) -> Result<T, AppError>
where
    Attempt: FnMut(url::Url) -> AttemptFuture,
    AttemptFuture: Future<Output = Result<T, AppError>>,
{
    let mut failures = Vec::new();
    let mut stale_failures = Vec::new();
    for endpoint in endpoints {
        let origin = redact_url(endpoint.as_str());
        match attempt(endpoint).await {
            Ok(value) => return Ok(value),
            // Consent is bound to the exact expected version/current pair, not
            // to one particular feed. A stale mirror may safely fall through to
            // GitHub, but no mismatching package is ever downloaded/installed.
            Err(AppError::StaleExpectation(message)) => {
                log::warn!("manager updater endpoint is stale origin={origin} error={message}");
                stale_failures.push(format!("{origin}: {message}"));
            }
            Err(err) => {
                log::warn!("manager updater endpoint attempt failed origin={origin} error={err}");
                failures.push(format!("{origin}: {err}"));
            }
        }
    }

    if !stale_failures.is_empty() {
        let mut details = stale_failures;
        details.extend(failures);
        return Err(AppError::StaleExpectation(format!(
            "管理器更新内容已变化，请重新检查后再确认。({})",
            details.join("; ")
        )));
    }

    Err(AppError::Engine(format!(
        "manager update download failed for all configured endpoints: {}",
        failures.join("; ")
    )))
}

async fn check_with_manager_fallback<T, Attempt, AttemptFuture>(
    endpoints: Vec<url::Url>,
    mut attempt: Attempt,
) -> Result<Option<T>, AppError>
where
    Attempt: FnMut(url::Url) -> AttemptFuture,
    AttemptFuture: Future<Output = Result<Option<T>, AppError>>,
{
    let mut failures_after_last_success = Vec::new();
    let mut saw_no_update = false;

    for endpoint in endpoints {
        let origin = redact_url(endpoint.as_str());
        match attempt(endpoint).await {
            Ok(Some(update)) => return Ok(Some(update)),
            Ok(None) => {
                // A valid but stale mirror (including HTTP 204) is not proof
                // that the authoritative fallback has no update. Continue all
                // the way through the configured feeds. A later successful
                // no-update result supersedes earlier transport failures.
                log::debug!("manager updater endpoint has no update origin={origin}");
                saw_no_update = true;
                failures_after_last_success.clear();
            }
            Err(err) => {
                log::warn!("manager updater check failed origin={origin} error={err}");
                failures_after_last_success.push(format!("{origin}: {err}"));
            }
        }
    }

    if saw_no_update && failures_after_last_success.is_empty() {
        return Ok(None);
    }

    Err(AppError::Engine(format!(
        "manager update check failed for all configured endpoints: {}",
        failures_after_last_success.join("; ")
    )))
}

async fn download_then_install_manager_update<T, Attempt, AttemptFuture, Install>(
    endpoints: Vec<url::Url>,
    attempt: Attempt,
    install: Install,
) -> Result<(), AppError>
where
    Attempt: FnMut(url::Url) -> AttemptFuture,
    AttemptFuture: Future<Output = Result<T, AppError>>,
    Install: FnOnce(T) -> Result<(), AppError>,
{
    // The fallback boundary ends when a fully downloaded package has passed its
    // Tauri signature verification. Installation is invoked exactly once.
    let package = download_with_manager_fallback(endpoints, attempt).await?;
    install(package)
}

struct DownloadedManagerUpdate {
    update: tauri_plugin_updater::Update,
    bytes: Vec<u8>,
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
    let config = configured_manager_update_config(&app)?;
    if config.endpoints.is_empty() {
        return Err(
            AppError::Internal("updater plugin has no configured endpoints".to_string()).into(),
        );
    }
    let endpoints = config.endpoints.clone();
    let pubkey = Arc::new(config.pubkey);
    let identity_client = manager_update_identity_client()?;
    let update = check_with_manager_fallback(endpoints, |endpoint| {
        let app = app.clone();
        let pubkey = Arc::clone(&pubkey);
        let identity_client = identity_client.clone();
        async move {
            let identity = fetch_authenticated_manager_release_identity(
                &identity_client,
                &endpoint,
                pubkey.as_str(),
            )
            .await?;
            let updater =
                manager_updater_builder_for_endpoints(&app, Some(vec![endpoint.clone()]))?
                    .build()
                    .map_err(|e| AppError::Engine(format!("build manager updater: {e}")))?;
            let update = updater
                .check()
                .await
                .map_err(|e| AppError::Engine(format!("check manager update: {e}")))?;
            match update {
                Some(update) => authenticate_manager_update(&endpoint, update, &identity).map(Some),
                None => Ok(None),
            }
        }
    })
    .await?;
    Ok(update.map(|authenticated| ManagerUpdateMetadata {
        version: authenticated.update.version,
        current_version: authenticated.update.current_version,
        body: authenticated.update.body,
    }))
}

#[tauri::command]
pub fn manager_get_update_runtime(
    app: AppHandle,
    state: State<'_, ManagerState>,
) -> Option<ManagerUpdateRuntimeSnapshot> {
    let snapshot = state.manager_update.snapshot()?;
    if snapshot.handoff_started_at.is_none() {
        return Some(snapshot);
    }

    let current_version = app.package_info().version.to_string();
    match manager_update_handoff_status(&current_version) {
        ManagerUpdateHandoffStatus::Active(record)
            if record.version == snapshot.version
                && record.current_version == snapshot.current_version
                && record.started_at_unix_ms == snapshot.handoff_started_at.unwrap_or_default() =>
        {
            Some(snapshot)
        }
        ManagerUpdateHandoffStatus::Active(_)
        | ManagerUpdateHandoffStatus::Expired(_)
        | ManagerUpdateHandoffStatus::None => {
            // Missing, mismatched, or expired durable evidence must release the
            // recovered cross-process lock and become an explicit retry surface.
            clear_manager_update_handoff();
            release_manager_update_handoff_guard(&state);
            state
                .manager_update
                .failed(manager_update_handoff_timeout())
        }
    }
}

#[tauri::command]
pub fn manager_ack_update_runtime(
    state: State<'_, ManagerState>,
    revision: u64,
    version: String,
    current_version: String,
) -> bool {
    state
        .manager_update
        .acknowledge_terminal(revision, &version, &current_version)
}

#[tauri::command]
pub async fn manager_install_update(
    app: AppHandle,
    state: State<'_, ManagerState>,
    expected_version: String,
    expected_current_version: String,
    expected_body: Option<String>,
) -> Result<(), CommandError> {
    // Own the same process/cross-process lock as Codex mutations. This is both
    // the final busy preflight and a race-free reservation: once acquired, no
    // install/update can start while the manager package is downloading.
    let op = begin_guard(&state, OperationKind::ManagerUpdate)?;
    let start = state.manager_update.begin(
        expected_version.clone(),
        expected_current_version.clone(),
        expected_body,
    );
    emit_manager_update_state(&app, start);

    let result = async {
        state
            .operations
            .set_phase(op.token(), OperationPhase::Downloading)
            .map_err(AppError::from)?;
        let config = configured_manager_update_config(&app)?;
        if config.endpoints.is_empty() {
            return Err(AppError::Internal(
                "updater plugin has no configured endpoints".to_string(),
            ));
        }
        let endpoints = config.endpoints.clone();
        let pubkey = Arc::new(config.pubkey);
        let identity_client = manager_update_identity_client()?;
        let runtime_for_attempts = state.manager_update.clone();
        let operations_for_attempts = state.operations.clone();
        let token_for_attempts = op.token().clone();
        let app_for_attempts = app.clone();
        let expected_version = Arc::new(expected_version);
        let expected_current_version = Arc::new(expected_current_version);

        download_then_install_manager_update(
            endpoints,
            move |endpoint| {
                let runtime = runtime_for_attempts.clone();
                let operations = operations_for_attempts.clone();
                let token = token_for_attempts.clone();
                let app = app_for_attempts.clone();
                let expected_version = Arc::clone(&expected_version);
                let expected_current_version = Arc::clone(&expected_current_version);
                let pubkey = Arc::clone(&pubkey);
                let identity_client = identity_client.clone();
                async move {
                    operations
                        .set_phase(&token, OperationPhase::Downloading)
                        .map_err(AppError::from)?;
                    if let Some(snapshot) = runtime.downloading(0, None) {
                        emit_manager_update_state(&app, snapshot);
                    }

                    let identity = fetch_authenticated_manager_release_identity(
                        &identity_client,
                        &endpoint,
                        pubkey.as_str(),
                    )
                    .await?;
                    let updater =
                        manager_updater_builder_for_endpoints(&app, Some(vec![endpoint.clone()]))?
                            .build()
                            .map_err(|e| AppError::Engine(format!("build manager updater: {e}")))?;
                    let update = updater
                        .check()
                        .await
                        .map_err(|e| {
                            AppError::Engine(format!("check manager update before install: {e}"))
                        })?
                        .ok_or_else(|| {
                            AppError::StaleExpectation(
                                "管理器更新内容已变化，请重新检查后再确认。".to_string(),
                            )
                        })?;
                    let authenticated = authenticate_manager_update(&endpoint, update, &identity)?;
                    let update = authenticated.update;
                    if !manager_update_matches_confirmation(
                        &update.version,
                        &update.current_version,
                        expected_version.as_str(),
                        expected_current_version.as_str(),
                    ) {
                        return Err(AppError::StaleExpectation(
                            "管理器更新内容已变化，请重新检查后再确认。".to_string(),
                        ));
                    }

                    let source = redact_url(endpoint.as_str());
                    let runtime_for_progress = runtime.clone();
                    let operations_for_progress = operations.clone();
                    let token_for_progress = token.clone();
                    let app_for_progress = app.clone();
                    let operations_for_verify = operations.clone();
                    let token_for_verify = token.clone();
                    let mut downloaded = 0_u64;
                    let bytes = update
                        .download(
                            move |chunk, total| {
                                downloaded = downloaded.saturating_add(chunk as u64);
                                let _ = operations_for_progress.set_progress(
                                    &token_for_progress,
                                    OperationProgress {
                                        downloaded,
                                        total: total.unwrap_or(0),
                                        source: source.clone(),
                                    },
                                );
                                if let Some(snapshot) =
                                    runtime_for_progress.downloading(downloaded, total)
                                {
                                    emit_manager_update_state(&app_for_progress, snapshot);
                                }
                            },
                            move || {
                                // `download` invokes this before minisign verification.
                                // Verifying remains interruptible and may still fall back.
                                let _ = operations_for_verify
                                    .set_phase(&token_for_verify, OperationPhase::Verifying);
                            },
                        )
                        .await
                        .map_err(|e| AppError::Engine(format!("download manager update: {e}")))?;
                    let actual_sha256 = sha256_hex(&bytes);
                    if actual_sha256 != authenticated.artifact_sha256 {
                        return Err(AppError::Engine(
                            "downloaded manager update does not match the signed release identity digest"
                                .to_string(),
                        ));
                    }
                    Ok(DownloadedManagerUpdate { update, bytes })
                }
            },
            |package| {
                // This is the no-return boundary on Windows. Set all durable and
                // renderer-independent state before invoking the installer.
                state
                    .operations
                    .set_phase(op.token(), OperationPhase::Committing)
                    .map_err(AppError::from)?;
                let runtime = state.manager_update.snapshot().ok_or_else(|| {
                    AppError::Internal("manager update runtime disappeared before install".into())
                })?;
                let handoff_started_at = persist_manager_update_handoff_before_install(&runtime)?;
                let snapshot = state
                    .manager_update
                    .installing(handoff_started_at)
                    .ok_or_else(|| {
                        clear_manager_update_handoff();
                        AppError::Internal(
                            "manager update runtime disappeared during install handoff".into(),
                        )
                    })?;
                emit_manager_update_state(&app, snapshot);
                let installed = package
                    .update
                    .install(package.bytes)
                    .map_err(|e| AppError::Engine(format!("install manager update: {e}")));
                if installed.is_err() {
                    clear_manager_update_handoff();
                }
                installed
            },
        )
        .await?;

        state
            .operations
            .set_phase(op.token(), OperationPhase::Finishing)
            .map_err(AppError::from)?;
        if let Some(snapshot) = state.manager_update.installed() {
            emit_manager_update_state(&app, snapshot);
        }
        clear_manager_update_handoff();
        Ok::<(), AppError>(())
    }
    .await;

    match result {
        Ok(()) => Ok(()),
        Err(err) => {
            let command_error = CommandError::from(err);
            if let Some(snapshot) = state.manager_update.failed(command_error.clone()) {
                emit_manager_update_state(&app, snapshot);
            }
            Err(command_error)
        }
    }
}

fn reserve_manager_relaunch(
    operations: &OperationManager,
) -> Result<OperationGuard, CommandError> {
    let guard = begin_operation_guard(operations, OperationKind::ManagerUpdate)?;
    operations
        .set_phase(guard.token(), OperationPhase::Committing)
        .map_err(AppError::from)
        .map_err(CommandError::from)?;
    Ok(guard)
}

#[tauri::command]
pub async fn manager_relaunch(
    app: AppHandle,
    state: State<'_, ManagerState>,
) -> Result<(), CommandError> {
    let relaunch_guard = reserve_manager_relaunch(&state.operations)?;
    // Run off the UI thread so Tauri delivers ExitRequested + Exit. The latter
    // lets the single-instance plugin release its mutex/socket before Tauri
    // spawns the replacement process. `restart() -> !` keeps the guard on this
    // worker stack until exit; `request_restart()` would return and reopen the
    // TOCTOU window we just closed.
    tauri::async_runtime::spawn_blocking(move || {
        let _relaunch_guard = relaunch_guard;
        app.state::<ManagerState>()
            .force_quit
            .store(true, Ordering::SeqCst);
        app.restart();
        #[allow(unreachable_code)]
        ()
    })
    .await
    .map_err(|error| AppError::Internal(format!("manager relaunch worker failed: {error}")))?;
    Err(AppError::Internal("manager relaunch returned without restarting".to_string()).into())
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

fn begin_operation_guard(
    operations: &OperationManager,
    kind: OperationKind,
) -> Result<OperationGuard, CommandError> {
    operations
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

fn begin_guard(state: &ManagerState, kind: OperationKind) -> Result<OperationGuard, CommandError> {
    begin_operation_guard(&state.operations, kind)
}

struct DetachedGuard {
    operations: OperationManager,
    token: Option<OperationToken>,
}

impl DetachedGuard {
    fn validate(
        state: &ManagerState,
        token: OperationToken,
        expected_kind: OperationKind,
    ) -> Result<Self, CommandError> {
        let operations = state.operations.clone();
        operations
            .validate_detached(&token, expected_kind)
            .map_err(destructive_token_error)?;
        Ok(Self {
            operations,
            token: Some(token),
        })
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
    let health =
        ConfigHealth::from_parts(settings_health, provenance_health).with_live_backup_flags();
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
    let op = DetachedGuard::validate(&state, token, OperationKind::Update)?;
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
        install_macos_with_network(&report, &network)
    })
    .await
    .map_err(|e| AppError::Internal(format!("join: {e}")))?
    .map_err(Into::into)
}

/// macOS-only: request pausing an active package download.
/// Partial bytes are left in place for the next resume-capable run.
#[tauri::command]
pub fn mac_pause_download(state: State<'_, ManagerState>) -> Result<bool, CommandError> {
    if !cfg!(target_os = "macos") {
        return Err(AppError::UnsupportedPlatform.into());
    }
    if let Some(snap) = state.operations.snapshot() {
        let _ = state.operations.set_paused(&OperationToken(snap.id), true);
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
        return Err(
            AppError::Internal(format!("已从 .bak 还原 {which}，但重新读取仍判定为损坏")).into(),
        );
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
        return Err(AppError::Internal(format!("已重置 {which}，但重新读取仍判定为损坏")).into());
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
            return Err(
                AppError::Internal("清除用户数据需要二次确认（confirm=true）".to_string()).into(),
            );
        }
        let token = token.ok_or_else(|| {
            AppError::Internal(
                "清除用户数据需要破坏性令牌（先 arm_destructive uninstall）".to_string(),
            )
        })?;
        RetryGuard::Detached(DetachedGuard::validate(
            &state,
            token,
            OperationKind::Uninstall,
        )?)
    } else {
        RetryGuard::Scoped(begin_guard(&state, OperationKind::Adopt)?)
    };
    match state.target.os {
        OperatingSystem::Macos => retry_macos_ancillary(actions, path, purge).map_err(Into::into),
        OperatingSystem::Windows => {
            let settings = windows_domain_settings_for_persisted(&state);
            retry_windows_ancillary(&settings, actions, path, purge).map_err(Into::into)
        }
        _ => Err(AppError::UnsupportedPlatform.into()),
    }
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
    // Renderer cleanup is intentionally limited to an armed token that no
    // backend worker has claimed. Internal guards use OperationManager::end.
    state
        .operations
        .cancel_unclaimed_detached(token)
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

/// The user confirmed quitting from the close dialog — flag it and exit so the
/// CloseRequested / ExitRequested guards stop intercepting and let it go.
/// Still refuses when the backend is in a non-interruptible install phase.
#[tauri::command]
pub fn confirm_quit(
    app: tauri::AppHandle,
    state: State<'_, ManagerState>,
) -> Result<(), CommandError> {
    let confirm_close = crate::app::settings_store::AppSettings::load().confirm_close;
    // Evaluate as if force_quit is not yet set so a point-of-no-return phase
    // still blocks even after the user clicks the confirm dialog.
    let policy = state.operations.quit_policy(false, confirm_close);
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
        let _ = app.emit("app://quit-blocked", &policy);
        return Err(AppError::Busy(reason.clone()).into());
    }
    // Interruptible phases: best-effort cancel so partial downloads settle cleanly.
    let _ = codex_mac_engine::cancel_active_download();
    let _ = codex_win_engine::cancel_active_download();
    state
        .force_quit
        .store(true, std::sync::atomic::Ordering::SeqCst);
    app.exit(0);
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
    selected
        .as_deref()
        .map(install_root_from_picked_dir)
        .transpose()
        .map_err(Into::into)
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
pub fn win_set_install_root(
    state: State<'_, ManagerState>,
    path: String,
) -> Result<PersistedAppSettings, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let _op = begin_guard(&state, OperationKind::SetInstallRoot)?;
    let install_root = validate_install_root_path(&path)?;
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
pub fn win_reset_install_root(
    state: State<'_, ManagerState>,
) -> Result<PersistedAppSettings, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    let _op = begin_guard(&state, OperationKind::SetInstallRoot)?;
    let install_root = validate_install_root_path(&PersistedAppSettings::default().install_root)?;
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
    let _op = DetachedGuard::validate(&state, token, OperationKind::Uninstall)?;
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
pub fn win_pause_download(state: State<'_, ManagerState>) -> Result<bool, CommandError> {
    if !matches!(state.target.os, OperatingSystem::Windows) {
        return Err(AppError::UnsupportedPlatform.into());
    }
    if let Some(snap) = state.operations.snapshot() {
        let _ = state.operations.set_paused(&OperationToken(snap.id), true);
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
    let op = DetachedGuard::validate(&state, token, OperationKind::Update)?;
    op.set_phase(OperationPhase::Preparing);
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
        perform_windows_update_with_install_mode_network_and_phase(
            &endpoints,
            &settings,
            confirm,
            &install_mode,
            expected,
            &report,
            &network,
            Some(&phase_hook),
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
    let _op = DetachedGuard::validate(&state, token, OperationKind::Uninstall)?;
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
        check_with_manager_fallback, download_then_install_manager_update,
        install_root_from_picked_dir, manager_update_matches_confirmation,
        manager_update_reqwest_error, manager_update_root_url, manager_update_versioned_url,
        normalize_windows_source_base, reserve_manager_relaunch, sha256_hex,
        validate_install_root_path,
        validate_manager_update_identity_claim, validated_custom_proxy_for_settings,
        ManagerReleaseChannel, ManagerReleaseIdentity, ManagerReleaseIdentityPlatform,
    };
    use crate::app::op_phase::OperationPhase;
    use crate::app::oplock::{OperationKind, OperationManager};
    use crate::errors::AppError;
    use std::collections::HashMap;
    use std::fs;
    use std::sync::{Arc, Mutex};

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
    fn manager_relaunch_reservation_rejects_a_live_codex_mutation() {
        let path = temp_path("manager-relaunch-busy.lock");
        let _ = fs::remove_file(&path);
        let operations = OperationManager::new(path.clone());
        let active = operations.begin(OperationKind::Update).unwrap();
        let error = match reserve_manager_relaunch(&operations) {
            Ok(_) => panic!("manager relaunch unexpectedly bypassed the active operation"),
            Err(error) => error,
        };

        assert_eq!(error.code, "operation_busy");
        assert_eq!(operations.active_kind(), Some(OperationKind::Update));
        drop(active);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn manager_relaunch_reservation_holds_the_shared_lock_until_exit() {
        let path = temp_path("manager-relaunch-reserved.lock");
        let _ = fs::remove_file(&path);
        let operations = OperationManager::new(path.clone());
        let reservation = reserve_manager_relaunch(&operations).unwrap();

        assert_eq!(operations.active_kind(), Some(OperationKind::ManagerUpdate));
        assert_eq!(operations.phase(), OperationPhase::Committing);
        assert!(operations.begin(OperationKind::Install).is_err());
        assert!(operations.begin(OperationKind::Update).is_err());
        drop(reservation);
        let _ = fs::remove_file(path);
    }

    fn manager_update_test_endpoints() -> Vec<url::Url> {
        vec![
            url::Url::parse("https://mirror.example/latest.json").unwrap(),
            url::Url::parse("https://github.com/example/releases/latest/download/latest.json")
                .unwrap(),
        ]
    }

    #[test]
    fn manager_identity_fetch_error_does_not_expose_associated_url() {
        use reqwest::ResponseBuilderExt;

        let secret_path = "/private/identity";
        let secret_query = "X-Amz-Credential=private-credential&X-Amz-Signature=private-signature";
        let url = url::Url::parse(&format!(
            "https://secret-host.example{secret_path}?{secret_query}"
        ))
        .unwrap();
        let response = tauri::http::Response::builder()
            .status(tauri::http::StatusCode::FORBIDDEN)
            .url(url)
            .body("")
            .unwrap();
        let reqwest_error = reqwest::Response::from(response)
            .error_for_status()
            .unwrap_err();
        let error =
            manager_update_reqwest_error("fetch", "manager release identity", reqwest_error);
        let message = error.to_string();

        assert!(message.contains("status"), "missing category: {message}");
        assert!(
            message.contains("403 Forbidden"),
            "missing status: {message}"
        );

        for sensitive in [
            "https://".to_string(),
            "secret-host.example".to_string(),
            secret_path.to_string(),
            "X-Amz-Credential".to_string(),
            "private-credential".to_string(),
            "X-Amz-Signature".to_string(),
            "private-signature".to_string(),
        ] {
            assert!(
                !message.contains(&sensitive),
                "network error exposed {sensitive:?}: {message}"
            );
        }
    }

    fn manager_release_identity(
        version: &str,
        notes: &str,
        channel: ManagerReleaseChannel,
    ) -> ManagerReleaseIdentity {
        ManagerReleaseIdentity {
            schema: 1,
            version: version.to_string(),
            channel,
            notes_sha256: sha256_hex(notes.as_bytes()),
            platforms: HashMap::from([(
                "windows-x86_64".to_string(),
                ManagerReleaseIdentityPlatform {
                    artifact: "CodexAppManager_0.3.1_x64-setup.exe".to_string(),
                    sha256: "a".repeat(64),
                },
            )]),
        }
    }

    #[test]
    fn manager_update_rejects_forged_v999_with_replayed_old_signature() {
        let identity =
            manager_release_identity("0.3.1", "reviewed notes", ManagerReleaseChannel::Stable);
        let error = validate_manager_update_identity_claim(
            "999.0.0",
            "stable",
            Some("reviewed notes"),
            "windows-x86_64",
            "CodexAppManager_0.3.1_x64-setup.exe",
            &"a".repeat(64),
            &identity,
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("does not match the signed release identity"));
    }

    #[test]
    fn manager_update_accepts_mirror_claim_bound_to_signed_identity() {
        let identity =
            manager_release_identity("0.3.1", "reviewed notes", ManagerReleaseChannel::Stable);
        let digest = validate_manager_update_identity_claim(
            "0.3.1",
            "stable",
            Some("reviewed notes"),
            "windows-x86_64",
            "CodexAppManager_0.3.1_x64-setup.exe",
            &"a".repeat(64),
            &identity,
        )
        .unwrap();

        assert_eq!(digest, "a".repeat(64));
    }

    #[test]
    fn manager_update_rejects_unsigned_manifest_digest_override() {
        let identity =
            manager_release_identity("0.3.1", "reviewed notes", ManagerReleaseChannel::Stable);
        let error = validate_manager_update_identity_claim(
            "0.3.1",
            "stable",
            Some("reviewed notes"),
            "windows-x86_64",
            "CodexAppManager_0.3.1_x64-setup.exe",
            &"b".repeat(64),
            &identity,
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("artifact does not match the signed release identity"));
    }

    #[test]
    fn manager_update_rejects_self_consistent_prerelease_replay_on_stable_channel() {
        let identity = manager_release_identity(
            "0.4.0-rc.1",
            "prerelease notes",
            ManagerReleaseChannel::Prerelease,
        );
        let error = validate_manager_update_identity_claim(
            "0.4.0-rc.1",
            "prerelease",
            Some("prerelease notes"),
            "windows-x86_64",
            "CodexAppManager_0.3.1_x64-setup.exe",
            &"a".repeat(64),
            &identity,
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("stable manager updater rejects prerelease"));
    }

    #[test]
    fn manager_update_derives_versioned_identity_and_artifact_urls_per_source() {
        let mirror =
            url::Url::parse("https://codexapp.agentsmirror.com/manager/latest.json?ignored=1")
                .unwrap();
        assert_eq!(
            manager_update_root_url(&mirror, "release-identity.json")
                .unwrap()
                .as_str(),
            "https://codexapp.agentsmirror.com/manager/release-identity.json"
        );

        let github = url::Url::parse(
            "https://github.com/Wangnov/Codex-App-Manager/releases/latest/download/latest.json",
        )
        .unwrap();
        assert_eq!(
            manager_update_root_url(&github, "release-identity.json")
                .unwrap()
                .as_str(),
            "https://github.com/Wangnov/Codex-App-Manager/releases/latest/download/release-identity.json"
        );
        assert_eq!(
            manager_update_versioned_url(
                &github,
                "0.3.2",
                "CodexAppManager_0.3.2_x64-setup.exe"
            )
            .unwrap()
            .as_str(),
            "https://github.com/Wangnov/Codex-App-Manager/releases/download/v0.3.2/CodexAppManager_0.3.2_x64-setup.exe"
        );
        assert!(manager_update_versioned_url(&mirror, "0.3.2", "../escape").is_err());
    }

    #[test]
    fn unsigned_manifest_version_cannot_select_the_root_identity() {
        let mirror =
            url::Url::parse("https://codexapp.agentsmirror.com/manager/latest.json").unwrap();
        let root = manager_update_root_url(&mirror, "release-identity.json").unwrap();

        assert_eq!(
            root.as_str(),
            "https://codexapp.agentsmirror.com/manager/release-identity.json"
        );
        assert!(!root.path().contains("999.0.0"));
    }

    #[test]
    fn manager_update_cn_path_accepts_authenticated_mirror_first() {
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let attempts_for_check = Arc::clone(&attempts);
        let identity = Arc::new(manager_release_identity(
            "0.3.1",
            "reviewed notes",
            ManagerReleaseChannel::Stable,
        ));
        let identity_for_check = Arc::clone(&identity);
        let result = tauri::async_runtime::block_on(check_with_manager_fallback(
            manager_update_test_endpoints(),
            move |endpoint| {
                let attempts = Arc::clone(&attempts_for_check);
                let identity = Arc::clone(&identity_for_check);
                async move {
                    attempts
                        .lock()
                        .unwrap()
                        .push(endpoint.host_str().unwrap().to_string());
                    validate_manager_update_identity_claim(
                        "0.3.1",
                        "stable",
                        Some("reviewed notes"),
                        "windows-x86_64",
                        "CodexAppManager_0.3.1_x64-setup.exe",
                        &"a".repeat(64),
                        &identity,
                    )?;
                    Ok(Some("authenticated-mirror-update"))
                }
            },
        ));

        assert_eq!(result.unwrap(), Some("authenticated-mirror-update"));
        assert_eq!(*attempts.lock().unwrap(), vec!["mirror.example"]);
    }

    #[test]
    fn manager_update_check_falls_back_after_primary_reports_no_update() {
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let attempts_for_check = Arc::clone(&attempts);

        let result = tauri::async_runtime::block_on(check_with_manager_fallback(
            manager_update_test_endpoints(),
            move |endpoint| {
                let attempts = Arc::clone(&attempts_for_check);
                async move {
                    attempts
                        .lock()
                        .unwrap()
                        .push(endpoint.host_str().unwrap().to_string());
                    if endpoint.host_str() == Some("mirror.example") {
                        Ok(None)
                    } else {
                        Ok(Some("github-update"))
                    }
                }
            },
        ));

        assert_eq!(result.unwrap(), Some("github-update"));
        assert_eq!(
            *attempts.lock().unwrap(),
            vec!["mirror.example", "github.com"]
        );
    }

    #[test]
    fn manager_update_check_falls_back_after_primary_check_failure() {
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let attempts_for_check = Arc::clone(&attempts);

        let result = tauri::async_runtime::block_on(check_with_manager_fallback(
            manager_update_test_endpoints(),
            move |endpoint| {
                let attempts = Arc::clone(&attempts_for_check);
                async move {
                    attempts
                        .lock()
                        .unwrap()
                        .push(endpoint.host_str().unwrap().to_string());
                    if endpoint.host_str() == Some("mirror.example") {
                        Err(AppError::Engine(
                            "manifest has no current platform".to_string(),
                        ))
                    } else {
                        Ok(Some("github-update"))
                    }
                }
            },
        ));

        assert_eq!(result.unwrap(), Some("github-update"));
        assert_eq!(
            *attempts.lock().unwrap(),
            vec!["mirror.example", "github.com"]
        );
    }

    #[test]
    fn manager_update_check_accepts_final_no_update_after_primary_failure() {
        let result = tauri::async_runtime::block_on(check_with_manager_fallback(
            manager_update_test_endpoints(),
            |endpoint| async move {
                if endpoint.host_str() == Some("mirror.example") {
                    Err(AppError::Engine("timed out".to_string()))
                } else {
                    Ok(None::<&'static str>)
                }
            },
        ));

        assert_eq!(result.unwrap(), None);
    }

    #[test]
    fn manager_update_check_rejects_unconfirmed_no_update() {
        let error = tauri::async_runtime::block_on(check_with_manager_fallback(
            manager_update_test_endpoints(),
            |endpoint| async move {
                if endpoint.host_str() == Some("mirror.example") {
                    Ok(None::<&'static str>)
                } else {
                    Err(AppError::Engine("timed out".to_string()))
                }
            },
        ))
        .unwrap_err();

        assert!(error.to_string().contains("github.com"));
        assert!(error.to_string().contains("timed out"));
    }

    #[test]
    fn manager_update_falls_back_after_mirror_artifact_failure() {
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let installed = Arc::new(Mutex::new(Vec::new()));
        let attempts_for_download = Arc::clone(&attempts);
        let installed_for_install = Arc::clone(&installed);

        let result = tauri::async_runtime::block_on(download_then_install_manager_update(
            manager_update_test_endpoints(),
            move |endpoint| {
                let attempts = Arc::clone(&attempts_for_download);
                async move {
                    attempts
                        .lock()
                        .unwrap()
                        .push(endpoint.host_str().unwrap().to_string());
                    if endpoint.host_str() == Some("mirror.example") {
                        Err(AppError::Engine(
                            "download manager update: HTTP 404".to_string(),
                        ))
                    } else {
                        Ok("github-verified-package")
                    }
                }
            },
            move |package| {
                installed_for_install.lock().unwrap().push(package);
                Ok(())
            },
        ));

        assert!(result.is_ok());
        assert_eq!(
            *attempts.lock().unwrap(),
            vec!["mirror.example", "github.com"]
        );
        assert_eq!(*installed.lock().unwrap(), vec!["github-verified-package"]);
    }

    #[test]
    fn manager_update_falls_back_when_mirror_serves_old_bytes_for_current_signature() {
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let attempts_for_download = Arc::clone(&attempts);
        let result = tauri::async_runtime::block_on(download_then_install_manager_update(
            manager_update_test_endpoints(),
            move |endpoint| {
                let attempts = Arc::clone(&attempts_for_download);
                async move {
                    attempts
                        .lock()
                        .unwrap()
                        .push(endpoint.host_str().unwrap().to_string());
                    if endpoint.host_str() == Some("mirror.example") {
                        Err(AppError::Engine(
                            "download manager update: minisign verification failed for old bytes"
                                .to_string(),
                        ))
                    } else {
                        Ok("github-current-signed-bytes")
                    }
                }
            },
            |_| Ok(()),
        ));

        assert!(result.is_ok());
        assert_eq!(
            *attempts.lock().unwrap(),
            vec!["mirror.example", "github.com"]
        );
    }

    #[test]
    fn manager_update_falls_back_after_mirror_stream_resets() {
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let attempts_for_download = Arc::clone(&attempts);

        let result = tauri::async_runtime::block_on(download_then_install_manager_update(
            manager_update_test_endpoints(),
            move |endpoint| {
                let attempts = Arc::clone(&attempts_for_download);
                async move {
                    attempts
                        .lock()
                        .unwrap()
                        .push(endpoint.host_str().unwrap().to_string());
                    if endpoint.host_str() == Some("mirror.example") {
                        Err(AppError::Engine(
                            "download manager update: connection reset mid-stream".to_string(),
                        ))
                    } else {
                        Ok("github-verified-package")
                    }
                }
            },
            |_| Ok(()),
        ));

        assert!(result.is_ok());
        assert_eq!(
            *attempts.lock().unwrap(),
            vec!["mirror.example", "github.com"]
        );
    }

    #[test]
    fn manager_update_falls_back_when_primary_manifest_is_stale() {
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let attempts_for_download = Arc::clone(&attempts);

        let result = tauri::async_runtime::block_on(download_then_install_manager_update(
            manager_update_test_endpoints(),
            move |endpoint| {
                let attempts = Arc::clone(&attempts_for_download);
                async move {
                    attempts
                        .lock()
                        .unwrap()
                        .push(endpoint.host_str().unwrap().to_string());
                    if endpoint.host_str() == Some("mirror.example") {
                        Err(AppError::StaleExpectation("mirror is stale".to_string()))
                    } else {
                        Ok("github-exact-expected-package")
                    }
                }
            },
            |_| Ok(()),
        ));

        assert!(result.is_ok());
        assert_eq!(
            *attempts.lock().unwrap(),
            vec!["mirror.example", "github.com"]
        );
    }

    #[test]
    fn manager_update_reports_when_both_artifact_sources_fail() {
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let install_called = Arc::new(Mutex::new(false));
        let attempts_for_download = Arc::clone(&attempts);
        let install_called_for_install = Arc::clone(&install_called);

        let error = tauri::async_runtime::block_on(download_then_install_manager_update::<
            &'static str,
            _,
            _,
            _,
        >(
            manager_update_test_endpoints(),
            move |endpoint| {
                let attempts = Arc::clone(&attempts_for_download);
                async move {
                    attempts
                        .lock()
                        .unwrap()
                        .push(endpoint.host_str().unwrap().to_string());
                    Err(AppError::Engine(format!(
                        "download failed from {}",
                        endpoint.host_str().unwrap()
                    )))
                }
            },
            move |_| {
                *install_called_for_install.lock().unwrap() = true;
                Ok(())
            },
        ))
        .unwrap_err();

        assert_eq!(
            *attempts.lock().unwrap(),
            vec!["mirror.example", "github.com"]
        );
        assert!(!*install_called.lock().unwrap());
        assert!(error.to_string().contains("mirror.example"));
        assert!(error.to_string().contains("github.com"));
    }

    #[test]
    fn manager_update_never_falls_back_after_install_starts() {
        let attempts = Arc::new(Mutex::new(Vec::new()));
        let install_calls = Arc::new(Mutex::new(0_u32));
        let attempts_for_download = Arc::clone(&attempts);
        let install_calls_for_install = Arc::clone(&install_calls);

        let error = tauri::async_runtime::block_on(download_then_install_manager_update(
            manager_update_test_endpoints(),
            move |endpoint| {
                let attempts = Arc::clone(&attempts_for_download);
                async move {
                    attempts
                        .lock()
                        .unwrap()
                        .push(endpoint.host_str().unwrap().to_string());
                    Ok("verified-package")
                }
            },
            move |_| {
                *install_calls_for_install.lock().unwrap() += 1;
                Err(AppError::Engine(
                    "installer failed after launch".to_string(),
                ))
            },
        ))
        .unwrap_err();

        assert_eq!(*attempts.lock().unwrap(), vec!["mirror.example"]);
        assert_eq!(*install_calls.lock().unwrap(), 1);
        assert!(error.to_string().contains("installer failed after launch"));
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
