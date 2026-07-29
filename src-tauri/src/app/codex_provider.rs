use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use fs4::{FileExt as Fs4FileExt, TryLockError};
use serde::Serialize;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::app::atomic_file;
use crate::app::orange_api::ORANGE_BASE_URL;

const MODEL: &str = "gpt-5.5";
const USAGE_SCRIPT: &str = r#"({
    request: {
      url: "{{baseUrl}}/v1/usage",
      method: "GET",
      headers: { "Authorization": "Bearer {{apiKey}}" }
    },
    extractor: function(response) {
      const remaining = response?.remaining ?? response?.quota?.remaining ?? response?.balance;
      const unit = response?.unit ?? response?.quota?.unit ?? "USD";
      return {
        isValid: response?.is_active ?? response?.isValid ?? true,
        remaining,
        unit
      };
    }
  })"#;

pub const CONFIG_TEMPLATE: &str = r#"model_provider = "OpenAI"
model = "gpt-5.5"
review_model = "gpt-5.5"
model_reasoning_effort = "xhigh"
disable_response_storage = true
network_access = "enabled"
windows_wsl_setup_acknowledged = true

[model_providers.OpenAI]
name = "OpenAI"
base_url = "https://token.cylonai.cn"
wire_api = "responses"
requires_openai_auth = true

[features]
goals = true"#;

#[derive(Debug, Clone)]
pub struct ProviderPaths {
    pub codex_home: PathBuf,
    pub backup_root: PathBuf,
}

