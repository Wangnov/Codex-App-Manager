//! codex-win-engine
//!
//! Pure Windows update/install logic for Codex App Manager. This crate keeps
//! parsing, verification, capability detection, and planning free of any Tauri
//! dependency so it can be tested in isolation and compiled as an unconditional
//! dependency of the cross-platform desktop app.
//!
//! Safety posture:
//!   - do not elevate;
//!   - do not change machine policy or trust stores;
//!   - treat OpenAI Authenticode as the Windows trust anchor;
//!   - fall back to portable only when the MSIX route is unavailable or fails.

pub mod authenticode;
pub mod capability;
pub mod checksums;
pub mod download;
pub mod manifest;
pub mod msix;
pub mod plan;
pub mod portable;
mod process;
pub mod sys;
pub mod version;

pub use authenticode::{
    verify_openai_authenticode, AuthenticodeReport, OPENAI_MARKETPLACE_PUBLISHER_SUBJECT,
};
pub use capability::{
    CapabilityCheck, CapabilityState, SideloadRecommendation, WinCapabilityReport,
};
pub use checksums::{find_msix_sha256, parse_checksums, ChecksumEntry};
pub use download::{
    cancel_active_download, download_to, download_to_with_progress, read_file, sha256_file,
};
pub use manifest::{parse_manifest, WindowsRelease};
pub use msix::{
    parse_appx_manifest_xml, read_msix_identity, validate_codex_identity, MsixIdentity,
};
pub use plan::{plan_update, WinInstallRoute, WindowsUpdatePlan};
pub use portable::{
    close_codex_gracefully, close_codex_gracefully_for_root, install_portable_from_msix,
    purge_codex_user_data, uninstall_portable, PortableInstallReport, PortableUninstallReport,
};
pub use sys::{
    detect_installed_codex, detect_portable_install, fetch_text, launch_codex,
    probe_capabilities, remove_msix_package, InstalledWindowsCodex, MsixRemoveReport,
};
pub use sys::{install_msix_sideload, verify_msix_health, MsixHealthReport, MsixSideloadReport};
pub use version::{compare_versions, version_key};

pub const OPENAI_PACKAGE_IDENTITY: &str = "OpenAI.Codex";

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("failed to parse manifest: {0}")]
    Manifest(String),
    #[error("failed to parse checksums: {0}")]
    Checksums(String),
    #[error("failed to parse MSIX manifest: {0}")]
    Msix(String),
    #[error("Authenticode verification error: {0}")]
    Authenticode(String),
    #[error("capability probe error: {0}")]
    Capability(String),
    #[error("install error: {0}")]
    Install(String),
    #[error("io error: {0}")]
    Io(String),
}
