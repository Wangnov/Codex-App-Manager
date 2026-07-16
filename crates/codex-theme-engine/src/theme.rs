//! Theme package loader (port of `theme.mjs`). A theme is a directory:
//!
//! ```text
//! <id>/
//!   theme.json    required — metadata, colors, strings, asset map
//!   theme.css     required — selectors scoped to html.codex-theme-studio
//!   chrome.html   optional — decorative overlay fragment (pointer-events: none)
//!   assets/*.webp bitmap assets referenced by theme.json "assets"
//! ```
//!
//! Everything is validated and inlined; nothing is fetched at runtime.
//! Validation mirrors the studio's lenient posture (invalid colors dropped,
//! strings clamped) so existing schemaVersion-2 packages load unchanged.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use base64::Engine as _;
use serde::Serialize;

use crate::{Result, ThemeEngineError};

/// Per-asset ceiling. Tighter than the studio's nominal 24 MB: Chromium
/// silently treats data: URLs over 2 MB as invalid (backgrounds just vanish),
/// so the real budget is base64(bytes) + header < 2 MB → raw ≤ ~1.4 MB.
/// Existing packages top out around 300 KB per asset.
pub const MAX_ASSET_BYTES: u64 = 1_400_000;
/// Combined raw-asset budget. The whole payload travels as one WebSocket
/// text message to `Runtime.evaluate`; 24 MB raw ≈ 32 MB payload, safely
/// under tungstenite's 64 MB default message cap.
pub const MAX_TOTAL_ASSET_BYTES: u64 = 24 * 1024 * 1024;

const NAME_MAX: usize = 64;

fn name_pattern(key: &str) -> bool {
    // ^[a-z0-9][a-z0-9-]{0,63}$
    let bytes = key.as_bytes();
    if bytes.is_empty() || bytes.len() > NAME_MAX {
        return false;
    }
    if !(bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit()) {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || *b == b'-')
}

fn clamp_text(value: Option<&serde_json::Value>, fallback: &str, max: usize) -> String {
    match value.and_then(|v| v.as_str()) {
        Some(s) if !s.trim().is_empty() => s.trim().chars().take(max).collect(),
        _ => fallback.to_string(),
    }
}

fn valid_color(value: &str) -> bool {
    let v = value.trim();
    let hex = v.strip_prefix('#').is_some_and(|rest| {
        (3..=8).contains(&rest.len()) && rest.bytes().all(|b| b.is_ascii_hexdigit())
    });
    let rgb = (v.to_ascii_lowercase().starts_with("rgb(")
        || v.to_ascii_lowercase().starts_with("rgba("))
        && v.ends_with(')')
        && v[v.find('(').unwrap_or(0) + 1..v.len() - 1]
            .bytes()
            .all(|b| b.is_ascii_digit() || b" .,%".contains(&b));
    hex || rgb
}

/// The metadata handed to the renderer runtime as `__CTS_THEME_JSON__`.
/// Field names and shape must match the studio's `theme.config` exactly —
/// the runtime consumes `id`, `colors` and `strings` by these names.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeConfig {
    pub schema_version: u32,
    pub id: String,
    pub name: String,
    pub description: String,
    pub colors: BTreeMap<String, String>,
    pub strings: BTreeMap<String, String>,
}

/// Delivery metadata (still schemaVersion 2 — every field optional, unknown
/// to older loaders). Deliberately NOT part of [`ThemeConfig`]: the renderer
/// never consumes it, so it stays out of the injected payload and out of the
/// stamp (a new preview screenshot must not force re-injection). Loading is
/// lenient (invalid entries dropped); the studio's `pack` tool is where
/// delivery requirements are enforced strictly.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeMeta {
    /// The package's own semver — the basis for update distribution.
    pub version: Option<String>,
    /// Display author (a bare string, or an object's `name`).
    pub author: Option<String>,
    /// Codex version the theme was verified against at build time.
    pub codex_verified: Option<String>,
    /// "dark" | "light" | "dual" — gallery badge/sorting.
    pub appearance: Option<String>,
    pub tags: Vec<String>,
    pub license: Option<String>,
    /// Package-relative preview images, first one is the cover.
    pub previews: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AssetRef {
    pub path: PathBuf,
    pub mime: &'static str,
    pub bytes: u64,
}