impl ProviderPaths {
    pub fn system() -> Result<Self, ProviderWriteError> {
        Ok(Self {
            codex_home: crate::app::paths::codex_home_dir()
                .ok_or(ProviderWriteError::PathUnavailable)?,
            backup_root: crate::app::paths::orange_backup_root()
                .ok_or(ProviderWriteError::PathUnavailable)?,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ProviderWriteOutcome {
    Committed,
    FailedBeforeMutation,
    Restored,
    RecoveryRequired,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderWriteReport {
    pub outcome: ProviderWriteOutcome,
    pub backup_dir: Option<String>,
    pub config_path: String,
    pub auth_path: String,
    pub codex_was_running: bool,
    pub write_verified: bool,
    pub rollback_verified: bool,
    pub error_code: Option<String>,
}

#[derive(Debug, Error)]
pub enum ProviderWriteError {
    #[error("Codex configuration path is unavailable")]
    PathUnavailable,
    #[error("Codex configuration destination is unsafe")]
    UnsafeDestination,
    #[error("another Codex provider write is already running")]
    Busy,
    #[error("the API key is empty")]
    EmptyKey,
    #[error("Codex provider IO failed")]
    Io,
    #[error("CC Switch is unavailable")]
    CcSwitchUnavailable,
    #[error("this operation is not supported on the current platform")]
    UnsupportedPlatform,
}

impl ProviderWriteError {
    pub fn code(&self) -> &'static str {
        match self {
            Self::PathUnavailable => "provider_path_unavailable",
            Self::UnsafeDestination => "provider_unsafe_destination",
            Self::Busy => "provider_busy",
            Self::EmptyKey => "provider_empty_key",
            Self::Io => "provider_io",
            Self::CcSwitchUnavailable => "ccswitch_unavailable",
            Self::UnsupportedPlatform => "unsupported_platform",
        }
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupManifest {
    version: u32,
    created_at_unix_ms: u128,
    files: Vec<BackupEntry>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BackupEntry {
    name: &'static str,
    existed: bool,
    sha256: Option<String>,
}

struct Preimage {
    config: Option<Vec<u8>>,
    auth: Option<Vec<u8>>,
}

struct ProviderLock(File);

impl Drop for ProviderLock {
    fn drop(&mut self) {
        let _ = Fs4FileExt::unlock(&self.0);
    }
}

pub struct ProviderWritePermit {
    paths: ProviderPaths,
    _lock: ProviderLock,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ProviderWriteFault {
    None,
    #[cfg(test)]
    BeforeAuthReplace,
    #[cfg(test)]
    VerificationMismatch,
    #[cfg(test)]
    RollbackFailure,
}

pub fn build_ccswitch_uri(provider_name: &str, api_key: &str) -> String {
    let usage_script = STANDARD.encode(USAGE_SCRIPT.as_bytes());
    let mut url = url::Url::parse("ccswitch://v1/import").expect("fixed CC Switch URL");
    url.query_pairs_mut()
        .append_pair("resource", "provider")
        .append_pair("app", "codex")
        .append_pair("model", MODEL)
        .append_pair("name", provider_name)
        .append_pair("homepage", ORANGE_BASE_URL)
        .append_pair("endpoint", ORANGE_BASE_URL)
        .append_pair("apiKey", api_key)
        .append_pair("configFormat", "json")
        .append_pair("usageEnabled", "true")
        .append_pair("usageScript", &usage_script)
        .append_pair("usageAutoInterval", "30");
    url.to_string()
}

pub fn import_into_ccswitch(api_key: &str) -> Result<(), ProviderWriteError> {
    if api_key.trim().is_empty() {
        return Err(ProviderWriteError::EmptyKey);
    }
    let uri = build_ccswitch_uri("OrangeAPI", api_key);
    open_ccswitch_uri(&uri)
}

pub fn begin_provider_write() -> Result<ProviderWritePermit, ProviderWriteError> {
    begin_provider_write_at(&ProviderPaths::system()?)
}

fn begin_provider_write_at(
    paths: &ProviderPaths,
) -> Result<ProviderWritePermit, ProviderWriteError> {
    fs::create_dir_all(&paths.codex_home).map_err(|_| ProviderWriteError::Io)?;
    fs::create_dir_all(&paths.backup_root).map_err(|_| ProviderWriteError::Io)?;
    set_private_directory(&paths.backup_root).map_err(|_| ProviderWriteError::Io)?;
    let canonical_home = paths
        .codex_home
        .canonicalize()
        .map_err(|_| ProviderWriteError::PathUnavailable)?;
    let canonical_backup_root = paths
        .backup_root
        .canonicalize()
        .map_err(|_| ProviderWriteError::PathUnavailable)?;
    let lock = acquire_lock(&canonical_backup_root)?;
    Ok(ProviderWritePermit {
        paths: ProviderPaths {
            codex_home: canonical_home,
            backup_root: canonical_backup_root,
        },
        _lock: lock,
    })
}

pub fn write_provider_files(
    permit: ProviderWritePermit,
    api_key: &str,
    codex_was_running: bool,
) -> Result<ProviderWriteReport, ProviderWriteError> {
    write_provider_files_with_permit(
        &permit,
        api_key,
        codex_was_running,
        ProviderWriteFault::None,
    )
}

pub fn write_provider_files_at(
    paths: &ProviderPaths,
    api_key: &str,
    codex_was_running: bool,
) -> Result<ProviderWriteReport, ProviderWriteError> {
    write_provider_files_at_with_fault(paths, api_key, codex_was_running, ProviderWriteFault::None)
}

fn write_provider_files_at_with_fault(
    paths: &ProviderPaths,
    api_key: &str,
    codex_was_running: bool,
    fault: ProviderWriteFault,
) -> Result<ProviderWriteReport, ProviderWriteError> {
    let permit = begin_provider_write_at(paths)?;
    write_provider_files_with_permit(&permit, api_key, codex_was_running, fault)
}

fn write_provider_files_with_permit(
    permit: &ProviderWritePermit,
    api_key: &str,
    codex_was_running: bool,
    _fault: ProviderWriteFault,
) -> Result<ProviderWriteReport, ProviderWriteError> {
    if api_key.trim().is_empty() {
        return Err(ProviderWriteError::EmptyKey);
    }

    let config_path = permit.paths.codex_home.join("config.toml");
    let auth_path = permit.paths.codex_home.join("auth.json");
    reject_symlink(&config_path)?;
    reject_symlink(&auth_path)?;

    let preimage = Preimage {
        config: read_optional(&config_path)?,
        auth: read_optional(&auth_path)?,
    };
    let backup_dir =
        permit
            .paths
            .backup_root
            .join(format!("{}-{}", unix_millis(), uuid::Uuid::new_v4()));
    let backup_string = backup_dir.to_string_lossy().into_owned();
    let mut report = report_base(
        &config_path,
        &auth_path,
        codex_was_running,
        Some(backup_string),
    );

    if create_backup(&backup_dir, &preimage).is_err() {
        report.outcome = ProviderWriteOutcome::FailedBeforeMutation;
        report.error_code = Some("provider_backup_failed".into());
        return Ok(report);
    }

    let auth_bytes = auth_template(api_key).map_err(|_| ProviderWriteError::Io)?;
    let mutation_result = (|| -> Result<(), ProviderWriteError> {
        atomic_file::write_atomic(&config_path, CONFIG_TEMPLATE.as_bytes())
            .map_err(|_| ProviderWriteError::Io)?;
        #[cfg(test)]
        if _fault == ProviderWriteFault::BeforeAuthReplace {
            return Err(ProviderWriteError::Io);
        }
        atomic_file::write_atomic(&auth_path, &auth_bytes).map_err(|_| ProviderWriteError::Io)?;
        set_private_file(&auth_path).map_err(|_| ProviderWriteError::Io)?;
        #[cfg(test)]
        if matches!(
            _fault,
            ProviderWriteFault::VerificationMismatch | ProviderWriteFault::RollbackFailure
        ) {
            fs::write(&auth_path, b"{}").map_err(|_| ProviderWriteError::Io)?;
        }
        verify_written(&config_path, &auth_path, api_key)
    })();

    match mutation_result {
        Ok(()) => {
            remove_atomic_sidecars(&config_path, &auth_path);
            report.outcome = ProviderWriteOutcome::Committed;
            report.write_verified = true;
            Ok(report)
        }
        Err(error) => {
            #[cfg(test)]
            let force_rollback_failure = _fault == ProviderWriteFault::RollbackFailure;
            #[cfg(not(test))]
            let force_rollback_failure = false;
            let rollback = if force_rollback_failure {
                false
            } else {
                restore_preimage(&config_path, &auth_path, &preimage).is_ok()
                    && verify_preimage(&config_path, &auth_path, &preimage)
            };
            remove_atomic_sidecars(&config_path, &auth_path);
            report.outcome = if rollback {
                ProviderWriteOutcome::Restored
            } else {
                ProviderWriteOutcome::RecoveryRequired
            };
            report.rollback_verified = rollback;
            report.error_code = Some(error.code().into());
            Ok(report)
        }
    }
}

pub fn read_local_api_key() -> Option<String> {
    let home = crate::app::paths::codex_home_dir()?;
    read_local_api_key_at(&home)
}

pub fn read_local_api_key_at(codex_home: &Path) -> Option<String> {
    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(codex_home.join("auth.json")).ok()?).ok()?;
    value
        .get("OPENAI_API_KEY")?
        .as_str()
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn auth_template(api_key: &str) -> Result<Vec<u8>, serde_json::Error> {
    let mut object = serde_json::Map::new();
    object.insert(
        "OPENAI_API_KEY".into(),
        serde_json::Value::String(api_key.into()),
    );
    serde_json::to_vec_pretty(&serde_json::Value::Object(object))
}

fn acquire_lock(backup_root: &Path) -> Result<ProviderLock, ProviderWriteError> {
    let path = backup_root.join("provider-write.lock");
    let file = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .open(path)
        .map_err(|_| ProviderWriteError::Io)?;
    match Fs4FileExt::try_lock(&file) {
        Ok(()) => Ok(ProviderLock(file)),
        Err(TryLockError::WouldBlock) => Err(ProviderWriteError::Busy),
        Err(TryLockError::Error(_)) => Err(ProviderWriteError::Io),
    }
}

fn reject_symlink(path: &Path) -> Result<(), ProviderWriteError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            Err(ProviderWriteError::UnsafeDestination)
        }
        Ok(_) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(ProviderWriteError::Io),
    }
}

fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, ProviderWriteError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(ProviderWriteError::Io),
    }
}

fn create_backup(backup_dir: &Path, preimage: &Preimage) -> io::Result<()> {
    fs::create_dir_all(backup_dir)?;
    set_private_directory(backup_dir)?;
    if let Some(bytes) = &preimage.config {
        write_durable(&backup_dir.join("config.toml"), bytes)?;
    }
    if let Some(bytes) = &preimage.auth {
        write_private_durable(&backup_dir.join("auth.json"), bytes)?;
    }
    let manifest = BackupManifest {
        version: 1,
        created_at_unix_ms: unix_millis(),
        files: vec![
            BackupEntry {
                name: "config.toml",
                existed: preimage.config.is_some(),
                sha256: preimage.config.as_deref().map(sha256_hex),
            },
            BackupEntry {
                name: "auth.json",
                existed: preimage.auth.is_some(),
                sha256: preimage.auth.as_deref().map(sha256_hex),
            },
        ],
    };
    let manifest = serde_json::to_vec_pretty(&manifest).map_err(io::Error::other)?;
    write_durable(&backup_dir.join("manifest.json"), &manifest)?;
    sync_dir(backup_dir)
}

fn write_durable(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()
}

fn write_private_durable(path: &Path, bytes: &[u8]) -> io::Result<()> {
    write_durable(path, bytes)?;
    set_private_file(path)
}

fn verify_written(
    config_path: &Path,
    auth_path: &Path,
    api_key: &str,
) -> Result<(), ProviderWriteError> {
    if fs::read(config_path).map_err(|_| ProviderWriteError::Io)? != CONFIG_TEMPLATE.as_bytes() {
        return Err(ProviderWriteError::Io);
    }
    let auth: serde_json::Value =
        serde_json::from_slice(&fs::read(auth_path).map_err(|_| ProviderWriteError::Io)?)
            .map_err(|_| ProviderWriteError::Io)?;
    if auth.get("OPENAI_API_KEY").and_then(|value| value.as_str()) != Some(api_key) {
        return Err(ProviderWriteError::Io);
    }
    Ok(())
}

fn restore_preimage(
    config_path: &Path,
    auth_path: &Path,
    preimage: &Preimage,
) -> Result<(), ProviderWriteError> {
    restore_one(config_path, preimage.config.as_deref())?;
    restore_one(auth_path, preimage.auth.as_deref())?;
    if preimage.auth.is_some() {
        set_private_file(auth_path).map_err(|_| ProviderWriteError::Io)?;
    }
    Ok(())
}

fn restore_one(path: &Path, bytes: Option<&[u8]>) -> Result<(), ProviderWriteError> {
    match bytes {
        Some(bytes) => atomic_file::write_atomic(path, bytes).map_err(|_| ProviderWriteError::Io),
        None => match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(ProviderWriteError::Io),
        },
    }
}

fn verify_preimage(config_path: &Path, auth_path: &Path, preimage: &Preimage) -> bool {
    read_optional(config_path).ok() == Some(preimage.config.clone())
        && read_optional(auth_path).ok() == Some(preimage.auth.clone())
}

fn remove_atomic_sidecars(config_path: &Path, auth_path: &Path) {
    let _ = fs::remove_file(atomic_file::backup_path(config_path));
    let _ = fs::remove_file(atomic_file::backup_path(auth_path));
}

fn report_base(
    config_path: &Path,
    auth_path: &Path,
    codex_was_running: bool,
    backup_dir: Option<String>,
) -> ProviderWriteReport {
    ProviderWriteReport {
        outcome: ProviderWriteOutcome::FailedBeforeMutation,
        backup_dir,
        config_path: config_path.to_string_lossy().into_owned(),
        auth_path: auth_path.to_string_lossy().into_owned(),
        codex_was_running,
        write_verified: false,
        rollback_verified: false,
        error_code: None,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

fn unix_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or(0)
}

#[cfg(unix)]
fn sync_dir(path: &Path) -> io::Result<()> {
    File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn set_private_file(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_private_file(_path: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(target_os = "windows")]
fn open_ccswitch_uri(uri: &str) -> Result<(), ProviderWriteError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::ShellExecuteW;
    use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

    let operation: Vec<u16> = std::ffi::OsStr::new("open")
        .encode_wide()
        .chain(Some(0))
        .collect();
    let target: Vec<u16> = std::ffi::OsStr::new(uri)
        .encode_wide()
        .chain(Some(0))
        .collect();
    let result = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            operation.as_ptr(),
            target.as_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    if result as isize <= 32 {
        Err(ProviderWriteError::CcSwitchUnavailable)
    } else {
        Ok(())
    }
}

#[cfg(target_os = "macos")]
fn open_ccswitch_uri(uri: &str) -> Result<(), ProviderWriteError> {
    let status = std::process::Command::new("/usr/bin/open")
        .arg(uri)
        .status()
        .map_err(|_| ProviderWriteError::CcSwitchUnavailable)?;
    if status.success() {
        Ok(())
    } else {
        Err(ProviderWriteError::CcSwitchUnavailable)
    }
}

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
fn open_ccswitch_uri(_uri: &str) -> Result<(), ProviderWriteError> {
    Err(ProviderWriteError::UnsupportedPlatform)
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn test_root(name: &str) -> PathBuf {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-data")
            .join(format!("{name}-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn fixture(name: &str) -> (PathBuf, ProviderPaths) {
        let root = test_root(name);
        let paths = ProviderPaths {
            codex_home: root.join(".codex"),
            backup_root: root.join("backups"),
        };
        fs::create_dir_all(&paths.codex_home).unwrap();
        fs::write(paths.codex_home.join("config.toml"), "old-config").unwrap();
        fs::write(
            paths.codex_home.join("auth.json"),
            r#"{"OPENAI_API_KEY":"old"}"#,
        )
        .unwrap();
        (root, paths)
    }

    #[test]
    fn ccs_link_matches_orangeapi_codex_contract() {
        let uri = build_ccswitch_uri("OrangeAPI", "sk-a+b&c");
        let parsed = url::Url::parse(&uri).unwrap();
        let query = parsed.query_pairs().into_owned().collect::<HashMap<_, _>>();
        assert_eq!(parsed.scheme(), "ccswitch");
        assert_eq!(parsed.host_str(), Some("v1"));
        assert_eq!(parsed.path(), "/import");
        assert_eq!(query["resource"], "provider");
        assert_eq!(query["app"], "codex");
        assert_eq!(query["model"], MODEL);
        assert_eq!(query["endpoint"], ORANGE_BASE_URL);
        assert_eq!(query["apiKey"], "sk-a+b&c");
        let script = STANDARD.decode(&query["usageScript"]).unwrap();
        assert!(String::from_utf8(script)
            .unwrap()
            .contains("{{baseUrl}}/v1/usage"));
    }

    #[test]
    fn replaces_both_files_after_durable_backup_and_verification() {
        let (root, paths) = fixture("provider-commit");
        let report = write_provider_files_at(&paths, "sk-new", false).unwrap();
        assert_eq!(report.outcome, ProviderWriteOutcome::Committed);
        assert!(report.write_verified);
        assert_eq!(
            fs::read_to_string(paths.codex_home.join("config.toml")).unwrap(),
            CONFIG_TEMPLATE
        );
        let backup = PathBuf::from(report.backup_dir.unwrap());
        assert_eq!(
            fs::read_to_string(backup.join("config.toml")).unwrap(),
            "old-config"
        );
        assert_eq!(
            read_local_api_key_at(&paths.codex_home).as_deref(),
            Some("sk-new")
        );
        assert!(backup.join("manifest.json").is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;

            assert_eq!(
                fs::metadata(paths.codex_home.join("auth.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(backup.join("auth.json"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o600
            );
            assert_eq!(
                fs::metadata(&backup).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn provider_write_permit_excludes_other_writers() {
        let (root, paths) = fixture("provider-lock");
        let permit = begin_provider_write_at(&paths).unwrap();

        assert!(matches!(
            begin_provider_write_at(&paths),
            Err(ProviderWriteError::Busy)
        ));

        drop(permit);
        assert!(begin_provider_write_at(&paths).is_ok());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn second_replace_failure_restores_both_preimages() {
        let (root, paths) = fixture("provider-rollback");
        let report = write_provider_files_at_with_fault(
            &paths,
            "sk-new",
            true,
            ProviderWriteFault::BeforeAuthReplace,
        )
        .unwrap();
        assert_eq!(report.outcome, ProviderWriteOutcome::Restored);
        assert!(report.rollback_verified);
        assert_eq!(
            fs::read_to_string(paths.codex_home.join("config.toml")).unwrap(),
            "old-config"
        );
        assert_eq!(
            read_local_api_key_at(&paths.codex_home).as_deref(),
            Some("old")
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn originally_missing_files_are_removed_during_rollback() {
        let root = test_root("provider-missing-rollback");
        let paths = ProviderPaths {
            codex_home: root.join(".codex"),
            backup_root: root.join("backups"),
        };
        fs::create_dir_all(&paths.codex_home).unwrap();
        let report = write_provider_files_at_with_fault(
            &paths,
            "sk-new",
            false,
            ProviderWriteFault::BeforeAuthReplace,
        )
        .unwrap();
        assert_eq!(report.outcome, ProviderWriteOutcome::Restored);
        assert!(!paths.codex_home.join("config.toml").exists());
        assert!(!paths.codex_home.join("auth.json").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn verification_failure_with_failed_rollback_requires_recovery() {
        let (root, paths) = fixture("provider-recovery-required");
        let report = write_provider_files_at_with_fault(
            &paths,
            "sk-new",
            false,
            ProviderWriteFault::RollbackFailure,
        )
        .unwrap();
        assert_eq!(report.outcome, ProviderWriteOutcome::RecoveryRequired);
        assert!(!report.rollback_verified);
        assert!(report.backup_dir.is_some());
        let _ = fs::remove_dir_all(root);
    }
}
