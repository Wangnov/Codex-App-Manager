# Release & code-signing

## Release notes

每个版本的 release note 写在 `docs/releases/v<X.Y.Z>.md`,随版本号 bump 一起进发版 PR;
tag 推送后无权限的 `release-source.yml` 发出信号,默认分支的 `release.yml` 按 tag
名取用该文件作为 GitHub Release 正文,并自动追加
"What's Changed" 与 Full Changelog。文件缺失时回退到 `docs/releases/FALLBACK.md`
(安装表 + 升级说明),正文永远不会为空。写法与双语风格见 `docs/releases/TEMPLATE.md`。

Cross-platform release starts with the unprivileged
[`release-source.yml`](../.github/workflows/release-source.yml) tag signal. Its
successful `workflow_run` invokes the current default-branch
[`release.yml`](../.github/workflows/release.yml), which builds, signs,
notarizes, and publishes the GitHub Release with the Tauri updater manifest.
The signal has no checkout, secrets, environment, write permission, or artifact
handoff; all credentialed jobs remain in the trusted default-branch workflow.
The publish job normally consumes the full four-platform build matrix. Windows
tag jobs currently fail closed while the SignPath Foundation application and
trusted-build migration are pending, so a new tag cannot publish a partial or
unsigned release. See the [code signing policy](./code-signing-policy.md).

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

## Manager updater trust and size limits

`latest.json` contains provider-specific download URLs, so the R2/IHEP copy is
intentionally not byte-identical to GitHub's copy and is not itself a trust
anchor. `gen-updater-manifest.mjs` also emits a deterministic, URL-free
`release-identity.json` containing the channel, version, release-note hash, exact
target artifact names, and SHA-256 values. The release workflow signs that
identity with the existing Tauri updater key, publishes it as a GitHub Release
asset, stages immutable `<version>/` copies on R2/IHEP, and promotes a root
`release-identity.json(.sig)` pair only after the immutable GitHub Release and
mirror `latest.json` pointer are committed.

Every `latest.json` platform entry must include a lowercase 64-character
`sha256`. Fresh publication, immutable historical-release reuse, and mirror
verification all fail closed when that field is missing or malformed, or when
it differs from either the signed release identity or the local artifact bytes.
Mirror same-version identity comparisons include the channel, release-note hash,
and artifact digests, so a rerun cannot treat drifted notes or a byte-different
artifact claim as idempotent.
The one compatibility exception is a strictly older mirror baseline published
before this field existed: a fully bound newer candidate may migrate it once.
Legacy baselines are never accepted for same-version reuse or downgrade.

The client still checks the mainland-friendly mirror first. It bounded-fetches
and verifies that source's root identity **before** reading unsigned `latest.json`,
accepts only a signed `stable` channel, then requires the manifest version,
channel, notes, platform set, artifact basenames, and SHA-256 values to match.
This prevents an unsigned manifest from selecting an old/prerelease signed
identity. Artifact bytes then pass both the normal Tauri minisign check and the
signed SHA-256 before `Update::install`. Any missing, oversized, invalid, or
mismatched mirror response falls through to GitHub; installation still occurs at
most once.

Hard in-memory limits are 256 KiB for manifests/identity, 16 KiB for the identity
signature, and 64 MiB for updater artifacts. For comparison, v0.3.1's largest
updater artifact is about 10.9 MiB. The exact vendored
`tauri-plugin-updater` 2.10.1 patch and its upgrade checklist live in
[`vendor/tauri-plugin-updater-2.10.1/VENDORED.md`](../vendor/tauri-plugin-updater-2.10.1/VENDORED.md).
Manifest/identity checks have a 30-second request deadline; artifact downloads
also have 15-second connect, 30-second read-stall, and 15-minute total bounds.

Because Tauri's minisign trusted comment contains a timestamp, a rerun would
normally create a byte-different identity `.sig`. The workflow first reuses and
verifies an existing GitHub Release asset (then the mirror copy), and the mirror
sync refuses to overwrite byte-different versioned identity objects.

## Windows

Windows has no light/dark adaptive app icon (`.ico` is static) — the single
Default icon is used. NSIS installer + updater bundle are produced by `tauri
build`; no Apple-style finalize step.

Windows Authenticode is in an explicit **application/migration pending** state:

1. The project is applying to SignPath Foundation; approval, certificate
   issuance, and trusted-build project identifiers do not exist yet.
2. `release.yml` runs `assert-signpath-foundation-ready.ps1` before a Windows
   tag build. The script intentionally always fails and has no variable/secret
   bypass. Because publish depends on the complete matrix, this blocks the
   whole release rather than shipping an unsigned or macOS-only set.
3. No `WINDOWS_CERTIFICATE` PFX, password, thumbprint config, or local
   `signCommand` is supported. SignPath Foundation signs submitted trusted-build
   artifacts and requires per-request human approval; it is not a PFX provider.
4. A future PR must prove the NSIS packaging/signing order with real SignPath
   pilot artifacts, exclude direct signing of third-party plugins, and verify
   the final setup, installed app, uninstaller, timestamp, updater `.sig`, and
   mirror hashes before replacing the blocker.
5. The existing `required` Authenticode and packaged-smoke steps are a
   post-integration verification contract, not evidence that signing is active.
   Removing the blocker alone still leaves unsigned output failing closed.

PR-time x64 packaged smoke lives in
[`win-installer-check.yml`](../.github/workflows/win-installer-check.yml)
(`scripts/windows-packaged-smoke.ps1`: install → launch → upgrade → uninstall).
Required CI (`ci.yml`) also runs standalone engine crate tests for
`codex-mac-engine` and `codex-win-engine`.

## Mirror promotion safety

Stable releases use a stage/verify/promote, dual-backend protocol implemented by
[`scripts/mirror-release.mjs`](../scripts/mirror-release.mjs):

1. Before uploading anything immutable, cryptographically verify every local
   updater payload against its manifest signature and the updater public key in
   `tauri.conf.json`. The manifest signature must also equal the local `.sig`
   sidecar. This gate runs for prereleases as well as stable releases.
2. Upload immutable, versioned artifacts and a run-specific candidate manifest
   to both R2 and IHEP. A versioned object is never overwritten.
3. Before publishing the GitHub Release, download the candidate and **every staged
   artifact** directly from each S3 endpoint. Verify every file's byte size and
   SHA-256, plus each updater bundle's embedded Tauri/minisign signature with the
   public key in `tauri.conf.json`. This includes both DMG installers even though
   they are not referenced by the updater manifest. Stable promotion requires all
   four updater platforms and requires every manifest signature to match its
   downloaded `<artifact>.sig` sidecar; `ALLOW_PARTIAL_RELEASE` cannot weaken the
   mirror gate. The same verify-only phase forces separate downloads of the
   run-specific candidate and all four updater payloads through the public
   Worker's R2 and mainland-China IHEP branches. Each response must identify the
   requested backend. The IHEP `Location` must also be a complete SigV4 URL whose
   origin, bucket, prefix, and exact object path match the trusted release
   configuration; the verifier refuses any follow-on redirect. This catches bad
   routes, bucket bindings, Worker secondary credentials, and cross-store
   redirects before immutable publication.
4. Publish the draft GitHub Release and require GitHub to report that it is
   immutable with canonical asset digests.
5. Repeat the complete direct-backend and public-route readback immediately before
   promotion, then read each backend's current `latest.json` and compare semantic
   versions. Newer candidates advance, a fully converged same-version rerun is a
   no-write success, and older tags fail closed. If a hard-killed run left R2 on
   the candidate while IHEP still lags, the rerun first reclaims R2 with CAS.
6. Treat R2 as the only linearization authority. The workflow conditionally
   writes R2, verifies the committed ETag and promotion token, then writes IHEP
   unconditionally as a follower. A CAS loser never writes IHEP. The winner
   checks R2 ownership immediately before and after the follower write. If it is
   superseded during that window, it either preserves the newer follower or
   repairs only its own IHEP value from the newer stable R2 snapshot. If another
   repair already moved IHEP to the exact R2 candidate after the CAS, that
   follower is accepted. A higher version or any identity that cannot be proven
   canonical is preserved but fails closed; it cannot force the owned R2 CAS to
   roll back and it is never overwritten by the older run.
7. If IHEP fails, roll R2 back only while its committed ETag and token are still
   owned by this transaction. IHEP is restored only when it still contains this
   transaction's token and bytes; an unchanged baseline or a concurrent value is
   preserved rather than overwritten.

