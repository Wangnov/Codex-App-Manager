//! Hot native-theme application over CDP — the running-Codex counterpart of
//! [`crate::native`]'s stopped-Codex file path.
//!
//! Codex's renderer ships a settings module whose `get-setting`/`set-setting`
//! wrappers post to the Electron main process; writing the five appearance
//! settings there applies LIVE (`applySettingSideEffects` refreshes window
//! backdrops and the settings query invalidates across views) and persists
//! through Codex's own store. This module locates those wrappers at runtime —
//! the chunk file names and minified export aliases change per Codex build,
//! so discovery is structural: scan candidate chunks, resolve the exported
//! wrappers, `import()` the chunk (which dedupes into the live module graph)
//! and cache the functions on `window`.
//!
//! Discovery is version-adapted: 26.707 keeps the eager loaded-chunk scan,
//! 26.715 follows the Vite dependency manifest to its lazy
//! `setting-storage-*` chunk, and 26.831.21537+ accepts the full ASCII
//! JavaScript identifier shape after Rolldown first minified the write wrapper
//! to `$D`. A version hint only controls probe order; every adapter still proves
//! the module structurally before it is used, so a stale installed-version
//! cache cannot select an incompatible implementation.
//! 26.901.20858 added native app-host settings branches. Its adapter inspects
//! exported wrappers' parameter/payload contracts instead of matching their
//! entire bodies, retaining Codex's own native/legacy dispatch and defaults.

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
    V26_831_21537,
    V26_901_20858,
    V26_1111_21308,
}

impl SettingsAdapter {
    const fn id(self) -> &'static str {
        match self {
            Self::V26_707 => "26.707",
            Self::V26_715 => "26.715",
            Self::V26_831_21537 => "26.831.21537+",
            Self::V26_901_20858 => "26.901.20858+",
            Self::V26_1111_21308 => "26.1111.21308+",
        }
    }
}

// Mirror package comparison pins the boundary: 26.831.20005 still emitted
// `nO`/`tO`, while the next published build, 26.831.21537, emitted `$D`/`QD`.
const FULL_JS_IDENTIFIER_MIN_VERSION: [u32; 3] = [26, 831, 21537];
// Mirror's adjacent macOS packages: 26.831.21537 has direct RPC wrappers;
// 26.901.20858 (build 7658) first adds app-host settings.read/write branches.
const NATIVE_SETTINGS_BRIDGE_MIN_VERSION: [u32; 3] = [26, 901, 20858];
// Codex 26.1111.21308 switched webview bundles from Vite/Rolldown ESM chunks
// to Rspack ESM chunks whose settings module (`RxZZ`) runs through `__webpack_require__`
// and wraps `appServices.settings.{read,write}` in `(0,s.P)(...)`.
const RSPACK_SETTINGS_BRIDGE_MIN_VERSION: [u32; 3] = [26, 1111, 21308];

fn version_triplet(version: &str) -> Option<[u32; 3]> {
    let mut parts = version.trim().split('.');
    Some([
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
        parts.next()?.parse().ok()?,
    ])
}

fn preferred_adapter(version_hint: Option<&str>) -> Option<SettingsAdapter> {
    let version = version_triplet(version_hint?)?;
    if version >= RSPACK_SETTINGS_BRIDGE_MIN_VERSION {
        return Some(SettingsAdapter::V26_1111_21308);
    }
    if version >= NATIVE_SETTINGS_BRIDGE_MIN_VERSION {
        return Some(SettingsAdapter::V26_901_20858);
    }
    if version >= FULL_JS_IDENTIFIER_MIN_VERSION {
        return Some(SettingsAdapter::V26_831_21537);
    }
    let train = version[1];
    match train {
        707..=714 => Some(SettingsAdapter::V26_707),
        715.. => Some(SettingsAdapter::V26_715),
        _ => None,
    }
}

