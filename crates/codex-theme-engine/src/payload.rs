//! Renderer payload assembly (port of `payload.mjs`): the runtime template
//! with the theme CSS, config, chrome fragment and inlined assets substituted
//! in, plus the remove/verify expressions used by the daemon and callers.

use std::path::Path;

use sha1::{Digest, Sha1};

use crate::theme::{
    inline_assets, inline_motion_assets, load_theme, LoadedTheme, ThemeConfig,
};
use crate::{Result, ENGINE_VERSION};

/// The injected renderer runtime — authored here; `awesome-codex-skins`'s
/// studio CLI vendors a pinned copy of this file and `composer-overflow.mjs`
/// instead of maintaining its own (see that repo's `studio/RUNTIME_SOURCE.json`).
/// It encodes the flicker discipline (compare-before-write), sticky route
/// detection, icon annotation and cleanup contract, plus Manager's host
/// compatibility shims for existing skin packages.
const RUNTIME_TEMPLATE: &str = include_str!("runtime/theme-runtime.js");
const COMPOSER_OVERFLOW_MODULE: &str = include_str!("runtime/composer-overflow.mjs");

/// Runtime-owned Composer scroll contract. Theme art may extend beyond the
/// shell without turning it into a scroll container; only the finite-height
/// editor root may scroll vertically. Appended after theme CSS so old packages
/// cannot reintroduce the shell-scroll bug.
const RUNTIME_HARDENING_CSS: &str = r#"
html.codex-theme-studio [data-cts-composer-overflow="shell"] {
  overflow: clip !important;
  overflow-clip-margin: 64px !important;
}

html.codex-theme-studio [data-cts-composer-overflow="lane"] {
  overflow: visible !important;
}

html.codex-theme-studio [data-cts-composer-overflow="editor"] {
  overflow-x: hidden !important;
  overflow-y: auto !important;
  overscroll-behavior: contain !important;
}
"#;

#[derive(Debug, Clone)]
pub struct BuiltPayload {
    pub payload: String,
    pub theme: ThemeConfig,
    /// Full stamp injected into the renderer: `<version>:<id>:<sha1[..12]>`.
    pub stamp: String,
    pub payload_bytes: usize,
    pub asset_count: usize,
}

/// Build the `Runtime.evaluate` payload for a theme directory. Still images
/// become CSS data URLs; motion assets become a dedicated data-URL map consumed
/// by the runtime's `<video>` element.
pub fn build_payload(theme_dir: &Path) -> Result<BuiltPayload> {
    build_payload_from(load_theme(theme_dir)?)
}

pub fn build_payload_from(theme: LoadedTheme) -> Result<BuiltPayload> {
    let data_urls = inline_assets(&theme)?;
    let motion_data_urls = inline_motion_assets(&theme)?;
    // Still-image assets ride the stylesheet as --cts-asset-* data: URLs, immune
    // to the blob revocation races that break late-loading images (border-image).
    // Motion assets skip CSS entirely and ride their own JSON slot as data URLs;
    // Codex already permits `media-src data:`, so playback needs no CSP bypass.
    let asset_variables = data_urls
        .iter()
        .map(|(key, url)| format!("  --cts-asset-{key}: url(\"{url}\");"))
        .collect::<Vec<_>>()
        .join("\n");
    // A JSON object literal injected directly as the runtime's `motionAssets`
    // argument — NOT wrapped as a string like the chrome fragment.
    let motion_json = serde_json::to_string(&motion_data_urls)
        .map_err(|e| crate::ThemeEngineError::Theme(format!("motion serialize: {e}")))?;
    let css_with_assets = format!(
        ":root.codex-theme-studio {{\n{asset_variables}\n}}\n\n{}\n\n{RUNTIME_HARDENING_CSS}",
        theme.css
    );
    let config_json = serde_json::to_string(&theme.config)
        .map_err(|e| crate::ThemeEngineError::Theme(format!("config serialize: {e}")))?;
    let chrome_html = theme.chrome_html.clone();

    // Fingerprint the executable packed payload, including the renderer runtime
    // and motion bytes. A video-only change must re-inject and replay the intro.
    let runtime_template = RUNTIME_TEMPLATE.replace(
        "__CTS_COMPOSER_OVERFLOW_HELPERS__",
        &composer_overflow_helpers_expression(),
    );
    let short = fingerprint(
        &runtime_template,
        &css_with_assets,
        chrome_html.as_deref().unwrap_or(""),
        &config_json,
        &motion_json,
    );
    let stamp = format!("{ENGINE_VERSION}:{}:{short}", theme.config.id);

    let payload = runtime_template
        .replace("__CTS_CSS_JSON__", &js_json(&css_with_assets)?)
        .replace("__CTS_THEME_JSON__", &config_json)
        .replace(
            "__CTS_CHROME_JSON__",
            &serde_json::to_string(&chrome_html)
                .map_err(|e| crate::ThemeEngineError::Theme(format!("chrome serialize: {e}")))?,
        )
        .replace("__CTS_MOTION_JSON__", &motion_json)
        .replace("__CTS_VERSION_JSON__", &js_json(ENGINE_VERSION)?)
        .replace("__CTS_STAMP_JSON__", &js_json(&stamp)?);

    Ok(BuiltPayload {
        payload_bytes: payload.len(),
        asset_count: data_urls.len() + motion_data_urls.len(),
        theme: theme.config,
        stamp,
        payload,
    })
}

