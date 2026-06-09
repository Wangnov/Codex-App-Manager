use std::path::Path;
#[cfg(windows)]
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::capability::WinCapabilityReport;
#[cfg(windows)]
use crate::capability::{CapabilityCheck, CapabilityState};
use crate::msix::parse_appx_manifest_xml;
use crate::process::hidden_command;
use crate::EngineError;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledWindowsCodex {
    pub path: String,
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

/// Filesystem mtime of `path` as Unix seconds, best-effort (None if unreadable).
fn path_mtime_secs(path: &str) -> Option<u64> {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs())
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
}

/// Post-install sanity check for a sideloaded MSIX. `Add-AppxPackage` returning
/// success only means the cmdlet did not throw — on a stripped Windows (no
/// Store / App Installer, missing framework packages) the package can register
/// yet fail to launch, which is exactly the failure users hit. We verify the
/// package is registered, its Status is Ok, the app entry (AUMID) resolves, and
/// every declared framework dependency is actually present; when any of these
/// fail the caller removes the package and falls back to the portable build.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MsixHealthReport {
    pub healthy: bool,
    /// Whether the health probe actually ran and the `healthy` verdict reflects
    /// real checks. `false` means the probe could not run and `healthy` is a
    /// conservative "keep the MSIX" default, not a clean bill of health that was
    /// observed. Callers/UI/notes use this to tell "verified healthy" apart from
    /// "kept because unverifiable".
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
    /// Human-facing reason when unhealthy; empty when healthy.
    pub reason: String,
}

