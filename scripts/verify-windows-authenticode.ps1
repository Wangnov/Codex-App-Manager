# Verify Authenticode signatures on Windows PE files (installer, app, uninstaller).
#
# Modes:
#   optional  — PR/local diagnostic for builds that intentionally receive no
#               release certificate. Reports unsigned files without blocking.
#   required  — release gate: every path must have Status -eq Valid. Can also
#               pin the SignPath subject and require timestamp evidence.
#
# Usage:
#   pwsh scripts/verify-windows-authenticode.ps1 -Path a.exe,b.exe -Mode optional
#   pwsh scripts/verify-windows-authenticode.ps1 -Path (Get-ChildItem *.exe) -Mode required
#
# Does NOT check Tauri updater (.sig / latest.json) signatures — that is a
# separate system (see docs/windows-signing.md).

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string[]]$Path,

    [ValidateSet("optional", "required")]
    [string]$Mode = "optional",

    # When set (required mode), SignerCertificate.Subject must contain this.
    [string]$ExpectedSubject = "",

    # Requires a timestamp countersigner on the artifact. This property alone
    # does not distinguish RFC3161 from every legacy timestamp form; the future
    # SignPath pilot must also validate its policy and verbose signtool output.
    [switch]$RequireTimestamp,

    [string]$Stage = "sign-verify"
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

Write-Stage "Authenticode verification (mode=$Mode)"

$results = @()
$failed = $false

foreach ($raw in $Path) {
    if ([string]::IsNullOrWhiteSpace($raw)) { continue }
    $item = Get-Item -LiteralPath $raw -ErrorAction SilentlyContinue
    if (-not $item) {
        $failed = $true
        Write-Host "::error::[$Stage] missing file: $raw"
        $results += [pscustomobject]@{
            Path    = $raw
            Status  = "Missing"
            Subject = ""
            Ok      = $false
        }
        continue
    }

    $sig = Get-AuthenticodeSignature -LiteralPath $item.FullName
    $subject = if ($sig.SignerCertificate) { $sig.SignerCertificate.Subject } else { "" }
    $thumbprint = if ($sig.SignerCertificate) { $sig.SignerCertificate.Thumbprint.Replace(" ", "").ToUpperInvariant() } else { "" }
    $timestampSubject = if ($sig.TimeStamperCertificate) { $sig.TimeStamperCertificate.Subject } else { "" }
    $status = [string]$sig.Status
    $ok = $false

    switch ($Mode) {
        "optional" {
            # PR builds intentionally receive no release secret. HashMismatch /
            # NotTrusted / UnknownError still surface as warnings for visibility.
            if ($status -eq "Valid") {
                $ok = $true
            }
            elseif ($status -eq "NotSigned") {
                $ok = $true
                Write-Host "::warning::[$Stage] unsigned (expected only for PR/local diagnostics): $($item.Name)"
            }
            else {
                # Soft-fail in optional mode: report but do not block release.
                $ok = $true
                Write-Host "::warning::[$Stage] $($item.Name): Status=$status Subject=$subject"
            }
        }
        "required" {
            if ($status -ne "Valid") {
                $ok = $false
                Write-Host "::error::[$Stage] $($item.Name): expected Valid, got $status"
            }
            elseif ($ExpectedSubject -and ($subject -notlike "*$ExpectedSubject*")) {
                $ok = $false
                Write-Host "::error::[$Stage] $($item.Name): subject '$subject' does not contain '$ExpectedSubject'"
            }
            elseif ($RequireTimestamp -and -not $sig.TimeStamperCertificate) {
                $ok = $false
                Write-Host "::error::[$Stage] $($item.Name): no RFC3161 timestamp countersigner"
            }
            else {
                $ok = $true
            }
        }
    }

    if (-not $ok) { $failed = $true }

    $results += [pscustomobject]@{
        Path    = $item.FullName
        Status  = $status
        Subject = $subject
        Thumbprint = $thumbprint
        TimestampSubject = $timestampSubject
        Ok      = $ok
    }

    Write-Host ("  {0,-12} {1}  signer={2} timestamp={3}" -f $status, $item.Name, $subject, $timestampSubject)
}

$results | Format-Table -AutoSize | Out-String | Write-Host
Close-Stage

if ($failed) {
    Fail-Stage "Authenticode verification failed (mode=$Mode)"
}

Write-Host "[$Stage] Authenticode check passed (mode=$Mode, files=$($results.Count))"
# Do not `exit` — scripts are invoked in-process with `&` from CI steps;
# `exit` would terminate the whole step (and skip e.g. smoke after verify).
return
