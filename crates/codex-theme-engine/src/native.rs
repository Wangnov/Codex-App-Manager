//! Native Codex theme sync against `~/.codex/config.toml`.
//!
//! Exactly five logical units are managed (SPEC §5) and nothing else:
//! three top-level keys — `appearanceTheme`, `appearanceDarkCodeThemeId`,
//! `appearanceLightCodeThemeId` — and the two
//! `[desktop.appearance{Dark,Light}ChromeTheme]` sections with their
//! `.fonts` / `.semanticColors` subtables. Every other key, table, comment
//! and blank line is preserved verbatim; ambiguity (duplicate managed keys or
//! duplicate managed section headers, at top level) fails closed.
//!
//! Deliberately line-based, not a TOML parser: the baseline captures the
//! user's original text verbatim (comments, spacing and all) and restore puts
//! those exact lines back. `present: false` is a first-class baseline state —
//! restoring it deletes the unit we introduced.
//!
//! Callers must only write while Codex is NOT running (it persists its
//! in-memory config on exit and would clobber external edits); the hot CDP
//! path in [`crate::native_hot`] is the running-Codex counterpart.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Digest as _;

use crate::codex_theme::{AppearanceTheme, ChromeTheme, CodexTheme};
use crate::{Result, ThemeEngineError};

pub const SECTION_PREFIXES: [&str; 2] = [
    "desktop.appearanceDarkChromeTheme",
    "desktop.appearanceLightChromeTheme",
];
pub const TOP_KEYS: [&str; 3] = [
    "appearanceTheme",
    "appearanceDarkCodeThemeId",
    "appearanceLightCodeThemeId",
];

fn err(message: impl Into<String>) -> ThemeEngineError {
    ThemeEngineError::Native(message.into())
}

/// Where the target config and our one-time baseline live. Parameterized so
/// the host app owns the locations (the manager keeps the baseline in its own
/// data dir, not the studio's).
#[derive(Debug, Clone)]
pub struct NativeThemePaths {
    pub config: PathBuf,
    pub backup: PathBuf,
}

/// User baseline. `format_version` 1 files (pre code-theme-id support) only
/// carried the two sections + the appearanceTheme line; v2 records every
/// managed top-level key with `None` meaning "absent at baseline" so restore
/// can delete keys the theme introduced.
#[derive(Debug, Serialize, Deserialize)]
struct NativeBackup {
    #[serde(default = "legacy_format")]
    format_version: u32,
    saved_at: String,
    #[serde(default)]
    source_sha256: Option<String>,
    /// Raw removed lines per section prefix; empty string = absent.
    sections: BTreeMap<String, String>,
    /// Legacy v1 field (raw `appearanceTheme = …` line). v2 keeps it in sync
    /// for downgrade safety but reads `top_keys` instead.
    appearance_theme: Option<String>,
    /// v2: raw line per managed top-level key; `None` = absent at baseline.
    #[serde(default)]
    top_keys: BTreeMap<String, Option<String>>,
}

fn legacy_format() -> u32 {
    1
}

// ── line-level model ────────────────────────────────────────────────────────

fn is_section_header(line: &str) -> bool {
    line.trim_start().starts_with('[')
}

fn header_name(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    let rest = trimmed.strip_prefix('[')?;
    let end = rest.find(']')?;
    Some(rest[..end].trim())
}

/// Index of the first section header — the end of the top-level key scope.
fn top_scope_end(lines: &[&str]) -> usize {
    lines
        .iter()
        .position(|line| is_section_header(line))
        .unwrap_or(lines.len())
}

fn is_key_line(line: &str, key: &str) -> bool {
    let trimmed = line.trim_start();
    if trimmed.starts_with('#') {
        return false;
    }
    trimmed
        .strip_prefix(key)
        .map(|rest| rest.trim_start().starts_with('='))
        .unwrap_or(false)
}

/// All line indexes (within the top-level scope) holding the managed key.
fn top_key_lines(lines: &[&str], key: &str) -> Vec<usize> {
    let scope = top_scope_end(lines);
    (0..scope).filter(|&i| is_key_line(lines[i], key)).collect()
}

/// Line ranges `[start, end)` of every section whose header is `prefix` or
/// `prefix.<sub>`.
fn section_ranges(lines: &[&str], prefix: &str) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    let mut current: Option<usize> = None;
    for (index, line) in lines.iter().enumerate() {
        if let Some(name) = header_name(line) {
            if let Some(start) = current.take() {
                ranges.push((start, index));
            }
            if name == prefix || name.starts_with(&format!("{prefix}.")) {
                current = Some(index);
            }
        }
    }
    if let Some(start) = current {
        ranges.push((start, lines.len()));
    }
    ranges
}

