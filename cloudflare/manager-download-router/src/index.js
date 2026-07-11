// Manager update-artifact router for https://codexapp.agentsmirror.com/manager/*
//
// Global  → served directly from the bound R2 bucket (codex-app-manager).
// Mainland China (request.cf.country in SECONDARY_COUNTRY_CODES) → 302 to a
//          presigned IHEP S3 URL, when those secrets are configured.
//
// The path after /manager/ is the object key (e.g. /manager/latest.json →
// "latest.json"). Mirrors codex-app-mirror's download-router, but binds R2
// directly (self-contained, no extra public domain) and keys on /manager/.

const PREFIX = "/manager/";
const DEFAULT_SECONDARY_COUNTRY_CODES = "CN";
const DEFAULT_SIGNED_URL_TTL_SECONDS = 3600;

export default {
  async fetch(request, env) {
    const url = new URL(request.url);
    if (!url.pathname.startsWith(PREFIX)) {
      return new Response("Not found", { status: 404 });
    }
    let key = decodeURIComponent(url.pathname.slice(PREFIX.length));
    if (!key || key.endsWith("/") || key.includes("..")) {
      return new Response("Not found", { status: 404 });
    }

    // /manager/latest/<file> → rewrite to the current immutable versioned key
    // (latest/CodexAppManager_aarch64.dmg → 0.1.12/CodexAppManager_aarch64.dmg;
    // latest/CodexAppManager_arm64-setup.exe → 0.1.12/CodexAppManager_0.1.12_arm64-setup.exe)
    // so the README can link a permanent URL that never needs a version bump.
    // The rewrite is IN-PLACE (no redirect): still ONE Worker invocation, and
    // the resolved versioned object keeps its hard cache. Cost is one cheap R2
    // GET of latest.json per first-install click; self-update traffic (which
    // fetches latest.json + the .app.tar.gz directly) never reaches this path.
    if (key.startsWith("latest/")) {
      const version = await currentVersion(env);
      if (!version) return new Response("Not found", { status: 404 });
      key = `${version}/${withVersion(key.slice("latest/".length), version)}`;
    }

    const country = request.cf?.country || request.headers.get("CF-IPCountry") || "";
    const probeBackend = requestedProbeBackend(url);
    const secondaryCountryCodes = new Set(
      (env.SECONDARY_COUNTRY_CODES || DEFAULT_SECONDARY_COUNTRY_CODES)
        .split(",")
        .map((code) => code.trim().toUpperCase())
        .filter(Boolean),
    );

    // ── Mainland China: presign the IHEP S3 object and redirect ──────────────
    const secondaryConfigured = hasSecondaryS3Config(env);
    if (probeBackend === "ihep" && !secondaryConfigured) {
      return new Response("Secondary mirror is not configured", {
        status: 503,
        headers: { "X-Codex-Mirror-Backend": "ihep" },
      });
    }
    const useSecondary =
      probeBackend === "ihep" ||
      (probeBackend !== "r2" &&
        secondaryCountryCodes.has(country.toUpperCase()) &&
        secondaryConfigured);
    if (useSecondary) {
      const objectKey = objectKeyForKey(key, env.SECONDARY_S3_PREFIX || "");
      const signedUrl = await presignS3Url({
        // Sign for the actual method: the redirect preserves it, and an
        // S3 URL presigned for GET is rejected when followed with HEAD — the
        // R2 branch below answers HEAD directly, so keep CN symmetric.
        method: request.method === "HEAD" ? "HEAD" : "GET",
        endpoint: env.SECONDARY_S3_ENDPOINT,
        bucket: env.SECONDARY_S3_BUCKET,
        key: objectKey,
        region: env.SECONDARY_S3_REGION || "auto",
        accessKeyId: env.SECONDARY_S3_ACCESS_KEY_ID,
        secretAccessKey: env.SECONDARY_S3_SECRET_ACCESS_KEY,
        expiresInSeconds: ttlSeconds(env.SECONDARY_S3_SIGNED_URL_TTL_SECONDS),
        responseHeaders: {},
      });
      return redirect(signedUrl, "ihep");
    }

    // ── Everyone else: stream straight from the bound R2 bucket ──────────────
    const object = await env.BUCKET.get(key);
    if (!object) {
      return new Response("Not found", { status: 404 });
    }
    const headers = new Headers();
    object.writeHttpMetadata(headers); // Content-Type etc. from R2 metadata
    headers.set("ETag", object.httpEtag);
    headers.set("Cache-Control", cacheControlForKey(key));
    headers.set("X-Codex-Mirror-Backend", "r2");
    if (request.method === "HEAD") {
      return new Response(null, { headers });
    }
    return new Response(object.body, { headers });
  },
};

