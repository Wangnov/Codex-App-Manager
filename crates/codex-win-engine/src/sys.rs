use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::app_version::read_codex_app_version_from_install_root;
use crate::capability::WinCapabilityReport;
#[cfg(windows)]
use crate::capability::{CapabilityCheck, CapabilityState};
use crate::limits::MAX_TEXT_BYTES;
use crate::msix::parse_appx_manifest_xml;
use crate::network::{
    is_schannel_revocation_offline, safe_curl_failure_message, safe_url_host, NetworkConfig,
    SchannelRevocationCheck,
};
use crate::process::{
    curl_exe, hidden_command, run_capturing, spawn_and_require_liveness, LivenessResult, RunError,
    RunLimits, TimeoutKind, MSIX_ACTIVATION_WINDOW_SECS, MSIX_LIVENESS_WINDOW_SECS,
    PORTABLE_LIVENESS_WINDOW,
};
use crate::EngineError;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledWindowsCodex {
    pub path: String,
    /// Human-facing Codex app version read from app.asar/package.json when available.
    /// Falls back to the Windows package version if the app payload is unreadable.
    pub version: String,
    pub arch: Option<String>,
    /// "msix" | "portable"
    pub source: String,
    pub package_family_name: Option<String>,
    /// Install-dir / executable mtime as Unix seconds — when this build landed
    /// on disk. Surfaced as the "installed" date (Windows has no Sparkle feed).
    #[serde(default)]
    pub installed_at: Option<u64>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LaunchOptions {
    pub disable_codex_self_updates: bool,
}

const CODEX_SELF_UPDATE_ENV_KEY: &str = "CODEX_SPARKLE_ENABLED";
const CODEX_SELF_UPDATE_ENV_DISABLED: &str = "false";

/// Filesystem mtime of `path` as Unix seconds, best-effort (None if unreadable).
fn path_mtime_secs(path: &str) -> Option<u64> {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
}

fn prefer_codex_app_version(mut codex: InstalledWindowsCodex) -> InstalledWindowsCodex {
    if let Some(version) = read_codex_app_version_from_install_root(Path::new(&codex.path)) {
        codex.version = version;
    }
    codex
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MsixSideloadReport {
    pub success: bool,
    pub message: String,
    pub installed: Option<InstalledWindowsCodex>,
    pub fallback_recommended: bool,
    pub raw_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MsixRemoveReport {
    pub success: bool,
    pub message: String,
    pub raw_error: Option<String>,
    #[serde(default)]
    pub notes: Vec<String>,
}

/// Post-install sanity check for a sideloaded MSIX. `Add-AppxPackage` returning
/// success only means the cmdlet did not throw — on a stripped Windows (no
/// Store / App Installer, missing framework packages) the package can register
/// yet fail to launch, which is exactly the failure users hit. We verify the
/// package is registered, its Status is Ok, the app entry (AUMID) resolves,
/// every declared framework dependency is present, **and** a real shell
/// activation leaves a process under the install location. When any of these
/// fail the caller removes the package and falls back to the portable build.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MsixHealthReport {
    pub healthy: bool,
    /// Whether the health probe actually ran and the `healthy` verdict reflects
    /// real checks. `false` means the probe could not run (e.g. PowerShell
    /// missing) — not a clean bill of health that was observed.
    pub verified: bool,
    pub package_registered: bool,
    /// Raw `Get-AppxPackage` Status string (e.g. "Ok", "Modified").
    pub status: String,
    pub status_ok: bool,
    /// The app entry (AUMID) could be resolved from the package manifest.
    pub aumid_resolved: bool,
    /// Declared framework dependencies that are NOT installed on this machine —
    /// the usual reason an MSIX installs but won't launch on a stripped Windows.
    pub missing_dependencies: Vec<String>,
    /// Shell activation succeeded and a process under the package install
    /// location stayed alive for the liveness window.
    #[serde(default)]
    pub activation_ok: bool,
    /// Machine-stable failure class for UI / fallback routing. Empty when healthy.
    /// Values: `not-registered` | `status-bad` | `aumid-unresolved` |
    /// `missing-dependencies` | `activation-failed` | `immediate-exit` |
    /// `timeout` | `probe-failed` | `policy`.
    #[serde(default)]
    pub failure_kind: String,
    /// Human-facing reason when unhealthy; empty when healthy.
    pub reason: String,
}

/// Stable failure-kind strings shared with the frontend and notes.
pub mod msix_failure {
    pub const NOT_REGISTERED: &str = "not-registered";
    pub const STATUS_BAD: &str = "status-bad";
    pub const AUMID_UNRESOLVED: &str = "aumid-unresolved";
    pub const MISSING_DEPENDENCIES: &str = "missing-dependencies";
    pub const ACTIVATION_FAILED: &str = "activation-failed";
    pub const IMMEDIATE_EXIT: &str = "immediate-exit";
    pub const TIMEOUT: &str = "timeout";
    pub const PROBE_FAILED: &str = "probe-failed";
    pub const POLICY: &str = "policy";
}

/// Result of the framework-dependency PRE-check run BEFORE attempting an MSIX
/// sideload. On a stripped / China / Store-disabled Windows, `Add-AppxPackage`
/// cannot auto-acquire missing framework packages (VCLibs, WindowsAppRuntime,
/// UI.Xaml, NET.Native), so a sideload that needs an absent framework is doomed:
/// it either errors outright or registers a package that won't launch. When a
/// required framework is missing we proactively route to the portable build
/// instead of burning a failed install attempt.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MsixDependencyPrecheck {
    /// Whether the pre-check could actually evaluate framework presence. False
    /// when the manifest could not be read or the `Get-AppxPackage` probe could
    /// not run — in that case we do NOT block the sideload on an unknown signal.
    pub checked: bool,
    /// True when every required framework dependency is present (or none are
    /// declared). When false, `missing_frameworks` lists what is absent.
    pub frameworks_ok: bool,
    /// Framework packages the manifest requires that are NOT installed.
    pub missing_frameworks: Vec<String>,
    /// Human-facing reason; empty when `frameworks_ok` and there is nothing to say.
    pub reason: String,
}

impl MsixDependencyPrecheck {
    /// The pre-check has positively determined a required framework is missing,
    /// so the sideload should be skipped in favor of the portable build.
    pub fn should_route_portable(&self) -> bool {
        self.checked && !self.frameworks_ok && !self.missing_frameworks.is_empty()
    }
}

pub fn fetch_text(url: &str) -> Result<String, EngineError> {
    fetch_text_with_network(url, &NetworkConfig::system())
}

fn fetch_text_output(
    url: &str,
    network: &NetworkConfig,
    revocation_check: SchannelRevocationCheck,
) -> Result<std::process::Output, EngineError> {
    let max_text = MAX_TEXT_BYTES.to_string();
    let mut command = hidden_command(curl_exe());
    network.apply_to_command_with_schannel_revocation(&mut command, revocation_check);
    command.args([
        "-fsSL",
        "--proto",
        "=https",
        "--proto-redir",
        "=https",
        "--connect-timeout",
        "20",
        "--max-time",
        "60",
        "--max-filesize",
        &max_text,
        url,
    ]);
    // curl's own --max-time is the primary budget; the outer deadline is a
    // backstop that also kills a hung curl that ignored max-time.
    run_capturing(
        command,
        RunLimits::total(std::time::Duration::from_secs(75)),
        None,
    )
    .map_err(|e| EngineError::Io(format!("curl: {}", e.message())))
}

pub fn fetch_text_with_network(url: &str, network: &NetworkConfig) -> Result<String, EngineError> {
    let source = safe_url_host(url);
    log::debug!("fetch Windows text source={source}");
    let mut output = fetch_text_output(url, network, SchannelRevocationCheck::Strict)?;
    let should_retry_without_revocation = {
        let stderr = String::from_utf8_lossy(&output.stderr);
        !output.status.success()
            && is_schannel_revocation_offline(output.status.code(), stderr.as_ref())
    };
    if should_retry_without_revocation {
        log::warn!(
            "Windows curl Schannel revocation check failed; retrying with --ssl-no-revoke source={source}"
        );
        output = fetch_text_output(url, network, SchannelRevocationCheck::Disabled)?;
    }

    if !output.status.success() {
        let err = EngineError::Io(curl_failure_message(
            url,
            output.status.code(),
            &String::from_utf8_lossy(&output.stderr),
        ));
        log::warn!("fetch Windows text failed source={source} error={err}");
        return Err(err);
    }

    if output.stdout.len() > MAX_TEXT_BYTES as usize {
        let err = EngineError::Io(format!("text response exceeded {MAX_TEXT_BYTES} bytes"));
        log::warn!("fetch Windows text failed source={source} error={err}");
        return Err(err);
    }

    let text = String::from_utf8(output.stdout).map_err(|e| EngineError::Io(e.to_string()))?;
    let bytes = text.len();
    log::debug!("fetch Windows text completed source={source} bytes={bytes}");
    Ok(text)
}

fn curl_failure_message(url: &str, exit_code: Option<i32>, stderr: &str) -> String {
    let base = safe_curl_failure_message(url, exit_code, stderr);
    // Append the proxy diagnostic only for connectivity failures — pasting it
    // onto write / disk / HTTP errors (e.g. exit 23) only misleads.
    if is_connectivity_exit(exit_code) {
        format!("{base}; {}", proxy_env_summary())
    } else {
        base
    }
}

fn is_connectivity_exit(exit_code: Option<i32>) -> bool {
    matches!(
        exit_code,
        Some(
            5 | 6
                | 7
                | 28
                | 35
                | 52
                | 53
                | 54
                | 55
                | 56
                | 58
                | 59
                | 60
                | 67
                | 77
                | 80
                | 82
                | 83
                | 91
        )
    )
}

fn proxy_env_summary() -> String {
    let vars = ["HTTPS_PROXY", "HTTP_PROXY", "ALL_PROXY", "NO_PROXY"];
    let configured = vars
        .iter()
        .filter(|name| std::env::var_os(name).is_some())
        .copied()
        .collect::<Vec<_>>();
    if configured.is_empty() {
        "no curl proxy environment variables are set; Windows system proxy/PAC may not be used automatically".to_string()
    } else {
        format!(
            "curl proxy environment variables set: {}",
            configured.join(", ")
        )
    }
}

#[cfg(test)]
mod curl_diagnostic_tests {
    use super::curl_failure_message;

    #[test]
    fn text_fetch_failure_never_persists_url_or_raw_stderr_secrets() {
        let raw = "https://basic-user:basic-pass@manifest.example/latest/manifest?token=presigned-secret#fragment-secret";
        let stderr = format!("curl: (35) TLS failed while requesting {raw}");
        let message = curl_failure_message(raw, Some(35), &stderr);

        assert!(message
            .starts_with("curl failed for host=manifest.example exit=35 reason='TLS failure'"));
        for secret in [
            "basic-user",
            "basic-pass",
            "/latest/manifest",
            "presigned-secret",
            "fragment-secret",
        ] {
            assert!(!message.contains(secret), "text error leaked {secret}");
        }
    }
}

pub fn detect_installed_codex(portable_root: &Path) -> Option<InstalledWindowsCodex> {
    detect_msix_install().or_else(|| detect_portable_install(portable_root))
}

#[cfg(windows)]
fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}

