//! codex-win-engine
//!
//! Pure Windows update/install logic for Codex App Manager. This crate keeps
//! parsing, verification, capability detection, and planning free of any Tauri
//! dependency so it can be tested in isolation and compiled as an unconditional
//! dependency of the cross-platform desktop app.
//!
//! Safety posture:
//!   - keep normal installs unelevated; request one-shot UAC only to release an
//!     exact SYSTEM Windows Update transaction blocking a verified local MSIX;
//!   - do not change machine policy or trust stores;
//!   - treat OpenAI Authenticode as the Windows trust anchor;
//!   - fall back to portable only when the MSIX route is unavailable or fails.

pub mod app_version;
pub mod authenticode;
pub mod capability;
pub mod checksums;
pub mod download;
pub mod limits;
pub mod manifest;
pub mod msix;
pub mod network;
pub mod plan;
pub mod portable;
mod process;
pub mod sys;
pub mod version;
mod windows_process;

pub use app_version::{
    read_codex_app_version_from_asar, read_codex_app_version_from_install_root,
};
pub use authenticode::{
    verify_openai_authenticode, AuthenticodeReport, OPENAI_MARKETPLACE_PUBLISHER_SUBJECT,
};
pub use capability::{
    CapabilityCheck, CapabilityState, SideloadRecommendation, WinCapabilityReport,
};
pub use checksums::{find_msix_sha256, parse_checksums, ChecksumEntry};
pub use download::{
    cancel_active_download, download_to, download_to_with_network, download_to_with_progress,
    download_to_with_progress_bounded, download_to_with_progress_bounded_with_network,
    download_to_with_progress_with_network, pause_active_download, read_file, sha256_file,
};
pub use manifest::{parse_manifest, WindowsRelease};
pub use msix::{
    framework_dependencies, is_framework_dependency, parse_appx_manifest_dependencies,
    parse_appx_manifest_xml, read_msix_dependencies, read_msix_identity, validate_codex_identity,
    MsixIdentity, MsixPackageDependency,
};
pub use network::NetworkConfig;
pub use plan::{plan_update, WinInstallRoute, WindowsUpdatePlan};
pub use portable::{
    cleanup_portable_metadata, close_codex_gracefully_for_root, codex_running_for_root,
    inject_portable_fault, install_portable_from_msix, install_portable_from_msix_with_observer,
    installed_app_exe, purge_codex_user_data, remove_directory_all_with_retry,
    rename_directory_with_retry, uninstall_portable, PortableBoundary, PortableFault,
    PortableInstallReport, PortableUninstallReport,
};
pub use sys::{
    close_msix_codex_processes, detect_installed_codex, detect_portable_install, fetch_text,
    fetch_text_with_network, launch_codex, launch_codex_with_options, probe_capabilities,
    remove_msix_package, InstalledWindowsCodex, LaunchOptions, MsixRemoveReport,
};
pub use sys::{
    install_msix_sideload, install_msix_sideload_with_observer, precheck_msix_dependencies,
    verify_msix_health, verify_msix_health_with_options, MsixDependencyPrecheck, MsixHealthReport,
    MsixSideloadReport,
};
// Failure-kind constants for structured MSIX health outcomes.
pub use sys::msix_failure;
pub use version::{compare_versions, version_key};
pub use windows_process::same_windows_path;

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

#[cfg(test)]
mod constrained_language_regressions {
    #[test]
    fn runtime_powershell_avoids_custom_object_casts() {
        let forbidden = ["[pscustom", "object]"].concat();
        for (name, source) in [
            ("authenticode.rs", include_str!("authenticode.rs")),
            ("sys.rs", include_str!("sys.rs")),
        ] {
            assert!(
                !source.to_ascii_lowercase().contains(&forbidden),
                "{name} contains a PowerShell custom-object cast that fails in ConstrainedLanguage"
            );
        }
    }
}