The release workflow has one repository-wide `release-latest-*` concurrency lane
with `queue: max`, so every pending tag remains queued instead of a third tag
replacing the second. This single-writer lane and the promotion-only credential
names are a correctness boundary because IHEP does not enforce conditional
writes. Do not run another credentialed promotion workflow outside this lane.
R2 CAS and the before/after ownership checks are defense in depth for accidental
overlap; they are not a claim of an atomic transaction across providers.
`mirror-stage-summary.json`,
`mirror-verification-summary.json`, and `mirror-promotion-summary.json` are shown
in the job summary and retained as workflow artifacts for 90 days. If a runner is
hard-killed after the R2 CAS but before IHEP follows, rerun that release or a newer
one: the new run reclaims R2 with CAS and converges IHEP. A hard kill during an
out-of-policy concurrent follower race can still leave IHEP temporarily stale;
rerunning the release currently authoritative in R2 is the recovery procedure.

Before `prepare` permits any build, and again at the start of every release-job
attempt, the workflow queries the repository Immutable Releases setting with a
dedicated fine-grained, read-only token. A missing token, failed query, or
`enabled: false` response fails closed before draft upload or mirror publication.
Two separate active tag rulesets restrict `refs/tags/v*` creation to the named
release publisher and forbid update/deletion for everyone (including that
publisher). The workflow also re-peels the live tag before publication. The
peeled commit must be an ancestor of the freshly fetched live default branch;
tagging an unmerged release-bump branch fails before build/signing and is checked
again in each build, release-job attempt, publication boundary, and final mirror
promotion. Every checkout after resolution uses that exact commit SHA, not the
tag name.

The `release` environment must allow the protected default branch only. Do not
allow `v*` deployment refs and do not duplicate release credentials as repository
or organization secrets: tag-triggered workflow definitions are read from the
tagged commit, whereas the credentialed `workflow_run` workflow is read from the
default branch. `release-source.yml` must stay an unprivileged signal and must
never download or pass artifacts.

Same-tag reruns reuse the artifacts, signed release identity, and `latest.json`
attached to a complete,
published GitHub Release only when GitHub reports `immutable: true` and a canonical
`sha256:` digest for every required asset. The workflow re-hashes every downloaded
asset against those API digests, and rejects a mutable release instead of calling
its current bytes canonical. Only a draft is repairable; a published mutable or
incomplete Release fails closed before any asset upload. The release job performs
this lookup live rather than
trusting `prepare` outputs from an earlier attempt, so **Re-run failed jobs** after
a mirror failure reuses the already-published immutable bytes. While a new or
partial release is still being repaired, the workflow selects the earliest
successful, attempt-qualified Actions artifact for each platform so every rerun
stages the same signed/notarized bytes. It never creates a second byte sequence for
an immutable version key. New releases are uploaded as drafts first and published
only after all assets succeed, including prereleases. Immediately after publish,
the workflow requires the Release itself to report `immutable: true` and canonical
digests for every required asset before any mirror pointer can advance.
Before reusing a failed attempt's mutable draft, the workflow deletes every
stale draft asset and proves the set is empty. After upload it requires an exact
name/size match against the local canonical files, downloads every draft asset
for byte-for-byte SHA-256 verification, and rechecks that the draft did not
change before publication. Immutable reuse rejects every asset outside the
fixed release/SBOM allowlist.
Each fresh release also publishes `release-binding.json` and a custom GitHub
attestation. `workflow_run` OIDC identifies `refs/heads/<default>` rather than the
tag, so verification separately pins the trusted signer workflow digest and the
default-branch source digest, then requires the signed predicate and attestation
subject set to match the target tag, peeled source SHA, and every canonical
subject digest exactly. Fresh bundles are self-verified before draft upload.
Immutable reuse downloads the API-digest-pinned binding and verifies both SLSA
provenance and this exact custom predicate. Both fresh publication and reuse also
require `gh release verify` to accept GitHub's immutable release attestation; an
old immutable release without the binding is deliberately not reusable.

Historical Actions runs execute their historical workflow revision, so the
release environment intentionally uses new `*_PROMOTION_*` credential names. The
four legacy access-key secrets listed below must be deleted; the current workflow
has a hard gate that rejects them if they are reintroduced.

Stable mirror promotion commits `latest.json` first, then writes the root
identity signature followed by its JSON. Any interrupted or mixed generation
fails verification and makes the client fall back to GitHub; a rerun repairs the
root pair and verifies both direct storage backends and both public Worker routes.

