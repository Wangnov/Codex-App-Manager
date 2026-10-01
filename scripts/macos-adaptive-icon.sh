#!/usr/bin/env bash
# Inject the macOS 26 (Tahoe) adaptive app icon into a built .app bundle.
#
# Tauri doesn't yet support the new .icon / Assets.car flow (tauri-apps/tauri#14979),
# so its bundler only ships a static .icns. This script compiles assets/icon.icon
# with actool and injects the result post-build, giving the app a Liquid Glass icon
# that follows the system appearance (Default / Dark / Clear / Tinted) on macOS 26.
# The static .icns stays in the bundle as a fallback for older macOS.
#
# Requires: Xcode 26+ (for actool). macOS only.
#
# Usage:
#   scripts/macos-adaptive-icon.sh [path/to/App.app]
# With no argument, it patches the release bundle under target.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
ICON_SRC="$ROOT/assets/icon.icon"

APP="${1:-$ROOT/target/release/bundle/macos/Codex App Manager.app}"

if [ ! -d "$ICON_SRC" ]; then
  echo "error: .icon source not found: $ICON_SRC" >&2
  exit 1
fi
if [ ! -d "$APP" ]; then
  echo "error: app bundle not found: $APP" >&2
  echo "  build it first (npm run tauri build) or pass the .app path explicitly." >&2
  exit 1
fi
# Prefer the prebuilt Assets.car vendored in the repo. GitHub's macOS runners
# ship an older Xcode whose actool can't compile the macOS 26 .icon format, so
# we compile it locally (Xcode 26+) and commit src-tauri/macos/Assets.car. Fall
# back to a live actool compile only when the prebuilt file is absent.
PREBUILT="$ROOT/src-tauri/macos/Assets.car"
if [ -f "$PREBUILT" ]; then
  echo "→ using prebuilt Assets.car ($PREBUILT)"
  cp "$PREBUILT" "$APP/Contents/Resources/Assets.car"
else
  if ! xcrun --find actool >/dev/null 2>&1; then
    echo "error: no prebuilt Assets.car and actool not found — install Xcode 26+." >&2
    exit 1
  fi
  echo "→ compiling $ICON_SRC with actool (needs Xcode 26+)"
  TMP="$(mktemp -d)"
  trap 'rm -rf "$TMP"' EXIT
  xcrun actool "$ICON_SRC" \
    --compile "$TMP" \
    --app-icon icon \
    --enable-on-demand-resources NO \
    --development-region en \
    --target-device mac --platform macosx \
    --minimum-deployment-target 11.0 \
    --output-partial-info-plist "$TMP/partial.plist" >/dev/null
  echo "→ injecting Assets.car into Resources/"
  cp "$TMP/Assets.car" "$APP/Contents/Resources/Assets.car"
fi

echo "→ patching Info.plist (CFBundleIconName = icon)"
PLIST="$APP/Contents/Info.plist"
plutil -replace CFBundleIconName -string icon "$PLIST"
plutil -replace CFBundleIconFile -string icon "$PLIST"

# The injection mutated the bundle, so any existing signature is now stale.
# Signing is intentionally delegated to scripts/sign-macos-app.sh (inside-out
# Developer ID) — run it AFTER this, BEFORE notarization, so there is a single
# source of truth for the distribution signature.
touch "$APP"  # nudge Finder/Dock to refresh the cached icon

echo "✓ adaptive icon injected into: $APP"
echo "  On macOS 26 the Dock/Finder icon now follows the system appearance."
echo "  NOTE: this script does NOT sign. For distribution, run next:"
echo "    scripts/sign-macos-app.sh \"$APP\" \"\$APPLE_SIGNING_IDENTITY\""
echo "    scripts/notarize-macos.sh \"$APP\""
echo "  If you ship a .dmg, re-bundle it after signing so its copy is correct too."
