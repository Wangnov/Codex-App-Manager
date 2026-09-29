# Provider-agnostic Authenticode signing entry point.
#
# This is the SINGLE script every Windows PE signing path in this repository
# calls: Tauri's own `bundle.windows.signCommand` hook (invoked once per file
# during `tauri build` for the main binary, the NSIS uninstaller via
# `!uninstfinalize`, and the final `-setup.exe`), the CI self-signed-certificate
# proof job, and any ad-hoc/manual signing. It dispatches on the
# WINDOWS_SIGNING_PROVIDER configuration instead of hard-coding one CA:
#
#   (unset / "" / "none")  — no provider configured. Prints a skip message and
#                             returns 0. This is the DEFAULT and matches today's
#                             behavior exactly: installers stay unsigned and the
#                             release is still published (non-blocking).
#   "local-pfx"             — imports an ephemeral PFX (WINDOWS_CERTIFICATE +
#                             WINDOWS_CERTIFICATE_PASSWORD, base64-encoded PFX)
#                             into CurrentUser\My, signs, then removes it. Used
#                             for local/CI testing (including with a throwaway
#                             self-signed certificate — see
#                             .github/workflows/win-installer-check.yml) and as
#                             a legacy fallback if a real OV/EV PFX is ever
#                             issued directly instead of through a cloud HSM.
#   "esigner"                — recommended provider (SSL.com eSigner, via the
#                             eSigner CKA Cloud Key Adapter). A separate CI
#                             provisioning step authenticates to SSL.com and
#                             installs a CNG-backed certificate into the
#                             runner's certificate store *before* this script
#                             runs, and exports its thumbprint as
#                             WINDOWS_SIGNING_THUMBPRINT. This script only
#                             calls signtool with that thumbprint — it never
#                             sees the eSigner account credentials.
#   "certum"                  — fallback provider (Certum Open Source Code
#                             Signing via SimplySign). Same shape as "esigner":
#                             a separate provisioning step logs into SimplySign
#                             and exports WINDOWS_SIGNING_THUMBPRINT; this
#                             script just signs with it.
#
# See docs/windows-signing.md and docs/code-signing-policy.md for the full
# provider comparison, the required secrets/variables, and the rollout plan
# (WINDOWS_SIGNING_PROVIDER stays unset until a certificate exists; repo
# variable AUTHENTICODE_REQUIRED then gates verify-windows-authenticode.ps1
# between "optional" and "required" mode once signing is proven end to end).
#
# This script signs. It does NOT decide whether a signature must be Valid —
# that is verify-windows-authenticode.ps1's job (optional vs required mode).
# A self-signed test certificate (local-pfx in CI) will never show
# Status=Valid; this script only asserts that signtool succeeded and that the
# resulting signature's certificate thumbprint matches the one it signed with.
#
# Never logs secret values (PFX password, eSigner/Certum credentials). The
# certificate thumbprint is not a secret and is safe to print.
#
# Does NOT produce Tauri updater .sig files — use `tauri signer sign` for that
# (see .github/workflows/release.yml, "Sign Windows updater artifact").
#
# Usage (manual/CI, one or more files):
#   $env:WINDOWS_SIGNING_PROVIDER = "local-pfx"
#   $env:WINDOWS_CERTIFICATE = "<base64 pfx>"
#   $env:WINDOWS_CERTIFICATE_PASSWORD = "..."
#   pwsh scripts/sign-windows-authenticode.ps1 -Path path\to\setup.exe
#
# Usage (Tauri signCommand, wired in src-tauri/tauri.conf.json):
#   { "cmd": "powershell", "args": ["-NoProfile", "-ExecutionPolicy", "Bypass",
#     "-File", "../scripts/sign-windows-authenticode.ps1", "-Path", "%1"] }
#   `tauri build` sets its process working directory to src-tauri/ before
#   bundling (see tauri-cli's `set_current_dir(dirs.tauri)`), so the -File
#   path here is relative to src-tauri/, NOT the repo root — hence "../".
#   Tauri substitutes %1 with the absolute path of each binary it signs
#   (main exe, generated uninstaller, final NSIS installer) and invokes this
#   script once per file with WINDOWS_SIGNING_PROVIDER (and the matching
#   secrets/thumbprint) already present in the job environment.
#   The hook intentionally invokes `powershell` (Windows PowerShell 5.1,
#   preinstalled on every supported Windows version) rather than `pwsh`
#   (PowerShell 7+): this script uses only PS 5.1-compatible syntax, and
#   `tauri build` runs this hook unconditionally — including for a
#   contributor's local unsigned build — so it must not require installing
#   PowerShell 7 just to keep local Windows builds working. CI workflow
#   steps that call this script directly (win-installer-check.yml,
#   release.yml) continue to use `shell: pwsh` for everything else.
#
#   Tauri's NSIS bundler also invokes this hook for the stock NSIS plugin
#   DLLs it stages next to the installer (NSISdl.dll, System.dll, etc.) and
#   its own nsis_tauri_utils.dll helper. This script recognizes those by
#   filename and skips them (see the $nsisThirdPartyPluginNames check
#   below) instead of signing third-party binaries with this project's
#   certificate — docs/code-signing-policy.md's scope explicitly rules
#   that out. The main exe, uninstaller, and final installer are never in
#   that list and are always signed when a provider is configured.

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string[]]$Path,

    [string]$Provider = $(if ($env:WINDOWS_SIGNING_PROVIDER) { $env:WINDOWS_SIGNING_PROVIDER } else { "" }),

    # local-pfx only.
    [string]$CertificateBase64 = $env:WINDOWS_CERTIFICATE,
    [string]$CertificatePassword = $env:WINDOWS_CERTIFICATE_PASSWORD,

    # esigner / certum only: the thumbprint of a certificate a prior CI step
    # already installed into the certificate store.
    [string]$Thumbprint = $env:WINDOWS_SIGNING_THUMBPRINT,

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

