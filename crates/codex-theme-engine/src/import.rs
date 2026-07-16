//! `.codexskin` import: a theme package zipped with `theme.json` at the
//! archive root. Extraction is paranoid (zip-slip, size/entry caps), the
//! extracted tree must pass the full `load_theme` validation, and the final
//! placement into `<themes_root>/<id>` is transactional — an existing install
//! of the same id is swapped out and restored on any failure.

use std::io::Read;
use std::path::{Component, Path, PathBuf};

use crate::theme::{load_theme, summarize, ThemeSummary};
use crate::{Result, ThemeEngineError};

/// Caps chosen far above any real package (biggest studio theme ≈ 3 MB
/// zipped, ~40 entries) but low enough to shrug off a zip bomb.
const MAX_ARCHIVE_BYTES: u64 = 50 * 1024 * 1024;
const MAX_UNPACKED_BYTES: u64 = 100 * 1024 * 1024;
const MAX_ENTRIES: usize = 500;

fn err(message: impl Into<String>) -> ThemeEngineError {
    ThemeEngineError::Theme(message.into())
}

/// A zip entry name is accepted only as a plain relative path — no roots, no
/// parent hops, no drive prefixes (`ZipFile::enclosed_name` plus belt and
/// braces for portability).
fn safe_entry_path(name: &str) -> Option<PathBuf> {
    let path = Path::new(name);
    if path
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
    {
        Some(path.to_path_buf())
    } else {
        None
    }
}

/// Import a `.codexskin` archive into `themes_root`, returning the installed
/// theme's summary. The archive must carry `theme.json` at its root.
pub fn import_codexskin(archive_path: &Path, themes_root: &Path) -> Result<ThemeSummary> {
    let archive_meta = std::fs::metadata(archive_path)
        .map_err(|e| err(format!("无法读取主题包: {e}")))?;
    if archive_meta.len() > MAX_ARCHIVE_BYTES {
        return Err(err("主题包超过大小上限 (50MB)"));
    }
    let file =
        std::fs::File::open(archive_path).map_err(|e| err(format!("无法打开主题包: {e}")))?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| err(format!("主题包不是有效的 zip: {e}")))?;
    if archive.len() > MAX_ENTRIES {
        return Err(err("主题包条目数超过上限"));
    }
    if archive.by_name("theme.json").is_err() {
        return Err(err(
            "主题包缺少根级 theme.json（.codexskin 的内容必须位于压缩包根目录）",
        ));
    }

    std::fs::create_dir_all(themes_root)
        .map_err(|e| err(format!("无法创建主题目录: {e}")))?;
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let staging = themes_root.join(format!(".import-{}-{nonce}", std::process::id()));
    let outcome = extract_and_place(&mut archive, &staging, themes_root);
    let _ = std::fs::remove_dir_all(&staging);
    outcome
}

