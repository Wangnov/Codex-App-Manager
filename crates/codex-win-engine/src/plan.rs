use serde::Serialize;

use crate::capability::{SideloadRecommendation, WinCapabilityReport};
use crate::manifest::WindowsRelease;
use crate::sys::InstalledWindowsCodex;
use crate::version::compare_versions;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum WinInstallRoute {
    MsixSideload,
    PortableFallback,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowsUpdatePlan {
    pub up_to_date: bool,
    pub current_version: Option<String>,
    pub latest_version: String,
    pub package_moniker: String,
    pub package_url: String,
    pub download_size: Option<u64>,
    pub sha256: String,
    pub route: WinInstallRoute,
    pub portable_fallback_ready: bool,
    pub warnings: Vec<String>,
}

pub fn plan_update(
    release: &WindowsRelease,
    sha256: &str,
    package_url: &str,
    installed: &Option<InstalledWindowsCodex>,
    capabilities: &WinCapabilityReport,
    portable_fallback_ready: bool,
) -> WindowsUpdatePlan {
    let current_version = installed.as_ref().map(|i| i.version.clone());
    let up_to_date = current_version
        .as_ref()
        .map(|current| compare_versions(current, &release.version).is_ge())
        .unwrap_or(false);

    let route = match capabilities.recommendation {
        SideloadRecommendation::MsixPreferred => WinInstallRoute::MsixSideload,
        SideloadRecommendation::PortableFallback => WinInstallRoute::PortableFallback,
    };

    let mut warnings = capabilities.notes.clone();
    if matches!(route, WinInstallRoute::PortableFallback) && !portable_fallback_ready {
        warnings.push("Portable fallback execution is unavailable in this context.".to_string());
    }

    WindowsUpdatePlan {
        up_to_date,
        current_version,
        latest_version: release.version.clone(),
        package_moniker: release.package_moniker.clone(),
        package_url: package_url.to_string(),
        download_size: release.content_length,
        sha256: sha256.to_ascii_lowercase(),
        route,
        portable_fallback_ready,
        warnings,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capability::{CapabilityCheck, WinCapabilityReport};

    fn release() -> WindowsRelease {
        WindowsRelease {
            version: "26.602.3474.0".to_string(),
            released_at: None,
            package_moniker: "OpenAI.Codex_26.602.3474.0_x64__2p2nqsd0c76g0".to_string(),
            architecture: Some("x64".to_string()),
            download_architecture: None,
            content_length: Some(10),
            etag: None,
            store_product_id: Some("9PLM9XGG6VKS".to_string()),
            package_identity: Some("OpenAI.Codex".to_string()),
        }
    }

    #[test]
    fn plans_msix_for_update_when_capable() {
        let capabilities = WinCapabilityReport::from_checks(
            CapabilityCheck::available("present"),
            CapabilityCheck::available("running"),
            CapabilityCheck::available("AllowAllTrustedApps=1"),
            CapabilityCheck::available("installed"),
            CapabilityCheck::available("PackageManager activates"),
            CapabilityCheck::unknown("not probed"),
            vec![],
        );
        let installed = Some(InstalledWindowsCodex {
            path: "C:/Program Files/WindowsApps/OpenAI.Codex".to_string(),
            version: "26.602.3000.0".to_string(),
            arch: Some("x64".to_string()),
            source: "msix".to_string(),
            package_family_name: Some("OpenAI.Codex_2p2nqsd0c76g0".to_string()),
            installed_at: None,
        });
        let plan = plan_update(
            &release(),
            "a".repeat(64).as_str(),
            "https://example/win",
            &installed,
            &capabilities,
            false,
        );
        assert!(!plan.up_to_date);
        assert_eq!(plan.route, WinInstallRoute::MsixSideload);
    }

    #[test]
    fn routes_to_portable_when_sideloading_is_blocked() {
        let capabilities = WinCapabilityReport::from_checks(
            CapabilityCheck::available("present"),
            CapabilityCheck::available("running"),
            CapabilityCheck::unavailable("policy blocks trusted apps"),
            CapabilityCheck::available("installed"),
            CapabilityCheck::available("PackageManager activates"),
            CapabilityCheck::unknown("not probed"),
            vec![],
        );
        let plan = plan_update(
            &release(),
            "a".repeat(64).as_str(),
            "https://example/win",
            &None,
            &capabilities,
            false,
        );
        assert_eq!(plan.route, WinInstallRoute::PortableFallback);
        assert!(!plan.portable_fallback_ready);
    }
}