pub fn fetch_text(url: &str) -> Result<String, EngineError> {
    let output = hidden_command("curl")
        .args(["-fsSL", "--connect-timeout", "20", url])
        .output()
        .map_err(|e| EngineError::Io(format!("spawn curl: {e}")))?;

    if !output.status.success() {
        return Err(EngineError::Io(format!(
            "curl failed for {url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    String::from_utf8(output.stdout).map_err(|e| EngineError::Io(e.to_string()))
}

pub fn detect_installed_codex(portable_root: &Path) -> Option<InstalledWindowsCodex> {
    detect_msix_install().or_else(|| detect_portable_install(portable_root))
}

#[cfg(windows)]
fn ps_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
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

#[cfg(windows)]
fn run_powershell_json(script: &str) -> Result<String, EngineError> {
    let output = hidden_command(powershell_exe())
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .output()
        .map_err(|e| EngineError::Capability(format!("spawn powershell: {e}")))?;
    if !output.status.success() {
        return Err(EngineError::Capability(format!(
            "powershell failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

#[cfg(windows)]
pub fn install_msix_sideload(path: &Path) -> Result<MsixSideloadReport, EngineError> {
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
    let json = run_powershell_json(&script)
        .map_err(|e| EngineError::Install(format!("run Add-AppxPackage: {e}")))?;
    serde_json::from_str(&json)
        .map_err(|e| EngineError::Install(format!("parse Add-AppxPackage result: {e}")))
}

#[cfg(not(windows))]
pub fn install_msix_sideload(_path: &Path) -> Result<MsixSideloadReport, EngineError> {
    Ok(MsixSideloadReport {
        success: false,
        message: "MSIX sideloading is only available on Windows".to_string(),
        installed: None,
        fallback_recommended: true,
        raw_error: None,
    })
}

#[cfg(windows)]
pub fn remove_msix_package() -> Result<MsixRemoveReport, EngineError> {
    let script = format!(
        r#"
$ErrorActionPreference = 'Stop'
try {{
  $packages = Get-AppxPackage -Name {name} -ErrorAction SilentlyContinue
  if (-not $packages) {{
    [pscustomobject]@{{
      success = $true
      message = 'MSIX package was not installed'
      rawError = $null
    }} | ConvertTo-Json -Compress
    exit 0
  }}
  foreach ($p in $packages) {{
    Remove-AppxPackage -Package $p.PackageFullName -ErrorAction Stop
  }}
  [pscustomobject]@{{
    success = $true
    message = 'Remove-AppxPackage succeeded'
    rawError = $null
  }} | ConvertTo-Json -Compress
}} catch {{
  [pscustomobject]@{{
    success = $false
    message = [string]$_.Exception.Message
    rawError = [string]$_
  }} | ConvertTo-Json -Compress
}}
"#,
        name = ps_quote(crate::OPENAI_PACKAGE_IDENTITY)
    );
    let json = run_powershell_json(&script)
        .map_err(|e| EngineError::Install(format!("run Remove-AppxPackage: {e}")))?;
    serde_json::from_str(&json)
        .map_err(|e| EngineError::Install(format!("parse Remove-AppxPackage result: {e}")))
}

#[cfg(not(windows))]
pub fn remove_msix_package() -> Result<MsixRemoveReport, EngineError> {
    Ok(MsixRemoveReport {
        success: false,
        message: "MSIX removal is only available on Windows".to_string(),
        raw_error: None,
    })
}

#[cfg(windows)]
pub fn verify_msix_health() -> MsixHealthReport {
    let script = format!(
        r#"
$ErrorActionPreference = 'SilentlyContinue'
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
  }} | ConvertTo-Json -Compress
  exit 0
}}
$statusStr = [string]$pkg.Status
$statusOk = ([string]::IsNullOrEmpty($statusStr) -or $statusStr -eq 'Ok')
$aumidResolved = $false
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
[pscustomobject]@{{
  packageRegistered = $true
  statusOk = $statusOk
  status = $statusStr
  aumidResolved = $aumidResolved
  missingDependencies = ($missing -join ', ')
}} | ConvertTo-Json -Compress
"#,
        name = ps_quote(crate::OPENAI_PACKAGE_IDENTITY)
    );

    let parsed = run_powershell_json(&script)
        .ok()
        .and_then(|json| serde_json::from_str::<serde_json::Value>(&json).ok());

    let Some(value) = parsed else {
        // The health probe itself could not run. Don't overturn a successful
        // Add-AppxPackage on an unverifiable signal — keep the MSIX install.
        // The keep-MSIX decision (healthy = true) is intentional, but mark the
        // report unverified so callers don't mistake it for an observed clean
        // bill of health.
        return MsixHealthReport {
            healthy: true,
            verified: false,
            package_registered: true,
            status: "probe-failed".to_string(),
            status_ok: true,
            aumid_resolved: true,
            missing_dependencies: vec![],
            reason: "health probe could not run; keeping the MSIX install".to_string(),
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

    let healthy =
        package_registered && status_ok && aumid_resolved && missing_dependencies.is_empty();
    let reason = if healthy {
        String::new()
    } else if !package_registered {
        "the package is not registered after install".to_string()
    } else if !status_ok {
        format!("package status is {status}")
    } else if !aumid_resolved {
        "could not resolve the app entry (AUMID)".to_string()
    } else {
        format!(
            "missing framework dependencies: {}",
            missing_dependencies.join(", ")
        )
    };

    MsixHealthReport {
        healthy,
        // The probe ran and produced a real verdict.
        verified: true,
        package_registered,
        status,
        status_ok,
        aumid_resolved,
        missing_dependencies,
        reason,
    }
}

#[cfg(not(windows))]
pub fn verify_msix_health() -> MsixHealthReport {
    // Non-Windows builds never sideload, so there is nothing to verify; report
    // healthy so this can never be the thing that blocks a (non-existent) path,
    // but leave verified = false since no real check was performed.
    MsixHealthReport {
        healthy: true,
        verified: false,
        package_registered: false,
        status: "not-windows".to_string(),
        status_ok: true,
        aumid_resolved: true,
        missing_dependencies: vec![],
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
    Some(codex)
}

#[cfg(not(windows))]
fn detect_msix_install() -> Option<InstalledWindowsCodex> {
    None
}

pub fn detect_portable_install(portable_root: &Path) -> Option<InstalledWindowsCodex> {
    let exe = portable_root.join("Codex.exe");
    if !exe.exists() {
        return None;
    }
    let identity = std::fs::read_to_string(portable_root.join("AppxManifest.xml"))
        .ok()
        .and_then(|xml| parse_appx_manifest_xml(&xml).ok());

    Some(InstalledWindowsCodex {
        path: portable_root.to_string_lossy().into_owned(),
        version: identity
            .as_ref()
            .map(|i| i.version.clone())
            .unwrap_or_else(|| "0.0.0.0".to_string()),
        arch: identity.as_ref().map(|i| i.processor_architecture.clone()),
        source: "portable".to_string(),
        package_family_name: None,
        installed_at: path_mtime_secs(&exe.to_string_lossy()),
    })
}

/// Open the installed Codex: run the portable `Codex.exe`, or hand the MSIX
/// app's resolved AUMID to the Windows shell.
pub fn launch_codex(installed: &InstalledWindowsCodex) -> Result<(), EngineError> {
    if installed.source == "portable" {
        let exe = Path::new(&installed.path).join("Codex.exe");
        // CREATE_NO_WINDOW only suppresses a console flash; the GUI still shows.
        hidden_command(exe)
            .spawn()
            .map(|_| ())
            .map_err(|e| EngineError::Io(format!("launch Codex: {e}")))
    } else {
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
    run_powershell_json(&script).map(|_| ())
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
}
