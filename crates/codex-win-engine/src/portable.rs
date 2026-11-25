use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{self, Read};
use std::path::{Component, Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::app_version::read_codex_app_version_from_install_root;
use crate::msix::{parse_appx_manifest_xml, MsixIdentity};
use crate::process::{
    hidden_command, LivenessResult, PORTABLE_LIVENESS_WINDOW, PORTABLE_STARTUP_WINDOW,
};
#[cfg(windows)]
use crate::process::{run_capturing, RunLimits};
use crate::EngineError;

pub const PORTABLE_STAGING_DIR_NAME: &str = ".codex-app-manager-staging";
pub const PORTABLE_FILES_MANIFEST_NAME: &str = "codex-portable-files.txt";
const PRESERVED_FILES_DIR_PREFIX: &str = "Codex-Preserved-Files-";
const MANAGER_TEMP_WRITE_PREFIX: &str = ".codex-portable-write-";
const MAX_PORTABLE_MANIFEST_BYTES: u64 = 32 * 1024 * 1024;

const MANAGER_ROOT_FILES: [&str; 6] = [
    "appxmanifest.xml",
    "codex-portable-launcher.txt",
    "codex-portable-files.txt",
    "launchcodex.exe",
    "codex.exe",
    "chatgpt.exe",
];

// Fallback allowlist for uninstalling pre-manifest (legacy) portable installs
// when neither `codex-portable-files.txt` nor `AppxBlockMap.xml` exists.
// Deliberately conservative: any unrecognized file or folder is preserved.
const LEGACY_OFFICIAL_APP_FILES: [&str; 21] = [
    "chatgpt.exe",
    "codex.exe",
    "chrome_100_percent.pak",
    "chrome_200_percent.pak",
    "d3dcompiler_47.dll",
    "dxcompiler.dll",
    "dxil.dll",
    "ffmpeg.dll",
    "icudtl.dat",
    "libegl.dll",
    "libglesv2.dll",
    "license",
    "licenses.chromium.html",
    "resources.pak",
    "resources.pri",
    "snapshot_blob.bin",
    "v8_context_snapshot.bin",
    "version",
    "vk_swiftshader.dll",
    "vk_swiftshader_icd.json",
    "vulkan-1.dll",
];

const LEGACY_OFFICIAL_APP_DIRS: [&str; 3] = ["locales", "resources", "swiftshader"];

#[derive(Debug, Clone, Default)]
pub struct PortableBackupCleanupOutcome {
    /// Set if the rollback backup directory was retained on disk (e.g. scan
    /// failed, any user item failed to move, or removing the backup failed).
    pub retained_backup: Option<PathBuf>,
    /// Sibling directory created under `install_parent` when `app/`-nested or
    /// name-colliding root user items were moved out of `backup`.
    pub preserved_dir: Option<PathBuf>,
    /// Relative paths of root-level user items moved back into `install_root`.
    pub restored_in_place: Vec<PathBuf>,
    /// Relative paths of user items moved into `preserved_dir`.
    pub moved_to_preserved_dir: Vec<PathBuf>,
    /// Human-readable preservation or retention notes.
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortableInstallReport {
    pub success: bool,
    pub install_root: String,
    pub executable_path: Option<String>,
    pub version: String,
    pub backup_path: Option<String>,
    pub shortcut_created: bool,
    pub uninstall_entry_created: bool,
    pub relaunched: bool,
    pub message: String,
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PortableUninstallReport {
    pub success: bool,
    #[serde(default)]
    pub partial: bool,
    pub install_root: String,
    pub removed_files: bool,
    pub removed_shortcut: bool,
    pub removed_uninstall_entry: bool,
    pub purged_user_data: bool,
    pub message: String,
    pub notes: Vec<String>,
}

struct PreparedPortable {
    payload_dir: PathBuf,
    identity: MsixIdentity,
}

#[derive(Debug, Clone)]
struct BlockMapEntry {
    logical_name: String,
    output_path: PathBuf,
    size: u64,
}

#[derive(Debug)]
struct BlockMapPaths {
    by_logical_name: HashMap<String, BlockMapEntry>,
    remaining: HashSet<String>,
}

fn io_err(context: &str, err: impl ToString) -> EngineError {
    EngineError::Io(format!("{context}: {}", err.to_string()))
}

// Directory replacement can briefly race process teardown, Windows Defender,
// or another file scanner even after every managed process has exited. Retry
// only the Windows errors that describe transient handle/lock contention; all
// other failures still return immediately.
const WINDOWS_FS_RETRY_DELAYS_MS: [u64; 8] = [50, 100, 200, 400, 800, 1_600, 2_500, 5_000];

fn is_transient_windows_fs_error(err: &io::Error) -> bool {
    // ERROR_ACCESS_DENIED, ERROR_SHARING_VIOLATION, ERROR_LOCK_VIOLATION.
    #[cfg(any(windows, test))]
    {
        matches!(err.raw_os_error(), Some(5 | 32 | 33))
    }
    #[cfg(not(any(windows, test)))]
    {
        let _ = err;
        false
    }
}

fn filesystem_operation_with_retry<F, S>(
    operation: &str,
    source: &Path,
    destination: Option<&Path>,
    mut action: F,
    mut sleep: S,
) -> io::Result<()>
where
    F: FnMut() -> io::Result<()>,
    S: FnMut(Duration),
{
    let destination_text = destination
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "none".to_string());
    let mut attempt = 1usize;
    loop {
        match action() {
            Ok(()) => {
                if attempt > 1 {
                    log::info!(
                        "portable filesystem operation succeeded after retry operation={operation} attempts={attempt} source={} destination={destination_text}",
                        source.display()
                    );
                }
                return Ok(());
            }
            Err(err) => {
                let retryable = is_transient_windows_fs_error(&err);
                let next_delay = WINDOWS_FS_RETRY_DELAYS_MS.get(attempt - 1).copied();
                let retry_delay = if retryable { next_delay } else { None };
                if let Some(delay_ms) = retry_delay {
                    log::warn!(
                        "portable filesystem operation temporarily blocked operation={operation} attempt={attempt} raw_os_error={:?} source_exists={} destination_exists={} source={} destination={destination_text} error={err}; retrying_in_ms={delay_ms}",
                        err.raw_os_error(),
                        source.exists(),
                        destination.is_some_and(Path::exists),
                        source.display()
                    );
                    sleep(Duration::from_millis(delay_ms));
                    attempt += 1;
                    continue;
                }

                log::error!(
                    "portable filesystem operation failed operation={operation} attempts={attempt} retryable={retryable} raw_os_error={:?} source_exists={} destination_exists={} source={} destination={destination_text} error={err}",
                    err.raw_os_error(),
                    source.exists(),
                    destination.is_some_and(Path::exists),
                    source.display()
                );
                return Err(err);
            }
        }
    }
}

fn rename_with_retry<F, S>(
    operation: &str,
    from: &Path,
    to: &Path,
    rename: F,
    sleep: S,
) -> io::Result<()>
where
    F: FnMut() -> io::Result<()>,
    S: FnMut(Duration),
{
    filesystem_operation_with_retry(operation, from, Some(to), rename, sleep)
}

/// Rename a portable-install directory, retrying only transient Windows lock
/// errors with the same bounded backoff used by the real install swap.
pub fn rename_directory_with_retry(operation: &str, from: &Path, to: &Path) -> io::Result<()> {
    rename_with_retry(operation, from, to, || fs::rename(from, to), thread::sleep)
}

/// Remove a portable-install directory with bounded retries for transient
/// Windows scanner/handle contention.
pub fn remove_directory_all_with_retry(operation: &str, path: &Path) -> io::Result<()> {
    filesystem_operation_with_retry(
        operation,
        path,
        None,
        || fs::remove_dir_all(path),
        thread::sleep,
    )
}

fn rename_portable_dir(operation: &str, from: &Path, to: &Path) -> Result<(), EngineError> {
    rename_directory_with_retry(operation, from, to)
        .map_err(|err| portable_rename_error(operation, from, to, &err))
}

fn remove_portable_dir(operation: &str, path: &Path) -> Result<(), EngineError> {
    remove_directory_all_with_retry(operation, path)
        .map_err(|err| portable_remove_error(operation, path, &err))
}

fn portable_remove_error(operation: &str, path: &Path, err: &io::Error) -> EngineError {
    if is_transient_windows_fs_error(err) {
        io_err(
            "portable install directory unavailable",
            format!("{operation}; source=\"{}\"; {err}", path.display()),
        )
    } else {
        io_err(operation, err)
    }
}

fn portable_rename_error(operation: &str, from: &Path, to: &Path, err: &io::Error) -> EngineError {
    if is_transient_windows_fs_error(err) {
        // Error 5 can mean either a handle held by another process or directory
        // permissions. Preserve both paths without claiming a definite cause.
        io_err(
            "portable install directory unavailable",
            format!(
                "{operation}; source=\"{}\"; destination=\"{}\"; {err}",
                from.display(),
                to.display()
            ),
        )
    } else {
        io_err(operation, err)
    }
}

fn copy_dir_all(from: &Path, to: &Path) -> Result<(), EngineError> {
    fs::create_dir_all(to).map_err(|e| io_err("create dir", e))?;
    for entry in fs::read_dir(from).map_err(|e| io_err("read dir", e))? {
        let entry = entry.map_err(|e| io_err("read dir entry", e))?;
        let ty = entry.file_type().map_err(|e| io_err("read file type", e))?;
        let dest = to.join(entry.file_name());
        if ty.is_dir() {
            copy_dir_all(&entry.path(), &dest)?;
        } else if ty.is_file() {
            fs::copy(entry.path(), &dest).map_err(|e| io_err("copy file", e))?;
        }
    }
    Ok(())
}

const MAX_BLOCK_MAP_BYTES: u64 = 32 * 1024 * 1024;

fn is_windows_reserved_component(component: &str) -> bool {
    let stem = component
        .split('.')
        .next()
        .unwrap_or(component)
        .to_ascii_uppercase();
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || stem.strip_prefix("COM").is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
        || stem.strip_prefix("LPT").is_some_and(|suffix| {
            matches!(
                suffix,
                "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
            )
        })
}

fn validate_windows_path_component(component: &str, source: &str) -> Result<(), EngineError> {
    if component.is_empty() || component == "." || component == ".." {
        return Err(EngineError::Msix(format!(
            "unsafe MSIX path component {component:?} in {source}"
        )));
    }
    if component.ends_with([' ', '.']) {
        return Err(EngineError::Msix(format!(
            "MSIX path component has a Windows-unsafe trailing space/dot in {source}: {component:?}"
        )));
    }
    if component.chars().any(|ch| {
        ch == '\0'
            || ch.is_control()
            || matches!(ch, '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*')
    }) {
        return Err(EngineError::Msix(format!(
            "MSIX path component contains a Windows-unsafe character in {source}: {component:?}"
        )));
    }
    if is_windows_reserved_component(component) {
        return Err(EngineError::Msix(format!(
            "MSIX path uses a reserved Windows device name in {source}: {component:?}"
        )));
    }
    Ok(())
}

fn path_from_components(components: &[String]) -> PathBuf {
    components.iter().fold(PathBuf::new(), |mut path, part| {
        path.push(part);
        path
    })
}

fn windows_path_key(components: &[String]) -> String {
    components
        .iter()
        .map(|component| component.to_lowercase())
        .collect::<Vec<_>>()
        .join("\\")
}

fn parse_logical_msix_path(name: &str) -> Result<Vec<String>, EngineError> {
    if name.is_empty() || name.starts_with(['\\', '/']) || name.contains('/') {
        return Err(EngineError::Msix(format!(
            "invalid AppxBlockMap logical path: {name:?}"
        )));
    }
    let components = name.split('\\').map(str::to_string).collect::<Vec<_>>();
    for component in &components {
        validate_windows_path_component(component, name)?;
    }
    Ok(components)
}

fn parse_appx_block_map(xml: &str) -> Result<BlockMapPaths, EngineError> {
    let document = roxmltree::Document::parse(xml)
        .map_err(|err| EngineError::Msix(format!("AppxBlockMap.xml: {err}")))?;
    let mut by_logical_name = HashMap::new();
    let mut windows_names = HashMap::<String, String>::new();

    for file in document
        .descendants()
        .filter(|node| node.has_tag_name("File"))
    {
        let logical_name = file
            .attribute("Name")
            .ok_or_else(|| EngineError::Msix("AppxBlockMap File missing Name".to_string()))?
            .to_string();
        let size = file
            .attribute("Size")
            .ok_or_else(|| {
                EngineError::Msix(format!("AppxBlockMap File missing Size: {logical_name}"))
            })?
            .parse::<u64>()
            .map_err(|err| {
                EngineError::Msix(format!(
                    "AppxBlockMap File has invalid Size for {logical_name}: {err}"
                ))
            })?;
        let components = parse_logical_msix_path(&logical_name)?;
        let windows_key = windows_path_key(&components);
        if let Some(previous) = windows_names.insert(windows_key, logical_name.clone()) {
            return Err(EngineError::Msix(format!(
                "AppxBlockMap paths collide on Windows: {previous:?} and {logical_name:?}"
            )));
        }
        let canonical_name = components.join("\\");
        let entry = BlockMapEntry {
            logical_name: logical_name.clone(),
            output_path: path_from_components(&components),
            size,
        };
        if by_logical_name.insert(canonical_name, entry).is_some() {
            return Err(EngineError::Msix(format!(
                "duplicate AppxBlockMap path: {logical_name}"
            )));
        }
    }

    if by_logical_name.is_empty() {
        return Err(EngineError::Msix(
            "AppxBlockMap.xml contains no payload files".to_string(),
        ));
    }
    let remaining = by_logical_name.keys().cloned().collect();
    Ok(BlockMapPaths {
        by_logical_name,
        remaining,
    })
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

fn decode_zip_component(component: &str, source: &str) -> Result<(String, bool), EngineError> {
    let bytes = component.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut changed = false;
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'%' {
            decoded.push(bytes[index]);
            index += 1;
            continue;
        }
        let high = bytes.get(index + 1).and_then(|byte| hex_value(*byte));
        let low = bytes.get(index + 2).and_then(|byte| hex_value(*byte));
        let (Some(high), Some(low)) = (high, low) else {
            return Err(EngineError::Msix(format!(
                "invalid percent escape in MSIX ZIP path: {source:?}"
            )));
        };
        let value = (high << 4) | low;
        if matches!(value, 0 | b'/' | b'\\') {
            return Err(EngineError::Msix(format!(
                "encoded path separator/NUL is forbidden in MSIX ZIP path: {source:?}"
            )));
        }
        decoded.push(value);
        changed = true;
        index += 3;
    }
    let decoded = String::from_utf8(decoded).map_err(|err| {
        EngineError::Msix(format!(
            "percent-decoded MSIX ZIP path is not UTF-8 ({source:?}): {err}"
        ))
    })?;
    validate_windows_path_component(&decoded, source)?;
    Ok((decoded, changed))
}

fn decode_zip_entry_path(name: &str) -> Result<(Vec<String>, bool), EngineError> {
    if name.is_empty() || name.starts_with(['/', '\\']) || name.contains('\\') {
        return Err(EngineError::Msix(format!(
            "invalid MSIX ZIP entry path: {name:?}"
        )));
    }
    let name = name.strip_suffix('/').unwrap_or(name);
    let mut changed = false;
    let mut components = Vec::new();
    for component in name.split('/') {
        let (decoded, component_changed) = decode_zip_component(component, name)?;
        changed |= component_changed;
        components.push(decoded);
    }
    Ok((components, changed))
}

fn read_appx_block_map(
    zip: &mut zip::ZipArchive<fs::File>,
) -> Result<Option<BlockMapPaths>, EngineError> {
    let mut block_map = match zip.by_name("AppxBlockMap.xml") {
        Ok(file) => file,
        Err(zip::result::ZipError::FileNotFound) => return Ok(None),
        Err(err) => return Err(EngineError::Msix(format!("read AppxBlockMap.xml: {err}"))),
    };
    if block_map.size() > MAX_BLOCK_MAP_BYTES {
        return Err(EngineError::Msix(format!(
            "AppxBlockMap.xml is unexpectedly large: {} bytes",
            block_map.size()
        )));
    }
    let mut xml = String::new();
    block_map
        .read_to_string(&mut xml)
        .map_err(|err| EngineError::Msix(format!("decode AppxBlockMap.xml: {err}")))?;
    parse_appx_block_map(&xml).map(Some)
}

fn extract_msix(msix_path: &Path, dest: &Path) -> Result<String, EngineError> {
    let file = fs::File::open(msix_path).map_err(|e| io_err("open MSIX", e))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| EngineError::Msix(format!("open zip: {e}")))?;
    let mut block_map = read_appx_block_map(&mut zip)?;
    let mut manifest_xml = None;
    let mut written_paths = HashMap::<String, String>::new();
    let mut logical_path_remaps = 0usize;

    for idx in 0..zip.len() {
        let mut file = zip
            .by_index(idx)
            .map_err(|e| EngineError::Msix(format!("read zip entry {idx}: {e}")))?;
        let zip_name = codex_archive::entry_name(&file)
            .map_err(|e| EngineError::Msix(format!("decode zip entry {idx} name: {e}")))?
            .into_owned();
        let (decoded_components, was_percent_encoded) = decode_zip_entry_path(&zip_name)?;
        if file.is_dir() {
            continue;
        }

        let decoded_name = decoded_components.join("\\");
        let output_path = if let Some(paths) = block_map.as_mut() {
            if let Some(entry) = paths.by_logical_name.get(&decoded_name) {
                if file.size() != entry.size {
                    return Err(EngineError::Msix(format!(
                        "MSIX payload size disagrees with AppxBlockMap for {}: zip={} blockMap={}",
                        entry.logical_name,
                        file.size(),
                        entry.size
                    )));
                }
                paths.remaining.remove(&decoded_name);
                if was_percent_encoded {
                    logical_path_remaps += 1;
                }
                entry.output_path.clone()
            } else {
                if was_percent_encoded {
                    return Err(EngineError::Msix(format!(
                        "percent-encoded MSIX ZIP entry is not described by AppxBlockMap.xml: {zip_name}"
                    )));
                }
                path_from_components(&decoded_components)
            }
        } else {
            if was_percent_encoded {
                return Err(EngineError::Msix(format!(
                    "cannot safely map percent-encoded MSIX ZIP entry without AppxBlockMap.xml: {zip_name}"
                )));
            }
            path_from_components(&decoded_components)
        };
        let output_components = output_path
            .iter()
            .map(|component| component.to_string_lossy().into_owned())
            .collect::<Vec<_>>();
        let output_key = windows_path_key(&output_components);
        if let Some(previous) = written_paths.insert(output_key, zip_name.clone()) {
            return Err(EngineError::Msix(format!(
                "MSIX entries collide on Windows after logical path mapping: {previous:?} and {zip_name:?}"
            )));
        }

        let out_path = dest.join(&output_path);
        if !out_path.starts_with(dest) {
            return Err(EngineError::Msix(format!(
                "MSIX logical path escaped extraction root: {}",
                output_path.display()
            )));
        }
        if let Some(parent) = out_path.parent() {
            fs::create_dir_all(parent).map_err(|e| io_err("create extracted parent", e))?;
        }
        let mut out =
            fs::File::create(&out_path).map_err(|e| io_err("create extracted file", e))?;
        std::io::copy(&mut file, &mut out).map_err(|e| io_err("extract file", e))?;

        if output_path
            .file_name()
            .and_then(|s| s.to_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("AppxManifest.xml"))
            && output_path.components().count() == 1
        {
            let mut xml = String::new();
            fs::File::open(&out_path)
                .and_then(|mut f| f.read_to_string(&mut xml))
                .map_err(|e| io_err("read extracted AppxManifest.xml", e))?;
            manifest_xml = Some(xml);
        }
    }

    if let Some(paths) = block_map {
        if !paths.remaining.is_empty() {
            let mut missing = paths.remaining.into_iter().collect::<Vec<_>>();
            missing.sort();
            let preview = missing
                .iter()
                .take(3)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ");
            return Err(EngineError::Msix(format!(
                "MSIX is missing {} payload file(s) declared by AppxBlockMap.xml: {preview}",
                missing.len()
            )));
        }
        log::info!(
            "portable MSIX extraction used AppxBlockMap logical paths remapped_entries={logical_path_remaps}"
        );
    }

    manifest_xml.ok_or_else(|| EngineError::Msix("MSIX missing AppxManifest.xml".to_string()))
}

/// Entry-executable basenames the Codex lineage has shipped, newest first.
/// Post-merge packages keep a legacy `Codex.exe` next to the real entrypoint,
/// so `ChatGPT.exe` must win when the manifest can't tell us (it normally can).
const APP_EXE_CANDIDATES: [&str; 2] = ["ChatGPT.exe", "Codex.exe"];

fn is_manager_launcher(path: &Path) -> bool {
    // Manager launchers are small; don't read a large official Electron binary
    // just to exclude aliases from a damaged portable root.
    let Ok(metadata) = fs::metadata(path) else {
        return false;
    };
    if metadata.len() > 4 * 1024 * 1024 {
        return false;
    }
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    if bytes.get(..2) != Some(b"MZ") {
        return false;
    }
    let u16_at = |offset: usize| {
        bytes
            .get(offset..offset.checked_add(2)?)
            .map(|b| u16::from_le_bytes([b[0], b[1]]) as usize)
    };
    let u32_at = |offset: usize| {
        bytes
            .get(offset..offset.checked_add(4)?)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as usize)
    };
    let Some(pe) = u32_at(0x3c) else {
        return false;
    };
    if pe > bytes.len().saturating_sub(24) || bytes.get(pe..pe + 4) != Some(b"PE\0\0") {
        return false;
    }
    let Some(count) = u16_at(pe + 6) else {
        return false;
    };
    let Some(optional_size) = u16_at(pe + 20) else {
        return false;
    };
    let table = pe + 24 + optional_size;
    for index in 0..count.min(128) {
        let offset = table + index * 40;
        if bytes.get(offset..offset + 8) != Some(b".camlnch") {
            continue;
        }
        let Some(size) = u32_at(offset + 16) else {
            return false;
        };
        let Some(start) = u32_at(offset + 20) else {
            return false;
        };
        return start
            .checked_add(size)
            .and_then(|end| bytes.get(start..end))
            .is_some_and(|section| section.starts_with(&crate::portable_command::LAUNCHER_MARKER));
    }
    false
}

fn is_installed_payload_exe(root: &Path, exe: &Path) -> bool {
    exe.is_file() && (exe.parent() != Some(root) || !is_manager_launcher(exe))
}

fn find_exe_named(root: &Path, name: &str) -> Result<Option<PathBuf>, EngineError> {
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).map_err(|e| io_err("walk extracted MSIX", e))? {
            let entry = entry.map_err(|e| io_err("walk extracted MSIX entry", e))?;
            let path = entry.path();
            let ty = entry.file_type().map_err(|e| io_err("read file type", e))?;
            if ty.is_dir() {
                stack.push(path);
            } else if path
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(|n| n.eq_ignore_ascii_case(name))
            {
                return Ok(Some(path));
            }
        }
    }
    Ok(None)
}

/// Locate the app's entry executable in an extracted MSIX.
///
/// The manifest's `Application@Executable` is authoritative: when declared it
/// is resolved as a package-root relative path, then by basename — and a
/// declared entry that cannot be found is a hard error, NOT a fallback case.
/// Falling back would silently select a non-entry binary (e.g. the legacy
/// `Codex.exe` shipped next to the real `ChatGPT.exe`) when the true entry is
/// missing or quarantined, and the install would then health-check the wrong
/// binary. The known-name candidates only serve manifests with no
/// `<Application>` declaration at all.
fn find_app_exe(root: &Path, manifest_xml: &str) -> Result<PathBuf, EngineError> {
    if let Some(declared) = crate::msix::parse_appx_application_executable(manifest_xml) {
        // Manifest paths use either separator; resolve component-wise. Only the
        // exact declared path counts — matching a same-named file elsewhere in
        // the package would select (and copy the parent directory of) a binary
        // that is not the entry.
        let relative: PathBuf = declared.replace('\\', "/").split('/').collect();
        let direct = root.join(&relative);
        if direct.is_file() {
            return Ok(direct);
        }
        return Err(EngineError::Msix(format!(
            "MSIX manifest declares entry executable '{declared}' but it is missing from the payload"
        )));
    }
    for name in APP_EXE_CANDIDATES {
        if let Some(found) = find_exe_named(root, name)? {
            return Ok(found);
        }
    }
    Err(EngineError::Msix(
        "MSIX did not contain an app entry executable (ChatGPT.exe / Codex.exe)".to_string(),
    ))
}

