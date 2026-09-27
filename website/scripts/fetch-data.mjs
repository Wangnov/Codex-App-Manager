// Snapshots the live data the page renders at build time, so the static HTML
// is complete even when the runtime refresh (/api/status.json) is unavailable.
//
//   node scripts/fetch-data.mjs            # refresh JSON, download new previews
//   node scripts/fetch-data.mjs --previews # also re-download every preview
//
// Outputs: site/data/site.json (committed) and assets/raw/skins/*.webp
// (git-ignored; `npm run images` turns them into public/img/skins/*).

import { mkdir, writeFile } from "node:fs/promises";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { summarizeManifest } from "../site/manifest.mjs";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const DATA = path.join(root, "site/data/site.json");
const PREVIEWS = path.join(root, "assets/raw/skins");
const refreshPreviews = process.argv.includes("--previews");

const SKINS_ORIGIN = "https://skins.agentsmirror.com";
const MIRROR_MANIFEST = "https://codexapp-r2.agentsmirror.com/latest/manifest";
const REPOS = ["Wangnov/codex-app-mirror", "Wangnov/Codex-App-Manager", "Wangnov/awesome-codex-skins"];

async function getJSON(url, headers = {}) {
  const res = await fetch(url, { headers: { "user-agent": "codexapp-website-build", ...headers } });
  if (!res.ok) throw new Error(`${url} -> HTTP ${res.status}`);
  return res.json();
}

const gh = (p) =>
  getJSON(`https://api.github.com/${p}`, {
    accept: "application/vnd.github+json",
    ...(process.env.GITHUB_TOKEN ? { authorization: `Bearer ${process.env.GITHUB_TOKEN}` } : {}),
  });

const [catalog, manifest, managerRelease, ...repos] = await Promise.all([
  getJSON(`${SKINS_ORIGIN}/index.json`),
  getJSON(MIRROR_MANIFEST),
  gh("repos/Wangnov/Codex-App-Manager/releases/latest"),
  ...REPOS.map((r) => gh(`repos/${r}`)),
]);

let downloads = null;
try {
  downloads = (await getJSON("https://codexapp.agentsmirror.com/stats/downloads.json")).message ?? null;
} catch (err) {
  console.warn(`downloads badge unavailable: ${err.message}`);
}

const skins = catalog.skins.map((s) => ({
  id: s.id,
  name: s.name,
  description: s.description,
  version: s.version,
  category: s.category ?? "other",
  appearance: s.appearance ?? null,
  codexVerified: s.codexVerified ?? null,
  bytes: s.bytes,
  pack: s.pack,
}));

const data = {
  generatedAt: new Date().toISOString(),
  codex: summarizeManifest(manifest),
  manager: {
    version: managerRelease.tag_name.replace(/^v/, ""),
    publishedAt: managerRelease.published_at,
  },
  stars: Object.fromEntries(repos.map((r) => [r.full_name, r.stargazers_count])),
  downloads,
  skins,
};

await mkdir(path.dirname(DATA), { recursive: true });
await writeFile(DATA, JSON.stringify(data, null, 2) + "\n");
console.log(
  `site.json: codex ${data.codex.version}, manager ${data.manager.version}, ${skins.length} skins, downloads ${downloads}`
);

await mkdir(PREVIEWS, { recursive: true });
let fetched = 0;
for (const s of catalog.skins) {
  const out = path.join(PREVIEWS, `${s.id}.webp`);
  if (existsSync(out) && !refreshPreviews) continue;
  const res = await fetch(`${SKINS_ORIGIN}/${s.preview}`);
  if (!res.ok) throw new Error(`preview ${s.id} -> HTTP ${res.status}`);
  await writeFile(out, Buffer.from(await res.arrayBuffer()));
  fetched++;
}
console.log(`previews: ${fetched} downloaded, ${catalog.skins.length - fetched} cached`);
