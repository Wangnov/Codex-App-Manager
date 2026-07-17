//! Strongly-typed native Codex theme contract (the package's `codexTheme`
//! block) plus the official `codex-theme-v1:` share-string serialization.
//!
//! Codex's Settings → Appearance importer accepts one share string per
//! light/dark variant. Theme packages keep the structured block as the single
//! source of truth; both share strings are derived deterministically from it
//! (fixed field order), re-parsed and structurally verified before any apply.
//! The strings themselves are used for contract validation, tests and
//! diagnostics — the apply paths write the equivalent settings directly.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Result, ThemeEngineError};

pub const SHARE_PREFIX: &str = "codex-theme-v1:";
pub const MIN_INK_SURFACE_CONTRAST: f64 = 4.5;
const MAX_FONT_LEN: usize = 240;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Variant {
    Dark,
    Light,
}

impl Variant {
    pub const ALL: [Variant; 2] = [Variant::Dark, Variant::Light];

    pub fn key(self) -> &'static str {
        match self {
            Variant::Dark => "dark",
            Variant::Light => "light",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AppearanceTheme {
    Dark,
    Light,
    System,
}

impl AppearanceTheme {
    pub fn as_str(self) -> &'static str {
        match self {
            AppearanceTheme::Dark => "dark",
            AppearanceTheme::Light => "light",
            AppearanceTheme::System => "system",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeThemeIds {
    pub dark: String,
    pub light: String,
}

impl CodeThemeIds {
    pub fn get(&self, variant: Variant) -> &str {
        match variant {
            Variant::Dark => &self.dark,
            Variant::Light => &self.light,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VariantFonts {
    pub code: Option<String>,
    pub ui: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticColors {
    pub diff_added: String,
    pub diff_removed: String,
    pub skill: String,
}

/// One ChromeTheme (the `theme` object of a share string / the value of the
/// `appearance*ChromeTheme` setting).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChromeTheme {
    pub accent: String,
    pub contrast: u8,
    pub ink: String,
    pub opaque_windows: bool,
    pub surface: String,
    pub fonts: VariantFonts,
    pub semantic_colors: SemanticColors,
}

/// A package's full native theme: both ChromeThemes, the appearance switch and
/// (for the strict delivery profile) both code theme ids.
#[derive(Debug, Clone, PartialEq)]
pub struct CodexTheme {
    pub appearance_theme: AppearanceTheme,
    /// `None` only for legacy v2 packages; a full apply must refuse those
    /// (`ThemePackageMissingNativeCodeThemeIds`), try-on may degrade.
    pub code_theme_ids: Option<CodeThemeIds>,
    pub dark: ChromeTheme,
    pub light: ChromeTheme,
}

impl CodexTheme {
    pub fn variant(&self, variant: Variant) -> &ChromeTheme {
        match variant {
            Variant::Dark => &self.dark,
            Variant::Light => &self.light,
        }
    }
}

/// Official share-string payload. Field DECLARATION ORDER is the wire order
/// (serde serializes struct fields in order): codeThemeId, theme{accent,
/// contrast, fonts{code,ui}, ink, opaqueWindows, semanticColors{diffAdded,
/// diffRemoved, skill}, surface}, variant.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SharePayload {
    pub code_theme_id: String,
    pub theme: ShareTheme,
    pub variant: Variant,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ShareTheme {
    pub accent: String,
    pub contrast: u8,
    pub fonts: VariantFonts,
    pub ink: String,
    pub opaque_windows: bool,
    pub semantic_colors: SemanticColors,
    pub surface: String,
}

fn is_hex6(value: &str) -> bool {
    value.len() == 7
        && value.starts_with('#')
        && value[1..].bytes().all(|b| b.is_ascii_hexdigit())
}

fn is_code_theme_id(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.is_empty() || bytes.len() > 128 {
        return false;
    }
    if !bytes[0].is_ascii_alphanumeric() {
        return false;
    }
    bytes[1..]
        .iter()
        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn relative_luminance(hex: &str) -> f64 {
    let channel = |i: usize| -> f64 {
        let raw = u8::from_str_radix(&hex[1 + i * 2..3 + i * 2], 16).unwrap_or(0) as f64 / 255.0;
        if raw <= 0.04045 {
            raw / 12.92
        } else {
            ((raw + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * channel(0) + 0.7152 * channel(1) + 0.0722 * channel(2)
}

/// WCAG contrast ratio between two `#RRGGBB` colors.
pub fn contrast_ratio(foreground: &str, background: &str) -> f64 {
    let a = relative_luminance(foreground);
    let b = relative_luminance(background);
    (a.max(b) + 0.05) / (a.min(b) + 0.05)
}

#[derive(Debug, Clone, Copy)]
pub struct ValidateOptions {
    /// Strict delivery profile requires both code theme ids; try-on may relax.
    pub require_code_theme_ids: bool,
    pub minimum_contrast: f64,
}

impl Default for ValidateOptions {
    fn default() -> Self {
        Self {
            require_code_theme_ids: true,
            minimum_contrast: MIN_INK_SURFACE_CONTRAST,
        }
    }
}

fn hex_field(obj: &Value, path: &str, key: &str, problems: &mut Vec<String>) -> Option<String> {
    match obj.get(key).and_then(Value::as_str) {
        Some(s) if is_hex6(s) => Some(s.to_string()),
        _ => {
            problems.push(format!("{path}.{key} must be #RRGGBB"));
            None
        }
    }
}

fn font_field(fonts: &Value, path: &str, key: &str, problems: &mut Vec<String>) -> Option<String> {
    match fonts.get(key) {
        Some(Value::Null) | None => None,
        Some(Value::String(s)) if !s.trim().is_empty() && s.len() <= MAX_FONT_LEN => {
            Some(s.clone())
        }
        Some(_) => {
            problems.push(format!(
                "{path}.fonts.{key} must be null or a non-empty string up to {MAX_FONT_LEN} characters"
            ));
            None
        }
    }
}

fn parse_variant_block(
    value: Option<&Value>,
    path: &str,
    minimum_contrast: f64,
    problems: &mut Vec<String>,
) -> Option<ChromeTheme> {
    let Some(obj) = value.filter(|v| v.is_object()) else {
        problems.push(format!("{path} must be an object"));
        return None;
    };
    let before = problems.len();

    let accent = hex_field(obj, path, "accent", problems);
    let ink = hex_field(obj, path, "ink", problems);
    let surface = hex_field(obj, path, "surface", problems);

    let contrast = match obj.get("contrast").and_then(Value::as_i64) {
        Some(c) if (0..=100).contains(&c) => Some(c as u8),
        _ => {
            problems.push(format!("{path}.contrast must be an integer from 0 to 100"));
            None
        }
    };
    let opaque_windows = match obj.get("opaqueWindows").and_then(Value::as_bool) {
        Some(b) => Some(b),
        None => {
            problems.push(format!("{path}.opaqueWindows must be a boolean"));
            None
        }
    };

    let fonts = match obj.get("fonts").filter(|v| v.is_object()) {
        Some(f) => VariantFonts {
            code: font_field(f, path, "code", problems),
            ui: font_field(f, path, "ui", problems),
        },
        None => {
            problems.push(format!("{path}.fonts must be an object"));
            VariantFonts { code: None, ui: None }
        }
    };

    let semantic = match obj.get("semanticColors").filter(|v| v.is_object()) {
        Some(sc) => {
            let diff_added = hex_field(sc, &format!("{path}.semanticColors"), "diffAdded", problems);
            let diff_removed =
                hex_field(sc, &format!("{path}.semanticColors"), "diffRemoved", problems);
            let skill = hex_field(sc, &format!("{path}.semanticColors"), "skill", problems);
            match (diff_added, diff_removed, skill) {
                (Some(a), Some(r), Some(s)) => Some(SemanticColors {
                    diff_added: a,
                    diff_removed: r,
                    skill: s,
                }),
                _ => None,
            }
        }
        None => {
            problems.push(format!("{path}.semanticColors must be an object"));
            None
        }
    };

    if let (Some(ink), Some(surface)) = (&ink, &surface) {
        let ratio = contrast_ratio(ink, surface);
        if ratio < minimum_contrast {
            problems.push(format!(
                "{path} ink/surface contrast {ratio:.2}:1 is below {minimum_contrast}:1"
            ));
        }
    }

    if problems.len() > before {
        return None;
    }
    Some(ChromeTheme {
        accent: accent?,
        contrast: contrast?,
        ink: ink?,
        opaque_windows: opaque_windows?,
        surface: surface?,
        fonts,
        semantic_colors: semantic?,
    })
}

/// Validate a package's `codexTheme` block and lift it into strong types.
/// Every problem is reported (not just the first), mirroring the studio's
/// validator so both ends of the contract reject identically.
pub fn parse_codex_theme(value: &Value, options: ValidateOptions) -> Result<CodexTheme> {
    let mut problems = Vec::new();
    if !value.is_object() {
        return Err(ThemeEngineError::Theme(
            "codexTheme must be an object".to_string(),
        ));
    }

    let appearance_theme = match value.get("appearanceTheme").and_then(Value::as_str) {
        Some("dark") => Some(AppearanceTheme::Dark),
        Some("light") => Some(AppearanceTheme::Light),
        Some("system") => Some(AppearanceTheme::System),
        _ => {
            problems.push("codexTheme.appearanceTheme must be dark, light, or system".to_string());
            None
        }
    };

    let dark = parse_variant_block(
        value.get("dark"),
        "codexTheme.dark",
        options.minimum_contrast,
        &mut problems,
    );
    let light = parse_variant_block(
        value.get("light"),
        "codexTheme.light",
        options.minimum_contrast,
        &mut problems,
    );

    let code_theme_ids = match value.get("codeThemeIds").filter(|v| v.is_object()) {
        Some(ids) => {
            let mut pick = |variant: &str| match ids.get(variant).and_then(Value::as_str) {
                Some(id) if is_code_theme_id(id) => Some(id.to_string()),
                _ => {
                    problems.push(format!(
                        "codexTheme.codeThemeIds.{variant} must be a valid Codex code theme id"
                    ));
                    None
                }
            };
            match (pick("dark"), pick("light")) {
                (Some(dark), Some(light)) => Some(CodeThemeIds { dark, light }),
                _ => None,
            }
        }
        None => {
            if options.require_code_theme_ids {
                problems.push("codexTheme.codeThemeIds must be an object".to_string());
            }
            None
        }
    };

    if !problems.is_empty() {
        return Err(ThemeEngineError::Theme(format!(
            "invalid codexTheme: {}",
            problems.join("; ")
        )));
    }
    Ok(CodexTheme {
        appearance_theme: appearance_theme.expect("validated"),
        code_theme_ids,
        dark: dark.expect("validated"),
        light: light.expect("validated"),
    })
}

/// Build the official share string for one variant. Requires code theme ids
/// (the official payload has no id-less form).
pub fn share_string(theme: &CodexTheme, variant: Variant) -> Result<String> {
    let ids = theme.code_theme_ids.as_ref().ok_or_else(|| {
        ThemeEngineError::Theme(
            "ThemePackageMissingNativeCodeThemeIds: 包缺少 codexTheme.codeThemeIds".to_string(),
        )
    })?;
    let source = theme.variant(variant);
    let payload = SharePayload {
        code_theme_id: ids.get(variant).to_string(),
        theme: ShareTheme {
            accent: source.accent.clone(),
            contrast: source.contrast,
            fonts: source.fonts.clone(),
            ink: source.ink.clone(),
            opaque_windows: source.opaque_windows,
            semantic_colors: source.semantic_colors.clone(),
            surface: source.surface.clone(),
        },
        variant,
    };
    let json = serde_json::to_string(&payload)
        .map_err(|e| ThemeEngineError::Theme(format!("share serialize: {e}")))?;
    Ok(format!("{SHARE_PREFIX}{json}"))
}

/// Parse + structurally verify a share string (round-trip gate).
pub fn parse_share_string(value: &str, expected_variant: Variant) -> Result<SharePayload> {
    let trimmed = value.trim();
    let body = trimmed
        .strip_prefix(SHARE_PREFIX)
        .ok_or_else(|| ThemeEngineError::Theme("share string prefix mismatch".to_string()))?;
    let payload: SharePayload = serde_json::from_str(body)
        .map_err(|e| ThemeEngineError::Theme(format!("share parse: {e}")))?;
    if payload.variant != expected_variant {
        return Err(ThemeEngineError::Theme("share variant mismatch".to_string()));
    }
    if !is_code_theme_id(&payload.code_theme_id) {
        return Err(ThemeEngineError::Theme("share codeThemeId invalid".to_string()));
    }
    Ok(payload)
}

/// Derive both share strings and verify each round-trips — the §4 contract
/// gate a package must pass before any native apply.
pub fn verified_share_strings(theme: &CodexTheme) -> Result<[(Variant, String); 2]> {
    let render = |variant: Variant| -> Result<(Variant, String)> {
        let s = share_string(theme, variant)?;
        let parsed = parse_share_string(&s, variant)?;
        let source = theme.variant(variant);
        let matches = parsed.theme.accent == source.accent
            && parsed.theme.contrast == source.contrast
            && parsed.theme.fonts == source.fonts
            && parsed.theme.ink == source.ink
            && parsed.theme.opaque_windows == source.opaque_windows
            && parsed.theme.semantic_colors == source.semantic_colors
            && parsed.theme.surface == source.surface;
        if !matches {
            return Err(ThemeEngineError::Theme(format!(
                "share round-trip mismatch for {}",
                variant.key()
            )));
        }
        Ok((variant, s))
    };
    Ok([render(Variant::Dark)?, render(Variant::Light)?])
}

/// The ChromeTheme as the JSON value Codex's `appearance*ChromeTheme` setting
/// stores (camelCase, same shape as the share string's `theme` object).
pub fn chrome_theme_value(theme: &ChromeTheme) -> Value {
    serde_json::json!({
        "accent": theme.accent,
        "contrast": theme.contrast,
        "fonts": { "code": theme.fonts.code, "ui": theme.fonts.ui },
        "ink": theme.ink,
        "opaqueWindows": theme.opaque_windows,
        "semanticColors": {
            "diffAdded": theme.semantic_colors.diff_added,
            "diffRemoved": theme.semantic_colors.diff_removed,
            "skill": theme.semantic_colors.skill,
        },
        "surface": theme.surface,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The SPEC §3 example block, verbatim.
    fn spec_block() -> Value {
        serde_json::json!({
            "appearanceTheme": "dark",
            "codeThemeIds": { "dark": "absolutely", "light": "absolutely" },
            "dark": {
                "accent": "#E8A33D",
                "contrast": 60,
                "ink": "#F7E8C2",
                "opaqueWindows": true,
                "surface": "#191A1D",
                "fonts": { "code": "SF Mono", "ui": "SF Pro Display, PingFang SC" },
                "semanticColors": {
                    "diffAdded": "#46C077",
                    "diffRemoved": "#D64541",
                    "skill": "#E8A33D"
                }
            },
            "light": {
                "accent": "#A65E00",
                "contrast": 60,
                "ink": "#3A2419",
                "opaqueWindows": true,
                "surface": "#FFF8E8",
                "fonts": { "code": "SF Mono", "ui": "SF Pro Display, PingFang SC" },
                "semanticColors": {
                    "diffAdded": "#24844F",
                    "diffRemoved": "#B53632",
                    "skill": "#8D5700"
                }
            }
        })
    }

    #[test]
    fn share_string_matches_spec_bytes() {
        let theme = parse_codex_theme(&spec_block(), ValidateOptions::default()).unwrap();
        let dark = share_string(&theme, Variant::Dark).unwrap();
        // SPEC §4's exact expected string, byte for byte.
        assert_eq!(
            dark,
            r##"codex-theme-v1:{"codeThemeId":"absolutely","theme":{"accent":"#E8A33D","contrast":60,"fonts":{"code":"SF Mono","ui":"SF Pro Display, PingFang SC"},"ink":"#F7E8C2","opaqueWindows":true,"semanticColors":{"diffAdded":"#46C077","diffRemoved":"#D64541","skill":"#E8A33D"},"surface":"#191A1D"},"variant":"dark"}"##
        );
        let light = share_string(&theme, Variant::Light).unwrap();
        assert!(light.contains(r#""variant":"light""#));
        assert!(light.contains(r##""surface":"#FFF8E8""##));
        verified_share_strings(&theme).unwrap();
    }

    #[test]
    fn share_round_trip_parses_back() {
        let theme = parse_codex_theme(&spec_block(), ValidateOptions::default()).unwrap();
        let s = share_string(&theme, Variant::Light).unwrap();
        let payload = parse_share_string(&s, Variant::Light).unwrap();
        assert_eq!(payload.code_theme_id, "absolutely");
        assert_eq!(payload.theme.surface, "#FFF8E8");
        assert!(parse_share_string(&s, Variant::Dark).is_err(), "variant gate");
    }

    #[test]
    fn rejects_each_contract_violation() {
        let cases: Vec<(&str, Box<dyn Fn(&mut Value)>)> = vec![
            ("missing dark", Box::new(|v| { v.as_object_mut().unwrap().remove("dark"); })),
            ("missing light", Box::new(|v| { v.as_object_mut().unwrap().remove("light"); })),
            ("bad hex", Box::new(|v| v["dark"]["accent"] = "#zzz".into())),
            ("short hex", Box::new(|v| v["dark"]["ink"] = "#fff".into())),
            ("contrast range", Box::new(|v| v["light"]["contrast"] = 101.into())),
            ("contrast type", Box::new(|v| v["light"]["contrast"] = "60".into())),
            ("opaque type", Box::new(|v| v["dark"]["opaqueWindows"] = "yes".into())),
            ("font empty", Box::new(|v| v["dark"]["fonts"]["code"] = "  ".into())),
            ("appearance", Box::new(|v| v["appearanceTheme"] = "auto".into())),
            ("code id", Box::new(|v| v["codeThemeIds"]["dark"] = "-bad".into())),
            (
                "low contrast",
                Box::new(|v| {
                    v["dark"]["ink"] = "#191A1D".into(); // ink == surface
                }),
            ),
        ];
        for (name, mutate) in cases {
            let mut block = spec_block();
            mutate(&mut block);
            assert!(
                parse_codex_theme(&block, ValidateOptions::default()).is_err(),
                "case must be rejected: {name}"
            );
        }
    }

    #[test]
    fn legacy_package_without_ids_is_lenient_only() {
        let mut block = spec_block();
        block.as_object_mut().unwrap().remove("codeThemeIds");
        assert!(parse_codex_theme(&block, ValidateOptions::default()).is_err());
        let lenient = parse_codex_theme(
            &block,
            ValidateOptions { require_code_theme_ids: false, ..Default::default() },
        )
        .unwrap();
        assert!(lenient.code_theme_ids.is_none());
        assert!(
            share_string(&lenient, Variant::Dark).is_err(),
            "share strings require ids"
        );
    }

    #[test]
    fn contrast_math_matches_wcag() {
        assert!((contrast_ratio("#000000", "#ffffff") - 21.0).abs() < 0.01);
        assert!((contrast_ratio("#ffffff", "#ffffff") - 1.0).abs() < 0.001);
        // SPEC example dark variant passes 4.5:1.
        assert!(contrast_ratio("#F7E8C2", "#191A1D") >= 4.5);
    }
}
