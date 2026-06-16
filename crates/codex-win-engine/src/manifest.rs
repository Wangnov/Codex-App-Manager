use serde::{Deserialize, Serialize};

use crate::EngineError;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowsRelease {
    pub version: String,
    pub package_moniker: String,
    pub architecture: Option<String>,
    pub content_length: Option<u64>,
    pub etag: Option<String>,
    pub store_product_id: Option<String>,
    pub package_identity: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MirrorManifest {
    schema_version: u64,
    sources: Sources,
}

#[derive(Debug, Deserialize)]
struct Sources {
    windows: WindowsSource,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowsSource {
    version: String,
    package_moniker: String,
    architecture: Option<String>,
    content_length: Option<u64>,
    etag: Option<String>,
    product_id: Option<String>,
    update_manifest: Option<WindowsUpdateManifest>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct WindowsUpdateManifest {
    store_product_id: Option<String>,
    package_identity: Option<String>,
}

pub fn parse_manifest(text: &str) -> Result<WindowsRelease, EngineError> {
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

    let windows = manifest.sources.windows;
    let release = WindowsRelease {
        version: windows.version,
        package_moniker: windows.package_moniker,
        architecture: windows.architecture,
        content_length: windows.content_length,
        etag: windows.etag,
        store_product_id: windows
            .update_manifest
            .as_ref()
            .and_then(|m| m.store_product_id.clone())
            .or(windows.product_id),
        package_identity: windows.update_manifest.and_then(|m| m.package_identity),
    };
    let arch = release.architecture.as_deref().unwrap_or("unknown");
    log::debug!(
        "parse Windows manifest succeeded version={} package_moniker={} arch={arch}",
        release.version,
        release.package_moniker
    );
    Ok(release)
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
        assert_eq!(release.package_identity.as_deref(), Some("OpenAI.Codex"));
        assert_eq!(release.content_length, Some(566_504_666));
    }
}
