# Print PE machine type for Windows binaries (build diagnostics).
#
# Used in release CI for ARM64 cross-builds: confirms the produced PE is
# IMAGE_FILE_MACHINE_ARM64 without claiming that it was *run* on ARM64 hardware.
# Cross-compilation ≠ runtime verification (see docs/windows-signing.md).
#
# Usage:
#   pwsh scripts/windows-pe-arch.ps1 -Path path\to\codex-app-manager.exe

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string[]]$Path
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Get-PeMachine([string]$FilePath) {
    $fs = [System.IO.File]::OpenRead($FilePath)
    try {
        $br = New-Object System.IO.BinaryReader($fs)
        if ($br.ReadUInt16() -ne 0x5A4D) {
            return [pscustomobject]@{ Path = $FilePath; Machine = "not-MZ"; Label = "not a PE" }
        }
        $fs.Seek(0x3C, [System.IO.SeekOrigin]::Begin) | Out-Null
        $peOffset = $br.ReadUInt32()
        $fs.Seek($peOffset, [System.IO.SeekOrigin]::Begin) | Out-Null
        if ($br.ReadUInt32() -ne 0x4550) {
            return [pscustomobject]@{ Path = $FilePath; Machine = "bad-PE"; Label = "invalid PE signature" }
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
            Path    = $FilePath
            Machine = ("0x{0:X4}" -f $machine)
            Label   = $label
        }
    }
    finally {
        $fs.Dispose()
    }
}

$rows = @()
foreach ($raw in $Path) {
    if (-not (Test-Path -LiteralPath $raw)) {
        Write-Host "::warning::PE arch: missing $raw"
        continue
    }
    $info = Get-PeMachine (Resolve-Path -LiteralPath $raw).Path
    $rows += $info
    Write-Host ("PE {0}  machine={1}  {2}" -f (Split-Path $info.Path -Leaf), $info.Machine, $info.Label)
}

if ($rows.Count -eq 0) {
    Write-Host "::error::No PE files inspected"
    exit 1
}

$rows | Format-Table -AutoSize | Out-String | Write-Host
exit 0
