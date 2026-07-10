# Authenticode-sign Windows PE files when a code-signing certificate is available.
#
# Non-blocking milestone: if WINDOWS_CERTIFICATE (base64 PFX) is unset/empty,
# the script prints a clear skip message and exits 0. Wire secrets into the
# `release` environment, then set repo variable AUTHENTICODE_REQUIRED=true to
# make verify-windows-authenticode.ps1 enforce Valid signatures.
#
# Signing order for a full release (see docs/windows-signing.md):
#   1. Prefer signing during `tauri build` via certificateThumbprint once the
#      cert is imported (covers main binary + uninstaller + installer).
#   2. This script is the post-build fallback for final published artifacts
#      (primarily the NSIS -setup.exe) and for local/CI verification of the path.
#
# Usage:
#   $env:WINDOWS_CERTIFICATE = "<base64 pfx>"
#   $env:WINDOWS_CERTIFICATE_PASSWORD = "..."
#   pwsh scripts/sign-windows-authenticode.ps1 -Path path\to\setup.exe
#
# Does NOT produce Tauri updater .sig files — use `tauri signer sign` for that.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string[]]$Path,

    [string]$CertificateBase64 = $env:WINDOWS_CERTIFICATE,
    [string]$CertificatePassword = $env:WINDOWS_CERTIFICATE_PASSWORD,
    [string]$TimestampUrl = $(if ($env:WINDOWS_TIMESTAMP_URL) { $env:WINDOWS_TIMESTAMP_URL } else { "http://timestamp.digicert.com" }),
    [string]$Stage = "sign"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Write-Stage([string]$Message) {
    Write-Host "::group::[$Stage] $Message"
}

function Close-Stage {
    Write-Host "::endgroup::"
}

function Fail-Stage([string]$Message) {
    Write-Host "::error::[$Stage] $Message"
    throw "[$Stage] $Message"
}

if ([string]::IsNullOrWhiteSpace($CertificateBase64)) {
    Write-Host "[$Stage] WINDOWS_CERTIFICATE not set — skipping Authenticode signing (non-blocking milestone)."
    Write-Host "[$Stage] Installer remains unsigned; see docs/windows-signing.md."
    # Do not `exit` — CI invokes this in-process with `&`.
    return
}

Write-Stage "Import certificate and locate signtool"

$tempDir = Join-Path ([System.IO.Path]::GetTempPath()) ("cam-codesign-" + [guid]::NewGuid().ToString("n"))
New-Item -ItemType Directory -Path $tempDir | Out-Null
$pfxPath = Join-Path $tempDir "codesign.pfx"
$securePass = $null
$cert = $null

try {
    [IO.File]::WriteAllBytes($pfxPath, [Convert]::FromBase64String($CertificateBase64.Trim()))

    if ([string]::IsNullOrEmpty($CertificatePassword)) {
        $securePass = New-Object System.Security.SecureString
    }
    else {
        $securePass = ConvertTo-SecureString -String $CertificatePassword -AsPlainText -Force
    }

    $cert = Import-PfxCertificate -FilePath $pfxPath -CertStoreLocation Cert:\CurrentUser\My -Password $securePass
    if (-not $cert) {
        Fail-Stage "Import-PfxCertificate returned no certificate"
    }
    $thumbprint = $cert.Thumbprint
    Write-Host "[$Stage] Imported cert thumbprint=$thumbprint subject=$($cert.Subject)"

    $signtool = $null
    $kitsRoot = "${env:ProgramFiles(x86)}\Windows Kits\10\bin"
    if (Test-Path $kitsRoot) {
        $candidates = Get-ChildItem -Path $kitsRoot -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match '\\x64\\signtool\.exe$' } |
            Sort-Object FullName -Descending
        if ($candidates) { $signtool = $candidates[0].FullName }
    }
    if (-not $signtool) {
        $cmd = Get-Command signtool.exe -ErrorAction SilentlyContinue
        if ($cmd) { $signtool = $cmd.Source }
    }
    if (-not $signtool) {
        Fail-Stage "signtool.exe not found (install Windows SDK signing tools on the runner)"
    }
    Write-Host "[$Stage] Using signtool: $signtool"
    Close-Stage

    foreach ($raw in $Path) {
        if ([string]::IsNullOrWhiteSpace($raw)) { continue }
        $item = Get-Item -LiteralPath $raw -ErrorAction Stop
        Write-Stage "Sign $($item.Name)"
        & $signtool sign `
            /fd SHA256 `
            /td SHA256 `
            /tr $TimestampUrl `
            /sha1 $thumbprint `
            $item.FullName
        if ($LASTEXITCODE -ne 0) {
            Fail-Stage "signtool failed for $($item.FullName) (exit=$LASTEXITCODE)"
        }
        $sig = Get-AuthenticodeSignature -LiteralPath $item.FullName
        if ($sig.Status -ne "Valid") {
            Fail-Stage "post-sign status for $($item.Name) is $($sig.Status), expected Valid"
        }
        Write-Host "[$Stage] Signed OK: $($item.Name) subject=$($sig.SignerCertificate.Subject)"
        Close-Stage
    }
}
finally {
    if ($cert -and $cert.Thumbprint) {
        Remove-Item -LiteralPath "Cert:\CurrentUser\My\$($cert.Thumbprint)" -ErrorAction SilentlyContinue
    }
    if (Test-Path $tempDir) {
        Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Write-Host "[$Stage] Authenticode signing complete"
return
