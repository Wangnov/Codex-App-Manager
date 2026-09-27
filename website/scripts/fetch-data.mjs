// Snapshots the live data the page renders at build time, so the static HTML
// is complete even when the runtime refresh (/api/status.json) is unavailable.
//
//   node scripts/fetch-data.mjs            # refresh JSON, download new previews
//   node scripts/fetch-data.mjs --previews # also re-download every preview
//
// Outputs: site/data/site.json (committed) and assets/raw/skins/*.webp
// (git-ignored; `npm run images` turns them into public/img/skins/*).

import { mkdir, open, rm, writeFile } from "node:fs/promises";
import sharp from "sharp";
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

// Everything that reaches disk is validated first: identifiers and hashes
// must match their formats, text is length-capped, numbers must be finite.
const ID = /^[a-z0-9][a-z0-9-]{0,80}$/;
const VERSION = /^\d+(\.\d+){1,3}$/;
const SHA256 = /^[0-9a-f]{64}$/;
const CATEGORIES = new Set(["anime", "guofeng", "games", "stars", "tech", "other"]);
const text = (v, max = 400) => (typeof v === "string" ? v.slice(0, max) : "");
const count = (v) => (Number.isFinite(v) && v >= 0 ? Math.floor(v) : null);
const isoDate = (v) => (typeof v === "string" && !Number.isNaN(Date.parse(v)) ? new Date(v).toISOString() : null);
const pick = (re, v) => (typeof v === "string" && re.test(v) ? v : null);

const skins = [];
for (const s of catalog.skins ?? []) {
  const id = pick(ID, s.id);
  const version = pick(VERSION, s.version);
  if (!id || !version) {
    console.warn(`skipping catalog entry with an unexpected id or version: ${JSON.stringify(s.id)}`);
    continue;
  }
  skins.push({
    id,
    name: text(s.name, 120),
    description: text(s.description, 400),
    version,
    category: CATEGORIES.has(s.category) ? s.category : "other",
    appearance: ["dual", "dark", "light"].includes(s.appearance) ? s.appearance : null,
    codexVerified: pick(VERSION, s.codexVerified),
    bytes: count(s.bytes),
    pack: `packs/${id}-${version}.codexskin`,
  });
}

const codex = summarizeManifest(manifest);
const cleanFile = (f) => (f ? { bytes: count(f.bytes), sha256: pick(SHA256, f.sha256) } : null);
const data = {
  generatedAt: new Date().toISOString(),
  codex: {
    version: pick(VERSION, codex.version),
    publishedAt: isoDate(codex.publishedAt),
    files: Object.fromEntries(Object.entries(codex.files).map(([k, f]) => [k, cleanFile(f)])),
  },
  manager: {
    version: pick(VERSION, String(managerRelease.tag_name ?? "").replace(/^v/, "")),
    publishedAt: isoDate(managerRelease.published_at),
  },
  stars: Object.fromEntries(REPOS.map((name, i) => [name, count(repos[i]?.stargazers_count) ?? 0])),
  downloads: pick(/^[\d.]+[kKmM]?$/, downloads),
  skins,
};
if (!data.codex.version || !data.manager.version) throw new Error("upstream versions failed validation");

await mkdir(path.dirname(DATA), { recursive: true });
await writeFile(DATA, JSON.stringify(data, null, 2) + "\n");
console.log(
  `site.json: codex ${data.codex.version}, manager ${data.manager.version}, ${skins.length} skins, downloads ${data.downloads}`
);

// Previews are decoded and re-encoded (which also proves they are images)
// before landing in assets/raw/skins. "wx" creates each file exactly once, so
// an existing preview is kept without a separate existence check.
await mkdir(PREVIEWS, { recursive: true });
let fetched = 0;
for (const s of skins) {
  const out = path.join(PREVIEWS, `${s.id}.webp`);
  if (refreshPreviews) await rm(out, { force: true });
  let handle;
  try {
    handle = await open(out, "wx");
  } catch (err) {
    if (err.code === "EEXIST") continue;
    throw err;
  }
  try {
    const res = await fetch(`${SKINS_ORIGIN}/previews/${s.id}.webp?v=${s.version}`);
    if (!res.ok) throw new Error(`preview ${s.id} -> HTTP ${res.status}`);
    const image = await sharp(Buffer.from(await res.arrayBuffer()), { limitInputPixels: 4096 * 4096 })
      .webp({ quality: 92 })
      .toBuffer();
    await handle.writeFile(image);
    fetched++;
  } catch (err) {
    await handle.close();
    await rm(out, { force: true });
    throw err;
  }
  await handle.close();
}
console.log(`previews: ${fetched} downloaded, ${skins.length - fetched} cached`);
