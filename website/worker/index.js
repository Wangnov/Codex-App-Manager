// codexapp.agentsmirror.com site worker.
//
// Static assets (dist/) are served by Workers Static Assets before this code
// runs; only /api/* reaches the script (run_worker_first in wrangler.jsonc).
// /api/status.json exposes a small, read-only summary of the latest mirrored
// Codex release and the latest Codex App Manager release so the page can show
// live versions, sizes and hashes without cross-origin requests.

import { summarizeManifest } from "../site/manifest.mjs";

const TTL_SECONDS = 300;

export default {
  async fetch(request, env, ctx) {
    const url = new URL(request.url);
    if (url.pathname === "/api/status.json") return status(request, env, ctx);
    return env.ASSETS.fetch(request);
  },
};

async function status(request, env, ctx) {
  if (request.method !== "GET" && request.method !== "HEAD") {
    return new Response("Method Not Allowed", { status: 405, headers: { allow: "GET, HEAD" } });
  }
  const cache = caches.default;
  const key = new Request(new URL("/api/status.json", request.url).toString());
  let response = await cache.match(key);
  if (!response) {
    const body = await buildStatus(env);
    const ok = Boolean(body.codex || body.manager);
    response = new Response(JSON.stringify(body), {
      status: ok ? 200 : 503,
      headers: {
        "content-type": "application/json; charset=utf-8",
        "cache-control": ok ? `public, max-age=60, s-maxage=${TTL_SECONDS}` : "no-store",
        "x-content-type-options": "nosniff",
      },
    });
    if (ok) ctx.waitUntil(cache.put(key, response.clone()));
  }
  return request.method === "HEAD" ? new Response(null, response) : response;
}

async function buildStatus(env) {
  const [manifest, manager] = await Promise.allSettled([
    readJSON(env.MIRROR_BUCKET, "latest/manifest"),
    readJSON(env.MANAGER_BUCKET, "latest.json"),
  ]);
  const out = { generatedAt: new Date().toISOString() };
  if (manifest.status === "fulfilled" && manifest.value?.codexVersion) {
    out.codex = summarizeManifest(manifest.value);
  }
  if (manager.status === "fulfilled" && manager.value?.version) {
    out.manager = {
      version: String(manager.value.version).replace(/^v/, ""),
      publishedAt: manager.value.pub_date ?? null,
    };
  }
  return out;
}

async function readJSON(bucket, key) {
  if (!bucket) return null;
  const object = await bucket.get(key);
  return object ? object.json() : null;
}
