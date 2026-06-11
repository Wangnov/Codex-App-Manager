use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use codex_app_manager_lib::adapters::host;
use codex_app_manager_lib::app::provenance::ProvenanceStore;
use codex_app_manager_lib::app::win_update::{
    perform_windows_update, plan_windows_update, uninstall_windows_codex, win_adopt,
    win_install_status, WinInstallStatus, WinPerformAction,
};
use codex_app_manager_lib::domain::manifest::MirrorEndpoints;
use codex_app_manager_lib::domain::settings::AppSettings;
use codex_app_manager_lib::domain::target::Target;
use codex_win_engine::{
    download_to, fetch_text, find_msix_sha256, install_msix_sideload, install_portable_from_msix,
    parse_manifest, purge_codex_user_data, read_msix_identity, remove_msix_package, sha256_file,
    validate_codex_identity, verify_openai_authenticode, version_key, WindowsRelease,
};
use serde::Serialize;

const OLD_MSIX_VERSION: &str = "26.601.2237.0";
const OLD_MSIX_MONIKER: &str = "OpenAI.Codex_26.601.2237.0_x64__2p2nqsd0c76g0";
const OLD_MSIX_URL: &str = "https://github.com/Wangnov/codex-app-mirror/releases/download/codex-app-win-26.601.2237.0-mac-26.601.21317-b3511/OpenAI.Codex_26.601.2237.0_x64__2p2nqsd0c76g0.Msix";
const OLD_MSIX_SHA256: &str = "432d5e75ee973bf8172db58435a86823ccd51272ea0a82395c70a6d485015caa";
const OLD_MSIX_SIZE: u64 = 564_451_929;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StepReport {
    step: String,
    ok: bool,
    detail: serde_json::Value,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct PrefetchReport {
    release: WindowsRelease,
    staged_path: String,
    downloaded: bool,
    size: u64,
    sha256: String,
    authenticode_status: String,
    authenticode_subject: String,
    identity_name: String,
    identity_version: String,
    identity_architecture: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SmokeReport {
    install_root: String,
    user_data_path: Option<String>,
    user_data_existed_before: bool,
    user_data_exists_after: bool,
    steps: Vec<StepReport>,
    final_status: WinInstallStatus,
}

fn settings_and_endpoints() -> (AppSettings, MirrorEndpoints) {
    let target = Target::current();
    let mirror_base_url = "https://codexapp.agentsmirror.com".to_string();
    let install_root = host::default_install_root(&target);
    let settings = AppSettings::new(mirror_base_url.clone(), install_root);
    let endpoints = MirrorEndpoints::from_base_url(&mirror_base_url);
    (settings, endpoints)
}

fn user_data_path() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
        .map(|home| home.join(".codex"))
}

fn staged_msix_path(release: &WindowsRelease) -> PathBuf {
    std::env::temp_dir()
        .join("codex-app-manager")
        .join("windows-staging")
        .join(format!("{}.msix", release.package_moniker))
}

fn old_release() -> WindowsRelease {
    WindowsRelease {
        version: OLD_MSIX_VERSION.to_string(),
        package_moniker: OLD_MSIX_MONIKER.to_string(),
        architecture: Some("x64".to_string()),
        content_length: Some(OLD_MSIX_SIZE),
        etag: None,
        store_product_id: Some("9PLM9XGG6VKS".to_string()),
        package_identity: Some("OpenAI.Codex".to_string()),
    }
}

fn prefetch_verified_release_msix(
    release: WindowsRelease,
    url: &str,
    expected_sha: &str,
) -> Result<PrefetchReport, String> {
    let dest = staged_msix_path(&release);
    let cached_ok = dest.exists()
        && sha256_file(&dest)
            .map(|actual| actual.eq_ignore_ascii_case(expected_sha))
            .unwrap_or(false);
    let mut downloaded = false;
    if !cached_ok {
        if dest.exists() {
            std::fs::remove_file(&dest)
                .map_err(|e| format!("remove stale staged MSIX {}: {e}", dest.display()))?;
        }
        download_to(url, &dest).map_err(|e| e.to_string())?;
        downloaded = true;
    }

    let size = std::fs::metadata(&dest)
        .map_err(|e| format!("read staged MSIX metadata {}: {e}", dest.display()))?
        .len();
    if let Some(expected_size) = release.content_length {
        if size != expected_size {
            return Err(format!(
                "MSIX size mismatch: actual {size}, expected {expected_size}"
            ));
        }
    }

    let actual_sha = sha256_file(&dest).map_err(|e| e.to_string())?;
    if !actual_sha.eq_ignore_ascii_case(expected_sha) {
        return Err(format!(
            "MSIX sha256 mismatch: actual {actual_sha}, expected {expected_sha}"
        ));
    }

    let authenticode = verify_openai_authenticode(Path::new(&dest)).map_err(|e| e.to_string())?;
    if !authenticode.is_valid_openai() {
        return Err(format!(
            "MSIX Authenticode verification failed: status={}, subject={}",
            authenticode.status, authenticode.subject
        ));
    }

    let identity = read_msix_identity(&dest).map_err(|e| e.to_string())?;
    validate_codex_identity(&identity, &release.version, release.architecture.as_deref())
        .map_err(|e| e.to_string())?;

    Ok(PrefetchReport {
        release,
        staged_path: dest.to_string_lossy().into_owned(),
        downloaded,
        size,
        sha256: actual_sha,
        authenticode_status: authenticode.status,
        authenticode_subject: authenticode.subject,
        identity_name: identity.name,
        identity_version: identity.version,
        identity_architecture: identity.processor_architecture,
    })
}

fn prefetch_verified_old_msix() -> Result<PrefetchReport, String> {
    prefetch_verified_release_msix(old_release(), OLD_MSIX_URL, OLD_MSIX_SHA256)
}

fn prefetch_verified_msix(endpoints: &MirrorEndpoints) -> Result<PrefetchReport, String> {
    let manifest_text = fetch_text(&endpoints.manifest_url).map_err(|e| e.to_string())?;
    let checksums_text = fetch_text(&endpoints.checksums_url).map_err(|e| e.to_string())?;
    let release = parse_manifest(&manifest_text).map_err(|e| e.to_string())?;
    let expected_sha =
        find_msix_sha256(&checksums_text, &release.package_moniker).map_err(|e| e.to_string())?;
    let dest = staged_msix_path(&release);

    let cached_ok = dest.exists()
        && sha256_file(&dest)
            .map(|actual| actual.eq_ignore_ascii_case(&expected_sha))
            .unwrap_or(false);
    let mut downloaded = false;
    if !cached_ok {
        if dest.exists() {
            std::fs::remove_file(&dest)
                .map_err(|e| format!("remove stale staged MSIX {}: {e}", dest.display()))?;
        }
        download_to(&endpoints.windows_msix_url, &dest).map_err(|e| e.to_string())?;
        downloaded = true;
    }

    let size = std::fs::metadata(&dest)
        .map_err(|e| format!("read staged MSIX metadata {}: {e}", dest.display()))?
        .len();
    if let Some(expected_size) = release.content_length {
        if size != expected_size {
            return Err(format!(
                "MSIX size mismatch: actual {size}, expected {expected_size}"
            ));
        }
    }

    let actual_sha = sha256_file(&dest).map_err(|e| e.to_string())?;
    if !actual_sha.eq_ignore_ascii_case(&expected_sha) {
        return Err(format!(
            "MSIX sha256 mismatch: actual {actual_sha}, expected {expected_sha}"
        ));
    }

    let authenticode = verify_openai_authenticode(Path::new(&dest)).map_err(|e| e.to_string())?;
    if !authenticode.is_valid_openai() {
        return Err(format!(
            "MSIX Authenticode verification failed: status={}, subject={}",
            authenticode.status, authenticode.subject
        ));
    }

    let identity = read_msix_identity(&dest).map_err(|e| e.to_string())?;
    validate_codex_identity(&identity, &release.version, release.architecture.as_deref())
        .map_err(|e| e.to_string())?;

    Ok(PrefetchReport {
        release,
        staged_path: dest.to_string_lossy().into_owned(),
        downloaded,
        size,
        sha256: actual_sha,
        authenticode_status: authenticode.status,
        authenticode_subject: authenticode.subject,
        identity_name: identity.name,
        identity_version: identity.version,
        identity_architecture: identity.processor_architecture,
    })
}

fn step<T: Serialize>(steps: &mut Vec<StepReport>, name: &str, detail: T) -> Result<(), String> {
    let detail = serde_json::to_value(detail).map_err(|e| format!("serialize {name}: {e}"))?;
    steps.push(StepReport {
        step: name.to_string(),
        ok: true,
        detail,
    });
    Ok(())
}

fn record_managed(path: String, version: &str, source: &str) -> Result<(), String> {
    let mut store = ProvenanceStore::load();
    store.record(path, version_key(version), source);
    store.save().map_err(|e| e.to_string())
}

fn remove_managed(path: &str) -> Result<(), String> {
    let mut store = ProvenanceStore::load();
    store.remove(path);
    store.save().map_err(|e| e.to_string())
}

fn remove_rollback(settings: &AppSettings) {
    let install_root = PathBuf::from(&settings.install_root);
    if let Some(parent) = install_root.parent() {
        let _ = std::fs::remove_dir_all(parent.join("Codex.rollback"));
    }
}

fn run_powershell_script(script: &str) -> Result<String, String> {
    let output = Command::new("powershell.exe")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-ExecutionPolicy",
            "Bypass",
            "-Command",
            script,
        ])
        .output()
        .map_err(|e| format!("spawn powershell.exe: {e}"))?;
    if !output.status.success() {
        return Err(format!(
            "powershell.exe exited with {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn portable_uninstall_entry_exists() -> Result<bool, String> {
    let output = run_powershell_script(
        "if (Test-Path 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Codex') { 'true' } else { 'false' }",
    )?;
    Ok(output.eq_ignore_ascii_case("true"))
}

fn portable_shortcut_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .map(|appdata| {
            appdata
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs")
                .join("Codex.lnk")
        })
}

fn cleanup_portable_shell_entry() {
    let _ = run_powershell_script(
        r#"
Remove-Item -LiteralPath 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Codex' -Recurse -Force -ErrorAction SilentlyContinue
if ($env:APPDATA) {
  $Shortcut = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Codex.lnk'
  Remove-Item -LiteralPath $Shortcut -Force -ErrorAction SilentlyContinue
}
"#,
    );
}

fn set_user_profile_env(path: &Path) -> (Option<std::ffi::OsString>, Option<std::ffi::OsString>) {
    let old_userprofile = std::env::var_os("USERPROFILE");
    let old_home = std::env::var_os("HOME");
    std::env::set_var("USERPROFILE", path);
    std::env::remove_var("HOME");
    (old_userprofile, old_home)
}

fn restore_user_profile_env(
    old_userprofile: Option<std::ffi::OsString>,
    old_home: Option<std::ffi::OsString>,
) {
    if let Some(value) = old_userprofile {
        std::env::set_var("USERPROFILE", value);
    } else {
        std::env::remove_var("USERPROFILE");
    }
    if let Some(value) = old_home {
        std::env::set_var("HOME", value);
    } else {
        std::env::remove_var("HOME");
    }
}

fn ensure_no_install(settings: &AppSettings, steps: &mut Vec<StepReport>) -> Result<(), String> {
    let status = win_install_status(settings);
    if status.status == "external" {
        let adopted = win_adopt(settings).map_err(|e| format!("adopt external install: {e}"))?;
        step(steps, "adopt-existing-external", &adopted)?;
    }
    if status.status != "none" {
        let uninstalled =
            uninstall_windows_codex(settings, true, false).map_err(|e| format!("{e}"))?;
        step(steps, "uninstall-existing-preserve-user-data", &uninstalled)?;
        remove_rollback(settings);
        let after_uninstall = win_install_status(settings);
        step(steps, "status-after-existing-uninstall", &after_uninstall)?;
        if after_uninstall.status != "none" {
            return Err(format!(
                "expected no install after uninstall, got {}",
                after_uninstall.status
            ));
        }
    }
    Ok(())
}

fn reinstall_final_msix(
    endpoints: &MirrorEndpoints,
    settings: &AppSettings,
    steps: &mut Vec<StepReport>,
) -> Result<WinInstallStatus, String> {
    let reinstalled = perform_windows_update(endpoints, settings, true)
        .map_err(|e| format!("reinstall final MSIX: {e}"))?;
    step(steps, "reinstall-final-msix", &reinstalled)?;
    let final_status = win_install_status(settings);
    step(steps, "final-status", &final_status)?;
    if final_status.status != "managed"
        || final_status.installed.as_ref().map(|i| i.source.as_str()) != Some("msix")
    {
        return Err(format!(
            "expected final managed MSIX install, got status={} source={}",
            final_status.status,
            final_status
                .installed
                .as_ref()
                .map(|i| i.source.as_str())
                .unwrap_or("none")
        ));
    }
    Ok(final_status)
}

fn run_install_uninstall_install() -> Result<SmokeReport, String> {
    let (settings, endpoints) = settings_and_endpoints();
    let user_data = user_data_path();
    let user_data_existed_before = user_data.as_ref().is_some_and(|path| path.exists());
    let mut steps = Vec::new();

    let initial = win_install_status(&settings);
    step(&mut steps, "initial-status", &initial)?;

    let prefetch = prefetch_verified_msix(&endpoints)?;
    step(&mut steps, "prefetch-verified-msix", &prefetch)?;

    if initial.status == "external" {
        let adopted = win_adopt(&settings).map_err(|e| format!("adopt external install: {e}"))?;
        step(&mut steps, "adopt-existing-external", &adopted)?;
    }

    if initial.status != "none" {
        let uninstalled =
            uninstall_windows_codex(&settings, true, false).map_err(|e| format!("{e}"))?;
        step(
            &mut steps,
            "uninstall-existing-preserve-user-data",
            &uninstalled,
        )?;
        let after_uninstall = win_install_status(&settings);
        step(
            &mut steps,
            "status-after-existing-uninstall",
            &after_uninstall,
        )?;
        if after_uninstall.status != "none" {
            return Err(format!(
                "expected no install after uninstall, got {}",
                after_uninstall.status
            ));
        }
    }

    let installed = perform_windows_update(&endpoints, &settings, true)
        .map_err(|e| format!("install after uninstall: {e}"))?;
    step(&mut steps, "install", &installed)?;
    let after_install = win_install_status(&settings);
    step(&mut steps, "status-after-install", &after_install)?;
    if after_install.status != "managed" {
        return Err(format!(
            "expected managed install after install, got {}",
            after_install.status
        ));
    }

    let uninstalled =
        uninstall_windows_codex(&settings, true, false).map_err(|e| format!("{e}"))?;
    step(
        &mut steps,
        "uninstall-installed-preserve-user-data",
        &uninstalled,
    )?;
    let after_second_uninstall = win_install_status(&settings);
    step(
        &mut steps,
        "status-after-installed-uninstall",
        &after_second_uninstall,
    )?;
    if after_second_uninstall.status != "none" {
        return Err(format!(
            "expected no install after second uninstall, got {}",
            after_second_uninstall.status
        ));
    }

    let reinstalled = perform_windows_update(&endpoints, &settings, true)
        .map_err(|e| format!("reinstall after uninstall: {e}"))?;
    step(&mut steps, "reinstall-final", &reinstalled)?;
    let final_status = win_install_status(&settings);
    step(&mut steps, "final-status", &final_status)?;
    if final_status.status != "managed" {
        return Err(format!(
            "expected final managed install, got {}",
            final_status.status
        ));
    }

    Ok(SmokeReport {
        install_root: settings.install_root,
        user_data_path: user_data
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        user_data_existed_before,
        user_data_exists_after: user_data.as_ref().is_some_and(|path| path.exists()),
        steps,
        final_status,
    })
}

fn run_msix_remove_with_temp_purge_fallback(
    settings: AppSettings,
    endpoints: MirrorEndpoints,
    real_user_data: PathBuf,
    user_data_existed_before: bool,
    installed_path: String,
    mut steps: Vec<StepReport>,
    backup_error: &std::io::Error,
) -> Result<SmokeReport, String> {
    step(
        &mut steps,
        "real-user-data-backup-unavailable",
        serde_json::json!({
            "realUserData": real_user_data.to_string_lossy(),
            "error": backup_error.to_string(),
            "fallback": "remove-msix-with-temp-user-data-purge",
        }),
    )?;

    let temp_profile =
        std::env::temp_dir().join(format!("codex-smoke-userprofile-{}", std::process::id()));
    let temp_codex = temp_profile.join(".codex");
    let _ = std::fs::remove_dir_all(&temp_profile);
    std::fs::create_dir_all(&temp_codex)
        .map_err(|e| format!("create temp .codex for purge fallback: {e}"))?;
    std::fs::write(temp_codex.join("sentinel.txt"), b"delete-me")
        .map_err(|e| format!("write temp .codex sentinel: {e}"))?;

    let msix = remove_msix_package().map_err(|e| e.to_string())?;
    step(&mut steps, "remove-msix-package", &msix)?;
    let mut notes = Vec::new();
    let purged_user_data = if msix.success {
        remove_managed(&installed_path)?;
        let (old_userprofile, old_home) = set_user_profile_env(&temp_profile);
        let purged = purge_codex_user_data(&mut notes).map_err(|e| e.to_string());
        restore_user_profile_env(old_userprofile, old_home);
        purged?
    } else {
        false
    };
    step(
        &mut steps,
        "purge-temp-user-data-after-msix-remove",
        serde_json::json!({
            "purgedUserData": purged_user_data,
            "tempCodexExists": temp_codex.exists(),
            "notes": notes,
        }),
    )?;
    if !msix.success || !purged_user_data || temp_codex.exists() {
        let _ = reinstall_final_msix(&endpoints, &settings, &mut steps);
        let _ = std::fs::remove_dir_all(&temp_profile);
        return Err(format!(
            "expected split MSIX remove + temp purge fallback to succeed; msix={}",
            serde_json::to_string(&msix).unwrap_or_else(|_| "<unserializable>".to_string())
        ));
    }

    let after_uninstall = win_install_status(&settings);
    step(&mut steps, "status-after-msix-remove", &after_uninstall)?;
    if after_uninstall.status != "none" {
        let _ = reinstall_final_msix(&endpoints, &settings, &mut steps);
        let _ = std::fs::remove_dir_all(&temp_profile);
        return Err(format!(
            "expected no install after split MSIX remove, got {}",
            after_uninstall.status
        ));
    }

    let final_status = reinstall_final_msix(&endpoints, &settings, &mut steps)?;
    let _ = std::fs::remove_dir_all(&temp_profile);

    Ok(SmokeReport {
        install_root: settings.install_root,
        user_data_path: Some(real_user_data.to_string_lossy().into_owned()),
        user_data_existed_before,
        user_data_exists_after: real_user_data.exists(),
        steps,
        final_status,
    })
}

fn run_msix_uninstall_purge_user_data_restore() -> Result<SmokeReport, String> {
    let (settings, endpoints) = settings_and_endpoints();
    let real_user_data = user_data_path()
        .ok_or_else(|| "USERPROFILE/HOME is required for purge smoke".to_string())?;
    let user_data_existed_before = real_user_data.exists();
    let backup = std::env::temp_dir().join(format!(
        "codex-smoke-real-codex-backup-{}",
        std::process::id()
    ));
    let mut steps = Vec::new();

    let initial = win_install_status(&settings);
    step(&mut steps, "initial-status", &initial)?;
    let prefetch = prefetch_verified_msix(&endpoints)?;
    step(&mut steps, "prefetch-verified-msix", &prefetch)?;

    let installed_source = initial.installed.as_ref().map(|i| i.source.as_str());
    if initial.status != "managed" || installed_source != Some("msix") {
        return Err(format!(
            "expected a managed MSIX before purge smoke, got status={} source={}",
            initial.status,
            installed_source.unwrap_or("none")
        ));
    }
    let installed_path = initial
        .installed
        .as_ref()
        .map(|installed| installed.path.clone())
        .unwrap_or_default();

    if backup.exists() {
        std::fs::remove_dir_all(&backup)
            .map_err(|e| format!("remove stale .codex backup {}: {e}", backup.display()))?;
    }
    if user_data_existed_before {
        if let Err(err) = std::fs::rename(&real_user_data, &backup) {
            return run_msix_remove_with_temp_purge_fallback(
                settings,
                endpoints,
                real_user_data,
                user_data_existed_before,
                installed_path,
                steps,
                &err,
            );
        }
    }

    let result = (|| {
        std::fs::create_dir_all(&real_user_data)
            .map_err(|e| format!("create sentinel .codex for purge smoke: {e}"))?;
        std::fs::write(real_user_data.join("sentinel.txt"), b"delete-me")
            .map_err(|e| format!("write sentinel .codex file: {e}"))?;
        step(
            &mut steps,
            "swap-real-user-data-for-sentinel",
            serde_json::json!({
                "realUserData": real_user_data.to_string_lossy(),
                "backup": backup.to_string_lossy(),
                "hadRealUserData": user_data_existed_before,
            }),
        )?;

        let uninstalled =
            uninstall_windows_codex(&settings, true, true).map_err(|e| e.to_string())?;
        step(&mut steps, "uninstall-msix-purge-user-data", &uninstalled)?;
        if !uninstalled.success || !uninstalled.purged_user_data || real_user_data.exists() {
            return Err(format!(
                "expected MSIX uninstall to purge sentinel .codex; uninstall={}",
                serde_json::to_string(&uninstalled)
                    .unwrap_or_else(|_| "<unserializable>".to_string())
            ));
        }

        let after_uninstall = win_install_status(&settings);
        step(&mut steps, "status-after-purge-uninstall", &after_uninstall)?;
        if after_uninstall.status != "none" {
            return Err(format!(
                "expected no install after purge uninstall, got {}",
                after_uninstall.status
            ));
        }

        reinstall_final_msix(&endpoints, &settings, &mut steps)
    })();

    if result.is_err() {
        let _ = reinstall_final_msix(&endpoints, &settings, &mut steps);
    }
    if real_user_data.exists() {
        std::fs::remove_dir_all(&real_user_data).map_err(|e| {
            format!(
                "remove sentinel .codex before restoring backup {}: {e}",
                real_user_data.display()
            )
        })?;
    }
    if backup.exists() {
        std::fs::rename(&backup, &real_user_data).map_err(|e| {
            format!(
                "restore real .codex backup {} -> {}: {e}",
                backup.display(),
                real_user_data.display()
            )
        })?;
    }
    let final_status = result.map_err(|err| {
        format!(
            "{err}; steps={}",
            serde_json::to_string(&steps).unwrap_or_else(|_| "<unserializable>".to_string())
        )
    })?;

    Ok(SmokeReport {
        install_root: settings.install_root,
        user_data_path: Some(real_user_data.to_string_lossy().into_owned()),
        user_data_existed_before,
        user_data_exists_after: real_user_data.exists(),
        steps,
        final_status,
    })
}

fn run_portable_apps_features_uninstall_entry() -> Result<SmokeReport, String> {
    let (settings, endpoints) = settings_and_endpoints();
    let user_data = user_data_path();
    let user_data_existed_before = user_data.as_ref().is_some_and(|path| path.exists());
    let mut steps = Vec::new();
    let root = PathBuf::from(&settings.install_root);

    let result = (|| {
        let initial = win_install_status(&settings);
        step(&mut steps, "initial-status", &initial)?;
        let prefetch = prefetch_verified_msix(&endpoints)?;
        step(&mut steps, "prefetch-verified-msix", &prefetch)?;
        ensure_no_install(&settings, &mut steps)?;

        let portable_install = install_portable_from_msix(
            Path::new(&prefetch.staged_path),
            Path::new(&settings.install_root),
            false,
        )
        .map_err(|e| format!("install portable for Apps & Features uninstall smoke: {e}"))?;
        record_managed(
            settings.install_root.clone(),
            &portable_install.version,
            "smoke-apps-features-uninstall",
        )?;
        step(&mut steps, "install-portable", &portable_install)?;
        if !portable_install.success || !portable_install.uninstall_entry_created {
            return Err("expected portable install to create an uninstall entry".to_string());
        }

        let after_install = win_install_status(&settings);
        step(&mut steps, "status-after-portable-install", &after_install)?;
        if after_install.installed.as_ref().map(|i| i.source.as_str()) != Some("portable") {
            return Err("expected managed portable before registry uninstall".to_string());
        }

        let shortcut_before = portable_shortcut_path()
            .as_ref()
            .is_some_and(|path| path.exists());
        let uninstall_string = run_powershell_script(
            r#"
$Key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Codex'
(Get-ItemProperty -Path $Key -ErrorAction Stop).UninstallString
"#,
        )?;
        step(
            &mut steps,
            "read-apps-features-uninstall-string",
            serde_json::json!({
                "uninstallString": uninstall_string,
                "shortcutExistedBefore": shortcut_before,
            }),
        )?;

        let uninstall_output = run_powershell_script(
            r#"
$Key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Codex'
$Command = (Get-ItemProperty -Path $Key -ErrorAction Stop).UninstallString
if (-not $Command) { throw 'missing UninstallString' }
cmd.exe /D /C $Command
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
"#,
        )?;
        step(
            &mut steps,
            "execute-apps-features-uninstall-string",
            serde_json::json!({ "stdout": uninstall_output }),
        )?;

        remove_managed(&settings.install_root)?;
        let install_root_exists = root.exists();
        let uninstall_entry_exists = portable_uninstall_entry_exists()?;
        let shortcut_exists = portable_shortcut_path()
            .as_ref()
            .is_some_and(|path| path.exists());
        step(
            &mut steps,
            "verify-apps-features-uninstall-cleanup",
            serde_json::json!({
                "installRootExists": install_root_exists,
                "uninstallEntryExists": uninstall_entry_exists,
                "shortcutExists": shortcut_exists,
            }),
        )?;
        if install_root_exists || uninstall_entry_exists || shortcut_exists {
            return Err(
                "expected Apps & Features uninstall entry to remove files, registry entry, and shortcut"
                    .to_string(),
            );
        }

        let after_uninstall = win_install_status(&settings);
        step(
            &mut steps,
            "status-after-apps-features-uninstall",
            &after_uninstall,
        )?;
        if after_uninstall.status != "none" {
            return Err(format!(
                "expected no install after Apps & Features uninstall, got {}",
                after_uninstall.status
            ));
        }

        let final_status = reinstall_final_msix(&endpoints, &settings, &mut steps)?;
        Ok(SmokeReport {
            install_root: settings.install_root.clone(),
            user_data_path: user_data
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            user_data_existed_before,
            user_data_exists_after: user_data.as_ref().is_some_and(|path| path.exists()),
            steps,
            final_status,
        })
    })();

    if result.is_err() {
        let _ = std::fs::remove_dir_all(&root);
        let _ = remove_managed(&settings.install_root);
        cleanup_portable_shell_entry();
    }
    result
}

fn run_force_portable_cycle() -> Result<SmokeReport, String> {
    let (settings, endpoints) = settings_and_endpoints();
    let user_data = user_data_path();
    let user_data_existed_before = user_data.as_ref().is_some_and(|path| path.exists());
    let mut steps = Vec::new();

    let initial = win_install_status(&settings);
    step(&mut steps, "initial-status", &initial)?;
    let prefetch = prefetch_verified_msix(&endpoints)?;
    step(&mut steps, "prefetch-verified-msix", &prefetch)?;
    ensure_no_install(&settings, &mut steps)?;

    let portable_install = install_portable_from_msix(
        Path::new(&prefetch.staged_path),
        Path::new(&settings.install_root),
        false,
    )
    .map_err(|e| format!("force portable install: {e}"))?;
    record_managed(
        settings.install_root.clone(),
        &portable_install.version,
        "smoke-forced-portable",
    )?;
    step(&mut steps, "force-portable-install", &portable_install)?;
    let after_install = win_install_status(&settings);
    step(&mut steps, "status-after-portable-install", &after_install)?;
    if after_install.status != "managed"
        || after_install.installed.as_ref().map(|i| i.source.as_str()) != Some("portable")
    {
        return Err("expected managed portable install after forced portable install".to_string());
    }

    let portable_update = install_portable_from_msix(
        Path::new(&prefetch.staged_path),
        Path::new(&settings.install_root),
        false,
    )
    .map_err(|e| format!("force portable update: {e}"))?;
    record_managed(
        settings.install_root.clone(),
        &portable_update.version,
        "smoke-forced-portable-update",
    )?;
    step(
        &mut steps,
        "force-portable-same-version-update",
        &portable_update,
    )?;
    let after_update = win_install_status(&settings);
    step(&mut steps, "status-after-portable-update", &after_update)?;
    if after_update.status != "managed"
        || after_update.installed.as_ref().map(|i| i.version.as_str())
            != Some(prefetch.release.version.as_str())
    {
        return Err("expected managed latest portable after forced portable update".to_string());
    }

    let uninstalled =
        uninstall_windows_codex(&settings, true, false).map_err(|e| format!("{e}"))?;
    step(
        &mut steps,
        "uninstall-forced-portable-preserve-user-data",
        &uninstalled,
    )?;
    remove_rollback(&settings);
    let after_uninstall = win_install_status(&settings);
    step(
        &mut steps,
        "status-after-forced-portable-uninstall",
        &after_uninstall,
    )?;
    if after_uninstall.status != "none" {
        return Err(format!(
            "expected no install after forced portable uninstall, got {}",
            after_uninstall.status
        ));
    }

    let final_status = reinstall_final_msix(&endpoints, &settings, &mut steps)?;

    Ok(SmokeReport {
        install_root: settings.install_root,
        user_data_path: user_data
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        user_data_existed_before,
        user_data_exists_after: user_data.as_ref().is_some_and(|path| path.exists()),
        steps,
        final_status,
    })
}

fn write_fake_old_portable(root: &Path) -> Result<(), String> {
    if root.exists() {
        std::fs::remove_dir_all(root).map_err(|e| format!("remove existing fake root: {e}"))?;
    }
    std::fs::create_dir_all(root.join("resources"))
        .map_err(|e| format!("create fake portable root: {e}"))?;
    std::fs::write(root.join("Codex.exe"), b"fake-old-codex")
        .map_err(|e| format!("write fake Codex.exe: {e}"))?;
    std::fs::write(
        root.join("AppxManifest.xml"),
        r#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Identity Name="OpenAI.Codex"
            Publisher="CN=50BDFD77-8903-4850-9FFE-6E8522F64D5B"
            Version="0.0.1.0"
            ProcessorArchitecture="x64" />
</Package>"#,
    )
    .map_err(|e| format!("write fake AppxManifest.xml: {e}"))?;
    Ok(())
}

fn run_fake_old_portable_update() -> Result<SmokeReport, String> {
    let (settings, endpoints) = settings_and_endpoints();
    let user_data = user_data_path();
    let user_data_existed_before = user_data.as_ref().is_some_and(|path| path.exists());
    let mut steps = Vec::new();

    let initial = win_install_status(&settings);
    step(&mut steps, "initial-status", &initial)?;
    let prefetch = prefetch_verified_msix(&endpoints)?;
    step(&mut steps, "prefetch-verified-msix", &prefetch)?;
    ensure_no_install(&settings, &mut steps)?;

    let root = PathBuf::from(&settings.install_root);
    write_fake_old_portable(&root)?;
    record_managed(
        settings.install_root.clone(),
        "0.0.1.0",
        "smoke-fake-old-portable",
    )?;
    let old_status = win_install_status(&settings);
    step(&mut steps, "status-after-fake-old-portable", &old_status)?;
    if old_status.installed.as_ref().map(|i| i.version.as_str()) != Some("0.0.1.0") {
        return Err("expected fake old portable version 0.0.1.0".to_string());
    }

    let updated = install_portable_from_msix(
        Path::new(&prefetch.staged_path),
        Path::new(&settings.install_root),
        false,
    )
    .map_err(|e| format!("update fake old portable: {e}"))?;
    record_managed(
        settings.install_root.clone(),
        &updated.version,
        "smoke-fake-old-portable-updated",
    )?;
    step(&mut steps, "update-fake-old-portable-to-latest", &updated)?;
    let after_update = win_install_status(&settings);
    step(&mut steps, "status-after-fake-old-update", &after_update)?;
    if after_update.installed.as_ref().map(|i| i.version.as_str())
        != Some(prefetch.release.version.as_str())
    {
        return Err("expected fake old portable updated to latest".to_string());
    }

    let uninstalled =
        uninstall_windows_codex(&settings, true, false).map_err(|e| format!("{e}"))?;
    step(
        &mut steps,
        "uninstall-updated-fake-old-portable-preserve-user-data",
        &uninstalled,
    )?;
    remove_rollback(&settings);
    let after_uninstall = win_install_status(&settings);
    step(
        &mut steps,
        "status-after-fake-old-update-uninstall",
        &after_uninstall,
    )?;
    if after_uninstall.status != "none" {
        return Err(format!(
            "expected no install after fake old update uninstall, got {}",
            after_uninstall.status
        ));
    }

    let final_status = reinstall_final_msix(&endpoints, &settings, &mut steps)?;

    Ok(SmokeReport {
        install_root: settings.install_root,
        user_data_path: user_data
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        user_data_existed_before,
        user_data_exists_after: user_data.as_ref().is_some_and(|path| path.exists()),
        steps,
        final_status,
    })
}

fn run_managed_portable_shadow_msix() -> Result<SmokeReport, String> {
    let (settings, endpoints) = settings_and_endpoints();
    let user_data = user_data_path();
    let user_data_existed_before = user_data.as_ref().is_some_and(|path| path.exists());
    let mut steps = Vec::new();
    let root = PathBuf::from(&settings.install_root);

    let initial = win_install_status(&settings);
    step(&mut steps, "initial-status", &initial)?;
    if root.exists() {
        return Err(format!(
            "refusing to create fake portable because install_root already exists: {}",
            root.display()
        ));
    }

    let result = (|| {
        write_fake_old_portable(&root)?;
        record_managed(
            settings.install_root.clone(),
            "0.0.1.0",
            "smoke-shadow-portable",
        )?;

        let shadow_status = win_install_status(&settings);
        step(
            &mut steps,
            "status-prefers-managed-portable",
            &shadow_status,
        )?;
        if shadow_status
            .installed
            .as_ref()
            .map(|i| (i.source.as_str(), i.version.as_str()))
            != Some(("portable", "0.0.1.0"))
        {
            return Err("expected managed fake portable to shadow any lingering MSIX".to_string());
        }

        let plan = plan_windows_update(&endpoints, &settings).map_err(|e| e.to_string())?;
        step(&mut steps, "plan-uses-managed-portable", &plan)?;
        if plan
            .installed
            .as_ref()
            .map(|i| (i.source.as_str(), i.version.as_str()))
            != Some(("portable", "0.0.1.0"))
            || plan.plan.current_version.as_deref() != Some("0.0.1.0")
        {
            return Err("expected update plan to compare against managed portable".to_string());
        }

        let uninstalled =
            uninstall_windows_codex(&settings, true, false).map_err(|e| e.to_string())?;
        step(
            &mut steps,
            "uninstall-removes-managed-portable",
            &uninstalled,
        )?;
        if !uninstalled.success || uninstalled.action != "remove-portable" {
            return Err("expected uninstall to remove the managed portable shadow".to_string());
        }
        if root.exists() {
            return Err("expected fake portable root to be removed after uninstall".to_string());
        }

        let final_status = win_install_status(&settings);
        step(
            &mut steps,
            "final-status-after-shadow-cleanup",
            &final_status,
        )?;
        if final_status.installed.as_ref().map(|i| i.source.as_str()) == Some("portable") {
            return Err("fake portable still shadows final status".to_string());
        }

        Ok(SmokeReport {
            install_root: settings.install_root.clone(),
            user_data_path: user_data
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned()),
            user_data_existed_before,
            user_data_exists_after: user_data.as_ref().is_some_and(|path| path.exists()),
            steps,
            final_status,
        })
    })();

    if result.is_err() {
        let _ = std::fs::remove_dir_all(&root);
        let _ = remove_managed(&settings.install_root);
    }
    result
}

