use std::fs::File;
use std::io::Read;
use std::path::Path;

use serde::Serialize;

use crate::{EngineError, OPENAI_PACKAGE_IDENTITY};

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MsixIdentity {
    pub name: String,
    pub publisher: String,
    pub version: String,
    pub processor_architecture: String,
}

/// A single `<PackageDependency>` declared in the staged MSIX manifest. These
/// are the framework packages (VCLibs, WindowsAppRuntime, UI.Xaml, NET.Native,
/// …) that `Add-AppxPackage` expects to already be present — on a stripped /
/// Store-disabled Windows it cannot auto-acquire them, so a missing one means a
/// doomed sideload. We read them up front to steer to the portable build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MsixPackageDependency {
    pub name: String,
    #[serde(default)]
    pub publisher: Option<String>,
    #[serde(default)]
    pub min_version: Option<String>,
    #[serde(default)]
    pub processor_architecture: Option<String>,
}

/// Known framework-package name prefixes that an MSIX can take a runtime
/// dependency on. Matching is case-insensitive and prefix-based so future minor
/// suffixes (e.g. `Microsoft.VCLibs.140.00`) are still recognized.
const FRAMEWORK_DEPENDENCY_PREFIXES: &[&str] = &[
    "Microsoft.VCLibs.",
    "Microsoft.WindowsAppRuntime.",
    "Microsoft.UI.Xaml.",
    "Microsoft.NET.Native.",
];

/// Whether a declared dependency name is one of the redistributable framework
/// packages we know how to pre-check. Non-framework dependencies (e.g. another
/// of the vendor's own packages) are intentionally NOT pre-checked here: we only
/// want to steer to portable when a *framework* the sideload cannot acquire is
/// absent. Pure + cross-platform so it is unit-tested off Windows.
pub fn is_framework_dependency(name: &str) -> bool {
    FRAMEWORK_DEPENDENCY_PREFIXES
        .iter()
        .any(|prefix| name.len() >= prefix.len() && name[..prefix.len()].eq_ignore_ascii_case(prefix))
}

pub fn parse_appx_manifest_xml(xml: &str) -> Result<MsixIdentity, EngineError> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| EngineError::Msix(format!("AppxManifest.xml: {e}")))?;
    let identity = doc
        .descendants()
        .find(|node| node.has_tag_name("Identity"))
        .ok_or_else(|| EngineError::Msix("AppxManifest.xml missing Identity".to_string()))?;

    let get = |name: &str| {
        identity
            .attribute(name)
            .map(str::to_string)
            .ok_or_else(|| EngineError::Msix(format!("Identity missing {name}")))
    };

    Ok(MsixIdentity {
        name: get("Name")?,
        publisher: get("Publisher")?,
        version: get("Version")?,
        processor_architecture: get("ProcessorArchitecture")?,
    })
}

pub fn read_msix_identity(path: &Path) -> Result<MsixIdentity, EngineError> {
    let file = File::open(path).map_err(|e| EngineError::Io(format!("open MSIX: {e}")))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| EngineError::Msix(format!("open MSIX zip: {e}")))?;
    let mut manifest = zip
        .by_name("AppxManifest.xml")
        .map_err(|e| EngineError::Msix(format!("read AppxManifest.xml: {e}")))?;
    let mut xml = String::new();
    manifest
        .read_to_string(&mut xml)
        .map_err(|e| EngineError::Msix(format!("decode AppxManifest.xml: {e}")))?;
    parse_appx_manifest_xml(&xml)
}

