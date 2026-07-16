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
//! The injected renderer runtime (`src/runtime/theme-runtime.js`) is the
//! studio's file verbatim — it encodes hard-won flicker/idempotence/route
//! discipline (see that file's comments) and must be edited there, not here.

pub mod cdp;
pub mod daemon;
pub mod native;
pub mod payload;
pub mod theme;

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