// The two root identity files are a coordinated signed pointer. Never cache
// either half: promotion writes the signature first and JSON last, and stale
// mixing must resolve to a quick verification failure/fallback instead of a
// day-long mirror outage. Other JSON refreshes quickly; immutable versioned
// installers and signatures can cache hard.
function cacheControlForKey(key) {
  if (key === "release-identity.json" || key === "release-identity.json.sig") {
    return "no-store";
  }
  if (key.endsWith(".json")) return "public, max-age=120, s-maxage=120";
  return "public, max-age=86400, s-maxage=86400";
}

function hasSecondaryS3Config(env) {
  return Boolean(
    env.SECONDARY_S3_ENDPOINT &&
      env.SECONDARY_S3_BUCKET &&
      env.SECONDARY_S3_ACCESS_KEY_ID &&
      env.SECONDARY_S3_SECRET_ACCESS_KEY,
  );
}

// Release verification must exercise both geographic branches from a single
// GitHub runner. This is a routing selector, not an authorization mechanism:
// every addressed object is already public through this Worker. Requiring the
// run-specific probe token shape prevents ordinary download links from changing
// backend accidentally while allowing CI to force and identify each branch.
function requestedProbeBackend(url) {
  const token = url.searchParams.get("cam_probe") || "";
  const backend = url.searchParams.get("cam_backend") || "";
  if (!/^[0-9a-f]{24}$/.test(token)) return null;
  return ["r2", "ihep"].includes(backend) ? backend : null;
}

function ttlSeconds(value) {
  const parsed = Number.parseInt(value || DEFAULT_SIGNED_URL_TTL_SECONDS, 10);
  if (!Number.isFinite(parsed)) return DEFAULT_SIGNED_URL_TTL_SECONDS;
  return Math.min(Math.max(parsed, 1), 604800);
}

function redirect(location, backend) {
  return new Response(null, {
    status: 302,
    headers: {
      Location: location,
      "Cache-Control": "private, no-store",
      "X-Codex-Mirror-Backend": backend,
    },
  });
}

function objectKeyForKey(key, prefix) {
  const cleanPrefix = prefix.replace(/^\/+|\/+$/g, "");
  return cleanPrefix ? `${cleanPrefix}/${key}` : key;
}

// Resolve the current release version from latest.json (root object on R2).
// Used only to rewrite /manager/latest/* — returns null on any miss so the
// caller 404s instead of guessing a version.
async function currentVersion(env) {
  try {
    const object = await env.BUCKET.get("latest.json");
    if (!object) return null;
    const manifest = await object.json();
    return typeof manifest.version === "string" && manifest.version ? manifest.version : null;
  } catch {
    return null;
  }
}

// Installers live under <version>/. The macOS .dmg names are version-less and
// pass straight through; the Windows NSIS .exe embeds the version in its file
// name, so insert it so a stable "latest" link resolves to the real object.
function withVersion(file, version) {
  const match = /^CodexAppManager_(x64|arm64)-setup\.exe$/.exec(file);
  return match ? `CodexAppManager_${version}_${match[1]}-setup.exe` : file;
}