fn composer_overflow_helpers_expression() -> String {
    let module = COMPOSER_OVERFLOW_MODULE.replace("export function ", "function ");
    format!(
        "(() => {{\n{module}\nreturn {{ clearComposerSurfaceCompat, createComposerOverflowAnnotator, reconcileComposerSurfaces, selectComposerSurfaces }};\n}})()"
    )
}

fn js_json(value: &str) -> Result<String> {
    serde_json::to_string(value)
        .map_err(|e| crate::ThemeEngineError::Theme(format!("payload serialize: {e}")))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn fingerprint(runtime: &str, css: &str, chrome: &str, config: &str, motion: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(runtime.as_bytes());
    hasher.update(css.as_bytes());
    hasher.update(chrome.as_bytes());
    hasher.update(config.as_bytes());
    hasher.update(motion.as_bytes());
    let digest = hasher.finalize();
    hex(&digest)[..12].to_string()
}

/// Tear the theme down in a renderer (idempotent; safe on stock pages).
pub const REMOVE_EXPRESSION: &str = r#"(() => {
  window.__CODEX_THEME_STUDIO_DISABLED__ = true;
  const state = window.__CODEX_THEME_STUDIO__;
  if (state?.cleanup) return state.cleanup();
  document.documentElement?.classList.remove('codex-theme-studio');
  document.documentElement?.removeAttribute('data-cts-theme');
  document.documentElement?.removeAttribute('data-cts-shell');
  document.querySelectorAll('[data-cts-main-surface-compat]').forEach((node) => {
    node.classList.remove('main-surface');
    node.removeAttribute('data-cts-main-surface-compat');
  });
  document.querySelectorAll('.cts-windows-menu-bar').forEach((node) => node.classList.remove('cts-windows-menu-bar'));
  document.querySelectorAll('[data-cts-menu-region]').forEach((node) => node.removeAttribute('data-cts-menu-region'));
  document.querySelectorAll('[data-cts-composer-overflow]').forEach((node) => node.removeAttribute('data-cts-composer-overflow'));
  document.querySelectorAll('[data-cts-composer-mode]').forEach((node) => node.removeAttribute('data-cts-composer-mode'));
  document.querySelectorAll('[data-cts-composer-action]').forEach((node) => node.removeAttribute('data-cts-composer-action'));
  document.querySelectorAll('[data-cts-composer-surface-compat]').forEach((node) => {
    node.classList.remove('composer-surface-chrome');
    node.removeAttribute('data-cts-composer-surface-compat');
  });
  document.documentElement?.style.removeProperty('--cts-windows-menu-height');
  document.documentElement?.style.removeProperty('--cts-windows-sidebar-padding-top');
  document.documentElement?.style.removeProperty('--cts-windows-main-padding-top');
  document.documentElement?.style.removeProperty('--cts-windows-sidebar-foreground');
  document.documentElement?.style.removeProperty('--cts-windows-main-foreground');
  document.getElementById('cts-style')?.remove();
  document.getElementById('cts-chrome')?.remove();
  document.getElementById('cts-stage')?.remove();
  document.getElementById('cts-intro')?.remove();
  delete window.__CODEX_THEME_STUDIO__;
  return true;
})()"#;

