//! Hot native-theme application over CDP — the running-Codex counterpart of
//! [`crate::native`]'s stopped-Codex file path.
//!
//! Codex's renderer ships a settings module whose `get-setting`/`set-setting`
//! wrappers post to the Electron main process; writing the five appearance
//! settings there applies LIVE (`applySettingSideEffects` refreshes window
//! backdrops and the settings query invalidates across views) and persists
//! through Codex's own store. This module locates those wrappers at runtime —
//! the chunk file names and minified export aliases change per Codex build,
//! so discovery is structural: scan the loaded chunks for the wrappers' exact
//! minified shape, resolve their export aliases, `import()` the chunk (which
//! dedupes into the live module graph) and cache the functions on `window`.
//!
//! Discovery is version-adapted: 26.707 keeps the eager loaded-chunk scan,
//! while 26.715 follows the Vite dependency manifest to its lazy
//! `setting-storage-*` chunk. A version hint only controls probe order; every
//! adapter still proves the module structurally before it is used, so a stale
//! installed-version cache cannot select an incompatible implementation.

use serde_json::Value;

use crate::cdp::CdpSession;
use crate::codex_theme::CodexTheme;
use crate::native::NativeSettingsSnapshot;
use crate::{Result, ThemeEngineError};

fn err(message: impl Into<String>) -> ThemeEngineError {
    ThemeEngineError::Cdp(message.into())
}

/// The five managed setting keys (same logical units as `native::TOP_KEYS` +
/// sections, in settings-store form).
pub const SETTING_KEYS: [&str; 5] = [
    "appearanceTheme",
    "appearanceDarkChromeTheme",
    "appearanceLightChromeTheme",
    "appearanceDarkCodeThemeId",
    "appearanceLightCodeThemeId",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SettingsAdapter {
    V26_707,
    V26_715,
}

impl SettingsAdapter {
    const fn id(self) -> &'static str {
        match self {
            Self::V26_707 => "26.707",
            Self::V26_715 => "26.715",
        }
    }
}

fn preferred_adapter(version_hint: Option<&str>) -> Option<SettingsAdapter> {
    let train = version_hint?
        .trim()
        .split('.')
        .nth(1)?
        .parse::<u32>()
        .ok()?;
    match train {
        707..=714 => Some(SettingsAdapter::V26_707),
        715.. => Some(SettingsAdapter::V26_715),
        _ => None,
    }
}

fn adapter_order(version_hint: Option<&str>) -> [SettingsAdapter; 2] {
    match preferred_adapter(version_hint) {
        Some(SettingsAdapter::V26_707) => [SettingsAdapter::V26_707, SettingsAdapter::V26_715],
        Some(SettingsAdapter::V26_715) | None => {
            [SettingsAdapter::V26_715, SettingsAdapter::V26_707]
        }
    }
}

/// Locate + cache the renderer's settings API (idempotent). The cache lives on
/// `window.__camThemeSettingsV1`, so repeated ops skip the chunk scan.
const PREFERRED_ADAPTER_TOKEN: &str = "__CAM_PREFERRED_ADAPTER__";

