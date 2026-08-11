//! Historical/offline Codex package catalog backed by GitHub Releases.
//!
//! The frontend never supplies a download URL. It selects a release tag and an
//! exact asset name; the command layer re-fetches that release and resolves the
//! URL + digest again before any bytes are trusted. Local packages deliberately
//! avoid the network and instead use the native OpenAI vendor-signature and
//! package-identity gates in the platform installers.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::errors::AppError;

const REPOSITORY: &str = "Wangnov/codex-app-mirror";
const RELEASES_API: &str =
    "https://api.github.com/repos/Wangnov/codex-app-mirror/releases?per_page=100";
const MAX_RELEASE_PAGES: usize = 50;
const RELEASE_TAG_API_PREFIX: &str =
    "https://api.github.com/repos/Wangnov/codex-app-mirror/releases/tags/";
const RELEASE_TAG_PREFIX: &str = "codex-app-";
const WINDOWS_IDENTITY_SUFFIX: &str = "__2p2nqsd0c76g0.Msix";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleasePlatform {
    Macos,
    Windows,
}

impl ReleasePlatform {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Macos => "macos",
            Self::Windows => "windows",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleaseArchitecture {
    Arm64,
    X64,
}

impl ReleaseArchitecture {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Arm64 => "arm64",
            Self::X64 => "x64",
        }
    }

    pub fn from_runtime(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "arm64" | "aarch64" => Some(Self::Arm64),
            "x64" | "x86_64" | "amd64" => Some(Self::X64),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReleasePackageFormat {
    Dmg,
    Zip,
    Msix,
}

impl ReleasePackageFormat {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dmg => "dmg",
            Self::Zip => "zip",
            Self::Msix => "msix",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoricalReleaseCatalog {
    pub repository: String,
    pub platform: ReleasePlatform,
    pub architecture: ReleaseArchitecture,
    pub releases: Vec<HistoricalRelease>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoricalRelease {
    pub tag: String,
    pub version: String,
    pub published_at: Option<String>,
    pub assets: Vec<HistoricalAsset>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HistoricalAsset {
    pub name: String,
    pub size: u64,
    pub architecture: ReleaseArchitecture,
    pub format: ReleasePackageFormat,
    pub package_version: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalReleasePackage {
    pub path: String,
    pub file_name: String,
    pub size: u64,
    pub release_tag: String,
    pub version: String,
    pub asset_name: String,
    pub architecture: ReleaseArchitecture,
    pub format: ReleasePackageFormat,
    pub package_version: Option<String>,
}

/// Fully resolved server-side asset. The URL/digest are intentionally never
/// serialized to the frontend; only native install code receives them.
#[derive(Debug, Clone)]
pub struct ResolvedReleaseAsset {
    pub release_tag: String,
    pub version: String,
    pub published_at: Option<String>,
    pub name: String,
    pub url: String,
    pub size: u64,
    pub digest: String,
    pub architecture: ReleaseArchitecture,
    pub format: ReleasePackageFormat,
    pub package_version: Option<String>,
    pub checksums_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GithubRelease {
    tag_name: String,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    published_at: Option<String>,
    #[serde(default)]
    assets: Vec<GithubAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct GithubAsset {
    name: String,
    browser_download_url: String,
    size: u64,
    digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedAssetName {
    architecture: ReleaseArchitecture,
    format: ReleasePackageFormat,
    package_version: Option<String>,
}

type TextFetcher<'a> = dyn Fn(&str) -> Result<String, AppError> + 'a;

pub fn fetch_catalog(
    platform: ReleasePlatform,
    architecture: ReleaseArchitecture,
    fetch: &TextFetcher<'_>,
) -> Result<HistoricalReleaseCatalog, AppError> {
    let releases = fetch_all_releases(fetch)?;
    catalog_from_releases(releases, platform, architecture)
}

pub fn resolve_release_asset(
    release_tag: &str,
    asset_name: &str,
    platform: ReleasePlatform,
    architecture: ReleaseArchitecture,
    fetch: &TextFetcher<'_>,
) -> Result<ResolvedReleaseAsset, AppError> {
    let version = version_from_tag(release_tag)?;
    validate_asset_component(asset_name)?;
    let url = format!("{RELEASE_TAG_API_PREFIX}{release_tag}");
    let text = fetch(&url)?;
    let release: GithubRelease = serde_json::from_str(&text)
        .map_err(|e| AppError::Engine(format!("解析 GitHub Release 响应失败: {e}")))?;
    if release.draft || release.prerelease || release.tag_name != release_tag {
        return Err(AppError::StaleExpectation(
            "所选 GitHub Release 已变化或不再可安装，请刷新版本列表".to_string(),
        ));
    }
    let asset = release
        .assets
        .iter()
        .find(|asset| asset.name == asset_name)
        .ok_or_else(|| {
            AppError::StaleExpectation(
                "所选安装包已不在该 GitHub Release 中，请刷新版本列表".to_string(),
            )
        })?;
    let parsed = parse_asset_name(&asset.name, &version).ok_or_else(|| {
        AppError::Engine("所选 Release 资产不是受支持的 Codex 安装包".to_string())
    })?;
    if parsed.architecture != architecture || platform_for_format(parsed.format) != platform {
        return Err(AppError::StaleExpectation(
            "所选安装包与当前平台或架构不匹配，请重新选择".to_string(),
        ));
    }
    let digest = asset_digest(asset)?;
    validate_download_url(&asset.browser_download_url, release_tag, asset_name)?;
    let checksums_name = match platform {
        ReleasePlatform::Macos => "SHA256SUMS-macos.txt",
        ReleasePlatform::Windows => "SHA256SUMS-windows.txt",
    };
    let checksums = release
        .assets
        .iter()
        .find(|asset| asset.name == checksums_name)
        .or_else(|| {
            release
                .assets
                .iter()
                .find(|asset| asset.name == "SHA256SUMS.txt")
        });
    if let Some(checksums) = checksums {
        validate_download_url(
            &checksums.browser_download_url,
            release_tag,
            &checksums.name,
        )?;
    }

    Ok(ResolvedReleaseAsset {
        release_tag: release_tag.to_string(),
        version,
        published_at: release.published_at,
        name: asset.name.clone(),
        url: asset.browser_download_url.clone(),
        size: asset.size,
        digest,
        architecture: parsed.architecture,
        format: parsed.format,
        package_version: parsed.package_version,
        checksums_url: checksums.map(|asset| asset.browser_download_url.clone()),
    })
}

pub fn verify_release_checksum(
    resolved: &ResolvedReleaseAsset,
    fetch: &TextFetcher<'_>,
) -> Result<(), AppError> {
    let Some(checksums_url) = resolved.checksums_url.as_deref() else {
        // Early releases predate SHA256SUMS. GitHub's immutable asset digest is
        // still enforced, followed by the native vendor-signature gate.
        return Ok(());
    };
    let text = fetch(checksums_url)?;
    let checksums = codex_win_engine::parse_checksums(&text)
        .map_err(|e| AppError::Engine(format!("解析 Release checksum 失败: {e}")))?;
    let checksum = checksums
        .iter()
        .find(|entry| entry.file_name == resolved.name)
        .ok_or_else(|| {
            AppError::Engine(format!(
                "Release checksum 中找不到 {}，拒绝安装",
                resolved.name
            ))
        })?;
    if !checksum.sha256.eq_ignore_ascii_case(&resolved.digest) {
        return Err(AppError::Engine(format!(
            "GitHub 资产 digest 与 Release checksum 不一致（{}），拒绝安装",
            resolved.name
        )));
    }
    Ok(())
}

pub fn verify_file_digest(path: &Path, expected: &str) -> Result<String, AppError> {
    let actual = codex_win_engine::sha256_file(path)
        .map_err(|e| AppError::Engine(format!("计算安装包 SHA-256 失败: {e}")))?;
    if !actual.eq_ignore_ascii_case(expected) {
        return Err(AppError::Engine(format!(
            "安装包 SHA-256 不匹配（实际 {}，期望 {}），拒绝安装",
            &actual[..12],
            &expected[..12]
        )));
    }
    Ok(actual)
}

/// Build the server-side descriptor for a local package. Native callers must
/// still enforce the platform trust chain before committing the installation.
/// Empty URLs are deliberate: callers with `local_path` never enter a network
/// download/checksum branch.
pub fn resolved_local_asset(
    path: &Path,
    version: &str,
    architecture: ReleaseArchitecture,
    format: ReleasePackageFormat,
    package_version: Option<String>,
) -> Result<ResolvedReleaseAsset, AppError> {
    version_from_tag(&format!("{RELEASE_TAG_PREFIX}{version}"))?;
    let metadata = std::fs::metadata(path)
        .map_err(|e| AppError::Internal(format!("读取本地安装包信息失败: {e}")))?;
    let size = metadata.len();
    if !metadata.is_file() || size == 0 || size > codex_win_engine::limits::MAX_PACKAGE_BYTES {
        return Err(AppError::Engine(format!(
            "本地安装包大小 {size} 超出允许范围"
        )));
    }
    let name = path
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::Internal("本地安装包文件名无效".to_string()))?
        .to_string();
    let digest = codex_win_engine::sha256_file(path)
        .map_err(|e| AppError::Engine(format!("计算本地安装包 SHA-256 失败: {e}")))?;
    Ok(ResolvedReleaseAsset {
        release_tag: format!("local-signed-{version}"),
        version: version.to_string(),
        published_at: None,
        name,
        url: String::new(),
        size,
        digest,
        architecture,
        format,
        package_version,
        checksums_url: None,
    })
}

/// Rebind the local macOS selection confirmed by the frontend without unpacking
/// a large DMG/ZIP for a second time. The installer hashes these bytes again and
/// performs the complete OpenAI signature, Gatekeeper/notarization, version,
/// architecture and minimum-OS gates on the extracted app before committing.
pub fn resolve_confirmed_macos_local_asset(
    path: &Path,
    release_tag: &str,
    asset_name: &str,
    architecture: ReleaseArchitecture,
) -> Result<ResolvedReleaseAsset, AppError> {
    let version = release_tag.strip_prefix("local-signed-").ok_or_else(|| {
        AppError::StaleExpectation("本地安装包确认信息无效，请重新选择并确认".to_string())
    })?;
    version_from_tag(&format!("{RELEASE_TAG_PREFIX}{version}"))?;
    let actual_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .ok_or_else(|| AppError::Internal("本地安装包文件名无效".to_string()))?;
    if actual_name != asset_name {
        return Err(AppError::StaleExpectation(
            "本地安装包在确认后发生变化，请重新选择并确认".to_string(),
        ));
    }
    let format = match path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("dmg") => ReleasePackageFormat::Dmg,
        Some("zip") => ReleasePackageFormat::Zip,
        _ => {
            return Err(AppError::Internal(
                "请选择从 GitHub Release 下载的 DMG 或 ZIP 文件".to_string(),
            ))
        }
    };
    let resolved = resolved_local_asset(path, version, architecture, format, None)?;
    if resolved.release_tag != release_tag || resolved.name != asset_name {
        return Err(AppError::StaleExpectation(
            "本地安装包在确认后发生变化，请重新选择并确认".to_string(),
        ));
    }
    Ok(resolved)
}

pub fn local_package_from_resolved(
    path: &Path,
    resolved: &ResolvedReleaseAsset,
) -> LocalReleasePackage {
    LocalReleasePackage {
        path: path.to_string_lossy().into_owned(),
        file_name: resolved.name.clone(),
        size: resolved.size,
        release_tag: resolved.release_tag.clone(),
        version: resolved.version.clone(),
        asset_name: resolved.name.clone(),
        architecture: resolved.architecture,
        format: resolved.format,
        package_version: resolved.package_version.clone(),
    }
}

fn catalog_from_releases(
    releases: Vec<GithubRelease>,
    platform: ReleasePlatform,
    architecture: ReleaseArchitecture,
) -> Result<HistoricalReleaseCatalog, AppError> {
    let mut items = Vec::new();
    for release in releases {
        if release.draft || release.prerelease {
            continue;
        }
        let Ok(version) = version_from_tag(&release.tag_name) else {
            continue;
        };
        let assets = release
            .assets
            .iter()
            .filter_map(|asset| {
                let parsed = parse_asset_name(&asset.name, &version)?;
                if parsed.architecture != architecture
                    || platform_for_format(parsed.format) != platform
                    || asset_digest(asset).is_err()
                    || validate_download_url(
                        &asset.browser_download_url,
                        &release.tag_name,
                        &asset.name,
                    )
                    .is_err()
                {
                    return None;
                }
                Some(HistoricalAsset {
                    name: asset.name.clone(),
                    size: asset.size,
                    architecture: parsed.architecture,
                    format: parsed.format,
                    package_version: parsed.package_version,
                })
            })
            .collect::<Vec<_>>();
        if !assets.is_empty() {
            items.push(HistoricalRelease {
                tag: release.tag_name,
                version,
                published_at: release.published_at,
                assets,
            });
        }
    }
    Ok(HistoricalReleaseCatalog {
        repository: REPOSITORY.to_string(),
        platform,
        architecture,
        releases: items,
    })
}

fn fetch_all_releases(fetch: &TextFetcher<'_>) -> Result<Vec<GithubRelease>, AppError> {
    let mut releases = Vec::new();
    for page in 1..=MAX_RELEASE_PAGES {
        let url = if page == 1 {
            RELEASES_API.to_string()
        } else {
            format!("{RELEASES_API}&page={page}")
        };
        let text = fetch(&url)?;
        let mut batch: Vec<GithubRelease> = serde_json::from_str(&text)
            .map_err(|e| AppError::Engine(format!("解析 GitHub Releases 响应失败: {e}")))?;
        let count = batch.len();
        releases.append(&mut batch);
        if count < 100 {
            return Ok(releases);
        }
    }
    Err(AppError::Engine(format!(
        "GitHub Releases 超过 {} 条安全分页上限，拒绝返回不完整版本列表",
        MAX_RELEASE_PAGES * 100
    )))
}

fn version_from_tag(tag: &str) -> Result<String, AppError> {
    let version = tag.strip_prefix(RELEASE_TAG_PREFIX).ok_or_else(|| {
        AppError::Engine("GitHub Release tag 不是受支持的 codex-app-* 格式".to_string())
    })?;
    if version.is_empty()
        || version.len() > 64
        || version.starts_with('.')
        || version.ends_with('.')
        || version.split('.').any(|part| {
            part.is_empty() || part.len() > 12 || !part.bytes().all(|byte| byte.is_ascii_digit())
        })
    {
        return Err(AppError::Engine(
            "GitHub Release tag 中的版本号无效".to_string(),
        ));
    }
    Ok(version.to_string())
}

fn validate_asset_component(name: &str) -> Result<(), AppError> {
    if name.is_empty()
        || name.len() > 200
        || name.contains('/')
        || name.contains('\\')
        || name.contains("..")
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
    {
        return Err(AppError::Engine(
            "GitHub Release 资产名称包含非法字符".to_string(),
        ));
    }
    Ok(())
}

fn asset_digest(asset: &GithubAsset) -> Result<String, AppError> {
    let digest = asset
        .digest
        .as_deref()
        .and_then(|value| value.strip_prefix("sha256:"))
        .ok_or_else(|| {
            AppError::Engine(format!("GitHub 资产 {} 缺少 SHA-256 digest", asset.name))
        })?;
    if digest.len() != 64 || !digest.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(AppError::Engine(format!(
            "GitHub 资产 {} 的 SHA-256 digest 无效",
            asset.name
        )));
    }
    Ok(digest.to_ascii_lowercase())
}

fn validate_download_url(url: &str, tag: &str, asset_name: &str) -> Result<(), AppError> {
    let parsed = url::Url::parse(url)
        .map_err(|e| AppError::Engine(format!("GitHub Release 资产 URL 无效: {e}")))?;
    let expected_path = format!("/{REPOSITORY}/releases/download/{tag}/{asset_name}");
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("github.com")
        || parsed.path() != expected_path
        || parsed.query().is_some()
        || parsed.fragment().is_some()
    {
        return Err(AppError::Engine(
            "GitHub Release 返回了非预期的资产 URL，拒绝安装".to_string(),
        ));
    }
    Ok(())
}

fn platform_for_format(format: ReleasePackageFormat) -> ReleasePlatform {
    match format {
        ReleasePackageFormat::Dmg | ReleasePackageFormat::Zip => ReleasePlatform::Macos,
        ReleasePackageFormat::Msix => ReleasePlatform::Windows,
    }
}

fn parse_asset_name(name: &str, version: &str) -> Option<ParsedAssetName> {
    let mac = [
        (
            "Codex-mac-arm64.dmg".to_string(),
            ReleaseArchitecture::Arm64,
            ReleasePackageFormat::Dmg,
        ),
        (
            "Codex-mac-x64.dmg".to_string(),
            ReleaseArchitecture::X64,
            ReleasePackageFormat::Dmg,
        ),
        (
            format!("Codex-darwin-arm64-{version}.zip"),
            ReleaseArchitecture::Arm64,
            ReleasePackageFormat::Zip,
        ),
        (
            format!("Codex-darwin-x64-{version}.zip"),
            ReleaseArchitecture::X64,
            ReleasePackageFormat::Zip,
        ),
    ];
    if let Some((_, architecture, format)) = mac.iter().find(|(expected, _, _)| expected == name) {
        return Some(ParsedAssetName {
            architecture: *architecture,
            format: *format,
            package_version: None,
        });
    }

    let stem = name.strip_suffix(WINDOWS_IDENTITY_SUFFIX)?;
    let rest = stem.strip_prefix("OpenAI.Codex_")?;
    let (package_version, arch) = rest.rsplit_once('_')?;
    let package_parts = package_version.split('.').collect::<Vec<_>>();
    if package_parts.len() != 4
        || package_parts.iter().any(|part| {
            part.is_empty()
                || !part.bytes().all(|byte| byte.is_ascii_digit())
                || part.parse::<u16>().is_err()
        })
    {
        return None;
    }
    let architecture = match arch {
        "arm64" => ReleaseArchitecture::Arm64,
        "x64" => ReleaseArchitecture::X64,
        _ => return None,
    };
    Some(ParsedAssetName {
        architecture,
        format: ReleasePackageFormat::Msix,
        package_version: Some(package_version.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(name: &str, digest: &str) -> GithubAsset {
        GithubAsset {
            name: name.to_string(),
            browser_download_url: format!(
                "https://github.com/{REPOSITORY}/releases/download/codex-app-26.727.51351/{name}"
            ),
            size: 42,
            digest: Some(format!("sha256:{digest}")),
        }
    }

    #[test]
    fn parses_supported_asset_names_and_rejects_cross_version_zip() {
        let mac = parse_asset_name("Codex-mac-arm64.dmg", "26.727.51351").unwrap();
        assert_eq!(mac.architecture, ReleaseArchitecture::Arm64);
        assert_eq!(mac.format, ReleasePackageFormat::Dmg);
        assert!(parse_asset_name("Codex-darwin-x64-26.727.51351.zip", "26.727.51351").is_some());
        assert!(parse_asset_name("Codex-darwin-x64-26.721.81911.zip", "26.727.51351").is_none());

        let win = parse_asset_name(
            "OpenAI.Codex_26.727.6591.0_x64__2p2nqsd0c76g0.Msix",
            "26.727.51351",
        )
        .unwrap();
        assert_eq!(win.architecture, ReleaseArchitecture::X64);
        assert_eq!(win.package_version.as_deref(), Some("26.727.6591.0"));
    }

    #[test]
    fn catalog_filters_platform_arch_and_unverified_assets() {
        let digest = "a".repeat(64);
        let release = GithubRelease {
            tag_name: "codex-app-26.727.51351".to_string(),
            draft: false,
            prerelease: false,
            published_at: Some("2026-08-01T00:17:13Z".to_string()),
            assets: vec![
                asset("Codex-mac-arm64.dmg", &digest),
                asset("Codex-mac-x64.dmg", &digest),
                asset("SHA256SUMS-macos.txt", &digest),
                GithubAsset {
                    digest: None,
                    ..asset("Codex-darwin-arm64-26.727.51351.zip", &digest)
                },
            ],
        };
        let catalog = catalog_from_releases(
            vec![release],
            ReleasePlatform::Macos,
            ReleaseArchitecture::Arm64,
        )
        .unwrap();
        assert_eq!(catalog.releases.len(), 1);
        assert_eq!(catalog.releases[0].assets.len(), 1);
        assert_eq!(catalog.releases[0].assets[0].name, "Codex-mac-arm64.dmg");
    }

    #[test]
    fn download_url_is_pinned_to_repository_tag_and_asset() {
        assert!(validate_download_url(
            "https://github.com/Wangnov/codex-app-mirror/releases/download/codex-app-26.727.51351/Codex-mac-arm64.dmg",
            "codex-app-26.727.51351",
            "Codex-mac-arm64.dmg"
        )
        .is_ok());
        assert!(validate_download_url(
            "https://evil.example/Codex-mac-arm64.dmg",
            "codex-app-26.727.51351",
            "Codex-mac-arm64.dmg"
        )
        .is_err());
    }

    #[test]
    fn early_release_without_checksums_uses_github_digest_then_native_gate() {
        let digest = "b".repeat(64);
        let response = serde_json::json!({
            "tag_name": "codex-app-26.513.20950",
            "draft": false,
            "prerelease": false,
            "published_at": "2026-05-13T00:00:00Z",
            "assets": [{
                "name": "Codex-mac-arm64.dmg",
                "browser_download_url": "https://github.com/Wangnov/codex-app-mirror/releases/download/codex-app-26.513.20950/Codex-mac-arm64.dmg",
                "size": 42,
                "digest": format!("sha256:{digest}")
            }]
        })
        .to_string();
        let resolved = resolve_release_asset(
            "codex-app-26.513.20950",
            "Codex-mac-arm64.dmg",
            ReleasePlatform::Macos,
            ReleaseArchitecture::Arm64,
            &|_| Ok(response.clone()),
        )
        .unwrap();
        assert!(resolved.checksums_url.is_none());
        assert!(
            verify_release_checksum(&resolved, &|_| { panic!("checksum fetch must not run") })
                .is_ok()
        );
    }

    #[test]
    fn local_signed_descriptor_has_no_network_surface() {
        let path = std::env::temp_dir().join(format!(
            "codex-local-release-{}-{}.dmg",
            std::process::id(),
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&path, b"signed-package-placeholder").unwrap();
        let resolved = resolved_local_asset(
            &path,
            "26.623.101652",
            ReleaseArchitecture::Arm64,
            ReleasePackageFormat::Dmg,
            None,
        )
        .unwrap();
        assert_eq!(resolved.release_tag, "local-signed-26.623.101652");
        assert!(resolved.url.is_empty());
        assert!(resolved.checksums_url.is_none());
        assert!(resolved_local_asset(
            &path,
            "not-a-version",
            ReleaseArchitecture::Arm64,
            ReleasePackageFormat::Dmg,
            None,
        )
        .is_err());

        let name = path.file_name().unwrap().to_str().unwrap();
        let rebound = resolve_confirmed_macos_local_asset(
            &path,
            "local-signed-26.623.101652",
            name,
            ReleaseArchitecture::Arm64,
        )
        .unwrap();
        assert_eq!(rebound.digest, resolved.digest);
        assert!(resolve_confirmed_macos_local_asset(
            &path,
            "local-signed-26.623.101652",
            "different.dmg",
            ReleaseArchitecture::Arm64,
        )
        .is_err());
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_path_like_asset_names_and_malformed_tags() {
        assert!(validate_asset_component("../Codex.dmg").is_err());
        assert!(version_from_tag("codex-app-26.727.51351").is_ok());
        assert!(version_from_tag("codex-app-26..1").is_err());
        assert!(version_from_tag("manager-v1").is_err());
    }
}
