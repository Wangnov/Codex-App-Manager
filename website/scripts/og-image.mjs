// Social cards (public/img/og.jpg for /, og-en.jpg for /en/), composed from the
// site's own renditions: cloud-sea backdrop, a real skin screenshot and the
// real Manager window. Text is converted to vector outlines (Mona Sans for
// Latin, Source Han Sans for CJK) so no browser or system font is needed.
//
//   node scripts/og-image.mjs   (runs after optimize-images in npm run images)

import { readFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
import sharp from "sharp";
import subsetFont from "subset-font";

const fontkit = createRequire(import.meta.url)("fontkit");
const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const IMG = path.join(root, "public/img");
const W = 1200;
const H = 630;

const monaSource = readFileSync(path.join(root, "public/fonts/mona-sans.woff2"));
const shsHeavy = fontkit.openSync(path.join(root, "assets/fonts-src/SourceHanSansCN-Heavy.otf"));
const shsBold = fontkit.openSync(path.join(root, "assets/fonts-src/SourceHanSansCN-Bold.otf"));
const isCJK = (ch) => /[\u2e80-\u9fff\uf900-\ufaff\ufe30-\ufe4f\uff00-\uffef]/.test(ch);
const ASCII = Array.from({ length: 95 }, (_, i) => String.fromCharCode(32 + i)).join("");

// fontkit cannot instance a variable WOFF2, so harfbuzz (subset-font) pins
// the Mona Sans axes into a static TrueType face first.
const instances = new Map();
async function monaFace(wght, wdth) {
  const key = `${wght}/${wdth}`;
  if (!instances.has(key)) {
    const ttf = await subsetFont(monaSource, ASCII + "“”‘’…·", { targetFormat: "truetype", variationAxes: { wght, wdth } });
    instances.set(key, fontkit.create(ttf));
  }
  return instances.get(key);
}

/** Lays out one line, switching faces between Latin and CJK runs. */
async function line(text, x, y, size, { weight = 800, stretch = 100, fill = "#fff" } = {}) {
  const latin = await monaFace(weight, stretch);
  const cjk = weight >= 800 ? shsHeavy : shsBold;
  const runs = [];
  for (const ch of text) {
    const face = isCJK(ch) ? cjk : latin;
    const last = runs[runs.length - 1];
    if (last && last.face === face) last.text += ch;
    else runs.push({ face, text: ch });
  }
  let cx = x;
  const parts = [];
  for (const { face, text: t } of runs) {
    const scale = size / face.unitsPerEm;
    const run = face.layout(t);
    run.glyphs.forEach((g, i) => {
      const p = run.positions[i];
      const d = g.path.toSVG();
      if (d) {
        parts.push(
          `<path transform="translate(${(cx + p.xOffset * scale).toFixed(2)} ${(y - p.yOffset * scale).toFixed(2)}) scale(${scale.toFixed(5)} ${(-scale).toFixed(5)})" d="${d}"/>`
        );
      }
      cx += p.xAdvance * scale;
    });
  }
  return `<g fill="${fill}">${parts.join("")}</g>`;
}

const CARDS = {
  zh: {
    file: "og.jpg",
    brand: "Codex App 全家桶",
    h1: ["官方 Codex 桌面版，", "装好、管好、穿好。"],
    h1Size: 60,
    sub: ["原样镜像国内直连，一键安装增量更新，50 款真机皮肤"],
    url: "codexapp.agentsmirror.com",
    top: 92,
  },
  en: {
    file: "og-en.jpg",
    brand: "Codex App Suite",
    h1: ["Get Codex.", "Keep it fresh.", "Make it yours."],
    h1Size: 56,
    sub: ["A verbatim mirror, a one-click manager", "and 50 real-app skins."],
    url: "codexapp.agentsmirror.com/en",
    top: 70,
  },
};

async function rounded(file, width, radius) {
  const img = sharp(file).resize({ width });
  const { height } = await img.clone().toBuffer({ resolveWithObject: true }).then((r) => r.info);
  const mask = Buffer.from(`<svg width="${width}" height="${height}"><rect width="${width}" height="${height}" rx="${radius}"/></svg>`);
  return { buf: await img.composite([{ input: mask, blend: "dest-in" }]).png().toBuffer(), height };
}

for (const [lang, c] of Object.entries(CARDS)) {
  const bg = await sharp(path.join(IMG, "hero-dark-1672.webp"))
    .resize(W, H, { fit: "cover", position: "right" })
    .toBuffer();
  const win = await rounded(path.join(IMG, "skins/journey-to-west-1280.webp"), 620, 12);
  const mgr = await sharp(path.join(IMG, `manager/compact-dark-${lang}-560.webp`)).resize({ width: 190 }).toBuffer();

  const h1Top = c.top + (lang === "zh" ? 62 : 52);
  const lines = await Promise.all(c.h1.map((t, i) => line(t, 64, h1Top + c.h1Size * (1 + i * 1.14), c.h1Size, { weight: 880, stretch: 108, fill: "#f3f4ff" })));
  const y = h1Top + c.h1Size * (1 + (c.h1.length - 1) * 1.14) + 48;
  const subs = await Promise.all(c.sub.map((t, i) => line(t, 64, y + i * 30, 21, { weight: 500, fill: "#c4c8ea" })));
  const brand = await line(c.brand, 122, c.top + 30, 26, { weight: 760, stretch: 104, fill: "#f3f4ff" });
  const url = await line(c.url, 64, H - 48, 18, { weight: 600, fill: "#9aa0d8" });

  const overlay = Buffer.from(`<svg xmlns="http://www.w3.org/2000/svg" width="${W}" height="${H}">
    <defs>
      <linearGradient id="x" x1="0" y1="0" x2="1" y2="0">
        <stop offset="0" stop-color="#090a14" stop-opacity="0.86"/>
        <stop offset="0.52" stop-color="#090a14" stop-opacity="0.35"/>
        <stop offset="0.75" stop-color="#090a14" stop-opacity="0"/>
      </linearGradient>
      <linearGradient id="y" x1="0" y1="0" x2="0" y2="1">
        <stop offset="0.6" stop-color="#090a14" stop-opacity="0"/>
        <stop offset="1" stop-color="#090a14" stop-opacity="0.6"/>
      </linearGradient>
      <filter id="s" x="-20%" y="-20%" width="140%" height="160%"><feGaussianBlur stdDeviation="22"/></filter>
    </defs>
    <rect width="${W}" height="${H}" fill="url(#x)"/>
    <rect width="${W}" height="${H}" fill="url(#y)"/>
    <rect x="660" y="160" width="600" height="360" rx="16" fill="#000" opacity="0.55" filter="url(#s)"/>
    ${brand}
    ${lines.join("")}
    ${subs.join("")}
    ${url}
  </svg>`);

  const logo = await sharp(path.join(IMG, "logo-manager-192.png")).resize(44).toBuffer();
  const out = path.join(IMG, c.file);
  await sharp(bg)
    .composite([
      { input: overlay, left: 0, top: 0 },
      { input: logo, left: 64, top: c.top },
      { input: win.buf, left: 640, top: 118 },
      { input: Buffer.from(`<svg width="620" height="${win.height}"><rect x="0.5" y="0.5" width="619" height="${win.height - 1}" rx="12" fill="none" stroke="#fff" stroke-opacity="0.14"/></svg>`), left: 640, top: 118 },
      { input: mgr, left: 585, top: 300 },
    ])
    .jpeg({ quality: 84, mozjpeg: true })
    .toFile(out);
  console.log(`${c.file} written`);
}

// Keep the generated card referenced by the page (sanity check for renames).
for (const f of ["og.jpg", "og-en.jpg"]) readFileSync(path.join(IMG, f));