pub const VERIFY_REMOVED_EXPRESSION: &str = r#"(() =>
  !document.documentElement.classList.contains('codex-theme-studio') &&
  !document.querySelector('.cts-windows-menu-bar') &&
  !document.querySelector('[data-cts-menu-region]') &&
  !document.querySelector('[data-cts-composer-overflow]') &&
  !document.querySelector('[data-cts-composer-mode]') &&
  !document.querySelector('[data-cts-composer-action]') &&
  !document.querySelector('[data-cts-composer-surface-compat]') &&
  !document.documentElement.style.getPropertyValue('--cts-windows-menu-height') &&
  !document.documentElement.style.getPropertyValue('--cts-windows-sidebar-padding-top') &&
  !document.documentElement.style.getPropertyValue('--cts-windows-main-padding-top') &&
  !document.documentElement.style.getPropertyValue('--cts-windows-sidebar-foreground') &&
  !document.documentElement.style.getPropertyValue('--cts-windows-main-foreground') &&
  !document.getElementById('cts-style') &&
  !document.getElementById('cts-chrome') &&
  !document.getElementById('cts-stage') &&
  !document.getElementById('cts-intro') &&
  !document.querySelector('[data-cts-main-surface-compat]') &&
  !window.__CODEX_THEME_STUDIO__
)()"#;

/// The daemon's per-tick reconciliation probe: what stamp (if any) does the
/// renderer currently carry? `null` on stock pages.
pub const CURRENT_STAMP_EXPRESSION: &str =
    "window.__CODEX_THEME_STUDIO__ ? (window.__CODEX_THEME_STUDIO__.stamp ?? null) : null";