fn run_old_msix_to_latest_msix() -> Result<SmokeReport, String> {
    let (settings, endpoints) = settings_and_endpoints();
    let user_data = user_data_path();
    let user_data_existed_before = user_data.as_ref().is_some_and(|path| path.exists());
    let mut steps = Vec::new();

    let initial = win_install_status(&settings);
    step(&mut steps, "initial-status", &initial)?;
    let old = prefetch_verified_old_msix()?;
    step(&mut steps, "prefetch-verified-old-msix", &old)?;
    let latest = prefetch_verified_msix(&endpoints)?;
    step(&mut steps, "prefetch-verified-latest-msix", &latest)?;
    ensure_no_install(&settings, &mut steps)?;

    let old_install =
        install_msix_sideload(Path::new(&old.staged_path)).map_err(|e| format!("{e}"))?;
    step(&mut steps, "install-old-msix", &old_install)?;
    if !old_install.success {
        return Err(format!("old MSIX install failed: {}", old_install.message));
    }
    let old_status = win_install_status(&settings);
    if let Some(installed) = &old_status.installed {
        record_managed(installed.path.clone(), &installed.version, "smoke-old-msix")?;
    }
    let old_status = win_install_status(&settings);
    step(&mut steps, "status-after-old-msix-install", &old_status)?;
    if old_status.status != "managed"
        || old_status.installed.as_ref().map(|i| i.version.as_str()) != Some(OLD_MSIX_VERSION)
    {
        return Err("expected managed old MSIX install before upgrade".to_string());
    }

    let upgraded = perform_windows_update(&endpoints, &settings, true)
        .map_err(|e| format!("upgrade old MSIX to latest: {e}"))?;
    step(&mut steps, "upgrade-old-msix-to-latest", &upgraded)?;
    if upgraded.action != WinPerformAction::MsixSideload {
        return Err(format!(
            "expected MSIX sideload upgrade action, got {}",
            upgraded.action
        ));
    }
    let final_status = win_install_status(&settings);
    step(&mut steps, "final-status", &final_status)?;
    if final_status.status != "managed"
        || final_status
            .installed
            .as_ref()
            .map(|i| (i.source.as_str(), i.version.as_str()))
            != Some(("msix", latest.release.version.as_str()))
    {
        return Err("expected final managed latest MSIX after old MSIX upgrade".to_string());
    }

    Ok(SmokeReport {
        install_root: settings.install_root,
        user_data_path: user_data
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        user_data_existed_before,
        user_data_exists_after: user_data.as_ref().is_some_and(|path| path.exists()),
        steps,
        final_status,
    })
}