/// Fail-closed ambiguity checks: duplicate managed top-level keys, or the same
/// managed section header appearing twice.
fn check_unambiguous(lines: &[&str]) -> Result<()> {
    for key in TOP_KEYS {
        let hits = top_key_lines(lines, key);
        if hits.len() > 1 {
            return Err(err(format!(
                "受管键 {key} 在 config.toml 顶层出现 {} 次，无法安全归并",
                hits.len()
            )));
        }
    }
    for prefix in SECTION_PREFIXES {
        let mut seen = std::collections::HashSet::new();
        for line in lines.iter() {
            if let Some(name) = header_name(line) {
                if (name == prefix || name.starts_with(&format!("{prefix}.")))
                    && !seen.insert(name.to_string())
                {
                    return Err(err(format!(
                        "受管 section [{name}] 重复出现，无法安全归并"
                    )));
                }
            }
        }
    }
    Ok(())
}

// ── unit plans ──────────────────────────────────────────────────────────────

/// What to do with one managed unit when rewriting the config.
#[derive(Debug, Clone, PartialEq)]
pub enum UnitPlan {
    /// Leave whatever is there untouched.
    Keep,
    /// Ensure the unit is absent.
    Remove,
    /// Replace (or introduce) with this exact text — a single raw line for a
    /// top-level key, a raw multi-line block for a section.
    Set(String),
}

/// Target state for the five managed units.
#[derive(Debug, Clone)]
pub struct NativePlanInput {
    pub appearance_theme: UnitPlan,
    pub dark_code_id: UnitPlan,
    pub light_code_id: UnitPlan,
    pub dark_chrome: UnitPlan,
    pub light_chrome: UnitPlan,
}

impl NativePlanInput {
    pub fn keep_all() -> Self {
        Self {
            appearance_theme: UnitPlan::Keep,
            dark_code_id: UnitPlan::Keep,
            light_code_id: UnitPlan::Keep,
            dark_chrome: UnitPlan::Keep,
            light_chrome: UnitPlan::Keep,
        }
    }