/// Structural verification of an applied theme (port of `verifyExpression`).
pub fn verify_expression(expected_version: &str) -> Result<String> {
    let version_json = js_json(expected_version)?;
    let composer_helpers = composer_overflow_helpers_expression();
    Ok(format!(
        r#"(() => {{
    const box = (node) => {{
      if (!node) return null;
      const r = node.getBoundingClientRect();
      const style = getComputedStyle(node);
      return {{
        x: Math.round(r.x), y: Math.round(r.y),
        width: Math.round(r.width), height: Math.round(r.height),
        visible: r.width > 0 && r.height > 0 && style.display !== 'none' && style.visibility !== 'hidden',
      }};
    }};
    const hiddenByAncestor = (node) => {{
      for (let current = node; current; current = current.parentElement) {{
        if (current.hidden || current.hasAttribute?.('inert') ||
            current.getAttribute?.('aria-hidden') === 'true' ||
            current.getAttribute?.('data-app-shell-active-page') === 'false') return true;
        const style = getComputedStyle(current);
        const contentVisibility = style.contentVisibility || style.getPropertyValue?.('content-visibility');
        if (style.display === 'none' || style.visibility === 'hidden' ||
            style.visibility === 'collapse' || contentVisibility === 'hidden' ||
            Number.parseFloat(style.opacity) === 0) return true;
      }}
      return false;
    }};
    const visibleSurfaceScore = (node) => {{
      if (!node?.isConnected || hiddenByAncestor(node)) return -1;
      const r = node.getBoundingClientRect();
      if (!(r.width > 0 && r.height > 0)) return -1;
      const viewportWidth = Math.max(document.documentElement?.clientWidth || 0, innerWidth || 0);
      const viewportHeight = Math.max(document.documentElement?.clientHeight || 0, innerHeight || 0);
      const width = Math.max(0, Math.min(r.right, viewportWidth) - Math.max(r.left, 0));
      const height = Math.max(0, Math.min(r.bottom, viewportHeight) - Math.max(r.top, 0));
      return width > 0 && height > 0 ? width * height : -1;
    }};
    const bestVisibleSurface = (nodes) => nodes.reduce((best, node) => {{
      const score = visibleSurfaceScore(node);
      return score >= 0 && score >= best.score ? {{ node, score }} : best;
    }}, {{ node: null, score: -1 }}).node;
    const chrome = document.getElementById('cts-chrome');
    const stage = document.getElementById('cts-stage');
    const currentMainSurfaces = [...document.querySelectorAll('main[data-app-shell-main-surface]')];
    const legacyMainSurfaces = [...document.querySelectorAll('main.main-surface')]
      .filter((node) => !node.hasAttribute('data-app-shell-main-surface'));
    const mainSurfaceNode = bestVisibleSurface(currentMainSurfaces) ||
      bestVisibleSurface(legacyMainSurfaces);
    const mainSurface = box(mainSurfaceNode);
    const state = window.__CODEX_THEME_STUDIO__;
    const hostVersion = (() => {{
      try {{
        const value = window.electronBridge?.getSentryInitOptions?.()?.appVersion;
        return typeof value === 'string' && /^\d+\./.test(value) ? value : null;
      }} catch {{
        return null;
      }}
    }})();
    const hostCompatibility = hostVersion === '26.715.31251'
      ? {{ audited: true, profile: 'composer-three-layer', composerLanePolicy: 'required' }}
      : hostVersion === '26.715.31925'
        ? {{ audited: true, profile: 'composer-two-or-three-layer', composerLanePolicy: 'optional' }}
        : hostVersion === '26.727.51351'
          ? {{ audited: true, profile: 'composer-current-multiline', composerLanePolicy: 'required' }}
          : {{ audited: false, profile: 'capability-adaptive', composerLanePolicy: 'optional' }};
    const {{ selectComposerSurfaces }} = {composer_helpers};
    const composerNodes = mainSurfaceNode ? selectComposerSurfaces(mainSurfaceNode) : [];
    const composerNode = composerNodes.find((node) => visibleSurfaceScore(node) >= 0) ?? null;
    const composer = box(composerNode);
    const composerEditor = composerNode?.querySelector('[data-cts-composer-overflow="editor"]') ?? null;
    const composerLanes = composerNode
      ? [...composerNode.querySelectorAll('[data-cts-composer-overflow="lane"]')]
      : [];
    const composerMode = composerNode?.getAttribute('data-cts-composer-mode') ?? null;
    const composerOverflow = composerNode ? {{
      shellRole: composerNode.getAttribute('data-cts-composer-overflow'),
      mode: composerMode,
      shellOverflowY: getComputedStyle(composerNode).overflowY,
      laneCount: composerLanes.length,
      laneOverflowYs: composerLanes.map((node) => getComputedStyle(node).overflowY),
      lanesValid: composerLanes.every((node) => getComputedStyle(node).overflowY === 'visible'),
      lanePolicyValid: hostCompatibility.composerLanePolicy !== 'required' ||
        composerMode === 'single-line' || composerLanes.length >= 1,
      editorCount: composerNode.querySelectorAll('[data-cts-composer-overflow="editor"]').length,
      editorOverflowY: composerEditor ? getComputedStyle(composerEditor).overflowY : null,
    }} : null;
    if (composerOverflow) {{
      composerOverflow.modeValid = composerOverflow.mode === 'single-line' ||
        composerOverflow.mode === 'scrolling';
      composerOverflow.editorValid = composerOverflow.mode === 'single-line'
        ? composerOverflow.editorCount === 0
        : composerOverflow.mode === 'scrolling' &&
          composerOverflow.editorCount === 1 &&
          composerOverflow.editorOverflowY === 'auto';
    }}
    const sidebar = box(document.querySelector('aside.app-shell-left-panel'));
    const result = {{
      installed: document.documentElement.classList.contains('codex-theme-studio'),
      themeId: document.documentElement.getAttribute('data-cts-theme'),
      version: state?.version ?? null,
      hostVersion,
      hostCompatibility,
      stylePresent: Boolean(document.getElementById('cts-style')),
      chromePresent: Boolean(chrome),
      chromePointerEvents: chrome ? getComputedStyle(chrome).pointerEvents : null,
      mainSurface,
      mainSurfaceMode: mainSurfaceNode?.hasAttribute('data-app-shell-main-surface') ? 'current' : (mainSurfaceNode ? 'legacy' : null),
      mainSurfaceCompatible: Boolean(mainSurfaceNode?.classList.contains('main-surface')),
      stageAttachedToMainSurface: !stage || stage.parentElement === mainSurfaceNode,
      composer,
      composerSurfaceMode: composerNode?.hasAttribute('data-composer-surface-variant') ? 'current' : (composerNode ? 'legacy' : null),
      composerSurfaceCompatible: Boolean(composerNode?.classList.contains('composer-surface-chrome')),
      composerOverflow,
      sidebar,
      viewport: {{ width: innerWidth, height: innerHeight }},
      documentOverflow: {{
        x: document.documentElement.scrollWidth > document.documentElement.clientWidth,
        y: document.documentElement.scrollHeight > document.documentElement.clientHeight,
      }},
    }};
    result.pass = Boolean(
      result.installed &&
      result.version === {version_json} &&
      result.stylePresent &&
      (!result.chromePresent || result.chromePointerEvents === 'none') &&
      Boolean(result.mainSurface?.visible) &&
      result.mainSurfaceCompatible &&
      result.stageAttachedToMainSurface &&
      Boolean(result.composer?.visible) &&
      result.composerSurfaceCompatible &&
      result.composerOverflow?.shellRole === 'shell' &&
      result.composerOverflow?.shellOverflowY === 'clip' &&
      result.composerOverflow?.lanesValid === true &&
      result.composerOverflow?.lanePolicyValid === true &&
      result.composerOverflow?.modeValid === true &&
      result.composerOverflow?.editorValid === true &&
      Boolean(result.sidebar?.visible) &&
      !result.documentOverflow.x
    );
    return result;
  }})()"#
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Searches the embedded `theme-runtime.js` source (as it appears inside
    /// a built payload string) for the legacy `main.main-surface` fallback
    /// that `resolveShellMain()` tries after the current
    /// `main[data-app-shell-main-surface]` selector, in any of its known
    /// source forms (template-literal, string-concatenation, or inlined
    /// literal).
    ///
    /// The search window is deliberately bounded to `[current,
    /// integrateWindowsMenu)`: `resolveShellMain()`'s legacy fallback is the
    /// only thing this check is meant to verify, but the embedded source
    /// also contains an unrelated, always-present `main.main-surface`
    /// substring later on, inside `integrateWindowsMenu()`'s
    /// `:scope > main.main-surface` query. An earlier, unbounded
    /// `payload[current..].find(...)` search would find that unrelated
    /// occurrence too and report the fallback as present even if
    /// `resolveShellMain()`'s actual legacy branch had been deleted
    /// entirely -- exactly the false-positive this function exists to rule
    /// out. Bounding the window to end at `integrateWindowsMenu` (which is
    /// always structurally after `resolveShellMain()` in the source) keeps
    /// the search scoped to the shim region it is supposed to verify.
    fn find_legacy_main_surface_fallback(payload: &str, current: usize) -> Option<usize> {
        let region_end = payload[current..]
            .find("integrateWindowsMenu")
            .map(|offset| current + offset)
            .unwrap_or(payload.len());
        let region = &payload[current..region_end];
        region
            .find("main.${LEGACY_SHELL_MAIN_CLASS}")
            .or_else(|| region.find("\"main.\" + LEGACY_SHELL_MAIN_CLASS"))
            .or_else(|| region.find("main.main-surface"))
    }

    /// Regression test for the exact false positive described above:
    /// deleting `resolveShellMain()`'s legacy fallback entirely (a real
    /// Codex <= 26.715 backward-compatibility regression) must make the
    /// search fail, even though an unrelated `main.main-surface` substring
    /// still exists later in the source (standing in for
    /// `integrateWindowsMenu()`'s unrelated query).
    #[test]
    fn legacy_main_surface_fallback_search_ignores_unrelated_later_occurrence() {
        let payload_without_legacy_fallback = concat!(
            "const resolveShellMain = () => {\n",
            "  const shellMain = document.querySelector(\"main[data-app-shell-main-surface]\");\n",
            "  return shellMain;\n",
            "};\n",
            "const integrateWindowsMenu = (shellMain) => {\n",
            "  const main = shellRow?.querySelector(\":scope > main.main-surface\");\n",
            "};\n",
        );
        let current = payload_without_legacy_fallback
            .find("main[data-app-shell-main-surface]")
            .unwrap();
        assert_eq!(
            find_legacy_main_surface_fallback(payload_without_legacy_fallback, current),
            None,
            "must not match the unrelated `main.main-surface` occurrence inside \
             integrateWindowsMenu when resolveShellMain's own legacy fallback is missing"
        );

        // Sanity check: the same helper does find the fallback when it is
        // actually present between the current selector and
        // `integrateWindowsMenu`.
        let payload_with_legacy_fallback = concat!(
            "const resolveShellMain = () => {\n",
            "  const shellMain = document.querySelector(\"main[data-app-shell-main-surface]\") ||\n",
            "    document.querySelector(`main.${LEGACY_SHELL_MAIN_CLASS}`);\n",
            "  return shellMain;\n",
            "};\n",
            "const integrateWindowsMenu = (shellMain) => {\n",
            "  const main = shellRow?.querySelector(\":scope > main.main-surface\");\n",
            "};\n",
        );
        let current = payload_with_legacy_fallback
            .find("main[data-app-shell-main-surface]")
            .unwrap();
        assert!(find_legacy_main_surface_fallback(payload_with_legacy_fallback, current).is_some());
    }

    fn fixture_theme(tmp: &Path) -> std::path::PathBuf {
        let dir = tmp.join("fixture");
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(
            dir.join("theme.json"),
            r##"{
              "schemaVersion": 2,
              "id": "fixture",
              "name": "Fixture",
              "colors": { "accent": "#abc" },
              "strings": { "hero-title": "T" },
              "chrome": "chrome.html",
              "assets": { "wall": "assets/wall.png" }
            }"##,
        )
        .unwrap();
        std::fs::write(dir.join("theme.css"), "html.codex-theme-studio body {}\n").unwrap();
        std::fs::write(dir.join("chrome.html"), "<div data-cts-layer=\"stage\"></div>").unwrap();
        // Tiny valid-enough PNG bytes (content is never decoded, only inlined).
        std::fs::write(dir.join("assets/wall.png"), [0x89, b'P', b'N', b'G', 0, 1]).unwrap();
        dir
    }

    #[test]
    fn payload_substitutes_every_placeholder() {
        let tmp = tempfile::tempdir().unwrap();
        let built = build_payload(&fixture_theme(tmp.path())).unwrap();
        assert!(!built.payload.contains("__CTS_"), "unsubstituted placeholder");
        // The CSS rides as a JSON string literal, so quotes appear escaped.
        assert!(built.payload.contains("--cts-asset-wall: url(\\\"data:image/png;base64,"));
        assert!(built.payload.contains("data-cts-layer"));
        assert!(built.payload.contains("main[data-app-shell-main-surface]"));
        assert!(built.payload.contains("data-cts-main-surface-compat"));
        assert!(built.payload.contains("createComposerOverflowAnnotator"));
        assert!(built.payload.contains("annotateComposerOverflow.invalidate()"));
        assert!(built.payload.contains("data-cts-composer-overflow=\\\"shell\\\""));
        assert!(built.payload.contains("overflow: clip !important"));
        assert_eq!(built.asset_count, 1);
        assert!(built.stamp.starts_with(&format!("{ENGINE_VERSION}:fixture:")));
    }

    #[test]
    fn stamp_tracks_packed_artifacts() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = fixture_theme(tmp.path());
        let first = build_payload(&dir).unwrap().stamp;
        assert_eq!(first, build_payload(&dir).unwrap().stamp, "stamp must be stable");
        std::fs::write(dir.join("theme.css"), "html.codex-theme-studio body { color: red }\n")
            .unwrap();
        assert_ne!(first, build_payload(&dir).unwrap().stamp, "css change must re-stamp");
    }

    #[test]
    fn fingerprint_tracks_runtime_and_motion_changes() {
        let base = fingerprint("runtime-a", "css", "chrome", "config", "{}");
        assert_ne!(
            base,
            fingerprint("runtime-b", "css", "chrome", "config", "{}"),
            "runtime change must re-stamp"
        );
        assert_ne!(
            base,
            fingerprint(
                "runtime-a",
                "css",
                "chrome",
                "config",
                r#"{"intro-video":"data:video/mp4;base64,AAAA"}"#
            ),
            "motion change must re-stamp"
        );
    }

    fn motion_fixture(tmp: &Path) -> std::path::PathBuf {
        let dir = tmp.join("ning-hongye");
        std::fs::create_dir_all(dir.join("assets")).unwrap();
        std::fs::write(
            dir.join("theme.json"),
            r##"{
              "schemaVersion": 2,
              "id": "ning-hongye",
              "name": "Ning",
              "assets": { "intro": "assets/intro.webp" },
              "motionAssets": { "intro-video": "assets/intro-video.mp4" }
            }"##,
        )
        .unwrap();
        std::fs::write(dir.join("theme.css"), "html.codex-theme-studio {}\n").unwrap();
        std::fs::write(dir.join("assets/intro.webp"), [0x52, 0x49, 0x46, 0x46, 1, 2]).unwrap();
        // A "video" far larger than the 1.4 MB CSS-image cap. It is valid in
        // the dedicated motion slot because it never becomes a CSS URL.
        std::fs::write(dir.join("assets/intro-video.mp4"), vec![7u8; 2_000_000]).unwrap();
        dir
    }

    #[test]
    fn motion_uses_dedicated_data_url_without_touching_css_or_csp() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = motion_fixture(tmp.path());
        let built = build_payload(&dir).unwrap();
        assert!(built.payload.contains("data:video/mp4;base64,"));
        assert!(!built.payload.contains("http://127.0.0.1"));
        assert!(!built.payload.contains("Page.setBypassCSP"));
        assert!(built.payload.len() > 2_500_000, "video bytes must enter motion JSON");
        // The still image still rides the stylesheet as a data: URL.
        assert!(built.payload.contains("--cts-asset-intro: url("));
        assert!(!built.payload.contains("--cts-asset-intro-video"));
        assert_eq!(built.asset_count, 2);
    }

    #[test]
    fn payload_without_motion_substitutes_an_empty_map() {
        let tmp = tempfile::tempdir().unwrap();
        let built = build_payload(&fixture_theme(tmp.path())).unwrap();
        assert!(!built.payload.contains("__CTS_MOTION_JSON__"));
        assert!(built.payload.trim_end().ends_with(", {})"));
    }

    #[test]
    fn swapped_video_bytes_restamp() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = motion_fixture(tmp.path());
        let first = build_payload(&dir).unwrap().stamp;
        std::fs::write(dir.join("assets/intro-video.mp4"), vec![9u8; 3_000_000]).unwrap();
        let second = build_payload(&dir).unwrap().stamp;
        assert_ne!(first, second, "a swapped video must re-stamp");
    }

    #[test]
    fn removal_covers_every_runtime_owned_layer() {
        for id in ["cts-style", "cts-chrome", "cts-stage", "cts-intro"] {
            assert!(REMOVE_EXPRESSION.contains(id), "remove expression misses {id}");
            assert!(
                VERIFY_REMOVED_EXPRESSION.contains(id),
                "removal verification misses {id}"
            );
        }
        for marker in [
            "data-cts-main-surface-compat",
            "cts-windows-menu-bar",
            "data-cts-menu-region",
            "data-cts-composer-overflow",
            "data-cts-composer-mode",
            "data-cts-composer-action",
            "data-cts-composer-surface-compat",
            "--cts-windows-menu-height",
            "--cts-windows-sidebar-padding-top",
            "--cts-windows-main-padding-top",
            "--cts-windows-sidebar-foreground",
            "--cts-windows-main-foreground",
        ] {
            assert!(
                REMOVE_EXPRESSION.contains(marker),
                "remove expression misses {marker}"
            );
            assert!(
                VERIFY_REMOVED_EXPRESSION.contains(marker),
                "removal verification misses {marker}"
            );
        }
    }

    #[test]
    fn runtime_supports_current_and_legacy_main_surfaces() {
        let current = RUNTIME_TEMPLATE
            .find("main[data-app-shell-main-surface]")
            .expect("current main-surface selector");
        let legacy = RUNTIME_TEMPLATE
            .find("main.${LEGACY_SHELL_MAIN_CLASS}")
            .expect("legacy main-surface selector");
        assert!(current < legacy, "current semantic marker must win");
        assert!(!RUNTIME_TEMPLATE.contains(
            "document.querySelector(\"main.main-surface\") || document.querySelector(\"main\")"
        ));
    }

    #[test]
    fn payload_supports_current_and_legacy_composer_surfaces() {
        let tmp = tempfile::tempdir().unwrap();
        let built = build_payload(&fixture_theme(tmp.path())).unwrap();
        assert!(built.payload.contains("[data-composer-surface-variant][data-composer-layout]"));
        assert!(built.payload.contains("data-cts-composer-surface-compat"));
        assert!(built.payload.contains("reconcileComposerSurfaces(document)"));
        assert!(built.payload.contains("clearComposerSurfaceCompat(document)"));
        assert!(built.payload.contains(".composer-surface-chrome"));
        assert!(!built.payload.contains("_ComposerLayoutRoot_"));
    }

    #[test]
    fn verify_expression_embeds_version() {
        let expr = verify_expression("9.9.9").unwrap();
        assert!(expr.contains("\"9.9.9\""));
        assert!(expr.contains("result.pass"));
        assert!(expr.contains("mainSurfaceMode"));
        assert!(expr.contains("mainSurfaceCompatible"));
        assert!(expr.contains("stageAttachedToMainSurface"));
        assert!(expr.contains("visibleSurfaceScore"));
        assert!(expr.contains("data-app-shell-active-page"));
        assert!(expr.contains("currentMainSurfaces"));
        assert!(expr.contains("legacyMainSurfaces"));
        assert!(expr.contains("mainSurfaceNode ? selectComposerSurfaces(mainSurfaceNode) : []"));
        assert!(expr.contains("composerNodes.find((node) => visibleSurfaceScore(node) >= 0)"));
        assert!(!expr.contains("selectComposerSurfaces(document)"));
        assert!(!expr.contains("composerNodes[0]"));
        assert!(expr.contains("composerOverflow"));
        assert!(expr.contains("composerSurfaceMode"));
        assert!(expr.contains("composerSurfaceCompatible"));
        assert!(expr.contains("modeValid"));
        assert!(expr.contains("editorValid"));
        assert!(expr.contains("26.727.51351"));
    }

    /// Golden-fixture parity test. `tests/fixtures/golden/` is the shared
    /// input the single-theme-runtime migration checks on both sides of the
    /// vendoring boundary: `awesome-codex-skins`'s
    /// `studio/test/runtime-golden.test.mjs` builds a payload from the same
    /// fixture shape (id `golden-fixture`, same CSS, an overlay+stage chrome
    /// fragment, one asset, one motion asset) through its own `buildPayload`
    /// and executes it against real DOM snapshots; this test builds the
    /// identical fixture through Rust's `build_payload` and asserts on the
    /// generated payload *text* instead (Rust has no DOM to execute against).
    /// The two are intentionally *not* compared by raw stamp equality —
    /// `payload.rs` and `payload.mjs` fingerprint different inputs (this
    /// crate's version constant vs. `STUDIO_VERSION`, and a
    /// post-substitution vs. pre-substitution runtime template) so a
    /// byte-identical package can legitimately produce different stamps
    /// between the two engines. What must match is the *normalized*
    /// structural contract below. If a future runtime edit changes one
    /// side's observable output without the other fixture/test being
    /// updated in lockstep, this test (or its studio counterpart) fails.
    #[test]
    fn golden_fixture_payload_matches_contract() {
        let fixture_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/golden");
        let built = build_payload(&fixture_dir).unwrap();

        // No placeholder survives substitution.
        assert!(!built.payload.contains("__CTS_"), "unsubstituted placeholder");

        // Exactly the assets the fixture declares: one CSS-inlined image
        // plus one dedicated-slot motion asset (asset_count sums both).
        assert_eq!(built.asset_count, 2, "fixture declares one still asset and one motion asset");
        assert!(
            built.payload.contains("--cts-asset-wall: url(\\\"data:image/png;base64,"),
            "still asset must ride the CSS custom property as a data URL"
        );
        assert!(
            built.payload.contains("data:video/mp4;base64,"),
            "motion asset must be present as a data URL in the dedicated motion slot"
        );
        assert!(
            !built.payload.contains("--cts-asset-intro-video"),
            "motion assets must never become a CSS custom property"
        );

        // Both chrome layers from the fixture's overlay+stage markup.
        assert!(built.payload.contains("data-cts-layer=\\\"overlay\\\""));
        assert!(built.payload.contains("data-cts-layer=\\\"stage\\\""));

        // Main-surface compatibility shim: current selector takes priority
        // over the legacy one, and the compat marker exists.
        //
        // theme-runtime.js is embedded into the payload as literal JS source
        // and is never evaluated by Rust, so `${LEGACY_SHELL_MAIN_CLASS}`
        // below is NOT Rust/format! interpolation -- it is (one form of) the
        // literal JS text theme-runtime.js writes for its
        // `document.querySelector(\`main.${LEGACY_SHELL_MAIN_CLASS}\`)` call.
        // This assertion is therefore checking the raw embedded JS source
        // text for known constructions of that call, not the executed
        // selector-ordering behavior itself -- that behavior is exercised
        // against a real DOM by studio's `runtime-golden.test.mjs`.
        //
        // `find_legacy_main_surface_fallback` (see its doc comment) bounds
        // the search to the `resolveShellMain()` shim region specifically so
        // an unrelated, always-present `main.main-surface` occurrence
        // elsewhere in the embedded source (e.g. `integrateWindowsMenu`'s
        // `:scope > main.main-surface` query, which lies after this region)
        // cannot satisfy the assertion by coincidence.
        let current = built
            .payload
            .find("main[data-app-shell-main-surface]")
            .expect("current main-surface selector must be present");
        find_legacy_main_surface_fallback(&built.payload, current).expect(
            "legacy main-surface fallback must be present between the current selector and \
             `integrateWindowsMenu` (checked the template-literal, string-concatenation, and \
             inlined-literal forms of theme-runtime.js's selector construction -- if \
             theme-runtime.js's legacy-selector expression changed to a different form, update \
             this test rather than assuming the fallback itself broke)",
        );
        assert!(built.payload.contains("data-cts-main-surface-compat"));

        // Composer-surface compatibility shim: both the current CSS-module
        // selector and the legacy-class compat marker are present.
        assert!(built.payload.contains("[data-composer-surface-variant][data-composer-layout]"));
        assert!(built.payload.contains("data-cts-composer-surface-compat"));
        assert!(built.payload.contains("reconcileComposerSurfaces(document)"));
        assert!(built.payload.contains("clearComposerSurfaceCompat(document)"));

        // Composer overflow contract markers that the injected runtime relies
        // on for classification and the hardening CSS appended after theme
        // CSS.
        assert!(built.payload.contains("annotateComposerOverflow.invalidate()"));
        assert!(built.payload.contains("data-cts-composer-overflow=\\\"shell\\\""));
        assert!(built.payload.contains("overflow: clip !important"));

        // Stamp shape: engine version, fixture id, and a non-empty hash
        // segment. Not compared against studio's stamp for a byte-identical
        // package — see the doc comment above.
        assert!(built.stamp.starts_with(&format!("{ENGINE_VERSION}:golden-fixture:")));
        assert!(
            built.stamp.rsplit(':').next().is_some_and(|h| !h.is_empty()),
            "stamp must carry a non-empty fingerprint segment"
        );
    }
}
