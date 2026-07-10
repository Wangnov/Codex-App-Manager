# Release & code-signing

## Release notes

每个版本的 release note 写在 `docs/releases/v<X.Y.Z>.md`,随版本号 bump 一起进发版 PR;
tag 推送后 `release.yml` 按 tag 名取用该文件作为 GitHub Release 正文,并自动追加
"What's Changed" 与 Full Changelog。文件缺失时回退到 `docs/releases/FALLBACK.md`
(安装表 + 升级说明),正文永远不会为空。写法与双语风格见 `docs/releases/TEMPLATE.md`。

Cross-platform release is tag-driven via [`.github/workflows/release.yml`](../.github/workflows/release.yml).
Push a `v*` tag and CI builds, signs, notarizes, and publishes a GitHub Release
with the Tauri updater manifest.

## macOS signing pipeline

`tauri build` deliberately runs **without** a `signingIdentity`. The bundle is
finalized afterward, because the vendored Sparkle `BinaryDelta` helper must be
signed inside-out and notarization rejects an ad-hoc nested binary. The chain
(wrapped by [`scripts/finalize-macos.sh`](../scripts/finalize-macos.sh)):

1. **Adaptive icon** — [`macos-adaptive-icon.sh`](../scripts/macos-adaptive-icon.sh)
   compiles `assets/icon.icon` with `actool` into `Assets.car`, injects it, and
   sets `CFBundleIconName` so macOS 26 follows the system appearance
   (Default / Dark / Clear / Tinted). Older macOS falls back to the static `.icns`.
2. **Sign** — [`sign-macos-app.sh`](../scripts/sign-macos-app.sh) signs every
   nested Mach-O first, then the outer app, with hardened runtime + entitlements.
3. **Notarize + staple** — [`notarize-macos.sh`](../scripts/notarize-macos.sh)
   submits with `notarytool` (App Store Connect API key) and staples the ticket.
4. **Repackage** — the updater `.app.tar.gz` is rebuilt + re-signed and the dmg
   is rebuilt so every artifact carries the finalized, stapled app.

### Local finalize

```bash
npm run tauri build                 # or: npm run tauri:build:mac (build + icon)
export APPLE_SIGNING_IDENTITY="Developer ID Application: NAME (TEAMID)"
export AC_API_KEY_ID=... AC_API_ISSUER_ID=... AC_API_KEY=/path/AuthKey.p8
export TAURI_SIGNING_PRIVATE_KEY=...   # updater key (optional locally)
bash scripts/finalize-macos.sh aarch64-apple-darwin
```

Omit the `AC_API_*` vars to skip notarization for a quick local dev finalize.

## Windows

Windows has no light/dark adaptive app icon (`.ico` is static) — the single
Default icon is used. NSIS installer + updater bundle are produced by `tauri
build`; no Apple-style finalize step.

Post-build on the Windows matrix (see [`release.yml`](../.github/workflows/release.yml)):

1. **PE arch diagnostic** — `scripts/windows-pe-arch.ps1` records x64 vs ARM64
   machine types. ARM64 is cross-built on x64 runners; this is **not** runtime
   verification ([`docs/windows-signing.md`](./windows-signing.md)).
2. **Optional Authenticode** — `scripts/sign-windows-authenticode.ps1` signs the
   final `-setup.exe` when `WINDOWS_CERTIFICATE` is present; otherwise skips.
3. **Authenticode verify** — `scripts/verify-windows-authenticode.ps1` in
   `optional` mode by default; set `AUTHENTICODE_REQUIRED=true` to gate.
4. **Tauri updater `.sig`** — `npx tauri signer sign` (always required for
   Windows in-app update entries in `latest.json`).
5. **Collect final artifacts** — space-stripped names under `dist-artifacts/`,
   with an explicit check that both `-setup.exe` and `-setup.exe.sig` exist.

PR-time x64 packaged smoke lives in
[`win-installer-check.yml`](../.github/workflows/win-installer-check.yml)
(`scripts/windows-packaged-smoke.ps1`: install → launch → upgrade → uninstall).
Required CI (`ci.yml`) also runs standalone engine crate tests for
`codex-mac-engine` and `codex-win-engine`.

## Required GitHub Actions secrets

| Secret | What |
|---|---|
| `APPLE_CERTIFICATE` | base64 of your `Developer ID Application` **.p12** |
| `APPLE_CERTIFICATE_PASSWORD` | password for that .p12 |
| `APPLE_SIGNING_IDENTITY` | `Developer ID Application: NAME (TEAMID)` |
| `KEYCHAIN_PASSWORD` | any throwaway password for the temp keychain |
| `AC_API_KEY_ID` | App Store Connect API key id |
| `AC_API_ISSUER_ID` | App Store Connect issuer id |
| `AC_API_KEY_BASE64` | base64 of the `AuthKey_XXXX.p8` |
| `TAURI_SIGNING_PRIVATE_KEY` | updater private key |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | its password (empty if none) |

### Optional Windows Authenticode secrets / vars

| Name | What |
|---|---|
| `WINDOWS_CERTIFICATE` | base64 of OV/EV code-signing **.pfx** (release env) |
| `WINDOWS_CERTIFICATE_PASSWORD` | password for that .pfx |
| `AUTHENTICODE_REQUIRED` (repo **variable**) | `true` → fail release when PE is not `Valid` |
| `WINDOWS_TIMESTAMP_URL` (repo **variable**) | optional RFC3161 timestamp URL |

Export your local .p12 / .p8 / .pfx to base64 with `base64 -i file -o -`.

> Artifact globs in the workflow's *Collect* step and the matcher regexes in
> `gen-updater-manifest.mjs` assume the default Tauri bundler output names —
> adjust them if your `productName`/bundler config changes the filenames.
>
> Keep **updater signature**, **Authenticode**, and **SmartScreen reputation**
> conceptually separate — see [`docs/windows-signing.md`](./windows-signing.md).