const ENSURE_API_JS_TEMPLATE: &str = r#"(async () => {
  const w = window;
  if (w.__camThemeSettingsV1?.read && w.__camThemeSettingsV1?.write) {
    return {
      ok: true,
      cached: true,
      adapter: w.__camThemeSettingsV1.adapter,
      url: w.__camThemeSettingsV1.url,
    };
  }
  const preferred = "__CAM_PREFERRED_ADAPTER__";
  const adapters = preferred === "26.707"
    ? ["26.707", "26.715"]
    : ["26.715", "26.707"];
  const loadedUrls = [...new Set([
    ...performance.getEntriesByType("resource").map((r) => r.name),
    ...[...document.querySelectorAll("script[src]")].map((el) => el.src),
    ...[...document.querySelectorAll('link[rel="modulepreload"]')].map((el) => el.href),
  ])].filter((u) => u.includes(".js"));
  const writeRe = /async function (\w+)\(e,t\)\{await (\w+)\([`'"]set-setting[`'"],\{params:\{key:e\.key,value:t\}\}\)\}/;
  const readRe = /async function (\w+)\(e\)\{return\(await (\w+)\([`'"]get-setting[`'"],\{params:\{key:e\.key\}\}\)\)\.value\?\?e\.default\}/;
  const fetched = new Map();
  const checked = new Set();
  const fetchText = async (url) => {
    if (fetched.has(url)) return fetched.get(url);
    let text = null;
    try {
      const response = await fetch(url);
      // Electron custom-protocol responses can expose a readable body while
      // `Response.ok` is false. Structural validation below is authoritative.
      text = await response.text();
    } catch {}
    fetched.set(url, text);
    if (text != null) checked.add(url);
    return text;
  };
  const loadedSettingChunks = loadedUrls.filter((url) =>
    /(?:^|\/)setting-storage-[A-Za-z0-9_-]+\.js(?:$|[?#])/.test(url)
  );
  const lazy715Candidates = async () => {
    const candidates = new Set(loadedSettingChunks);
    const chunkRef = /(?:\.\/)?setting-storage-[A-Za-z0-9_-]+\.js/g;
    for (const sourceUrl of loadedUrls) {
      const text = await fetchText(sourceUrl);
      if (text == null) continue;
      for (const match of text.matchAll(chunkRef)) {
        try { candidates.add(new URL(match[0], sourceUrl).href); } catch {}
      }
    }
    return [...candidates];
  };
  const resolveModule = async (adapter, candidates) => {
    for (const url of candidates) {
      const text = await fetchText(url);
      if (text == null || !text.includes("set-setting")) continue;
      const writeMatch = text.match(writeRe);
      const readMatch = text.match(readRe);
      if (!writeMatch || !readMatch) continue;
      const aliasOf = (name) => {
        const match = text.match(new RegExp("\\b" + name + " as (\\w+)"));
        return match ? match[1] : null;
      };
      const writeAlias = aliasOf(writeMatch[1]);
      const readAlias = aliasOf(readMatch[1]);
      if (!writeAlias || !readAlias) continue;
      let mod;
      try { mod = await import(url); } catch { continue; }
      const read = mod[readAlias];
      const write = mod[writeAlias];
      if (typeof read !== "function" || typeof write !== "function") continue;
      w.__camThemeSettingsV1 = { read, write, url, adapter };
      return { ok: true, cached: false, adapter, url, checked: checked.size };
    }
    return null;
  };
  for (const adapter of adapters) {
    const candidates = adapter === "26.715" ? await lazy715Candidates() : loadedUrls;
    const outcome = await resolveModule(adapter, candidates);
    if (outcome != null) return outcome;
  }
  return {
    ok: false,
    error: "settings module not found (adapters " + adapters.join(", ") +
      "; " + checked.size + " chunks scanned)",
  };
})()"#;

fn ensure_api_expression(version_hint: Option<&str>) -> String {
    let preferred = adapter_order(version_hint)[0].id();
    ENSURE_API_JS_TEMPLATE.replace(PREFERRED_ADAPTER_TOKEN, preferred)
}

#[derive(Debug, serde::Deserialize)]
struct JsOutcome {
    ok: bool,
    #[serde(default)]
    error: Option<String>,
    #[serde(default)]
    values: Option<serde_json::Map<String, Value>>,
    #[serde(default)]
    adapter: Option<String>,
    #[serde(default)]
    url: Option<String>,
}

async fn run_js(session: &CdpSession, expression: &str, what: &str) -> Result<JsOutcome> {
    let value = session.evaluate(expression).await?;
    let outcome: JsOutcome = serde_json::from_value(value)
        .map_err(|e| err(format!("{what}: 结果解析失败: {e}")))?;
    if !outcome.ok {
        return Err(err(format!(
            "hot-import-unsupported: {what}: {}",
            outcome.error.as_deref().unwrap_or("unknown")
        )));
    }
    Ok(outcome)
}

/// Make sure the settings API is reachable in this renderer. Cheap when
/// already cached; a clean error otherwise (callers fall back to the file
/// path).
pub async fn ensure_api(session: &CdpSession, version_hint: Option<&str>) -> Result<()> {
    let expression = ensure_api_expression(version_hint);
    let outcome = run_js(session, &expression, "定位设置接口").await?;
    log::info!(
        "native settings adapter selected adapter={} version_hint={} source={}",
        outcome.adapter.as_deref().unwrap_or("cached-unknown"),
        version_hint.unwrap_or("unknown"),
        outcome.url.as_deref().unwrap_or("cached")
    );
    Ok(())
}

/// Read the five managed settings' live (effective) values.
pub async fn read_snapshot(
    session: &CdpSession,
    version_hint: Option<&str>,
) -> Result<NativeSettingsSnapshot> {
    ensure_api(session, version_hint).await?;
    let keys_json = serde_json::to_string(&SETTING_KEYS).expect("static keys");
    let expression = format!(
        r#"(async () => {{
  const api = window.__camThemeSettingsV1;
  if (!api) return {{ ok: false, error: "api not initialized" }};
  try {{
    const values = {{}};
    for (const key of {keys_json}) values[key] = await api.read({{ key }});
    return {{ ok: true, values }};
  }} catch (e) {{
    return {{ ok: false, error: String(e) }};
  }}
}})()"#
    );
    let outcome = run_js(session, &expression, "读取外观设置").await?;
    let mut values = outcome.values.unwrap_or_default();
    let mut take = |key: &str| values.remove(key).filter(|v| !v.is_null());
    Ok(NativeSettingsSnapshot {
        appearance_theme: take("appearanceTheme"),
        dark_chrome: take("appearanceDarkChromeTheme"),
        light_chrome: take("appearanceLightChromeTheme"),
        dark_code_id: take("appearanceDarkCodeThemeId"),
        light_code_id: take("appearanceLightCodeThemeId"),
    })
}

/// Write settings sequentially; the main process zod-parses each value, so a
/// malformed one fails loudly (and we report which key). No partial-failure
/// rollback here — callers hold the pre-write snapshot and decide.
pub async fn write_values(
    session: &CdpSession,
    entries: &[(&str, Value)],
    version_hint: Option<&str>,
) -> Result<()> {
    if entries.is_empty() {
        return Ok(());
    }
    ensure_api(session, version_hint).await?;
    let payload: Vec<Value> = entries
        .iter()
        .map(|(key, value)| serde_json::json!([key, value]))
        .collect();
    let payload_json = serde_json::to_string(&payload)
        .map_err(|e| err(format!("写入负载序列化失败: {e}")))?;
    let expression = format!(
        r#"(async () => {{
  const api = window.__camThemeSettingsV1;
  if (!api) return {{ ok: false, error: "api not initialized" }};
  const entries = {payload_json};
  for (const [key, value] of entries) {{
    try {{
      await api.write({{ key }}, value);
    }} catch (e) {{
      return {{ ok: false, error: key + ": " + String(e) }};
    }}
  }}
  return {{ ok: true }};
}})()"#
    );
    run_js(session, &expression, "写入外观设置").await.map(|_| ())
}

/// The write set for a typed theme: both ChromeThemes, both code theme ids
/// (when the package carries them — legacy packages degrade to palette-only)
/// and the appearance switch last, so the mode flip lands on fully-written
/// palettes.
pub fn theme_write_entries(theme: &CodexTheme) -> Vec<(&'static str, Value)> {
    let mut entries: Vec<(&'static str, Value)> = vec![
        (
            "appearanceDarkChromeTheme",
            crate::codex_theme::chrome_theme_value(&theme.dark),
        ),
        (
            "appearanceLightChromeTheme",
            crate::codex_theme::chrome_theme_value(&theme.light),
        ),
    ];
    if let Some(ids) = &theme.code_theme_ids {
        entries.push(("appearanceDarkCodeThemeId", Value::String(ids.dark.clone())));
        entries.push((
            "appearanceLightCodeThemeId",
            Value::String(ids.light.clone()),
        ));
    }
    entries.push((
        "appearanceTheme",
        Value::String(theme.appearance_theme.as_str().to_string()),
    ));
    entries
}

/// The write set restoring a previously captured snapshot. Effective reads
/// always yield a value for these keys, so a restore rewrites what the user
/// effectively had; byte-precise deletion of introduced config keys remains
/// the file path's job (`native::restore_native_theme`).
pub fn snapshot_write_entries(snapshot: &NativeSettingsSnapshot) -> Vec<(&'static str, Value)> {
    let mut entries: Vec<(&'static str, Value)> = Vec::new();
    if let Some(v) = &snapshot.dark_chrome {
        entries.push(("appearanceDarkChromeTheme", v.clone()));
    }
    if let Some(v) = &snapshot.light_chrome {
        entries.push(("appearanceLightChromeTheme", v.clone()));
    }
    if let Some(v) = &snapshot.dark_code_id {
        entries.push(("appearanceDarkCodeThemeId", v.clone()));
    }
    if let Some(v) = &snapshot.light_code_id {
        entries.push(("appearanceLightCodeThemeId", v.clone()));
    }
    if let Some(v) = &snapshot.appearance_theme {
        entries.push(("appearanceTheme", v.clone()));
    }
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codex_theme::{parse_codex_theme, ValidateOptions};

    fn theme() -> CodexTheme {
        parse_codex_theme(
            &serde_json::json!({
                "appearanceTheme": "dark",
                "codeThemeIds": { "dark": "absolutely", "light": "absolutely" },
                "dark": {
                    "accent": "#e8a33d", "contrast": 60, "ink": "#f7e8c2",
                    "opaqueWindows": true, "surface": "#191a1d",
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
    fn write_entries_cover_all_five_keys_switch_last() {
        let entries = theme_write_entries(&theme());
        let keys: Vec<&str> = entries.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys.len(), 5);
        assert_eq!(keys.last(), Some(&"appearanceTheme"), "switch flips last");
        for key in SETTING_KEYS {
            assert!(keys.contains(&key), "missing {key}");
        }
        let dark = &entries[0].1;
        assert_eq!(dark["accent"], "#e8a33d");
        assert_eq!(dark["fonts"]["code"], "SF Mono");
        assert_eq!(dark["fonts"]["ui"], Value::Null);
        assert_eq!(dark["semanticColors"]["diffAdded"], "#46c077");
    }

    #[test]
    fn legacy_theme_without_ids_writes_palettes_only() {
        let mut t = theme();
        t.code_theme_ids = None;
        let keys: Vec<&str> = theme_write_entries(&t).iter().map(|(k, _)| *k).collect();
        assert_eq!(keys.len(), 3);
        assert!(!keys.iter().any(|k| k.contains("CodeThemeId")));
    }

    #[test]
    fn snapshot_entries_skip_absent_values() {
        let snapshot = NativeSettingsSnapshot {
            appearance_theme: Some(serde_json::json!("system")),
            dark_chrome: None,
            light_chrome: Some(serde_json::json!({ "accent": "#ffffff" })),
            dark_code_id: None,
            light_code_id: None,
        };
        let entries = snapshot_write_entries(&snapshot);
        let keys: Vec<&str> = entries.iter().map(|(k, _)| *k).collect();
        assert_eq!(keys, vec!["appearanceLightChromeTheme", "appearanceTheme"]);
    }

    #[test]
    fn discovery_script_carries_the_known_minified_shapes() {
        // The wrapper shapes measured in Codex 26.707.91948. The regexes live
        // in page-side JS; here we pin the load-bearing fragments so an
        // accidental edit of ENSURE_API_JS fails loudly.
        for fragment in [
            r"async function (\w+)\(e,t\)\{await (\w+)\(",
            "set-setting",
            "get-setting",
            "__camThemeSettingsV1",
            "modulepreload",
            "await import(url)",
        ] {
            assert!(
                ENSURE_API_JS_TEMPLATE.contains(fragment),
                "ENSURE_API_JS_TEMPLATE lost fragment: {fragment}"
            );
        }
    }

    #[test]
    fn adapter_order_tracks_supported_codex_trains() {
        assert_eq!(
            adapter_order(Some("26.707.9981.0")),
            [SettingsAdapter::V26_707, SettingsAdapter::V26_715]
        );
        assert_eq!(
            adapter_order(Some("26.715.2305.0")),
            [SettingsAdapter::V26_715, SettingsAdapter::V26_707]
        );
    }

    #[test]
    fn unknown_or_new_version_probes_latest_adapter_first() {
        assert_eq!(
            adapter_order(Some("unexpected")),
            [SettingsAdapter::V26_715, SettingsAdapter::V26_707]
        );
        assert_eq!(
            adapter_order(Some("26.706.1")),
            [SettingsAdapter::V26_715, SettingsAdapter::V26_707]
        );
        assert_eq!(
            adapter_order(None),
            [SettingsAdapter::V26_715, SettingsAdapter::V26_707]
        );
    }

    #[test]
    fn discovery_script_embeds_versioned_probe_order_and_lazy_715_chunk() {
        let legacy = ensure_api_expression(Some("26.707.9981.0"));
        let current = ensure_api_expression(Some("26.715.2305.0"));
        assert!(legacy.contains(r#"const preferred = "26.707""#));
        assert!(current.contains(r#"const preferred = "26.715""#));
        assert!(current.contains("setting-storage-"));
        assert!(current.contains(r#"["26.715", "26.707"]"#));
        assert!(!current.contains("response.ok"));
    }
}
