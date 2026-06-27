use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::EngineError;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowsRelease {
    /// Human-facing Codex app version read from the app payload, e.g. 26.623.42026.
    pub version: String,
    /// Four-part Windows MSIX package version, e.g. 26.623.5546.0.
    pub package_version: String,
    pub released_at: Option<String>,
    pub package_moniker: String,
    pub architecture: Option<String>,
    #[serde(skip)]
    pub download_architecture: Option<String>,
    pub content_length: Option<u64>,
    pub etag: Option<String>,
    pub store_product_id: Option<String>,
    pub package_identity: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MirrorManifest {
    schema_version: u64,
    codex_version: Option<String>,
    published_at: Option<String>,
    sources: Sources,
}

#[derive(Debug, Deserialize)]
struct Sources {
    windows: WindowsSource,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowsSource {
    version: Option<String>,
    app_version: Option<String>,
    package_moniker: Option<String>,
    architecture: Option<String>,
    content_length: Option<u64>,
    last_modified: Option<String>,
    etag: Option<String>,
    product_id: Option<String>,
    update_manifest: Option<WindowsUpdateManifest>,
    #[serde(default)]
    architectures: BTreeMap<String, WindowsArchitectureSource>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowsArchitectureSource {
    version: Option<String>,
    app_version: Option<String>,
    package_moniker: Option<String>,
    architecture: Option<String>,
    content_length: Option<u64>,
    last_modified: Option<String>,
    etag: Option<String>,
    #[serde(default)]
    downloadable: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowsUpdateManifest {
    store_product_id: Option<String>,
    package_identity: Option<String>,
}

pub fn parse_manifest(text: &str) -> Result<WindowsRelease, EngineError> {
    parse_manifest_for_arch(text, Some(current_windows_package_architecture().as_str()))
}

pub fn parse_manifest_for_arch(
    text: &str,
    preferred_architecture: Option<&str>,
) -> Result<WindowsRelease, EngineError> {
    let len = text.len();
    log::debug!("parse Windows manifest start len={len}");
    let manifest: MirrorManifest = serde_json::from_str(text).map_err(|e| {
        log::warn!("parse Windows manifest failed error={e}");
        EngineError::Manifest(format!("json: {e}"))
    })?;
    if manifest.schema_version < 2 {
        let err = EngineError::Manifest(format!(
            "unsupported schemaVersion {}",
            manifest.schema_version
        ));
        log::warn!("parse Windows manifest failed error={err}");
        return Err(err);
    }

    let root_codex_version = manifest.codex_version.clone();
    let root_published_at = manifest.published_at.clone();
    let windows = manifest.sources.windows;
    let requested_architecture = preferred_architecture.and_then(normalize_architecture);
    let selected_architecture =
        select_architecture(&windows.architectures, requested_architecture.as_deref())?;

    let package_version = selected_architecture
        .as_ref()
        .and_then(|(_, source)| source.version.clone())
        .or_else(|| windows.version.clone())
        .ok_or_else(|| EngineError::Manifest("missing Windows version".to_string()))?;
    let version = selected_architecture
        .as_ref()
        .and_then(|(_, source)| source.app_version.clone())
        .or_else(|| windows.app_version.clone())
        .or(root_codex_version)
        .unwrap_or_else(|| package_version.clone());
    let package_moniker = match selected_architecture.as_ref() {
        Some((_, source)) => source.package_moniker.clone(),
        None => windows.package_moniker.clone(),
    }
    .ok_or_else(|| EngineError::Manifest("missing Windows packageMoniker".to_string()))?;
    let architecture = match selected_architecture.as_ref() {
        Some((arch, source)) => source.architecture.clone().or_else(|| Some(arch.clone())),
        None => windows.architecture,
    };
    let content_length = match selected_architecture.as_ref() {
        Some((_, source)) => source.content_length,
        None => windows.content_length,
    };
    let etag = match selected_architecture.as_ref() {
        Some((_, source)) => source.etag.clone(),
        None => windows.etag,
    };
    let released_at = match selected_architecture.as_ref() {
        Some((_, source)) => source
            .last_modified
            .clone()
            .or(windows.last_modified)
            .or(root_published_at),
        None => windows.last_modified.or(root_published_at),
    };

    let release = WindowsRelease {
        version,
        package_version,
        released_at,
        package_moniker,
        architecture,
        download_architecture: selected_architecture.as_ref().map(|(arch, _)| arch.clone()),
        content_length,
        etag,
        store_product_id: windows
            .update_manifest
            .as_ref()
            .and_then(|m| m.store_product_id.clone())
            .or(windows.product_id),
        package_identity: windows.update_manifest.and_then(|m| m.package_identity),
    };
    let arch = release.architecture.as_deref().unwrap_or("unknown");
    log::debug!(
        "parse Windows manifest succeeded version={} package_version={} package_moniker={} arch={arch}",
        release.version,
        release.package_version,
        release.package_moniker
    );
    Ok(release)
}

fn select_architecture<'a>(
    architectures: &'a BTreeMap<String, WindowsArchitectureSource>,
    requested_architecture: Option<&str>,
) -> Result<Option<(String, &'a WindowsArchitectureSource)>, EngineError> {
    let Some(requested) = requested_architecture.or(Some("x64")) else {
        return Ok(None);
    };
    if architectures.is_empty() {
        return Ok(None);
    }

    let matching = architectures
        .iter()
        .find(|(arch, _)| normalize_architecture(arch).as_deref() == Some(requested));
    match matching {
        Some((_, source)) if source.downloadable.unwrap_or(true) => {
            Ok(Some((requested.to_string(), source)))
        }
        Some((_, _)) => Err(EngineError::Manifest(format!(
            "Windows {requested} package is not available in the current mirror manifest"
        ))),
        None if requested == "arm64" => Err(EngineError::Manifest(
            "Windows arm64 package is not available in the current mirror manifest".to_string(),
        )),
        None => Ok(None),
    }
}

fn normalize_architecture(architecture: &str) -> Option<String> {
    match architecture.trim().to_ascii_lowercase().as_str() {
        "x64" | "x86_64" | "amd64" => Some("x64".to_string()),
        "arm64" | "aarch64" => Some("arm64".to_string()),
        _ => None,
    }
}

fn current_windows_package_architecture() -> String {
    #[cfg(windows)]
    {
        if let Some(arch) = native_windows_architecture() {
            return arch;
        }
        let env_arch = std::env::var("PROCESSOR_ARCHITEW6432")
            .ok()
            .or_else(|| std::env::var("PROCESSOR_ARCHITECTURE").ok());
        if env_arch
            .as_deref()
            .and_then(normalize_architecture)
            .as_deref()
            == Some("arm64")
        {
            return "arm64".to_string();
        }
        if normalize_architecture(std::env::consts::ARCH).as_deref() == Some("arm64") {
            return "arm64".to_string();
        }
    }
    "x64".to_string()
}

#[cfg(windows)]
fn native_windows_architecture() -> Option<String> {
    use windows_sys::Win32::System::SystemInformation::{
        IMAGE_FILE_MACHINE, IMAGE_FILE_MACHINE_AMD64, IMAGE_FILE_MACHINE_ARM64,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, IsWow64Process2};

    let mut process_machine: IMAGE_FILE_MACHINE = 0;
    let mut native_machine: IMAGE_FILE_MACHINE = 0;
    let ok = unsafe {
        IsWow64Process2(
            GetCurrentProcess(),
            &mut process_machine,
            &mut native_machine,
        )
    };
    if ok == 0 {
        return None;
    }
    match native_machine {
        IMAGE_FILE_MACHINE_ARM64 => Some("arm64".to_string()),
        IMAGE_FILE_MACHINE_AMD64 => Some("x64".to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_windows_source_from_v2_manifest() {
        let json = r#"{
          "schemaVersion": 2,
          "sources": {
            "windows": {
              "productId": "9PLM9XGG6VKS",
              "architecture": "x64",
              "version": "26.602.3474.0",
              "packageMoniker": "OpenAI.Codex_26.602.3474.0_x64__2p2nqsd0c76g0",
              "contentLength": 566504666,
              "lastModified": "Fri, 26 Jun 2026 05:10:43 GMT",
              "etag": "\"abc\"",
              "updateManifest": {
                "storeProductId": "9PLM9XGG6VKS",
                "packageIdentity": "OpenAI.Codex"
              }
            }
          }
        }"#;

        let release = parse_manifest(json).unwrap();
        assert_eq!(release.version, "26.602.3474.0");
        assert_eq!(release.package_version, "26.602.3474.0");
        assert_eq!(release.package_identity.as_deref(), Some("OpenAI.Codex"));
        assert_eq!(release.content_length, Some(566_504_666));
        assert_eq!(
            release.released_at.as_deref(),
            Some("Fri, 26 Jun 2026 05:10:43 GMT")
        );
        assert_eq!(release.download_architecture, None);
    }

    #[test]
    fn parses_schema_v3_codex_app_version_separately_from_package_version() {
        let json = r#"{
          "schemaVersion": 3,
          "codexVersion": "26.623.42026",
          "publishedAt": "Sat, 27 Jun 2026 07:19:49 GMT",
          "sources": {
            "windows": {
              "productId": "9PLM9XGG6VKS",
              "version": "26.623.5546.0",
              "appVersion": "26.623.42026",
              "packageMoniker": "OpenAI.Codex_26.623.5546.0_x64__2p2nqsd0c76g0",
              "contentLength": 671037642,
              "lastModified": "Sat, 27 Jun 2026 05:28:48 GMT",
              "architectures": {
                "x64": {
                  "architecture": "x64",
                  "status": "downloadable",
                  "downloadable": true,
                  "version": "26.623.5546.0",
                  "appVersion": "26.623.42026",
                  "packageMoniker": "OpenAI.Codex_26.623.5546.0_x64__2p2nqsd0c76g0",
                  "contentLength": 671037642,
                  "lastModified": "Sat, 27 Jun 2026 05:28:48 GMT",
                  "etag": "\"x64\""
                }
              },
              "updateManifest": {
                "storeProductId": "9PLM9XGG6VKS",
                "packageIdentity": "OpenAI.Codex"
              }
            }
          }
        }"#;

        let release = parse_manifest_for_arch(json, Some("x64")).unwrap();

        assert_eq!(release.version, "26.623.42026");
        assert_eq!(release.package_version, "26.623.5546.0");
        assert_eq!(
            release.package_moniker,
            "OpenAI.Codex_26.623.5546.0_x64__2p2nqsd0c76g0"
        );
    }

    #[test]
    fn selects_requested_windows_architecture_from_manifest() {
        let json = r#"{
          "schemaVersion": 2,
          "sources": {
            "windows": {
              "productId": "9PLM9XGG6VKS",
              "version": "26.616.9593.0",
              "packageMoniker": "OpenAI.Codex_26.616.9593.0_x64__2p2nqsd0c76g0",
              "contentLength": 667793718,
              "lastModified": "Fri, 26 Jun 2026 04:00:00 GMT",
              "architectures": {
                "x64": {
                  "architecture": "x64",
                  "status": "downloadable",
                  "downloadable": true,
                  "version": "26.616.9593.0",
                  "packageMoniker": "OpenAI.Codex_26.616.9593.0_x64__2p2nqsd0c76g0",
                  "contentLength": 667793718,
                  "lastModified": "Fri, 26 Jun 2026 05:00:00 GMT",
                  "etag": "\"x64\""
                },
                "arm64": {
                  "architecture": "arm64",
                  "status": "downloadable",
                  "downloadable": true,
                  "version": "26.616.9593.0",
                  "packageMoniker": "OpenAI.Codex_26.616.9593.0_arm64__2p2nqsd0c76g0",
                  "contentLength": 667217153,
                  "lastModified": "Fri, 26 Jun 2026 05:10:43 GMT",
                  "etag": "\"arm64\""
                }
              },
              "updateManifest": {
                "storeProductId": "9PLM9XGG6VKS",
                "packageIdentity": "OpenAI.Codex"
              }
            }
          }
        }"#;

        let release = parse_manifest_for_arch(json, Some("arm64")).unwrap();

        assert_eq!(release.version, "26.616.9593.0");
        assert_eq!(release.package_version, "26.616.9593.0");
        assert_eq!(release.architecture.as_deref(), Some("arm64"));
        assert_eq!(
            release.package_moniker,
            "OpenAI.Codex_26.616.9593.0_arm64__2p2nqsd0c76g0"
        );
        assert_eq!(release.content_length, Some(667_217_153));
        assert_eq!(
            release.released_at.as_deref(),
            Some("Fri, 26 Jun 2026 05:10:43 GMT")
        );
        assert_eq!(release.etag.as_deref(), Some("\"arm64\""));
        assert_eq!(release.download_architecture.as_deref(), Some("arm64"));
    }

    #[test]
    fn rejects_requested_architecture_when_not_downloadable() {
        let json = r#"{
          "schemaVersion": 2,
          "sources": {
            "windows": {
              "version": "26.616.9593.0",
              "architectures": {
                "x64": {
                  "architecture": "x64",
                  "downloadable": true,
                  "version": "26.616.9593.0",
                  "packageMoniker": "OpenAI.Codex_26.616.9593.0_x64__2p2nqsd0c76g0",
                  "contentLength": 667793718
                },
                "arm64": {
                  "architecture": "arm64",
                  "downloadable": false,
                  "version": "26.616.9593.0",
                  "packageMoniker": "OpenAI.Codex_26.616.9593.0_arm64__2p2nqsd0c76g0",
                  "contentLength": 667217153
                }
              }
            }
          }
        }"#;

        let err = parse_manifest_for_arch(json, Some("arm64")).unwrap_err();

        assert!(err
            .to_string()
            .contains("Windows arm64 package is not available"));
    }

    #[test]
    fn selected_architecture_requires_its_own_package_moniker() {
        let json = r#"{
          "schemaVersion": 2,
          "sources": {
            "windows": {
              "version": "26.616.9593.0",
              "packageMoniker": "OpenAI.Codex_26.616.9593.0_x64__2p2nqsd0c76g0",
              "architectures": {
                "arm64": {
                  "architecture": "arm64",
                  "downloadable": true,
                  "version": "26.616.9593.0"
                }
              }
            }
          }
        }"#;

        let err = parse_manifest_for_arch(json, Some("arm64")).unwrap_err();

        assert!(err.to_string().contains("missing Windows packageMoniker"));
    }
}