fn run_old_portable_to_latest_portable() -> Result<SmokeReport, String> {
    let (settings, endpoints) = settings_and_endpoints();
    let user_data = user_data_path();
    let user_data_existed_before = user_data.as_ref().is_some_and(|path| path.exists());
    let mut steps = Vec::new();

    let initial = win_install_status(&settings);
    step(&mut steps, "initial-status", &initial)?;
    let old = prefetch_verified_old_msix()?;
    step(&mut steps, "prefetch-verified-old-msix", &old)?;
    let latest = prefetch_verified_msix(&endpoints)?;
    step(&mut steps, "prefetch-verified-latest-msix", &latest)?;
    ensure_no_install(&settings, &mut steps)?;

    let old_portable = install_portable_from_msix(
        Path::new(&old.staged_path),
        Path::new(&settings.install_root),
        false,
    )
    .map_err(|e| format!("install old portable: {e}"))?;
    record_managed(
        settings.install_root.clone(),
        &old_portable.version,
        "smoke-old-portable",
    )?;
    step(&mut steps, "install-old-portable", &old_portable)?;
    let old_status = win_install_status(&settings);
    step(&mut steps, "status-after-old-portable-install", &old_status)?;
    if old_status.status != "managed"
        || old_status
            .installed
            .as_ref()
            .map(|i| (i.source.as_str(), i.version.as_str()))
            != Some(("portable", OLD_MSIX_VERSION))
    {
        return Err("expected managed old portable install before upgrade".to_string());
    }

    let updated_portable = install_portable_from_msix(
        Path::new(&latest.staged_path),
        Path::new(&settings.install_root),
        false,
    )
    .map_err(|e| format!("update old portable to latest: {e}"))?;
    record_managed(
        settings.install_root.clone(),
        &updated_portable.version,
        "smoke-old-portable-updated",
    )?;
    step(
        &mut steps,
        "upgrade-old-portable-to-latest-portable",
        &updated_portable,
    )?;
    let after_update = win_install_status(&settings);
    step(
        &mut steps,
        "status-after-old-portable-upgrade",
        &after_update,
    )?;
    if after_update.status != "managed"
        || after_update
            .installed
            .as_ref()
            .map(|i| (i.source.as_str(), i.version.as_str()))
            != Some(("portable", latest.release.version.as_str()))
    {
        return Err("expected managed latest portable after old portable upgrade".to_string());
    }

    let uninstalled =
        uninstall_windows_codex(&settings, true, false).map_err(|e| format!("{e}"))?;
    step(
        &mut steps,
        "uninstall-upgraded-old-portable-preserve-user-data",
        &uninstalled,
    )?;
    remove_rollback(&settings);
    let after_uninstall = win_install_status(&settings);
    step(
        &mut steps,
        "status-after-old-portable-uninstall",
        &after_uninstall,
    )?;
    if after_uninstall.status != "none" {
        return Err(format!(
            "expected no install after old portable uninstall, got {}",
            after_uninstall.status
        ));
    }

    let final_status = reinstall_final_msix(&endpoints, &settings, &mut steps)?;

    Ok(SmokeReport {
        install_root: settings.install_root,
        user_data_path: user_data
            .as_ref()
            .map(|path| path.to_string_lossy().into_owned()),
        user_data_existed_before,
        user_data_exists_after: user_data.as_ref().is_some_and(|path| path.exists()),
        steps,
        final_status,
    })
}