/// The entry executable of an installed portable root. Reads the payload's
/// `AppxManifest.xml` (written at install time) for the declared executable's
/// basename. New installations keep the official files in `app/`; older
/// installations flattened them into the root. Never select a root launcher
/// alias for a nested installation. A declared-but-missing entry returns `None`
/// (picking a leftover non-entry binary would mask a broken install). The known
/// entry names are probed only for roots without a declaring manifest.
pub fn installed_app_exe(install_root: &Path) -> Option<PathBuf> {
    let nested = install_root.join(crate::portable_command::APP_DIR_NAME);
    // The config also identifies a nested install whose app directory was
    // removed/quarantined. Its remaining root aliases are not app executables.
    let nested_target =
        fs::read_to_string(install_root.join(crate::portable_command::LAUNCH_TARGET_NAME))
            .ok()
            .and_then(|config| {
                crate::portable_command::parse_launch_config(&config)
                    .map(|(target, _)| target.replace('\\', "/").starts_with("app/"))
            });
    let app_root = if nested_target.unwrap_or_else(|| nested.is_dir()) {
        nested.as_path()
    } else {
        install_root
    };
    let manifest = install_root.join("AppxManifest.xml");
    if let Ok(xml) = fs::read_to_string(&manifest) {
        if let Some(declared) = crate::msix::parse_appx_application_executable(&xml) {
            let basename = declared.replace('\\', "/");
            let name = basename.rsplit('/').next()?;
            let exe = app_root.join(name);
            return is_installed_payload_exe(install_root, &exe).then_some(exe);
        }
    }
    APP_EXE_CANDIDATES
        .into_iter()
        .map(|name| app_root.join(name))
        .find(|exe| is_installed_payload_exe(install_root, exe))
}

fn prepare_portable_payload(
    msix_path: &Path,
    work_dir: &Path,
) -> Result<PreparedPortable, EngineError> {
    let extracted = work_dir.join("extracted");
    let payload = work_dir.join("payload");
    if extracted.exists() {
        fs::remove_dir_all(&extracted).map_err(|e| io_err("clear extracted dir", e))?;
    }
    if payload.exists() {
        fs::remove_dir_all(&payload).map_err(|e| io_err("clear payload dir", e))?;
    }
    fs::create_dir_all(&extracted).map_err(|e| io_err("create extracted dir", e))?;

    let manifest_xml = extract_msix(msix_path, &extracted)?;
    let identity = parse_appx_manifest_xml(&manifest_xml)?;
    let exe = find_app_exe(&extracted, &manifest_xml)?;
    let exe_dir = exe.parent().ok_or_else(|| {
        EngineError::Msix("app entry executable had no parent directory".to_string())
    })?;

    // Keep Electron's executable, DLLs and resources together, while the root
    // contains only Manager launchers. The entire tree is swapped atomically.
    copy_dir_all(
        exe_dir,
        &payload.join(crate::portable_command::APP_DIR_NAME),
    )?;
    fs::write(payload.join("AppxManifest.xml"), manifest_xml)
        .map_err(|e| io_err("write portable AppxManifest.xml", e))?;
    ensure_portable_launcher(&payload)?;
    write_portable_files_manifest(&payload)?;

    Ok(PreparedPortable {
        payload_dir: payload,
        identity,
    })
}

#[derive(Debug, Clone, Default)]
struct PortableRootScan {
    /// Relative paths under `root` belonging to non-official user files,
    /// directories, or symlinks/junctions. Unknown directories are recorded as
    /// a single entry without recursing into their contents.
    unknown_entries: Vec<PathBuf>,
    /// Regular official files safe to delete after user entries are preserved.
    official_files: Vec<PathBuf>,
    /// Official directories safe to remove via non-recursive `remove_dir` once empty.
    official_dirs: Vec<PathBuf>,
}

#[cfg(windows)]
fn is_symlink_or_reparse_point(meta: &fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0400;
    meta.file_type().is_symlink() || (meta.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT) != 0
}

#[cfg(not(windows))]
fn is_symlink_or_reparse_point(meta: &fs::Metadata) -> bool {
    meta.file_type().is_symlink()
}

fn is_manager_temp_write_file(name: &str) -> bool {
    name.to_ascii_lowercase()
        .starts_with(MANAGER_TEMP_WRITE_PREFIX)
}

fn is_manager_root_file_name(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    MANAGER_ROOT_FILES.contains(&lower.as_str()) || lower.starts_with(MANAGER_TEMP_WRITE_PREFIX)
}

fn normalize_rel_key(rel: &Path) -> String {
    rel.iter()
        .map(|part| part.to_string_lossy().to_lowercase())
        .collect::<Vec<_>>()
        .join("/")
}

fn display_rel_path(rel: &Path) -> String {
    rel.iter()
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn add_ancestor_dir_keys(rel_key: &str, dirs: &mut HashSet<String>) {
    let mut cur = rel_key;
    while let Some((parent, _)) = cur.rsplit_once('/') {
        if parent.is_empty() {
            break;
        }
        if !dirs.insert(parent.to_string()) {
            break;
        }
        cur = parent;
    }
}

fn write_portable_files_manifest(payload: &Path) -> Result<(), EngineError> {
    let mut files = Vec::new();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel_dir) = stack.pop() {
        let abs_dir = if rel_dir.as_os_str().is_empty() {
            payload.to_path_buf()
        } else {
            payload.join(&rel_dir)
        };
        for entry in fs::read_dir(&abs_dir).map_err(|e| io_err("scan portable payload", e))? {
            let entry = entry.map_err(|e| io_err("read portable payload entry", e))?;
            let path = entry.path();
            let meta = fs::symlink_metadata(&path)
                .map_err(|e| io_err("stat portable payload entry", e))?;
            if is_symlink_or_reparse_point(&meta) {
                continue;
            }
            let rel = rel_dir.join(entry.file_name());
            if meta.is_dir() {
                stack.push(rel);
            } else if meta.is_file() {
                files.push(display_rel_path(&rel));
            }
        }
    }
    files.push(PORTABLE_FILES_MANIFEST_NAME.to_string());
    files.sort();
    files.dedup();
    let mut body = files.join("\n");
    body.push('\n');
    fs::write(payload.join(PORTABLE_FILES_MANIFEST_NAME), body)
        .map_err(|e| io_err("write portable files manifest", e))
}

fn validate_manifest_rel_line(line: &str) -> Result<String, EngineError> {
    if line.is_empty() || line.starts_with(['/', '\\']) || line.contains(':') {
        return Err(EngineError::Install(format!(
            "invalid portable manifest entry: {line:?}"
        )));
    }
    let normalized = line.replace('\\', "/");
    let mut parts = Vec::new();
    for segment in normalized.split('/') {
        if segment.is_empty()
            || segment == "."
            || segment == ".."
            || segment.chars().any(|ch| ch == '\0' || ch.is_control())
        {
            return Err(EngineError::Install(format!(
                "unsafe portable manifest entry: {line:?}"
            )));
        }
        parts.push(segment.to_lowercase());
    }
    if parts.is_empty() {
        return Err(EngineError::Install(
            "empty portable manifest entry".to_string(),
        ));
    }
    Ok(parts.join("/"))
}

fn read_portable_files_manifest(root: &Path) -> Result<Option<HashSet<String>>, EngineError> {
    let path = root.join(PORTABLE_FILES_MANIFEST_NAME);
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(io_err("inspect portable files manifest", err)),
    };
    if is_symlink_or_reparse_point(&meta) || !meta.is_file() {
        return Err(EngineError::Install(format!(
            "portable files manifest is not a regular file: {}",
            path.display()
        )));
    }
    if meta.len() > MAX_PORTABLE_MANIFEST_BYTES {
        return Err(EngineError::Install(format!(
            "portable files manifest is unexpectedly large: {} bytes",
            meta.len()
        )));
    }
    let content =
        fs::read_to_string(&path).map_err(|e| io_err("read portable files manifest", e))?;
    let mut keys = HashSet::new();
    for raw_line in content.lines() {
        let line = raw_line.trim();
        if line.is_empty() {
            continue;
        }
        keys.insert(validate_manifest_rel_line(line)?);
    }
    if keys.is_empty() {
        return Err(EngineError::Install(
            "portable files manifest is empty".to_string(),
        ));
    }
    Ok(Some(keys))
}

fn read_root_block_map_keys(root: &Path) -> Result<Option<HashSet<String>>, EngineError> {
    let path = root.join("AppxBlockMap.xml");
    let meta = match fs::symlink_metadata(&path) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(io_err("inspect AppxBlockMap.xml", err)),
    };
    if is_symlink_or_reparse_point(&meta) || !meta.is_file() {
        return Err(EngineError::Install(format!(
            "AppxBlockMap.xml is not a regular file: {}",
            path.display()
        )));
    }
    if meta.len() > MAX_BLOCK_MAP_BYTES {
        return Err(EngineError::Install(format!(
            "AppxBlockMap.xml is unexpectedly large: {} bytes",
            meta.len()
        )));
    }
    let xml = fs::read_to_string(&path).map_err(|e| io_err("read AppxBlockMap.xml", e))?;
    let parsed = parse_appx_block_map(&xml)?;
    let mut keys = HashSet::new();
    for entry in parsed.by_logical_name.values() {
        keys.insert(normalize_rel_key(&entry.output_path));
    }
    for meta_file in [
        "appxblockmap.xml",
        "appxmanifest.xml",
        "appxsignature.p7x",
        "[content_types].xml",
        "appxmetadata/codeintegrity.cat",
    ] {
        keys.insert(meta_file.to_string());
    }
    Ok(Some(keys))
}

fn declared_manifest_exe_basename(root: &Path) -> Option<String> {
    let manifest = root.join("AppxManifest.xml");
    let meta = fs::symlink_metadata(&manifest).ok()?;
    if is_symlink_or_reparse_point(&meta) || !meta.is_file() {
        return None;
    }
    let xml = fs::read_to_string(&manifest).ok()?;
    let declared = crate::msix::parse_appx_application_executable(&xml)?;
    let normalized = declared.replace('\\', "/");
    normalized
        .rsplit('/')
        .next()
        .filter(|name| !name.is_empty())
        .map(|name| name.to_lowercase())
}

fn is_legacy_nested_root(root: &Path) -> bool {
    let app_dir = root.join(crate::portable_command::APP_DIR_NAME);
    let app_is_real_dir = fs::symlink_metadata(&app_dir)
        .is_ok_and(|meta| meta.is_dir() && !is_symlink_or_reparse_point(&meta));
    if !app_is_real_dir {
        return false;
    }
    let launch_target = root.join(crate::portable_command::LAUNCH_TARGET_NAME);
    if fs::symlink_metadata(&launch_target)
        .is_ok_and(|meta| meta.is_file() && !is_symlink_or_reparse_point(&meta))
    {
        if let Ok(config) = fs::read_to_string(&launch_target) {
            if let Some((target, _)) = crate::portable_command::parse_launch_config(&config) {
                return target.replace('\\', "/").starts_with("app/");
            }
        }
    }
    // Without a valid launch config, only treat `app/` as the official nested
    // payload directory if it contains a real Codex entrypoint or app.asar,
    // so a user project folder literally named `app/` in a flat install is
    // never mistaken for the official app directory.
    let declared = declared_manifest_exe_basename(root);
    app_dir.join("ChatGPT.exe").is_file()
        || app_dir.join("Codex.exe").is_file()
        || declared.is_some_and(|name| app_dir.join(name).is_file())
        || app_dir.join("resources").join("app.asar").is_file()
}

fn scan_with_exact_manifest(
    root: &Path,
    mut official_file_keys: HashSet<String>,
) -> Result<PortableRootScan, EngineError> {
    for fixed in MANAGER_ROOT_FILES {
        official_file_keys.insert(fixed.to_string());
    }
    let mut official_dir_keys = HashSet::new();
    for file_key in &official_file_keys {
        add_ancestor_dir_keys(file_key, &mut official_dir_keys);
    }

    let mut scan = PortableRootScan::default();
    let mut stack = vec![PathBuf::new()];
    while let Some(rel_dir) = stack.pop() {
        let abs_dir = if rel_dir.as_os_str().is_empty() {
            root.to_path_buf()
        } else {
            root.join(&rel_dir)
        };
        for entry in fs::read_dir(&abs_dir).map_err(|e| io_err("scan portable directory", e))? {
            let entry = entry.map_err(|e| io_err("read portable directory entry", e))?;
            let path = entry.path();
            let meta = fs::symlink_metadata(&path)
                .map_err(|e| io_err("inspect portable directory entry", e))?;
            let rel = rel_dir.join(entry.file_name());
            if is_symlink_or_reparse_point(&meta) {
                scan.unknown_entries.push(rel);
                continue;
            }
            let key = normalize_rel_key(&rel);
            if meta.is_dir() {
                if official_dir_keys.contains(&key) {
                    scan.official_dirs.push(rel.clone());
                    stack.push(rel);
                } else {
                    scan.unknown_entries.push(rel);
                }
            } else if meta.is_file() {
                let is_root_temp = rel_dir.as_os_str().is_empty()
                    && entry
                        .file_name()
                        .to_str()
                        .is_some_and(is_manager_temp_write_file);
                if official_file_keys.contains(&key) || is_root_temp {
                    scan.official_files.push(rel);
                } else {
                    scan.unknown_entries.push(rel);
                }
            } else {
                scan.unknown_entries.push(rel);
            }
        }
    }
    scan.unknown_entries.sort();
    Ok(scan)
}

/// Walk a legacy official subdirectory (such as `resources/` or `locales/`),
/// recording regular files and real subdirectories as official while still
/// treating any symlink/junction encountered inside as unknown (never followed).
fn scan_legacy_official_subtree(
    root: &Path,
    subtree_rel: PathBuf,
    scan: &mut PortableRootScan,
) -> Result<(), EngineError> {
    scan.official_dirs.push(subtree_rel.clone());
    let mut stack = vec![subtree_rel];
    while let Some(rel_dir) = stack.pop() {
        let abs_dir = root.join(&rel_dir);
        for entry in
            fs::read_dir(&abs_dir).map_err(|e| io_err("scan legacy official subtree", e))?
        {
            let entry = entry.map_err(|e| io_err("read legacy official subtree entry", e))?;
            let path = entry.path();
            let meta = fs::symlink_metadata(&path)
                .map_err(|e| io_err("inspect legacy official subtree entry", e))?;
            let rel = rel_dir.join(entry.file_name());
            if is_symlink_or_reparse_point(&meta) {
                scan.unknown_entries.push(rel);
            } else if meta.is_dir() {
                scan.official_dirs.push(rel.clone());
                stack.push(rel);
            } else if meta.is_file() {
                scan.official_files.push(rel);
            } else {
                scan.unknown_entries.push(rel);
            }
        }
    }
    Ok(())
}

fn collect_reference_app_top_level(
    reference_install_root: &Path,
) -> Result<(HashSet<String>, HashSet<String>), EngineError> {
    let app_dir = reference_install_root.join(crate::portable_command::APP_DIR_NAME);
    let mut files = HashSet::new();
    let mut dirs = HashSet::new();
    for entry in fs::read_dir(&app_dir).map_err(|e| io_err("read reference payload app dir", e))? {
        let entry = entry.map_err(|e| io_err("read reference payload app entry", e))?;
        let meta = fs::symlink_metadata(entry.path())
            .map_err(|e| io_err("inspect reference payload app entry", e))?;
        if is_symlink_or_reparse_point(&meta) {
            continue;
        }
        let lower = entry.file_name().to_string_lossy().to_lowercase();
        if meta.is_dir() {
            dirs.insert(lower);
        } else if meta.is_file() {
            files.insert(lower);
        }
    }
    for exe in APP_EXE_CANDIDATES {
        files.insert(exe.to_lowercase());
    }
    Ok((files, dirs))
}

fn scan_portable_root(
    root: &Path,
    reference_payload: Option<&Path>,
) -> Result<PortableRootScan, EngineError> {
    if let Some(manifest_keys) = read_portable_files_manifest(root)? {
        return scan_with_exact_manifest(root, manifest_keys);
    }
    if let Some(block_map_keys) = read_root_block_map_keys(root)? {
        return scan_with_exact_manifest(root, block_map_keys);
    }

    // Legacy portable installation (no codex-portable-files.txt / AppxBlockMap.xml).
    let (mut app_files, app_dirs) = match reference_payload {
        Some(reference) => collect_reference_app_top_level(reference)?,
        None => (
            LEGACY_OFFICIAL_APP_FILES
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
            LEGACY_OFFICIAL_APP_DIRS
                .iter()
                .map(|name| (*name).to_string())
                .collect(),
        ),
    };
    if let Some(declared) = declared_manifest_exe_basename(root) {
        app_files.insert(declared);
    }

    let nested = is_legacy_nested_root(root);
    let mut scan = PortableRootScan::default();
    for entry in fs::read_dir(root).map_err(|e| io_err("scan legacy portable root", e))? {
        let entry = entry.map_err(|e| io_err("read legacy portable root entry", e))?;
        let path = entry.path();
        let meta = fs::symlink_metadata(&path)
            .map_err(|e| io_err("inspect legacy portable root entry", e))?;
        let rel = PathBuf::from(entry.file_name());
        if is_symlink_or_reparse_point(&meta) {
            scan.unknown_entries.push(rel);
            continue;
        }
        let lower = entry.file_name().to_string_lossy().to_lowercase();
        if nested {
            if meta.is_dir() && lower == crate::portable_command::APP_DIR_NAME {
                scan.official_dirs.push(rel.clone());
                for app_entry in
                    fs::read_dir(&path).map_err(|e| io_err("scan legacy portable app dir", e))?
                {
                    let app_entry =
                        app_entry.map_err(|e| io_err("read legacy portable app entry", e))?;
                    let app_path = app_entry.path();
                    let app_meta = fs::symlink_metadata(&app_path)
                        .map_err(|e| io_err("inspect legacy portable app entry", e))?;
                    let app_rel = rel.join(app_entry.file_name());
                    if is_symlink_or_reparse_point(&app_meta) {
                        scan.unknown_entries.push(app_rel);
                        continue;
                    }
                    let app_lower = app_entry.file_name().to_string_lossy().to_lowercase();
                    if app_meta.is_dir() {
                        if app_dirs.contains(&app_lower) {
                            scan_legacy_official_subtree(root, app_rel, &mut scan)?;
                        } else {
                            scan.unknown_entries.push(app_rel);
                        }
                    } else if app_meta.is_file() && app_files.contains(&app_lower) {
                        scan.official_files.push(app_rel);
                    } else {
                        scan.unknown_entries.push(app_rel);
                    }
                }
            } else if meta.is_file() && is_manager_root_file_name(&lower) {
                scan.official_files.push(rel);
            } else {
                scan.unknown_entries.push(rel);
            }
        } else if meta.is_dir() {
            if app_dirs.contains(&lower) {
                scan_legacy_official_subtree(root, rel, &mut scan)?;
            } else {
                scan.unknown_entries.push(rel);
            }
        } else if meta.is_file() && (is_manager_root_file_name(&lower) || app_files.contains(&lower))
        {
            scan.official_files.push(rel);
        } else {
            scan.unknown_entries.push(rel);
        }
    }

    scan.unknown_entries.sort();
    Ok(scan)
}

fn format_utc_timestamp(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let hour = rem / 3_600;
    let minute = (rem % 3_600) / 60;
    let second = rem % 60;
    // Howard Hinnant's civil_from_days algorithm (days since 1970-01-01).
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = (yoe as i64) + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = y + i64::from(month <= 2);
    format!("{year:04}{month:02}{day:02}-{hour:02}{minute:02}{second:02}")
}

fn create_unique_preserved_files_dir(install_parent: &Path) -> io::Result<PathBuf> {
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let stamp = format_utc_timestamp(now_secs);
    let primary = install_parent.join(format!("{PRESERVED_FILES_DIR_PREFIX}{stamp}"));
    match fs::create_dir(&primary) {
        Ok(()) => return Ok(primary),
        Err(err) if err.kind() == io::ErrorKind::AlreadyExists => {}
        Err(err) => return Err(err),
    }
    for _ in 0..16 {
        let suffix = &uuid::Uuid::new_v4().simple().to_string()[..8];
        let candidate = install_parent.join(format!("{PRESERVED_FILES_DIR_PREFIX}{stamp}-{suffix}"));
        match fs::create_dir(&candidate) {
            Ok(()) => return Ok(candidate),
            Err(err) if err.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(err),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "could not allocate unique preserved-files directory",
    ))
}

fn target_collides_in_dir(parent: &Path, name: &std::ffi::OsStr) -> Result<bool, io::Error> {
    let direct = parent.join(name);
    match fs::symlink_metadata(&direct) {
        Ok(_) => return Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    let needle = name.to_string_lossy().to_lowercase();
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        if entry.file_name().to_string_lossy().to_lowercase() == needle {
            return Ok(true);
        }
    }
    Ok(false)
}

fn summarize_rel_entries(entries: &[PathBuf]) -> String {
    const MAX_PREVIEW: usize = 5;
    let preview = entries
        .iter()
        .take(MAX_PREVIEW)
        .map(|p| display_rel_path(p))
        .collect::<Vec<_>>()
        .join(", ");
    if entries.len() > MAX_PREVIEW {
        format!("{preview} (+{} more)", entries.len() - MAX_PREVIEW)
    } else {
        preview
    }
}

fn is_dir_not_empty_error(dir: &Path, err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::DirectoryNotEmpty
        || fs::read_dir(dir)
            .ok()
            .and_then(|mut entries| entries.next())
            .is_some()
}

fn remove_file_with_retry(operation: &str, path: &Path) -> io::Result<()> {
    filesystem_operation_with_retry(operation, path, None, || fs::remove_file(path), thread::sleep)
}

fn remove_empty_dir_with_retry(operation: &str, path: &Path) -> io::Result<()> {
    filesystem_operation_with_retry(operation, path, None, || fs::remove_dir(path), thread::sleep)
}