#[derive(Debug, Clone)]
pub struct LoadedTheme {
    pub dir: PathBuf,
    pub config: ThemeConfig,
    pub meta: ThemeMeta,
    pub css: String,
    pub chrome_html: Option<String>,
    pub assets: BTreeMap<String, AssetRef>,
    /// Optional native Codex appearance block, applied to ~/.codex/config.toml
    /// while Codex is stopped. Passed through as-is (validated on apply).
    pub codex_theme: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ThemeSummary {
    pub id: String,
    pub name: String,
    pub description: String,
    pub dir: PathBuf,
    pub has_native_theme: bool,
    /// The package's color tokens — the UI renders theme cards from these
    /// (swatch strip + abstract mini-preview) when no preview image ships.
    pub colors: BTreeMap<String, String>,
    /// Absolute path of the cover preview (first valid `meta.previews`
    /// entry), when the package ships one.
    pub preview: Option<PathBuf>,
    pub meta: ThemeMeta,
}

fn err(message: impl Into<String>) -> ThemeEngineError {
    ThemeEngineError::Theme(message.into())
}

/// Lexical containment check (port of `assertInside`): the referenced file
/// must stay inside the theme directory. Stricter than the original — no
/// absolute paths, no `..` components at all.
fn resolve_inside(root: &Path, candidate: &str, label: &str) -> Result<PathBuf> {
    let rel = Path::new(candidate);
    if rel.is_absolute()
        || rel
            .components()
            .any(|c| !matches!(c, Component::Normal(_)))
    {
        return Err(err(format!(
            "{label} must stay inside the theme directory: {candidate}"
        )));
    }
    Ok(root.join(rel))
}

fn mime_for(path: &Path) -> Option<&'static str> {
    match path
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase()
        .as_str()
    {
        "png" => Some("image/png"),
        "jpg" | "jpeg" => Some("image/jpeg"),
        "webp" => Some("image/webp"),
        _ => None,
    }
}

/// Preview images may be larger than the spec's 500 KB recommendation but
/// anything past this is a packaging mistake, not a screenshot.
const MAX_PREVIEW_BYTES: u64 = 2 * 1024 * 1024;

/// Lenient delivery-metadata extraction: absent/invalid entries drop out
/// silently — strict enforcement is the packer's job, not the loader's.
fn extract_meta(raw: &serde_json::Value, dir: &Path) -> ThemeMeta {
    let text_field = |key: &str, max: usize| -> Option<String> {
        raw.get(key)
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.chars().take(max).collect())
    };
    let author = match raw.get("author") {
        Some(serde_json::Value::String(s)) if !s.trim().is_empty() => {
            Some(s.trim().chars().take(80).collect())
        }
        Some(serde_json::Value::Object(map)) => map
            .get("name")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().chars().take(80).collect()),
        _ => None,
    };
    let appearance = text_field("appearance", 8)
        .filter(|value| matches!(value.as_str(), "dark" | "light" | "dual"));
    let tags = raw
        .get("tags")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str())
                .filter(|s| name_pattern(s))
                .take(8)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    let previews = raw
        .get("previews")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str())
                .filter_map(|rel| {
                    let path = resolve_inside(dir, rel, "preview").ok()?;
                    let mime = mime_for(&path)?;
                    let _ = mime;
                    let meta = std::fs::metadata(&path).ok()?;
                    (meta.is_file() && meta.len() >= 1 && meta.len() <= MAX_PREVIEW_BYTES)
                        .then(|| rel.to_string())
                })
                .take(4)
                .collect()
        })
        .unwrap_or_default();
    ThemeMeta {
        version: text_field("version", 32),
        author,
        codex_verified: text_field("codexVerified", 32),
        appearance,
        tags,
        license: text_field("license", 80),
        previews,
    }
}

