# Windows x64 packaged lifecycle smoke test.
#
# Stages (each failure message is prefixed so CI logs pin the phase):
#   build     — caller already built; this script only consumes the installer
#   install   — passive NSIS install (/P)
#   launch    — first start of the installed main executable
#   upgrade   — re-run installer with /P /UPDATE (in-place upgrade path)
#   uninstall — passive uninstall of the installed product (always attempted in finally)
#   sign-verify — optional Authenticode probe on installer + installed PE files
#
# Usage (prefer in-process from CI shell:pwsh steps — nested `pwsh -File` breaks
# array binding for -Path on some runners):
#   & .\scripts\windows-packaged-smoke.ps1 -Installer path\to\*-setup.exe
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

function Invoke-Installer([string]$Stage, [string]$Exe, [string[]]$InstallerArgs) {
    Write-Host "[$Stage] Running: $Exe $($InstallerArgs -join ' ')"
    $p = Start-Process -FilePath $Exe -ArgumentList $InstallerArgs -Wait -PassThru -NoNewWindow
    if ($p.ExitCode -ne 0) {
        Fail-Stage $Stage "installer/uninstaller exited $($p.ExitCode)"
    }
}

function Invoke-UninstallBestEffort([string]$Stage, [string]$UninstallerPath, [string]$BinaryName, [string]$MainExePath) {
    Stop-AppProcesses $BinaryName
    if (-not (Test-Path -LiteralPath $UninstallerPath)) {
        Write-Host "[$Stage] uninstaller not present; nothing to clean"
        return
    }
    Write-Host "[$Stage] Running: $UninstallerPath /P"
    $p = Start-Process -FilePath $UninstallerPath -ArgumentList @("/P") -Wait -PassThru -NoNewWindow
    if ($p.ExitCode -ne 0) {
        Write-Host "::warning::[$Stage] uninstaller exited $($p.ExitCode)"
    }
    Start-Sleep -Seconds 2
    if (Test-Path -LiteralPath $MainExePath) {
        Write-Host "::warning::[$Stage] main executable still present after uninstall attempt: $MainExePath"
    }
    else {
        Write-Host "[$Stage] main executable removed"
    }
}

$installerItem = Get-Item -LiteralPath $Installer -ErrorAction Stop
$installDir = Join-Path $env:LOCALAPPDATA $ProductName
$mainExe = Join-Path $installDir "$MainBinaryName.exe"
$uninstaller = Join-Path $installDir "uninstall.exe"
$scriptRoot = Split-Path -Parent $MyInvocation.MyCommand.Path
$verifyScript = Join-Path $scriptRoot "verify-windows-authenticode.ps1"
$didInstall = $false
$smokeFailed = $false
$smokeError = $null

Write-Host "Installer: $($installerItem.FullName)"
Write-Host "InstallDir: $installDir"
Write-Host "MainExe: $mainExe"

try {
    # ── sign-verify (installer artifact, pre-install) ───────────────────────
    if ($AuthenticodeMode -ne "skip" -and (Test-Path $verifyScript)) {
        Write-Stage "sign-verify" "Probe Authenticode on installer ($AuthenticodeMode)"
        & $verifyScript -Path $installerItem.FullName -Mode $AuthenticodeMode -Stage "sign-verify"
        Close-Stage
    }

    # Clean slate if a previous run left leftovers.
    if (Test-Path $uninstaller) {
        Write-Stage "install" "Removing leftover install from a previous run"
        Invoke-UninstallBestEffort "install" $uninstaller $MainBinaryName $mainExe
        Close-Stage
    }

    # ── install ─────────────────────────────────────────────────────────────
    Write-Stage "install" "Passive install (/P)"
    Stop-AppProcesses $MainBinaryName
    Invoke-Installer "install" $installerItem.FullName @("/P")
    $didInstall = $true

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

    # ── sign-verify (installed PE) ──────────────────────────────────────────
    if ($AuthenticodeMode -ne "skip" -and (Test-Path $verifyScript)) {
        Write-Stage "sign-verify" "Probe Authenticode on installed executable + uninstaller"
        & $verifyScript -Path @($mainExe, $uninstaller) -Mode $AuthenticodeMode -Stage "sign-verify"
        Close-Stage
    }

    # ── launch ──────────────────────────────────────────────────────────────
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

    # ── upgrade ─────────────────────────────────────────────────────────────
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

    # ── uninstall (happy path) ──────────────────────────────────────────────
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
    Write-Host "[uninstall] main executable removed"
    $didInstall = $false
    Close-Stage

    Write-Host "Packaged lifecycle smoke passed: install → launch → upgrade → uninstall"
}
catch {
    $smokeFailed = $true
    $smokeError = $_
    Write-Host "::error::[smoke] $($_.Exception.Message)"
}
finally {
    # Always attempt cleanup so a failed launch/upgrade does not leave the product
    # installed on the runner (and so retry jobs start clean).
    if ($didInstall -or (Test-Path -LiteralPath $mainExe) -or (Test-Path -LiteralPath $uninstaller)) {
        Write-Stage "uninstall" "finally: best-effort uninstall after smoke"
        try {
            Invoke-UninstallBestEffort "uninstall" $uninstaller $MainBinaryName $mainExe
        }
        catch {
            Write-Host "::warning::[uninstall] finally cleanup failed: $_"
        }
        Close-Stage
    }
}

if ($smokeFailed) {
    throw $smokeError
}

# Do not `exit` — CI invokes this in-process with `&`.
return
