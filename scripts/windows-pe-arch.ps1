# Print PE machine type for Windows binaries (build diagnostics).
#
# Used in release CI for ARM64 cross-builds: confirms the produced PE is
# IMAGE_FILE_MACHINE_ARM64 without claiming that it was *run* on ARM64 hardware.
# Cross-compilation ≠ runtime verification (see docs/windows-signing.md).
#
# Usage:
#   & .\scripts\windows-pe-arch.ps1 -Path path\to\codex-app-manager.exe
#   & .\scripts\windows-pe-arch.ps1 -Path $main -ExpectMachine 0xAA64

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string[]]$Path,

    # Optional hard assert: every inspected PE must match this machine code
    # (e.g. 0xAA64 for ARM64, 0x8664 for x64). Accepts int or hex string.
    [string]$ExpectMachine = ""
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-PeMachine([string]$FilePath) {
    $fs = [System.IO.File]::OpenRead($FilePath)
    try {
        $br = New-Object System.IO.BinaryReader($fs)
        if ($br.ReadUInt16() -ne 0x5A4D) {
            return [pscustomobject]@{ Path = $FilePath; MachineValue = -1; Machine = "not-MZ"; Label = "not a PE" }
        }
        $fs.Seek(0x3C, [System.IO.SeekOrigin]::Begin) | Out-Null
        $peOffset = $br.ReadUInt32()
        $fs.Seek($peOffset, [System.IO.SeekOrigin]::Begin) | Out-Null
        if ($br.ReadUInt32() -ne 0x4550) {
            return [pscustomobject]@{ Path = $FilePath; MachineValue = -1; Machine = "bad-PE"; Label = "invalid PE signature" }
        }
        $machine = $br.ReadUInt16()
        $label = switch ($machine) {
            0x014c { "i386" }
            0x8664 { "x86_64 / AMD64" }
            0xAA64 { "aarch64 / ARM64" }
            0x01c4 { "ARMNT" }
            default { "unknown(0x{0:X4})" -f $machine }
        }
        return [pscustomobject]@{
            Path         = $FilePath
            MachineValue = [int]$machine
            Machine      = ("0x{0:X4}" -f $machine)
            Label        = $label
        }
    }
    finally {
        $fs.Dispose()
    }
}

$expectValue = $null
if (-not [string]::IsNullOrWhiteSpace($ExpectMachine)) {
    $expectValue = [int]($ExpectMachine.Trim())
}

$rows = @()
$failed = $false
foreach ($raw in $Path) {
    if (-not (Test-Path -LiteralPath $raw)) {
        Write-Host "::error::PE arch: missing $raw"
        $failed = $true
        continue
    }
    $info = Get-PeMachine (Resolve-Path -LiteralPath $raw).Path
    $rows += $info
    Write-Host ("PE {0}  machine={1}  {2}" -f (Split-Path $info.Path -Leaf), $info.Machine, $info.Label)

    if ($null -ne $expectValue) {
        if ($info.MachineValue -ne $expectValue) {
            Write-Host ("::error::PE arch mismatch for {0}: expected 0x{1:X4}, got {2}" -f (Split-Path $info.Path -Leaf), $expectValue, $info.Machine)
            $failed = $true
        }
    }
}

if ($rows.Count -eq 0) {
    Write-Host "::error::No PE files inspected"
    throw "No PE files inspected"
}

$rows | Format-Table -AutoSize | Out-String | Write-Host

if ($failed) {
    Write-Host "::error::PE architecture check failed"
    throw "PE architecture check failed"
}

if ($null -ne $expectValue) {
    Write-Host ("PE architecture assert OK (ExpectMachine=0x{0:X4}, files={1})" -f $expectValue, $rows.Count)
}

# Do not `exit` — CI invokes this in-process with `&`.
return
