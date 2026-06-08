# manager-download-router

Cloudflare Worker that serves the **manager's own** self-update artifacts at
`https://codexapp.agentsmirror.com/manager/*`, so the in-app updater doesn't
depend on GitHub (slow/blocked for the mainland-China audience).

It mirrors the `codex-app-mirror` download-router's dual-backend design, but on
the manager's **own** bucket so the two never mix:

- **Global** → streamed directly from the bound R2 bucket `codex-app-manager`.
- **Mainland China** (`request.cf.country` ∈ `SECONDARY_COUNTRY_CODES`) → 302 to
  a presigned **IHEP S3** URL, once the `SECONDARY_S3_*` secrets are set. Until
  then CN also falls back to R2 (still far better than GitHub).

The updater (`src-tauri/tauri.conf.json`) already checks
`…/manager/latest.json` first, GitHub second — no app change needed.

## Why a separate latest.json on the mirror
`latest.json`'s embedded signatures sign the artifact **bytes**, not the URL, so
re-hosting the same files keeps them valid — we only rewrite the download URLs to
`…/manager/<version>/<file>`. `scripts/sync-mirror.sh` does this on every
(non-pre-)release.

Installers are uploaded under a **per-version** key (`<version>/<file>`) and
`latest.json` stays at the fixed root the updater polls. macOS updater tarballs
are renamed upstream to versionless arch-only names, so without the version
segment every release would reuse one URL and the worker's long installer cache
could serve a previous version's bytes against the new signature. (The seeded
v0.1.8 predates this and lives at flat `…/manager/<file>` keys — still
self-consistent; v0.1.9+ use the versioned layout.)

> ⚠️ The mirror's `latest.json` MUST be refreshed on every stable release, or the
> first endpoint serves a stale version and **blocks** updates. That's why the
> `release.yml` "Sync to CDN mirror" step exists.

## Already provisioned (done)
- R2 bucket `codex-app-manager` created.
- This worker deployed with route `codexapp.agentsmirror.com/manager/*` + the R2
  binding.
- v0.1.8 seeded (latest.json + installers) — the endpoint is live.

## Remaining setup (your part — S3 + secrets)

### 1. IHEP S3 (CN acceleration) — worker secrets
Create the manager's IHEP bucket, then set the worker's secrets so CN traffic is
presigned there:
```bash
cd cloudflare/manager-download-router
wrangler secret put SECONDARY_S3_ENDPOINT          # e.g. https://s3.ihep.ac.cn
wrangler secret put SECONDARY_S3_BUCKET
wrangler secret put SECONDARY_S3_ACCESS_KEY_ID
wrangler secret put SECONDARY_S3_SECRET_ACCESS_KEY
# optional: SECONDARY_S3_REGION (default "auto"), SECONDARY_S3_PREFIX
```

### 2. Release auto-sync — GitHub repo secrets
So each release uploads to the mirror (`release.yml` → `scripts/sync-mirror.sh`):

| Secret | Notes |
| --- | --- |
| `MANAGER_R2_S3_ENDPOINT` | `https://d39dc6c92d1c4cfde580bf13e946b616.r2.cloudflarestorage.com` |
| `MANAGER_R2_ACCESS_KEY_ID` / `MANAGER_R2_SECRET_ACCESS_KEY` | R2 S3 API token (can reuse the mirror's; account-scoped) |
| `MANAGER_IHEP_S3_ENDPOINT` / `_BUCKET` / `_ACCESS_KEY_ID` / `_SECRET_ACCESS_KEY` | IHEP creds (optional; `_REGION`/`_PREFIX` optional) |

R2 secrets missing → step warns and skips (mirror goes stale). IHEP missing →
CN just falls back to R2 via the worker.

## Deploy / manual sync
```bash
wrangler deploy                         # from this directory
# manual re-sync of a tag's assets (download release → rewrite → upload):
#   gh release download vX.Y.Z -D dist && cp -f dist/latest.json dist/ && \
#   MANAGER_R2_S3_ENDPOINT=… MANAGER_R2_ACCESS_KEY_ID=… MANAGER_R2_SECRET_ACCESS_KEY=… \
#   bash ../../scripts/sync-mirror.sh dist
```
