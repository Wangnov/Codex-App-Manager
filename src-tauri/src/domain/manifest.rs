use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MirrorEndpoints {
    pub manifest_url: String,
    pub checksums_url: String,
    pub windows_msix_url: String,
    pub windows_x64_msix_url: String,
    pub windows_arm64_msix_url: String,
    pub windows_unpacked_url: String,
    pub mac_arm64_url: String,
    pub mac_intel_url: String,
}

impl MirrorEndpoints {
    pub fn from_base_url(base_url: &str) -> Self {
        let base = base_url.trim_end_matches('/');

        Self {
            manifest_url: format!("{base}/latest/manifest"),
            checksums_url: format!("{base}/latest/checksums"),
            windows_msix_url: format!("{base}/latest/win"),
            windows_x64_msix_url: format!("{base}/latest/win-x64"),
            windows_arm64_msix_url: format!("{base}/latest/win-arm64"),
            windows_unpacked_url: format!("{base}/latest/win-unpacked"),
            mac_arm64_url: format!("{base}/latest/mac-arm64"),
            mac_intel_url: format!("{base}/latest/mac-intel"),
        }
    }

    pub fn windows_msix_url_for_arch(&self, architecture: Option<&str>) -> &str {
        match architecture.map(|arch| arch.trim().to_ascii_lowercase()) {
            Some(arch) if arch == "arm64" || arch == "aarch64" => &self.windows_arm64_msix_url,
            Some(arch) if arch == "x64" || arch == "x86_64" || arch == "amd64" => {
                &self.windows_x64_msix_url
            }
            _ => &self.windows_msix_url,
        }
    }
}
