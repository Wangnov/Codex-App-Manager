# Verify Authenticode signatures on Windows PE files (installer, app, uninstaller).
#
# Modes:
#   optional  — report status; unsigned/NotSigned exits 0 (current milestone while
#               no WINDOWS_SIGNING_PROVIDER is configured). Fail only if a path is
#               missing or Get-AuthenticodeSignature itself errors.
#   required  — every path must have Status -eq Valid AND carry an RFC3161
#               countersignature (TimeStamperCertificate present). Use after a
#               real provider is wired into release (repo var
#               AUTHENTICODE_REQUIRED=true) — see docs/code-signing-policy.md's
#               "Valid Authenticode + timestamp on all three PE layers" clause.
#
# -ExpectedThumbprint (either mode): when set, every path's
#   SignerCertificate.Thumbprint must exactly match it, or that path fails —
#   even in "optional" mode, where an unsigned/NotSigned file would otherwise
#   soft-pass. This is what tells the difference between "no provider
#   configured yet" (soft pass) and "a provider is configured and this file
#   was expected to carry ITS certificate but doesn't" (hard fail). Used by:
#   release.yml passing the real provider's WINDOWS_SIGNING_THUMBPRINT (once
#   esigner/certum export it) so `required` mode also enforces publisher
#   identity, not just Valid+timestamp; and by win-installer-check.yml's CI
#   proof passing the throwaway self-signed certificate's thumbprint, so the
#   proof fails if any expected PE (including the uninstaller, verified only
#   after install) was not actually signed by the certificate the build was
#   given.
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

    # When set (either mode), SignerCertificate.Thumbprint must exactly
    # match this. See the header comment above.
    [string]$ExpectedThumbprint = "",

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
    $hasTimestamp = [bool]$sig.TimeStamperCertificate
    $ok = $false

    switch ($Mode) {
        "optional" {
            # NotSigned is expected until a provider is configured. HashMismatch /
            # NotTrusted / UnknownError still surface as warnings for visibility.
            if ($status -eq "Valid") {
                $ok = $true
            }
            elseif ($status -eq "NotSigned") {
                $ok = $true
                Write-Host "::warning::[$Stage] unsigned (expected until Authenticode provider is configured): $($item.Name)"
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
            elseif (-not $hasTimestamp) {
                $ok = $false
                Write-Host "::error::[$Stage] $($item.Name): Valid but missing an RFC3161 timestamp (TimeStamperCertificate absent) — the signature will invalidate at certificate expiry"
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

    # Identity check: independent of Mode, and independent of whether the
    # Mode-level check above already passed. Runs even in optional mode's
    # soft-pass branches (e.g. NotSigned) so that "a certificate was
    # expected but this file doesn't carry it" is always a hard failure once
    # the caller supplies -ExpectedThumbprint, not just once enforcement
    # (required mode) is fully on.
    if ($ExpectedThumbprint) {
        $actualThumbprint = if ($sig.SignerCertificate) { $sig.SignerCertificate.Thumbprint } else { "" }
        if ($actualThumbprint -ne $ExpectedThumbprint) {
            $ok = $false
            Write-Host "::error::[$Stage] $($item.Name): signer thumbprint '$actualThumbprint' does not match expected '$ExpectedThumbprint'"
        }
    }

    if (-not $ok) { $failed = $true }

    $results += [pscustomobject]@{
        Path      = $item.FullName
        Status    = $status
        Timestamp = $hasTimestamp
        Subject   = $subject
        Ok        = $ok
    }

    Write-Host ("  {0,-12} {1,-6} {2}  {3}" -f $status, $hasTimestamp, $item.Name, $subject)
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