/// Delete only verified official regular files and empty official directories
/// from `root`, never calling `remove_dir_all` and never following symlinks or
/// NTFS junctions. Returns `Ok(true)` if `root` itself became empty and was
/// removed, or `Ok(false)` if non-official entries remain in `root`.
fn remove_verified_official_entries(
    root: &Path,
    scan: &PortableRootScan,
    allow_non_empty_dirs: bool,
) -> Result<bool, EngineError> {
    for rel in &scan.official_files {
        let path = root.join(rel);
        match fs::symlink_metadata(&path) {
            Ok(meta) => {
                if is_symlink_or_reparse_point(&meta) || !meta.is_file() {
                    return Err(EngineError::Install(format!(
                        "refusing to delete unexpected non-regular entry: {}",
                        path.display()
                    )));
                }
                remove_file_with_retry("remove portable official file", &path)
                    .map_err(|e| io_err("remove portable official file", e))?;
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(io_err("inspect portable official file", err)),
        }
    }

    let mut dirs = scan.official_dirs.clone();
    dirs.sort_by(|a, b| {
        b.components()
            .count()
            .cmp(&a.components().count())
            .then_with(|| b.cmp(a))
    });
    for rel in &dirs {
        let path = root.join(rel);
        match fs::symlink_metadata(&path) {
            Ok(meta) => {
                if is_symlink_or_reparse_point(&meta) || !meta.is_dir() {
                    continue;
                }
                match remove_empty_dir_with_retry("remove portable official dir", &path) {
                    Ok(()) => {}
                    Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                    Err(err) if allow_non_empty_dirs && is_dir_not_empty_error(&path, &err) => {}
                    Err(err) => return Err(io_err("remove portable official dir", err)),
                }
            }
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(io_err("inspect portable official dir", err)),
        }
    }

    match remove_empty_dir_with_retry("remove portable root dir", root) {
        Ok(()) => Ok(true),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(true),
        Err(err) if allow_non_empty_dirs && is_dir_not_empty_error(root, &err) => Ok(false),
        Err(err) => Err(io_err("remove portable root dir", err)),
    }
}

/// Safely dispose of a portable rollback backup after the new payload is in
/// place at `install_root`. Any non-official user files/directories found inside
/// `backup` (at root level or inside `app/`) are migrated via same-volume rename
/// before any official backup files are deleted:
///   - Root-level user items without a name collision are moved back into
///     `install_root` in-place so existing project paths and Codex threads stay valid.
///   - `app/`-nested user items and colliding root items are moved into a sibling
///     `<install_parent>/Codex-Preserved-Files-<timestamp>/` directory keeping
///     their relative paths and never overwriting existing entries.
///   - If scanning fails or ANY migration fails, the entire `backup` directory
///     is left untouched and returned via `retained_backup`.
pub fn cleanup_portable_backup_preserving_user_files(
    backup: &Path,
    install_root: &Path,
) -> PortableBackupCleanupOutcome {
    cleanup_portable_backup_preserving_user_files_with_mover(
        backup,
        install_root,
        |src, dst| rename_directory_with_retry("preserve user entry from portable backup", src, dst),
    )
}

fn cleanup_portable_backup_preserving_user_files_with_mover<M>(
    backup: &Path,
    install_root: &Path,
    mut move_entry: M,
) -> PortableBackupCleanupOutcome
where
    M: FnMut(&Path, &Path) -> io::Result<()>,
{
    let mut outcome = PortableBackupCleanupOutcome::default();
    let backup_meta = match fs::symlink_metadata(backup) {
        Ok(meta) => meta,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return outcome,
        Err(err) => {
            outcome.retained_backup = Some(backup.to_path_buf());
            outcome.notes.push(format!(
                "Could not inspect previous portable backup ({err}); kept backup at {} so no user files are lost.",
                backup.display()
            ));
            return outcome;
        }
    };
    if is_symlink_or_reparse_point(&backup_meta) || !backup_meta.is_dir() {
        outcome.retained_backup = Some(backup.to_path_buf());
        outcome.notes.push(format!(
            "Previous portable backup is not a regular directory; kept {} untouched.",
            backup.display()
        ));
        return outcome;
    }

    let scan = match scan_portable_root(backup, Some(install_root)) {
        Ok(scan) => scan,
        Err(err) => {
            log::warn!(
                "portable backup scan failed; retaining backup path={} error={err}",
                backup.display()
            );
            outcome.retained_backup = Some(backup.to_path_buf());
            outcome.notes.push(format!(
                "Could not verify files in previous portable backup ({err}); kept full backup at {} so no user files are lost.",
                backup.display()
            ));
            return outcome;
        }
    };

    let install_parent = install_root
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(install_root);
    let mut migration_failed = false;

    for rel in &scan.unknown_entries {
        let src = backup.join(rel);
        let is_root_level = rel
            .components()
            .filter(|c| matches!(c, Component::Normal(_)))
            .count()
            == 1;

        let try_in_place = if is_root_level {
            match rel
                .file_name()
                .map(|name| target_collides_in_dir(install_root, name))
            {
                Some(Ok(false)) => true,
                Some(Ok(true)) => false,
                Some(Err(err)) => {
                    migration_failed = true;
                    outcome.notes.push(format!(
                        "Could not check destination for user item '{}' ({err}); kept backup at {} so no user files are lost.",
                        display_rel_path(rel),
                        backup.display()
                    ));
                    break;
                }
                None => false,
            }
        } else {
            false
        };

        if try_in_place {
            let dst = install_root.join(rel);
            match move_entry(&src, &dst) {
                Ok(()) => {
                    outcome.restored_in_place.push(rel.clone());
                }
                Err(err) => {
                    migration_failed = true;
                    log::warn!(
                        "failed to restore user item in place src={} dst={} error={err}",
                        src.display(),
                        dst.display()
                    );
                    outcome.notes.push(format!(
                        "Failed to preserve user item '{}' into {} ({err}); kept previous backup at {} so no files are lost.",
                        display_rel_path(rel),
                        install_root.display(),
                        backup.display()
                    ));
                    break;
                }
            }
        } else {
            let preserved_root = match &outcome.preserved_dir {
                Some(dir) => dir.clone(),
                None => match create_unique_preserved_files_dir(install_parent) {
                    Ok(dir) => {
                        outcome.preserved_dir = Some(dir.clone());
                        dir
                    }
                    Err(err) => {
                        migration_failed = true;
                        outcome.notes.push(format!(
                            "Could not create preserved-files directory under {} ({err}); kept previous backup at {} so no files are lost.",
                            install_parent.display(),
                            backup.display()
                        ));
                        break;
                    }
                },
            };
            let dst = preserved_root.join(rel);
            if let Some(parent) = dst.parent() {
                if let Err(err) = fs::create_dir_all(parent) {
                    migration_failed = true;
                    outcome.notes.push(format!(
                        "Could not prepare preserved path for '{}' ({err}); kept previous backup at {} so no files are lost.",
                        display_rel_path(rel),
                        backup.display()
                    ));
                    break;
                }
            }
            if dst
                .parent()
                .zip(dst.file_name())
                .is_some_and(|(parent, name)| {
                    target_collides_in_dir(parent, name).unwrap_or(true)
                })
            {
                migration_failed = true;
                outcome.notes.push(format!(
                    "Preserved destination already exists for '{}'; kept previous backup at {} so no files are overwritten.",
                    display_rel_path(rel),
                    backup.display()
                ));
                break;
            }
            match move_entry(&src, &dst) {
                Ok(()) => {
                    outcome.moved_to_preserved_dir.push(rel.clone());
                }
                Err(err) => {
                    migration_failed = true;
                    // Prune any empty intermediate dirs created inside preserved_root.
                    let mut cur = dst.parent();
                    while let Some(dir) = cur {
                        if dir == preserved_root {
                            break;
                        }
                        if fs::remove_dir(dir).is_err() {
                            break;
                        }
                        cur = dir.parent();
                    }
                    log::warn!(
                        "failed to move user item to preserved dir src={} dst={} error={err}",
                        src.display(),
                        dst.display()
                    );
                    outcome.notes.push(format!(
                        "Failed to move user item '{}' to {} ({err}); kept previous backup at {} so no files are lost.",
                        display_rel_path(rel),
                        preserved_root.display(),
                        backup.display()
                    ));
                    break;
                }
            }
        }
    }

    if outcome.moved_to_preserved_dir.is_empty() {
        if let Some(empty_dir) = outcome.preserved_dir.take() {
            let _ = fs::remove_dir(&empty_dir);
        }
    }

    if !outcome.restored_in_place.is_empty() {
        outcome.notes.push(format!(
            "Preserved {} non-official item(s) in {} ({}). Consider moving personal projects outside the Codex install folder.",
            outcome.restored_in_place.len(),
            install_root.display(),
            summarize_rel_entries(&outcome.restored_in_place)
        ));
    }
    if !outcome.moved_to_preserved_dir.is_empty() {
        if let Some(dir) = &outcome.preserved_dir {
            outcome.notes.push(format!(
                "Moved {} non-official item(s) from the previous portable directory to {} ({}).",
                outcome.moved_to_preserved_dir.len(),
                dir.display(),
                summarize_rel_entries(&outcome.moved_to_preserved_dir)
            ));
        }
    }

    if migration_failed {
        outcome.retained_backup = Some(backup.to_path_buf());
        return outcome;
    }

    match remove_verified_official_entries(backup, &scan, false) {
        Ok(true) => {}
        Ok(false) => {
            outcome.retained_backup = Some(backup.to_path_buf());
            outcome.notes.push(format!(
                "Portable rollback backup was not empty after removing official files; kept {} so no unexpected files are lost.",
                backup.display()
            ));
        }
        Err(err) => {
            outcome.retained_backup = Some(backup.to_path_buf());
            outcome.notes.push(format!(
                "Portable rollback backup could not be removed after successful install: {err}"
            ));
        }
    }

    outcome
}

fn remove_portable_official_files(
    install_root: &Path,
    notes: &mut Vec<String>,
) -> Result<(bool, bool), EngineError> {
    let scan = scan_portable_root(install_root, None)?;
    let has_official = !scan.official_files.is_empty() || !scan.official_dirs.is_empty();
    let has_unknown = !scan.unknown_entries.is_empty();
    let removed_root = remove_verified_official_entries(install_root, &scan, true)?;
    let preserved_user_files = has_unknown || !removed_root;
    if has_unknown {
        notes.push(format!(
            "Preserved {} non-official user item(s) in {} ({}); only Codex program files were removed.",
            scan.unknown_entries.len(),
            install_root.display(),
            summarize_rel_entries(&scan.unknown_entries)
        ));
    } else if !removed_root {
        notes.push(format!(
            "Install directory {} was not empty after removing Codex program files; remaining files were preserved.",
            install_root.display()
        ));
    }
    Ok((has_official || removed_root, preserved_user_files))
}

struct PortableWorkDirGuard {
    work_dir: PathBuf,
    keep: bool,
}

impl PortableWorkDirGuard {
    fn new(work_dir: PathBuf) -> Self {
        Self {
            work_dir,
            keep: false,
        }
    }

    /// Leave the staging directory intact when a crash/recovery boundary
    /// intentionally preserves the staged payload for startup recovery.
    fn keep(&mut self) {
        self.keep = true;
    }

    fn cleanup_now(&mut self) {
        self.keep = true;
        cleanup_portable_work_dir(&self.work_dir);
    }
}

impl Drop for PortableWorkDirGuard {
    fn drop(&mut self) {
        if !self.keep {
            cleanup_portable_work_dir(&self.work_dir);
        }
    }
}

fn cleanup_portable_work_dir(work_dir: &Path) {
    if fs::symlink_metadata(work_dir).is_ok() {
        if let Err(err) = remove_directory_all_with_retry("cleanup portable staging", work_dir) {
            log::warn!(
                "failed to clean up portable staging directory path={} error={err}",
                work_dir.display()
            );
            return;
        }
    }
    // Remove `.codex-app-manager-staging` ONLY via non-recursive `remove_dir`
    // when it is empty, never touching sibling runs or unrelated entries.
    if let Some(staging_parent) = work_dir.parent() {
        if staging_parent
            .file_name()
            .and_then(|name| name.to_str())
            == Some(PORTABLE_STAGING_DIR_NAME)
        {
            match fs::remove_dir(staging_parent) {
                Ok(()) => {}
                Err(err) if err.kind() == io::ErrorKind::NotFound => {}
                Err(_) => {}
            }
        }
    }
}

/// Materialize the Manager-owned native launcher without touching upstream files.
/// Called inside staging for atomic update/rollback, and on launch to repair old installs.
pub fn ensure_portable_launcher(root: &Path) -> Result<PathBuf, EngineError> {
    let exe = installed_app_exe(root).ok_or_else(|| {
        EngineError::Install(format!("no portable app executable in {}", root.display()))
    })?;
    // Validate a required CLI before committing a broken payload, even when no
    // post-install launch was requested. Older fixture/legacy packages may omit it.
    portable_launch_command(&exe)?;
    #[cfg(windows)]
    {
        use crate::portable_command::{LAUNCHER_NAME, LAUNCH_TARGET_NAME};
        const BYTES: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/LaunchCodex.exe"));
        let target = exe
            .strip_prefix(root)
            .ok()
            .and_then(|s| s.to_str())
            .ok_or_else(|| EngineError::Install("invalid portable executable name".into()))?;
        let required = crate::app_version::requires_portable_cli(exe.parent().unwrap_or(root));
        let config = format!("{target}\nrequire-cli={}", u8::from(required));
        if crate::portable_command::parse_launch_config(&config) != Some((target, required)) {
            return Err(EngineError::Install(
                "invalid portable launch target".into(),
            ));
        }
        let nested =
            exe.parent() == Some(root.join(crate::portable_command::APP_DIR_NAME).as_path());
        let names: &[&str] = if nested {
            // Preserve existing root shortcuts and pins, including shortcuts
            // that used to point directly at the official ChatGPT executable.
            &["Codex.exe", "ChatGPT.exe", LAUNCHER_NAME]
        } else {
            &[LAUNCHER_NAME]
        };
        for name in names {
            write_portable_entry_file(&root.join(name), BYTES)?;
        }
        // New launchers accept old single-line configs. Commit the new format
        // only after every launcher is upgraded; a failed repair leaves the
        // original config usable. Atomic replacement avoids truncated files.
        write_portable_entry_file(&root.join(LAUNCH_TARGET_NAME), config.as_bytes())?;
        Ok(root.join(names[0]))
    }
    #[cfg(not(windows))]
    Ok(exe)
}

#[cfg(windows)]
fn write_portable_entry_file(path: &Path, bytes: &[u8]) -> Result<(), EngineError> {
    if fs::read(path).ok().as_deref() == Some(bytes) {
        return Ok(());
    }
    let parent = path
        .parent()
        .ok_or_else(|| EngineError::Install("missing launcher parent".into()))?;
    let temporary = parent.join(format!(".codex-portable-write-{}", uuid::Uuid::new_v4()));
    let mut created = false;
    let result = (|| {
        use std::io::Write;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| io_err("create portable entry file", error))?;
        created = true;
        file.write_all(bytes)
            .map_err(|error| io_err("write portable entry file", error))?;
        drop(file);
        rename_portable_dir("replace portable entry file", &temporary, path)
    })();
    if created && result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub(crate) fn portable_launch_command(exe: &Path) -> Result<std::process::Command, EngineError> {
    let root = exe
        .parent()
        .ok_or_else(|| EngineError::Install("missing portable directory".into()))?;
    let mut command = hidden_command(exe);
    crate::portable_command::configure(
        &mut command,
        exe,
        crate::app_version::requires_portable_cli(root),
    )
    .map_err(|e| io_err("configure portable launch", e))?;
    Ok(command)
}

pub(crate) fn repair_portable_launch_entry(root: &Path) -> Result<(), EngineError> {
    ensure_portable_launcher(root)?;
    if let Err(error) = create_start_menu_shortcut(root) {
        log::warn!("portable shortcut repair failed: {error}");
    }
    for warning in repair_portable_shortcut_icons(root) {
        log::warn!("{warning}");
    }
    // Legacy uninstall commands must learn protocol cleanup before its handler
    // is added. Refreshing them is also required when repairing a flat install.
    let version = read_codex_app_version_from_install_root(root).or_else(|| {
        fs::read_to_string(root.join("AppxManifest.xml")).ok()
            .and_then(|xml| parse_appx_manifest_xml(&xml).ok())
            .map(|identity| identity.version)
    }).unwrap_or_default();
    match register_uninstall_entry(root, &version, None) {
        Ok(true) => {
            match register_portable_protocol(root) {
                Ok(true) => {},
                Ok(false) => log::warn!("portable protocol repair skipped: existing registration was preserved"),
                Err(error) => log::warn!("portable protocol repair failed: {error}"),
            }
        }
        Err(error) => log::warn!("portable uninstall metadata repair failed: {error}"),
        Ok(false) => {}
    }
    Ok(())
}

#[cfg(windows)]
fn register_portable_protocol(root: &Path) -> Result<bool, EngineError> {
    let launcher = ensure_portable_launcher(root)?;
    let Some(icon) = installed_app_exe(root) else { return Ok(false); };
    crate::portable_protocol::register(&launcher, &icon)
        .map_err(|error| io_err("register portable protocol", error))
}

#[cfg(not(windows))]
fn register_portable_protocol(_root: &Path) -> Result<bool, EngineError> { Ok(false) }

#[cfg(windows)]
fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(windows)]
fn powershell_exe() -> PathBuf {
    std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .map(|windir| {
            windir
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe")
        })
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from("powershell.exe"))
}

#[cfg(all(windows, not(test)))]
fn run_powershell(script: &str) -> Result<String, EngineError> {
    // Shortcut/uninstall metadata scripts can wait on COM or registry work; use
    // the install budget so a stuck policy machine cannot hang forever.
    run_powershell_with_limits(script, RunLimits::install())
}

