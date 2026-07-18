//! Renderer payload assembly (port of `payload.mjs`): the runtime template
//! with the theme CSS, config, chrome fragment and inlined assets substituted
//! in, plus the remove/verify expressions used by the daemon and callers.

use std::path::Path;

use sha1::{Digest, Sha1};

use crate::theme::{inline_assets, load_theme, LoadedTheme, ThemeConfig};
use crate::{Result, ENGINE_VERSION};

/// The injected renderer runtime — codex-theme-studio's file verbatim. It
/// encodes the flicker discipline (compare-before-write), sticky route
/// detection, icon annotation and cleanup contract; edit it in the studio,
/// not here.
const RUNTIME_TEMPLATE: &str = include_str!("runtime/theme-runtime.js");

#[derive(Debug, Clone)]
pub struct BuiltPayload {
    pub payload: String,
    pub theme: ThemeConfig,
    /// Full stamp injected into the renderer: `<version>:<id>:<sha1[..12]>`.
    pub stamp: String,
    pub payload_bytes: usize,
    pub asset_count: usize,
}

/// Build the `Runtime.evaluate` payload for a theme directory.
pub fn build_payload(theme_dir: &Path) -> Result<BuiltPayload> {
    build_payload_from(load_theme(theme_dir)?)
}

pub fn build_payload_from(theme: LoadedTheme) -> Result<BuiltPayload> {
    let data_urls = inline_assets(&theme)?;
    // Asset variables ride inside the stylesheet as data: URLs — immune to the
    // blob revocation races that break late-loading images (border-image).
    let asset_variables = data_urls
        .iter()
        .map(|(key, url)| format!("  --cts-asset-{key}: url(\"{url}\");"))
        .collect::<Vec<_>>()
        .join("\n");
    let css_with_assets = format!(
        ":root.codex-theme-studio {{\n{asset_variables}\n}}\n\n{}",
        theme.css
    );
    let config_json = serde_json::to_string(&theme.config)
        .map_err(|e| crate::ThemeEngineError::Theme(format!("config serialize: {e}")))?;
    let chrome_html = theme.chrome_html.clone();

    // Fingerprint the executable packed payload, including the renderer
    // runtime. Without the runtime template, renderer-only bug fixes share the
    // old stamp and the daemon cannot distinguish them from an installed copy.
    let short = fingerprint(
        RUNTIME_TEMPLATE,
        &css_with_assets,
        chrome_html.as_deref().unwrap_or(""),
        &config_json,
    );
    let stamp = format!("{ENGINE_VERSION}:{}:{short}", theme.config.id);

    let payload = RUNTIME_TEMPLATE
        .replace("__CTS_CSS_JSON__", &js_json(&css_with_assets)?)
        .replace("__CTS_THEME_JSON__", &config_json)
        .replace(
            "__CTS_CHROME_JSON__",
            &serde_json::to_string(&chrome_html)
                .map_err(|e| crate::ThemeEngineError::Theme(format!("chrome serialize: {e}")))?,
        )
        .replace("__CTS_VERSION_JSON__", &js_json(ENGINE_VERSION)?)
        .replace("__CTS_STAMP_JSON__", &js_json(&stamp)?);

    Ok(BuiltPayload {
        payload_bytes: payload.len(),
        asset_count: data_urls.len(),
        theme: theme.config,
        stamp,
        payload,
    })
}

fn js_json(value: &str) -> Result<String> {
    serde_json::to_string(value)
        .map_err(|e| crate::ThemeEngineError::Theme(format!("payload serialize: {e}")))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn fingerprint(runtime: &str, css: &str, chrome: &str, config: &str) -> String {
    let mut hasher = Sha1::new();
    hasher.update(runtime.as_bytes());
    hasher.update(css.as_bytes());
    hasher.update(chrome.as_bytes());
    hasher.update(config.as_bytes());
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
  document.querySelectorAll('.cts-windows-menu-bar').forEach((node) => node.classList.remove('cts-windows-menu-bar'));
  document.querySelectorAll('[data-cts-menu-region]').forEach((node) => node.removeAttribute('data-cts-menu-region'));
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
  !document.documentElement.style.getPropertyValue('--cts-windows-menu-height') &&
  !document.documentElement.style.getPropertyValue('--cts-windows-sidebar-padding-top') &&
  !document.documentElement.style.getPropertyValue('--cts-windows-main-padding-top') &&
  !document.documentElement.style.getPropertyValue('--cts-windows-sidebar-foreground') &&
  !document.documentElement.style.getPropertyValue('--cts-windows-main-foreground') &&
  !document.getElementById('cts-style') &&
  !document.getElementById('cts-chrome') &&
  !document.getElementById('cts-stage') &&
  !document.getElementById('cts-intro') &&
  !window.__CODEX_THEME_STUDIO__
)()"#;

/// The daemon's per-tick reconciliation probe: what stamp (if any) does the
/// renderer currently carry? `null` on stock pages.
pub const CURRENT_STAMP_EXPRESSION: &str =
    "window.__CODEX_THEME_STUDIO__ ? (window.__CODEX_THEME_STUDIO__.stamp ?? null) : null";

/// Structural verification of an applied theme (port of `verifyExpression`).
pub fn verify_expression(expected_version: &str) -> Result<String> {
    let version_json = js_json(expected_version)?;
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
    const state = window.__CODEX_THEME_STUDIO__;
    const composer = box(document.querySelector('.composer-surface-chrome'));
    const sidebar = box(document.querySelector('aside.app-shell-left-panel'));
    const result = {{
      installed: document.documentElement.classList.contains('codex-theme-studio'),
      themeId: document.documentElement.getAttribute('data-cts-theme'),
      version: state?.version ?? null,
      stylePresent: Boolean(document.getElementById('cts-style')),
      chromePresent: Boolean(chrome),
      chromePointerEvents: chrome ? getComputedStyle(chrome).pointerEvents : null,
      composer,
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
      Boolean(result.composer?.visible) &&
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
    fn fingerprint_tracks_runtime_changes() {
        let first = fingerprint("runtime-a", "css", "chrome", "config");
        let second = fingerprint("runtime-b", "css", "chrome", "config");
        assert_ne!(first, second, "runtime change must re-stamp");
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
            "cts-windows-menu-bar",
            "data-cts-menu-region",
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
    fn verify_expression_embeds_version() {
        let expr = verify_expression("9.9.9").unwrap();
        assert!(expr.contains("\"9.9.9\""));
        assert!(expr.contains("result.pass"));
    }
}
