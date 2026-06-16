#!/usr/bin/env bash
# Publish the manager's release artifacts + a URL-rewritten latest.json to the
# CDN mirror, so in-app self-update works without depending on GitHub (which is
# slow/blocked for the mainland-China audience).
#
#   • R2 (global)  — uploaded when MANAGER_R2_* env vars are set.
#   • IHEP S3 (CN) — uploaded when MANAGER_IHEP_S3_* env vars are set.
#
# The worker at codexapp.agentsmirror.com/manager/* serves R2 globally and the
# presigned IHEP object for CN. The artifact bytes are identical to the GitHub
# release, so the signatures already embedded in latest.json stay valid — we
# only rewrite the download URLs.
#
# Usage: scripts/sync-mirror.sh <dist-dir>
#        MIRROR_PHASE=all     upload versioned assets + latest.json (default,
#                             the original one-step behavior)
#        MIRROR_PHASE=stage   upload versioned assets + latest.candidate.json
#        MIRROR_PHASE=promote health-check candidate, then publish latest.json
#
#        <dist-dir> holds the release assets + the GitHub latest.json produced
#        by gen-updater-manifest.mjs.
set -euo pipefail

dist="${1:?usage: sync-mirror.sh <dist-dir>}"
phase="${MIRROR_PHASE:-all}"
mirror_base="${MIRROR_BASE_URL:-https://codexapp.agentsmirror.com/manager}"

case "$phase" in
  all|stage|promote) ;;
  *)
    echo "::error::unsupported MIRROR_PHASE=$phase (expected all, stage, or promote)" >&2
    exit 2
    ;;
esac

if [ ! -f "$dist/latest.json" ]; then
  echo "::error::no latest.json in $dist" >&2
  exit 1
fi

# Installers live under a per-version path on the mirror (see below), so read the
# version once and reuse it for both the rewritten URLs and the upload keys.
version="$(node -e 'process.stdout.write(String(JSON.parse(require("fs").readFileSync(process.argv[1],"utf8")).version||""))' "$dist/latest.json")"
if [ -z "$version" ]; then
  echo "::error::latest.json has no version" >&2
  exit 1
fi

# 1) Mirror variant of latest.json: same signatures, URLs pointed at the mirror.
#    Each installer URL gets a /<version>/ segment so every release is an
#    immutable, uniquely-addressed object. The macOS updater tarballs are renamed
#    to versionless arch-only names upstream (CodexAppManager_<arch>.app.tar.gz),
#    so without this each release would reuse one URL — and the worker's long
#    installer cache could then serve a previous version's bytes against the new
#    latest.json signature, breaking self-update. latest.json itself stays at the
#    fixed root path the updater polls.
node -e '
  const fs = require("fs");
  const [dir, base, ver] = [process.argv[1], process.argv[2], process.argv[3]];
  const j = JSON.parse(fs.readFileSync(dir + "/latest.json", "utf8"));
  for (const k of Object.keys(j.platforms || {})) {
    const name = j.platforms[k].url.split("/").pop();
    j.platforms[k].url = `${base}/${ver}/${name}`;
  }
  fs.writeFileSync(dir + "/latest.mirror.json", JSON.stringify(j, null, 2) + "\n");
' "$dist" "$mirror_base" "$version"

content_type() {
  case "$1" in
    *.json) echo "application/json" ;;
    *.tar.gz) echo "application/gzip" ;;
    *.exe) echo "application/octet-stream" ;;
    *.dmg) echo "application/x-apple-diskimage" ;;
    *) echo "application/octet-stream" ;;
  esac
}

candidate_key="latest.candidate.json"
latest_key="latest.json"

