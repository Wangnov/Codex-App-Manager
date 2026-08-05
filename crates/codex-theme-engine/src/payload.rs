//! Renderer payload assembly (port of `payload.mjs`): the runtime template
//! with the theme CSS, config, chrome fragment and inlined assets substituted
//! in, plus the remove/verify expressions used by the daemon and callers.

use std::path::Path;

use sha1::{Digest, Sha1};

use crate::theme::{
    inline_assets, inline_motion_assets, load_theme, LoadedTheme, ThemeConfig,
};
use crate::{Result, ENGINE_VERSION};

/// The injected renderer runtime — codex-theme-studio's file verbatim. It
/// encodes the flicker discipline (compare-before-write), sticky route
/// detection, icon annotation and cleanup contract; edit it in the studio,
/// not here.
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
        "(() => {{\n{module}\nreturn {{ createComposerOverflowAnnotator, selectComposerSurfaces }};\n}})()"
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
    const chrome = document.getElementById('cts-chrome');
    const stage = document.getElementById('cts-stage');
    const mainSurfaceNode = document.querySelector('main[data-app-shell-main-surface], main.main-surface');
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
    const composerNodes = selectComposerSurfaces(document);
    const composerNode = composerNodes.find((node) => {{
      const r = node.getBoundingClientRect();
      const style = getComputedStyle(node);
      return r.width > 0 && r.height > 0 && style.display !== 'none' && style.visibility !== 'hidden';
    }}) ?? composerNodes[0] ?? null;
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
    fn verify_expression_embeds_version() {
        let expr = verify_expression("9.9.9").unwrap();
        assert!(expr.contains("\"9.9.9\""));
        assert!(expr.contains("result.pass"));
        assert!(expr.contains("mainSurfaceMode"));
        assert!(expr.contains("mainSurfaceCompatible"));
        assert!(expr.contains("stageAttachedToMainSurface"));
        assert!(expr.contains("composerOverflow"));
        assert!(expr.contains("modeValid"));
        assert!(expr.contains("editorValid"));
        assert!(expr.contains("26.727.51351"));
    }
}
