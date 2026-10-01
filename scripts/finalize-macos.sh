#!/usr/bin/env bash
# One-shot macOS distribution finalize for a freshly `tauri build`-ed bundle.
# Chains the whole post-build pipeline so CI (and you, locally) call ONE script:
#
#   1. inject the macOS 26 adaptive (Liquid Glass) icon      (adaptive-icon.sh)
#   2. inside-out Developer ID code-sign + hardened runtime  (sign-macos-app.sh)
#   3. notarize + staple                                     (notarize-macos.sh)
#   4. repackage the updater tarball (+ re-sign) from the finalized app
#   5. repackage the dmg so its embedded .app is the finalized, stapled one
#
# Run on macOS AFTER:  npm run tauri build [-- --target <triple>]
#
# Env (all via CI secrets / local env — never committed):
#   APPLE_SIGNING_IDENTITY   "Developer ID Application: NAME (TEAMID)"
#   AC_API_KEY_ID / AC_API_ISSUER_ID / AC_API_KEY   App Store Connect API key
#   TAURI_SIGNING_PRIVATE_KEY[_PASSWORD]            updater signing key
# If AC_API_* is unset, notarization is skipped (handy for local dev finalizes).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="${1:-}"   # optional rust target triple, e.g. aarch64-apple-darwin
BUNDLE="$ROOT/target/${TARGET:+$TARGET/}release/bundle"

APP="$(/usr/bin/find "$BUNDLE/macos" -maxdepth 1 -name '*.app' 2>/dev/null | head -1)"
[[ -d "$APP" ]] || { echo "no .app under $BUNDLE/macos — build first" >&2; exit 1; }
NAME="$(basename "$APP" .app)"
log() { printf '\033[32m[finalize]\033[0m %s\n' "$*"; }

# 1) adaptive icon  2) sign  3) notarize/staple
bash "$ROOT/scripts/macos-adaptive-icon.sh" "$APP"
bash "$ROOT/scripts/sign-macos-app.sh" "$APP" "${APPLE_SIGNING_IDENTITY:--}"
if [[ -n "${AC_API_KEY_ID:-}" ]]; then
  bash "$ROOT/scripts/notarize-macos.sh" "$APP"
else
  log "AC_API_* not set — skipping notarization (dev finalize)"
fi

# 4) repackage updater tarball + signature from the finalized app
if [[ -n "${TAURI_SIGNING_PRIVATE_KEY:-}" ]]; then
  TAR="$BUNDLE/macos/$NAME.app.tar.gz"
  tar -C "$BUNDLE/macos" -czf "$TAR" "$NAME.app"
  npx tauri signer sign "$TAR" >/dev/null
  log "updater tarball repacked + re-signed: ${TAR##*/}"
else
  log "TAURI_SIGNING_PRIVATE_KEY not set — leaving updater tarball untouched"
fi

# 5) swap the finalized app into Tauri's dmg, PRESERVING its drag-to-Applications
#    layout (Applications symlink, window/.DS_Store icon positions, volume icon).
#    A plain `hdiutil create -srcfolder app` would drop all of that and ship a
#    bare .app with no Applications target — not a real installer dmg.
DMG="$(/usr/bin/find "$BUNDLE/dmg" -maxdepth 1 -name '*.dmg' 2>/dev/null | head -1 || true)"
if [[ -n "$DMG" ]]; then
  RW="$(mktemp -u).dmg"
  hdiutil convert "$DMG" -format UDRW -o "$RW" >/dev/null
  MNT="$(mktemp -d)"
  hdiutil attach "$RW" -nobrowse -noverify -mountpoint "$MNT" >/dev/null
  rm -rf "$MNT/$NAME.app"
  cp -R "$APP" "$MNT/$NAME.app"
  hdiutil detach "$MNT" >/dev/null
  rm -f "$DMG"
  hdiutil convert "$RW" -format UDZO -o "$DMG" >/dev/null
  rm -f "$RW"
  codesign --force --sign "${APPLE_SIGNING_IDENTITY:--}" "$DMG" 2>/dev/null || true
  log "dmg repacked (Tauri layout kept, finalized app swapped in): ${DMG##*/}"
fi

log "done — every macOS artifact now carries the signed, stapled, adaptive-icon app."