    fn top_key_plans(&self) -> [(&'static str, &UnitPlan); 3] {
        [
            ("appearanceTheme", &self.appearance_theme),
            ("appearanceDarkCodeThemeId", &self.dark_code_id),
            ("appearanceLightCodeThemeId", &self.light_code_id),
        ]
    }

    fn section_plans(&self) -> [(&'static str, &UnitPlan); 2] {
        [
            (SECTION_PREFIXES[0], &self.dark_chrome),
            (SECTION_PREFIXES[1], &self.light_chrome),
        ]
    }
}

fn detected_eol(text: &str) -> &'static str {
    if text.contains("\r\n") {
        "\r\n"
    } else {
        "\n"
    }
}

/// Pure rewrite: apply the unit plans to `current`, preserving every
/// non-managed byte. Kept lines keep their own endings; introduced lines use
/// the file's dominant ending. Returns the full new text (single trailing
/// newline).
pub fn plan_native_config(current: &str, plan: &NativePlanInput) -> Result<String> {
    let lines: Vec<&str> = current.split('\n').collect();
    check_unambiguous(&lines)?;
    let eol = detected_eol(current);
    let cr = if eol == "\r\n" { "\r" } else { "" };

    // 1) Drop lines being removed/replaced.
    let mut cut = vec![false; lines.len()];
    for (key, unit) in plan.top_key_plans() {
        if matches!(unit, UnitPlan::Keep) {
            continue;
        }
        for index in top_key_lines(&lines, key) {
            cut[index] = true;
        }
    }
    for (prefix, unit) in plan.section_plans() {
        if matches!(unit, UnitPlan::Keep) {
            continue;
        }
        for (start, end) in section_ranges(&lines, prefix) {
            for flag in cut.iter_mut().take(end).skip(start) {
                *flag = true;
            }
        }
    }

    // 2) Rebuild the top-level scope with replacements in place; a key being
    //    introduced lands at the end of the scope.
    let scope_end = top_scope_end(&lines);
    let mut head: Vec<String> = Vec::new();
    for (index, line) in lines.iter().enumerate().take(scope_end) {
        if !cut[index] {
            head.push((*line).to_string());
            continue;
        }
        for (key, unit) in plan.top_key_plans() {
            if is_key_line(line, key) {
                if let UnitPlan::Set(raw) = unit {
                    head.push(format!("{}{cr}", raw.trim_end_matches(['\r', '\n'])));
                }
                break;
            }
        }
    }
    for (key, unit) in plan.top_key_plans() {
        let UnitPlan::Set(raw) = unit else { continue };
        let already = head.iter().any(|line| is_key_line(line, key));
        if !already {
            while head.last().map(|l| l.trim().is_empty()).unwrap_or(false) {
                head.pop();
            }
            head.push(format!("{}{cr}", raw.trim_end_matches(['\r', '\n'])));
        }
    }

    // 3) Keep the rest (sections) minus cut ranges.
    let mut tail: Vec<String> = Vec::new();
    for (index, line) in lines.iter().enumerate().skip(scope_end) {
        if !cut[index] {
            tail.push((*line).to_string());
        }
    }

    // 4) Reassemble; introduced section blocks append at the end, separated by
    //    one blank line (original blocks re-enter verbatim the same way).
    let mut text = head.join("\n");
    if !tail.is_empty() {
        if !text.is_empty() && !text.trim().is_empty() {
            // Ensure the head/tail boundary keeps a separating blank line.
            while text.ends_with('\n') || text.ends_with('\r') {
                text.pop();
            }
            text.push_str(eol);
            text.push_str(eol);
        }
        text.push_str(&tail.join("\n"));
    }
    for (_, unit) in plan.section_plans() {
        let UnitPlan::Set(block) = unit else { continue };
        let rendered = block.trim_matches(['\r', '\n']);
        if rendered.is_empty() {
            continue;
        }
        while text.ends_with('\n') || text.ends_with('\r') {
            text.pop();
        }
        if !text.is_empty() {
            text.push_str(eol);
            text.push_str(eol);
        }
        if eol == "\r\n" {
            let normalized: Vec<String> = rendered
                .split('\n')
                .map(|l| l.trim_end_matches('\r').to_string())
                .collect();
            text.push_str(&normalized.join("\r\n"));
        } else {
            text.push_str(rendered);
        }
    }

    while text.ends_with('\n') || text.ends_with('\r') {
        text.pop();
    }
    text.push_str(eol);
    Ok(text)
}

// ── managed-state extraction (baseline + verification) ─────────────────────

/// The five managed units as raw text, `None` = absent. Fails closed on
/// ambiguity.
#[derive(Debug, Clone, PartialEq)]
pub struct ManagedState {
    pub appearance_theme: Option<String>,
    pub dark_code_id: Option<String>,
    pub light_code_id: Option<String>,
    pub dark_chrome: Option<String>,
    pub light_chrome: Option<String>,
}

pub fn managed_state(text: &str) -> Result<ManagedState> {
    let lines: Vec<&str> = text.split('\n').collect();
    check_unambiguous(&lines)?;
    let top = |key: &str| -> Option<String> {
        top_key_lines(&lines, key)
            .first()
            .map(|&i| lines[i].trim_end_matches('\r').to_string())
    };
    let section = |prefix: &str| -> Option<String> {
        let ranges = section_ranges(&lines, prefix);
        if ranges.is_empty() {
            return None;
        }
        let mut chunks = Vec::new();
        for (start, end) in ranges {
            chunks.push(
                lines[start..end]
                    .iter()
                    .map(|l| l.trim_end_matches('\r'))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
        }
        Some(chunks.join("\n").trim_matches('\n').to_string())
    };
    Ok(ManagedState {
        appearance_theme: top("appearanceTheme"),
        dark_code_id: top("appearanceDarkCodeThemeId"),
        light_code_id: top("appearanceLightCodeThemeId"),
        dark_chrome: section(SECTION_PREFIXES[0]),
        light_chrome: section(SECTION_PREFIXES[1]),
    })
}

/// SHA-256 of everything that is NOT ours — post-commit proof that the write
/// touched only the managed units. Line endings are normalized so the digest
/// is stable across the join/split round-trip.
pub fn non_managed_digest(text: &str) -> Result<String> {
    let lines: Vec<&str> = text.split('\n').collect();
    check_unambiguous(&lines)?;
    let mut cut = vec![false; lines.len()];
    for key in TOP_KEYS {
        for index in top_key_lines(&lines, key) {
            cut[index] = true;
        }
    }
    for prefix in SECTION_PREFIXES {
        for (start, end) in section_ranges(&lines, prefix) {
            for flag in cut.iter_mut().take(end).skip(start) {
                *flag = true;
            }
        }
    }
    let mut hasher = sha2::Sha256::new();
    for (index, line) in lines.iter().enumerate() {
        if cut[index] {
            continue;
        }
        let trimmed = line.trim_end_matches('\r').trim_end();
        if trimmed.is_empty() {
            continue;
        }
        hasher.update(trimmed.as_bytes());
        hasher.update(b"\n");
    }
    Ok(hex(&hasher.finalize()))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex(&sha2::Sha256::digest(bytes))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

// ── rendering ───────────────────────────────────────────────────────────────

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

/// Render one `[desktop.appearance*ChromeTheme]` block (plus fonts /
/// semanticColors subtables) from a typed ChromeTheme.
fn chrome_theme_block(section_name: &str, theme: &ChromeTheme) -> String {
    let mut lines = vec![format!("[desktop.{section_name}]")];
    lines.push(format!("accent = {}", toml_string(&theme.accent)));
    lines.push(format!("contrast = {}", theme.contrast));
    lines.push(format!("ink = {}", toml_string(&theme.ink)));
    lines.push(format!("opaqueWindows = {}", theme.opaque_windows));
    lines.push(format!("surface = {}", toml_string(&theme.surface)));
    let fonts: Vec<(&str, &Option<String>)> =
        vec![("code", &theme.fonts.code), ("ui", &theme.fonts.ui)];
    let set_fonts: Vec<_> = fonts.iter().filter(|(_, v)| v.is_some()).collect();
    if !set_fonts.is_empty() {
        lines.push(String::new());
        lines.push(format!("[desktop.{section_name}.fonts]"));
        for (key, value) in set_fonts {
            if let Some(v) = value {
                lines.push(format!("{key} = {}", toml_string(v)));
            }
        }
    }
    lines.push(String::new());
    lines.push(format!("[desktop.{section_name}.semanticColors]"));
    lines.push(format!(
        "diffAdded = {}",
        toml_string(&theme.semantic_colors.diff_added)
    ));
    lines.push(format!(
        "diffRemoved = {}",
        toml_string(&theme.semantic_colors.diff_removed)
    ));
    lines.push(format!("skill = {}", toml_string(&theme.semantic_colors.skill)));
    lines.join("\n")
}

/// Render a raw JSON settings value into a managed-unit plan: chrome objects
/// become sections, strings become key lines, absent means Remove. Used to
/// write a hot-path snapshot back while Codex is stopped.
fn value_unit_plan_key(key: &str, value: Option<&Value>) -> UnitPlan {
    match value.and_then(Value::as_str) {
        Some(s) => UnitPlan::Set(format!("{key} = {}", toml_string(s))),
        None => UnitPlan::Remove,
    }
}

fn value_unit_plan_section(section_name: &str, value: Option<&Value>) -> UnitPlan {
    let Some(obj) = value.filter(|v| v.is_object()) else {
        return UnitPlan::Remove;
    };
    let mut lines = vec![format!("[desktop.{section_name}]")];
    for key in ["accent", "contrast", "ink", "opaqueWindows", "surface"] {
        if let Some(rendered) = obj.get(key).and_then(toml_scalar) {
            lines.push(format!("{key} = {rendered}"));
        }
    }
    for sub in ["fonts", "semanticColors"] {
        if let Some(map) = obj.get(sub).and_then(|v| v.as_object()) {
            let set: Vec<_> = map.iter().filter(|(_, v)| !v.is_null()).collect();
            if set.is_empty() {
                continue;
            }
            lines.push(String::new());
            lines.push(format!("[desktop.{section_name}.{sub}]"));
            for (key, value) in set {
                if let Some(rendered) = toml_scalar(value) {
                    lines.push(format!("{key} = {rendered}"));
                }
            }
        }
    }
    UnitPlan::Set(lines.join("\n"))
}

// ── file plumbing ───────────────────────────────────────────────────────────

fn read_config(paths: &NativeThemePaths) -> Result<String> {
    std::fs::read_to_string(&paths.config)
        .map_err(|e| err(format!("read {}: {e}", paths.config.display())))
}

static TMP_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Atomic replace with durability: unique sibling temp (no fixed name → no
/// concurrent clobber), original permissions preserved, fsync file, rename,
/// fsync directory.
pub fn write_config_atomic(paths: &NativeThemePaths, text: &str) -> Result<()> {
    let parent = paths
        .config
        .parent()
        .ok_or_else(|| err("config path has no parent directory"))?;
    let unique = format!(
        ".cam-native-{}-{}.tmp",
        std::process::id(),
        TMP_COUNTER.fetch_add(1, Ordering::SeqCst)
    );
    let tmp = parent.join(unique);
    let mut file =
        std::fs::File::create(&tmp).map_err(|e| err(format!("create temp config: {e}")))?;
    file.write_all(text.as_bytes())
        .map_err(|e| err(format!("write temp config: {e}")))?;
    file.sync_all()
        .map_err(|e| err(format!("fsync temp config: {e}")))?;
    drop(file);
    if let Ok(meta) = std::fs::metadata(&paths.config) {
        let _ = std::fs::set_permissions(&tmp, meta.permissions());
    }
    std::fs::rename(&tmp, &paths.config).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        err(format!("replace config: {e}"))
    })?;
    #[cfg(unix)]
    if let Ok(dir) = std::fs::File::open(parent) {
        let _ = dir.sync_all();
    }
    Ok(())
}

// ── baseline ────────────────────────────────────────────────────────────────

pub fn has_backup(paths: &NativeThemePaths) -> bool {
    paths.backup.is_file()
}

/// Seconds-precision UTC timestamp without pulling in a date crate; the field
/// is informational only.
fn timestamp() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    format!("unix:{secs}")
}

/// Snapshot the managed units once (v2 format). An existing baseline — either
/// format — is the real user state; never overwrite it with themed state.
/// Written atomically (temp + rename) so a crash can't leave half a baseline.
pub fn backup_native_theme(paths: &NativeThemePaths) -> Result<bool> {
    if has_backup(paths) {
        return Ok(false);
    }
    let text = read_config(paths)?;
    let state = managed_state(&text)?;
    let mut sections = BTreeMap::new();
    sections.insert(
        SECTION_PREFIXES[0].to_string(),
        state.dark_chrome.clone().unwrap_or_default(),
    );
    sections.insert(
        SECTION_PREFIXES[1].to_string(),
        state.light_chrome.clone().unwrap_or_default(),
    );
    let mut top_keys = BTreeMap::new();
    top_keys.insert("appearanceTheme".to_string(), state.appearance_theme.clone());
    top_keys.insert(
        "appearanceDarkCodeThemeId".to_string(),
        state.dark_code_id.clone(),
    );
    top_keys.insert(
        "appearanceLightCodeThemeId".to_string(),
        state.light_code_id.clone(),
    );
    let backup = NativeBackup {
        format_version: 2,
        saved_at: timestamp(),
        source_sha256: Some(sha256_hex(text.as_bytes())),
        sections,
        appearance_theme: state.appearance_theme.clone(),
        top_keys,
    };
    if let Some(parent) = paths.backup.parent() {
        std::fs::create_dir_all(parent).map_err(|e| err(format!("backup dir: {e}")))?;
    }
    let rendered = serde_json::to_string_pretty(&backup)
        .map_err(|e| err(format!("backup serialize: {e}")))?;
    let tmp = paths.backup.with_extension("json.tmp");
    std::fs::write(&tmp, format!("{rendered}\n"))
        .map_err(|e| err(format!("write backup: {e}")))?;
    std::fs::rename(&tmp, &paths.backup).map_err(|e| err(format!("commit backup: {e}")))?;
    Ok(true)
}

// ── apply / restore / snapshot-write ────────────────────────────────────────

/// Plan for a full native apply of a typed theme. Code-theme-id units are only
/// written when the package carries ids (strict callers enforce presence
/// beforehand); `appearanceTheme` is always set — the baseline remembers
/// whether it existed, and a full restore deletes an introduced one.
pub fn apply_plan(theme: &CodexTheme) -> NativePlanInput {
    let id_plan = |id: Option<&str>, key: &str| match id {
        Some(id) => UnitPlan::Set(format!("{key} = {}", toml_string(id))),
        None => UnitPlan::Keep,
    };
    NativePlanInput {
        appearance_theme: UnitPlan::Set(format!(
            "appearanceTheme = {}",
            toml_string(theme.appearance_theme.as_str())
        )),
        dark_code_id: id_plan(
            theme.code_theme_ids.as_ref().map(|i| i.dark.as_str()),
            "appearanceDarkCodeThemeId",
        ),
        light_code_id: id_plan(
            theme.code_theme_ids.as_ref().map(|i| i.light.as_str()),
            "appearanceLightCodeThemeId",
        ),
        dark_chrome: UnitPlan::Set(chrome_theme_block(
            "appearanceDarkChromeTheme",
            &theme.dark,
        )),
        light_chrome: UnitPlan::Set(chrome_theme_block(
            "appearanceLightChromeTheme",
            &theme.light,
        )),
    }
}

/// Apply a typed theme to config.toml: baseline-once, plan, atomic write.
/// Caller contract: Codex must not be running.
pub fn apply_native_theme(paths: &NativeThemePaths, theme: &CodexTheme) -> Result<()> {
    backup_native_theme(paths)?;
    let current = read_config(paths)?;
    let text = plan_native_config(&current, &apply_plan(theme))?;
    write_config_atomic(paths, &text)
}

/// Compatibility shim for raw `codexTheme` JSON blocks (legacy callers).
/// Lenient parse (ids optional). Returns false when the block is absent.
pub fn apply_native_theme_value(paths: &NativeThemePaths, codex_theme: &Value) -> Result<bool> {
    if !codex_theme.is_object() {
        return Ok(false);
    }
    let parsed = crate::codex_theme::parse_codex_theme(
        codex_theme,
        crate::codex_theme::ValidateOptions {
            require_code_theme_ids: false,
            ..Default::default()
        },
    )
    .map_err(|e| err(format!("codexTheme 校验失败: {e}")))?;
    apply_native_theme(paths, &parsed)?;
    Ok(true)
}

/// Hot-path settings snapshot rendered back into config.toml (used to undo a
/// try-on when Codex is already stopped, or by startup recovery). `None`
/// values delete the corresponding unit.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NativeSettingsSnapshot {
    pub appearance_theme: Option<Value>,
    pub dark_chrome: Option<Value>,
    pub light_chrome: Option<Value>,
    pub dark_code_id: Option<Value>,
    pub light_code_id: Option<Value>,
}