// ── AWS SigV4 presigner (GET/HEAD), identical scheme to codex-app-mirror ─────
async function presignS3Url(options) {
  const endpointUrl = new URL(options.endpoint);
  const now = new Date();
  const amzDate = formatAmzDate(now);
  const dateStamp = amzDate.slice(0, 8);
  const credentialScope = `${dateStamp}/${options.region}/s3/aws4_request`;
  const signedHeaders = "host";
  const canonicalUri = canonicalS3Uri(endpointUrl, options.bucket, options.key);

  const queryParams = [
    ["X-Amz-Algorithm", "AWS4-HMAC-SHA256"],
    ["X-Amz-Credential", `${options.accessKeyId}/${credentialScope}`],
    ["X-Amz-Date", amzDate],
    ["X-Amz-Expires", String(options.expiresInSeconds)],
    ["X-Amz-SignedHeaders", signedHeaders],
    ...Object.entries(options.responseHeaders || {}),
  ];
  const canonicalQuery = canonicalQueryString(queryParams);
  const canonicalHeaders = `host:${endpointUrl.host}\n`;
  const canonicalRequest = [
    options.method || "GET",
    canonicalUri,
    canonicalQuery,
    canonicalHeaders,
    signedHeaders,
    "UNSIGNED-PAYLOAD",
  ].join("\n");
  const canonicalRequestHash = await sha256Hex(canonicalRequest);
  const stringToSign = ["AWS4-HMAC-SHA256", amzDate, credentialScope, canonicalRequestHash].join("\n");
  const signingKey = await signingKeyBytes(options.secretAccessKey, dateStamp, options.region, "s3");
  const signature = toHex(await hmac(signingKey, stringToSign));

  endpointUrl.pathname = canonicalUri;
  endpointUrl.search = `${canonicalQuery}&X-Amz-Signature=${signature}`;
  return endpointUrl.toString();
}

function canonicalS3Uri(endpointUrl, bucket, key) {
  const basePath = endpointUrl.pathname === "/" ? "" : endpointUrl.pathname.replace(/\/$/, "");
  const encodedKey = key.split("/").map(encodeRfc3986).join("/");
  return `${basePath}/${encodeRfc3986(bucket)}/${encodedKey}`;
}

function canonicalQueryString(params) {
  return params
    .map(([key, value]) => [encodeRfc3986(key), encodeRfc3986(value)])
    .sort(([lk, lv], [rk, rv]) => (lk === rk ? (lv < rv ? -1 : lv > rv ? 1 : 0) : lk < rk ? -1 : 1))
    .map(([key, value]) => `${key}=${value}`)
    .join("&");
}

function encodeRfc3986(value) {
  return encodeURIComponent(value).replace(
    /[!'()*]/g,
    (char) => `%${char.charCodeAt(0).toString(16).toUpperCase()}`,
  );
}

function formatAmzDate(date) {
  return date.toISOString().replace(/[:-]|\.\d{3}/g, "");
}

async function signingKeyBytes(secretAccessKey, dateStamp, region, service) {
  const dateKey = await hmac(utf8(`AWS4${secretAccessKey}`), dateStamp);
  const regionKey = await hmac(dateKey, region);
  const serviceKey = await hmac(regionKey, service);
  return hmac(serviceKey, "aws4_request");
}

async function sha256Hex(value) {
  const digest = await crypto.subtle.digest("SHA-256", utf8(value));
  return toHex(new Uint8Array(digest));
}

async function hmac(keyBytes, value) {
  const cryptoKey = await crypto.subtle.importKey("raw", keyBytes, { name: "HMAC", hash: "SHA-256" }, false, [
    "sign",
  ]);
  const signature = await crypto.subtle.sign("HMAC", cryptoKey, utf8(value));
  return new Uint8Array(signature);
}

function utf8(value) {
  return new TextEncoder().encode(value);
}

function toHex(bytes) {
  return [...bytes].map((byte) => byte.toString(16).padStart(2, "0")).join("");
}