# Upload every asset (+ the mirror latest.json) to one S3-compatible endpoint.
# Path-style addressing works for both R2 and IHEP.
upload_assets() { # endpoint bucket region access_key secret_key phase [prefix]
  local endpoint="$1" bucket="$2" region="$3" ak="$4" sk="$5" mode="$6" prefix="${7:-}"
  local f name key
  for f in "$dist"/*; do
    name="$(basename "$f")"
    [ "$name" = "latest.json" ] && continue          # GitHub variant — skip
    if [ "$name" = "latest.mirror.json" ]; then
      if [ "$mode" = "stage" ]; then
        key="$candidate_key"                         # candidate only; updater never polls it
      else
        key="$latest_key"                            # original one-step behavior
      fi
    else
      key="$version/$name"                           # immutable, per-version object
    fi
    [ -n "$prefix" ] && key="${prefix%/}/$key"
    AWS_ACCESS_KEY_ID="$ak" \
    AWS_SECRET_ACCESS_KEY="$sk" \
    AWS_DEFAULT_REGION="$region" \
      aws s3 cp "$f" "s3://$bucket/$key" \
        --endpoint-url "$endpoint" \
        --content-type "$(content_type "$name")" \
        --only-show-errors
    echo "  ↑ $key"
  done
}

upload_latest() { # endpoint bucket region access_key secret_key [prefix]
  local endpoint="$1" bucket="$2" region="$3" ak="$4" sk="$5" prefix="${6:-}"
  local key="$latest_key"
  [ -n "$prefix" ] && key="${prefix%/}/$key"
  AWS_ACCESS_KEY_ID="$ak" \
  AWS_SECRET_ACCESS_KEY="$sk" \
  AWS_DEFAULT_REGION="$region" \
    aws s3 cp "$dist/latest.mirror.json" "s3://$bucket/$key" \
      --endpoint-url "$endpoint" \
      --content-type "application/json" \
      --only-show-errors
  echo "  ↑ $key"
}

check_candidate_health() {
  local candidate_url="$mirror_base/$candidate_key"
  local candidate urls
  candidate="$(mktemp)"
  urls="$(mktemp)"
  CANDIDATE_TMP="$candidate"
  CANDIDATE_URLS_TMP="$urls"
  trap 'rm -f "$AWS_CONFIG_FILE" "${CANDIDATE_TMP:-}" "${CANDIDATE_URLS_TMP:-}"' EXIT

  echo "→ health-check $candidate_url"
  curl -fsSL --retry 3 --max-time 30 "$candidate_url" -o "$candidate"
  node - "$candidate" "$version" > "$urls" <<'NODE'
const fs = require("fs");
const [path, expectedVersion] = process.argv.slice(2);
const manifest = JSON.parse(fs.readFileSync(path, "utf8"));
if (manifest.version !== expectedVersion) {
  console.error(`candidate version ${manifest.version || "(missing)"} does not match ${expectedVersion}`);
  process.exit(1);
}
const urls = Object.values(manifest.platforms || {}).map((platform) => platform && platform.url).filter(Boolean);
if (urls.length === 0) {
  console.error("candidate has no platform URLs");
  process.exit(1);
}
for (const url of urls) console.log(url);
NODE

  while IFS= read -r url; do
    [ -n "$url" ] || continue
    echo "  HEAD $url"
    curl -fsSI --retry 3 --max-time 30 "$url" >/dev/null
  done < "$urls"
}

r2_configured() {
  [ -n "${MANAGER_R2_S3_ENDPOINT:-}" ] && [ -n "${MANAGER_R2_ACCESS_KEY_ID:-}" ] && [ -n "${MANAGER_R2_SECRET_ACCESS_KEY:-}" ]
}

ihep_configured() {
  [ -n "${MANAGER_IHEP_S3_ENDPOINT:-}" ] && [ -n "${MANAGER_IHEP_S3_BUCKET:-}" ] && [ -n "${MANAGER_IHEP_S3_ACCESS_KEY_ID:-}" ] && [ -n "${MANAGER_IHEP_S3_SECRET_ACCESS_KEY:-}" ]
}

sync_r2_assets() {
  if r2_configured; then
    echo "→ R2 (${MANAGER_R2_BUCKET:-codex-app-manager})"
    upload_assets "$MANAGER_R2_S3_ENDPOINT" "${MANAGER_R2_BUCKET:-codex-app-manager}" "auto" \
      "$MANAGER_R2_ACCESS_KEY_ID" "$MANAGER_R2_SECRET_ACCESS_KEY" "$phase"
  else
    if [ "${ALLOW_STALE_MIRROR:-}" = "1" ]; then
      echo "::warning::MANAGER_R2_* not set — skipped R2 mirror sync because ALLOW_STALE_MIRROR=1"
    else
      echo "::error::MANAGER_R2_* not set — refusing stable release with stale self-update mirror (set ALLOW_STALE_MIRROR=1 only for an intentional manual/pre-release bypass)" >&2
      exit 1
    fi
  fi
}

sync_ihep_assets() {
  if ihep_configured; then
    echo "→ IHEP S3 (${MANAGER_IHEP_S3_BUCKET})"
    upload_assets "$MANAGER_IHEP_S3_ENDPOINT" "$MANAGER_IHEP_S3_BUCKET" "${MANAGER_IHEP_S3_REGION:-auto}" \
      "$MANAGER_IHEP_S3_ACCESS_KEY_ID" "$MANAGER_IHEP_S3_SECRET_ACCESS_KEY" "$phase" "${MANAGER_IHEP_S3_PREFIX:-}"
  else
    echo "IHEP S3 not configured — CN falls back to R2 via the worker"
  fi
}

promote_r2_latest() {
  if r2_configured; then
    echo "→ R2 promote (${MANAGER_R2_BUCKET:-codex-app-manager})"
    upload_latest "$MANAGER_R2_S3_ENDPOINT" "${MANAGER_R2_BUCKET:-codex-app-manager}" "auto" \
      "$MANAGER_R2_ACCESS_KEY_ID" "$MANAGER_R2_SECRET_ACCESS_KEY"
  else
    if [ "${ALLOW_STALE_MIRROR:-}" = "1" ]; then
      echo "::warning::MANAGER_R2_* not set — skipped R2 latest promote because ALLOW_STALE_MIRROR=1"
    else
      echo "::error::MANAGER_R2_* not set — cannot promote latest.json" >&2
      exit 1
    fi
  fi
}

promote_ihep_latest() {
  if ihep_configured; then
    echo "→ IHEP S3 promote (${MANAGER_IHEP_S3_BUCKET})"
    upload_latest "$MANAGER_IHEP_S3_ENDPOINT" "$MANAGER_IHEP_S3_BUCKET" "${MANAGER_IHEP_S3_REGION:-auto}" \
      "$MANAGER_IHEP_S3_ACCESS_KEY_ID" "$MANAGER_IHEP_S3_SECRET_ACCESS_KEY" "${MANAGER_IHEP_S3_PREFIX:-}"
  else
    echo "IHEP S3 not configured — CN falls back to R2 via the worker"
  fi
}

# Force path-style addressing via a temp config — the AWS CLI ignores an
# AWS_S3_ADDRESSING_STYLE env var, and both R2 and IHEP need path-style here.
export AWS_EC2_METADATA_DISABLED=true
AWS_CONFIG_FILE="$(mktemp)"
export AWS_CONFIG_FILE
printf '[default]\nregion = auto\ns3 =\n    addressing_style = path\n' > "$AWS_CONFIG_FILE"
trap 'rm -f "$AWS_CONFIG_FILE"' EXIT

case "$phase" in
  all|stage)
    sync_r2_assets
    sync_ihep_assets
    echo "✓ mirror $phase done"
    ;;
  promote)
    if r2_configured; then
      check_candidate_health
    elif [ "${ALLOW_STALE_MIRROR:-}" = "1" ]; then
      echo "::warning::skipped mirror health check because MANAGER_R2_* is not set and ALLOW_STALE_MIRROR=1"
    else
      echo "::error::MANAGER_R2_* not set — cannot health-check or promote latest.json" >&2
      exit 1
    fi
    promote_ihep_latest
    promote_r2_latest
    echo "✓ mirror promote done"
    ;;
esac
