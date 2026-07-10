# Verify Authenticode signatures on Windows PE files (installer, app, uninstaller).
#
# Modes:
#   optional  — report status; unsigned/NotSigned exits 0 (current milestone while
#               OV/EV cert budget is not in place). Fail only if a path is missing
#               or Get-AuthenticodeSignature itself errors.
#   required  — every path must have Status -eq Valid. Use after cert is wired
#               into release (repo var AUTHENTICODE_REQUIRED=true).
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
    $status = [string]$sig.Status
    $ok = $false

    switch ($Mode) {
        "optional" {
            # NotSigned is expected until OV/EV is configured. HashMismatch /
            # NotTrusted / UnknownError still surface as warnings for visibility.
            if ($status -eq "Valid") {
                $ok = $true
            }
            elseif ($status -eq "NotSigned") {
                $ok = $true
                Write-Host "::warning::[$Stage] unsigned (expected until Authenticode cert is configured): $($item.Name)"
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
        Ok      = $ok
    }

    Write-Host ("  {0,-12} {1}  {2}" -f $status, $item.Name, $subject)
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
