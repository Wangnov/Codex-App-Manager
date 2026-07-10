# Windows x64 packaged lifecycle smoke test.
#
# Stages (each failure message is prefixed so CI logs pin the phase):
#   build     — caller already built; this script only consumes the installer
#   install   — passive NSIS install (/P)
#   launch    — first start of the installed main executable
#   upgrade   — re-run installer with /P /UPDATE (in-place upgrade path)
#   uninstall — passive uninstall of the installed product
#   sign-verify — optional Authenticode probe on installer + installed PE files
#
# Usage:
#   pwsh scripts/windows-packaged-smoke.ps1 -Installer path\to\*-setup.exe
#
# Safe for CI: currentUser installMode → %LOCALAPPDATA%\Codex App Manager
# (no admin elevation). Kills the app between stages. Does not touch ~/.codex.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Installer,

    [string]$ProductName = "Codex App Manager",
    [string]$MainBinaryName = "codex-app-manager",
    [int]$LaunchSeconds = 12,
    [ValidateSet("optional", "required", "skip")]
    [string]$AuthenticodeMode = "optional"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Write-Stage([string]$Stage, [string]$Message) {
    Write-Host "::group::[$Stage] $Message"
}

function Close-Stage {
    Write-Host "::endgroup::"
}

function Fail-Stage([string]$Stage, [string]$Message) {
    Write-Host "::error::[$Stage] $Message"
    throw "[$Stage] $Message"
}

function Stop-AppProcesses([string]$BinaryName) {
    Get-Process -Name $BinaryName -ErrorAction SilentlyContinue | ForEach-Object {
        Write-Host "Stopping process $($_.Id) ($($_.ProcessName))"
        Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
    }
    Start-Sleep -Seconds 2
}

function Invoke-Installer([string]$Stage, [string]$Exe, [string[]]$Args) {
    Write-Host "[$Stage] Running: $Exe $($Args -join ' ')"
    $p = Start-Process -FilePath $Exe -ArgumentList $Args -Wait -PassThru -NoNewWindow
    if ($p.ExitCode -ne 0) {
        Fail-Stage $Stage "installer/uninstaller exited $($p.ExitCode)"
    }
}

$installerItem = Get-Item -LiteralPath $Installer -ErrorAction Stop
$installDir = Join-Path $env:LOCALAPPDATA $ProductName
$mainExe = Join-Path $installDir "$MainBinaryName.exe"
$uninstaller = Join-Path $installDir "uninstall.exe"
$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$verifyScript = Join-Path $scriptRoot "verify-windows-authenticode.ps1"

Write-Host "Installer: $($installerItem.FullName)"
Write-Host "InstallDir: $installDir"
Write-Host "MainExe: $mainExe"

# ── sign-verify (installer artifact, pre-install) ───────────────────────────
if ($AuthenticodeMode -ne "skip" -and (Test-Path $verifyScript)) {
    Write-Stage "sign-verify" "Probe Authenticode on installer ($AuthenticodeMode)"
    & $verifyScript -Path @($installerItem.FullName) -Mode $AuthenticodeMode -Stage "sign-verify"
    Close-Stage
}

# Clean slate if a previous run left leftovers.
if (Test-Path $uninstaller) {
    Write-Stage "install" "Removing leftover install from a previous run"
    Stop-AppProcesses $MainBinaryName
    try {
        Invoke-Installer "install" $uninstaller @("/P")
    }
    catch {
        Write-Host "::warning::[install] pre-clean uninstall failed: $_"
    }
    Close-Stage
}

# ── install ─────────────────────────────────────────────────────────────────
Write-Stage "install" "Passive install (/P)"
Stop-AppProcesses $MainBinaryName
Invoke-Installer "install" $installerItem.FullName @("/P")

if (-not (Test-Path -LiteralPath $mainExe)) {
    Fail-Stage "install" "main executable missing after install: $mainExe"
}
if (-not (Test-Path -LiteralPath $uninstaller)) {
    Fail-Stage "install" "uninstaller missing after install: $uninstaller"
}

$vi = (Get-Item -LiteralPath $mainExe).VersionInfo
Write-Host "[install] FileVersion=$($vi.FileVersion) ProductVersion=$($vi.ProductVersion)"
Write-Host "[install] installed PE size=$((Get-Item -LiteralPath $mainExe).Length) bytes"
Close-Stage

# ── sign-verify (installed PE) ──────────────────────────────────────────────
if ($AuthenticodeMode -ne "skip" -and (Test-Path $verifyScript)) {
    Write-Stage "sign-verify" "Probe Authenticode on installed executable + uninstaller"
    & $verifyScript -Path @($mainExe, $uninstaller) -Mode $AuthenticodeMode -Stage "sign-verify"
    Close-Stage
}

# ── launch ──────────────────────────────────────────────────────────────────
Write-Stage "launch" "First launch (${LaunchSeconds}s observe window)"
Stop-AppProcesses $MainBinaryName

$proc = Start-Process -FilePath $mainExe -PassThru -WindowStyle Minimized
Start-Sleep -Seconds $LaunchSeconds

if ($proc.HasExited) {
    # A GUI app exiting immediately with non-zero is a real failure. Exit 0 can
    # happen if single-instance hands off, but a brand-new install should stay up.
    if ($proc.ExitCode -ne 0) {
        Fail-Stage "launch" "process exited early with code $($proc.ExitCode)"
    }
    Write-Host "::warning::[launch] process exited during observe window with code 0 — treating as soft pass"
}
else {
    Write-Host "[launch] process still running (pid=$($proc.Id)) — first launch OK"
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 2
}
# Ensure nothing leftover holds files open for upgrade.
Stop-AppProcesses $MainBinaryName
Close-Stage

# ── upgrade ─────────────────────────────────────────────────────────────────
Write-Stage "upgrade" "Re-run installer with /P /UPDATE"
Stop-AppProcesses $MainBinaryName
Invoke-Installer "upgrade" $installerItem.FullName @("/P", "/UPDATE")

if (-not (Test-Path -LiteralPath $mainExe)) {
    Fail-Stage "upgrade" "main executable missing after upgrade: $mainExe"
}
if (-not (Test-Path -LiteralPath $uninstaller)) {
    Fail-Stage "upgrade" "uninstaller missing after upgrade: $uninstaller"
}
Write-Host "[upgrade] post-upgrade PE size=$((Get-Item -LiteralPath $mainExe).Length) bytes"
Close-Stage

# ── uninstall ───────────────────────────────────────────────────────────────
Write-Stage "uninstall" "Passive uninstall (/P)"
Stop-AppProcesses $MainBinaryName
if (-not (Test-Path -LiteralPath $uninstaller)) {
    Fail-Stage "uninstall" "uninstaller missing: $uninstaller"
}
Invoke-Installer "uninstall" $uninstaller @("/P")
Start-Sleep -Seconds 2

if (Test-Path -LiteralPath $mainExe) {
    Fail-Stage "uninstall" "main executable still present after uninstall: $mainExe"
}
# Install dir may remain empty or with leftover logs; main PE must be gone.
Write-Host "[uninstall] main executable removed"
Close-Stage

Write-Host "Packaged lifecycle smoke passed: install → launch → upgrade → uninstall"
exit 0