/// Parse the `<Dependencies><PackageDependency .../>` entries from an
/// AppxManifest.xml. Pure + namespace-agnostic (we match by local tag name and
/// read attributes directly) so it is unit-tested off Windows. Mirrors how
/// `verify_msix_health` reads `Package.Dependencies.PackageDependency` in
/// PowerShell, but here it runs against the staged package *before* install.
pub fn parse_appx_manifest_dependencies(
    xml: &str,
) -> Result<Vec<MsixPackageDependency>, EngineError> {
    let doc = roxmltree::Document::parse(xml)
        .map_err(|e| EngineError::Msix(format!("AppxManifest.xml: {e}")))?;
    let deps = doc
        .descendants()
        .filter(|node| node.has_tag_name("PackageDependency"))
        .filter_map(|node| {
            let name = node.attribute("Name")?.trim();
            if name.is_empty() {
                return None;
            }
            let min_version = node
                .attribute("MinVersion")
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let publisher = node
                .attribute("Publisher")
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let processor_architecture = node
                .attribute("ProcessorArchitecture")
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            Some(MsixPackageDependency {
                name: name.to_string(),
                publisher,
                min_version,
                processor_architecture,
            })
        })
        .collect();
    Ok(deps)
}

/// Read and parse the declared `PackageDependency` entries from a staged MSIX on
/// disk (same zip + AppxManifest.xml path as `read_msix_identity`).
pub fn read_msix_dependencies(path: &Path) -> Result<Vec<MsixPackageDependency>, EngineError> {
    let file = File::open(path).map_err(|e| EngineError::Io(format!("open MSIX: {e}")))?;
    let mut zip =
        zip::ZipArchive::new(file).map_err(|e| EngineError::Msix(format!("open MSIX zip: {e}")))?;
    let mut manifest = zip
        .by_name("AppxManifest.xml")
        .map_err(|e| EngineError::Msix(format!("read AppxManifest.xml: {e}")))?;
    let mut xml = String::new();
    manifest
        .read_to_string(&mut xml)
        .map_err(|e| EngineError::Msix(format!("decode AppxManifest.xml: {e}")))?;
    parse_appx_manifest_dependencies(&xml)
}

/// The subset of declared dependencies that are redistributable *framework*
/// packages we pre-check before sideloading. Pure + cross-platform.
pub fn framework_dependencies(deps: &[MsixPackageDependency]) -> Vec<MsixPackageDependency> {
    deps.iter()
        .filter(|d| is_framework_dependency(&d.name))
        .cloned()
        .collect()
}

fn arch_matches(actual: &str, expected: Option<&str>) -> bool {
    let Some(expected) = expected else {
        return true;
    };
    let actual = actual.to_ascii_lowercase();
    let expected = expected.to_ascii_lowercase();
    match expected.as_str() {
        "x64" | "x86_64" | "amd64" => actual == "x64" || actual == "neutral",
        "arm64" | "aarch64" => actual == "arm64" || actual == "neutral",
        _ => actual == expected,
    }
}