function Find-SignTool {
    $kitsRoot = "${env:ProgramFiles(x86)}\Windows Kits\10\bin"
    if (Test-Path $kitsRoot) {
        $candidates = Get-ChildItem -Path $kitsRoot -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match '\\x64\\signtool\.exe$' } |
            Sort-Object FullName -Descending
        if ($candidates) { return $candidates[0].FullName }
    }
    $cmd = Get-Command signtool.exe -ErrorAction SilentlyContinue
    if ($cmd) { return $cmd.Source }
    return $null
}

# RFC 3161 timestamp servers are public best-effort services and blip
# occasionally; a single failure must not abort a whole `tauri build` (and,
# on PRs, turn win-installer-check red for an unrelated change). Try the
# configured URL first, then well-known fallbacks, for a few rounds with
# backoff. A failed signtool attempt leaves the file unmodified (the
# signature is only written after the timestamp is obtained), so retrying
# cannot produce a doubly-signed binary. Non-timestamp failures (missing
# certificate, HSM login expired, ...) also retry but ultimately fail with
# signtool's own output above.
$TimestampFallbackUrls = @(
    "http://timestamp.digicert.com",
    "http://timestamp.sectigo.com",
    "http://timestamp.globalsign.com/tsa/r6advanced1"
)

function Get-TimestampUrlCandidates {
    $urls = New-Object System.Collections.Generic.List[string]
    foreach ($u in @($TimestampUrl) + $TimestampFallbackUrls) {
        if (-not [string]::IsNullOrWhiteSpace($u) -and -not $urls.Contains($u)) { $urls.Add($u) }
    }
    return , $urls.ToArray()
}

