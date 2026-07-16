//! Native Codex theme sync (port of `native-theme.mjs`): a theme package may
//! carry a `codexTheme` block that maps onto `~/.codex/config.toml`'s
//! `[desktop.appearance*ChromeTheme]` sections plus the `appearanceTheme`
//! switch. We only ever touch those exact sections, always snapshot the
//! originals first, and callers must only write while Codex is NOT running
//! (it persists its in-memory config on exit and would clobber external
//! edits).
//!
//! Deliberately line-based, not a TOML parser: the backup captures the user's
//! original section text verbatim (comments, spacing and all) and restore
//! puts those exact lines back.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Result, ThemeEngineError};

const SECTION_PREFIXES: [&str; 2] = [
    "desktop.appearanceDarkChromeTheme",
    "desktop.appearanceLightChromeTheme",
];

fn err(message: impl Into<String>) -> ThemeEngineError {
    ThemeEngineError::Native(message.into())
}

/// Where the target config and our one-time backup live. Parameterized so the
/// host app owns the locations (the manager keeps the backup in its own data
/// dir, not the studio's).
#[derive(Debug, Clone)]
pub struct NativeThemePaths {
    pub config: PathBuf,
    pub backup: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct NativeBackup {
    saved_at: String,
    sections: std::collections::BTreeMap<String, String>,
    appearance_theme: Option<String>,
}

/// Remove every section whose header is `prefix` or `prefix.<sub>`; returns
/// the remaining text and the removed lines (verbatim).
fn strip_sections(text: &str, prefix: &str) -> (String, String) {
    let lines: Vec<&str> = text.split('\n').collect();
    let mut cut = vec![false; lines.len()];
    let mut current_start: Option<usize> = None;
    let mark = |start: Option<usize>, end: usize, cut: &mut Vec<bool>| {
        if let Some(start) = start {
            for flag in cut.iter_mut().take(end).skip(start) {
                *flag = true;
            }
        }
    };
    for (index, line) in lines.iter().enumerate() {
        let header = line
            .trim_end()
            .strip_prefix('[')
            .and_then(|rest| rest.strip_suffix(']'));
        if let Some(name) = header {
            mark(current_start.take(), index, &mut cut);
            if name == prefix || name.starts_with(&format!("{prefix}.")) {
                current_start = Some(index);
            }
        }
    }
    mark(current_start.take(), lines.len(), &mut cut);

    let mut kept = Vec::new();
    let mut removed = Vec::new();
    for (index, line) in lines.iter().enumerate() {
        if cut[index] {
            removed.push(*line);
        } else {
            kept.push(*line);
        }
    }
    (kept.join("\n"), removed.join("\n"))
}

fn toml_string(value: &str) -> String {
    if value.contains('"') {
        format!("'{value}'")
    } else {
        format!("\"{value}\"")
    }
}

fn toml_scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(toml_string(s)),
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        _ => None,
    }
}

/// Render one `[desktop.appearance*ChromeTheme]` block (plus optional fonts /
/// semanticColors subtables) from a theme's variant object.
fn chrome_theme_to_toml(section_name: &str, theme: &Value) -> String {
    let mut lines = vec![format!("[desktop.{section_name}]")];
    for key in ["accent", "contrast", "ink", "opaqueWindows", "surface"] {
        if let Some(rendered) = theme.get(key).and_then(toml_scalar) {
            lines.push(format!("{key} = {rendered}"));
        }
    }
    for (sub, label) in [("fonts", "fonts"), ("semanticColors", "semanticColors")] {
        if let Some(map) = theme.get(sub).and_then(|v| v.as_object()) {
            lines.push(String::new());
            lines.push(format!("[desktop.{section_name}.{label}]"));
            for (key, value) in map {
                if let Some(rendered) = toml_scalar(value) {
                    lines.push(format!("{key} = {rendered}"));
                }
            }
        }
    }
    lines.join("\n")
}

fn read_config(paths: &NativeThemePaths) -> Result<String> {
    std::fs::read_to_string(&paths.config)
        .map_err(|e| err(format!("read {}: {e}", paths.config.display())))
}

