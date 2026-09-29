# Download, SHA-256-verify, and extract the pinned SSL.com eSigner CKA
# release archive, normalizing the extracted installer to one fixed,
# predictable filename.
#
# The archive (SSL.COM-eSigner-CKA_<version>.zip) does NOT contain a file
# named "eSigner_CKA_Installer.exe". It contains exactly one executable
# named after the specific build, e.g.
# "SSL.COM eSigner CKA_1.0.6_build_20230829.exe" — SSL.com's own published
# CI/CD integration guide renames it with `Move-Item` immediately after
# extraction for exactly this reason:
# https://www.ssl.com/how-to/how-to-integrate-esigner-cka-with-ci-cd-tools-for-automated-code-signing/
# This script mirrors that rename so every caller can depend on one stable
# path regardless of the exact build-stamped filename SSL.com ships inside
# a given release.
#
# Used by:
#   - .github/workflows/release.yml ("Provision eSigner CKA certificate") —
#     the credentialed step that actually installs and loads the
#     certificate from the resolved installer.
#   - .github/workflows/win-installer-check.yml ("eSigner CKA archive
#     extraction check") — a secret-free, no-execute dry run on every PR
#     that touches this script or the pin, so a future SSL.com release
#     that changes the archive layout (or a bad pin bump) fails CI instead
#     of only failing the first real release attempt.
#
# Never runs the installer itself — only resolves its path. Only the
# credentialed "Provision eSigner CKA certificate" step should ever execute
# it, since that is the only place ESIGNER_* secrets are in scope.
#
# Usage:
#   $installer = & ./scripts/resolve-esigner-cka-installer.ps1 -DestinationDir $installDir
#
# Update -Version / -ExpectedSha256 together in a reviewed PR when bumping
# the eSigner CKA release.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$DestinationDir,

    [string]$Version = "1.0.6",
    [string]$ExpectedSha256 = "e4971440e4ebed94328492cf36e18999554c5c657c856f1cb14a6072c8b1c263",
    [string]$Stage = "esigner-resolve"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Fail-Stage([string]$Message) {
    Write-Host "::error::[$Stage] $Message"
    throw "[$Stage] $Message"
}

$archiveUrl = "https://github.com/SSLcom/eSignerCKA/releases/download/v$Version/SSL.COM-eSigner-CKA_$Version.zip"
New-Item -ItemType Directory -Force -Path $DestinationDir | Out-Null
$zip = Join-Path $DestinationDir "eSignerCKA.zip"
Invoke-WebRequest -OutFile $zip -Uri $archiveUrl

$actualSha256 = (Get-FileHash -LiteralPath $zip -Algorithm SHA256).Hash
if ($actualSha256 -ne $ExpectedSha256) {
    Fail-Stage "eSigner CKA archive SHA-256 mismatch: expected $ExpectedSha256, got $actualSha256 — refusing to extract/execute a possibly-tampered release asset"
}
Write-Host "[$Stage] eSigner CKA archive SHA-256 verified: $actualSha256"

Expand-Archive -Force -Path $zip -DestinationPath $DestinationDir

# The archive contains exactly one top-level executable, named after the
# specific build rather than a fixed name — normalize it so callers have
# one stable path to invoke. See the header comment above for why this
# rename is required.
$extracted = Get-ChildItem -Path $DestinationDir -Filter "*.exe" -Recurse | Select-Object -First 1
if (-not $extracted) {
    Fail-Stage "no installer executable found after extracting the eSigner CKA release archive"
}

$installerPath = Join-Path $DestinationDir "eSigner_CKA_Installer.exe"
if ($extracted.FullName -ne $installerPath) {
    Move-Item -Force -LiteralPath $extracted.FullName -Destination $installerPath
}
Write-Host "[$Stage] eSigner CKA installer ready at $installerPath (extracted as `"$($extracted.Name)`")"
Write-Output $installerPath