/// Load and validate a schemaVersion-2 theme package.
pub fn load_theme(theme_dir: &Path) -> Result<LoadedTheme> {
    let dir = theme_dir.to_path_buf();
    let raw: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(dir.join("theme.json"))
            .map_err(|e| err(format!("theme.json unreadable: {e}")))?,
    )
    .map_err(|e| err(format!("theme.json invalid JSON: {e}")))?;

    if raw.get("schemaVersion").and_then(|v| v.as_u64()) != Some(2) {
        return Err(err(format!(
            "theme.json schemaVersion must be 2 (got {})",
            raw.get("schemaVersion").unwrap_or(&serde_json::Value::Null)
        )));
    }
    let id = clamp_text(raw.get("id"), "", 160);
    if !name_pattern(&id) {
        return Err(err(format!(
            "theme id must match ^[a-z0-9][a-z0-9-]{{0,63}}$: {id:?}"
        )));
    }

    let mut colors = BTreeMap::new();
    if let Some(map) = raw.get("colors").and_then(|v| v.as_object()) {
        for (key, value) in map {
            if !name_pattern(key) {
                return Err(err(format!("invalid color key: {key}")));
            }
            if let Some(v) = value.as_str() {
                if valid_color(v) {
                    colors.insert(key.clone(), v.trim().to_string());
                }
            }
        }
    }
    let mut strings = BTreeMap::new();
    if let Some(map) = raw.get("strings").and_then(|v| v.as_object()) {
        for (key, value) in map {
            if !name_pattern(key) {
                return Err(err(format!("invalid string key: {key}")));
            }
            strings.insert(key.clone(), clamp_text(Some(value), "", 200));
        }
    }

    let config = ThemeConfig {
        schema_version: 2,
        id: id.clone(),
        name: clamp_text(raw.get("name"), &id, 80),
        description: clamp_text(raw.get("description"), "", 240),
        colors,
        strings,
    };

    let css_rel = clamp_text(raw.get("css"), "theme.css", 120);
    let css_path = resolve_inside(&dir, &css_rel, "css")?;
    let css = std::fs::read_to_string(&css_path)
        .map_err(|e| err(format!("css unreadable ({}): {e}", css_path.display())))?;

    let chrome_html = match raw.get("chrome") {
        Some(v) if !v.is_null() => {
            let chrome_rel = clamp_text(Some(v), "chrome.html", 120);
            let chrome_path = resolve_inside(&dir, &chrome_rel, "chrome")?;
            Some(
                std::fs::read_to_string(&chrome_path)
                    .map_err(|e| err(format!("chrome unreadable: {e}")))?,
            )
        }
        _ => None,
    };

    let mut assets = BTreeMap::new();
    let mut total_bytes = 0u64;
    if let Some(map) = raw.get("assets").and_then(|v| v.as_object()) {
        for (key, value) in map {
            if !name_pattern(key) {
                return Err(err(format!("invalid asset key: {key}")));
            }
            let rel = value
                .as_str()
                .ok_or_else(|| err(format!("asset {key} path must be a string")))?;
            let asset_path = resolve_inside(&dir, rel, &format!("asset {key}"))?;
            let mime = mime_for(&asset_path)
                .ok_or_else(|| err(format!("unsupported asset format for {key}: {rel}")))?;
            let meta = std::fs::metadata(&asset_path)
                .map_err(|e| err(format!("asset {key} unreadable: {e}")))?;
            if !meta.is_file() || meta.len() < 1 || meta.len() > MAX_ASSET_BYTES {
                return Err(err(format!(
                    "asset {key} must be a non-empty file up to {MAX_ASSET_BYTES} bytes \
                     (base64 of anything larger exceeds Chromium's 2 MB data-URL cap)"
                )));
            }
            total_bytes += meta.len();
            if total_bytes > MAX_TOTAL_ASSET_BYTES {
                return Err(err("combined theme assets exceed the size budget"));
            }
            assets.insert(
                key.clone(),
                AssetRef {
                    path: asset_path,
                    mime,
                    bytes: meta.len(),
                },
            );
        }
    }

    let codex_theme = raw.get("codexTheme").filter(|v| v.is_object()).cloned();
    let meta = extract_meta(&raw, &dir);

    Ok(LoadedTheme {
        dir,
        config,
        meta,
        css,
        chrome_html,
        assets,
        codex_theme,
    })
}

/// Inline every asset as a `data:` URL (immune to the blob-revocation races
/// that break late-loading images such as border-image sources).
pub fn inline_assets(theme: &LoadedTheme) -> Result<BTreeMap<String, String>> {
    let mut data_urls = BTreeMap::new();
    for (key, asset) in &theme.assets {
        let bytes = std::fs::read(&asset.path)
            .map_err(|e| err(format!("asset {key} unreadable: {e}")))?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
        data_urls.insert(key.clone(), format!("data:{};base64,{}", asset.mime, encoded));
    }
    Ok(data_urls)
}

/// Collapse a loaded theme into its gallery summary.
pub fn summarize(theme: LoadedTheme) -> ThemeSummary {
    let preview = theme
        .meta
        .previews
        .first()
        .map(|rel| theme.dir.join(rel));
    ThemeSummary {
        id: theme.config.id.clone(),
        name: theme.config.name.clone(),
        description: theme.config.description.clone(),
        dir: theme.dir.clone(),
        has_native_theme: theme.codex_theme.is_some(),
        colors: theme.config.colors.clone(),
        preview,
        meta: theme.meta,
    }
}