pub fn validate_codex_identity(
    identity: &MsixIdentity,
    expected_version: &str,
    expected_architecture: Option<&str>,
) -> Result<(), EngineError> {
    if identity.name != OPENAI_PACKAGE_IDENTITY {
        return Err(EngineError::Msix(format!(
            "unexpected package identity {}; expected {}",
            identity.name, OPENAI_PACKAGE_IDENTITY
        )));
    }
    if identity.version != expected_version {
        return Err(EngineError::Msix(format!(
            "unexpected package version {}; expected {}",
            identity.version, expected_version
        )));
    }
    if !arch_matches(&identity.processor_architecture, expected_architecture) {
        return Err(EngineError::Msix(format!(
            "unexpected architecture {}; expected {}",
            identity.processor_architecture,
            expected_architecture.unwrap_or("any")
        )));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_appx_identity_xml() {
        let xml = r#"
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Identity Name="OpenAI.Codex"
            Publisher="CN=OpenAI OpCo, LLC, O=OpenAI OpCo, LLC, C=US"
            Version="26.602.3474.0"
            ProcessorArchitecture="x64" />
</Package>"#;
        let identity = parse_appx_manifest_xml(xml).unwrap();
        assert_eq!(identity.name, "OpenAI.Codex");
        assert_eq!(identity.version, "26.602.3474.0");
        validate_codex_identity(&identity, "26.602.3474.0", Some("x64")).unwrap();
    }

    #[test]
    fn rejects_wrong_identity() {
        let identity = MsixIdentity {
            name: "OpenAI.Other".to_string(),
            publisher: "CN=OpenAI".to_string(),
            version: "26.602.3474.0".to_string(),
            processor_architecture: "x64".to_string(),
        };
        assert!(validate_codex_identity(&identity, "26.602.3474.0", Some("x64")).is_err());
    }

    #[test]
    fn classifies_framework_dependencies_by_prefix() {
        for name in [
            "Microsoft.VCLibs.140.00",
            "Microsoft.VCLibs.140.00.UWPDesktop",
            "Microsoft.WindowsAppRuntime.1.5",
            "Microsoft.UI.Xaml.2.8",
            "Microsoft.NET.Native.Framework.2.2",
            // Case-insensitive: registry/manifest casing can vary.
            "microsoft.vclibs.140.00",
        ] {
            assert!(is_framework_dependency(name), "{name} should be a framework");
        }
        for name in ["OpenAI.Codex", "Microsoft.WindowsTerminal", "Contoso.App"] {
            assert!(
                !is_framework_dependency(name),
                "{name} should NOT be a framework"
            );
        }
    }

    #[test]
    fn parses_package_dependencies_from_manifest() {
        let xml = r#"
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Identity Name="OpenAI.Codex" Publisher="CN=OpenAI" Version="26.602.3474.0" ProcessorArchitecture="x64" />
  <Dependencies>
    <TargetDeviceFamily Name="Windows.Desktop" MinVersion="10.0.17763.0" MaxVersionTested="10.0.22000.0" />
    <PackageDependency Name="Microsoft.VCLibs.140.00" MinVersion="14.0.30704.0" Publisher="CN=Microsoft" />
    <PackageDependency Name="Microsoft.WindowsAppRuntime.1.5" MinVersion="5000.522.2030.0" Publisher="CN=Microsoft" />
    <PackageDependency Name="Contoso.Helper" MinVersion="1.0.0.0" />
  </Dependencies>
</Package>"#;
        let deps = parse_appx_manifest_dependencies(xml).unwrap();
        assert_eq!(deps.len(), 3);
        assert_eq!(deps[0].name, "Microsoft.VCLibs.140.00");
        assert_eq!(deps[0].publisher.as_deref(), Some("CN=Microsoft"));
        assert_eq!(deps[0].min_version.as_deref(), Some("14.0.30704.0"));
        assert!(deps[0].processor_architecture.is_none());

        let frameworks = framework_dependencies(&deps);
        // Contoso.Helper is a non-framework dependency and is excluded.
        assert_eq!(frameworks.len(), 2);
        assert!(frameworks.iter().all(|d| is_framework_dependency(&d.name)));
        assert!(frameworks
            .iter()
            .any(|d| d.name == "Microsoft.WindowsAppRuntime.1.5"));
    }

    #[test]
    fn parses_empty_when_no_dependencies_block() {
        let xml = r#"
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Identity Name="OpenAI.Codex" Publisher="CN=OpenAI" Version="1.0.0.0" ProcessorArchitecture="x64" />
</Package>"#;
        let deps = parse_appx_manifest_dependencies(xml).unwrap();
        assert!(deps.is_empty());
        assert!(framework_dependencies(&deps).is_empty());
    }

    #[test]
    fn skips_package_dependency_without_name() {
        let xml = r#"
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Dependencies>
    <PackageDependency MinVersion="1.0.0.0" />
    <PackageDependency Name="" MinVersion="1.0.0.0" />
    <PackageDependency Name="Microsoft.UI.Xaml.2.8" />
  </Dependencies>
</Package>"#;
        let deps = parse_appx_manifest_dependencies(xml).unwrap();
        assert_eq!(deps.len(), 1);
        assert_eq!(deps[0].name, "Microsoft.UI.Xaml.2.8");
        assert!(deps[0].min_version.is_none());
    }

    #[test]
    fn parses_package_dependency_architecture() {
        let xml = r#"
<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Dependencies>
    <PackageDependency Name="Microsoft.VCLibs.140.00" Publisher="CN=Microsoft" MinVersion="14.0.0.0" ProcessorArchitecture="x64" />
  </Dependencies>
</Package>"#;
        let deps = parse_appx_manifest_dependencies(xml).unwrap();
        assert_eq!(deps[0].processor_architecture.as_deref(), Some("x64"));
    }
}
