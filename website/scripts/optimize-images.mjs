// Turns the raw inputs (assets/raw, git-ignored) into the renditions the site
// ships (public/img). Sources:
//   bg-*.png, obj-*.png  images generated with gpt-image-2-skill (Codex provider)
//   skins/*.webp         real screenshots from skins.agentsmirror.com (fetch-data)
//   manager/*.png        Codex App Manager UI captured from its browser preview
//   logo-mirror.png      codex-app-mirror's assets/logo.png
//
//   node scripts/optimize-images.mjs

import { mkdir, rm, stat } from "node:fs/promises";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import sharp from "sharp";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const RAW = path.join(root, "assets/raw");
const OUT = path.join(root, "public/img");
const data = JSON.parse(readFileSync(path.join(root, "site/data/site.json"), "utf8"));
const { HERO_SKINS } = await import("../site/config.mjs");

let total = 0;
async function emit(pipeline, file, fmt, opts = {}) {
  const out = path.join(OUT, file);
  await mkdir(path.dirname(out), { recursive: true });
  const p = pipeline.clone();
  if (fmt === "avif") await p.avif({ quality: opts.q ?? 50, effort: 6 }).toFile(out);
  else if (fmt === "jpeg") await p.jpeg({ quality: opts.q ?? 80, mozjpeg: true }).toFile(out);
  else await p.webp({ quality: opts.q ?? 78, alphaQuality: 92, effort: 6 }).toFile(out);
  const { size } = await stat(out);
  total += size;
  if (!opts.quiet) console.log(`${file.padEnd(46)} ${(size / 1024).toFixed(0).padStart(5)} KB`);
}

async function job(src, name, widths, formats, opts = {}) {
  const file = path.join(RAW, src);
  if (!existsSync(file)) throw new Error(`missing raw asset: ${src}`);
  for (const w of widths) {
    const base = sharp(file).resize({ width: w, withoutEnlargement: true });
    for (const fmt of formats) await emit(base, `${name}-${w}.${fmt}`, fmt, opts);
  }
}

// Start clean so renamed or retired renditions never linger in the bundle.
await rm(OUT, { recursive: true, force: true });
await mkdir(OUT, { recursive: true });

// Brand marks are the real repository logos, resized once.
const repoRoot = path.dirname(root);
await sharp(path.join(repoRoot, "assets/logo.png")).resize(192).png().toFile(path.join(OUT, "logo-manager-192.png"));
await sharp(path.join(RAW, "logo-mirror.png")).resize(192).png().toFile(path.join(OUT, "logo-mirror-192.png"));
await sharp(path.join(repoRoot, "assets/sponsor-duckcoding.jpg")).resize(128).webp({ quality: 86 }).toFile(path.join(OUT, "sponsor-duckcoding.webp"));

// Atmospheres (opaque): AVIF first, WebP fallback.
for (const theme of ["dark", "light"]) {
  await job(`bg-hero-${theme}.png`, `hero-${theme}`, [960, 1672], ["avif", "webp"], { q: 52 });
  await job(`bg-lake-${theme}.png`, `lake-${theme}`, [960, 1672], ["avif", "webp"], { q: 52 });
}

// Glossy objects (transparent).
for (const obj of ["cloud", "sync", "cards", "shield", "globe"]) {
  await job(`obj-${obj}.png`, `obj/${obj}`, [320, 640], ["webp"], { q: 82 });
}

// Manager UI captures (transparent window chrome), per theme and language.
for (const theme of ["dark", "light"]) {
  for (const lang of ["zh", "en"]) {
    await job(`manager/compact-update-${theme}-${lang}.png`, `manager/compact-${theme}-${lang}`, [560], ["webp"], { q: 84 });
    await job(`manager/workbench-home-${theme}-${lang}.png`, `manager/home-${theme}-${lang}`, [1200, 2256], ["webp"], { q: 82 });
    await job(`manager/workbench-store-${theme}-${lang}.png`, `manager/store-${theme}-${lang}`, [1200, 2256], ["webp"], { q: 80 });
  }
}

// Skin gallery thumbnails, plus full-size frames and switcher dots for the hero.
for (const s of data.skins) {
  const src = path.join(RAW, "skins", `${s.id}.webp`);
  if (!existsSync(src)) throw new Error(`missing preview for ${s.id}; run npm run data`);
  await emit(sharp(src).resize({ width: 640 }), `skins/${s.id}-640.webp`, "webp", { q: 74, quiet: true });
}
for (const id of HERO_SKINS) {
  const src = sharp(path.join(RAW, "skins", `${id}.webp`));
  await emit(src, `skins/${id}-1280.avif`, "avif", { q: 56 });
  await emit(src, `skins/${id}-1280.webp`, "webp", { q: 80 });
  await emit(src.clone().extract({ left: 880, top: 120, width: 400, height: 400 }).resize(96, 96), `skins/${id}-dot.webp`, "webp", { q: 80, quiet: true });
}

console.log(`total ${(total / 1024 / 1024).toFixed(1)} MB`);