/// Enumerate valid theme packages under a directory; invalid ones are skipped
/// (matching the studio's listing behavior).
pub fn list_themes(themes_root: &Path) -> Vec<ThemeSummary> {
    let Ok(entries) = std::fs::read_dir(themes_root) else {
        return Vec::new();
    };
    let mut themes: Vec<ThemeSummary> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| load_theme(&e.path()).ok())
        .map(summarize)
        .collect();
    themes.sort_by(|a, b| a.id.cmp(&b.id));
    themes
}

/// Resolve a theme reference: an explicit directory (contains theme.json)
/// wins; otherwise it is an id under `themes_root`.
pub fn resolve_theme_dir(themes_root: &Path, id_or_path: &str) -> Result<PathBuf> {
    let direct = PathBuf::from(id_or_path);
    if direct.join("theme.json").is_file() {
        return Ok(direct);
    }
    if !name_pattern(id_or_path) {
        return Err(err(format!("unknown theme: {id_or_path}")));
    }
    let by_id = themes_root.join(id_or_path);
    if by_id.join("theme.json").is_file() {
        return Ok(by_id);
    }
    Err(err(format!("unknown theme: {id_or_path}")))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_min_theme(dir: &Path, id: &str) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            dir.join("theme.json"),
            format!(
                r##"{{
                  "schemaVersion": 2,
                  "id": "{id}",
                  "name": "Test Theme",
                  "colors": {{ "accent": "#ff6a00", "bad": "url(x)" }},
                  "strings": {{ "hero-title": "hello" }}
                }}"##
            ),
        )
        .unwrap();
        std::fs::write(dir.join("theme.css"), "html.codex-theme-studio {}\n").unwrap();
    }

    #[test]
    fn loads_and_sanitizes_a_minimal_theme() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("mini");
        write_min_theme(&dir, "mini");
        let theme = load_theme(&dir).unwrap();
        assert_eq!(theme.config.id, "mini");
        // Invalid color dropped, valid one kept.
        assert_eq!(theme.config.colors.len(), 1);
        assert_eq!(theme.config.colors["accent"], "#ff6a00");
        assert_eq!(theme.config.strings["hero-title"], "hello");
        assert!(theme.chrome_html.is_none());
        assert!(theme.codex_theme.is_none());
    }

    #[test]
    fn rejects_wrong_schema_and_bad_ids() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("bad");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("theme.json"), r#"{"schemaVersion":1,"id":"x"}"#).unwrap();
        assert!(load_theme(&dir).is_err());
        std::fs::write(dir.join("theme.json"), r#"{"schemaVersion":2,"id":"Bad_ID"}"#).unwrap();
        assert!(load_theme(&dir).is_err());
    }

    #[test]
    fn refuses_asset_escape_and_oversize() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("escape");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("theme.json"),
            r##"{"schemaVersion":2,"id":"escape","assets":{"wall":"../outside.png"}}"##,
        )
        .unwrap();
        std::fs::write(dir.join("theme.css"), "").unwrap();
        let error = load_theme(&dir).unwrap_err().to_string();
        assert!(error.contains("inside the theme directory"), "{error}");
    }

    #[test]
    fn resolve_prefers_explicit_dir_then_id() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("themes");
        write_min_theme(&root.join("alpha"), "alpha");
        let by_id = resolve_theme_dir(&root, "alpha").unwrap();
        assert!(by_id.ends_with("alpha"));
        let by_path = resolve_theme_dir(&root, by_id.to_str().unwrap()).unwrap();
        assert_eq!(by_path, by_id);
        assert!(resolve_theme_dir(&root, "missing").is_err());
        // A path-shaped reference that doesn't exist must not be treated as id.
        assert!(resolve_theme_dir(&root, "../alpha").is_err());
    }

    #[test]
    fn listing_skips_invalid_and_sorts() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("themes");
        write_min_theme(&root.join("zeta"), "zeta");
        write_min_theme(&root.join("alpha"), "alpha");
        std::fs::create_dir_all(root.join("broken")).unwrap();
        std::fs::write(root.join("broken/theme.json"), "not json").unwrap();
        let listed = list_themes(&root);
        assert_eq!(
            listed.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
            ["alpha", "zeta"]
        );
    }
}
