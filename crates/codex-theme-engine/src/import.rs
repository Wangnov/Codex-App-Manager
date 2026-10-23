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
/// parent hops, no drive prefixes (decoded archive path plus belt and
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
        let entry_name = codex_archive::entry_name(&entry)
            .map_err(|e| err(format!("读取压缩包条目名称失败: {e}")))?;
        let Some(relative) = codex_archive::enclosed_path(&entry_name)
            .and_then(|p| safe_entry_path(p.to_string_lossy().as_ref()))
        else {
            return Err(err(format!("主题包含非法路径条目: {entry_name}")));
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
    fn imports_legacy_cp437_entry_names() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("themes");
        let skin = tmp.path().join("cp437-name.codexskin");
        let mut entries = minimal_entries("cp437-name");
        entries.push(("cafe.bin".to_string(), b"legacy filename".to_vec()));
        let refs: Vec<(&str, &[u8])> =
            entries.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
        write_zip(&skin, &refs);
        let mut bytes = std::fs::read(&skin).unwrap();
        let name = b"cafe.bin";
        let mut replaced = 0;
        for index in 0..=bytes.len() - name.len() {
            if bytes[index..index + name.len()] == *name {
                bytes[index + 3] = 0x82; // CP437 encodes é as one byte.
                replaced += 1;
            }
        }
        assert_eq!(replaced, 2, "update local and central directory names");
        std::fs::write(&skin, bytes).unwrap();
        assert_eq!(import_codexskin(&skin, &root).unwrap().id, "cp437-name");
        assert_eq!(
            std::fs::read(root.join("cp437-name/café.bin")).unwrap(),
            b"legacy filename"
        );
    }

    #[test]
    fn imports_cp437_names_that_are_also_valid_utf8() {
        use crate::zip_test::{archive, Entry};

        let tmp = tempfile::tempdir().unwrap();
        let skin = tmp.path().join("ambiguous.codexskin");
        let root = tmp.path().join("themes");
        let manifest = r#"{"schemaVersion":2,"id":"cp437","name":"T",
            "assets":{"intro":"assets/├⌐.webp"}}"#;
        let bytes = archive(&[
            Entry::new(b"theme.json", manifest.as_bytes()),
            Entry::new(b"theme.css", b"html.codex-theme-studio {}"),
            Entry::new(b"assets/\xc3\xa9.webp", b"RIFF\x01\x02"),
        ]);
        std::fs::write(&skin, bytes).unwrap();
        import_codexskin(&skin, &root).unwrap();
        assert_eq!(
            std::fs::read(root.join("cp437/assets/├⌐.webp")).unwrap(),
            b"RIFF\x01\x02"
        );
        assert!(!root.join("cp437/assets/é.webp").exists());
    }

    #[test]
    fn imports_utf8_and_unicode_path_asset_names() {
        use crate::zip_test::{archive, unicode_path_extra, Entry};

        let raw = b"assets/\xc3\xa9.webp";
        let unicode = unicode_path_extra(raw, "assets/资源.webp".as_bytes(), true);
        let bad_crc = unicode_path_extra(raw, "assets/资源.webp".as_bytes(), false);
        for (flags, extra, expected) in [
            (1 << 11, &[][..], "assets/é.webp"),
            (0, unicode.as_slice(), "assets/资源.webp"),
            (0, bad_crc.as_slice(), "assets/├⌐.webp"),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let skin = tmp.path().join("names.codexskin");
            let root = tmp.path().join("themes");
            let manifest = format!(
                r#"{{"schemaVersion":2,"id":"names","name":"T",
                "assets":{{"intro":"{expected}"}}}}"#
            );
            let mut asset = Entry::new(raw, b"RIFF\x01\x02");
            asset.flags = flags;
            asset.extra = extra;
            std::fs::write(
                &skin,
                archive(&[
                    Entry::new(b"theme.json", manifest.as_bytes()),
                    Entry::new(b"theme.css", b"html.codex-theme-studio {}"),
                    asset,
                ]),
            )
            .unwrap();
            import_codexskin(&skin, &root).unwrap();
            assert_eq!(
                std::fs::read(root.join("names").join(expected)).unwrap(),
                b"RIFF\x01\x02"
            );
        }
    }

    #[test]
    fn rejects_unsafe_unicode_path_before_writing_assets() {
        use crate::zip_test::{archive, unicode_path_extra, Entry};

        let raw = b"assets/safe.webp";
        for unsafe_name in [
            "../escape.webp", r"..\escape.webp", "/absolute", r"C:\escape", "nul\0path",
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let skin = tmp.path().join("unsafe.codexskin");
            let root = tmp.path().join("themes");
            let extra = unicode_path_extra(raw, unsafe_name.as_bytes(), true);
            let mut asset = Entry::new(raw, b"RIFF\x01\x02");
            asset.extra = &extra;
            std::fs::write(
                &skin,
                archive(&[
                    Entry::new(b"theme.json", br#"{"schemaVersion":2,"id":"unsafe","name":"T"}"#),
                    asset,
                ]),
            )
            .unwrap();
            let error = import_codexskin(&skin, &root).unwrap_err().to_string();
            assert!(error.contains("非法路径"), "{unsafe_name:?}: {error}");
            assert!(!root.join("unsafe").exists());
            assert!(!root.join("escape.webp").exists());
        }
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

    #[test]
    fn imports_a_codexskin_with_motion() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("themes");
        let skin = tmp.path().join("motion.codexskin");
        // A package carrying an mp4 motion asset (plus its required static intro
        // fallback) must import without any extra rejection path.
        write_zip(
            &skin,
            &[
                (
                    "theme.json",
                    br#"{"schemaVersion":2,"id":"motion-skin","name":"M","version":"1.0.0",
                        "assets":{"intro":"assets/intro.webp"},
                        "motionAssets":{"intro-video":"assets/intro-video.mp4"},
                        "previews":["previews/home.webp"]}"# as &[u8],
                ),
                ("theme.css", b"html.codex-theme-studio {}\n"),
                ("assets/intro.webp", b"RIFF\x01\x02"),
                ("assets/intro-video.mp4", b"\x00\x00\x00\x18ftypisom"),
                ("previews/home.webp", b"RIFF\x01\x02"),
            ],
        );

        let summary = import_codexskin(&skin, &root).unwrap();
        assert_eq!(summary.id, "motion-skin");
        // The installed package validates and exposes the motion asset with its
        // video mime — ready for the payload's dedicated motion data-URL map.
        let installed = crate::theme::load_theme(&root.join("motion-skin")).unwrap();
        assert!(installed.motion_assets.contains_key("intro-video"));
        assert_eq!(installed.motion_assets["intro-video"].mime, "video/mp4");
    }
}