fn extract_and_place(
    archive: &mut zip::ZipArchive<std::fs::File>,
    staging: &Path,
    themes_root: &Path,
) -> Result<ThemeSummary> {
    std::fs::create_dir_all(staging).map_err(|e| err(format!("创建临时目录失败: {e}")))?;

    let mut unpacked: u64 = 0;
    for index in 0..archive.len() {
        let entry = archive
            .by_index(index)
            .map_err(|e| err(format!("读取压缩包条目失败: {e}")))?;
        let Some(relative) = entry
            .enclosed_name()
            .and_then(|p| safe_entry_path(p.to_string_lossy().as_ref()))
        else {
            return Err(err(format!("主题包含非法路径条目: {}", entry.name())));
        };
        if entry.is_dir() {
            std::fs::create_dir_all(staging.join(&relative))
                .map_err(|e| err(format!("解压目录失败: {e}")))?;
            continue;
        }
        unpacked = unpacked.saturating_add(entry.size());
        if unpacked > MAX_UNPACKED_BYTES {
            return Err(err("主题包解压后超过大小上限"));
        }
        let target = staging.join(&relative);
        if let Some(parent) = target.parent() {
            std::fs::create_dir_all(parent).map_err(|e| err(format!("解压目录失败: {e}")))?;
        }
        let mut contents = Vec::with_capacity(entry.size().min(8 * 1024 * 1024) as usize);
        entry
            .take(MAX_UNPACKED_BYTES)
            .read_to_end(&mut contents)
            .map_err(|e| err(format!("解压 {} 失败: {e}", relative.display())))?;
        std::fs::write(&target, contents)
            .map_err(|e| err(format!("写入 {} 失败: {e}", relative.display())))?;
    }

    // Full package validation before anything touches the live gallery.
    let theme = load_theme(staging)?;
    let id = theme.config.id.clone();

    // Transactional placement: park any existing install of this id, move the
    // new tree in, drop the parked copy only on success.
    let destination = themes_root.join(&id);
    let parked = themes_root.join(format!(".replaced-{id}-{}", std::process::id()));
    let had_previous = destination.exists();
    if had_previous {
        std::fs::rename(&destination, &parked)
            .map_err(|e| err(format!("暂存旧版本失败: {e}")))?;
    }
    match std::fs::rename(staging, &destination) {
        Ok(()) => {
            if had_previous {
                let _ = std::fs::remove_dir_all(&parked);
            }
            let installed = load_theme(&destination)?;
            Ok(summarize(installed))
        }
        Err(error) => {
            if had_previous {
                let _ = std::fs::rename(&parked, &destination);
            }
            Err(err(format!("安装主题失败: {error}")))
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
        let file = std::fs::File::create(path).unwrap();
        let mut writer = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Deflated);
        for (name, bytes) in entries {
            writer.start_file(*name, options).unwrap();
            writer.write_all(bytes).unwrap();
        }
        writer.finish().unwrap();
    }

    fn minimal_entries(id: &str) -> Vec<(String, Vec<u8>)> {
        vec![
            (
                "theme.json".to_string(),
                format!(
                    r##"{{"schemaVersion":2,"id":"{id}","name":"T","version":"1.0.0",
                        "colors":{{"accent":"#abc"}},
                        "previews":["previews/home.webp"]}}"##
                )
                .into_bytes(),
            ),
            ("theme.css".to_string(), b"html.codex-theme-studio {}\n".to_vec()),
            ("previews/home.webp".to_string(), vec![0x52, 0x49, 0x46, 0x46, 1, 2]),
        ]
    }

    #[test]
    fn imports_a_valid_codexskin_and_replaces_existing() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("themes");
        let skin = tmp.path().join("pkg.codexskin");
        let entries = minimal_entries("import-me");
        let refs: Vec<(&str, &[u8])> =
            entries.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
        write_zip(&skin, &refs);

        let summary = import_codexskin(&skin, &root).unwrap();
        assert_eq!(summary.id, "import-me");
        assert_eq!(summary.meta.version.as_deref(), Some("1.0.0"));
        assert!(summary.preview.as_ref().unwrap().ends_with("previews/home.webp"));
        assert!(root.join("import-me/theme.json").is_file());

        // Re-import (upgrade path) replaces in place and leaves no debris.
        let summary2 = import_codexskin(&skin, &root).unwrap();
        assert_eq!(summary2.id, "import-me");
        let leftovers: Vec<_> = std::fs::read_dir(&root)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
            .collect();
        assert!(leftovers.is_empty(), "staging/parked dirs must be cleaned");
    }

    #[test]
    fn rejects_zip_slip_and_missing_manifest() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("themes");

        let evil = tmp.path().join("evil.codexskin");
        write_zip(
            &evil,
            &[
                ("theme.json", br#"{"schemaVersion":2,"id":"evil"}"# as &[u8]),
                ("../escape.txt", b"boom"),
            ],
        );
        assert!(import_codexskin(&evil, &root).is_err());
        assert!(!tmp.path().join("escape.txt").exists());

        let hollow = tmp.path().join("hollow.codexskin");
        write_zip(&hollow, &[("readme.md", b"hi" as &[u8])]);
        let error = import_codexskin(&hollow, &root).unwrap_err().to_string();
        assert!(error.contains("theme.json"), "{error}");
    }

    #[test]
    fn rejects_invalid_package_content() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("themes");
        let bad = tmp.path().join("bad.codexskin");
        // Valid zip, but schemaVersion is wrong — must fail load_theme and
        // leave the gallery untouched.
        write_zip(
            &bad,
            &[("theme.json", br#"{"schemaVersion":1,"id":"bad"}"# as &[u8])],
        );
        assert!(import_codexskin(&bad, &root).is_err());
        assert!(!root.join("bad").exists());
    }
}