function Invoke-SignToolWithRetry([string]$SignTool, [string]$SignThumbprint, [string]$FilePath) {
    $rounds = 3
    $urls = Get-TimestampUrlCandidates
    $lastExit = 0
    for ($round = 1; $round -le $rounds; $round++) {
        foreach ($url in $urls) {
            & $SignTool sign `
                /fd SHA256 `
                /td SHA256 `
                /tr $url `
                /sha1 $SignThumbprint `
                $FilePath
            $lastExit = $LASTEXITCODE
            if ($lastExit -eq 0) { return }
            Write-Host "::warning::[$Stage] signtool failed for $(Split-Path -Leaf $FilePath) with timestamp server $url (exit=$lastExit, round $round/$rounds)"
        }
        if ($round -lt $rounds) {
            $delay = 5 * $round
            Write-Host "[$Stage] Retrying in ${delay}s..."
            Start-Sleep -Seconds $delay
        }
    }
    Fail-Stage "signtool failed for $FilePath after $rounds rounds across $($urls.Count) timestamp server(s) (last exit=$lastExit)"
}

# Signs every $Path with signtool using an already-known certificate
# thumbprint (already imported/loaded into a certificate store the local
# signtool.exe can see). Shared by local-pfx (after import), esigner, and
# certum (after their provisioning steps have loaded a cert).
function Invoke-ThumbprintSign([string]$SignTool, [string]$SignThumbprint, [string[]]$Paths) {
    foreach ($raw in $Paths) {
        if ([string]::IsNullOrWhiteSpace($raw)) { continue }
        $item = Get-Item -LiteralPath $raw -ErrorAction Stop
        Write-Stage "Sign $($item.Name)"
        Invoke-SignToolWithRetry -SignTool $SignTool -SignThumbprint $SignThumbprint -FilePath $item.FullName

        # Do NOT require Status -eq Valid here: a throwaway self-signed
        # certificate (local-pfx CI proof) will never chain to a trusted
        # root, so its Status will be UnknownError/NotTrusted even though
        # signing genuinely succeeded. Assert instead that a signature was
        # actually attached and that it came from the certificate we signed
        # with. Whether the signature must additionally be Status=Valid is
        # verify-windows-authenticode.ps1's job (optional vs required mode).
        $sig = Get-AuthenticodeSignature -LiteralPath $item.FullName
        if ($sig.Status -eq "NotSigned" -or -not $sig.SignerCertificate) {
            Fail-Stage "post-sign check for $($item.Name): no signature attached (Status=$($sig.Status))"
        }
        if ($sig.SignerCertificate.Thumbprint -ne $SignThumbprint) {
            Fail-Stage "post-sign check for $($item.Name): signer thumbprint $($sig.SignerCertificate.Thumbprint) does not match $SignThumbprint"
        }
        Write-Host "[$Stage] Signed OK: $($item.Name) status=$($sig.Status) subject=$($sig.SignerCertificate.Subject) thumbprint=$($sig.SignerCertificate.Thumbprint)"
        Close-Stage
    }
}

# Tauri's NSIS bundler also invokes signCommand for the stock NSIS plugin
# DLLs it copies alongside the installer (NSISdl.dll, StartMenu.dll,
# System.dll, nsDialogs.dll) plus its own nsis_tauri_utils.dll helper, in
# addition to this project's own main exe / uninstaller / installer — see
# tauri-bundler's `nsis/mod.rs` ("Signing NSIS plugins", NSIS_PLUGIN_FILES).
# Those DLLs ship as part of the NSIS toolset (and the tauri-apps org's own
# helper), not code this project authored — signing them with this
# project's certificate would present third-party binaries as project-owned
# code, which docs/code-signing-policy.md's scope section rules out ("third-
# party binaries must not be presented or separately signed as
# project-owned code"). Skip them by filename; every other invocation
# (main exe, `!uninstfinalize` uninstaller, final `-setup.exe`) is this
# project's own artifact and still gets signed normally.
$nsisThirdPartyPluginNames = @(
    "NSISdl.dll",
    "StartMenu.dll",
    "System.dll",
    "nsDialogs.dll",
    "nsis_tauri_utils.dll"
)

function Get-SignablePaths([string[]]$Candidates, [string]$StageName) {
    $result = New-Object System.Collections.Generic.List[string]
    foreach ($raw in $Candidates) {
        if ([string]::IsNullOrWhiteSpace($raw)) { continue }
        $leaf = Split-Path -Path $raw -Leaf
        if ($nsisThirdPartyPluginNames -contains $leaf) {
            Write-Host "[$StageName] Skipping $leaf — third-party NSIS plugin DLL, not project-owned code (see docs/code-signing-policy.md)."
            continue
        }
        $result.Add($raw)
    }
    return , $result.ToArray()
}

$normalizedProvider = $Provider.Trim().ToLowerInvariant()
$Path = Get-SignablePaths -Candidates $Path -StageName $Stage

if ($Path.Count -eq 0) {
    Write-Host "[$Stage] Nothing left to sign after excluding third-party NSIS plugin DLLs."
    return
}

if ([string]::IsNullOrWhiteSpace($normalizedProvider) -or $normalizedProvider -eq "none") {
    # Migration guard: before this script existed, release.yml's old
    # "Authenticode-sign Windows installer (optional)" step signed the
    # installer whenever the WINDOWS_CERTIFICATE secret alone was present —
    # no separate provider switch existed. If that secret is still set but
    # WINDOWS_SIGNING_PROVIDER has not been added, silently returning here
    # would turn a previously-signed release into an unsigned one with no
    # error. Fail loudly instead and require an explicit provider choice,
    # rather than ever silently downgrading an existing signing setup.
    if (-not [string]::IsNullOrWhiteSpace($CertificateBase64)) {
        Fail-Stage "WINDOWS_CERTIFICATE is set but WINDOWS_SIGNING_PROVIDER is not — refusing to silently publish an unsigned release. Set the repo variable WINDOWS_SIGNING_PROVIDER=local-pfx to keep signing with this PFX, or remove the WINDOWS_CERTIFICATE secret if it is no longer intended to be used."
    }
    Write-Host "[$Stage] WINDOWS_SIGNING_PROVIDER not set — skipping Authenticode signing (non-blocking milestone)."
    Write-Host "[$Stage] Binaries remain unsigned; see docs/windows-signing.md."
    # Do not `exit` — CI and Tauri's signCommand invoke this in-process/as a
    # subprocess whose success (exit 0) must not block the unsigned release.
    return
}

if ($normalizedProvider -eq "local-pfx") {
    if ([string]::IsNullOrWhiteSpace($CertificateBase64)) {
        Fail-Stage "WINDOWS_SIGNING_PROVIDER=local-pfx but WINDOWS_CERTIFICATE is empty — set the base64 PFX secret or unset the provider."
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
        Write-Host "[$Stage] Imported cert thumbprint=$($cert.Thumbprint) subject=$($cert.Subject)"

        $signtool = Find-SignTool
        if (-not $signtool) {
            Fail-Stage "signtool.exe not found (install Windows SDK signing tools on the runner)"
        }
        Write-Host "[$Stage] Using signtool: $signtool"
        Close-Stage

        Invoke-ThumbprintSign -SignTool $signtool -SignThumbprint $cert.Thumbprint -Paths $Path
    }
    finally {
        if ($cert -and $cert.Thumbprint) {
            Remove-Item -LiteralPath "Cert:\CurrentUser\My\$($cert.Thumbprint)" -ErrorAction SilentlyContinue
        }
        if (Test-Path $tempDir) {
            Remove-Item -LiteralPath $tempDir -Recurse -Force -ErrorAction SilentlyContinue
        }
    }

    Write-Host "[$Stage] Authenticode signing complete (provider=local-pfx)"
    return
}

if ($normalizedProvider -eq "esigner" -or $normalizedProvider -eq "certum") {
    if ([string]::IsNullOrWhiteSpace($Thumbprint)) {
        Fail-Stage "WINDOWS_SIGNING_PROVIDER=$normalizedProvider but WINDOWS_SIGNING_THUMBPRINT is empty — the $normalizedProvider provisioning step must run and export the certificate thumbprint before this script (see docs/windows-signing.md)."
    }

    Write-Stage "Locate signtool ($normalizedProvider)"
    $signtool = Find-SignTool
    if (-not $signtool) {
        Fail-Stage "signtool.exe not found (install Windows SDK signing tools on the runner)"
    }
    Write-Host "[$Stage] Using signtool: $signtool"
    Write-Host "[$Stage] Using $normalizedProvider certificate thumbprint=$Thumbprint"
    Close-Stage

    Invoke-ThumbprintSign -SignTool $signtool -SignThumbprint $Thumbprint -Paths $Path

    Write-Host "[$Stage] Authenticode signing complete (provider=$normalizedProvider)"
    return
}

Fail-Stage "unknown WINDOWS_SIGNING_PROVIDER '$Provider' (expected one of: (empty)/none, local-pfx, esigner, certum)"
