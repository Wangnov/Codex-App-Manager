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

The updater checks the signed root `…/manager/release-identity.json(.sig)` before
the unsigned `latest.json`, then falls back to GitHub. A mirror manifest is
accepted only after its version,
platform, artifact basename, release-note hash, updater signature, and SHA-256
root authority (also retained under `<version>/` for immutable audit/retry). This
keeps mainland-China discovery/download on the mirror without
letting the URL-rewritten (and therefore unsigned) `latest.json` invent a
candidate or replay an older signed package as a higher version.

## Why a separate latest.json on the mirror
`latest.json`'s embedded signatures sign the artifact **bytes**, not the URL, so
re-hosting the same files keeps them valid — we only rewrite the download URLs to
`…/manager/<version>/<file>`. `scripts/sync-mirror.sh` stages stable releases,
then verifies both storage endpoints and separately forces this Worker's R2 and
IHEP branches before the GitHub Release becomes immutable. Probe responses identify
their backend with `X-Codex-Mirror-Backend`; an explicit IHEP probe fails instead
of falling back when its Worker secrets are incomplete. The release repeats that
readback immediately before advancing the mirror's `latest.json` pointer.

Installers are uploaded under a **per-version** key (`<version>/<file>`) and
`latest.json` stays at the fixed root the updater polls. macOS updater tarballs
are renamed upstream to versionless arch-only names, so without the version
segment every release would reuse one URL and the worker's long installer cache
could serve a previous version's bytes against the new signature. (The seeded
v0.1.8 predates this and lives at flat `…/manager/<file>` keys — still
self-consistent; v0.1.9+ use the versioned layout.)

`release-identity.json` and `release-identity.json.sig` are also immutable
per-version objects. Release reruns reuse a previously verified signature, and
the sync script rejects a byte-different overwrite (the minisign trusted-comment
timestamp otherwise makes a freshly generated signature differ on every run).
After `latest.json` promotion, the root signature is written before the root JSON
and both are read back through R2 and IHEP. The Worker marks both root files
`no-store`; a mixed/interrupted pair fails verification and the client falls back
to GitHub until a retry converges it.

> ⚠️ The mirror pointers must be refreshed on every stable release. If they are
> stale or inconsistent, the mirror path is skipped and the client falls back to
> GitHub. That's why the `release.yml` stage, verify, and promote steps exist.

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
| `MANAGER_R2_PROMOTION_ACCESS_KEY_ID` / `MANAGER_R2_PROMOTION_SECRET_ACCESS_KEY` | R2 S3 API token for the current protected release workflow |
| `MANAGER_IHEP_S3_ENDPOINT` / `_BUCKET` environment variables | IHEP endpoint and bucket (`_REGION`/`_PREFIX` variables optional) |
| `MANAGER_IHEP_S3_PROMOTION_ACCESS_KEY_ID` / `MANAGER_IHEP_S3_PROMOTION_SECRET_ACCESS_KEY` | IHEP token for the current protected release workflow |

Both backends must already contain the same valid `latest.json` baseline and
preserve custom user metadata through `HeadObject`. R2 is the sole CAS authority
and must enforce conditional `PutObject` (`If-Match` / `If-None-Match`). IHEP is
the serialized unconditional follower; its ignored conditional headers are not a
promotion prerequisite. Delete the legacy access-key secret names after migration
so historical workflow revisions cannot perform their old unconditional write.

Deploy this Worker revision before enabling the protected release workflow. The
release gate intentionally rejects an older Worker that cannot force and identify
both public backend branches.

## Deploy / manual sync
```bash
wrangler deploy                         # from this directory
# manual re-sync of a tag's assets (download release → rewrite → upload):
#   rm -rf dist && gh release download vX.Y.Z -D dist && \
#   MANAGER_R2_S3_ENDPOINT=… MANAGER_R2_ACCESS_KEY_ID=… MANAGER_R2_SECRET_ACCESS_KEY=… \
#   MANAGER_IHEP_S3_ENDPOINT=… MANAGER_IHEP_S3_BUCKET=… \
#   MANAGER_IHEP_S3_ACCESS_KEY_ID=… MANAGER_IHEP_S3_SECRET_ACCESS_KEY=… \
#   bash ../../scripts/sync-mirror.sh dist
```