#[cfg(windows)]
fn ps_nullable(value: Option<&str>) -> String {
    value.map(ps_quote).unwrap_or_else(|| "$null".to_string())
}

#[cfg(windows)]
fn powershell_exe() -> PathBuf {
    std::env::var_os("WINDIR")
        .map(PathBuf::from)
        .map(|windir| {
            windir
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe")
        })
        .filter(|path| path.exists())
        .unwrap_or_else(|| PathBuf::from("powershell.exe"))
}

/// Typed PowerShell runner failure so callers can branch on timeout without
/// string-matching human messages.
#[cfg(windows)]
#[derive(Debug)]
enum PowerShellRunError {
    Timeout(TimeoutKind),
    Other(String),
}

#[cfg(windows)]
impl PowerShellRunError {
    fn message(&self) -> String {
        match self {
            Self::Timeout(TimeoutKind::Total) => {
                "powershell: process exceeded total deadline".to_string()
            }
            Self::Timeout(TimeoutKind::Stall) => {
                "powershell: process made no progress within stall timeout".to_string()
            }
            Self::Other(msg) => msg.clone(),
        }
    }

    fn into_capability(self) -> EngineError {
        EngineError::Capability(self.message())
    }

    fn into_install(self) -> EngineError {
        EngineError::Install(self.message())
    }
}

#[cfg(windows)]
fn run_powershell_json(script: &str) -> Result<String, EngineError> {
    run_powershell_json_with_limits(script, RunLimits::probe()).map_err(|e| e.into_capability())
}

