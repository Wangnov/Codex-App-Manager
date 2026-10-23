//! Asset-based UI themes for the official Codex desktop app.
//!
//! Rust port of codex-theme-studio's core: a theme is AI-generated bitmap
//! assets + CSS + a decorative chrome layer, injected into the running Codex
//! renderer over the Chrome DevTools Protocol (loopback only). **No app file
//! is ever modified, unpacked or replaced** — `app.asar` stays untouched and
//! the notarized signature stays valid; turning the theme off restores stock.
//!
//! Module map (1:1 with the studio's Node modules):
//! - [`theme`]  — theme-package loading/validation (`theme.mjs`)
//! - [`payload`] — renderer payload assembly + fingerprint (`payload.mjs`)
//! - [`cdp`]    — CDP client, target discovery and probing (`cdp.mjs`)
//! - [`native`] — `~/.codex/config.toml` appearance sections (`native-theme.mjs`)
//! - [`daemon`] — in-process keeper that re-injects across reloads/new targets
//!
//! The injected renderer runtime (`src/runtime/theme-runtime.js` and
//! `src/runtime/composer-overflow.mjs`) is authored here — this crate is now
//! the single canonical implementation. It encodes hard-won
//! flicker/idempotence/route discipline (see those files' comments) and must
//! be edited here, not in the studio repo. `awesome-codex-skins`'s
//! `studio/src/runtime/theme-runtime.js` and `studio/src/composer-overflow.mjs`
//! are generated, vendored copies pinned to a commit of this crate (see that
//! repo's `studio/RUNTIME_SOURCE.json` and `SPEC.md` §5); the relationship
//! used to run the other way (this was a Rust port of the studio's file) but
//! the studio now syncs *from* here instead.

pub mod cdp;
pub mod codex_theme;
pub mod daemon;
pub mod import;
pub mod native;
pub mod native_hot;
pub mod payload;
pub mod theme;
pub mod transaction;

#[cfg(test)]
#[path = "../../test-support/zip.rs"]
mod zip_test;

/// Engine version stamped into every payload; the renderer's verify pass
/// checks it, so an engine upgrade re-injects on the next daemon tick.
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Debug, thiserror::Error)]
pub enum ThemeEngineError {
    #[error("theme package: {0}")]
    Theme(String),
    #[error("cdp: {0}")]
    Cdp(String),
    #[error("native theme: {0}")]
    Native(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type Result<T> = std::result::Result<T, ThemeEngineError>;
