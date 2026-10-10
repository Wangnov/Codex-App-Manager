# macOS delta (incremental) updates

Codex App Manager can update an installed Codex.app using a small Sparkle
**delta** (~18–40 MB) instead of the full package (~406 MB) when the installed
build matches one the appcast offers a delta from. The client side is already
complete and proven; this doc covers the two build-time pieces that make a
shipped CAM build delta-capable, plus how it ties into the mirror.

## How it works end-to-end

1. The **mirror** republishes OpenAI's Sparkle appcast. For each arch it copies
   OpenAI's full `.zip` **and** the per-arch `<sparkle:deltas>` enclosures
   verbatim — same bytes, same `sparkle:edSignature`. The mirror has no OpenAI
   EdDSA key, so it never runs `BinaryDelta` and never re-signs anything; it only
   rewrites enclosure URLs to point at the mirror.
2. CAM's client (`codex-mac-engine`) parses the appcast, and when the installed
   build matches a delta's `sparkle:deltaFrom`, it downloads that delta, verifies
   its EdDSA signature against the pinned key, and reconstructs the new bundle
   with the vendored `BinaryDelta` tool. **Any** failure (tool missing, modified
   basis, version skew) falls back to the full enclosure in the same appcast item
   — a delta failure never breaks an update.
3. The reconstructed bundle passes a `codesign` gate (Developer ID, Team
   `2DC432GLL2`, notarized) before the atomic same-volume swap.

So a delta-capable CAM build needs exactly two things baked in: the `BinaryDelta`
tool (C1) and a correctly signed nested copy of it (C2).

## C1 — vendor `BinaryDelta`

```sh
./scripts/vendor-binary-delta.sh
```

- Downloads **Sparkle 2.9.1** (matches the Sparkle framework version inside
  OpenAI's Codex.app, so it reads OpenAI's delta patch format v4.2 exactly),
  verifies the release tarball **and** the extracted binary against pinned
  SHA-256 values, and installs the **universal** (x86_64 + arm64) `bin/BinaryDelta`
  to `src-tauri/resources/BinaryDelta`.
- The binary is **not committed** (`.gitignore`); re-run the script on a fresh
  checkout / CI before building. It's idempotent (skips when the bytes match).
- Sparkle ships `BinaryDelta` already universal and code-signed, so the script
  copies it **as-is** — it deliberately does **not** `lipo`, which would rewrite
  the Mach-O and strip the signature.
- `tauri.conf.json` bundles `resources/*`, so the tool lands in the app at
  `Contents/Resources/…/BinaryDelta`. The client resolves it via the Tauri
  resource dir (or a `CODEX_BINARY_DELTA` override for testing).

Non-macOS builds: the script is a no-op (delta is macOS-only; the client falls
back to full updates when the tool is absent).

## C2 — sign the nested helper

A Mach-O helper under `Contents/Resources/` must be signed **inside-out** (helper
first, then the app) or `codesign --verify --deep --strict` rejects the bundle
and notarization rejects an adhoc nested binary inside a Developer ID app.

```sh
# Local / dev (adhoc) — verifies --deep --strict passes with the helper present:
./scripts/sign-macos-app.sh "target/release/bundle/macos/Codex App Manager.app"

# Distribution — Developer ID + hardened runtime + entitlements, then notarize:
CAM_SIGN_IDENTITY="Developer ID Application: <NAME> (2DC432GLL2)" \
  ./scripts/sign-macos-app.sh "path/to/Codex App Manager.app"
```

For a **distribution** build, build without a tauri `signingIdentity` (so the
bundle comes out adhoc), then run `sign-macos-app.sh` with the Developer ID
identity: it signs the nested `BinaryDelta` (hardened runtime + secure
timestamp), then the outer app (same, plus `src-tauri/entitlements.macos.plist`
— `allow-jit` for WKWebView), then verifies. Notarize + staple afterward.

## Mirror coupling

The mirror work that produces the delta enclosures lives in `codex-app-mirror`
(probe captures deltas → download mirrors `.delta` → build-appcast emits the
`<sparkle:deltas>` block with mirror URLs + official signatures → sync uploads
`.delta` before the appcast → prune keep-list protects live `.delta` basenames).
Until the mirror serves deltas, CAM simply uses full updates; once it does, no
client change is required — a delta-capable build just starts using them.