### Emergency mirror downgrade

A normal old-tag rerun can never downgrade the mirror. For an incident rollback:

1. Open the current `Release` workflow and dispatch it from the repository's
   **default branch** (never select the historical tag's workflow revision).
2. Set `target_tag` to the already-published `vX.Y.Z` release to restore.
3. Enable `allow_mirror_downgrade`.
4. Enter a concrete `mirror_downgrade_reason` (at least 10 characters).

The workflow downloads the target release's original assets instead of rebuilding
them. The target GitHub Release must be immutable and expose canonical SHA-256
digests; older mutable releases must first be migrated through an explicitly
reviewed process and cannot be used directly. The script rejects the override on
tag-push events or without GitHub actor/run metadata. The triggering actor (plus
the original workflow actor on a rerun), reason, whether the override was actually
used, and the run URL are written to the promotion audit and job summary.

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
| `IMMUTABLE_RELEASES_READ_TOKEN` | fine-grained token used only to read this repository's Immutable Releases setting; grant **Administration: read-only** and store it in the `release` environment |
| `MANAGER_R2_S3_ENDPOINT` | R2 S3-compatible endpoint |
| `MANAGER_R2_PROMOTION_ACCESS_KEY_ID` | R2 write/read credential used only by the protected workflow |
| `MANAGER_R2_PROMOTION_SECRET_ACCESS_KEY` | R2 write/read secret used only by the protected workflow |
| `MANAGER_IHEP_S3_PROMOTION_ACCESS_KEY_ID` | IHEP write/read credential used only by the protected workflow |
| `MANAGER_IHEP_S3_PROMOTION_SECRET_ACCESS_KEY` | IHEP write/read secret used only by the protected workflow |

There are currently **no Windows signing secrets or variables to configure**.
After Foundation approval, use only the exact organization/project/signing
policy/artifact-configuration values provisioned by SignPath, in a separately
reviewed trusted-build integration. Do not guess names and do not add a PFX
fallback.

After creating the promotion credentials, delete (and preferably revoke/rotate)
`MANAGER_R2_ACCESS_KEY_ID`, `MANAGER_R2_SECRET_ACCESS_KEY`,
`MANAGER_IHEP_S3_ACCESS_KEY_ID`, and `MANAGER_IHEP_S3_SECRET_ACCESS_KEY` from the
repository and `release` environment. Historical workflow revisions reference
those exact names; leaving any of them available defeats old-run isolation.

### Release variables

| Name | What |
|---|---|
| `MANAGER_R2_BUCKET` (repo **variable**) | R2 bucket; defaults to `codex-app-manager` |
| `MANAGER_IHEP_S3_ENDPOINT` (`release` environment **variable**) | IHEP S3-compatible endpoint |
| `MANAGER_IHEP_S3_BUCKET` (`release` environment **variable**) | IHEP bucket |
| `MANAGER_IHEP_S3_REGION` (`release` environment **variable**) | IHEP region; defaults to `auto` when empty |
| `MANAGER_IHEP_S3_PREFIX` (`release` environment **variable**) | optional object-key prefix for IHEP |

Export Apple `.p12` / `.p8` files to base64 with `base64 -i file -o -`.

> Artifact globs in the workflow's *Collect* step and the matcher regexes in
> `gen-updater-manifest.mjs` assume the default Tauri bundler output names —
> adjust them if your `productName`/bundler config changes the filenames.
>
> Keep **updater signature**, **Authenticode**, and **SmartScreen reputation**
> conceptually separate. Current Windows installers remain unsigned and new tag
> publication is blocked until the SignPath migration is proven. See
> [`docs/windows-signing.md`](./windows-signing.md), the
> [code signing policy](./code-signing-policy.md), and the
> [privacy policy](./privacy.md).
>
> Before enabling promotion, seed both S3-compatible endpoints with a valid
> `latest.json` baseline. R2 must enforce conditional `PutObject` requests
> (`If-Match` / `If-None-Match`) and preserve custom user metadata through
> `HeadObject`. IHEP must preserve metadata and support ordinary read/write, but
> it is explicitly allowed to ignore conditional headers because the workflow
> uses it only as the serialized unconditional follower. Promotion fails closed
> if either baseline is absent.