fn adapter_order(version_hint: Option<&str>) -> [SettingsAdapter; 5] {
    match preferred_adapter(version_hint) {
        Some(SettingsAdapter::V26_707) => [
            SettingsAdapter::V26_707,
            SettingsAdapter::V26_715,
            SettingsAdapter::V26_831_21537,
            SettingsAdapter::V26_901_20858,
            SettingsAdapter::V26_1111_21308,
        ],
        Some(SettingsAdapter::V26_715) => [
            SettingsAdapter::V26_715,
            SettingsAdapter::V26_707,
            SettingsAdapter::V26_831_21537,
            SettingsAdapter::V26_901_20858,
            SettingsAdapter::V26_1111_21308,
        ],
        Some(SettingsAdapter::V26_831_21537) => [
            SettingsAdapter::V26_831_21537,
            SettingsAdapter::V26_715,
            SettingsAdapter::V26_707,
            SettingsAdapter::V26_901_20858,
            SettingsAdapter::V26_1111_21308,
        ],
        Some(SettingsAdapter::V26_901_20858) => [
            SettingsAdapter::V26_901_20858,
            SettingsAdapter::V26_1111_21308,
            SettingsAdapter::V26_831_21537,
            SettingsAdapter::V26_715,
            SettingsAdapter::V26_707,
        ],
        Some(SettingsAdapter::V26_1111_21308) | None => [
            SettingsAdapter::V26_1111_21308,
            SettingsAdapter::V26_901_20858,
            SettingsAdapter::V26_831_21537,
            SettingsAdapter::V26_715,
            SettingsAdapter::V26_707,
        ],
    }
}