fn run() -> Result<serde_json::Value, String> {
    let (settings, endpoints) = settings_and_endpoints();
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str).unwrap_or("status") {
        "status" => serde_json::to_value(win_install_status(&settings)).map_err(|e| e.to_string()),
        "prefetch" => {
            serde_json::to_value(prefetch_verified_msix(&endpoints)?).map_err(|e| e.to_string())
        }
        "install" => serde_json::to_value(
            perform_windows_update(&endpoints, &settings, true).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string()),
        "uninstall" => serde_json::to_value(
            uninstall_windows_codex(&settings, true, false).map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string()),
        "install-uninstall-install" => {
            serde_json::to_value(run_install_uninstall_install()?).map_err(|e| e.to_string())
        }
        "force-portable-cycle" => {
            serde_json::to_value(run_force_portable_cycle()?).map_err(|e| e.to_string())
        }
        "fake-old-portable-update" => {
            serde_json::to_value(run_fake_old_portable_update()?).map_err(|e| e.to_string())
        }
        "msix-uninstall-purge-user-data" | "msix-uninstall-purge-temp-profile" => {
            serde_json::to_value(run_msix_uninstall_purge_user_data_restore()?)
                .map_err(|e| e.to_string())
        }
        "portable-apps-features-uninstall-entry" => {
            serde_json::to_value(run_portable_apps_features_uninstall_entry()?)
                .map_err(|e| e.to_string())
        }
        "managed-portable-shadow-msix" => {
            serde_json::to_value(run_managed_portable_shadow_msix()?).map_err(|e| e.to_string())
        }
        "old-msix-to-latest-msix" => {
            serde_json::to_value(run_old_msix_to_latest_msix()?).map_err(|e| e.to_string())
        }
        "old-portable-to-latest-portable" => {
            serde_json::to_value(run_old_portable_to_latest_portable()?)
                .map_err(|e| e.to_string())
        }
        other => Err(format!(
            "unknown command {other}; use status | prefetch | install | uninstall | install-uninstall-install | force-portable-cycle | fake-old-portable-update | msix-uninstall-purge-user-data | portable-apps-features-uninstall-entry | managed-portable-shadow-msix | old-msix-to-latest-msix | old-portable-to-latest-portable"
        )),
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(value) => {
            println!(
                "{}",
                serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("{err}");
            ExitCode::FAILURE
        }
    }
}
