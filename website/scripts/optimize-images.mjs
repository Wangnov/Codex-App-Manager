// Converts the raw AI-generated assets (assets/raw, git-ignored) into the
// optimized renditions the site actually ships (public/img).
//
//   node scripts/optimize-images.mjs
//
// 4K originals never ship: every output is resized + converted to AVIF/WebP.

import { mkdir, stat } from "node:fs/promises";
import { existsSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import sharp from "sharp";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const RAW = path.join(root, "assets/raw");
const OUT = path.join(root, "public/img");

const jobs = [
  // Opaque atmospheres: AVIF (primary) + WebP (fallback), responsive widths.
  { src: "hero-bg-dark.png",  name: "hero-dark",  widths: [2560, 1600, 960], formats: ["avif", "webp"] },
  { src: "hero-bg-light.png", name: "hero-light", widths: [2560, 1600, 960], formats: ["avif", "webp"] },
  { src: "texture-dark.png",  name: "texture-dark",  widths: [1280], formats: ["webp"], quality: 62 },
  { src: "texture-light.png", name: "texture-light", widths: [1280], formats: ["webp"], quality: 62 },

  // Transparent layers: WebP keeps alpha at a fraction of PNG size.
  { src: "cloud-porcelain.png", name: "cloud",  widths: [1200, 640], formats: ["webp"] },
  { src: "mist-band.png",       name: "mist",   widths: [1600, 960], formats: ["webp"] },
  { src: "bokeh-field.png",     name: "bokeh",  widths: [1600, 960], formats: ["webp"] },
  { src: "orb-node.png",        name: "orb",    widths: [512],       formats: ["webp"] },
  { src: "arc-globe.png",       name: "globe",  widths: [800],       formats: ["webp"] },
];

await mkdir(OUT, { recursive: true });

for (const job of jobs) {
  const src = path.join(RAW, job.src);
  if (!existsSync(src)) {
    console.warn(`SKIP (missing): ${job.src}`);
    continue;
  }
  for (const width of job.widths) {
    const base = sharp(src).resize({ width, withoutEnlargement: true });
    for (const fmt of job.formats) {
      const file = path.join(OUT, `${job.name}-${width}.${fmt}`);
      const pipe = base.clone();
      if (fmt === "avif") await pipe.avif({ quality: 52 }).toFile(file);
      else await pipe.webp({ quality: job.quality ?? 76, alphaQuality: 90 }).toFile(file);
      const { size } = await stat(file);
      console.log(`${path.basename(file)} ${(size / 1024).toFixed(0)} KB`);
    }
  }
}

// Social card: 1200x630 center crop of the dark hero.
await sharp(path.join(RAW, "hero-bg-dark.png"))
  .resize(1200, 630, { fit: "cover", position: "centre" })
  .jpeg({ quality: 78 })
  .toFile(path.join(OUT, "og.jpg"));
console.log("og.jpg done");