pub fn snapshot_plan(snapshot: &NativeSettingsSnapshot) -> NativePlanInput {
    NativePlanInput {
        appearance_theme: value_unit_plan_key(
            "appearanceTheme",
            snapshot.appearance_theme.as_ref(),
        ),
        dark_code_id: value_unit_plan_key(
            "appearanceDarkCodeThemeId",
            snapshot.dark_code_id.as_ref(),
        ),
        light_code_id: value_unit_plan_key(
            "appearanceLightCodeThemeId",
            snapshot.light_code_id.as_ref(),
        ),
        dark_chrome: value_unit_plan_section(
            "appearanceDarkChromeTheme",
            snapshot.dark_chrome.as_ref(),
        ),
        light_chrome: value_unit_plan_section(
            "appearanceLightChromeTheme",
            snapshot.light_chrome.as_ref(),
        ),
    }
}

pub fn write_snapshot_to_config(
    paths: &NativeThemePaths,
    snapshot: &NativeSettingsSnapshot,
) -> Result<()> {
    let current = read_config(paths)?;
    let text = plan_native_config(&current, &snapshot_plan(snapshot))?;
    write_config_atomic(paths, &text)
}

fn restore_plan(backup: &NativeBackup) -> NativePlanInput {
    let section_plan = |prefix: &str| -> UnitPlan {
        match backup.sections.get(prefix) {
            Some(raw) if !raw.trim().is_empty() => UnitPlan::Set(raw.trim().to_string()),
            _ => UnitPlan::Remove,
        }
    };
    let top_plan = |key: &str| -> UnitPlan {
        if backup.format_version >= 2 {
            match backup.top_keys.get(key) {
                Some(Some(raw)) => UnitPlan::Set(raw.clone()),
                Some(None) => UnitPlan::Remove,
                // v2 file without the key recorded (shouldn't happen) — leave.
                None => UnitPlan::Keep,
            }
        } else if key == "appearanceTheme" {
            // Legacy baseline: only the appearance line was captured, and the
            // legacy writer never introduced one — replace when we have it,
            // otherwise leave whatever is there (matches old behavior).
            match &backup.appearance_theme {
                Some(raw) => UnitPlan::Set(raw.clone()),
                None => UnitPlan::Keep,
            }
        } else {
            // Legacy baseline predates code-theme-id management.
            UnitPlan::Keep
        }
    };
    NativePlanInput {
        appearance_theme: top_plan("appearanceTheme"),
        dark_code_id: top_plan("appearanceDarkCodeThemeId"),
        light_code_id: top_plan("appearanceLightCodeThemeId"),
        dark_chrome: section_plan(SECTION_PREFIXES[0]),
        light_chrome: section_plan(SECTION_PREFIXES[1]),
    }
}