#[cfg(windows)]
fn run_powershell_json_with_limits(
    script: &str,
    limits: RunLimits,
) -> Result<String, PowerShellRunError> {
    let mut command = hidden_command(powershell_exe());
    command.args(["-NoProfile", "-NonInteractive", "-Command", script]);
    let output = run_capturing(command, limits, None).map_err(|e| match e {
        RunError::Timeout { kind, .. } => PowerShellRunError::Timeout(kind),
        other => PowerShellRunError::Other(format!("powershell: {}", other.message())),
    })?;
    if !output.status.success() {
        return Err(PowerShellRunError::Other(format!(
            "powershell failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(windows)]
pub fn install_msix_sideload(path: &Path) -> Result<MsixSideloadReport, EngineError> {
    let path_display = path.display();
    log::info!("MSIX sideload start path={path_display}");
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
try {{
  $cmd = Get-Command Add-AppxPackage -ErrorAction Stop
  $args = @{{ ErrorAction = 'Stop' }}
  if ($cmd.Parameters.ContainsKey('LiteralPath')) {{
    $args['LiteralPath'] = {path}
  }} else {{
    $args['Path'] = {path}
  }}
  if ($cmd.Parameters.ContainsKey('ForceUpdateFromAnyVersion')) {{
    $args['ForceUpdateFromAnyVersion'] = $true
  }}
  Add-AppxPackage @args
  $p = Get-AppxPackage -Name {name} -ErrorAction SilentlyContinue |
    Sort-Object -Property Version -Descending |
    Select-Object -First 1
  [pscustomobject]@{{
    success = $true
    message = 'Add-AppxPackage succeeded'
    fallbackRecommended = $false
    rawError = $null
    installed = if ($null -ne $p) {{
      [pscustomobject]@{{
        path = [string]$p.InstallLocation
        version = [string]$p.Version
        arch = $null
        source = 'msix'
        packageFamilyName = [string]$p.PackageFamilyName
      }}
    }} else {{ $null }}
  }} | ConvertTo-Json -Compress -Depth 4
}} catch {{
  [pscustomobject]@{{
    success = $false
    message = [string]$_.Exception.Message
    fallbackRecommended = $true
    rawError = [string]$_
    installed = $null
  }} | ConvertTo-Json -Compress -Depth 4
}}
"#,
        path = ps_quote(&path.to_string_lossy()),
        name = ps_quote(crate::OPENAI_PACKAGE_IDENTITY)
    );
    let json = run_powershell_json_with_limits(&script, RunLimits::install())
        .map_err(|e| EngineError::Install(format!("run Add-AppxPackage: {}", e.message())))?;
    let mut report: MsixSideloadReport = serde_json::from_str(&json)
        .map_err(|e| EngineError::Install(format!("parse Add-AppxPackage result: {e}")))?;
    if let Some(installed) = report.installed.take() {
        report.installed = Some(prefer_codex_app_version(installed));
    }
    if report.success {
        let path_display = path.display();
        log::info!("MSIX sideload completed path={path_display}");
    } else {
        let path_display = path.display();
        let error = &report.message;
        log::error!("MSIX sideload failed path={path_display} error={error}");
    }
    Ok(report)
}

#[cfg(not(windows))]
pub fn install_msix_sideload(_path: &Path) -> Result<MsixSideloadReport, EngineError> {
    log::info!("MSIX sideload start path=unsupported-platform");
    log::error!("MSIX sideload failed error=unsupported-platform");
    Ok(MsixSideloadReport {
        success: false,
        message: "MSIX sideloading is only available on Windows".to_string(),
        installed: None,
        fallback_recommended: true,
        raw_error: None,
    })
}

/// PRE-check, before sideloading, that every redistributable framework the
/// staged MSIX declares as a `PackageDependency` is already installed. Reads the
/// staged manifest's `PackageDependency` entries (same source the post-install
/// `verify_msix_health` inspects), filters to the framework packages, and asks
/// `Get-AppxPackage` whether each is present. A missing framework means the
/// sideload cannot succeed on this machine (no Store / App Installer to acquire
/// it), so the caller routes straight to the portable build.
///
/// This is intentionally conservative: if the manifest cannot be read or the
/// PowerShell probe cannot run, it returns `checked = false` and does NOT block
/// the sideload — the existing post-install health check + portable fallback
/// remain the backstop.
#[cfg(windows)]
pub fn precheck_msix_dependencies(path: &Path) -> MsixDependencyPrecheck {
    let frameworks = match crate::msix::read_msix_dependencies(path) {
        Ok(deps) => crate::msix::framework_dependencies(&deps),
        Err(err) => {
            log::warn!("MSIX dependency precheck could not read manifest error={err}");
            return MsixDependencyPrecheck {
                checked: false,
                frameworks_ok: true,
                missing_frameworks: vec![],
                reason: format!("could not read staged MSIX dependencies: {err}"),
            };
        }
    };

    if frameworks.is_empty() {
        log::info!("MSIX dependency precheck completed missing=[]");
        return MsixDependencyPrecheck {
            checked: true,
            frameworks_ok: true,
            missing_frameworks: vec![],
            reason: String::new(),
        };
    }

    let package_architecture = crate::msix::read_msix_identity(path)
        .ok()
        .map(|identity| identity.processor_architecture);
    let deps_literal = frameworks
        .iter()
        .map(|dep| {
            format!(
                "[pscustomobject]@{{ Name = {name}; Publisher = {publisher}; MinVersion = {min_version}; ProcessorArchitecture = {arch} }}",
                name = ps_quote(&dep.name),
                publisher = ps_nullable(dep.publisher.as_deref()),
                min_version = ps_nullable(dep.min_version.as_deref()),
                arch = ps_nullable(dep.processor_architecture.as_deref()),
            )
        })
        .collect::<Vec<_>>()
        .join(", ");
    let package_arch_literal = ps_nullable(package_architecture.as_deref());
    let script = format!(
        r#"
	$ErrorActionPreference = 'SilentlyContinue'
	$deps = @({deps_literal})
	$mainArch = {package_arch_literal}
	$missing = @()
	function Convert-ToVersion($value) {{
	  try {{
	    $text = [string]$value
	    if ([string]::IsNullOrWhiteSpace($text)) {{ return $null }}
	    return [version]$text
	  }} catch {{
	    return $null
	  }}
	}}
	function Same-Publisher($package, [string]$publisher) {{
	  if ([string]::IsNullOrWhiteSpace($publisher)) {{ return $true }}
	  return [string]$package.Publisher -eq $publisher
	}}
	function Same-Architecture($package, [string]$required) {{
	  if ([string]::IsNullOrWhiteSpace($required) -or $required -eq 'neutral') {{ return $true }}
	  $arch = [string]$package.Architecture
	  return [string]::IsNullOrWhiteSpace($arch) -or $arch -eq 'Neutral' -or $arch -eq $required
	}}
	foreach ($d in $deps) {{
	  $name = [string]$d.Name
	  if ([string]::IsNullOrWhiteSpace($name)) {{ continue }}
	  $publisher = [string]$d.Publisher
	  $requiredArch = [string]$d.ProcessorArchitecture
	  if ([string]::IsNullOrWhiteSpace($requiredArch)) {{ $requiredArch = $mainArch }}
	  $candidates = @(Get-AppxPackage -Name $name -ErrorAction SilentlyContinue)
	  if ($candidates.Count -eq 0) {{
	    $missing += "$name not installed"
	    continue
	  }}
	  $publisherCandidates = @($candidates | Where-Object {{ Same-Publisher $_ $publisher }})
	  if ($candidates.Count -gt 0 -and $publisherCandidates.Count -eq 0) {{
	    $missing += "$name publisher $publisher not installed"
	    continue
	  }}
	  $archCandidates = @($publisherCandidates | Where-Object {{ Same-Architecture $_ $requiredArch }})
	  if ($publisherCandidates.Count -gt 0 -and $archCandidates.Count -eq 0) {{
	    $missing += "$name architecture $requiredArch not installed"
	    continue
	  }}
	  $depPkg = $archCandidates |
	    Sort-Object -Property @{{ Expression = {{ Convert-ToVersion $_.Version }}; Descending = $true }} |
	    Select-Object -First 1
	  if ($null -eq $depPkg) {{
	    $missing += "$name not installed"
	    continue
	  }}
	  $minText = [string]$d.MinVersion
	  $min = Convert-ToVersion $minText
	  $installedVersion = Convert-ToVersion $depPkg.Version
	  if ($null -ne $min -and $null -ne $installedVersion -and $installedVersion -lt $min) {{
	    $missing += "$name >= $minText required (installed $($depPkg.Version))"
	  }}
	}}
	[pscustomobject]@{{
	  missing = ($missing -join ', ')
	}} | ConvertTo-Json -Compress
	"#,
    );

    let parsed = run_powershell_json(&script)
        .ok()
        .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok());

    let Some(value) = parsed else {
        // Probe could not run — don't overturn the sideload on an unknown signal.
        return MsixDependencyPrecheck {
            checked: false,
            frameworks_ok: true,
            missing_frameworks: vec![],
            reason: "framework dependency probe could not run; proceeding with sideload"
                .to_string(),
        };
    };

    let missing_frameworks: Vec<String> = value
        .get("missing")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    let frameworks_ok = missing_frameworks.is_empty();
    if !frameworks_ok {
        let missing = missing_frameworks.join(",");
        log::warn!("MSIX dependency precheck missing frameworks missing={missing}");
    }
    let reason = if frameworks_ok {
        String::new()
    } else {
        format!(
            "required framework packages are not installed: {}",
            missing_frameworks.join(", ")
        )
    };

    MsixDependencyPrecheck {
        checked: true,
        frameworks_ok,
        missing_frameworks,
        reason,
    }
}

#[cfg(not(windows))]
pub fn precheck_msix_dependencies(_path: &Path) -> MsixDependencyPrecheck {
    // Non-Windows builds never sideload, so there is nothing to pre-check. Report
    // checked = false / frameworks_ok = true so this can never block a path that
    // does not exist off Windows.
    MsixDependencyPrecheck {
        checked: false,
        frameworks_ok: true,
        missing_frameworks: vec![],
        reason: "MSIX dependency pre-checks are only meaningful on Windows".to_string(),
    }
}

#[cfg(windows)]
pub fn remove_msix_package() -> Result<MsixRemoveReport, EngineError> {
    let package = crate::OPENAI_PACKAGE_IDENTITY;
    log::info!("remove MSIX package package={package}");
    let script = format!(
        r#"
	$ErrorActionPreference = 'Stop'
	$script:notes = @()
	function Add-ResidualNotes {{
	  try {{
	    $allUsers = @(Get-AppxPackage -AllUsers -Name {name} -ErrorAction SilentlyContinue)
	    if ($allUsers.Count -gt 0) {{
	      $script:notes += 'MSIX package still exists for another user or elevated context: ' + (($allUsers | ForEach-Object {{ $_.PackageFullName }}) -join ', ')
	    }}
	  }} catch {{
	    $script:notes += 'Could not query all-user MSIX registrations: ' + [string]$_.Exception.Message
	  }}
	  try {{
	    $provisioned = @(Get-AppxProvisionedPackage -Online -ErrorAction SilentlyContinue | Where-Object {{ $_.DisplayName -eq {name} }})
	    if ($provisioned.Count -gt 0) {{
	      $script:notes += 'Provisioned MSIX package remains and may be reinstalled for new users; remove it from an elevated shell with Remove-AppxProvisionedPackage if desired.'
	    }}
	  }} catch {{
	    $script:notes += 'Could not query provisioned MSIX packages: ' + [string]$_.Exception.Message
	  }}
	}}
	try {{
	  $packages = Get-AppxPackage -Name {name} -ErrorAction SilentlyContinue
	  if (-not $packages) {{
	    Add-ResidualNotes
	    [pscustomobject]@{{
	      success = $true
	      message = 'MSIX package was not installed'
	      rawError = $null
	      notes = $script:notes
	    }} | ConvertTo-Json -Compress
	    exit 0
	  }}
	  foreach ($p in $packages) {{
	    Remove-AppxPackage -Package $p.PackageFullName -ErrorAction Stop
	  }}
	  Add-ResidualNotes
	  [pscustomobject]@{{
	    success = $true
	    message = 'Remove-AppxPackage succeeded'
	    rawError = $null
	    notes = $script:notes
	  }} | ConvertTo-Json -Compress
	}} catch {{
	  [pscustomobject]@{{
	    success = $false
	    message = [string]$_.Exception.Message
	    rawError = [string]$_
	    notes = $script:notes
	  }} | ConvertTo-Json -Compress
	}}
"#,
        name = ps_quote(crate::OPENAI_PACKAGE_IDENTITY)
    );
    let json = run_powershell_json_with_limits(&script, RunLimits::install())
        .map_err(|e| EngineError::Install(format!("run Remove-AppxPackage: {}", e.message())))?;
    let report: MsixRemoveReport = serde_json::from_str(&json)
        .map_err(|e| EngineError::Install(format!("parse Remove-AppxPackage result: {e}")))?;
    if report.success {
        let package = crate::OPENAI_PACKAGE_IDENTITY;
        log::info!("remove MSIX package completed package={package}");
    } else {
        let package = crate::OPENAI_PACKAGE_IDENTITY;
        let error = &report.message;
        log::error!("remove MSIX package failed package={package} error={error}");
    }
    Ok(report)
}

#[cfg(not(windows))]
pub fn remove_msix_package() -> Result<MsixRemoveReport, EngineError> {
    let package = crate::OPENAI_PACKAGE_IDENTITY;
    log::info!("remove MSIX package package={package}");
    Ok(MsixRemoveReport {
        success: false,
        message: "MSIX removal is only available on Windows".to_string(),
        raw_error: None,
        notes: vec![],
    })
}

/// Best-effort: close Codex processes belonging to the registered OpenAI.Codex
/// MSIX package (by InstallLocation). Used after an activation probe that may
/// have started the app, and before portable fallback / Remove-AppxPackage.
/// Failures are logged and swallowed — cleanup must not block fallback.
#[cfg(windows)]
pub fn close_msix_codex_processes(timeout_secs: u64) -> Result<(), EngineError> {
    let script = format!(
        r#"
$ErrorActionPreference = 'SilentlyContinue'
$pkg = Get-AppxPackage -Name {name} |
  Sort-Object -Property Version -Descending |
  Select-Object -First 1
if ($null -eq $pkg) {{ 'no-package'; exit 0 }}
$loc = [string]$pkg.InstallLocation
if ([string]::IsNullOrWhiteSpace($loc)) {{ 'no-location'; exit 0 }}
try {{ $loc = [System.IO.Path]::GetFullPath($loc).TrimEnd('\') }} catch {{}}
Write-Output $loc
"#,
        name = ps_quote(crate::OPENAI_PACKAGE_IDENTITY)
    );
    let loc = match run_powershell_json_with_limits(&script, RunLimits::probe()) {
        Ok(s) => s.trim().to_string(),
        Err(err) => {
            log::warn!(
                "close MSIX Codex processes: could not resolve install location error={}",
                err.message()
            );
            return Ok(());
        }
    };
    if loc.is_empty() || loc == "no-package" || loc == "no-location" {
        return Ok(());
    }
    crate::portable::close_codex_gracefully_for_root(timeout_secs, Path::new(&loc))
}

#[cfg(not(windows))]
pub fn close_msix_codex_processes(_timeout_secs: u64) -> Result<(), EngineError> {
    Ok(())
}

#[cfg(windows)]
fn best_effort_close_msix_after_probe() {
    // Start-Process detaches from the PowerShell tree, so killing the probe on
    // timeout leaves Codex running. Always try to reap it before the caller
    // falls back / removes the package.
    if let Err(err) = close_msix_codex_processes(15) {
        log::warn!("best-effort MSIX process cleanup after probe failed error={err}");
    }
}

#[cfg(windows)]
pub fn verify_msix_health() -> MsixHealthReport {
    log::info!("MSIX health check start");
    // Activation is the expensive step — budget deps probe + cold-start window +
    // continuous liveness + cleanup + slack.
    let limits = RunLimits::total(std::time::Duration::from_secs(
        60 + MSIX_ACTIVATION_WINDOW_SECS + MSIX_LIVENESS_WINDOW_SECS + 30,
    ));
    let script = format!(
        r#"
$ErrorActionPreference = 'SilentlyContinue'
$activationWindowSecs = {activation_window}
$livenessWindowSecs = {liveness_window}
$pkg = Get-AppxPackage -Name {name} |
  Sort-Object -Property Version -Descending |
  Select-Object -First 1
if ($null -eq $pkg) {{
  [pscustomobject]@{{
    packageRegistered = $false
    statusOk = $false
    status = 'not-registered'
    aumidResolved = $false
    missingDependencies = ''
    activationOk = $false
    failureKind = 'not-registered'
    activationDetail = ''
  }} | ConvertTo-Json -Compress
  exit 0
}}
$statusStr = [string]$pkg.Status
$statusOk = ([string]::IsNullOrEmpty($statusStr) -or $statusStr -eq 'Ok')
$aumidResolved = $false
$missing = @()
$activationOk = $false
$failureKind = ''
$activationDetail = ''
$appId = ''
$installLoc = ''
$targetIds = @()
$activationAttempted = $false
function Convert-ToVersion($value) {{
  try {{
    $text = [string]$value
    if ([string]::IsNullOrWhiteSpace($text)) {{ return $null }}
    return [version]$text
  }} catch {{
    return $null
  }}
}}
function Same-Publisher($package, [string]$publisher) {{
  if ([string]::IsNullOrWhiteSpace($publisher)) {{ return $true }}
  return [string]$package.Publisher -eq $publisher
}}
function Same-Architecture($package, [string]$required) {{
  if ([string]::IsNullOrWhiteSpace($required) -or $required -eq 'neutral') {{ return $true }}
  $arch = [string]$package.Architecture
  return [string]::IsNullOrWhiteSpace($arch) -or $arch -eq 'Neutral' -or $arch -eq $required
}}
# AppX / protected processes often leave Get-Process.Path empty. Fall through
# MainModule and CIM so a live Codex is not treated as activation-failed.
function Get-ProcessExePath($p) {{
  try {{
    $path = [string]$p.Path
    if (-not [string]::IsNullOrWhiteSpace($path)) {{ return $path }}
  }} catch {{}}
  try {{
    $path = [string]$p.MainModule.FileName
    if (-not [string]::IsNullOrWhiteSpace($path)) {{ return $path }}
  }} catch {{}}
  try {{
    $cim = Get-CimInstance -ClassName Win32_Process -Filter ("ProcessId=" + $p.Id) -ErrorAction SilentlyContinue
    if ($null -ne $cim -and -not [string]::IsNullOrWhiteSpace([string]$cim.ExecutablePath)) {{
      return [string]$cim.ExecutablePath
    }}
  }} catch {{}}
  return $null
}}
function Test-UnderInstall($p, [string]$root) {{
  if ([string]::IsNullOrWhiteSpace($root)) {{ return $false }}
  $path = Get-ProcessExePath $p
  if ([string]::IsNullOrWhiteSpace($path)) {{ return $false }}
  try {{
    $full = [System.IO.Path]::GetFullPath($path)
    return ($full.Equals($root, [System.StringComparison]::OrdinalIgnoreCase) -or
            $full.StartsWith($root + '\', [System.StringComparison]::OrdinalIgnoreCase))
  }} catch {{
    return $false
  }}
}}
function Get-PackageProcesses([string]$root) {{
  $found = @()
  foreach ($p in @(Get-Process -Name Codex,ChatGPT -ErrorAction SilentlyContinue)) {{
    if (Test-UnderInstall $p $root) {{ $found += $p }}
  }}
  return $found
}}
function Stop-PackageProcesses([string]$root, $ids) {{
  foreach ($id in @($ids)) {{
    try {{ Stop-Process -Id $id -Force -ErrorAction SilentlyContinue }} catch {{}}
  }}
  foreach ($p in @(Get-PackageProcesses $root)) {{
    try {{ Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue }} catch {{}}
  }}
}}
try {{
  $manifest = Get-AppxPackageManifest $pkg -ErrorAction Stop
  $app = $manifest.Package.Applications.Application
  if ($app -is [array]) {{ $app = $app[0] }}
  $appId = [string]$app.Id
  if (-not [string]::IsNullOrEmpty($appId)) {{ $aumidResolved = $true }}
  $mainArch = [string]$pkg.Architecture
  $deps = $manifest.Package.Dependencies.PackageDependency
  foreach ($d in @($deps)) {{
    if ($null -ne $d) {{
      $dn = [string]$d.Name
      if (-not [string]::IsNullOrEmpty($dn)) {{
        $depPublisher = [string]$d.Publisher
        $depMinText = [string]$d.MinVersion
        $depMin = Convert-ToVersion $depMinText
        $depArch = [string]$d.ProcessorArchitecture
        if ([string]::IsNullOrWhiteSpace($depArch)) {{ $depArch = $mainArch }}
        $candidates = @(Get-AppxPackage -Name $dn -ErrorAction SilentlyContinue |
          Where-Object {{ Same-Publisher $_ $depPublisher }})
        $archCandidates = @($candidates | Where-Object {{ Same-Architecture $_ $depArch }})
        if ($candidates.Count -gt 0 -and $archCandidates.Count -eq 0) {{
          $missing += "$dn architecture $depArch not installed"
          continue
        }}
        $depPkg = $archCandidates |
          Sort-Object -Property @{{ Expression = {{ Convert-ToVersion $_.Version }}; Descending = $true }} |
          Select-Object -First 1
        if ($null -eq $depPkg) {{
          $missing += "$dn not installed"
        }} else {{
          $installedVersion = Convert-ToVersion $depPkg.Version
          if ($null -ne $depMin -and $null -ne $installedVersion -and $installedVersion -lt $depMin) {{
            $missing += "$dn >= $depMinText required (installed $($depPkg.Version))"
          }}
        }}
      }}
    }}
  }}
}} catch {{}}

# Real activation: shell-start the AUMID and require a process under InstallLocation
# for a continuous liveness window aligned with portable. Registration alone is
# not enough on stripped Windows.
if ($statusOk -and $aumidResolved -and $missing.Count -eq 0) {{
  if ([string]::IsNullOrEmpty($appId)) {{ $appId = 'App' }}
  $aumid = [string]$pkg.PackageFamilyName + '!' + $appId
  $installLoc = [string]$pkg.InstallLocation
  try {{ $installLoc = [System.IO.Path]::GetFullPath($installLoc).TrimEnd('\') }} catch {{}}
  $activationAttempted = $true
  try {{
    Start-Process ("shell:AppsFolder\" + $aumid) -ErrorAction Stop | Out-Null
    $deadline = (Get-Date).AddSeconds($activationWindowSecs)
    $sawProcess = $false
    $stillAlive = $false
    while ((Get-Date) -lt $deadline) {{
      Start-Sleep -Milliseconds 300
      $found = @(Get-PackageProcesses $installLoc)
      if ($found.Count -eq 0) {{ continue }}
      $sawProcess = $true
      $targetIds = @($found | ForEach-Object {{ $_.Id }} | Select-Object -Unique)
      # Continuous survival for $livenessWindowSecs (same bar as portable).
      $liveDeadline = (Get-Date).AddSeconds($livenessWindowSecs)
      $continuous = $true
      while ((Get-Date) -lt $liveDeadline) {{
        Start-Sleep -Milliseconds 250
        $alive = @()
        foreach ($id in $targetIds) {{
          $p = Get-Process -Id $id -ErrorAction SilentlyContinue
          if ($null -ne $p) {{ $alive += $p }}
        }}
        # Also accept replacements under install root (Electron restarts).
        if ($alive.Count -eq 0) {{
          $alive = @(Get-PackageProcesses $installLoc)
          $targetIds = @($alive | ForEach-Object {{ $_.Id }} | Select-Object -Unique)
        }}
        if ($alive.Count -eq 0) {{
          $continuous = $false
          break
        }}
      }}
      if ($continuous) {{
        $stillAlive = $true
        break
      }}
    }}
    if ($stillAlive) {{
      $activationOk = $true
    }} elseif ($sawProcess) {{
      $failureKind = 'immediate-exit'
      $activationDetail = 'package process started then exited during the liveness window'
    }} else {{
      $failureKind = 'activation-failed'
      $activationDetail = 'no process under install location after shell activation'
    }}
  }} catch {{
    $msg = [string]$_.Exception.Message
    $activationDetail = $msg
    if ($msg -match '0x80073|policy|denied|Access is denied|0x80070005|blocked') {{
      $failureKind = 'policy'
    }} else {{
      $failureKind = 'activation-failed'
    }}
  }}
  # Unhealthy activation must not leave Codex holding package files open for
  # the subsequent portable fallback / Remove-AppxPackage.
  if (-not $activationOk -and $activationAttempted) {{
    Stop-PackageProcesses $installLoc $targetIds
  }}
}}

[pscustomobject]@{{
  packageRegistered = $true
  statusOk = $statusOk
  status = $statusStr
  aumidResolved = $aumidResolved
  missingDependencies = ($missing -join ', ')
  activationOk = $activationOk
  failureKind = $failureKind
  activationDetail = $activationDetail
}} | ConvertTo-Json -Compress
"#,
        name = ps_quote(crate::OPENAI_PACKAGE_IDENTITY),
        activation_window = MSIX_ACTIVATION_WINDOW_SECS,
        liveness_window = MSIX_LIVENESS_WINDOW_SECS
    );

    let run_result = run_powershell_json_with_limits(&script, limits);
    let parsed = match &run_result {
        Ok(json) => serde_json::from_str::<serde_json::Value>(json).ok(),
        Err(PowerShellRunError::Timeout(kind)) => {
            log::info!("MSIX health check result healthy=false status=timeout kind={kind:?}");
            // Start-Process detaches; kill any process the timed-out probe left running.
            best_effort_close_msix_after_probe();
            return MsixHealthReport {
                healthy: false,
                verified: true,
                package_registered: true,
                status: "timeout".to_string(),
                status_ok: false,
                aumid_resolved: false,
                missing_dependencies: vec![],
                activation_ok: false,
                failure_kind: msix_failure::TIMEOUT.to_string(),
                reason: "health probe timed out; routed to portable fallback".to_string(),
            };
        }
        Err(_) => None,
    };

    let Some(value) = parsed else {
        // The health probe itself could not run. On managed/stripped Windows this
        // is exactly the situation where an MSIX can register but fail to launch,
        // so treat the verdict as degraded and let the caller fall back to the
        // portable build instead of silently keeping an unverifiable package.
        log::info!("MSIX health check result healthy=false status=probe-failed");
        best_effort_close_msix_after_probe();
        return MsixHealthReport {
            healthy: false,
            verified: false,
            package_registered: true,
            status: "probe-failed".to_string(),
            status_ok: false,
            aumid_resolved: false,
            missing_dependencies: vec![],
            activation_ok: false,
            failure_kind: msix_failure::PROBE_FAILED.to_string(),
            reason: "health probe could not run; routed to portable fallback".to_string(),
        };
    };

    let package_registered = value
        .get("packageRegistered")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let status_ok = value
        .get("statusOk")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let status = value
        .get("status")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let aumid_resolved = value
        .get("aumidResolved")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let missing_text = value
        .get("missingDependencies")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let missing_dependencies: Vec<String> = missing_text
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let activation_ok = value
        .get("activationOk")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let activation_detail = value
        .get("activationDetail")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    let reported_kind = value
        .get("failureKind")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let (healthy, failure_kind, reason) = if !package_registered {
        (
            false,
            msix_failure::NOT_REGISTERED.to_string(),
            "the package is not registered after install".to_string(),
        )
    } else if !status_ok {
        (
            false,
            msix_failure::STATUS_BAD.to_string(),
            format!("package status is {status}"),
        )
    } else if !aumid_resolved {
        (
            false,
            msix_failure::AUMID_UNRESOLVED.to_string(),
            "could not resolve the app entry (AUMID)".to_string(),
        )
    } else if !missing_dependencies.is_empty() {
        (
            false,
            msix_failure::MISSING_DEPENDENCIES.to_string(),
            format!(
                "missing framework dependencies: {}",
                missing_dependencies.join(", ")
            ),
        )
    } else if !activation_ok {
        let kind = if reported_kind.is_empty() {
            msix_failure::ACTIVATION_FAILED.to_string()
        } else {
            reported_kind
        };
        let reason = if activation_detail.is_empty() {
            "package failed real activation / liveness check".to_string()
        } else {
            format!("package activation failed: {activation_detail}")
        };
        (false, kind, reason)
    } else {
        (true, String::new(), String::new())
    };

    let report = MsixHealthReport {
        healthy,
        // The probe ran and produced a real verdict.
        verified: true,
        package_registered,
        status,
        status_ok,
        aumid_resolved,
        missing_dependencies,
        activation_ok,
        failure_kind,
        reason,
    };
    let healthy = report.healthy;
    let status = &report.status;
    let kind = &report.failure_kind;
    log::info!("MSIX health check result healthy={healthy} status={status} failure_kind={kind}");
    report
}

#[cfg(not(windows))]
pub fn verify_msix_health() -> MsixHealthReport {
    // Non-Windows builds never sideload, so there is nothing to verify; report
    // healthy so this can never be the thing that blocks a (non-existent) path,
    // but leave verified = false since no real check was performed.
    log::info!("MSIX health check start");
    log::info!("MSIX health check result healthy=true status=not-windows");
    MsixHealthReport {
        healthy: true,
        verified: false,
        package_registered: false,
        status: "not-windows".to_string(),
        status_ok: true,
        aumid_resolved: true,
        missing_dependencies: vec![],
        activation_ok: true,
        failure_kind: String::new(),
        reason: "MSIX health checks are only meaningful on Windows".to_string(),
    }
}

#[cfg(windows)]
fn detect_msix_install() -> Option<InstalledWindowsCodex> {
    let script = format!(
        r#"
$p = Get-AppxPackage -Name {name} -ErrorAction SilentlyContinue |
  Sort-Object -Property Version -Descending |
  Select-Object -First 1
if ($null -ne $p) {{
  [pscustomobject]@{{
    path = [string]$p.InstallLocation
    version = [string]$p.Version
    arch = $null
    source = 'msix'
    packageFamilyName = [string]$p.PackageFamilyName
  }} | ConvertTo-Json -Compress
}}
"#,
        name = ps_quote(crate::OPENAI_PACKAGE_IDENTITY)
    );
    let json = run_powershell_json(&script).ok()?;
    if json.trim().is_empty() {
        return None;
    }
    let mut codex: InstalledWindowsCodex = serde_json::from_str(&json).ok()?;
    codex.installed_at = path_mtime_secs(&codex.path);
    Some(prefer_codex_app_version(codex))
}

#[cfg(not(windows))]
fn detect_msix_install() -> Option<InstalledWindowsCodex> {
    None
}

pub fn detect_portable_install(portable_root: &Path) -> Option<InstalledWindowsCodex> {
    // Entry-exe aware (manifest-declared, ChatGPT.exe/Codex.exe fallback) so
    // both pre- and post-rebrand portable payloads are recognized.
    let exe = crate::portable::installed_app_exe(portable_root)?;
    // Identity gate: an entry executable alone is not product identity — a
    // ChatGPT Classic payload also ships a root-level ChatGPT.exe.
    //   - With a manifest present (our installer always writes one), it must
    //     declare exactly the Codex package identity; a foreign or unparseable
    //     manifest is never a Codex install.
    //   - Without a manifest (a user-unpacked `app/` dir or a legacy adopted
    //     root), fall back to the payload's package-level identity: the
    //     `name` in app.asar's package.json. MSIX-internal executables carry
    //     no embedded Authenticode (integrity lives in the package-level
    //     AppxSignature.p7x, which does not survive unpacking), so signature
    //     checks cannot recognize these roots — the asar marker can.
    let identity = match std::fs::read_to_string(portable_root.join("AppxManifest.xml")) {
        Ok(xml) => match parse_appx_manifest_xml(&xml) {
            Ok(identity) if identity.name == crate::OPENAI_PACKAGE_IDENTITY => Some(identity),
            Ok(identity) => {
                log::debug!(
                    "portable root at {} declares identity {} (expected {}); not a Codex install",
                    portable_root.display(),
                    identity.name,
                    crate::OPENAI_PACKAGE_IDENTITY
                );
                return None;
            }
            Err(err) => {
                log::debug!(
                    "portable root at {} has an unparseable AppxManifest.xml ({err}); not a Codex install",
                    portable_root.display()
                );
                return None;
            }
        },
        Err(_) => {
            let asar_name =
                crate::app_version::read_asar_package_name_from_install_root(portable_root);
            if asar_name.as_deref() != Some(crate::app_version::CODEX_ASAR_PACKAGE_NAME) {
                log::debug!(
                    "portable root at {} has no manifest and its app payload name is {:?} (expected {}); not a Codex install",
                    portable_root.display(),
                    asar_name,
                    crate::app_version::CODEX_ASAR_PACKAGE_NAME
                );
                return None;
            }
            None
        }
    };

    let installed = InstalledWindowsCodex {
        path: portable_root.to_string_lossy().into_owned(),
        version: identity
            .as_ref()
            .map(|identity| identity.version.clone())
            .unwrap_or_else(|| "0.0.0.0".to_string()),
        arch: identity
            .as_ref()
            .map(|identity| identity.processor_architecture.clone()),
        source: "portable".to_string(),
        package_family_name: None,
        installed_at: path_mtime_secs(&exe.to_string_lossy()),
    };
    let installed = prefer_codex_app_version(installed);
    let path = &installed.path;
    log::debug!("detected portable install path={path}");
    Some(installed)
}

/// Open the installed Codex: run the portable `Codex.exe`, or hand the MSIX
/// app's resolved AUMID to the Windows shell.
pub fn launch_codex(installed: &InstalledWindowsCodex) -> Result<(), EngineError> {
    launch_codex_with_options(installed, LaunchOptions::default())
}

pub fn launch_codex_with_options(
    installed: &InstalledWindowsCodex,
    options: LaunchOptions,
) -> Result<(), EngineError> {
    if installed.source == "portable" {
        let root = Path::new(&installed.path);
        let exe = crate::portable::installed_app_exe(root).ok_or_else(|| {
            EngineError::Io(format!(
                "no app entry executable (ChatGPT.exe / Codex.exe) in {}",
                root.display()
            ))
        })?;
        // CREATE_NO_WINDOW only suppresses a console flash; the GUI still shows.
        // Require a short liveness window so an immediate crash is reported as a
        // launch failure instead of a silent no-op.
        let mut command = hidden_command(exe);
        if options.disable_codex_self_updates {
            command.env(CODEX_SELF_UPDATE_ENV_KEY, CODEX_SELF_UPDATE_ENV_DISABLED);
        }
        match spawn_and_require_liveness(command, PORTABLE_LIVENESS_WINDOW) {
            Ok(LivenessResult::Survived { child }) => {
                std::mem::forget(child);
                Ok(())
            }
            Ok(LivenessResult::ExitedEarly { code }) => Err(EngineError::Io(format!(
                "Codex exited immediately after launch (exit={})",
                code.map(|c| c.to_string())
                    .unwrap_or_else(|| "signal".to_string())
            ))),
            Err(err) => Err(EngineError::Io(format!("launch Codex: {}", err.message()))),
        }
    } else {
        if options.disable_codex_self_updates {
            log::debug!(
                "launching MSIX Codex with updater disabled via persisted user environment"
            );
        }
        launch_msix_app()
    }
}

#[cfg(windows)]
fn launch_msix_app() -> Result<(), EngineError> {
    // Resolve the real AUMID (PackageFamilyName!AppId) from the manifest so the
    // shell can activate the package; fall back to the conventional "App" id.
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
$pkg = Get-AppxPackage -Name {name} | Sort-Object -Property Version -Descending | Select-Object -First 1
if ($null -eq $pkg) {{ throw 'Codex is not installed' }}
$app = (Get-AppxPackageManifest $pkg).Package.Applications.Application
if ($app -is [array]) {{ $app = $app[0] }}
$id = $app.Id
if (-not $id) {{ $id = 'App' }}
Start-Process ("shell:AppsFolder\" + $pkg.PackageFamilyName + "!" + $id)
"#,
        name = ps_quote(crate::OPENAI_PACKAGE_IDENTITY)
    );
    // Activation is fire-and-forget from the shell; bound the AUMID resolve + Start-Process.
    run_powershell_json_with_limits(&script, RunLimits::probe())
        .map(|_| ())
        .map_err(|e| e.into_install())
}

#[cfg(not(windows))]
fn launch_msix_app() -> Result<(), EngineError> {
    Err(EngineError::Io(
        "MSIX launch is only available on Windows".to_string(),
    ))
}

#[cfg(windows)]
pub fn probe_capabilities() -> WinCapabilityReport {
    let script = r#"
$ErrorActionPreference = 'SilentlyContinue'
$add = Get-Command Add-AppxPackage -ErrorAction SilentlyContinue
$svc = Get-Service AppXSvc -ErrorAction SilentlyContinue
$appInstaller = Get-AppxPackage -Name Microsoft.DesktopAppInstaller -ErrorAction SilentlyContinue |
  Select-Object -First 1

$policy = $null
$policySource = ''
foreach ($p in @(
  'HKLM:\SOFTWARE\Policies\Microsoft\Windows\Appx',
  'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock'
)) {
  try {
    $value = (Get-ItemProperty -Path $p -Name AllowAllTrustedApps -ErrorAction Stop).AllowAllTrustedApps
    $policy = [int]$value
    $policySource = $p
    break
  } catch {}
}

$meteredKnown = $false
$metered = $null
$costType = ''
try {
  $profile = [Windows.Networking.Connectivity.NetworkInformation,Windows.Networking.Connectivity,ContentType=WindowsRuntime]::GetInternetConnectionProfile()
  if ($null -ne $profile) {
    $cost = $profile.GetConnectionCost()
    $meteredKnown = $true
    $costType = [string]$cost.NetworkCostType
    $metered = [bool]($cost.NetworkCostType -ne 'Unrestricted' -or $cost.Roaming -or $cost.OverDataLimit)
  }
} catch {}

$msixDeploymentKnown = $false
$msixDeploymentOk = $false
$msixDeploymentError = ''
try {
  $pm = New-Object -TypeName Windows.Management.Deployment.PackageManager -ErrorAction Stop
  $msixDeploymentKnown = $true
  $msixDeploymentOk = ($null -ne $pm)
} catch {
  $msixDeploymentKnown = $true
  $msixDeploymentOk = $false
  $hr = 0
  try { $hr = [int]$_.Exception.HResult } catch {}
  if ($hr -ne 0) {
    $msixDeploymentError = ('{0} (HRESULT=0x{1:X8})' -f $_.Exception.Message, $hr)
  } else {
    $msixDeploymentError = [string]$_.Exception.Message
  }
}

[pscustomobject]@{
  addAppxPackage = [bool]$add
  appxSvcExists = [bool]$svc
  appxSvcStatus = if ($svc) { [string]$svc.Status } else { '' }
  appxSvcStartType = if ($svc) { [string]$svc.StartType } else { '' }
  appInstallerInstalled = [bool]$appInstaller
  appInstallerVersion = if ($appInstaller) { [string]$appInstaller.Version } else { '' }
  allowAllTrustedApps = $policy
  allowAllTrustedAppsSource = $policySource
  meteredKnown = $meteredKnown
  metered = $metered
  networkCostType = $costType
  msixDeploymentKnown = $msixDeploymentKnown
  msixDeploymentOk = $msixDeploymentOk
  msixDeploymentError = $msixDeploymentError
} | ConvertTo-Json -Compress
"#;

    match run_powershell_json(script)
        .ok()
        .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok())
    {
        Some(value) => capabilities_from_probe_json(&value),
        None => WinCapabilityReport::from_checks(
            CapabilityCheck::unknown("PowerShell capability probe failed"),
            CapabilityCheck::unknown("PowerShell capability probe failed"),
            CapabilityCheck::unknown("PowerShell capability probe failed"),
            CapabilityCheck::unknown("PowerShell capability probe failed"),
            CapabilityCheck::unknown("PowerShell capability probe failed"),
            CapabilityCheck::unknown("PowerShell capability probe failed"),
            vec!["Could not complete Windows capability probe.".to_string()],
        ),
    }
}

#[cfg(not(windows))]
pub fn probe_capabilities() -> WinCapabilityReport {
    WinCapabilityReport::unknown_for_non_windows()
}

#[cfg(windows)]
fn capabilities_from_probe_json(value: &serde_json::Value) -> WinCapabilityReport {
    let add = if value
        .get("addAppxPackage")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        CapabilityCheck::available("Add-AppxPackage command is present")
    } else {
        CapabilityCheck::unavailable("Add-AppxPackage command is not present")
    };

    let svc_exists = value
        .get("appxSvcExists")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let svc_status = value
        .get("appxSvcStatus")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let svc_start = value
        .get("appxSvcStartType")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let appx_service = if !svc_exists {
        CapabilityCheck::unavailable("AppXSvc service is missing")
    } else if svc_start.eq_ignore_ascii_case("Disabled") {
        CapabilityCheck::unavailable("AppXSvc service is disabled")
    } else {
        CapabilityCheck::available(format!("AppXSvc is {svc_status}, start type {svc_start}"))
    };

    let policy = value.get("allowAllTrustedApps").and_then(|v| v.as_i64());
    let policy_source = value
        .get("allowAllTrustedAppsSource")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let sideload_policy = match policy {
        Some(0) => {
            CapabilityCheck::unavailable(format!("AllowAllTrustedApps=0 at {policy_source}"))
        }
        Some(1) => CapabilityCheck::available(format!("AllowAllTrustedApps=1 at {policy_source}")),
        Some(other) => {
            CapabilityCheck::unknown(format!("AllowAllTrustedApps={other} at {policy_source}"))
        }
        None => CapabilityCheck::unknown("AllowAllTrustedApps is not explicitly set"),
    };

    let app_installer = if value
        .get("appInstallerInstalled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let version = value
            .get("appInstallerVersion")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        CapabilityCheck::available(format!("Desktop App Installer {version}"))
    } else {
        CapabilityCheck::unknown("Desktop App Installer package was not detected")
    };

    let metered_network = if !value
        .get("meteredKnown")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        CapabilityCheck::unknown("WinRT network cost could not be read")
    } else if value
        .get("metered")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        let cost = value
            .get("networkCostType")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        CapabilityCheck {
            state: CapabilityState::Unavailable,
            detail: format!("current connection is metered ({cost})"),
        }
    } else {
        let cost = value
            .get("networkCostType")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        CapabilityCheck::available(format!("current connection is not metered ({cost})"))
    };

    // The functional probe: did the WinRT PackageManager actually activate? Only
    // an explicit activation failure (known && !ok) flips this to Unavailable —
    // an unrun probe stays Unknown so a good machine is never steered to portable
    // on a signal we couldn't read. This is the check that catches 0x80040154.
    let msix_deployment = if !value
        .get("msixDeploymentKnown")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        CapabilityCheck::unknown("MSIX deployment (PackageManager) could not be probed")
    } else if value
        .get("msixDeploymentOk")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        CapabilityCheck::available("PackageManager (MSIX deployment runtime) activates")
    } else {
        let err = value
            .get("msixDeploymentError")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        CapabilityCheck::unavailable(format!(
            "PackageManager (MSIX deployment runtime) cannot activate: {err}"
        ))
    };

    WinCapabilityReport::from_checks(
        add,
        appx_service,
        sideload_policy,
        app_installer,
        msix_deployment,
        metered_network,
        vec!["Certificate trust is verified after the MSIX is staged.".to_string()],
    )
}

#[cfg(test)]
mod tests {
    #[cfg(windows)]
    use super::*;

    #[cfg(windows)]
    #[test]
    fn parses_probe_json_into_portable_recommendation_when_policy_blocks() {
        let value = serde_json::json!({
            "addAppxPackage": true,
            "appxSvcExists": true,
            "appxSvcStatus": "Running",
            "appxSvcStartType": "Manual",
            "appInstallerInstalled": true,
            "appInstallerVersion": "1.0.0.0",
            "allowAllTrustedApps": 0,
            "allowAllTrustedAppsSource": "HKLM:\\SOFTWARE\\Policies\\Microsoft\\Windows\\Appx",
            "meteredKnown": true,
            "metered": false,
            "networkCostType": "Unrestricted",
            "msixDeploymentKnown": true,
            "msixDeploymentOk": true,
            "msixDeploymentError": ""
        });
        let report = capabilities_from_probe_json(&value);
        assert_eq!(
            report.recommendation,
            crate::capability::SideloadRecommendation::PortableFallback
        );
    }

    #[cfg(windows)]
    #[test]
    fn parses_probe_json_into_portable_when_deployment_broken() {
        // Every existence check looks fine, but the functional PackageManager
        // probe failed (0x80040154). The recommendation must be portable and the
        // deployment check must read Unavailable — this is the issue #13 machine.
        let value = serde_json::json!({
            "addAppxPackage": true,
            "appxSvcExists": true,
            "appxSvcStatus": "Running",
            "appxSvcStartType": "Manual",
            "appInstallerInstalled": true,
            "appInstallerVersion": "1.0.0.0",
            "allowAllTrustedApps": 1,
            "allowAllTrustedAppsSource": "HKLM:\\SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\AppModelUnlock",
            "meteredKnown": true,
            "metered": false,
            "networkCostType": "Unrestricted",
            "msixDeploymentKnown": true,
            "msixDeploymentOk": false,
            "msixDeploymentError": "没有注册类 (HRESULT=0x80040154)"
        });
        let report = capabilities_from_probe_json(&value);
        assert_eq!(
            report.recommendation,
            crate::capability::SideloadRecommendation::PortableFallback
        );
        assert_eq!(
            report.msix_deployment.state,
            crate::capability::CapabilityState::Unavailable
        );
    }

    #[test]
    fn msix_health_failure_kinds_are_stable_strings() {
        // Keep these stable: frontend / notes may switch on them.
        assert_eq!(super::msix_failure::NOT_REGISTERED, "not-registered");
        assert_eq!(super::msix_failure::ACTIVATION_FAILED, "activation-failed");
        assert_eq!(super::msix_failure::IMMEDIATE_EXIT, "immediate-exit");
        assert_eq!(super::msix_failure::TIMEOUT, "timeout");
        assert_eq!(super::msix_failure::POLICY, "policy");
    }

    #[test]
    fn msix_liveness_window_matches_portable() {
        assert_eq!(
            crate::process::MSIX_LIVENESS_WINDOW_SECS,
            crate::process::PORTABLE_LIVENESS_WINDOW.as_secs()
        );
        assert!(crate::process::MSIX_ACTIVATION_WINDOW_SECS >= 20);
    }

    #[cfg(windows)]
    #[test]
    fn powershell_timeout_is_typed_not_string_matched() {
        let err = super::PowerShellRunError::Timeout(crate::process::TimeoutKind::Total);
        assert!(matches!(
            err,
            super::PowerShellRunError::Timeout(crate::process::TimeoutKind::Total)
        ));
        // Callers classify via the enum arm, not by scraping message text.
        let msg = err.message();
        assert!(msg.contains("deadline"));
    }

    #[test]
    fn msix_health_report_defaults_new_fields_on_deserialize() {
        // Older fixtures without activationOk / failureKind still parse.
        let report: super::MsixHealthReport = serde_json::from_value(serde_json::json!({
            "healthy": true,
            "verified": true,
            "packageRegistered": true,
            "status": "Ok",
            "statusOk": true,
            "aumidResolved": true,
            "missingDependencies": [],
            "reason": ""
        }))
        .unwrap();
        assert!(report.healthy);
        assert!(!report.activation_ok); // default false when absent
        assert!(report.failure_kind.is_empty());
    }

    // Pure routing logic for the framework pre-check — verified on every host so
    // the steer-to-portable decision is covered even though the probe that fills
    // the struct only runs on Windows.
    #[test]
    fn precheck_routes_portable_only_when_a_framework_is_positively_missing() {
        // Missing framework, positively determined -> route to portable.
        let missing = super::MsixDependencyPrecheck {
            checked: true,
            frameworks_ok: false,
            missing_frameworks: vec!["Microsoft.VCLibs.140.00".to_string()],
            reason: "required framework packages are not installed: Microsoft.VCLibs.140.00"
                .to_string(),
        };
        assert!(missing.should_route_portable());

        // All frameworks present -> proceed with the sideload.
        let ok = super::MsixDependencyPrecheck {
            checked: true,
            frameworks_ok: true,
            missing_frameworks: vec![],
            reason: String::new(),
        };
        assert!(!ok.should_route_portable());

        // Probe could not run (manifest unreadable / PowerShell failed) -> do NOT
        // block on an unknown signal; let the sideload + health check decide.
        let unknown = super::MsixDependencyPrecheck {
            checked: false,
            frameworks_ok: true,
            missing_frameworks: vec![],
            reason: "framework dependency probe could not run; proceeding with sideload"
                .to_string(),
        };
        assert!(!unknown.should_route_portable());

        // Defensive: even if flags say not-ok, an empty list must not route (we
        // only steer when we can name the missing framework).
        let inconsistent = super::MsixDependencyPrecheck {
            checked: true,
            frameworks_ok: false,
            missing_frameworks: vec![],
            reason: String::new(),
        };
        assert!(!inconsistent.should_route_portable());
    }
}

#[cfg(test)]
mod portable_identity_tests {
    use super::detect_portable_install;
    use std::path::Path;

    fn write_root(name: &str, exe: &str, manifest: Option<&str>) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("codex-sys-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        std::fs::write(root.join(exe), b"fake exe").unwrap();
        if let Some(identity_name) = manifest {
            std::fs::write(
                root.join("AppxManifest.xml"),
                format!(
                    r#"<Package xmlns="http://schemas.microsoft.com/appx/manifest/foundation/windows10">
  <Identity Name="{identity_name}" Publisher="CN=OpenAI OpCo, LLC" Version="26.707.3748.0" ProcessorArchitecture="x64" />
  <Applications><Application Id="App" Executable="app/{exe}" /></Applications>
</Package>"#
                ),
            )
            .unwrap();
        }
        root
    }

    fn cleanup(root: &Path) {
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn accepts_rebranded_portable_with_codex_identity() {
        let root = write_root("rebrand", "ChatGPT.exe", Some("OpenAI.Codex"));
        let installed = detect_portable_install(&root).expect("codex identity accepted");
        assert_eq!(installed.version, "26.707.3748.0");
        assert_eq!(installed.source, "portable");
        cleanup(&root);
    }

    #[test]
    fn rejects_portable_with_foreign_identity() {
        // Same file shape as an unpacked ChatGPT Classic: entry exe + manifest,
        // but the identity is not OpenAI.Codex — never a Codex install.
        let root = write_root("classic", "ChatGPT.exe", Some("OpenAI.ChatGPT"));
        assert!(detect_portable_install(&root).is_none());
        cleanup(&root);
    }

    #[test]
    fn rejects_portable_without_manifest_identity() {
        // Without a manifest the gate falls back to the app.asar package-name
        // marker; this root has no asar at all, so it is not recognized.
        let root = write_root("no-manifest", "Codex.exe", None);
        assert!(detect_portable_install(&root).is_none());
        cleanup(&root);
    }

    #[test]
    fn accepts_manifestless_portable_with_codex_asar_marker() {
        // A user-unpacked official `app/` dir: no package-root manifest, but
        // the payload's asar carries the Codex package name — the supported
        // self-extracted layout must stay detectable/adoptable.
        let root = write_root("selfextracted", "ChatGPT.exe", None);
        let resources = root.join("resources");
        std::fs::create_dir_all(&resources).unwrap();
        crate::app_version::write_test_asar(
            &resources.join("app.asar"),
            br#"{"version":"26.707.31428","name":"openai-codex-electron"}"#,
        );
        let installed = detect_portable_install(&root).expect("asar marker accepted");
        assert_eq!(installed.version, "26.707.31428");
        cleanup(&root);
    }

    #[test]
    fn rejects_manifestless_portable_with_foreign_asar_name() {
        // Same shape but a non-Codex Electron payload (e.g. an unpacked
        // Classic): the asar package name differs, so it is never adopted.
        let root = write_root("foreign-asar", "ChatGPT.exe", None);
        let resources = root.join("resources");
        std::fs::create_dir_all(&resources).unwrap();
        crate::app_version::write_test_asar(
            &resources.join("app.asar"),
            br#"{"version":"1.2026.160","name":"chatgpt-desktop"}"#,
        );
        assert!(detect_portable_install(&root).is_none());
        cleanup(&root);
    }
}
