use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum CapabilityState {
    Available,
    Unavailable,
    Unknown,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityCheck {
    pub state: CapabilityState,
    pub detail: String,
}

impl CapabilityCheck {
    pub fn available(detail: impl Into<String>) -> Self {
        Self {
            state: CapabilityState::Available,
            detail: detail.into(),
        }
    }

    pub fn unavailable(detail: impl Into<String>) -> Self {
        Self {
            state: CapabilityState::Unavailable,
            detail: detail.into(),
        }
    }

    pub fn unknown(detail: impl Into<String>) -> Self {
        Self {
            state: CapabilityState::Unknown,
            detail: detail.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum SideloadRecommendation {
    MsixPreferred,
    PortableFallback,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WinCapabilityReport {
    pub add_appx_package: CapabilityCheck,
    pub appx_service: CapabilityCheck,
    pub sideload_policy: CapabilityCheck,
    pub app_installer: CapabilityCheck,
    /// Can the WinRT `PackageManager` (the COM class `Add-AppxPackage` deploys
    /// through) actually be activated? Unlike the checks above, this exercises
    /// the deployment runtime itself — so it catches the "registered but broken"
    /// machines where MSIX install dies with `0x80040154 REGDB_E_CLASSNOTREG`
    /// ("没有注册类") even though the cmdlet, service and policy all look present.
    pub msix_deployment: CapabilityCheck,
    pub metered_network: CapabilityCheck,
    pub recommendation: SideloadRecommendation,
    pub notes: Vec<String>,
}

impl WinCapabilityReport {
    pub fn from_checks(
        add_appx_package: CapabilityCheck,
        appx_service: CapabilityCheck,
        sideload_policy: CapabilityCheck,
        app_installer: CapabilityCheck,
        msix_deployment: CapabilityCheck,
        metered_network: CapabilityCheck,
        mut notes: Vec<String>,
    ) -> Self {
        let msix_blocked = add_appx_package.state == CapabilityState::Unavailable
            || appx_service.state == CapabilityState::Unavailable
            || sideload_policy.state == CapabilityState::Unavailable
            || msix_deployment.state == CapabilityState::Unavailable;
        let recommendation = if msix_blocked {
            if msix_deployment.state == CapabilityState::Unavailable {
                // The deployment runtime itself is broken — the exact 0x80040154
                // case. Sideloading cannot possibly succeed, so go straight to
                // portable instead of letting the user hit a failed MSIX attempt.
                notes.push(
                    "MSIX deployment is broken on this machine (the PackageManager COM class is not registered, e.g. 0x80040154); using the portable build."
                        .to_string(),
                );
            } else {
                notes.push(
                    "MSIX sideloading appears blocked; use the portable fallback when available."
                        .to_string(),
                );
            }
            SideloadRecommendation::PortableFallback
        } else {
            if sideload_policy.state == CapabilityState::Unknown {
                notes.push(
                    "Sideload policy is not explicitly enabled; installation may still succeed on modern Windows and will fall back if it fails."
                        .to_string(),
                );
            }
            SideloadRecommendation::MsixPreferred
        };

        Self {
            add_appx_package,
            appx_service,
            sideload_policy,
            app_installer,
            msix_deployment,
            metered_network,
            recommendation,
            notes,
        }
    }

    pub fn unknown_for_non_windows() -> Self {
        Self::from_checks(
            CapabilityCheck::unknown("not running on Windows"),
            CapabilityCheck::unknown("not running on Windows"),
            CapabilityCheck::unknown("not running on Windows"),
            CapabilityCheck::unknown("not running on Windows"),
            CapabilityCheck::unknown("not running on Windows"),
            CapabilityCheck::unknown("not running on Windows"),
            vec!["Windows capability checks are only meaningful on Windows.".to_string()],
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recommends_msix_when_no_blocking_signal_exists() {
        let report = WinCapabilityReport::from_checks(
            CapabilityCheck::available("present"),
            CapabilityCheck::available("running"),
            CapabilityCheck::unknown("policy absent"),
            CapabilityCheck::available("installed"),
            CapabilityCheck::available("PackageManager activates"),
            CapabilityCheck::unknown("not probed"),
            vec![],
        );
        assert_eq!(report.recommendation, SideloadRecommendation::MsixPreferred);
    }

    #[test]
    fn recommends_portable_when_policy_blocks_sideloading() {
        let report = WinCapabilityReport::from_checks(
            CapabilityCheck::available("present"),
            CapabilityCheck::available("running"),
            CapabilityCheck::unavailable("AllowAllTrustedApps=0"),
            CapabilityCheck::available("installed"),
            CapabilityCheck::available("PackageManager activates"),
            CapabilityCheck::unknown("not probed"),
            vec![],
        );
        assert_eq!(
            report.recommendation,
            SideloadRecommendation::PortableFallback
        );
    }

    #[test]
    fn recommends_portable_when_msix_deployment_is_broken() {
        // The 0x80040154 case: cmdlet, service and policy all look fine, but the
        // deployment runtime itself cannot activate, so sideloading cannot work.
        let report = WinCapabilityReport::from_checks(
            CapabilityCheck::available("present"),
            CapabilityCheck::available("running"),
            CapabilityCheck::available("AllowAllTrustedApps=1"),
            CapabilityCheck::available("installed"),
            CapabilityCheck::unavailable(
                "PackageManager activation failed: 没有注册类 (HRESULT=0x80040154)",
            ),
            CapabilityCheck::unknown("not probed"),
            vec![],
        );
        assert_eq!(
            report.recommendation,
            SideloadRecommendation::PortableFallback
        );
        assert!(report.notes.iter().any(|note| note.contains("0x80040154")));
    }
}