fn read_backup(paths: &NativeThemePaths) -> Result<NativeBackup> {
    serde_json::from_str(
        &std::fs::read_to_string(&paths.backup).map_err(|e| err(format!("read backup: {e}")))?,
    )
    .map_err(|e| err(format!("backup parse: {e}")))
}

/// The exact text a full restore would write, without writing it — lets a
/// transactional caller stage the bytes (journal + crash recovery) before
/// committing. `None` when there is no baseline.
pub fn planned_restore_text(paths: &NativeThemePaths, current: &str) -> Result<Option<String>> {
    if !has_backup(paths) {
        return Ok(None);
    }
    let backup = read_backup(paths)?;
    plan_native_config(current, &restore_plan(&backup)).map(Some)
}

/// Delete the baseline — only after a verified successful restore commit.
pub fn drop_backup(paths: &NativeThemePaths) -> Result<()> {
    std::fs::remove_file(&paths.backup).map_err(|e| err(format!("drop backup: {e}")))
}

/// Put the user's original units back and drop the baseline. Handles both
/// baseline formats; v2 deletes units that were absent at baseline.
pub fn restore_native_theme(paths: &NativeThemePaths) -> Result<bool> {
    let current = read_config(paths)?;
    let Some(text) = planned_restore_text(paths, &current)? else {
        return Ok(false);
    };
    write_config_atomic(paths, &text)?;
    drop_backup(paths)?;
    Ok(true)
}