/// Atomic write via sibling temp + rename, matching the studio's discipline.
fn write_config(paths: &NativeThemePaths, text: &str) -> Result<()> {
    let tmp = paths.config.with_extension("toml.cam-tmp");
    std::fs::write(&tmp, text).map_err(|e| err(format!("write temp config: {e}")))?;
    std::fs::rename(&tmp, &paths.config).map_err(|e| err(format!("replace config: {e}")))
}

fn appearance_theme_line(text: &str) -> Option<String> {
    text.lines()
        .find(|line| {
            let trimmed = line.trim_start();
            trimmed.starts_with("appearanceTheme") && {
                let rest = trimmed["appearanceTheme".len()..].trim_start();
                rest.starts_with('=')
            }
        })
        .map(|line| line.to_string())
}

fn replace_appearance_theme(text: &str, replacement: &str) -> String {
    let mut replaced = false;
    text.split('\n')
        .map(|line| {
            let trimmed = line.trim_start();
            if !replaced
                && trimmed.starts_with("appearanceTheme")
                && trimmed["appearanceTheme".len()..].trim_start().starts_with('=')
            {
                replaced = true;
                replacement.to_string()
            } else {
                line.to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

pub fn has_backup(paths: &NativeThemePaths) -> bool {
    paths.backup.is_file()
}

/// Snapshot the current appearance sections once. An existing backup is the
/// real user baseline — never overwrite it with themed state.
pub fn backup_native_theme(paths: &NativeThemePaths) -> Result<bool> {
    if has_backup(paths) {
        return Ok(false);
    }
    let text = read_config(paths)?;
    let mut sections = std::collections::BTreeMap::new();
    for prefix in SECTION_PREFIXES {
        let (_, removed) = strip_sections(&text, prefix);
        sections.insert(prefix.to_string(), removed);
    }
    let backup = NativeBackup {
        saved_at: chrono_free_timestamp(),
        sections,
        appearance_theme: appearance_theme_line(&text),
    };
    if let Some(parent) = paths.backup.parent() {
        std::fs::create_dir_all(parent).map_err(|e| err(format!("backup dir: {e}")))?;
    }
    let rendered = serde_json::to_string_pretty(&backup)
        .map_err(|e| err(format!("backup serialize: {e}")))?;
    std::fs::write(&paths.backup, format!("{rendered}\n"))
        .map_err(|e| err(format!("write backup: {e}")))?;
    Ok(true)
}

/// Seconds-precision UTC timestamp without pulling in a date crate; the field
/// is informational only.
fn chrono_free_timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}

/// Apply a theme's `codexTheme` block. Returns false when the block is absent
/// or not an object. Caller contract: Codex must not be running.
pub fn apply_native_theme(paths: &NativeThemePaths, codex_theme: &Value) -> Result<bool> {
    if !codex_theme.is_object() {
        return Ok(false);
    }
    backup_native_theme(paths)?;
    let mut text = read_config(paths)?;

    for (variant, prefix) in [("dark", SECTION_PREFIXES[0]), ("light", SECTION_PREFIXES[1])] {
        let Some(block) = codex_theme.get(variant) else {
            continue;
        };
        if !block.is_object() {
            continue;
        }
        let section_name = prefix.trim_start_matches("desktop.");
        let (stripped, _) = strip_sections(&text, prefix);
        text = format!(
            "{}\n\n{}\n",
            stripped.trim_end(),
            chrome_theme_to_toml(section_name, block)
        );
    }

    if let Some(appearance) = codex_theme.get("appearanceTheme").and_then(|v| v.as_str()) {
        // Only replace an existing switch — never introduce one (mirrors the
        // studio: a user who never set appearanceTheme keeps that default).
        if appearance_theme_line(&text).is_some() {
            text = replace_appearance_theme(
                &text,
                &format!("appearanceTheme = {}", toml_string(appearance)),
            );
        }
    }

    write_config(paths, &text)?;
    Ok(true)
}

/// Put the user's original appearance sections back and drop the backup.
pub fn restore_native_theme(paths: &NativeThemePaths) -> Result<bool> {
    if !has_backup(paths) {
        return Ok(false);
    }
    let backup: NativeBackup = serde_json::from_str(
        &std::fs::read_to_string(&paths.backup).map_err(|e| err(format!("read backup: {e}")))?,
    )
    .map_err(|e| err(format!("backup parse: {e}")))?;
    let mut text = read_config(paths)?;
    for prefix in SECTION_PREFIXES {
        let (stripped, _) = strip_sections(&text, prefix);
        text = stripped;
        if let Some(original) = backup.sections.get(prefix) {
            if !original.trim().is_empty() {
                text = format!("{}\n\n{}\n", text.trim_end(), original.trim());
            }
        }
    }
    if let Some(original_line) = &backup.appearance_theme {
        if appearance_theme_line(&text).is_some() {
            text = replace_appearance_theme(&text, original_line);
        }
    }
    write_config(paths, &text)?;
    std::fs::remove_file(&paths.backup).map_err(|e| err(format!("drop backup: {e}")))?;
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    fn paths(tmp: &Path) -> NativeThemePaths {
        NativeThemePaths {
            config: tmp.join("config.toml"),
            backup: tmp.join("backup.json"),
        }
    }

    const BASE_CONFIG: &str = "# user config\nmodel = \"o4\"\nappearanceTheme = \"light\"\n\n[desktop.appearanceDarkChromeTheme]\naccent = \"#111111\"\n\n[desktop.appearanceDarkChromeTheme.fonts]\ncode = \"UserMono\"\n\n[profiles.a]\nkey = \"kept\"\n";

    fn theme_block() -> Value {
        serde_json::json!({
            "appearanceTheme": "dark",
            "dark": {
                "accent": "#d97e2a",
                "opaqueWindows": true,
                "fonts": { "code": "SF Mono" },
                "semanticColors": { "skill": "#e8a33d" }
            },
            "light": { "accent": "#e8a33d" }
        })
    }

    #[test]
    fn apply_replaces_sections_and_keeps_user_content() {
        let tmp = tempfile::tempdir().unwrap();
        let p = paths(tmp.path());
        std::fs::write(&p.config, BASE_CONFIG).unwrap();

        assert!(apply_native_theme(&p, &theme_block()).unwrap());
        let text = std::fs::read_to_string(&p.config).unwrap();
        // User content untouched, themed sections in, switch flipped.
        assert!(text.contains("# user config"));
        assert!(text.contains("[profiles.a]"));
        assert!(text.contains("accent = \"#d97e2a\""));
        assert!(text.contains("opaqueWindows = true"));
        assert!(text.contains("[desktop.appearanceDarkChromeTheme.semanticColors]"));
        assert!(text.contains("[desktop.appearanceLightChromeTheme]"));
        assert!(text.contains("appearanceTheme = \"dark\""));
        assert!(!text.contains("UserMono"), "old themed subtable must be replaced");
    }

    #[test]
    fn backup_is_taken_once_and_restore_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let p = paths(tmp.path());
        std::fs::write(&p.config, BASE_CONFIG).unwrap();

        apply_native_theme(&p, &theme_block()).unwrap();
        // A second apply must not overwrite the pristine backup.
        let backup_before = std::fs::read_to_string(&p.backup).unwrap();
        apply_native_theme(&p, &theme_block()).unwrap();
        assert_eq!(backup_before, std::fs::read_to_string(&p.backup).unwrap());

        assert!(restore_native_theme(&p).unwrap());
        let text = std::fs::read_to_string(&p.config).unwrap();
        assert!(text.contains("accent = \"#111111\""), "original section restored");
        assert!(text.contains("code = \"UserMono\""));
        assert!(text.contains("appearanceTheme = \"light\""), "switch restored");
        assert!(!text.contains("#d97e2a"));
        assert!(!p.backup.exists(), "backup consumed");
        assert!(!restore_native_theme(&p).unwrap(), "second restore is a no-op");
    }

    #[test]
    fn config_without_appearance_switch_never_gains_one() {
        let tmp = tempfile::tempdir().unwrap();
        let p = paths(tmp.path());
        std::fs::write(&p.config, "model = \"o4\"\n").unwrap();
        apply_native_theme(&p, &theme_block()).unwrap();
        let text = std::fs::read_to_string(&p.config).unwrap();
        assert!(!text.contains("appearanceTheme"), "switch must not be introduced");
        assert!(text.contains("[desktop.appearanceDarkChromeTheme]"));
    }
}
