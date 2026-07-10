use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde_json::Value;

const MAX_ASAR_HEADER_BYTES: u32 = 16 * 1024 * 1024;

/// The Codex Electron app's npm package name — a package-level identity marker
/// that survives inside `resources/app.asar` of any unpacked payload. Verified
/// on the real 26.707.31428 MSIX and the 26.707.30751 macOS bundle (both
/// post-rebrand: `name` stayed `openai-codex-electron` while display names
/// moved to ChatGPT). Used to recognize manifest-less portable roots, where the
/// MSIX's package-level signature is not available and the inner executables
/// carry no embedded Authenticode.
pub const CODEX_ASAR_PACKAGE_NAME: &str = "openai-codex-electron";

#[derive(Debug, Deserialize)]
struct PackageJson {
    version: String,
    #[serde(default)]
    name: Option<String>,
}

pub fn read_codex_app_version_from_install_root(root: &Path) -> Option<String> {
    for candidate in app_asar_candidates(root) {
        if let Some(package) = read_package_json_from_asar(&candidate) {
            let version = package.version.trim();
            if !version.is_empty() {
                return Some(version.to_string());
            }
        }
    }
    None
}

/// The `name` field of the app payload's `package.json` (from `app.asar`),
/// e.g. `openai-codex-electron`. `None` when no asar/package.json is readable.
pub fn read_asar_package_name_from_install_root(root: &Path) -> Option<String> {
    for candidate in app_asar_candidates(root) {
        if let Some(package) = read_package_json_from_asar(&candidate) {
            if let Some(name) = package.name.as_deref().map(str::trim) {
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
    }
    None
}

fn app_asar_candidates(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![
        root.join("resources").join("app.asar"),
        root.join("VFS")
            .join("ProgramFilesX64")
            .join("Codex")
            .join("resources")
            .join("app.asar"),
        root.join("VFS")
            .join("ProgramFilesArm64")
            .join("Codex")
            .join("resources")
            .join("app.asar"),
    ];

    if out.iter().any(|candidate| candidate.is_file()) {
        return out;
    }

    if let Some(found) = find_app_asar(root, 0, 6) {
        if !out.iter().any(|candidate| candidate == &found) {
            out.push(found);
        }
    }
    out
}

fn find_app_asar(root: &Path, depth: usize, max_depth: usize) -> Option<PathBuf> {
    if depth > max_depth {
        return None;
    }
    let entries = std::fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.file_name().and_then(|name| name.to_str()) == Some("app.asar") {
            return Some(path);
        }
        if path.is_dir() {
            if let Some(found) = find_app_asar(&path, depth + 1, max_depth) {
                return Some(found);
            }
        }
    }
    None
}

pub fn read_codex_app_version_from_asar(path: &Path) -> Option<String> {
    let package = read_package_json_from_asar(path)?;
    let version = package.version.trim();
    (!version.is_empty()).then(|| version.to_string())
}

fn read_package_json_from_asar(path: &Path) -> Option<PackageJson> {
    let (mut file, header, data_offset) = read_asar_header(path).ok()?;
    let entry = find_asar_entry(&header, &["package.json"])?;
    let offset = asar_entry_offset(entry)?;
    let size = asar_entry_size(entry)?;
    let absolute_offset = data_offset.checked_add(offset)?;
    file.seek(SeekFrom::Start(absolute_offset)).ok()?;
    let mut bytes = vec![0; size.try_into().ok()?];
    file.read_exact(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn read_asar_header(path: &Path) -> Result<(File, Value, u64), ()> {
    let mut file = File::open(path).map_err(|_| ())?;
    let mut prefix = [0_u8; 8];
    file.read_exact(&mut prefix).map_err(|_| ())?;
    let first = u32::from_le_bytes(prefix[0..4].try_into().map_err(|_| ())?);
    let second = u32::from_le_bytes(prefix[4..8].try_into().map_err(|_| ())?);

    let (header_start, header_size) = if first == 4 {
        (8_u64, second)
    } else {
        (4_u64, first)
    };
    if header_size == 0 || header_size > MAX_ASAR_HEADER_BYTES {
        return Err(());
    }

    file.seek(SeekFrom::Start(header_start)).map_err(|_| ())?;
    let mut header_bytes = vec![0; header_size.try_into().map_err(|_| ())?];
    file.read_exact(&mut header_bytes).map_err(|_| ())?;
    let header = parse_header_json(&header_bytes).ok_or(())?;
    Ok((file, header, header_start + u64::from(header_size)))
}

fn parse_header_json(bytes: &[u8]) -> Option<Value> {
    for (idx, byte) in bytes.iter().enumerate() {
        if *byte != b'{' {
            continue;
        }
        let mut de = serde_json::Deserializer::from_slice(&bytes[idx..]);
        if let Ok(value) = Value::deserialize(&mut de) {
            return Some(value);
        }
    }
    None
}

fn find_asar_entry<'a>(header: &'a Value, components: &[&str]) -> Option<&'a Value> {
    let mut files = header.get("files")?;
    for (idx, component) in components.iter().enumerate() {
        let node = files.get(*component)?;
        if idx == components.len() - 1 {
            return Some(node);
        }
        files = node.get("files")?;
    }
    None
}

fn asar_entry_offset(entry: &Value) -> Option<u64> {
    entry
        .get("offset")?
        .as_str()
        .and_then(|offset| offset.parse().ok())
}

fn asar_entry_size(entry: &Value) -> Option<u64> {
    entry.get("size")?.as_u64()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_root_package_json_version_from_asar() {
        let dir = std::env::temp_dir().join(format!("codex-asar-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let asar = dir.join("app.asar");
        write_test_asar(&asar, br#"{"version":"26.623.42026","name":"Codex"}"#);

        assert_eq!(
            read_codex_app_version_from_asar(&asar).as_deref(),
            Some("26.623.42026")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn finds_version_in_common_install_layout() {
        let dir =
            std::env::temp_dir().join(format!("codex-install-version-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let resources = dir.join("resources");
        std::fs::create_dir_all(&resources).unwrap();
        write_test_asar(
            &resources.join("app.asar"),
            br#"{"version":"26.623.42026"}"#,
        );

        assert_eq!(
            read_codex_app_version_from_install_root(&dir).as_deref(),
            Some("26.623.42026")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn reads_asar_package_name_from_install_root() {
        let dir =
            std::env::temp_dir().join(format!("codex-asar-name-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let resources = dir.join("resources");
        std::fs::create_dir_all(&resources).unwrap();
        write_test_asar(
            &resources.join("app.asar"),
            br#"{"version":"26.707.31428","name":"openai-codex-electron"}"#,
        );

        assert_eq!(
            read_asar_package_name_from_install_root(&dir).as_deref(),
            Some(CODEX_ASAR_PACKAGE_NAME)
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}

// Shared with sys.rs's manifest-less portable detection tests.
#[cfg(test)]
pub(crate) fn write_test_asar(path: &Path, package_json: &[u8]) {
    let header_json = format!(
        r#"{{"files":{{"package.json":{{"size":{},"offset":"0"}}}}}}"#,
        package_json.len()
    );
    let mut header = Vec::new();
    header.extend_from_slice(&0_u32.to_le_bytes());
    header.extend_from_slice(header_json.as_bytes());
    while header.len() % 4 != 0 {
        header.push(0);
    }

    let mut out = Vec::new();
    out.extend_from_slice(&4_u32.to_le_bytes());
    out.extend_from_slice(&(header.len() as u32).to_le_bytes());
    out.extend_from_slice(&header);
    out.extend_from_slice(package_json);
    std::fs::write(path, out).unwrap();
}