/// Locate + cache the renderer's settings API (idempotent). The cache lives on
/// `window.__camThemeSettingsV1`, so repeated ops skip the chunk scan.
const ADAPTERS_TOKEN: &str = "__CAM_ADAPTERS__";

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
  const adapters = __CAM_ADAPTERS__;
  const scriptUrls = [...new Set(
    [...document.querySelectorAll("script[src]")].map((el) => el.src),
  )].filter((u) => u.includes(".js"));
  const loadedUrls = [...new Set([
    ...performance.getEntriesByType("resource").map((r) => r.name),
    ...scriptUrls,
    ...[...document.querySelectorAll('link[rel="modulepreload"]')].map((el) => el.href),
  ])].filter((u) => u.includes(".js"));
  const legacyWriteRe = /async function (\w+)\(e,t\)\{await (\w+)\([`'"]set-setting[`'"],\{params:\{key:e\.key,value:t\}\}\)\}/;
  const legacyReadRe = /async function (\w+)\(e\)\{return\(await (\w+)\([`'"]get-setting[`'"],\{params:\{key:e\.key\}\}\)\)\.value\?\?e\.default\}/;
  const modernWriteRe = /async function ([$A-Za-z_][$A-Za-z0-9_]*)\(e,t\)\{await ([$A-Za-z_][$A-Za-z0-9_]*)\([`'"]set-setting[`'"],\{params:\{key:e\.key,value:t\}\}\)\}/;
  const modernReadRe = /async function ([$A-Za-z_][$A-Za-z0-9_]*)\(e\)\{return\(await ([$A-Za-z_][$A-Za-z0-9_]*)\([`'"]get-setting[`'"],\{params:\{key:e\.key\}\}\)\)\.value\?\?e\.default\}/;
  const escapeRegExp = (value) => value.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
  // Resolve the real exported functions, not hard-coded/minified export names.
  // Inspect but never invoke candidates during discovery. The two wrappers
  // must use their own key/value parameters and the same RPC fallback or native
  // settings bridge. Guards and whitespace around that contract may vary.
  const exportedSettingsWrappers = (mod) => {
    if (!mod || (typeof mod !== "object" && typeof mod !== "function")) return null;
    const id = "[$A-Za-z_][$A-Za-z0-9_]*";
    const idChain = id + "(?:\\s*\\.\\s*" + id + ")*";
    const receiverChain = id + "(?:\\s*\\.\\s*(?!settings\\b)" + id + ")*";
    const callWrap = "(?:(?:\\(\\s*0\\s*,\\s*" + idChain + "\\s*\\)|" + id + ")\\s*\\(\\s*)?";
    const paramsRe = new RegExp(
      "^async\\s+function(?:\\s+" + id + ")?\\s*\\(\\s*(" + id +
      ")(?:\\s*,\\s*(" + id + "))?\\s*\\)\\s*\\{"
    );
    const reads = [];
    const writes = [];
    const rawValues = [];
    for (const exportKey of Object.keys(mod)) {
      try { rawValues.push(mod[exportKey]); } catch {}
    }
    for (const fn of new Set(rawValues)) {
      if (typeof fn !== "function") continue;
      let source;
      // Codex's instrumentation wraps Function#toString; some unrelated
      // exports cannot be inspected through it. Skip those, not the module.
      try { source = Function.prototype.toString.call(fn); } catch { continue; }
      const params = source.match(paramsRe);
      if (!params) continue;
      const key = escapeRegExp(params[1]) + "\\s*\\.\\s*key";
      if (params[2] == null) {
        const readRe = new RegExp(
          "\\(\\s*await\\s+(" + id + ")\\s*\\(\\s*[`'\"]get-setting[`'\"]\\s*,\\s*" +
          "\\{\\s*params\\s*:\\s*\\{\\s*key\\s*:\\s*" + key + "\\s*\\}\\s*\\}\\s*\\)\\s*\\)" +
          "\\s*\\.\\s*value\\s*\\?\\?\\s*" + escapeRegExp(params[1]) + "\\s*\\.\\s*default"
        );
        const match = source.match(readRe);
        if (match) reads.push({ fn, rpc: match[1] });
        else if (!source.includes("get-setting") && !source.includes("set-setting")) {
          const nativeReadRe = new RegExp(
            "\\breturn\\s*\\(\\s*await\\s+" + callWrap + "(" + receiverChain +
            ")\\s*\\.\\s*settings\\s*\\.\\s*read\\s*\\(\\s*" + key +
            "\\s*\\)\\s*\\)\\s*\\)?\\s*\\.\\s*effective"
          );
          const nativeMatch = source.match(nativeReadRe);
          if (nativeMatch) {
            reads.push({ fn, rpc: "native:" + nativeMatch[1].replace(/\s+/g, "") });
          }
        }
      } else {
        const writeRe = new RegExp(
          "\\bawait\\s+(" + id + ")\\s*\\(\\s*[`'\"]set-setting[`'\"]\\s*,\\s*" +
          "\\{\\s*params\\s*:\\s*\\{\\s*key\\s*:\\s*" + key + "\\s*,\\s*value\\s*:\\s*" +
          escapeRegExp(params[2]) + "\\s*\\}\\s*\\}\\s*\\)"
        );
        const match = source.match(writeRe);
        if (match) writes.push({ fn, rpc: match[1] });
        else if (!source.includes("get-setting") && !source.includes("set-setting")) {
          const nativeWriteRe = new RegExp(
            "\\bawait\\s+" + callWrap + "(" + receiverChain +
            ")\\s*\\.\\s*settings\\s*\\.\\s*write\\s*\\(\\s*" + key +
            "\\s*,\\s*" + escapeRegExp(params[2]) + "\\s*\\)"
          );
          const nativeMatch = source.match(nativeWriteRe);
          if (nativeMatch) {
            writes.push({ fn, rpc: "native:" + nativeMatch[1].replace(/\s+/g, "") });
          }
        }
      }
    }
    // Do not guess when a module exposes several different settings APIs.
    if (reads.length !== 1 || writes.length !== 1 || reads[0].rpc !== writes[0].rpc) return null;
    return { read: reads[0].fn, write: writes[0].fn };
  };
  const FETCH_CONCURRENCY = 16;
  const fetched = new Map();
  const checked = new Set();
  const fetchText = async (url) => {
    if (fetched.has(url)) return fetched.get(url);
    let text = null;
    try {
      const response = await fetch(url);
      // Electron custom-protocol responses can expose a readable body while
      // standard status flags are false. Structural validation below is authoritative.
      text = await response.text();
    } catch {}
    fetched.set(url, text);
    if (text != null) checked.add(url);
    return text;
  };
  const fetchBatched = async (urls, limit = FETCH_CONCURRENCY) => {
    const list = [...urls];
    const out = new Array(list.length);
    let next = 0;
    const worker = async () => {
      while (next < list.length) {
        const idx = next++;
        out[idx] = [list[idx], await fetchText(list[idx])];
      }
    };
    const workers = Math.min(limit, list.length);
    if (workers > 0) await Promise.all(Array.from({ length: workers }, worker));
    return out;
  };
  const findFirstInBatches = async (urls, predicate) => {
    for (let i = 0; i < urls.length; i += FETCH_CONCURRENCY) {
      const batch = await fetchBatched(urls.slice(i, i + FETCH_CONCURRENCY));
      for (const [url, text] of batch) {
        if (text == null) continue;
        const hit = await predicate(url, text);
        if (hit != null) return hit;
      }
    }
    return null;
  };
  const hasNativeSettingsMarkers = (text) =>
    /\.\s*settings\s*\.\s*read\s*\(/.test(text) &&
    /\.\s*settings\s*\.\s*write\s*\(/.test(text);
  const isSettingsCandidateText = (text) => {
    const nativeSettings = hasNativeSettingsMarkers(text);
    return (
      (nativeSettings || (text.includes("get-setting") && text.includes("set-setting"))) &&
      text.includes("async function")
    );
  };
  const rspackEntryRe =
    /\bexport\s*\{[^}]*\bas\s+__webpack_require__\b|\bexport\s+(?:const|let|var|function)\s+__webpack_require__\b/;
  const loadedSettingChunks = loadedUrls.filter((url) =>
    /(?:^|\/)setting-storage-[A-Za-z0-9_-]+\.js(?:$|[?#])/.test(url)
  );
  const lazy715Candidates = async () => {
    const candidates = new Set(loadedSettingChunks);
    const chunkRef = /(?:\.\/)?setting-storage-[A-Za-z0-9_-]+\.js/g;
    for (const [sourceUrl, text] of await fetchBatched(loadedUrls)) {
      if (text == null) continue;
      for (const match of text.matchAll(chunkRef)) {
        try { candidates.add(new URL(match[0], sourceUrl).href); } catch {}
      }
    }
    return [...candidates];
  };
  const resolveRspack = async (adapter) => {
    const runtimes = [];
    const tryExports = (exports, url) => {
      const api = exportedSettingsWrappers(exports);
      if (!api) return null;
      w.__camThemeSettingsV1 = { ...api, url, adapter };
      return { ok: true, cached: false, adapter, url, checked: checked.size };
    };
    const tryModuleEntries = async (req, entries, url) => {
      for (const [modId, factory] of entries) {
        if (typeof factory !== "function") continue;
        let src;
        try { src = Function.prototype.toString.call(factory); } catch { continue; }
        if (!isSettingsCandidateText(src)) continue;
        let exp;
        try { exp = await Promise.resolve(req(modId)); } catch { continue; }
        const hit = tryExports(exp, url);
        if (hit) return hit;
      }
      return null;
    };
    const collectRuntime = async (url, text) => {
      if (!rspackEntryRe.test(text)) return null;
      let mod;
      try { mod = await import(url); } catch { return null; }
      const req = mod?.__webpack_require__;
      if (typeof req !== "function") return null;
      runtimes.push({ url, req });
      if (req.c && typeof req.c === "object") {
        for (const entry of Object.values(req.c)) {
          if (!entry || entry.error) continue;
          const hit = tryExports(entry.exports, url);
          if (hit) return hit;
        }
      }
      if (req.m && typeof req.m === "object") {
        const hit = await tryModuleEntries(req, Object.entries(req.m), url);
        if (hit) return hit;
      }
      return null;
    };
    const fastHit = await findFirstInBatches(scriptUrls, collectRuntime);
    if (fastHit != null) return fastHit;
    if (runtimes.length === 0) {
      const scriptSet = new Set(scriptUrls);
      const otherUrls = loadedUrls.filter((u) => !scriptSet.has(u));
      const slowHit = await findFirstInBatches(otherUrls, collectRuntime);
      if (slowHit != null) return slowHit;
    }
    if (runtimes.length === 0) return null;
    return findFirstInBatches(loadedUrls, async (url, text) => {
      if (!text.includes("__webpack_modules__") || !isSettingsCandidateText(text)) return null;
      let chunkMod;
      try { chunkMod = await import(url); } catch { return null; }
      const modules = chunkMod?.__webpack_modules__;
      if (!modules || typeof modules !== "object") return null;
      for (const { req } of runtimes) {
        try {
          if (typeof req.C === "function") req.C(chunkMod);
          else if (req.m && typeof req.m === "object") Object.assign(req.m, modules);
        } catch { continue; }
        const hit = await tryModuleEntries(req, Object.entries(modules), url);
        if (hit) return hit;
      }
      return null;
    });
  };
  const resolveModule = async (adapter, candidates) => {
    const modern = adapter === "26.831.21537+";
    const writeRe = modern ? modernWriteRe : legacyWriteRe;
    const readRe = modern ? modernReadRe : legacyReadRe;
    const exportIdentifier = modern ? "[$A-Za-z_][$A-Za-z0-9_]*" : "\\w+";
    return findFirstInBatches(candidates, async (url, text) => {
      const nativeSettings = hasNativeSettingsMarkers(text);
      if (!text.includes("set-setting") && !nativeSettings) return null;
      if (adapter === "26.901.20858+") {
        // Only import an app settings candidate, never every loaded chunk.
        if (!isSettingsCandidateText(text)) return null;
        let mod;
        try { mod = await import(url); } catch { return null; }
        const api = exportedSettingsWrappers(mod);
        if (!api) return null;
        w.__camThemeSettingsV1 = { ...api, url, adapter };
        return { ok: true, cached: false, adapter, url, checked: checked.size };
      }
      const writeMatch = text.match(writeRe);
      const readMatch = text.match(readRe);
      if (!writeMatch || !readMatch) return null;
      const aliasOf = (name) => {
        const match = text.match(new RegExp(
          "(?:^|[^$A-Za-z0-9_])" + escapeRegExp(name) +
            " as (" + exportIdentifier + ")"
        ));
        return match ? match[1] : null;
      };
      const writeAlias = aliasOf(writeMatch[1]);
      const readAlias = aliasOf(readMatch[1]);
      if (!writeAlias || !readAlias) return null;
      let mod;
      try { mod = await import(url); } catch { return null; }
      const read = mod[readAlias];
      const write = mod[writeAlias];
      if (typeof read !== "function" || typeof write !== "function") return null;
      w.__camThemeSettingsV1 = { read, write, url, adapter };
      return { ok: true, cached: false, adapter, url, checked: checked.size };
    });
  };
  for (const adapter of adapters) {
    if (adapter === "26.1111.21308+") {
      const outcome = await resolveRspack(adapter);
      if (outcome != null) return outcome;
      continue;
    }
    const candidates = adapter === "26.715" ? await lazy715Candidates() : loadedUrls;
    const outcome = await resolveModule(adapter, candidates);
    if (outcome != null) return outcome;
    if (adapter === "26.901.20858+") {
      const lazyOutcome = await resolveModule(adapter, await lazy715Candidates());
      if (lazyOutcome != null) return lazyOutcome;
    }
  }
  return {
    ok: false,
    error: "settings module not found (adapters " + adapters.join(", ") +
      "; " + checked.size + " chunks scanned)",
  };
})()"#;

fn ensure_api_expression(version_hint: Option<&str>) -> String {
    let adapters = adapter_order(version_hint).map(SettingsAdapter::id);
    ENSURE_API_JS_TEMPLATE.replace(
        ADAPTERS_TOKEN,
        &serde_json::to_string(&adapters).expect("static adapter IDs"),
    )
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
    let outcome: JsOutcome =
        serde_json::from_value(value).map_err(|e| err(format!("{what}: 结果解析失败: {e}")))?;
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
    let payload_json =
        serde_json::to_string(&payload).map_err(|e| err(format!("写入负载序列化失败: {e}")))?;
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
    run_js(session, &expression, "写入外观设置")
        .await
        .map(|_| ())
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
        // The legacy wrapper shape was measured in Codex 26.707.91948. The
        // modern identifier shape was first observed in 26.831.21537, whose
        // write wrapper is `$D` and export alias is `ezt`. The regexes live in
        // page-side JS; pin the load-bearing fragments so an accidental edit
        // of ENSURE_API_JS fails loudly.
        for fragment in [
            r"async function (\w+)\(e,t\)\{await (\w+)\(",
            r"async function ([$A-Za-z_][$A-Za-z0-9_]*)\(e,t\)",
            "set-setting",
            "get-setting",
            "__camThemeSettingsV1",
            "modulepreload",
            "escapeRegExp(name)",
            "await import(url)",
            "Function.prototype.toString.call(fn)",
            "reads[0].rpc !== writes[0].rpc",
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
            [
                SettingsAdapter::V26_707,
                SettingsAdapter::V26_715,
                SettingsAdapter::V26_831_21537,
                SettingsAdapter::V26_901_20858,
                SettingsAdapter::V26_1111_21308,
            ]
        );
        assert_eq!(
            adapter_order(Some("26.715.2305.0")),
            [
                SettingsAdapter::V26_715,
                SettingsAdapter::V26_707,
                SettingsAdapter::V26_831_21537,
                SettingsAdapter::V26_901_20858,
                SettingsAdapter::V26_1111_21308,
            ]
        );
    }

    #[test]
    fn full_identifier_adapter_starts_at_first_observed_codex_build() {
        for previous in ["26.831.11858", "26.831.20005"] {
            assert_eq!(
                preferred_adapter(Some(previous)),
                Some(SettingsAdapter::V26_715),
                "{previous} must stay on the pre-boundary adapter"
            );
        }
        for current_or_newer in ["26.831.21537", "26.831.21537.0", "26.832.1"] {
            assert_eq!(
                preferred_adapter(Some(current_or_newer)),
                Some(SettingsAdapter::V26_831_21537),
                "{current_or_newer} must accept full JS identifiers"
            );
        }
    }

    #[test]
    fn unknown_or_new_version_probes_latest_adapter_first() {
        assert_eq!(
            adapter_order(Some("unexpected")),
            [
                SettingsAdapter::V26_1111_21308,
                SettingsAdapter::V26_901_20858,
                SettingsAdapter::V26_831_21537,
                SettingsAdapter::V26_715,
                SettingsAdapter::V26_707,
            ]
        );
        assert_eq!(
            adapter_order(Some("26.706.1")),
            [
                SettingsAdapter::V26_1111_21308,
                SettingsAdapter::V26_901_20858,
                SettingsAdapter::V26_831_21537,
                SettingsAdapter::V26_715,
                SettingsAdapter::V26_707,
            ]
        );
        assert_eq!(
            adapter_order(None),
            [
                SettingsAdapter::V26_1111_21308,
                SettingsAdapter::V26_901_20858,
                SettingsAdapter::V26_831_21537,
                SettingsAdapter::V26_715,
                SettingsAdapter::V26_707,
            ]
        );
    }

    #[test]
    fn native_bridge_adapter_starts_at_first_observed_codex_build() {
        for previous in ["26.831.21537", "26.901.20857"] {
            assert_eq!(
                preferred_adapter(Some(previous)),
                Some(SettingsAdapter::V26_831_21537),
                "{previous} must stay on the pre-bridge adapter"
            );
        }
        for current_in_range in ["26.901.20858", "26.901.20858.0", "26.901.22334", "26.1111.21307"]
        {
            assert_eq!(
                preferred_adapter(Some(current_in_range)),
                Some(SettingsAdapter::V26_901_20858),
                "{current_in_range} must accept native bridge branches"
            );
        }
    }

    #[test]
    fn rspack_adapter_starts_at_first_observed_codex_build() {
        for current_or_newer in ["26.1111.21308", "26.1111.21308.0", "27.1.1"] {
            assert_eq!(
                preferred_adapter(Some(current_or_newer)),
                Some(SettingsAdapter::V26_1111_21308),
                "{current_or_newer} must prefer the Rspack runtime adapter"
            );
        }
    }

    #[test]
    fn discovery_script_embeds_versioned_probe_order_and_lazy_715_chunk() {
        let legacy = ensure_api_expression(Some("26.707.9981.0"));
        let current = ensure_api_expression(Some("26.715.2305.0"));
        let modern = ensure_api_expression(Some("26.831.21537"));
        let native = ensure_api_expression(Some("26.901.20858"));
        let rspack = ensure_api_expression(Some("26.1111.21308"));
        assert!(legacy.contains(
            r#"const adapters = ["26.707","26.715","26.831.21537+","26.901.20858+","26.1111.21308+"]"#
        ));
        assert!(current.contains(
            r#"const adapters = ["26.715","26.707","26.831.21537+","26.901.20858+","26.1111.21308+"]"#
        ));
        assert!(modern.contains(
            r#"const adapters = ["26.831.21537+","26.715","26.707","26.901.20858+","26.1111.21308+"]"#
        ));
        assert!(native.contains(
            r#"const adapters = ["26.901.20858+","26.1111.21308+","26.831.21537+","26.715","26.707"]"#
        ));
        assert!(rspack.contains(
            r#"const adapters = ["26.1111.21308+","26.901.20858+","26.831.21537+","26.715","26.707"]"#
        ));
        assert!(current.contains("setting-storage-"));
        assert!(rspack.contains("__webpack_require__"));
        assert!(rspack.contains("FETCH_CONCURRENCY"));
        assert!(!current.contains("response.ok"));
        for script in [legacy, current, modern, native, rspack] {
            assert!(!script.contains(ADAPTERS_TOKEN));
        }
    }
}