#[cfg(windows)]
fn run_powershell_with_limits(script: &str, limits: RunLimits) -> Result<String, EngineError> {
    let mut command = hidden_command(powershell_exe());
    let script = format!("try {{ [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false) }} catch {{ }}\n{script}");
    let encoded = crate::sys::encode_powershell_command(&script);
    command.args(["-NoProfile", "-NonInteractive", "-EncodedCommand", &encoded]);
    let output = run_capturing(command, limits, None)
        .map_err(|e| EngineError::Install(format!("powershell: {}", e.message())))?;
    if !output.status.success() {
        return Err(EngineError::Install(format!(
            "powershell failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

pub fn close_codex_gracefully_for_root(timeout_secs: u64, root: &Path) -> Result<(), EngineError> {
    crate::windows_process::close_codex_processes_for_root(timeout_secs, root)
}

/// Whether Codex currently has any process running from this exact install
/// root. The same native, path-pinned discovery is used by the close gate.
pub fn codex_running_for_root(root: &Path) -> Result<bool, EngineError> {
    crate::windows_process::codex_processes_running_for_root(root)
}

/// Only remove the independently created Manager shortcut, preserving links
/// which the user or the Windows package created at the same location.
#[cfg(windows)]
pub fn remove_manager_launch_shortcut() -> Result<(), EngineError> {
    let Some(appdata) = std::env::var_os("APPDATA") else {
        return Ok(());
    };
    let shortcut = PathBuf::from(appdata).join("Microsoft/Windows/Start Menu/Programs/Codex.lnk");
    if !shortcut.exists() {
        return Ok(());
    }
    let manager = std::env::current_exe().map_err(|error| io_err("locate Manager", error))?;
    if crate::portable_shortcut::is_manager_launcher(&shortcut, &manager)
        .map_err(|error| io_err("inspect Codex shortcut", error))?
    {
        fs::remove_file(shortcut).map_err(|error| io_err("remove Codex shortcut", error))?;
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn remove_manager_launch_shortcut() -> Result<(), EngineError> {
    Ok(())
}

/// Icon for shortcuts that start Codex: the official icon file when shipped,
/// then the official executable's own icon, then the Manager launcher stub
/// (which embeds an icon). Never the Manager executable.
#[cfg(any(windows, test))]
pub(crate) fn pick_shortcut_icon(
    app_exe: Option<&Path>,
    launcher: &Path,
    exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    if let Some(exe) = app_exe {
        if let Some(dir) = exe.parent() {
            let ico = dir.join("resources").join("icon-chatgpt.ico");
            if exists(&ico) {
                return ico;
            }
        }
        return exe.to_path_buf();
    }
    launcher.to_path_buf()
}

#[cfg(windows)]
fn shortcut_icon(install_root: &Path) -> PathBuf {
    let launcher = install_root.join(crate::portable_command::LAUNCHER_NAME);
    pick_shortcut_icon(installed_app_exe(install_root).as_deref(), &launcher, |p| {
        p.is_file()
    })
}

/// A stable Manager entry activates MSIX with package identity, or launches a
/// portable build, using the saved arguments. It also works while Manager is closed.
#[cfg(windows)]
pub fn create_manager_launch_shortcut(
    manager_exe: &Path,
    installed: &Path,
) -> Result<(), EngineError> {
    let appdata = std::env::var_os("APPDATA")
        .ok_or_else(|| EngineError::Io("APPDATA is unavailable".into()))?;
    let shortcut = PathBuf::from(appdata).join("Microsoft/Windows/Start Menu/Programs/Codex.lnk");
    if let Some(parent) = shortcut.parent() {
        fs::create_dir_all(parent).map_err(|error| io_err("create shortcut directory", error))?;
    }
    let icon = shortcut_icon(installed);
    let workdir = manager_exe.parent()
        .ok_or_else(|| EngineError::Io("Manager executable has no parent directory".into()))?;
    let spec = crate::portable_shortcut::ShortcutSpec::new(
        manager_exe,
        workdir,
        &icon,
        "--launch-codex",
    );
    // Read-compare-write: an unchanged shortcut is not rewritten on every launch.
    crate::portable_shortcut::ensure(&shortcut, &spec)
        .map(|_| ())
        .map_err(|error| io_err("update Codex shortcut", error))
}

#[cfg(not(windows))]
pub fn create_manager_launch_shortcut(
    _manager_exe: &Path,
    _installed: &Path,
) -> Result<(), EngineError> {
    Ok(())
}

#[cfg(any(windows, test))]
fn is_default_official_icon_name(icon: &Path) -> bool {
    let normalized = icon.to_string_lossy().replace('/', "\\").to_ascii_lowercase();
    let trimmed = normalized.trim().trim_matches('"').trim_end_matches('\\');
    let Some(file_name) = trimmed.rsplit('\\').next() else {
        return false;
    };
    matches!(file_name, "icon-chatgpt.ico" | "chatgpt.exe" | "codex.exe")
}

#[cfg(any(windows, test))]
fn portable_entry_candidates(install_root: &Path, app_exe: &Path) -> [PathBuf; 6] {
    let app_dir = install_root.join(crate::portable_command::APP_DIR_NAME);
    [
        app_exe.to_path_buf(),
        install_root.join(crate::portable_command::LAUNCHER_NAME),
        install_root.join("Codex.exe"),
        install_root.join("ChatGPT.exe"),
        app_dir.join("ChatGPT.exe"),
        app_dir.join("Codex.exe"),
    ]
}

#[cfg(any(windows, test))]
fn is_install_launcher_alias(install_root: &Path, app_exe: &Path, candidate: &Path) -> bool {
    let launcher = install_root.join(crate::portable_command::LAUNCHER_NAME);
    if crate::same_windows_path(candidate, &launcher) {
        return true;
    }
    // Root-level ChatGPT.exe / Codex.exe are launcher/alias executables whenever
    // the active payload lives elsewhere (e.g. app\ChatGPT.exe in nested layout).
    [install_root.join("Codex.exe"), install_root.join("ChatGPT.exe")]
        .iter()
        .any(|alias| {
            !crate::same_windows_path(alias, app_exe)
                && crate::same_windows_path(candidate, alias)
        })
}

#[cfg(any(windows, test))]
fn should_repair_portable_shortcut_icon(
    install_root: &Path,
    app_exe: &Path,
    manager_exe: Option<&Path>,
    target: &Path,
    arguments: &str,
    icon: &Path,
    icon_exists: impl Fn(&Path) -> bool,
) -> bool {
    let entries = portable_entry_candidates(install_root, app_exe);
    let is_direct_entry = entries
        .iter()
        .any(|path| crate::same_windows_path(target, path));
    let is_manager_entry = manager_exe.is_some_and(|manager| {
        crate::same_windows_path(target, manager) && arguments.trim() == "--launch-codex"
    });
    if !is_direct_entry && !is_manager_entry {
        return false;
    }
    if icon.as_os_str().is_empty() || is_install_launcher_alias(install_root, app_exe, icon) {
        return true;
    }
    if icon_exists(icon) {
        return false;
    }
    let app_dir = install_root.join(crate::portable_command::APP_DIR_NAME);
    let known_official_paths = [
        install_root.join("resources/icon-chatgpt.ico"),
        app_dir.join("resources/icon-chatgpt.ico"),
        install_root.join("ChatGPT.exe"),
        install_root.join("Codex.exe"),
        app_dir.join("ChatGPT.exe"),
        app_dir.join("Codex.exe"),
    ];
    known_official_paths
        .iter()
        .any(|path| crate::same_windows_path(icon, path))
        || is_default_official_icon_name(icon)
}

/// Repair a Start Menu or TaskBar shortcut whose target belongs to this
/// installation when its icon is blank, points to the launcher, or points to a
/// missing default official icon path/filename. Preserve custom icon sources,
/// including unresolved custom paths.
#[cfg(windows)]
fn repair_legacy_shortcut_icon(install_root: &Path, shortcut: &Path) -> Result<bool, EngineError> {
    if !shortcut.exists() {
        return Ok(false);
    }
    let Some(app_exe) = installed_app_exe(install_root) else {
        return Ok(false);
    };
    #[cfg(not(test))]
    let manager_exe = std::env::current_exe().ok();
    #[cfg(test)]
    let manager_exe: Option<PathBuf> = None;
    crate::portable_shortcut::ensure_icon_if(shortcut, &shortcut_icon(install_root), |current| {
        should_repair_portable_shortcut_icon(
            install_root,
            &app_exe,
            manager_exe.as_deref(),
            &current.target,
            &current.arguments,
            &current.icon,
            |path| path.is_file(),
        )
    })
    .map_err(|error| io_err("repair legacy portable shortcut icon", error))
}

#[cfg(windows)]
fn repair_portable_shortcut_icons_in(install_root: &Path, appdata: &Path) -> Vec<String> {
    let programs = appdata.join("Microsoft/Windows/Start Menu/Programs");
    let taskbar = appdata.join("Microsoft/Internet Explorer/Quick Launch/User Pinned/TaskBar");
    let candidates = [
        programs.join("ChatGPT.lnk"),
        programs.join("Codex.lnk"),
        taskbar.join("Codex.lnk"),
        taskbar.join("ChatGPT.lnk"),
    ];
    let mut warnings = Vec::new();
    for shortcut in candidates {
        if let Err(error) = repair_legacy_shortcut_icon(install_root, &shortcut) {
            warnings.push(format!(
                "Shortcut icon repair failed for {}: {error}",
                shortcut.display()
            ));
        }
    }
    warnings
}

#[cfg(all(windows, not(test)))]
fn repair_portable_shortcut_icons(install_root: &Path) -> Vec<String> {
    let Some(appdata) = std::env::var_os("APPDATA") else {
        return Vec::new();
    };
    repair_portable_shortcut_icons_in(install_root, &PathBuf::from(appdata))
}

#[cfg(any(not(windows), test))]
fn repair_portable_shortcut_icons(_install_root: &Path) -> Vec<String> {
    Vec::new()
}

#[cfg(all(windows, not(test)))]
fn create_start_menu_shortcut(install_root: &Path) -> Result<bool, EngineError> {
    let Some(exe) = installed_app_exe(install_root) else {
        return Ok(false);
    };
    let Some(appdata) = std::env::var_os("APPDATA") else {
        return Ok(false);
    };
    let shortcut = PathBuf::from(appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Codex.lnk");
    // Repair must not replace the saved-arguments entry, including when the
    // subsequent launch fails and the user retries from the Start Menu.
    if shortcut.exists() {
        let manager = std::env::current_exe().map_err(|error| io_err("locate Manager", error))?;
        if crate::portable_shortcut::is_manager_launcher(&shortcut, &manager)
            .map_err(|error| io_err("inspect Codex shortcut", error))?
        {
            // Keep the user's arguments; only repair a blank/stale icon.
            crate::portable_shortcut::ensure_icon(&shortcut, &shortcut_icon(install_root))
                .map_err(|error| io_err("repair Codex shortcut icon", error))?;
            return Ok(true);
        }
    }
    if let Some(parent) = shortcut.parent() {
        fs::create_dir_all(parent).map_err(|error| io_err("create shortcut directory", error))?;
    }
    let launcher = ensure_portable_launcher(install_root)?;
    let workdir = crate::portable_command::preferred_working_dir(install_root);
    let spec = crate::portable_shortcut::ShortcutSpec::new(
        &launcher,
        &workdir,
        &pick_shortcut_icon(Some(&exe), &launcher, |p| p.is_file()),
        "",
    );
    crate::portable_shortcut::ensure(&shortcut, &spec)
        .map_err(|error| io_err("create portable shortcut", error))?;
    Ok(true)
}

#[cfg(any(not(windows), test))]
fn create_start_menu_shortcut(_install_root: &Path) -> Result<bool, EngineError> {
    Ok(false)
}

#[cfg(all(windows, not(test)))]
fn register_uninstall_entry(install_root: &Path, version: &str, estimated_size_kb: Option<u64>) -> Result<bool, EngineError> {
    let exe=installed_app_exe(install_root).unwrap_or_else(|| install_root.join("Codex.exe"));
    let launcher=ensure_portable_launcher(install_root)?;
    run_powershell(&uninstall_metadata_script(install_root,&launcher,&exe,version,estimated_size_kb))?;
    Ok(true)
}

#[cfg(windows)]
fn portable_files_uninstall_ps_snippet(install_root: &Path) -> String {
    let escaped_root = install_root.to_string_lossy().replace('\'', "''");
    // Only remove recorded official files and Manager root entries; never run a
    // blanket recursive delete on `install_root` and never follow reparse points.
    // Uses compact built-in aliases so double-encoded UninstallString stays well
    // under the Windows command-line budget.
    format!(
        "function _g($x){{gi -LiteralPath $x -Force -ea 0}};\
         function _n($o){{$o -and ($o.Attributes -band 1024) -eq 0}};\
         function _z($x){{ri -LiteralPath $x -Force -ea 0}};\
         $r=('{root}').TrimEnd('\\','/');\
         $i=if($r){{_g $r}};\
         if((_n $i) -and $i.PSIsContainer){{\
         $d=@{{}};$m=Join-Path $r '{manifest}';$mi=_g $m;$ls=@();\
         if((_n $mi) -and !$mi.PSIsContainer){{$ls=@(gc -LiteralPath $m -Encoding UTF8 -ea 0)}};\
         ($ls+'Codex.exe','ChatGPT.exe','LaunchCodex.exe','codex-portable-launcher.txt','{manifest}','AppxManifest.xml')|%{{\
         $l=\"$_\".Trim();\
         if($l -and $l -notmatch '(^|[\\\\/])\\.\\.([\\\\/]|$)' -and $l -notlike '*:*' -and $l -notlike '/*' -and $l -notlike '\\*'){{ \
         $p=Join-Path $r ($l -replace '/','\\');$ok=$true;$c=Split-Path $p;$a=@();\
         while($c -and $c.Length -gt $r.Length){{$ai=_g $c;if($ai -and !(_n $ai)){{$ok=$false;break}};$a+=$c;$c=Split-Path $c}};\
         if($ok){{$f=_g $p;if((_n $f) -and !$f.PSIsContainer){{_z $p;$a|%{{$d[$_]=1}}}}}}}}}};\
         $d.Keys|sort Length -Desc|%{{$x=_g $_;if((_n $x) -and $x.PSIsContainer -and !(gci -LiteralPath $_ -Force -ea 0)){{_z $_}}}};\
         if(!(gci -LiteralPath $r -Force -ea 0)){{_z $r}}}}",
        root = escaped_root,
        manifest = PORTABLE_FILES_MANIFEST_NAME,
    )
}

#[cfg(windows)]
fn uninstall_metadata_script(install_root: &Path, launcher: &Path, exe: &Path, version: &str, estimated_size_kb: Option<u64>) -> String {
    let uninstall_script = format!(
        "{protocol}; if ($env:APPDATA) {{ $Shortcut = Join-Path $env:APPDATA 'Microsoft\\Windows\\Start Menu\\Programs\\Codex.lnk'; Remove-Item -LiteralPath $Shortcut -Force -ErrorAction SilentlyContinue }}; {files}; Remove-Item -LiteralPath 'HKCU:\\Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Codex' -Recurse -Force -ErrorAction SilentlyContinue",
        protocol = crate::portable_protocol::uninstall_script(launcher),
        files = portable_files_uninstall_ps_snippet(install_root),
    );
    // The protocol cleanup compares a quoted executable command. Encode the
    // complete script to avoid another Windows command-line quoting layer.
    let uninstall_string = format!(
        "powershell.exe -NoProfile -NonInteractive -ExecutionPolicy Bypass -EncodedCommand {}",
        crate::sys::encode_powershell_command(&uninstall_script)
    );
    // Launch repair preserves the existing size (or leaves it unknown). Only
    // an actual install/update traverses the payload to refresh this estimate.
    let estimated_size_update = estimated_size_kb.map(|size| format!(
        "New-ItemProperty -Path $key -Name EstimatedSize -Value {} -PropertyType DWord -Force | Out-Null",
        size.min(u32::MAX as u64)
    )).unwrap_or_default();
    let script = format!(
        r#"
$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Codex'
$uninstallCommand = {uninstall_string}
New-Item -Path $key -Force | Out-Null
New-ItemProperty -Path $key -Name DisplayName -Value 'Codex' -PropertyType String -Force | Out-Null
New-ItemProperty -Path $key -Name DisplayVersion -Value {version} -PropertyType String -Force | Out-Null
New-ItemProperty -Path $key -Name Publisher -Value 'OpenAI' -PropertyType String -Force | Out-Null
New-ItemProperty -Path $key -Name InstallLocation -Value {install_root} -PropertyType String -Force | Out-Null
New-ItemProperty -Path $key -Name DisplayIcon -Value {icon} -PropertyType String -Force | Out-Null
New-ItemProperty -Path $key -Name UninstallString -Value $uninstallCommand -PropertyType String -Force | Out-Null
New-ItemProperty -Path $key -Name QuietUninstallString -Value $uninstallCommand -PropertyType String -Force | Out-Null
New-ItemProperty -Path $key -Name NoModify -Value 1 -PropertyType DWord -Force | Out-Null
New-ItemProperty -Path $key -Name NoRepair -Value 1 -PropertyType DWord -Force | Out-Null
{estimated_size_update}
"#,
        version = ps_quote(version),
        install_root = ps_quote(&install_root.to_string_lossy()),
        icon = ps_quote(&format!("{},0", exe.to_string_lossy())),
        uninstall_string = ps_quote(&uninstall_string),
        estimated_size_update = estimated_size_update
    );
    script
}

#[cfg(any(not(windows), test))]
fn register_uninstall_entry(
    _install_root: &Path,
    _version: &str,
    _estimated_size_kb: Option<u64>,
) -> Result<bool, EngineError> {
    Ok(false)
}

#[cfg(all(windows, not(test)))]
fn remove_start_menu_shortcut() -> Result<bool, EngineError> {
    let Some(appdata) = std::env::var_os("APPDATA") else {
        return Ok(false);
    };
    let shortcut = PathBuf::from(appdata)
        .join("Microsoft")
        .join("Windows")
        .join("Start Menu")
        .join("Programs")
        .join("Codex.lnk");
    if shortcut.exists() {
        fs::remove_file(shortcut).map_err(|e| io_err("remove shortcut", e))?;
        Ok(true)
    } else {
        Ok(false)
    }
}

#[cfg(any(not(windows), test))]
fn remove_start_menu_shortcut() -> Result<bool, EngineError> {
    Ok(false)
}

#[cfg(all(windows, not(test)))]
fn remove_uninstall_entry() -> Result<bool, EngineError> {
    let script = r#"
$key = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\Codex'
if (Test-Path $key) {
  Remove-Item -Path $key -Recurse -Force
  'removed'
} else {
  'missing'
}
"#;
    Ok(run_powershell(script)?.trim().ends_with("removed"))
}

#[cfg(any(not(windows), test))]
fn remove_uninstall_entry() -> Result<bool, EngineError> {
    Ok(false)
}

fn dir_size_kb(root: &Path) -> u64 {
    let mut total = 0u64;
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            if meta.is_dir() {
                stack.push(entry.path());
            } else {
                total = total.saturating_add(meta.len());
            }
        }
    }
    total / 1024
}

fn restore_previous_install(
    install_root: &Path,
    backup: &Path,
    had_previous: bool,
) -> Result<(), EngineError> {
    // Never delete the failed/new tree and then discover that the old tree we
    // promised to restore is gone. Missing rollback material is ambiguous and
    // must not emit positive rollback evidence.
    if had_previous && !backup.exists() {
        return Err(EngineError::Install(format!(
            "portable rollback backup is missing: {}",
            backup.display()
        )));
    }
    if install_root.exists() {
        if let Err(close_err) = close_codex_gracefully_for_root(15, install_root) {
            log::warn!(
                "close processes in failed portable install before rollback root={} error={close_err}",
                install_root.display()
            );
        }
        remove_portable_dir("remove failed portable install", install_root)?;
    }
    if had_previous {
        rename_portable_dir("restore portable rollback backup", backup, install_root)?;
    }
    Ok(())
}

fn rollback_install_error(
    install_root: &Path,
    backup: &Path,
    had_previous: bool,
    observer: &mut PortableObserver<'_>,
    err: EngineError,
) -> EngineError {
    // Give the app layer a durable, pre-rename marker before the only backup is
    // consumed. The rollback itself still proceeds if bookkeeping fails: disk
    // safety wins, and RollbackCompleted gets a second chance to persist truth.
    let intent_error = observer(PortableBoundary::BeforeRollback {
        install_root: install_root.to_path_buf(),
        backup: backup.to_path_buf(),
        had_previous,
    })
    .err();
    match restore_previous_install(install_root, backup, had_previous) {
        Ok(()) => {
            let restored = if had_previous {
                "previous install was restored"
            } else {
                "new install was removed and the absent state was restored"
            };
            let completion = observer(PortableBoundary::RollbackCompleted {
                install_root: install_root.to_path_buf(),
                backup: backup.to_path_buf(),
                had_previous,
            });
            match (intent_error, completion) {
                (None, Ok(())) => EngineError::Install(format!("{err}; {restored}")),
                (Some(intent_err), Ok(())) => EngineError::Install(format!(
                    "{err}; {restored}, but recording rollback intent failed: {intent_err}"
                )),
                (_, Err(evidence_err)) => EngineError::Install(format!(
                    "{err}; {restored}, but recording rollback evidence failed: {evidence_err}"
                )),
            }
        }
        Err(rollback_err) => {
            EngineError::Install(format!("{err}; rollback failed: {rollback_err}"))
        }
    }
}

fn health_check_portable_install(
    install_root: &Path,
    launch: bool,
    keep_running: bool,
    arguments: &str,
) -> Result<bool, EngineError> {
    let exe = installed_app_exe(install_root).ok_or_else(|| {
        EngineError::Install(format!(
            "portable health check failed: no app entry executable (ChatGPT.exe / Codex.exe) in {}",
            install_root.display()
        ))
    })?;
    if !launch {
        return Ok(false);
    }
    // Spawn alone is not enough: a broken payload can exit immediately after
    // CreateProcess succeeds. Require a short liveness window, then leave the
    // process running (this path is the post-install relaunch).
    let requires_cli = crate::app_version::requires_portable_cli(install_root);
    let spawn_once = || -> Result<_, EngineError> {
        let mut command = portable_launch_command(&exe)?;
        crate::sys::apply_launch_arguments(&mut command, arguments)?;
        crate::process::spawn_and_check_startup(
            command,
            PORTABLE_LIVENESS_WINDOW,
            PORTABLE_STARTUP_WINDOW,
            requires_cli,
        )
        .map_err(|err| {
            EngineError::Install(format!(
                "portable health check launch failed: {}",
                err.message()
            ))
        })
    };

    let mut startup = spawn_once()?;
    if matches!(startup, LivenessResult::ExitedEarly { code: Some(0) }) {
        // Electron exits 0 immediately when another Codex instance (e.g. a
        // lingering MSIX/tray build) still holds the user-session single-instance
        // lock and receives the activation handoff. Sweep both MSIX and target
        // portable roots once, then retry the health-check launch.
        log::warn!(
            "portable health check exited immediately with code 0 (possible single-instance handoff); closing lingering Codex processes and retrying once root={}",
            install_root.display()
        );
        #[cfg(not(test))]
        if let Err(err) = crate::sys::close_msix_codex_processes(15) {
            log::warn!("close lingering MSIX Codex during portable health check retry error={err}");
        }
        if let Err(err) = close_codex_gracefully_for_root(15, install_root) {
            log::warn!(
                "close lingering portable Codex during health check retry root={} error={err}",
                install_root.display()
            );
        }
        startup = spawn_once()?;
    }

    match startup {
        LivenessResult::Survived { child } => {
            if keep_running {
                // Intentionally leak the Child handle so the relaunched app
                // keeps running after the manager drops the wait loop.
                std::mem::forget(child);
                Ok(true)
            } else {
                // The launch was only a health check. Close the whole Electron
                // process tree so an update cannot open an app that was closed
                // beforehand.
                let deadline = Instant::now() + Duration::from_secs(30);
                loop {
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    if remaining.is_zero() {
                        return Err(EngineError::Install(format!(
                            "portable health check cleanup failed: Codex is still running from {}",
                            install_root.display()
                        )));
                    }
                    // A closing Electron parent may replace itself with another
                    // process. Use bounded close slices and rescan the exact root
                    // between them instead of trusting one PID snapshot.
                    close_codex_gracefully_for_root(remaining.as_secs().clamp(1, 5), install_root)?;
                    if !codex_running_for_root(install_root)? {
                        break;
                    }
                }
                drop(child);
                Ok(false)
            }
        }
        LivenessResult::ExitedEarly { code } => Err(EngineError::Install(format!(
            "portable health check failed: entry executable exited immediately after launch (exit={})",
            code.map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string())
        ))),
    }
}

pub fn install_portable_from_msix(
    msix_path: &Path,
    install_root: &Path,
    relaunch: bool,
) -> Result<PortableInstallReport, EngineError> {
    let root = install_root.display();
    log::info!("portable install start install_root={root}");
    match install_portable_from_msix_inner(msix_path, install_root, true, relaunch) {
        Ok(report) => {
            let root = &report.install_root;
            log::info!("portable install completed install_root={root}");
            Ok(report)
        }
        Err(err) => {
            log::error!(
                "portable install failed install_root={} error={err}",
                install_root.display()
            );
            Err(err)
        }
    }
}

/// Rename boundary markers for crash-recovery callbacks and fault injection.
/// Path-carrying variants let the app layer persist a durable transaction log
/// with the real staging/backup locations chosen by this install.
#[derive(Debug, Clone)]
pub enum PortableBoundary {
    /// About to move the current install aside. `payload` is the staged new tree;
    /// `backup` is where the old install will go (if any).
    BeforeMoveOld {
        install_root: PathBuf,
        payload: PathBuf,
        backup: PathBuf,
        had_previous: bool,
    },
    /// Old install is at `backup`; install path is empty.
    AfterMoveOld {
        install_root: PathBuf,
        payload: PathBuf,
        backup: PathBuf,
        had_previous: bool,
    },
    BeforeMoveNew {
        install_root: PathBuf,
        payload: PathBuf,
        backup: PathBuf,
        had_previous: bool,
    },
    /// New payload is at install root.
    AfterMoveNew {
        install_root: PathBuf,
        backup: PathBuf,
        had_previous: bool,
    },
    /// A failure chose rollback; persist that intent before consuming backup.
    BeforeRollback {
        install_root: PathBuf,
        backup: PathBuf,
        had_previous: bool,
    },
    /// A later failure was fully rolled back. The install root now matches its
    /// pre-operation state (the old tree was restored, or a fresh tree removed).
    RollbackCompleted {
        install_root: PathBuf,
        backup: PathBuf,
        had_previous: bool,
    },
}

pub type PortableObserver<'a> = dyn FnMut(PortableBoundary) -> Result<(), EngineError> + 'a;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortableFault {
    BeforeMoveOld,
    AfterMoveOld,
    OnMoveNew,
    AfterMoveNew,
}

thread_local! {
    static PORTABLE_FAULT: std::cell::Cell<Option<PortableFault>> =
        const { std::cell::Cell::new(None) };
}

/// Install a one-shot fault for the next portable install rename sequence.
pub fn inject_portable_fault(fault: Option<PortableFault>) {
    PORTABLE_FAULT.with(|cell| cell.set(fault));
}

fn take_portable_fault() -> Option<PortableFault> {
    PORTABLE_FAULT.with(|cell| cell.take())
}

fn portable_fault_err(boundary: &str) -> EngineError {
    EngineError::Io(format!("injected portable fault at {boundary}"))
}

fn install_portable_from_msix_inner(
    msix_path: &Path,
    install_root: &Path,
    manage_process: bool,
    relaunch: bool,
) -> Result<PortableInstallReport, EngineError> {
    install_portable_from_msix_with_observer(
        msix_path,
        install_root,
        manage_process,
        relaunch,
        &mut |_| Ok(()),
    )
}

/// Like the normal portable install path, but notifies `observer` at each
/// rename boundary so callers can persist a crash-recovery transaction log.
pub fn install_portable_from_msix_with_observer(
    msix_path: &Path,
    install_root: &Path,
    manage_process: bool,
    relaunch: bool,
    observer: &mut PortableObserver<'_>,
) -> Result<PortableInstallReport, EngineError> {
    install_portable_from_msix_with_launch_arguments(
        msix_path,
        install_root,
        manage_process,
        relaunch,
        "",
        observer,
    )
}

pub fn install_portable_from_msix_with_launch_arguments(
    msix_path: &Path,
    install_root: &Path,
    manage_process: bool,
    relaunch: bool,
    arguments: &str,
    observer: &mut PortableObserver<'_>,
) -> Result<PortableInstallReport, EngineError> {
    crate::sys::validate_launch_arguments(arguments)?;
    if crate::sys::is_msix_package_path(install_root) {
        return Err(EngineError::Io(
            "Portable install location cannot be inside WindowsApps; choose a writable folder in Settings".to_string(),
        ));
    }
    let install_parent = install_root
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(install_root);
    fs::create_dir_all(install_parent).map_err(|e| io_err("create install parent", e))?;
    let operation_id = uuid::Uuid::new_v4();
    let work_dir = install_parent
        .join(PORTABLE_STAGING_DIR_NAME)
        .join(format!("portable-{operation_id}"));
    if work_dir.exists() {
        fs::remove_dir_all(&work_dir).map_err(|e| io_err("clear portable staging", e))?;
    }
    fs::create_dir_all(&work_dir).map_err(|e| io_err("create portable staging", e))?;
    let mut work_dir_guard = PortableWorkDirGuard::new(work_dir.clone());

    let prepared = prepare_portable_payload(msix_path, &work_dir)?;
    let payload = prepared.payload_dir;
    let backup = install_parent.join(format!("Codex.rollback-{operation_id}"));
    let mut notes = Vec::new();

    if manage_process {
        close_codex_gracefully_for_root(30, install_root)?;
    }

    let fault = take_portable_fault();
    let had_previous = install_root.exists();

    observer(PortableBoundary::BeforeMoveOld {
        install_root: install_root.to_path_buf(),
        payload: payload.clone(),
        backup: backup.clone(),
        had_previous,
    })?;
    if fault == Some(PortableFault::BeforeMoveOld) {
        return Err(portable_fault_err("before-move-old"));
    }

    if had_previous {
        rename_portable_dir("move current install to rollback", install_root, &backup)?;
    }

    // Observer must persist OldMoved. On failure: restore previous install when
    // possible so we never leave an empty root without a recovery path.
    if let Err(obs_err) = observer(PortableBoundary::AfterMoveOld {
        install_root: install_root.to_path_buf(),
        payload: payload.clone(),
        backup: backup.clone(),
        had_previous,
    }) {
        let rollback_err = rollback_install_error(
            install_root,
            &backup,
            had_previous,
            observer,
            obs_err,
        );
        if had_previous && !install_root.exists() && payload.exists() {
            work_dir_guard.keep();
        }
        return Err(rollback_err);
    }
    if fault == Some(PortableFault::AfterMoveOld) {
        // Leave the crash window intact for recovery tests (no auto-rollback).
        work_dir_guard.keep();
        return Err(portable_fault_err("after-move-old"));
    }

    if let Err(obs_err) = observer(PortableBoundary::BeforeMoveNew {
        install_root: install_root.to_path_buf(),
        payload: payload.clone(),
        backup: backup.clone(),
        had_previous,
    }) {
        if payload.exists() && !install_root.exists() {
            work_dir_guard.keep();
        }
        return Err(obs_err);
    }
    if fault == Some(PortableFault::OnMoveNew) {
        work_dir_guard.cleanup_now();
        return Err(rollback_install_error(
            install_root,
            &backup,
            had_previous,
            observer,
            portable_fault_err("on-move-new"),
        ));
    }

    match rename_portable_dir("install portable payload", &payload, install_root) {
        Ok(()) => {
            if let Err(obs_err) = observer(PortableBoundary::AfterMoveNew {
                install_root: install_root.to_path_buf(),
                backup: backup.clone(),
                had_previous,
            }) {
                // Payload is already at install_root; leave tx for recovery.
                log::error!("portable observer failed after move-new: {obs_err}");
                return Err(obs_err);
            }
        }
        Err(err) => {
            let rollback_err = rollback_install_error(
                install_root,
                &backup,
                had_previous,
                observer,
                err,
            );
            if had_previous && !install_root.exists() && payload.exists() {
                work_dir_guard.keep();
            }
            return Err(rollback_err);
        }
    }

    if fault == Some(PortableFault::AfterMoveNew) {
        work_dir_guard.cleanup_now();
        return Err(rollback_install_error(
            install_root,
            &backup,
            had_previous,
            observer,
            portable_fault_err("after-move-new"),
        ));
    }

    let relaunched = match health_check_portable_install(
        install_root,
        manage_process,
        manage_process && relaunch,
        arguments,
    ) {
        Ok(relaunched) => relaunched,
        Err(err) => {
            work_dir_guard.cleanup_now();
            return Err(rollback_install_error(
                install_root,
                &backup,
                had_previous,
                observer,
                err,
            ));
        }
    };

    let shortcut_created = match create_start_menu_shortcut(install_root) {
        Ok(created) => created,
        Err(err) => {
            notes.push(format!("Start menu shortcut was not created: {err}"));
            false
        }
    };
    notes.extend(repair_portable_shortcut_icons(install_root));
    let uninstall_entry_created = match register_uninstall_entry(
        install_root,
        &prepared.identity.version,
        Some(dir_size_kb(install_root)),
    ) {
        Ok(created) => created,
        Err(err) => {
            notes.push(format!(
                "Apps & Features uninstall entry was not created: {err}"
            ));
            false
        }
    };
    if uninstall_entry_created {
        match register_portable_protocol(install_root) {
            Ok(true) => {},
            Ok(false) => notes.push("Codex protocol registration was skipped: existing registration was preserved.".to_string()),
            Err(err) => notes.push(format!("Codex protocol registration failed: {err}")),
        }
    }

    let installed_exe = installed_app_exe(install_root);
    let mut backup_path = None;
    if had_previous && fs::symlink_metadata(&backup).is_ok() {
        let cleanup = cleanup_portable_backup_preserving_user_files(&backup, install_root);
        notes.extend(cleanup.notes);
        if let Some(retained) = cleanup.retained_backup {
            backup_path = Some(retained.to_string_lossy().into_owned());
        }
    }

    work_dir_guard.cleanup_now();

    let version = read_codex_app_version_from_install_root(install_root)
        .unwrap_or_else(|| prepared.identity.version.clone());

    Ok(PortableInstallReport {
        success: true,
        install_root: install_root.to_string_lossy().into_owned(),
        executable_path: installed_exe.map(|exe| exe.to_string_lossy().into_owned()),
        version,
        backup_path,
        shortcut_created,
        uninstall_entry_created,
        relaunched,
        message: "Portable Codex install completed.".to_string(),
        notes,
    })
}

/// Remove the user's Codex data directory (`~/.codex`: sign-in, sessions,
/// config). Returns whether a directory was actually deleted. Shared by the
/// portable and MSIX uninstall paths so both honor the "don't keep my data"
/// choice identically: a missing home directory is recorded as a note (nothing
/// to delete), while an IO failure removing an existing directory propagates.
pub fn purge_codex_user_data(notes: &mut Vec<String>) -> Result<bool, EngineError> {
    let Some(home) = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")) else {
        notes.push("User data purge requested but home directory was not available.".to_string());
        return Ok(false);
    };
    let user_data = PathBuf::from(home).join(".codex");
    if user_data.exists() {
        let path = user_data.display();
        log::warn!("purging Codex user data path={path}");
        fs::remove_dir_all(&user_data).map_err(|e| io_err("purge user data", e))?;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Ancillary-only cleanup after the portable app tree is already gone (or
/// never existed). Retries Start Menu shortcut + Apps & Features entry removal
/// without touching an install directory. Optional user-data purge.
pub fn cleanup_portable_metadata(
    purge_user_data: bool,
) -> Result<PortableUninstallReport, EngineError> {
    let mut notes = Vec::new();
    #[cfg(windows)]
    if let Err(error) = crate::portable_protocol::unregister() {
        notes.push(format!("Codex protocol cleanup failed: {error}"));
    }
    let removed_shortcut = match remove_start_menu_shortcut() {
        Ok(removed) => removed,
        Err(err) => {
            notes.push(format!("Start Menu shortcut cleanup failed: {err}"));
            false
        }
    };
    let removed_uninstall_entry = match remove_uninstall_entry() {
        Ok(removed) => removed,
        Err(err) => {
            notes.push(format!(
                "Apps & Features uninstall entry cleanup failed: {err}"
            ));
            false
        }
    };
    // User-data purge is ancillary: a failure must not abort the whole cleanup
    // report (matches the MSIX uninstall path — partial success + recovery CTA).
    let purged_user_data = if purge_user_data {
        match purge_codex_user_data(&mut notes) {
            Ok(purged) => purged,
            Err(err) => {
                notes.push(format!("User data cleanup failed: {err}"));
                false
            }
        }
    } else {
        false
    };
    let partial = notes.iter().any(|note| note.contains("cleanup failed"));
    Ok(PortableUninstallReport {
        success: true,
        partial,
        install_root: String::new(),
        removed_files: false,
        removed_shortcut,
        removed_uninstall_entry,
        purged_user_data,
        message: if partial {
            "Portable metadata cleanup completed with warnings.".to_string()
        } else {
            "Portable metadata cleanup completed.".to_string()
        },
        notes,
    })
}

pub fn uninstall_portable(
    install_root: &Path,
    purge_user_data: bool,
) -> Result<PortableUninstallReport, EngineError> {
    let path = install_root.display();
    log::info!("portable uninstall start path={path}");
    close_codex_gracefully_for_root(30, install_root)?;

    let mut uninstall_notes = Vec::new();
    let (removed_files, preserved_user_files) = match fs::symlink_metadata(install_root) {
        Err(err) if err.kind() == io::ErrorKind::NotFound => (false, false),
        Err(err) => return Err(io_err("inspect portable install", err)),
        Ok(meta) => {
            if is_symlink_or_reparse_point(&meta) {
                return Err(EngineError::Install(format!(
                    "refusing to remove symbolic link or junction at {}",
                    install_root.display()
                )));
            }
            if !meta.is_dir() {
                return Err(EngineError::Install(format!(
                    "portable install path is not a directory: {}",
                    install_root.display()
                )));
            }
            remove_portable_official_files(install_root, &mut uninstall_notes)?
        }
    };

    let mut meta = cleanup_portable_metadata(purge_user_data)?;
    // Preserve install-root context on the combined report.
    meta.install_root = install_root.to_string_lossy().into_owned();
    meta.removed_files = removed_files;
    if !uninstall_notes.is_empty() {
        uninstall_notes.append(&mut meta.notes);
        meta.notes = uninstall_notes;
    }
    meta.message = if meta.partial {
        "Portable Codex uninstall completed with cleanup warnings.".to_string()
    } else if preserved_user_files {
        "Portable Codex uninstall completed; non-official user files were preserved.".to_string()
    } else {
        "Portable Codex uninstall completed.".to_string()
    };
    let path = &meta.install_root;
    log::info!("portable uninstall completed path={path}");
    Ok(meta)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};
    use std::io::Write;
    use std::sync::atomic::{AtomicU64, Ordering};
    use zip::write::SimpleFileOptions;

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

    #[test]
    fn shortcut_icon_prefers_official_ico_then_app_exe_then_launcher() {
        let root = Path::new(r"C:\Codex");
        let launcher = root.join("LaunchCodex.exe");
        let exe = root.join("app").join("ChatGPT.exe");
        let ico = root.join("app").join("resources").join("icon-chatgpt.ico");
        assert_eq!(
            pick_shortcut_icon(Some(&exe), &launcher, |p| p == ico.as_path()),
            ico
        );
        // Nested layout without the ico: the real executable, never the stub.
        assert_eq!(pick_shortcut_icon(Some(&exe), &launcher, |_| false), exe);
        assert_eq!(pick_shortcut_icon(None, &launcher, |_| false), launcher);
    }

    #[test]
    fn shortcut_icon_repair_policy_handles_flat_to_nested_upgrade_and_custom_icons() {
        let root = Path::new(r"C:\Portable\Codex");
        let nested_exe = root.join("app/ChatGPT.exe");
        let nested_ico = root.join("app/resources/icon-chatgpt.ico");
        let flat_exe = root.join("ChatGPT.exe");
        let flat_ico = root.join("resources/icon-chatgpt.ico");
        let launcher = root.join(crate::portable_command::LAUNCHER_NAME);
        let manager = Path::new(r"C:\Program Files\Codex App Manager\Codex App Manager.exe");

        // Flat -> nested upgrade: pre-upgrade Start Menu & TaskBar links used
        // flat entry paths or deleted flat icons.
        for target in [&launcher, &flat_exe, &nested_exe] {
            for obsolete_icon in [
                Path::new(""),
                launcher.as_path(),
                flat_ico.as_path(),
                flat_exe.as_path(),
                root.join("Codex.exe").as_path(),
                nested_ico.as_path(),
                Path::new("icon-chatgpt.ico"),
                Path::new(r"C:\OldInstall\resources\icon-chatgpt.ico"),
            ] {
                assert!(
                    should_repair_portable_shortcut_icon(
                        root,
                        &nested_exe,
                        Some(manager),
                        target,
                        "",
                        obsolete_icon,
                        |p| crate::same_windows_path(p, &nested_exe)
                            || crate::same_windows_path(p, &launcher),
                    ),
                    "target={target:?} icon={obsolete_icon:?} should repair after flat->nested upgrade"
                );
            }
        }

        // Manager-owned pinned TaskBar shortcut also repairs missing flat icons
        // and preserves custom icons.
        assert!(should_repair_portable_shortcut_icon(
            root,
            &nested_exe,
            Some(manager),
            manager,
            "--launch-codex",
            &flat_ico,
            |_| false,
        ));
        assert!(!should_repair_portable_shortcut_icon(
            root,
            &nested_exe,
            Some(manager),
            manager,
            "--other-arg",
            &flat_ico,
            |_| false,
        ));

        // Reverse nested -> flat switch repairs missing nested icon path.
        assert!(should_repair_portable_shortcut_icon(
            root,
            &flat_exe,
            Some(manager),
            &nested_exe,
            "",
            &nested_ico,
            |p| crate::same_windows_path(p, &flat_exe),
        ));

        // Preserve explicit custom icons (even when missing) and valid existing current icons.
        for preserved in [
            Path::new(r"C:\Portable\Codex\custom.ico"),
            Path::new(r"D:\Icons\missing-custom.ico"),
            Path::new("relative.ico"),
            Path::new(r"%USERPROFILE%\custom.ico"),
            nested_ico.as_path(),
            nested_exe.as_path(),
        ] {
            assert!(
                !should_repair_portable_shortcut_icon(
                    root,
                    &nested_exe,
                    Some(manager),
                    &launcher,
                    "",
                    preserved,
                    |p| crate::same_windows_path(p, &nested_ico)
                        || crate::same_windows_path(p, &nested_exe),
                ),
                "icon={preserved:?} should be preserved"
            );
        }

        // Unrelated target is untouched even if its icon looks missing/default.
        assert!(!should_repair_portable_shortcut_icon(
            root,
            &nested_exe,
            Some(manager),
            Path::new(r"D:\OtherCodex\app\ChatGPT.exe"),
            "",
            &flat_ico,
            |_| false,
        ));
    }

    #[cfg(windows)]
    #[test]
    fn legacy_shortcut_icon_repairs_only_this_installs_entry_points() {
        let parent = temp_test_dir("legacy-shortcut-icon");
        let root = parent.join("便携 O'Neil & 子目录");
        let app = root.join("app");
        fs::create_dir_all(app.join("resources")).unwrap();
        let exe = app.join("ChatGPT.exe");
        fs::write(&exe, b"payload fixture").unwrap();
        let icon = app.join("resources/icon-chatgpt.ico");
        fs::write(&icon, b"icon fixture").unwrap();
        let shortcut = parent.join("ChatGPT.lnk");
        assert!(!repair_legacy_shortcut_icon(&root, &shortcut).unwrap());
        assert!(!shortcut.exists());

        for target in [
            root.join("ChatGPT.exe"),
            root.join("Codex.exe"),
            root.join(crate::portable_command::LAUNCHER_NAME),
            exe.clone(),
        ] {
            if !target.exists() {
                fs::write(&target, b"launcher fixture").unwrap();
            }
            crate::portable_shortcut::create_with_arguments(
                &shortcut,
                &target,
                &app,
                &root.join("ChatGPT.exe"),
                "--user-flag codex://threads/example",
            )
            .unwrap();
            let before = crate::portable_shortcut::read_spec(&shortcut).unwrap();
            assert!(repair_legacy_shortcut_icon(&root, &shortcut).unwrap());
            let mut expected = before;
            expected.icon = icon.clone();
            expected.icon_index = 0;
            assert_eq!(
                crate::portable_shortcut::read_spec(&shortcut).unwrap(),
                expected
            );
            let saved = fs::read(&shortcut).unwrap();
            let modified = fs::metadata(&shortcut).unwrap().modified().unwrap();
            assert!(!repair_legacy_shortcut_icon(&root, &shortcut).unwrap());
            assert_eq!(fs::read(&shortcut).unwrap(), saved);
            assert_eq!(
                fs::metadata(&shortcut).unwrap().modified().unwrap(),
                modified
            );
        }

        for target in [
            parent.join("ChatGPT.exe"),
            parent.join("other-install/app/ChatGPT.exe"),
            root.join("tools/helper.exe"),
            app.join("resources/codex.exe"),
        ] {
            fs::create_dir_all(target.parent().unwrap()).unwrap();
            fs::write(&target, b"unrelated fixture").unwrap();
            // Use a repairable icon so this checks target ownership rather
            // than passing because the custom-icon policy skips the link.
            crate::portable_shortcut::create(
                &shortcut,
                &target,
                &app,
                &root.join("ChatGPT.exe"),
            )
            .unwrap();
            let saved = fs::read(&shortcut).unwrap();
            assert!(!repair_legacy_shortcut_icon(&root, &shortcut).unwrap());
            assert_eq!(fs::read(&shortcut).unwrap(), saved);
        }
        fs::remove_dir_all(parent).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn legacy_shortcut_icon_supports_flat_installs_and_missing_payloads() {
        let root = temp_test_dir("legacy-shortcut-flat");
        let exe = root.join("ChatGPT.exe");
        fs::write(&exe, b"legacy payload fixture").unwrap();
        let shortcut = root.join("ChatGPT.lnk");
        let stale = root.join("resources/icon-chatgpt.ico");
        crate::portable_shortcut::create(&shortcut, &exe, &root, &stale).unwrap();
        assert!(repair_legacy_shortcut_icon(&root, &shortcut).unwrap());
        assert!(crate::same_windows_path(
            &crate::portable_shortcut::read_spec(&shortcut).unwrap().icon,
            &exe,
        ));
        crate::portable_shortcut::create(&shortcut, &exe, &root, &stale).unwrap();
        fs::remove_file(&exe).unwrap();
        let saved = fs::read(&shortcut).unwrap();
        assert!(!repair_legacy_shortcut_icon(&root, &shortcut).unwrap());
        assert_eq!(fs::read(&shortcut).unwrap(), saved);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn legacy_shortcut_icon_repairs_blank_and_missing_default_sources() {
        let parent = temp_test_dir("legacy-shortcut-default-icon");
        let root = parent.join("portable");
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        let exe = app.join("ChatGPT.exe");
        fs::write(&exe, b"payload fixture").unwrap();
        let shortcut = parent.join("ChatGPT.lnk");

        for icon in [
            PathBuf::new(),
            app.join("resources/icon-chatgpt.ico"),
            app.join("Codex.exe"),
            root.join("resources/icon-chatgpt.ico"),
            PathBuf::from("icon-chatgpt.ico"),
        ] {
            crate::portable_shortcut::create_with_arguments(
                &shortcut,
                &exe,
                &app,
                &icon,
                "--user-flag",
            )
            .unwrap();
            let mut expected = crate::portable_shortcut::read_spec(&shortcut).unwrap();
            assert!(repair_legacy_shortcut_icon(&root, &shortcut).unwrap());
            expected.icon = exe.clone();
            expected.icon_index = 0;
            assert_eq!(
                crate::portable_shortcut::read_spec(&shortcut).unwrap(),
                expected
            );
            let bytes = fs::read(&shortcut).unwrap();
            let modified = fs::metadata(&shortcut).unwrap().modified().unwrap();
            assert!(!repair_legacy_shortcut_icon(&root, &shortcut).unwrap());
            assert_eq!(fs::read(&shortcut).unwrap(), bytes);
            assert_eq!(
                fs::metadata(&shortcut).unwrap().modified().unwrap(),
                modified
            );
        }
        fs::remove_dir_all(parent).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn legacy_shortcut_icon_preserves_explicit_custom_and_current_icons() {
        let parent = temp_test_dir("legacy-shortcut-custom-icon");
        let root = parent.join("portable");
        let app = root.join("app");
        fs::create_dir_all(app.join("resources")).unwrap();
        let exe = app.join("ChatGPT.exe");
        fs::write(&exe, b"payload fixture").unwrap();
        let official_icon = app.join("resources/icon-chatgpt.ico");
        fs::write(&official_icon, b"official icon fixture").unwrap();
        let custom = root.join("custom.ico");
        fs::write(&custom, b"custom icon fixture").unwrap();
        let external = parent.join("custom.ico");
        fs::write(&external, b"external icon fixture").unwrap();
        let shortcut = parent.join("ChatGPT.lnk");

        for icon in [
            custom,
            external,
            parent.join("missing-custom.ico"),
            PathBuf::from("relative.ico"),
            PathBuf::from(r"%USERPROFILE%\custom.ico"),
            official_icon,
            exe.clone(),
        ] {
            crate::portable_shortcut::create(&shortcut, &exe, &app, &icon).unwrap();
            let bytes = fs::read(&shortcut).unwrap();
            let modified = fs::metadata(&shortcut).unwrap().modified().unwrap();
            assert!(
                !repair_legacy_shortcut_icon(&root, &shortcut).unwrap(),
                "{icon:?}"
            );
            assert_eq!(fs::read(&shortcut).unwrap(), bytes, "{icon:?}");
            assert_eq!(
                fs::metadata(&shortcut).unwrap().modified().unwrap(),
                modified
            );
        }

        // In the flat layout ChatGPT.exe is the payload, not a launcher alias.
        let flat = parent.join("flat");
        fs::create_dir_all(&flat).unwrap();
        let flat_exe = flat.join("ChatGPT.exe");
        fs::write(&flat_exe, b"legacy payload fixture").unwrap();
        crate::portable_shortcut::create(&shortcut, &flat_exe, &flat, &flat_exe).unwrap();
        let bytes = fs::read(&shortcut).unwrap();
        assert!(!repair_legacy_shortcut_icon(&flat, &shortcut).unwrap());
        assert_eq!(fs::read(&shortcut).unwrap(), bytes);
        fs::remove_dir_all(parent).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn legacy_shortcut_icon_does_not_replace_malformed_or_other_named_links() {
        let parent = temp_test_dir("legacy-shortcut-invalid");
        let root = parent.join("portable");
        let app = root.join("app");
        fs::create_dir_all(&app).unwrap();
        fs::write(app.join("ChatGPT.exe"), b"payload fixture").unwrap();
        let untouched = b"not a Shell Link";
        let other = parent.join("Other.lnk");
        fs::write(&other, untouched).unwrap();
        let shortcut = parent.join("ChatGPT.lnk");
        assert!(!repair_legacy_shortcut_icon(&root, &shortcut).unwrap());
        assert_eq!(fs::read(&other).unwrap(), untouched);
        assert!(!shortcut.exists());
        fs::write(&shortcut, untouched).unwrap();
        assert!(repair_legacy_shortcut_icon(&root, &shortcut).is_err());
        assert_eq!(fs::read(&shortcut).unwrap(), untouched);
        assert_eq!(fs::read(&other).unwrap(), untouched);
        fs::remove_dir_all(parent).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn flat_to_nested_upgrade_repairs_start_menu_and_taskbar_icons() {
        let parent = temp_test_dir("flat-to-nested-shortcuts");
        let root = parent.join("portable");
        let appdata = parent.join("appdata");
        let programs = appdata.join("Microsoft/Windows/Start Menu/Programs");
        let taskbar = appdata.join("Microsoft/Internet Explorer/Quick Launch/User Pinned/TaskBar");
        fs::create_dir_all(&programs).unwrap();
        fs::create_dir_all(&taskbar).unwrap();
        fs::create_dir_all(&root).unwrap();

        let flat_exe = root.join("ChatGPT.exe");
        let flat_ico = root.join("resources/icon-chatgpt.ico");
        let launcher = root.join(crate::portable_command::LAUNCHER_NAME);
        fs::create_dir_all(flat_ico.parent().unwrap()).unwrap();
        fs::write(&flat_exe, b"flat payload fixture").unwrap();
        fs::write(&flat_ico, b"flat icon fixture").unwrap();
        fs::write(&launcher, b"launcher fixture").unwrap();

        let start_chatgpt = programs.join("ChatGPT.lnk");
        let start_codex = programs.join("Codex.lnk");
        let taskbar_codex = taskbar.join("Codex.lnk");
        let taskbar_custom = taskbar.join("ChatGPT.lnk");
        let taskbar_unrelated = taskbar.join("Other.lnk");
        let custom_ico = root.join("custom.ico");
        fs::write(&custom_ico, b"custom icon fixture").unwrap();

        crate::portable_shortcut::create(&start_chatgpt, &flat_exe, &root, &flat_ico).unwrap();
        crate::portable_shortcut::create(&start_codex, &launcher, &root, &flat_exe).unwrap();
        crate::portable_shortcut::create_with_arguments(
            &taskbar_codex,
            &launcher,
            &root,
            &flat_ico,
            "--saved",
        )
        .unwrap();
        crate::portable_shortcut::create(&taskbar_custom, &flat_exe, &root, &custom_ico).unwrap();
        let untouched = b"not a Shell Link";
        fs::write(&taskbar_unrelated, untouched).unwrap();

        // Simulate upgrade switching layout from flat -> nested: flat payload/ico
        // no longer exist, new nested app\ChatGPT.exe & app\resources\icon-chatgpt.ico do.
        fs::remove_file(&flat_exe).unwrap();
        fs::remove_dir_all(flat_ico.parent().unwrap()).unwrap();
        let nested_app = root.join("app");
        fs::create_dir_all(nested_app.join("resources")).unwrap();
        let nested_exe = nested_app.join("ChatGPT.exe");
        let nested_ico = nested_app.join("resources/icon-chatgpt.ico");
        fs::write(&nested_exe, b"nested payload fixture").unwrap();
        fs::write(&nested_ico, b"nested icon fixture").unwrap();

        let warnings = repair_portable_shortcut_icons_in(&root, &appdata);
        assert!(warnings.is_empty(), "{warnings:?}");

        for link in [&start_chatgpt, &start_codex, &taskbar_codex] {
            let spec = crate::portable_shortcut::read_spec(link).unwrap();
            assert!(
                crate::same_windows_path(&spec.icon, &nested_ico),
                "{link:?} icon={:?}",
                spec.icon
            );
        }
        assert_eq!(
            crate::portable_shortcut::read_spec(&taskbar_codex)
                .unwrap()
                .arguments,
            "--saved"
        );
        assert!(crate::same_windows_path(
            &crate::portable_shortcut::read_spec(&taskbar_custom)
                .unwrap()
                .icon,
            &custom_ico
        ));
        assert_eq!(fs::read(&taskbar_unrelated).unwrap(), untouched);

        // Idempotent second run keeps bytes untouched.
        let saved_taskbar = fs::read(&taskbar_codex).unwrap();
        assert!(repair_portable_shortcut_icons_in(&root, &appdata).is_empty());
        assert_eq!(fs::read(&taskbar_codex).unwrap(), saved_taskbar);
        fs::remove_dir_all(parent).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn portable_shortcut_identity_preserves_its_launcher_target() {
        let parent = temp_test_dir("shortcut-identity");
        let root = parent.join("便携 O'Neil & 子目录");
        fs::create_dir_all(&root).unwrap();
        let exe = root.join("Codex.exe");
        fs::write(&exe, b"launcher fixture").unwrap();
        let shortcut = root.join("Codex.lnk");
        crate::portable_shortcut::create(&shortcut, &exe, &root, &exe).unwrap();
        let (target, id) = crate::portable_shortcut::inspect(&shortcut).unwrap();
        assert_eq!(fs::canonicalize(target).unwrap(), fs::canonicalize(exe).unwrap());
        assert_eq!(id, crate::portable_shortcut::APP_ID);
        fs::remove_dir_all(parent).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn portable_metadata_script_preserves_unicode_input_and_output() {
        let root = temp_test_dir("unicode-metadata");
        let path = root.join("中文 O'Neil & file.txt");
        let value = "中文 O'Neil & metadata";
        let output = run_powershell_with_limits(&format!(
            "$value = {}; [System.IO.File]::WriteAllText({}, $value); $value",
            ps_quote(value), ps_quote(&path.to_string_lossy())
        ), RunLimits::probe()).unwrap();
        assert_eq!(output, value);
        assert_eq!(fs::read_to_string(path).unwrap(), value);
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn encoded_uninstall_metadata_fits_the_windows_command_line() {
        let root = PathBuf::from(format!(r"C:\便携 O'Neil & long path\{}", "x".repeat(220)));
        let script = uninstall_metadata_script(&root, &root.join("Codex.exe"),
            &root.join("app/ChatGPT.exe"), "26.930.31730", Some(900_000));
        // Include the real transport prefix and leave room for its exe/flags.
        let wrapped = format!("try {{ [Console]::OutputEncoding = [System.Text.UTF8Encoding]::new($false) }} catch {{ }}\n{script}");
        let encoded = crate::sys::encode_powershell_command(&wrapped);
        assert!(encoded.len() < 30_000, "metadata command is too long: {}", encoded.len());
    }

    #[test]
    fn packaged_core_requires_cli_before_install_is_committed() {
        let root = temp_test_dir("required-cli");
        fs::create_dir_all(root.join("resources")).unwrap();
        fs::write(root.join("ChatGPT.exe"), b"app").unwrap();
        crate::app_version::write_test_asar(&root.join("resources/app.asar"),
            br#"{"version":"26.915.31029","name":"openai-codex-electron","codexWindowsAppContainedCore":"1"}"#);
        assert!(crate::app_version::requires_portable_cli(&root));
        // Explicit user overrides intentionally need no bundled executable.
        if std::env::var_os("CODEX_CLI_PATH").is_none_or(|v| v.to_string_lossy().trim().is_empty())
        {
            assert!(ensure_portable_launcher(&root)
                .unwrap_err()
                .to_string()
                .contains("missing its bundled CLI"));
        }
        fs::write(root.join("resources/codex.exe"), b"cli").unwrap();
        assert!(ensure_portable_launcher(&root).unwrap().is_file());
        fs::remove_dir_all(root).unwrap();
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires a marked disposable nested payload via CODEX_REAL_PORTABLE"]
    fn real_portable_root_launcher_reaches_main_window() {
        let root =
            PathBuf::from(std::env::var_os("CODEX_REAL_PORTABLE").expect("CODEX_REAL_PORTABLE"));
        assert!(
            root.join(".codex-manager-smoke").is_file(),
            "use a marked, disposable copy"
        );
        assert!(
            root.join("app/ChatGPT.exe").is_file(),
            "use the nested payload layout"
        );
        let profile = temp_test_dir("isolated-root-launcher");
        let launcher = ensure_portable_launcher(&root).unwrap();
        assert_eq!(launcher, root.join("Codex.exe"));
        let result = (|| -> Result<(), String> {
            let mut command = hidden_command(&launcher);
            command
                .env_remove("CODEX_CLI_PATH")
                .env_remove("CODEX_WINDOWS_REGISTERED_CORE")
                .env("CODEX_HOME", profile.join("home"))
                .env("CODEX_ELECTRON_USER_DATA_PATH", profile.join("profile"))
                .arg(format!(
                    "--user-data-dir={}",
                    profile.join("profile").display()
                ));
            // Do not pipe stdio: Electron inherits it from the launcher and
            // could keep a capture reader open after the launcher exits.
            let mut child = command
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
                .map_err(|error| error.to_string())?;
            let launcher_deadline = Instant::now() + Duration::from_secs(10);
            loop {
                match child.try_wait().map_err(|error| error.to_string())? {
                    Some(status) if status.success() => break,
                    Some(_) => return Err("launcher exited unsuccessfully".into()),
                    None if Instant::now() >= launcher_deadline => {
                        let _ = child.kill();
                        let _ = child.wait();
                        return Err("launcher did not exit within 10 seconds".into());
                    }
                    None => thread::sleep(Duration::from_millis(50)),
                }
            }
            // The launcher exits after spawning Electron, so observe the scoped
            // payload processes rather than accepting the launcher's exit=0.
            let started = Instant::now();
            let mut progress = crate::startup_window::StartupProgress::default();
            while started.elapsed() < Duration::from_secs(30) {
                let (_, window) = crate::windows_process::startup_window_for_root(&root)
                    .map_err(|error| error.to_string())?;
                if progress.observe(started.elapsed(), &window, PORTABLE_LIVENESS_WINDOW)? {
                    return Ok(());
                }
                thread::sleep(Duration::from_millis(100));
            }
            Err("root launcher did not open a stable main window".into())
        })();
        close_codex_gracefully_for_root(30, &root).unwrap();
        assert!(result.is_ok(), "{result:?}");
        let _ = fs::remove_dir_all(profile);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires an isolated payload copy via CODEX_REAL_PORTABLE"]
    fn real_portable_bootstrap_uses_bundled_cli() {
        let root =
            PathBuf::from(std::env::var_os("CODEX_REAL_PORTABLE").expect("CODEX_REAL_PORTABLE"));
        assert!(
            root.join(".codex-manager-smoke").is_file(),
            "use a marked, disposable copy"
        );
        let profile = temp_test_dir("isolated-bootstrap");
        let exe = installed_app_exe(&root).unwrap();
        ensure_portable_launcher(&root).unwrap();
        let mut command = portable_launch_command(&exe).unwrap();
        command
            .env("CODEX_HOME", profile.join("home"))
            .env("CODEX_ELECTRON_USER_DATA_PATH", profile.join("profile"))
            .env("CODEX_SPARKLE_ENABLED", "false")
            .arg(format!(
                "--user-data-dir={}",
                profile.join("profile").display()
            ));
        let result = crate::process::spawn_and_check_startup(
            command,
            PORTABLE_LIVENESS_WINDOW,
            PORTABLE_STARTUP_WINDOW,
            true,
        );
        let cleanup = close_codex_gracefully_for_root(30, &root);
        cleanup.unwrap();
        assert!(
            matches!(result, Ok(LivenessResult::Survived { .. })),
            "{result:?}"
        );
        let _ = fs::remove_dir_all(profile);
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires official CODEX_REAL_MSIX and a marked disposable CODEX_REAL_MSIX_SMOKE_DIR"]
    fn real_msix_portable_startup() {
        let msix = PathBuf::from(std::env::var_os("CODEX_REAL_MSIX").expect("CODEX_REAL_MSIX"));
        let root = PathBuf::from(
            std::env::var_os("CODEX_REAL_MSIX_SMOKE_DIR").expect("CODEX_REAL_MSIX_SMOKE_DIR"),
        );
        assert!(
            root.join(".codex-manager-smoke").is_file(),
            "use a marked disposable directory"
        );
        assert!(crate::authenticode::verify_openai_authenticode(&msix)
            .unwrap()
            .is_valid_openai());
        let prepared = prepare_portable_payload(&msix, &root).unwrap();
        let payload = prepared.payload_dir;
        ensure_portable_launcher(&payload).unwrap();
        let mut command = portable_launch_command(&installed_app_exe(&payload).unwrap()).unwrap();
        command
            .env("CODEX_HOME", root.join("home"))
            .env("CODEX_ELECTRON_USER_DATA_PATH", root.join("profile"))
            .env("CODEX_SPARKLE_ENABLED", "false")
            .arg(format!(
                "--user-data-dir={}",
                root.join("profile").display()
            ));
        let result = crate::process::spawn_and_check_startup(
            command,
            PORTABLE_LIVENESS_WINDOW,
            PORTABLE_STARTUP_WINDOW,
            true,
        );
        let cleanup = close_codex_gracefully_for_root(10, &payload);
        cleanup.unwrap();
        assert!(
            matches!(result, Ok(LivenessResult::Survived { .. })),
            "{result:?}"
        );
        // Keep the marked lab and app logs for investigation. No MSIX install
        // or shortcut is created. The desktop app itself can register Chrome
        // hosts; restore lab discovery/manifest entries as documented in #353.
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "requires an isolated affected 26.915.31029 payload via CODEX_REAL_PORTABLE"]
    fn real_portable_bootstrap_dialog_is_rejected() {
        let root =
            PathBuf::from(std::env::var_os("CODEX_REAL_PORTABLE").expect("CODEX_REAL_PORTABLE"));
        assert!(
            root.join(".codex-manager-smoke").is_file(),
            "use a marked, disposable copy"
        );
        let profile = temp_test_dir("isolated-baseline");
        let mut command = hidden_command(installed_app_exe(&root).unwrap());
        command
            .env_remove("CODEX_CLI_PATH")
            .env_remove("CODEX_WINDOWS_REGISTERED_CORE")
            .env("CODEX_HOME", profile.join("home"))
            .env("CODEX_ELECTRON_USER_DATA_PATH", profile.join("profile"))
            .env("CODEX_SPARKLE_ENABLED", "false")
            .arg(format!(
                "--user-data-dir={}",
                profile.join("profile").display()
            ));
        let result = crate::process::spawn_and_check_startup(
            command,
            PORTABLE_LIVENESS_WINDOW,
            PORTABLE_STARTUP_WINDOW,
            true,
        );
        let cleanup = close_codex_gracefully_for_root(30, &root);
        cleanup.unwrap();
        assert!(
            matches!(result, Err(ref e) if e.message().contains("startup dialog") || e.message().contains("did not open its main window")),
            "{result:?}"
        );
        let _ = fs::remove_dir_all(profile);
    }

    // Invoked only by the compiled launcher roundtrip below; no test harness flags
    // are interpreted by our launcher, and all child environment state is isolated.
    #[cfg(windows)]
    #[test]
    #[ignore = "child process fixture for portable_launcher_survives_directory_move"]
    fn portable_launcher_child() {
        let Some(output) = std::env::var_os("CODEX_TEST_LAUNCH_REPORT") else {
            return;
        };
        let report = serde_json::json!({
            "cli": std::env::var("CODEX_CLI_PATH").ok(),
            "registered": std::env::var("CODEX_WINDOWS_REGISTERED_CORE").ok(),
            "cwd": std::env::current_dir().unwrap(),
            "args": std::env::args().collect::<Vec<_>>(),
        });
        fs::write(output, serde_json::to_vec(&report).unwrap()).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn portable_launcher_survives_directory_move() {
        let parent = temp_test_dir("launcher-roundtrip");
        for nested in [false, true] {
            let original = parent.join(format!("before-{nested}"));
            let app_root = if nested {
                original.join("app")
            } else {
                original.clone()
            };
            fs::create_dir_all(app_root.join("resources")).unwrap();
            fs::copy(
                std::env::current_exe().unwrap(),
                app_root.join("ChatGPT.exe"),
            )
            .unwrap();
            fs::write(app_root.join("resources/codex.exe"), b"cli fixture").unwrap();
            let original_exe = fs::read(app_root.join("ChatGPT.exe")).unwrap();
            let launcher = ensure_portable_launcher(&original).unwrap();
            assert!(
                is_manager_launcher(&launcher),
                "native launcher PE identity must survive linking"
            );
            assert!(!is_manager_launcher(&app_root.join("ChatGPT.exe")));
            assert_launcher_has_no_vc_runtime_import(&fs::read(&launcher).unwrap());
            assert_launcher_embeds_icon(&fs::read(&launcher).unwrap());
            assert_eq!(
                fs::read(app_root.join("ChatGPT.exe")).unwrap(),
                original_exe
            );
            assert_eq!(
                launcher,
                original.join(if nested {
                    "Codex.exe"
                } else {
                    "LaunchCodex.exe"
                })
            );
            assert_eq!(
                fs::read_to_string(original.join(crate::portable_command::LAUNCH_TARGET_NAME))
                    .unwrap(),
                if nested {
                    "app\\ChatGPT.exe\nrequire-cli=0"
                } else {
                    "ChatGPT.exe\nrequire-cli=0"
                }
            );
            let moved = parent.join(format!("便携 测试's & folder-{nested}"));
            fs::rename(&original, &moved).unwrap();
            let moved_app = if nested {
                moved.join("app")
            } else {
                moved.clone()
            };
            let launcher_home = parent.join(format!("home-{nested}"));
            fs::create_dir_all(&launcher_home).unwrap();
            let names: &[&str] = if nested {
                &["Codex.exe", "ChatGPT.exe", "LaunchCodex.exe"]
            } else {
                &["LaunchCodex.exe"]
            };
            for name in names {
                let output = parent.join(format!("result-{nested}-{name}.json"));
                let marker = "codex://test/参数 with spaces & \"quotes\"";
                let mut command = hidden_command(moved.join(name));
                command
                    .env("USERPROFILE", &launcher_home)
                    .env_remove("CODEX_CLI_PATH")
                    .env("CODEX_WINDOWS_REGISTERED_CORE", "1")
                    .env("CODEX_TEST_LAUNCH_REPORT", &output)
                    .args([
                        "--exact",
                        "portable::tests::portable_launcher_child",
                        "--ignored",
                        "--skip",
                        marker,
                    ]);
                assert!(command.status().unwrap().success());
                let deadline = Instant::now() + Duration::from_secs(10);
                while !output.exists() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(50));
                }
                let report: serde_json::Value =
                    serde_json::from_slice(&fs::read(output).expect("launcher child report"))
                        .unwrap();
                assert_eq!(
                    report["cli"],
                    moved_app
                        .join("resources/codex.exe")
                        .to_string_lossy()
                        .as_ref()
                );
                assert!(report["registered"].is_null());
                assert_eq!(
                    fs::canonicalize(Path::new(report["cwd"].as_str().unwrap())).unwrap(),
                    fs::canonicalize(&launcher_home).unwrap()
                );
                assert!(report["args"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|v| v == marker));
            }
        }
        remove_directory_all_with_retry("remove launcher test", &parent).unwrap();
    }

    #[test]
    fn failed_layout_upgrade_restores_the_flat_install() {
        let parent = temp_test_dir("layout-rollback");
        let root = parent.join("Codex");
        fs::create_dir_all(root.join("resources")).unwrap();
        fs::write(root.join("ChatGPT.exe"), b"old official entry").unwrap();
        fs::write(root.join("resources/app.asar"), b"old official resources").unwrap();
        let old_launcher = ensure_portable_launcher(&root).unwrap();
        let old_bytes = fs::read(&old_launcher).unwrap();
        let msix = parent.join("new.msix");
        write_fake_rebranded_msix(&msix);
        let mut saw_new_layout = false;
        inject_portable_fault(Some(PortableFault::AfterMoveNew));
        let error =
            install_portable_from_msix_with_observer(&msix, &root, false, false, &mut |boundary| {
                if matches!(boundary, PortableBoundary::AfterMoveNew { .. }) {
                    saw_new_layout = root.join("app/ChatGPT.exe").is_file();
                }
                Ok(())
            })
            .unwrap_err();
        assert!(saw_new_layout);
        assert!(error.to_string().contains("previous install was restored"));
        assert!(!root.join("app").exists());
        assert_eq!(
            fs::read(root.join("ChatGPT.exe")).unwrap(),
            b"old official entry"
        );
        assert_eq!(
            fs::read(root.join("resources/app.asar")).unwrap(),
            b"old official resources"
        );
        assert_eq!(fs::read(old_launcher).unwrap(), old_bytes);
        #[cfg(windows)]
        assert_eq!(
            fs::read_to_string(root.join(crate::portable_command::LAUNCH_TARGET_NAME)).unwrap(),
            "ChatGPT.exe\nrequire-cli=0"
        );
        remove_directory_all_with_retry("remove layout rollback test", &parent).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn legacy_payload_without_cli_runs_through_each_root_launcher() {
        let parent = temp_test_dir("launcher-no-cli");
        for nested in [false, true] {
            let root = parent.join(format!("legacy-{nested}"));
            let app = if nested {
                root.join("app")
            } else {
                root.clone()
            };
            fs::create_dir_all(&app).unwrap();
            fs::copy(std::env::current_exe().unwrap(), app.join("ChatGPT.exe")).unwrap();
            ensure_portable_launcher(&root).unwrap();
            let launcher_home = parent.join(format!("legacy-home-{nested}"));
            fs::create_dir_all(&launcher_home).unwrap();
            let names: &[&str] = if nested {
                &["Codex.exe", "ChatGPT.exe", "LaunchCodex.exe"]
            } else {
                &["LaunchCodex.exe"]
            };
            for name in names {
                let output = parent.join(format!("{nested}-{name}.json"));
                let mut command = hidden_command(root.join(name));
                command
                    .env("USERPROFILE", &launcher_home)
                    .env_remove("CODEX_CLI_PATH")
                    .env("CODEX_TEST_LAUNCH_REPORT", &output)
                    .args([
                        "--exact",
                        "portable::tests::portable_launcher_child",
                        "--ignored",
                    ]);
                assert!(command.status().unwrap().success());
                let deadline = Instant::now() + Duration::from_secs(10);
                while !output.exists() && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(50));
                }
                let report: serde_json::Value =
                    serde_json::from_slice(&fs::read(output).expect("CLI-less child report"))
                        .unwrap();
                assert!(
                    report["cli"].is_null(),
                    "a legacy payload must not require or invent a CLI"
                );
                assert_eq!(
                    fs::canonicalize(Path::new(report["cwd"].as_str().unwrap())).unwrap(),
                    fs::canonicalize(&launcher_home).unwrap()
                );
            }
        }
        remove_directory_all_with_retry("remove CLI-less launcher test", &parent).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn failed_launcher_repair_preserves_the_old_config() {
        use crate::portable_command::{LAUNCHER_NAME, LAUNCH_TARGET_NAME};
        use std::os::windows::fs::OpenOptionsExt;
        let parent = temp_test_dir("launcher-repair-lock");
        for blocked in [LAUNCHER_NAME, LAUNCH_TARGET_NAME] {
            let root = parent.join(blocked);
            fs::create_dir_all(&root).unwrap();
            fs::write(root.join("ChatGPT.exe"), b"legacy official exe").unwrap();
            fs::write(root.join(LAUNCHER_NAME), b"old launcher").unwrap();
            fs::write(root.join(LAUNCH_TARGET_NAME), "ChatGPT.exe").unwrap();
            let lock = fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(root.join(blocked))
                .unwrap();
            assert!(ensure_portable_launcher(&root).is_err());
            drop(lock);
            assert_eq!(
                fs::read_to_string(root.join(LAUNCH_TARGET_NAME)).unwrap(),
                "ChatGPT.exe"
            );
            if blocked == LAUNCHER_NAME {
                assert_eq!(fs::read(root.join(LAUNCHER_NAME)).unwrap(), b"old launcher");
            }
            assert!(!fs::read_dir(&root).unwrap().any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with(".codex-portable-write-")));
            assert!(ensure_portable_launcher(&root).is_ok());
            assert_eq!(
                fs::read_to_string(root.join(LAUNCH_TARGET_NAME)).unwrap(),
                "ChatGPT.exe\nrequire-cli=0"
            );
        }
        remove_directory_all_with_retry("remove locked launcher repair test", &parent).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn lost_payload_and_config_never_adopts_root_aliases() {
        let parent = temp_test_dir("lost-launcher-layout");
        let msix = parent.join("codex.msix");
        write_fake_rebranded_msix(&msix);
        let root = parent.join("Codex");
        install_portable_from_msix_inner(&msix, &root, false, false).unwrap();
        assert_eq!(installed_app_exe(&root), Some(root.join("app/ChatGPT.exe")));
        fs::remove_dir_all(root.join("app")).unwrap();
        fs::remove_file(root.join(crate::portable_command::LAUNCH_TARGET_NAME)).unwrap();
        assert_eq!(installed_app_exe(&root), None);
        assert!(crate::sys::detect_portable_install(&root).is_none());
        for config in ["", "broken", "ChatGPT.exe\nrequire-cli=0"] {
            fs::write(
                root.join(crate::portable_command::LAUNCH_TARGET_NAME),
                config,
            )
            .unwrap();
            assert_eq!(installed_app_exe(&root), None);
        }
        fs::remove_file(root.join("AppxManifest.xml")).unwrap();
        assert_eq!(installed_app_exe(&root), None);
        remove_directory_all_with_retry("remove lost layout test", &parent).unwrap();
    }

    // Inspect the PE import table, not arbitrary strings in the binary: a native
    // runner already has VC++ installed and would otherwise hide this regression.
    #[cfg(windows)]
    fn assert_launcher_embeds_icon(bytes: &[u8]) {
        let u16_at = |p| u16::from_le_bytes(bytes[p..p + 2].try_into().unwrap()) as usize;
        let u32_at = |p| u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap()) as usize;
        let coff = u32_at(0x3c) + 4;
        let section_table = coff + 20 + u16_at(coff + 16);
        let rsrc = (0..u16_at(coff + 2))
            .map(|i| section_table + i * 40)
            .find(|s| &bytes[*s..*s + 5] == b".rsrc")
            .expect("launcher must have a .rsrc section");
        let dir = u32_at(rsrc + 20);
        let entries = u16_at(dir + 12) + u16_at(dir + 14);
        let ids: Vec<usize> = (0..entries)
            .map(|i| u32_at(dir + 16 + i * 8))
            .collect();
        assert!(ids.contains(&3), "RT_ICON missing: {ids:?}");
        assert!(ids.contains(&14), "RT_GROUP_ICON missing: {ids:?}");
    }

    #[cfg(windows)]
    fn assert_launcher_has_no_vc_runtime_import(bytes: &[u8]) {
        let u16_at = |p| u16::from_le_bytes(bytes[p..p + 2].try_into().unwrap()) as usize;
        let u32_at = |p| u32::from_le_bytes(bytes[p..p + 4].try_into().unwrap()) as usize;
        let coff = u32_at(0x3c) + 4;
        let optional = coff + 20;
        let section_table = optional + u16_at(coff + 16);
        let offset = |rva: usize| {
            (0..u16_at(coff + 2))
                .find_map(|i| {
                    let section = section_table + i * 40;
                    let start = u32_at(section + 12);
                    let size = u32_at(section + 8).max(u32_at(section + 16));
                    (rva >= start && rva < start + size).then(|| u32_at(section + 20) + rva - start)
                })
                .expect("PE RVA must map to a section")
        };
        let directories = optional + if u16_at(optional) == 0x20b { 112 } else { 96 };
        let mut import = offset(u32_at(directories + 8));
        let mut count = 0;
        while u32_at(import + 12) != 0 {
            let name = offset(u32_at(import + 12));
            let len = bytes[name..].iter().position(|b| *b == 0).unwrap();
            let dll = std::str::from_utf8(&bytes[name..name + len])
                .unwrap()
                .to_ascii_lowercase();
            assert!(
                !dll.starts_with("vcruntime") && !dll.starts_with("msvcp"),
                "portable launcher must not require the VC++ redistributable: {dll}"
            );
            count += 1;
            import += 20;
        }
        assert!(
            count > 0,
            "launcher import table must actually be inspected"
        );
    }

    fn temp_test_dir(name: &str) -> PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("codex-portable-{name}-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn portable_directory_rename_retries_transient_windows_lock_errors() {
        let attempts = Cell::new(0usize);
        let sleeps = RefCell::new(Vec::new());

        rename_with_retry(
            "test rename",
            Path::new("source"),
            Path::new("destination"),
            || {
                let attempt = attempts.get() + 1;
                attempts.set(attempt);
                if attempt < 3 {
                    Err(io::Error::from_raw_os_error(32))
                } else {
                    Ok(())
                }
            },
            |duration| sleeps.borrow_mut().push(duration),
        )
        .unwrap();

        assert_eq!(attempts.get(), 3);
        assert_eq!(
            sleeps.into_inner(),
            vec![Duration::from_millis(50), Duration::from_millis(100)]
        );
    }

    #[test]
    fn portable_directory_rename_recognizes_all_transient_windows_codes() {
        for code in [5, 32, 33] {
            assert!(is_transient_windows_fs_error(
                &io::Error::from_raw_os_error(code)
            ));
        }
        assert!(!is_transient_windows_fs_error(
            &io::Error::from_raw_os_error(2)
        ));
    }

    #[test]
    fn portable_directory_rename_does_not_retry_permanent_errors() {
        let attempts = Cell::new(0usize);
        let sleeps = Cell::new(0usize);

        let err = rename_with_retry(
            "test rename",
            Path::new("source"),
            Path::new("destination"),
            || {
                attempts.set(attempts.get() + 1);
                Err(io::Error::from_raw_os_error(2))
            },
            |_| sleeps.set(sleeps.get() + 1),
        )
        .unwrap_err();

        assert_eq!(err.raw_os_error(), Some(2));
        assert_eq!(attempts.get(), 1);
        assert_eq!(sleeps.get(), 0);
    }

    #[test]
    fn portable_directory_rename_retry_is_bounded() {
        let attempts = Cell::new(0usize);
        let sleeps = Cell::new(0usize);

        let err = rename_with_retry(
            "test rename",
            Path::new("source"),
            Path::new("destination"),
            || {
                attempts.set(attempts.get() + 1);
                Err(io::Error::from_raw_os_error(5))
            },
            |_| sleeps.set(sleeps.get() + 1),
        )
        .unwrap_err();

        assert_eq!(err.raw_os_error(), Some(5));
        assert_eq!(attempts.get(), WINDOWS_FS_RETRY_DELAYS_MS.len() + 1);
        assert_eq!(sleeps.get(), WINDOWS_FS_RETRY_DELAYS_MS.len());
    }

    #[test]
    fn portable_directory_failure_preserves_paths_and_windows_error_code() {
        for code in [5, 32, 33] {
            let error = portable_rename_error(
                "move current install to rollback",
                Path::new(r"C:\Users\Test User\Codex"),
                Path::new(r"C:\Users\Test User\Codex.rollback"),
                &io::Error::from_raw_os_error(code),
            )
            .to_string();
            assert!(error.contains("portable install directory unavailable"));
            assert!(error.contains(r#"source="C:\Users\Test User\Codex""#));
            assert!(error.contains(r#"destination="C:\Users\Test User\Codex.rollback""#));
            assert!(error.contains(&format!("os error {code}")));
        }
        let error = portable_rename_error(
            "move current install to rollback",
            Path::new("missing"),
            Path::new("backup"),
            &io::Error::from_raw_os_error(2),
        )
        .to_string();
        assert!(!error.contains("portable install directory unavailable"));
    }

    /// When `~/.codex` is a file (not a directory), purge fails — must stay
    /// non-fatal so the portable uninstall can still report partial success.
    #[test]
    fn user_data_purge_failure_is_non_fatal_in_metadata_cleanup() {
        let home = temp_test_dir("purge-home");
        // A regular file at `.codex` makes remove_dir_all fail.
        fs::write(home.join(".codex"), b"not-a-directory").unwrap();
        let prev_user = std::env::var_os("USERPROFILE");
        let prev_home = std::env::var_os("HOME");
        // SAFETY: test-only, serialised by the unique temp dir; restored below.
        std::env::set_var("USERPROFILE", &home);
        std::env::set_var("HOME", &home);
        let report = cleanup_portable_metadata(true).expect("purge failure must not Err");
        match prev_user {
            Some(v) => std::env::set_var("USERPROFILE", v),
            None => std::env::remove_var("USERPROFILE"),
        }
        match prev_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
        assert!(report.success);
        assert!(report.partial, "purge IO failure should mark partial");
        assert!(!report.purged_user_data);
        assert!(report
            .notes
            .iter()
            .any(|n| n.contains("User data cleanup failed")));
        let _ = fs::remove_dir_all(home);
    }

    fn write_fake_msix(path: &Path) {
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default();
        zip.start_file("AppxManifest.xml", opts).unwrap();
        zip.write_all(
            br#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Identity Name="OpenAI.Codex" Publisher="CN=OpenAI OpCo, LLC" Version="26.602.3474.0" ProcessorArchitecture="x64" />
</Package>"#,
        )
        .unwrap();
        zip.start_file("VFS/ProgramFilesX64/Codex/Codex.exe", opts)
            .unwrap();
        zip.write_all(b"fake exe").unwrap();
        zip.start_file("VFS/ProgramFilesX64/Codex/resources/app.asar", opts)
            .unwrap();
        zip.write_all(b"fake asar").unwrap();
        zip.finish().unwrap();
    }

    fn write_fake_rebranded_msix(path: &Path) {
        // Post-rebrand layout: manifest entry is app/ChatGPT.exe while a legacy
        // Codex.exe still ships next to it (as on the real 26.707.x package).
        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default();
        zip.start_file("AppxManifest.xml", opts).unwrap();
        zip.write_all(
            br#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Identity Name="OpenAI.Codex" Publisher="CN=OpenAI OpCo, LLC" Version="26.707.3748.0" ProcessorArchitecture="x64" />
  <Applications>
    <Application Id="App" Executable="app/ChatGPT.exe" EntryPoint="Windows.FullTrustApplication" />
  </Applications>
</Package>"#,
        )
        .unwrap();
        zip.start_file("app/ChatGPT.exe", opts).unwrap();
        zip.write_all(b"fake entry exe").unwrap();
        zip.start_file("app/Codex.exe", opts).unwrap();
        zip.write_all(b"legacy compat exe").unwrap();
        zip.start_file("app/resources/app.asar", opts).unwrap();
        zip.write_all(b"fake asar").unwrap();
        zip.finish().unwrap();
    }

    fn write_block_mapped_msix(
        path: &Path,
        block_map_files: &[(&str, u64)],
        zip_files: &[(&str, &[u8])],
    ) {
        let block_map_files = block_map_files
            .iter()
            .map(|(name, size)| format!(r#"  <File Name="{name}" Size="{size}" LfhSize="30" />"#))
            .collect::<Vec<_>>()
            .join("\n");
        let block_map = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<BlockMap xmlns="http://schemas.microsoft.com/appx/2010/blockmap" HashMethod="http://www.w3.org/2001/04/xmlenc#sha256">
{block_map_files}
</BlockMap>"#
        );
        let manifest = br#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Identity Name="OpenAI.Codex" Publisher="CN=OpenAI OpCo, LLC" Version="26.803.10989.0" ProcessorArchitecture="x64" />
  <Applications>
    <Application Id="App" Executable="app/ChatGPT.exe" EntryPoint="Windows.FullTrustApplication" />
  </Applications>
</Package>"#;

        let file = fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default();
        zip.start_file("AppxBlockMap.xml", opts).unwrap();
        zip.write_all(block_map.as_bytes()).unwrap();
        zip.start_file("AppxManifest.xml", opts).unwrap();
        zip.write_all(manifest).unwrap();
        for (name, data) in zip_files {
            zip.start_file(*name, opts).unwrap();
            zip.write_all(data).unwrap();
        }
        zip.finish().unwrap();
    }

    #[test]
    fn extracts_legacy_cp437_zip_entry_names() {
        let root = temp_test_dir("cp437-zip-entry-name");
        let msix = root.join("codex.msix");
        let extracted = root.join("extracted");
        fs::create_dir_all(&extracted).unwrap();
        let payload = b"legacy filename";
        write_block_mapped_msix(
            &msix,
            &[("café.bin", payload.len() as u64)],
            &[("cafe.bin", payload)],
        );
        let mut bytes = fs::read(&msix).unwrap();
        let name = b"cafe.bin";
        let mut replaced = 0;
        for index in 0..=bytes.len() - name.len() {
            if bytes[index..index + name.len()] == *name {
                bytes[index + 3] = 0x82; // CP437 encodes é as one byte.
                replaced += 1;
            }
        }
        assert_eq!(replaced, 2, "update local and central directory names");
        fs::write(&msix, bytes).unwrap();
        extract_msix(&msix, &extracted).unwrap();
        assert_eq!(
            fs::read(extracted.join("café.bin")).unwrap(),
            b"legacy filename"
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn extracts_cp437_names_that_are_also_valid_utf8() {
        use crate::zip_test::{archive, Entry};

        let root = temp_test_dir("ambiguous-cp437");
        let msix = root.join("codex.msix");
        let extracted = root.join("extracted");
        fs::create_dir_all(&extracted).unwrap();
        let block_map = r#"<BlockMap xmlns="http://schemas.microsoft.com/appx/2010/blockmap">
            <File Name="app\├⌐.bin" Size="7" LfhSize="30" />
            </BlockMap>"#;
        let bytes = archive(&[
            Entry::new(b"AppxBlockMap.xml", block_map.as_bytes()),
            Entry::new(b"AppxManifest.xml", b"<Package />"),
            Entry::new(b"app/\xc3\xa9.bin", b"payload"),
        ]);
        fs::write(&msix, bytes).unwrap();
        extract_msix(&msix, &extracted).unwrap();
        assert_eq!(fs::read(extracted.join("app/├⌐.bin")).unwrap(), b"payload");
        assert!(!extracted.join("app/é.bin").exists());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn installs_cp437_utf8_and_unicode_path_block_map_names() {
        use crate::zip_test::{archive, unicode_path_extra, Entry};

        let raw = b"app/\xc3\xa9.bin";
        let unicode = unicode_path_extra(raw, "app/资源.bin".as_bytes(), true);
        let bad_crc = unicode_path_extra(raw, "app/资源.bin".as_bytes(), false);
        for (flags, extra, expected) in [
            (0, &[][..], "├⌐.bin"),
            (1 << 11, &[][..], "é.bin"),
            (0, unicode.as_slice(), "资源.bin"),
            (0, bad_crc.as_slice(), "├⌐.bin"),
        ] {
            let root = temp_test_dir("encoded-msix-install");
            let msix = root.join("codex.msix");
            let installed = root.join("Codex");
            let block_map = format!(
                r#"<BlockMap xmlns="http://schemas.microsoft.com/appx/2010/blockmap">
                <File Name="app\{expected}" Size="7" LfhSize="30" />
                </BlockMap>"#
            );
            let manifest = br#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
                <Identity Name="OpenAI.Codex" Publisher="CN=OpenAI OpCo, LLC" Version="26.803.10989.0" ProcessorArchitecture="x64" />
                <Applications><Application Id="App" Executable="app/ChatGPT.exe" EntryPoint="Windows.FullTrustApplication" /></Applications>
                </Package>"#;
            let mut resource = Entry::new(raw, b"payload");
            resource.flags = flags;
            resource.extra = extra;
            fs::write(
                &msix,
                archive(&[
                    Entry::new(b"AppxBlockMap.xml", block_map.as_bytes()),
                    Entry::new(b"AppxManifest.xml", manifest),
                    Entry::new(b"app/ChatGPT.exe", b"fake entry exe"),
                    resource,
                ]),
            )
            .unwrap();
            let report = install_portable_from_msix_inner(&msix, &installed, false, false).unwrap();
            assert!(report.success);
            assert_eq!(
                fs::read(installed.join("app").join(expected)).unwrap(),
                b"payload"
            );
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn rejects_unsafe_unicode_path_during_msix_extraction() {
        use crate::zip_test::{archive, unicode_path_extra, Entry};

        let raw = b"app/safe.bin";
        for unsafe_name in [
            "../escape.bin", r"..\escape.bin", "/absolute", r"C:\escape", "nul\0path",
        ] {
            let root = temp_test_dir("unsafe-unicode-msix");
            let msix = root.join("codex.msix");
            let extracted = root.join("extracted");
            fs::create_dir_all(&extracted).unwrap();
            let extra = unicode_path_extra(raw, unsafe_name.as_bytes(), true);
            let mut entry = Entry::new(raw, b"payload");
            entry.extra = &extra;
            fs::write(
                &msix,
                archive(&[Entry::new(b"AppxManifest.xml", b"<Package />"), entry]),
            )
            .unwrap();
            assert!(extract_msix(&msix, &extracted).is_err(), "{unsafe_name:?}");
            assert!(!root.join("escape.bin").exists());
            fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn extracts_percent_encoded_payload_to_block_map_logical_paths() {
        let root = temp_test_dir("block-map-logical-paths");
        let msix = root.join("codex.msix");
        let extracted = root.join("extracted");
        fs::create_dir_all(&extracted).unwrap();
        let sky = b"sky";
        let statsig = b"statsig";
        let chatgpt = b"fake entry";
        let asar = b"fake asar";
        write_block_mapped_msix(
            &msix,
            &[
                (r"app\ChatGPT.exe", chatgpt.len() as u64),
                (r"app\resources\app.asar", asar.len() as u64),
                (
                    r"app\resources\cua_node\bin\node_modules\@oai\sky\package.json",
                    sky.len() as u64,
                ),
                (
                    r"app\resources\cua_node\bin\node_modules\@statsig\client-core\src\$_StatsigGlobal.js",
                    statsig.len() as u64,
                ),
            ],
            &[
                ("app/ChatGPT.exe", chatgpt),
                ("app/resources/app.asar", asar),
                (
                    "app/resources/cua_node/bin/node_modules/%40oai/sky/package.json",
                    sky,
                ),
                (
                    "app/resources/cua_node/bin/node_modules/%40statsig/client-core/src/%24_StatsigGlobal.js",
                    statsig,
                ),
            ],
        );

        extract_msix(&msix, &extracted).unwrap();

        let modules = extracted.join("app/resources/cua_node/bin/node_modules");
        assert!(modules.join("@oai/sky/package.json").is_file());
        assert!(modules
            .join("@statsig/client-core/src/$_StatsigGlobal.js")
            .is_file());
        assert!(!modules.join("%40oai").exists());
        assert!(!modules.join("%40statsig").exists());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[ignore = "requires CODEX_REAL_MSIX pointing at a current official Codex MSIX"]
    fn real_msix_uses_computer_use_logical_paths() {
        let msix = std::env::var_os("CODEX_REAL_MSIX")
            .map(PathBuf::from)
            .expect("set CODEX_REAL_MSIX to an official Codex MSIX");
        assert!(msix.is_file(), "MSIX does not exist: {}", msix.display());
        #[cfg(windows)]
        {
            let signature = crate::authenticode::verify_openai_authenticode(&msix)
                .expect("verify official MSIX Authenticode signature");
            assert!(
                signature.is_valid_openai(),
                "MSIX is not validly signed by the trusted OpenAI publisher: status={} subject={}",
                signature.status,
                signature.subject
            );
        }
        let root = temp_test_dir("real-msix-logical-paths");
        let prepared = prepare_portable_payload(&msix, &root).unwrap();
        let modules = prepared
            .payload_dir
            .join("app/resources/cua_node/bin/node_modules");

        assert!(modules
            .join("@oai/sky/dist/project/cua/sky_js/src/targets/windows/internal/computer_use_client_base.js")
            .is_file());
        assert!(modules
            .join("@statsig/client-core/src/$_StatsigGlobal.js")
            .is_file());
        assert!(!modules.join("%40oai").exists());
        assert!(!modules.join("%40statsig").exists());
        assert!(!modules
            .join("@statsig/client-core/src/%24_StatsigGlobal.js")
            .exists());

        #[cfg(windows)]
        {
            let node_bin = prepared.payload_dir.join("app/resources/cua_node/bin");
            let node = node_bin.join("node.exe");
            assert!(node.is_file(), "bundled cua_node is missing");
            let output = std::process::Command::new(&node)
                .arg("-e")
                .arg(
                    r#"const root=process.argv[1]; console.log(require.resolve('@oai/sky',{paths:[root]})); console.log(require.resolve('@statsig/client-core',{paths:[root]}));"#,
                )
                .arg(&node_bin)
                .output()
                .expect("launch bundled cua_node");
            assert!(
                output.status.success(),
                "bundled cua_node module resolution failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            let stdout = String::from_utf8_lossy(&output.stdout);
            assert!(stdout.contains("@oai"), "unexpected resolution: {stdout}");
            assert!(
                stdout.contains("@statsig"),
                "unexpected resolution: {stdout}"
            );
        }

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[ignore = "requires CODEX_REAL_MSIX pointing at an official Codex MSIX"]
    fn real_historical_msix_block_map_round_trips() {
        let msix = std::env::var_os("CODEX_REAL_MSIX")
            .map(PathBuf::from)
            .expect("set CODEX_REAL_MSIX to an official Codex MSIX");
        assert!(msix.is_file(), "MSIX does not exist: {}", msix.display());
        let root = temp_test_dir("real-historical-msix");
        let prepared = prepare_portable_payload(&msix, &root).unwrap();

        assert!(prepared.payload_dir.join("AppxManifest.xml").is_file());
        assert!(
            installed_app_exe(&prepared.payload_dir).is_some(),
            "portable entry executable was not reconstructed"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_encoded_separator_and_traversal_components() {
        for (name, expected) in [
            ("app/resources/%2Fescape.txt", "encoded path separator"),
            ("app/resources/%5Cescape.txt", "encoded path separator"),
            (
                "app/resources/%2E%2E/escape.txt",
                "unsafe MSIX path component",
            ),
        ] {
            let root = temp_test_dir("encoded-traversal");
            let msix = root.join("codex.msix");
            let extracted = root.join("extracted");
            fs::create_dir_all(&extracted).unwrap();
            write_block_mapped_msix(
                &msix,
                &[(r"app\resources\escape.txt", 6)],
                &[(name, b"escape")],
            );

            let err = extract_msix(&msix, &extracted).unwrap_err();
            assert!(
                err.to_string().contains(expected),
                "unexpected error for {name}: {err}"
            );
            assert!(!root.join("escape.txt").exists());
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn rejects_decoded_output_collisions_instead_of_overwriting() {
        let root = temp_test_dir("decoded-collision");
        let msix = root.join("codex.msix");
        let extracted = root.join("extracted");
        fs::create_dir_all(&extracted).unwrap();
        write_block_mapped_msix(
            &msix,
            &[(r"app\node_modules\@oai\file.js", 4)],
            &[
                ("app/node_modules/%40oai/file.js", b"safe"),
                ("app/node_modules/@oai/file.js", b"evil"),
            ],
        );

        let err = extract_msix(&msix, &extracted).unwrap_err();
        assert!(
            err.to_string().contains("collide on Windows"),
            "unexpected error: {err}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_percent_encoded_payload_without_a_block_map() {
        let root = temp_test_dir("encoded-without-block-map");
        let msix = root.join("codex.msix");
        let extracted = root.join("extracted");
        fs::create_dir_all(&extracted).unwrap();
        let file = fs::File::create(&msix).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts = SimpleFileOptions::default();
        zip.start_file("AppxManifest.xml", opts).unwrap();
        zip.write_all(
            br#"<Package><Identity Name="OpenAI.Codex" Publisher="CN=X" Version="1.0.0.0" ProcessorArchitecture="x64" /></Package>"#,
        )
        .unwrap();
        zip.start_file("app/node_modules/%40oai/file.js", opts)
            .unwrap();
        zip.write_all(b"payload").unwrap();
        zip.finish().unwrap();

        let err = extract_msix(&msix, &extracted).unwrap_err();
        assert!(
            err.to_string().contains("without AppxBlockMap.xml"),
            "unexpected error: {err}"
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rejects_missing_or_size_mismatched_block_map_payloads() {
        for (files, expected) in [
            (
                vec![("app/present.txt", b"data".as_slice())],
                "missing 1 payload file",
            ),
            (
                vec![("app/missing.txt", b"short".as_slice())],
                "payload size disagrees",
            ),
        ] {
            let root = temp_test_dir("block-map-integrity");
            let msix = root.join("codex.msix");
            let extracted = root.join("extracted");
            fs::create_dir_all(&extracted).unwrap();
            write_block_mapped_msix(&msix, &[(r"app\missing.txt", 10)], &files);

            let err = extract_msix(&msix, &extracted).unwrap_err();
            assert!(
                err.to_string().contains(expected),
                "unexpected error: {err}"
            );
            let _ = fs::remove_dir_all(root);
        }
    }

    #[test]
    fn installs_portable_payload_from_msix_layout() {
        let root = std::env::temp_dir().join(format!("codex-portable-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let msix = root.join("codex.msix");
        let install_root = root.join("Codex");
        write_fake_msix(&msix);

        let report = install_portable_from_msix_inner(&msix, &install_root, false, false).unwrap();
        assert!(report.success);
        assert!(install_root.join("app/Codex.exe").exists());
        assert!(install_root.join("app/resources/app.asar").exists());
        assert!(install_root.join("AppxManifest.xml").exists());
        assert_eq!(report.version, "26.602.3474.0");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn installs_rebranded_portable_payload_by_manifest_entry() {
        let root = std::env::temp_dir().join(format!(
            "codex-portable-rebrand-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let msix = root.join("codex.msix");
        let install_root = root.join("Codex");
        write_fake_rebranded_msix(&msix);

        let report = install_portable_from_msix_inner(&msix, &install_root, false, false).unwrap();
        assert!(report.success);
        // Both official binaries and their resources remain intact in app/.
        assert_eq!(
            fs::read(install_root.join("app/ChatGPT.exe")).unwrap(),
            b"fake entry exe"
        );
        assert_eq!(
            fs::read(install_root.join("app/Codex.exe")).unwrap(),
            b"legacy compat exe"
        );
        assert!(install_root.join("app/resources/app.asar").exists());
        assert_eq!(report.version, "26.707.3748.0");
        // The entry executable resolves to ChatGPT.exe, not the legacy binary.
        assert_eq!(
            installed_app_exe(&install_root),
            Some(install_root.join("app/ChatGPT.exe"))
        );
        assert_eq!(
            report.executable_path.as_deref(),
            Some(
                install_root
                    .join("app")
                    .join("ChatGPT.exe")
                    .to_string_lossy()
                    .as_ref()
            )
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn installed_app_exe_prefers_manifest_then_known_names() {
        let root =
            std::env::temp_dir().join(format!("codex-portable-exe-probe-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        // No manifest, legacy layout → Codex.exe via known-name probe.
        fs::write(root.join("Codex.exe"), b"legacy").unwrap();
        assert_eq!(installed_app_exe(&root), Some(root.join("Codex.exe")));

        // Both names present without a manifest → the newer entry name wins.
        fs::write(root.join("ChatGPT.exe"), b"entry").unwrap();
        assert_eq!(installed_app_exe(&root), Some(root.join("ChatGPT.exe")));

        // A manifest declaring the legacy entry overrides the probe order.
        fs::write(
            root.join("AppxManifest.xml"),
            br#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Identity Name="OpenAI.Codex" Publisher="CN=X" Version="1.0.0.0" ProcessorArchitecture="x64" />
  <Applications><Application Id="App" Executable="app\Codex.exe" /></Applications>
</Package>"#,
        )
        .unwrap();
        assert_eq!(installed_app_exe(&root), Some(root.join("Codex.exe")));

        // A declared-but-missing entry means the install is broken: never
        // silently fall back to a leftover binary that happens to exist.
        fs::write(
            root.join("AppxManifest.xml"),
            br#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Identity Name="OpenAI.Codex" Publisher="CN=X" Version="1.0.0.0" ProcessorArchitecture="x64" />
  <Applications><Application Id="App" Executable="app\Gone.exe" /></Applications>
</Package>"#,
        )
        .unwrap();
        assert_eq!(installed_app_exe(&root), None);

        // An existing nested payload is authoritative, even if quarantined:
        // a root launcher alias must never be mistaken for the real app.
        fs::create_dir_all(root.join("app")).unwrap();
        fs::write(root.join("app/Gone.exe"), b"nested entry").unwrap();
        assert_eq!(installed_app_exe(&root), Some(root.join("app/Gone.exe")));
        for config in ["", "broken", "ChatGPT.exe\nrequire-cli=broken"] {
            fs::write(
                root.join(crate::portable_command::LAUNCH_TARGET_NAME),
                config,
            )
            .unwrap();
            assert_eq!(installed_app_exe(&root), Some(root.join("app/Gone.exe")));
        }
        fs::remove_file(root.join("app/Gone.exe")).unwrap();
        assert_eq!(installed_app_exe(&root), None);
        fs::write(
            root.join(crate::portable_command::LAUNCH_TARGET_NAME),
            "app/Gone.exe",
        )
        .unwrap();
        fs::remove_dir(root.join("app")).unwrap();
        assert_eq!(installed_app_exe(&root), None);

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn install_fails_when_declared_entry_is_missing_from_payload() {
        // Manifest declares app/ChatGPT.exe but the payload only carries the
        // legacy app/Codex.exe (e.g. the entry was quarantined). Selecting the
        // leftover binary would health-check the wrong thing — must error out.
        let root = std::env::temp_dir().join(format!(
            "codex-portable-missing-entry-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let msix = root.join("codex.msix");
        {
            let file = fs::File::create(&msix).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let opts = SimpleFileOptions::default();
            zip.start_file("AppxManifest.xml", opts).unwrap();
            zip.write_all(
                br#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Identity Name="OpenAI.Codex" Publisher="CN=OpenAI OpCo, LLC" Version="26.707.3748.0" ProcessorArchitecture="x64" />
  <Applications>
    <Application Id="App" Executable="app/ChatGPT.exe" EntryPoint="Windows.FullTrustApplication" />
  </Applications>
</Package>"#,
            )
            .unwrap();
            zip.start_file("app/Codex.exe", opts).unwrap();
            zip.write_all(b"legacy only").unwrap();
            zip.finish().unwrap();
        }

        let install_root = root.join("Codex");
        let err = install_portable_from_msix_inner(&msix, &install_root, false, false).unwrap_err();
        assert!(
            err.to_string().contains("missing from the payload"),
            "unexpected error: {err}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn replaces_existing_portable_and_removes_rollback_backup() {
        let root = std::env::temp_dir().join(format!(
            "codex-portable-replace-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let msix = root.join("codex.msix");
        let install_root = root.join("Codex");
        write_fake_msix(&msix);

        fs::create_dir_all(install_root.join("resources")).unwrap();
        fs::write(install_root.join("Codex.exe"), b"old exe").unwrap();
        fs::write(install_root.join("resources/old-asar.bin"), b"old").unwrap();

        let report = install_portable_from_msix_inner(&msix, &install_root, false, false).unwrap();
        assert!(report.success);
        assert!(report.backup_path.is_none());
        assert!(!fs::read_dir(&root).unwrap().any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with("Codex.rollback")));
        assert!(!root.join(PORTABLE_STAGING_DIR_NAME).exists());
        assert!(!install_root.join("resources/old-asar.bin").exists());
        assert!(install_root.join("app/resources/app.asar").exists());
        assert!(install_root.join(PORTABLE_FILES_MANIFEST_NAME).is_file());

        let _ = fs::remove_dir_all(&root);
    }

    #[cfg(windows)]
    #[test]
    fn health_check_detects_immediate_exit_entry() {
        // whoami.exe exits instantly — models a broken payload that CreateProcess
        // accepts then immediately dies. The health check must fail closed.
        let root =
            std::env::temp_dir().join(format!("codex-portable-liveness-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let windir = std::env::var_os("WINDIR").unwrap_or_else(|| "C:\\Windows".into());
        let whoami = PathBuf::from(windir).join("System32").join("whoami.exe");
        if !whoami.is_file() {
            let _ = fs::remove_dir_all(&root);
            return;
        }
        fs::copy(&whoami, root.join("ChatGPT.exe")).unwrap();
        fs::write(
            root.join("AppxManifest.xml"),
            br#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Identity Name="OpenAI.Codex" Publisher="CN=X" Version="1.0.0.0" ProcessorArchitecture="x64" />
  <Applications><Application Id="App" Executable="app\ChatGPT.exe" /></Applications>
</Package>"#,
        )
        .unwrap();

        let err = health_check_portable_install(&root, true, true, "").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("exited immediately"),
            "unexpected error: {msg}"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn restore_previous_install_removes_failed_payload() {
        let root = std::env::temp_dir().join(format!(
            "codex-portable-rollback-test-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let install_root = root.join("Codex");
        let backup = root.join("Codex.rollback-test");

        fs::create_dir_all(&backup).unwrap();
        fs::write(backup.join("old-marker.txt"), b"old").unwrap();
        fs::create_dir_all(&install_root).unwrap();
        fs::write(install_root.join("new-marker.txt"), b"new").unwrap();

        restore_previous_install(&install_root, &backup, true).unwrap();
        assert!(install_root.join("old-marker.txt").exists());
        assert!(!install_root.join("new-marker.txt").exists());
        assert!(!backup.exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fault_after_move_old_leaves_crash_window_for_recovery() {
        let root = std::env::temp_dir().join(format!(
            "codex-portable-fault-after-old-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let msix = root.join("codex.msix");
        let install_root = root.join("Codex");
        write_fake_msix(&msix);
        fs::create_dir_all(&install_root).unwrap();
        fs::write(install_root.join("old-marker.txt"), b"old").unwrap();

        inject_portable_fault(Some(PortableFault::AfterMoveOld));
        let err = install_portable_from_msix_inner(&msix, &install_root, false, false).unwrap_err();
        assert!(err.to_string().contains("after-move-old"));
        // Crash window: install missing, rollback backup present, staging payload present.
        assert!(!install_root.exists());
        let backup = fs::read_dir(&root)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("Codex.rollback-"))
            });
        assert!(backup.is_some(), "rollback backup must remain");
        assert!(backup.unwrap().join("old-marker.txt").exists());
        let staging_dir = root.join(PORTABLE_STAGING_DIR_NAME);
        let staged_run = fs::read_dir(&staging_dir)
            .expect("staging container must remain during AfterMoveOld crash window")
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .find(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with("portable-"))
            });
        assert!(
            staged_run.is_some_and(|run| run.join("payload/app/Codex.exe").is_file()),
            "staged payload must survive AfterMoveOld for startup recovery"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fault_before_move_old_leaves_install_intact() {
        let root = std::env::temp_dir().join(format!(
            "codex-portable-fault-before-old-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let msix = root.join("codex.msix");
        let install_root = root.join("Codex");
        write_fake_msix(&msix);
        fs::create_dir_all(&install_root).unwrap();
        fs::write(install_root.join("old-marker.txt"), b"old").unwrap();

        inject_portable_fault(Some(PortableFault::BeforeMoveOld));
        let err = install_portable_from_msix_inner(&msix, &install_root, false, false).unwrap_err();
        assert!(err.to_string().contains("before-move-old"));
        assert!(install_root.join("old-marker.txt").exists());
        assert!(
            !root.join(PORTABLE_STAGING_DIR_NAME).exists(),
            "RAII staging guard must remove work_dir and empty staging container"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn observer_sees_rename_boundaries() {
        let root =
            std::env::temp_dir().join(format!("codex-portable-observer-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let msix = root.join("codex.msix");
        let install_root = root.join("Codex");
        write_fake_msix(&msix);
        let mut kinds = Vec::new();
        install_portable_from_msix_with_observer(&msix, &install_root, false, false, &mut |b| {
            kinds.push(match b {
                PortableBoundary::BeforeMoveOld { .. } => "before-old",
                PortableBoundary::AfterMoveOld { .. } => "after-old",
                PortableBoundary::BeforeMoveNew { .. } => "before-new",
                PortableBoundary::AfterMoveNew { .. } => "after-new",
                PortableBoundary::BeforeRollback { .. } => "before-rollback",
                PortableBoundary::RollbackCompleted { .. } => "rollback-completed",
            });
            Ok(())
        })
        .unwrap();
        assert_eq!(
            kinds,
            ["before-old", "after-old", "before-new", "after-new"]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn successful_fresh_install_rollback_emits_absent_state_evidence() {
        let root = std::env::temp_dir().join(format!(
            "codex-portable-fresh-rollback-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let msix = root.join("codex.msix");
        let install_root = root.join("Codex");
        write_fake_msix(&msix);
        let mut kinds = Vec::new();

        inject_portable_fault(Some(PortableFault::AfterMoveNew));
        let err = install_portable_from_msix_with_observer(
            &msix,
            &install_root,
            false,
            false,
            &mut |boundary| {
                kinds.push(match boundary {
                    PortableBoundary::BeforeMoveOld { .. } => "before-old",
                    PortableBoundary::AfterMoveOld { .. } => "after-old",
                    PortableBoundary::BeforeMoveNew { .. } => "before-new",
                    PortableBoundary::AfterMoveNew { .. } => "after-new",
                    PortableBoundary::BeforeRollback { .. } => "before-rollback",
                    PortableBoundary::RollbackCompleted { .. } => "rollback-completed",
                });
                Ok(())
            },
        )
        .unwrap_err();

        assert!(err.to_string().contains("absent state was restored"));
        assert_eq!(
            kinds,
            [
                "before-old",
                "after-old",
                "before-new",
                "after-new",
                "before-rollback",
                "rollback-completed"
            ]
        );
        assert!(
            !install_root.exists(),
            "a fully rolled-back fresh install must be absent and safe to retry"
        );
        assert!(
            !fs::read_dir(&root).unwrap().any(|entry| entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("Codex.rollback-")),
            "a fresh rollback must not leave rollback material"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_upgrade_backup_never_reports_rollback_completion() {
        let root = std::env::temp_dir().join(format!(
            "codex-portable-missing-rollback-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let install_root = root.join("Codex");
        let backup = root.join("Codex.rollback-missing");
        fs::create_dir_all(&install_root).unwrap();
        fs::write(install_root.join("new-marker.txt"), b"new").unwrap();
        let mut boundaries = Vec::new();

        let err = rollback_install_error(
            &install_root,
            &backup,
            true,
            &mut |boundary| {
                boundaries.push(boundary);
                Ok(())
            },
            portable_fault_err("after-move-new"),
        );

        assert!(err.to_string().contains("rollback backup is missing"));
        assert_eq!(boundaries.len(), 1);
        assert!(matches!(
            boundaries[0],
            PortableBoundary::BeforeRollback {
                had_previous: true,
                ..
            }
        ));
        assert!(
            !boundaries
                .iter()
                .any(|boundary| matches!(boundary, PortableBoundary::RollbackCompleted { .. })),
            "failed rollback must not emit completion evidence"
        );
        assert!(
            install_root.join("new-marker.txt").exists(),
            "failed rollback must preserve the current tree for recovery"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn successful_upgrade_rollback_restores_old_tree_before_evidence() {
        let root = std::env::temp_dir().join(format!(
            "codex-portable-upgrade-rollback-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        let install_root = root.join("Codex");
        let backup = root.join("Codex.rollback-old");
        fs::create_dir_all(&install_root).unwrap();
        fs::write(install_root.join("new-marker.txt"), b"new").unwrap();
        fs::create_dir_all(&backup).unwrap();
        fs::write(backup.join("old-marker.txt"), b"old").unwrap();
        let mut boundaries = Vec::new();
        let mut saw_restored_tree = false;

        let err = rollback_install_error(
            &install_root,
            &backup,
            true,
            &mut |boundary| {
                match boundary {
                    PortableBoundary::BeforeRollback {
                        had_previous: true, ..
                    } => {
                        boundaries.push("before-rollback");
                        assert!(install_root.join("new-marker.txt").exists());
                        assert!(backup.join("old-marker.txt").exists());
                    }
                    PortableBoundary::RollbackCompleted {
                        had_previous: true, ..
                    } => {
                        boundaries.push("rollback-completed");
                        saw_restored_tree = install_root.join("old-marker.txt").exists()
                            && !install_root.join("new-marker.txt").exists();
                    }
                    other => panic!("unexpected rollback boundary: {other:?}"),
                }
                Ok(())
            },
            portable_fault_err("after-move-new"),
        );

        assert!(err.to_string().contains("previous install was restored"));
        assert_eq!(boundaries, ["before-rollback", "rollback-completed"]);
        assert!(
            saw_restored_tree,
            "evidence must follow the completed restore"
        );
        assert!(!backup.exists());

        let _ = fs::remove_dir_all(&root);
    }

    fn find_preserved_dirs(parent: &Path) -> Vec<PathBuf> {
        let mut dirs: Vec<PathBuf> = fs::read_dir(parent)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.starts_with(PRESERVED_FILES_DIR_PREFIX))
            })
            .collect();
        dirs.sort();
        dirs
    }

    #[test]
    fn format_utc_timestamp_formats_epoch_and_leap_day() {
        assert_eq!(format_utc_timestamp(0), "19700101-000000");
        // 2024-02-29 23:59:59 UTC
        assert_eq!(format_utc_timestamp(1_709_251_199), "20240229-235959");
    }

    #[test]
    fn clean_nested_upgrade_has_no_false_positives_even_when_old_files_are_dropped() {
        let parent = temp_test_dir("clean-nested-upgrade");
        let msix_v1 = parent.join("v1.msix");
        let msix_v2 = parent.join("v2.msix");
        let install_root = parent.join("Codex");

        write_block_mapped_msix(
            &msix_v1,
            &[
                (r"app\ChatGPT.exe", 6),
                (r"app\old-helper.dll", 7),
                (r"app\resources\app.asar", 5),
            ],
            &[
                ("app/ChatGPT.exe", b"v1-exe"),
                ("app/old-helper.dll", b"old-dll"),
                ("app/resources/app.asar", b"v1-as"),
            ],
        );
        write_fake_rebranded_msix(&msix_v2);

        install_portable_from_msix_inner(&msix_v1, &install_root, false, false).unwrap();
        assert!(install_root.join(PORTABLE_FILES_MANIFEST_NAME).is_file());
        assert!(install_root.join("app/old-helper.dll").is_file());

        let report = install_portable_from_msix_inner(&msix_v2, &install_root, false, false).unwrap();
        assert!(report.success);
        assert!(report.backup_path.is_none());
        assert!(
            find_preserved_dirs(&parent).is_empty(),
            "clean upgrade must not create a preserved-files directory"
        );
        assert!(!parent.join(PORTABLE_STAGING_DIR_NAME).exists());
        assert!(!install_root.join("app/old-helper.dll").exists());
        assert!(install_root.join("app/ChatGPT.exe").is_file());
        assert!(
            !report
                .notes
                .iter()
                .any(|n| n.contains("Preserved") || n.contains("Moved")),
            "clean upgrade must not emit user-file preservation warnings: {:?}",
            report.notes
        );

        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn flat_to_nested_upgrade_preserves_user_projects_and_resolves_app_name_collision() {
        let parent = temp_test_dir("flat-to-nested-user-files");
        let install_root = parent.join("Codex");
        let msix = parent.join("new.msix");
        write_fake_rebranded_msix(&msix);

        // Pre-fix flat install (no codex-portable-files.txt) with user projects:
        // one at root (`my-repo/`) and one literally named `app/` that collides
        // with the new nested layout's `app/` directory.
        fs::create_dir_all(install_root.join("resources")).unwrap();
        fs::write(install_root.join("ChatGPT.exe"), b"old flat entry").unwrap();
        fs::write(install_root.join("resources/app.asar"), b"old asar").unwrap();
        fs::write(
            install_root.join(crate::portable_command::LAUNCH_TARGET_NAME),
            "ChatGPT.exe\nrequire-cli=0",
        )
        .unwrap();
        fs::create_dir_all(install_root.join("my-repo/src")).unwrap();
        fs::write(install_root.join("my-repo/src/main.rs"), b"fn main() {}").unwrap();
        fs::create_dir_all(install_root.join("app")).unwrap();
        fs::write(install_root.join("app/user-webapp.ts"), b"export const x = 1;").unwrap();

        let report = install_portable_from_msix_inner(&msix, &install_root, false, false).unwrap();
        assert!(report.success);
        assert!(report.backup_path.is_none());
        // Root project without collision stays in place so project paths remain intact.
        assert_eq!(
            fs::read(install_root.join("my-repo/src/main.rs")).unwrap(),
            b"fn main() {}"
        );
        // New nested official app directory is intact and not overwritten.
        assert_eq!(
            fs::read(install_root.join("app/ChatGPT.exe")).unwrap(),
            b"fake entry exe"
        );
        assert!(!install_root.join("app/user-webapp.ts").exists());
        // Colliding `app/` user folder was safely moved to sibling preserved dir.
        let preserved = find_preserved_dirs(&parent);
        assert_eq!(preserved.len(), 1, "expected one preserved dir: {preserved:?}");
        assert_eq!(
            fs::read(preserved[0].join("app/user-webapp.ts")).unwrap(),
            b"export const x = 1;"
        );
        assert!(!parent.join(PORTABLE_STAGING_DIR_NAME).exists());

        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn nested_to_nested_upgrade_preserves_root_app_and_deep_user_files() {
        let parent = temp_test_dir("nested-to-nested-user-files");
        let install_root = parent.join("Codex");
        let msix_v1 = parent.join("v1.msix");
        let msix_v2 = parent.join("v2.msix");
        write_fake_msix(&msix_v1);
        write_fake_rebranded_msix(&msix_v2);

        install_portable_from_msix_inner(&msix_v1, &install_root, false, false).unwrap();

        // User projects in all three locations: root, app/ top-level (previous CWD),
        // and inside an official subdirectory.
        fs::create_dir_all(install_root.join("root-project")).unwrap();
        fs::write(install_root.join("root-project/README.md"), b"root readme").unwrap();
        fs::create_dir_all(install_root.join("app/cwd-project/src")).unwrap();
        fs::write(
            install_root.join("app/cwd-project/src/lib.rs"),
            b"pub fn ok() {}",
        )
        .unwrap();
        fs::write(
            install_root.join("app/resources/personal-notes.txt"),
            b"do not delete",
        )
        .unwrap();

        let report = install_portable_from_msix_inner(&msix_v2, &install_root, false, false).unwrap();
        assert!(report.success);
        assert!(report.backup_path.is_none());

        // Root-level project restored in-place.
        assert_eq!(
            fs::read(install_root.join("root-project/README.md")).unwrap(),
            b"root readme"
        );
        // `app/`-level and deeper user files moved out of `app/` into sibling preserved dir.
        assert!(!install_root.join("app/cwd-project").exists());
        assert!(!install_root.join("app/resources/personal-notes.txt").exists());
        let preserved = find_preserved_dirs(&parent);
        assert_eq!(preserved.len(), 1);
        assert_eq!(
            fs::read(preserved[0].join("app/cwd-project/src/lib.rs")).unwrap(),
            b"pub fn ok() {}"
        );
        assert_eq!(
            fs::read(preserved[0].join("app/resources/personal-notes.txt")).unwrap(),
            b"do not delete"
        );
        assert!(report.notes.iter().any(|n| n.contains("root-project")));
        assert!(report.notes.iter().any(|n| n.contains("app/cwd-project")));

        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn legacy_nested_upgrade_without_manifest_preserves_root_and_app_user_files() {
        let parent = temp_test_dir("legacy-nested-upgrade");
        let install_root = parent.join("Codex");
        let msix = parent.join("new.msix");
        write_fake_rebranded_msix(&msix);

        // Pre-fix nested install: has `app/` + `AppxManifest.xml` + launcher config,
        // NO `codex-portable-files.txt`.
        fs::create_dir_all(install_root.join("app/resources")).unwrap();
        fs::write(install_root.join("app/ChatGPT.exe"), b"old nested exe").unwrap();
        fs::write(install_root.join("app/resources/app.asar"), b"old asar").unwrap();
        fs::write(
            install_root.join(crate::portable_command::LAUNCH_TARGET_NAME),
            "app/ChatGPT.exe\nrequire-cli=0",
        )
        .unwrap();
        fs::create_dir_all(install_root.join("user-root-repo")).unwrap();
        fs::write(install_root.join("user-root-repo/a.txt"), b"alpha").unwrap();
        fs::create_dir_all(install_root.join("app/user-app-repo")).unwrap();
        fs::write(install_root.join("app/user-app-repo/b.txt"), b"beta").unwrap();

        let report = install_portable_from_msix_inner(&msix, &install_root, false, false).unwrap();
        assert!(report.success);
        assert!(report.backup_path.is_none());
        assert_eq!(
            fs::read(install_root.join("user-root-repo/a.txt")).unwrap(),
            b"alpha"
        );
        let preserved = find_preserved_dirs(&parent);
        assert_eq!(preserved.len(), 1);
        assert_eq!(
            fs::read(preserved[0].join("app/user-app-repo/b.txt")).unwrap(),
            b"beta"
        );

        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn consecutive_preserved_migrations_never_overwrite_existing_preserved_dir() {
        let parent = temp_test_dir("preserved-collision");
        let install_root = parent.join("Codex");
        let msix = parent.join("v.msix");
        write_fake_rebranded_msix(&msix);

        install_portable_from_msix_inner(&msix, &install_root, false, false).unwrap();
        fs::create_dir_all(install_root.join("app/first-project")).unwrap();
        fs::write(install_root.join("app/first-project/one.txt"), b"one").unwrap();
        install_portable_from_msix_inner(&msix, &install_root, false, false).unwrap();

        fs::create_dir_all(install_root.join("app/second-project")).unwrap();
        fs::write(install_root.join("app/second-project/two.txt"), b"two").unwrap();
        install_portable_from_msix_inner(&msix, &install_root, false, false).unwrap();

        let preserved = find_preserved_dirs(&parent);
        assert_eq!(
            preserved.len(),
            2,
            "second upgrade must allocate a distinct preserved dir: {preserved:?}"
        );
        assert!(preserved
            .iter()
            .any(|d| fs::read(d.join("app/first-project/one.txt")).ok().as_deref() == Some(b"one")));
        assert!(preserved
            .iter()
            .any(|d| fs::read(d.join("app/second-project/two.txt")).ok().as_deref() == Some(b"two")));

        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn migration_failure_retains_full_backup_without_deleting_user_files() {
        let parent = temp_test_dir("migration-failure-retains-backup");
        let install_root = parent.join("Codex");
        let backup = parent.join("Codex.rollback-fail");
        let msix = parent.join("v.msix");
        write_fake_rebranded_msix(&msix);

        install_portable_from_msix_inner(&msix, &install_root, false, false).unwrap();
        install_portable_from_msix_inner(&msix, &backup, false, false).unwrap();
        fs::create_dir_all(backup.join("app/locked-repo")).unwrap();
        fs::write(backup.join("app/locked-repo/secret.txt"), b"precious").unwrap();

        let outcome = cleanup_portable_backup_preserving_user_files_with_mover(
            &backup,
            &install_root,
            |_src, _dst| Err(io::Error::new(io::ErrorKind::PermissionDenied, "simulated lock")),
        );

        assert_eq!(outcome.retained_backup.as_deref(), Some(backup.as_path()));
        assert_eq!(
            fs::read(backup.join("app/locked-repo/secret.txt")).unwrap(),
            b"precious",
            "backup and user files must stay completely intact when migration fails"
        );
        assert!(
            find_preserved_dirs(&parent).is_empty(),
            "empty preserved dir must be cleaned up when migration fails before moving anything"
        );
        assert!(outcome
            .notes
            .iter()
            .any(|n| n.contains("kept previous backup")));

        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn uninstall_portable_removes_clean_install_and_preserves_user_files_when_present() {
        let parent = temp_test_dir("uninstall-user-files");
        let msix = parent.join("v.msix");
        write_fake_rebranded_msix(&msix);

        // 1. Clean install: entire directory is removed.
        let clean_root = parent.join("Codex-Clean");
        install_portable_from_msix_inner(&msix, &clean_root, false, false).unwrap();
        let clean_report = uninstall_portable(&clean_root, false).unwrap();
        assert!(clean_report.success);
        assert!(clean_report.removed_files);
        assert!(!clean_root.exists());

        // 2. Install containing user projects in both root and app/:
        //    official files are removed while user projects remain untouched.
        let mixed_root = parent.join("Codex-Mixed");
        install_portable_from_msix_inner(&msix, &mixed_root, false, false).unwrap();
        fs::create_dir_all(mixed_root.join("my-root-project")).unwrap();
        fs::write(mixed_root.join("my-root-project/code.rs"), b"root").unwrap();
        fs::create_dir_all(mixed_root.join("app/my-app-project")).unwrap();
        fs::write(mixed_root.join("app/my-app-project/index.js"), b"app").unwrap();

        let mixed_report = uninstall_portable(&mixed_root, false).unwrap();
        assert!(mixed_report.success);
        assert!(!mixed_report.partial);
        assert!(mixed_report.removed_files);
        assert_eq!(installed_app_exe(&mixed_root), None);
        assert!(!mixed_root.join("app/ChatGPT.exe").exists());
        assert!(!mixed_root.join("AppxManifest.xml").exists());
        assert!(!mixed_root.join(PORTABLE_FILES_MANIFEST_NAME).exists());
        assert_eq!(
            fs::read(mixed_root.join("my-root-project/code.rs")).unwrap(),
            b"root"
        );
        assert_eq!(
            fs::read(mixed_root.join("app/my-app-project/index.js")).unwrap(),
            b"app"
        );
        assert!(mixed_report
            .notes
            .iter()
            .any(|n| n.contains("Preserved 2 non-official user item(s)")));

        // 3. Legacy install (without codex-portable-files.txt) also preserves user files.
        let legacy_root = parent.join("Codex-Legacy");
        fs::create_dir_all(legacy_root.join("app/resources")).unwrap();
        fs::write(legacy_root.join("app/ChatGPT.exe"), b"legacy exe").unwrap();
        fs::write(legacy_root.join("app/resources/app.asar"), b"legacy asar").unwrap();
        fs::write(
            legacy_root.join(crate::portable_command::LAUNCH_TARGET_NAME),
            "app/ChatGPT.exe\nrequire-cli=0",
        )
        .unwrap();
        fs::create_dir_all(legacy_root.join("app/my-legacy-project")).unwrap();
        fs::write(legacy_root.join("app/my-legacy-project/keep.txt"), b"keep").unwrap();

        let legacy_report = uninstall_portable(&legacy_root, false).unwrap();
        assert!(legacy_report.success);
        assert!(legacy_report.removed_files);
        assert!(!legacy_root.join("app/ChatGPT.exe").exists());
        assert!(!legacy_root.join("app/resources").exists());
        assert_eq!(
            fs::read(legacy_root.join("app/my-legacy-project/keep.txt")).unwrap(),
            b"keep"
        );

        let _ = fs::remove_dir_all(&parent);
    }

    #[cfg(unix)]
    #[test]
    fn upgrade_and_uninstall_never_follow_symlinks() {
        use std::os::unix::fs::symlink;

        let parent = temp_test_dir("symlink-safety");
        let external = parent.join("ExternalRepo");
        fs::create_dir_all(&external).unwrap();
        fs::write(external.join("important.txt"), b"must survive").unwrap();

        let install_root = parent.join("Codex");
        let msix = parent.join("v.msix");
        write_fake_rebranded_msix(&msix);
        install_portable_from_msix_inner(&msix, &install_root, false, false).unwrap();

        symlink(&external, install_root.join("app/linked-project")).unwrap();

        // Upgrade moves the symlink itself into preserved dir without touching external target.
        let report = install_portable_from_msix_inner(&msix, &install_root, false, false).unwrap();
        assert!(report.success);
        assert_eq!(
            fs::read(external.join("important.txt")).unwrap(),
            b"must survive"
        );
        let preserved = find_preserved_dirs(&parent);
        assert_eq!(preserved.len(), 1);
        assert!(fs::symlink_metadata(preserved[0].join("app/linked-project"))
            .unwrap()
            .file_type()
            .is_symlink());

        // Uninstall also leaves a symlink and its external target intact.
        symlink(&external, install_root.join("linked-root")).unwrap();
        let uninstall = uninstall_portable(&install_root, false).unwrap();
        assert!(uninstall.success);
        assert!(fs::symlink_metadata(install_root.join("linked-root"))
            .unwrap()
            .file_type()
            .is_symlink());
        assert_eq!(
            fs::read(external.join("important.txt")).unwrap(),
            b"must survive"
        );

        let _ = fs::remove_dir_all(&parent);
    }

    #[test]
    fn early_install_failures_clean_portable_staging_and_empty_parent() {
        let parent = temp_test_dir("raii-staging-cleanup");
        let install_root = parent.join("Codex");
        let staging_parent = parent.join(PORTABLE_STAGING_DIR_NAME);

        // 1. Failure inside prepare_portable_payload (corrupt MSIX) cleans work_dir + empty parent.
        let corrupt_msix = parent.join("corrupt.msix");
        fs::write(&corrupt_msix, b"not a zip").unwrap();
        assert!(install_portable_from_msix_inner(&corrupt_msix, &install_root, false, false).is_err());
        assert!(
            !staging_parent.exists(),
            "corrupt MSIX failure must remove work_dir and empty .codex-app-manager-staging"
        );

        // 2. Observer failure at BeforeMoveOld cleans work_dir + empty parent.
        let valid_msix = parent.join("valid.msix");
        write_fake_rebranded_msix(&valid_msix);
        let err = install_portable_from_msix_with_observer(
            &valid_msix,
            &install_root,
            false,
            false,
            &mut |boundary| {
                if matches!(boundary, PortableBoundary::BeforeMoveOld { .. }) {
                    return Err(EngineError::Io("simulated pre-swap abort".to_string()));
                }
                Ok(())
            },
        )
        .unwrap_err();
        assert!(err.to_string().contains("simulated pre-swap abort"));
        assert!(!staging_parent.exists());

        // 3. Sibling entry inside `.codex-app-manager-staging` is never removed by non-recursive parent cleanup.
        let sibling = staging_parent.join("keep-sibling");
        fs::create_dir_all(&sibling).unwrap();
        fs::write(sibling.join("marker.txt"), b"keep").unwrap();
        assert!(install_portable_from_msix_inner(&corrupt_msix, &install_root, false, false).is_err());
        assert!(sibling.join("marker.txt").is_file());
        assert_eq!(
            fs::read_dir(&staging_parent).unwrap().count(),
            1,
            "failed run's portable-<uuid> must be removed while sibling stays intact"
        );

        let _ = fs::remove_dir_all(&parent);
    }

    #[cfg(windows)]
    #[test]
    fn apps_features_uninstall_script_removes_official_files_and_preserves_user_files() {
        let parent = temp_test_dir("ps-uninstall-preserves-user-files");
        let msix = parent.join("v.msix");
        write_fake_rebranded_msix(&msix);

        // 1. Clean install: snippet removes the entire directory.
        let clean_root = parent.join("Codex-Clean");
        install_portable_from_msix_inner(&msix, &clean_root, false, false).unwrap();
        run_powershell_with_limits(
            &portable_files_uninstall_ps_snippet(&clean_root),
            RunLimits::install(),
        )
        .unwrap();
        assert!(
            !clean_root.exists(),
            "clean install root must be removed by Apps & Features uninstall snippet"
        );

        // 2. Mixed install: official files removed, user files at root and app/ survive.
        let install_root = parent.join("Codex");
        install_portable_from_msix_inner(&msix, &install_root, false, false).unwrap();
        fs::create_dir_all(install_root.join("my-root-project")).unwrap();
        fs::write(install_root.join("my-root-project/main.rs"), b"keep root").unwrap();
        fs::create_dir_all(install_root.join("app/my-app-project")).unwrap();
        fs::write(install_root.join("app/my-app-project/app.ts"), b"keep app").unwrap();

        run_powershell_with_limits(
            &portable_files_uninstall_ps_snippet(&install_root),
            RunLimits::install(),
        )
        .unwrap();
        assert!(!install_root.join("app/ChatGPT.exe").exists());
        assert!(!install_root.join("AppxManifest.xml").exists());
        assert!(!install_root.join(PORTABLE_FILES_MANIFEST_NAME).exists());
        assert_eq!(
            fs::read(install_root.join("my-root-project/main.rs")).unwrap(),
            b"keep root"
        );
        assert_eq!(
            fs::read(install_root.join("app/my-app-project/app.ts")).unwrap(),
            b"keep app"
        );

        let _ = fs::remove_dir_all(&parent);
    }
}