/// Post-commit verification (SPEC §8.7): the five units on disk equal the
/// plan's outcome and nothing else changed relative to the preimage.
pub fn verify_commit(preimage: &str, planned: &str, on_disk: &str) -> Result<()> {
    let want = managed_state(planned)?;
    let got = managed_state(on_disk)?;
    if want != got {
        return Err(err("提交校验失败：受管单元与目标不一致"));
    }
    let before = non_managed_digest(preimage)?;
    let after = non_managed_digest(on_disk)?;
    if before != after {
        return Err(err("提交校验失败：非受管内容发生了变化"));
    }
    Ok(())
}

/// Convenience for callers needing the typed appearance for a variant choice.
pub fn appearance_of(theme: &CodexTheme) -> AppearanceTheme {
    theme.appearance_theme
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::codex_theme::{parse_codex_theme, ValidateOptions};

    fn paths(tmp: &Path) -> NativeThemePaths {
        NativeThemePaths {
            config: tmp.join("config.toml"),
            backup: tmp.join("backup.json"),
        }
    }

    const BASE_CONFIG: &str = "# user config\nmodel = \"o4\"\nappearanceTheme = \"light\"\nappearanceDarkCodeThemeId = \"userdark\"\n\n[desktop.appearanceDarkChromeTheme]\naccent = \"#111111\"\n\n[desktop.appearanceDarkChromeTheme.fonts]\ncode = \"UserMono\"\n\n[profiles.a]\nkey = \"kept\"\n";

    fn full_theme() -> CodexTheme {
        parse_codex_theme(
            &serde_json::json!({
                "appearanceTheme": "dark",
                "codeThemeIds": { "dark": "absolutely", "light": "absolutely" },
                "dark": {
                    "accent": "#d97e2a", "contrast": 60, "ink": "#f2e9d8",
                    "opaqueWindows": true, "surface": "#1a1d24",
                    "fonts": { "code": "SF Mono", "ui": null },
                    "semanticColors": { "diffAdded": "#46c077", "diffRemoved": "#d64541", "skill": "#e8a33d" }
                },
                "light": {
                    "accent": "#a65e00", "contrast": 60, "ink": "#3a2419",
                    "opaqueWindows": true, "surface": "#fff8e8",
                    "fonts": { "code": "SF Mono", "ui": null },
                    "semanticColors": { "diffAdded": "#24844f", "diffRemoved": "#b53632", "skill": "#8d5700" }
                }
            }),
            ValidateOptions::default(),
        )
        .unwrap()
    }

    #[test]
    fn apply_covers_all_five_units_and_keeps_user_content() {
        let tmp = tempfile::tempdir().unwrap();
        let p = paths(tmp.path());
        std::fs::write(&p.config, BASE_CONFIG).unwrap();

        apply_native_theme(&p, &full_theme()).unwrap();
        let text = std::fs::read_to_string(&p.config).unwrap();
        assert!(text.contains("# user config"));
        assert!(text.contains("model = \"o4\""));
        assert!(text.contains("[profiles.a]"));
        assert!(text.contains("appearanceTheme = \"dark\""));
        assert!(text.contains("appearanceDarkCodeThemeId = \"absolutely\""));
        assert!(text.contains("appearanceLightCodeThemeId = \"absolutely\""));
        assert!(text.contains("accent = \"#d97e2a\""));
        assert!(text.contains("[desktop.appearanceLightChromeTheme.semanticColors]"));
        assert!(!text.contains("UserMono"), "old themed subtable replaced");
        assert!(!text.contains("userdark"), "old code id replaced");
        // Managed state matches the plan exactly.
        verify_commit(BASE_CONFIG, &text, &text).unwrap();
    }

    #[test]
    fn baseline_v2_captures_absence_and_restore_deletes_introduced_units() {
        let tmp = tempfile::tempdir().unwrap();
        let p = paths(tmp.path());
        // No appearanceTheme, no code ids, no sections at all.
        std::fs::write(&p.config, "model = \"o4\"\n\n[profiles.a]\nkey = \"kept\"\n").unwrap();

        apply_native_theme(&p, &full_theme()).unwrap();
        let themed = std::fs::read_to_string(&p.config).unwrap();
        assert!(themed.contains("appearanceTheme = \"dark\""), "introduced");
        assert!(themed.contains("appearanceDarkCodeThemeId"));

        assert!(restore_native_theme(&p).unwrap());
        let text = std::fs::read_to_string(&p.config).unwrap();
        assert!(!text.contains("appearanceTheme"), "introduced switch deleted");
        assert!(!text.contains("CodeThemeId"), "introduced ids deleted");
        assert!(!text.contains("ChromeTheme"), "introduced sections deleted");
        assert!(text.contains("model = \"o4\""));
        assert!(text.contains("[profiles.a]"));
    }

    #[test]
    fn a_to_b_to_off_restores_the_original_user_state() {
        let tmp = tempfile::tempdir().unwrap();
        let p = paths(tmp.path());
        std::fs::write(&p.config, BASE_CONFIG).unwrap();

        apply_native_theme(&p, &full_theme()).unwrap();
        let baseline_bytes = std::fs::read_to_string(&p.backup).unwrap();

        let mut theme_b = full_theme();
        theme_b.dark.accent = "#00ff00".to_string();
        apply_native_theme(&p, &theme_b).unwrap();
        assert_eq!(
            baseline_bytes,
            std::fs::read_to_string(&p.backup).unwrap(),
            "baseline must never be overwritten by themed state"
        );

        restore_native_theme(&p).unwrap();
        let text = std::fs::read_to_string(&p.config).unwrap();
        assert!(text.contains("appearanceTheme = \"light\""), "user switch back");
        assert!(text.contains("appearanceDarkCodeThemeId = \"userdark\""));
        assert!(text.contains("accent = \"#111111\""));
        assert!(text.contains("code = \"UserMono\""));
        assert!(!text.contains("#d97e2a"));
        assert!(!text.contains("#00ff00"));
    }

    #[test]
    fn crlf_comments_and_unmanaged_sections_survive() {
        let tmp = tempfile::tempdir().unwrap();
        let p = paths(tmp.path());
        let config = "# top comment\r\nmodel = \"o4\"\r\nappearanceTheme = \"light\"\r\n\r\n[profiles.a]\r\n# inner comment\r\nkey = \"kept\"\r\n";
        std::fs::write(&p.config, config).unwrap();

        apply_native_theme(&p, &full_theme()).unwrap();
        let themed = std::fs::read_to_string(&p.config).unwrap();
        assert!(themed.contains("# top comment\r\n"), "CRLF comment kept");
        assert!(themed.contains("# inner comment\r\n"));
        assert!(themed.contains("appearanceTheme = \"dark\"\r"), "managed line uses file EOL");

        restore_native_theme(&p).unwrap();
        let restored = std::fs::read_to_string(&p.config).unwrap();
        assert!(restored.contains("# top comment\r\n"));
        assert!(restored.contains("appearanceTheme = \"light\"\r"));
        assert!(!restored.contains("ChromeTheme"));
    }

    #[test]
    fn duplicate_managed_units_fail_closed() {
        let dup_key = "appearanceTheme = \"light\"\nappearanceTheme = \"dark\"\n";
        assert!(plan_native_config(dup_key, &NativePlanInput::keep_all()).is_err());

        let dup_section = "[desktop.appearanceDarkChromeTheme]\naccent = \"#111111\"\n\n[desktop.appearanceDarkChromeTheme]\naccent = \"#222222\"\n";
        assert!(plan_native_config(dup_section, &NativePlanInput::keep_all()).is_err());

        // A managed-looking key inside an unmanaged table is NOT ours.
        let nested = "[profiles.a]\nappearanceTheme = \"light\"\n";
        let state = managed_state(nested).unwrap();
        assert!(state.appearance_theme.is_none());
    }

    #[test]
    fn snapshot_write_round_trips_hot_values() {
        let tmp = tempfile::tempdir().unwrap();
        let p = paths(tmp.path());
        std::fs::write(&p.config, BASE_CONFIG).unwrap();

        let snapshot = NativeSettingsSnapshot {
            appearance_theme: Some(serde_json::json!("system")),
            dark_chrome: Some(serde_json::json!({
                "accent": "#f1b83b", "contrast": 60, "ink": "#f7e8c2",
                "opaqueWindows": true, "surface": "#1c100d",
                "fonts": { "code": "SF Mono", "ui": "PingFang SC" },
                "semanticColors": { "diffAdded": "#65b987", "diffRemoved": "#e16b5c", "skill": "#f1b83b" }
            })),
            light_chrome: None,
            dark_code_id: Some(serde_json::json!("absolutely")),
            light_code_id: None,
        };
        write_snapshot_to_config(&p, &snapshot).unwrap();
        let text = std::fs::read_to_string(&p.config).unwrap();
        assert!(text.contains("appearanceTheme = \"system\""));
        assert!(text.contains("appearanceDarkCodeThemeId = \"absolutely\""));
        assert!(!text.contains("appearanceLightCodeThemeId"), "absent value deletes");
        assert!(text.contains("accent = \"#f1b83b\""));
        assert!(!text.contains("[desktop.appearanceLightChromeTheme]"));
        assert!(text.contains("[profiles.a]"));

        let state = managed_state(&text).unwrap();
        assert!(state.light_chrome.is_none());
        assert_eq!(
            state.dark_code_id.as_deref(),
            Some("appearanceDarkCodeThemeId = \"absolutely\"")
        );
    }

    #[test]
    fn verify_commit_detects_foreign_damage() {
        let planned = plan_native_config(BASE_CONFIG, &apply_plan(&full_theme())).unwrap();
        // Simulate the write also clobbering a user key.
        let damaged = planned.replace("model = \"o4\"", "model = \"clobbered\"");
        assert!(verify_commit(BASE_CONFIG, &planned, &damaged).is_err());
        // And the healthy case passes.
        verify_commit(BASE_CONFIG, &planned, &planned).unwrap();
    }

    #[test]
    fn legacy_v1_backup_still_restores() {
        let tmp = tempfile::tempdir().unwrap();
        let p = paths(tmp.path());
        std::fs::write(&p.config, BASE_CONFIG).unwrap();
        // Hand-write a legacy (format 1) backup like the old engine produced.
        let legacy = serde_json::json!({
            "saved_at": "unix:0",
            "sections": {
                "desktop.appearanceDarkChromeTheme": "[desktop.appearanceDarkChromeTheme]\naccent = \"#101010\"",
                "desktop.appearanceLightChromeTheme": ""
            },
            "appearance_theme": "appearanceTheme = \"system\""
        });
        std::fs::write(&p.backup, serde_json::to_string_pretty(&legacy).unwrap()).unwrap();

        restore_native_theme(&p).unwrap();
        let text = std::fs::read_to_string(&p.config).unwrap();
        assert!(text.contains("appearanceTheme = \"system\""));
        assert!(text.contains("accent = \"#101010\""));
        assert!(
            text.contains("appearanceDarkCodeThemeId = \"userdark\""),
            "legacy restore leaves code ids alone"
        );
    }
}
