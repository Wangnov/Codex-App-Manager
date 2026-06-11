// Generates the repo README banner (assets/banner.svg) from the website's
// visual assets: night-sky backdrop, porcelain cloud, travelling byte orb,
// and the display type converted to vector outlines (no font dependency).
//
//   node scripts/readme-banner.mjs [out.svg]

import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { createRequire } from "node:module";
const fontkit = createRequire(import.meta.url)("fontkit");
import sharp from "sharp";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const OUT = process.argv[2] ?? path.join(root, "..", "assets", "banner.svg");

const b64 = (buf, mime) => `data:${mime};base64,${buf.toString("base64")}`;
const asset = (rel) => readFileSync(path.join(root, rel));

/* ---- embedded bitmaps (website pipeline output) ------------------------- */
const bgWebp = b64(asset("public/img/hero-dark-960.webp"), "image/webp");
const cloudWebp = b64(asset("public/img/cloud-640.webp"), "image/webp");
const orbWebp = b64(asset("public/img/orb-512.webp"), "image/webp");
const logoWebp = b64(
  await sharp(path.join(root, "public/img/logo-manager-192.png"))
    .resize(96)
    .webp({ quality: 88 })
    .toBuffer(),
  "image/webp"
);

/* ---- display type -> outlines ------------------------------------------- */
const heavy = fontkit.openSync(path.join(root, "assets/fonts-src/SourceHanSerifSC-Heavy.otf"));
const bold = fontkit.openSync(path.join(root, "assets/fonts-src/SourceHanSerifSC-Bold.otf"));

function outlines(font, text, x, y, size, attrs = "") {
  const scale = size / font.unitsPerEm;
  const run = font.layout(text);
  let cx = x;
  const parts = [];
  for (let i = 0; i < run.glyphs.length; i++) {
    const g = run.glyphs[i];
    const p = run.positions[i];
    const d = g.path.toSVG();
    if (d) {
      const gx = (cx + p.xOffset * scale).toFixed(2);
      const gy = (y - p.yOffset * scale).toFixed(2);
      parts.push(
        `<path transform="translate(${gx} ${gy}) scale(${scale.toFixed(5)} ${(-scale).toFixed(5)})" d="${d}"/>`
      );
    }
    cx += p.xAdvance * scale;
  }
  return { svg: `<g ${attrs}>${parts.join("")}</g>`, width: cx - x };
}

const wordmark = outlines(heavy, "Codex App Manager", 196, 152, 64, 'fill="#f5f4fb"');
const tagline = outlines(bold, "一键装好官方 Codex,持续最新", 112, 226, 31, 'fill="url(#tagGrad)"');

/* ---- journey path (orb travels it) --------------------------------------- */
const ROUTE = "M 70,386 C 240,366 360,344 490,352 C 640,361 700,376 810,368 C 950,358 1060,338 1148,322";

const svg = `<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 1400 420" role="img" aria-labelledby="title desc" font-family="-apple-system, BlinkMacSystemFont, 'Segoe UI', 'PingFang SC', 'Microsoft YaHei', system-ui, sans-serif">
  <title id="title">Codex App Manager</title>
  <desc id="desc">官方 Codex 桌面应用,一键装好,持续最新 — install, update and uninstall the official Codex desktop app, with mirrors reachable from mainland China.</desc>

  <defs>
    <clipPath id="frame"><rect width="1400" height="420" rx="24"/></clipPath>
    <linearGradient id="veilX" x1="0" y1="0" x2="1" y2="0">
      <stop offset="0" stop-color="#101019" stop-opacity="0.84"/>
      <stop offset="0.46" stop-color="#101019" stop-opacity="0.42"/>
      <stop offset="1" stop-color="#101019" stop-opacity="0.08"/>
    </linearGradient>
    <linearGradient id="veilY" x1="0" y1="0" x2="0" y2="1">
      <stop offset="0.55" stop-color="#0c0c14" stop-opacity="0"/>
      <stop offset="1" stop-color="#0c0c14" stop-opacity="0.45"/>
    </linearGradient>
    <linearGradient id="tagGrad" x1="0" y1="0" x2="1" y2="0">
      <stop offset="0" stop-color="#ecbf7a"/>
      <stop offset="1" stop-color="#b9bcf2"/>
    </linearGradient>
    <linearGradient id="routeGrad" x1="0" y1="0" x2="1" y2="0">
      <stop offset="0" stop-color="#d99e4d"/>
      <stop offset="1" stop-color="#ecbf7a"/>
    </linearGradient>
    <filter id="soft" x="-60%" y="-60%" width="220%" height="220%">
      <feGaussianBlur stdDeviation="9"/>
    </filter>
    <filter id="softer" x="-80%" y="-80%" width="260%" height="260%">
      <feGaussianBlur stdDeviation="16"/>
    </filter>
  </defs>

  <g clip-path="url(#frame)">
    <!-- night sky from the website hero -->
    <image href="${bgWebp}" x="0" y="-330" width="1400" height="788" preserveAspectRatio="xMidYMid slice"/>
    <rect width="1400" height="420" fill="url(#veilX)"/>
    <rect width="1400" height="420" fill="url(#veilY)"/>

    <!-- vector bokeh, gently breathing -->
    <g>
      <circle cx="1010" cy="84" r="13" fill="#ecbf7a" opacity="0.4" filter="url(#soft)">
        <animate attributeName="opacity" values="0.4;0.65;0.4" dur="5.2s" repeatCount="indefinite"/>
      </circle>
      <circle cx="1180" cy="58" r="9" fill="#9aa0ee" opacity="0.45" filter="url(#soft)">
        <animate attributeName="opacity" values="0.45;0.2;0.45" dur="6.4s" repeatCount="indefinite"/>
      </circle>
      <circle cx="1330" cy="150" r="17" fill="#9aa0ee" opacity="0.3" filter="url(#softer)">
        <animate attributeName="opacity" values="0.3;0.55;0.3" dur="7.6s" repeatCount="indefinite"/>
      </circle>
      <circle cx="920" cy="180" r="7" fill="#ecbf7a" opacity="0.35" filter="url(#soft)">
        <animate attributeName="opacity" values="0.35;0.6;0.35" dur="4.6s" repeatCount="indefinite"/>
      </circle>
      <circle cx="1265" cy="240" r="11" fill="#ecbf7a" opacity="0.3" filter="url(#softer)">
        <animate attributeName="opacity" values="0.3;0.5;0.3" dur="8.4s" repeatCount="indefinite"/>
      </circle>
    </g>

    <!-- porcelain cloud, floating -->
    <g>
      <animateTransform attributeName="transform" type="translate" values="0 0; 0 -8; 0 0" dur="7s" repeatCount="indefinite" calcMode="spline" keySplines="0.45 0 0.55 1; 0.45 0 0.55 1"/>
      <image href="${cloudWebp}" x="1090" y="74" width="250" height="250"/>
    </g>

    <!-- the journey: faint track, lit route, pulsing nodes, travelling byte -->
    <path d="${ROUTE}" fill="none" stroke="#f0eff7" stroke-opacity="0.16" stroke-width="1.6" stroke-dasharray="3 9" stroke-linecap="round"/>
    <path d="${ROUTE}" fill="none" stroke="url(#routeGrad)" stroke-opacity="0.9" stroke-width="3" stroke-linecap="round" stroke-dasharray="90 994">
      <animate attributeName="stroke-dashoffset" values="90;-994" dur="7s" repeatCount="indefinite"/>
    </path>

    <g stroke="#ecbf7a" fill="none">
      <circle cx="490" cy="352" r="5.5" fill="#15151d" stroke-width="1.8"/>
      <circle cx="810" cy="368" r="5.5" fill="#15151d" stroke-width="1.8"/>
      <circle cx="490" cy="352" r="6" stroke-opacity="0.8" stroke-width="1.4">
        <animate attributeName="r" values="6;20" dur="3.5s" repeatCount="indefinite"/>
        <animate attributeName="stroke-opacity" values="0.8;0" dur="3.5s" repeatCount="indefinite"/>
      </circle>
      <circle cx="810" cy="368" r="6" stroke-opacity="0.8" stroke-width="1.4">
        <animate attributeName="r" values="6;20" dur="3.5s" begin="1.7s" repeatCount="indefinite"/>
        <animate attributeName="stroke-opacity" values="0.8;0" dur="3.5s" begin="1.7s" repeatCount="indefinite"/>
      </circle>
    </g>

    <!-- destination: your desktop -->
    <g transform="translate(1148 322)" stroke="#66d9b0" stroke-width="2" fill="none" stroke-linecap="round" stroke-linejoin="round">
      <rect x="-22" y="-30" width="44" height="30" rx="4"/>
      <path d="M -30 8 H 30"/>
      <path d="M -8 -16 L -3 -10 L 8 -22" stroke-opacity="0.9"/>
    </g>

    <!-- the byte -->
    <g>
      <animateMotion path="${ROUTE}" dur="7s" repeatCount="indefinite" calcMode="linear"/>
      <circle r="17" fill="#ecbf7a" opacity="0.35" filter="url(#soft)"/>
      <image href="${orbWebp}" x="-21" y="-21" width="42" height="42"/>
    </g>

    <!-- lockup -->
    <image href="${logoWebp}" x="112" y="86" width="64" height="64"/>
    ${wordmark.svg}
    ${tagline.svg}
    <text x="114" y="262" font-size="16" fill="#b8b6c9">Install, update &amp; uninstall the official Codex desktop app — verified byte by byte.</text>

    <!-- fact chips -->
    <g font-size="12.5" fill="#cac8d9">
      <rect x="112" y="286" width="120" height="28" rx="14" fill="#f0eff7" fill-opacity="0.07" stroke="#f0eff7" stroke-opacity="0.16"/>
      <text x="172" y="304" text-anchor="middle" fill="#cac8d9">macOS 增量更新</text>
      <rect x="244" y="286" width="112" height="28" rx="14" fill="#f0eff7" fill-opacity="0.07" stroke="#f0eff7" stroke-opacity="0.16"/>
      <text x="300" y="304" text-anchor="middle" fill="#cac8d9">EdDSA 校验</text>
      <rect x="368" y="286" width="142" height="28" rx="14" fill="#f0eff7" fill-opacity="0.07" stroke="#f0eff7" stroke-opacity="0.16"/>
      <text x="439" y="304" text-anchor="middle" fill="#cac8d9">R2 + IHEP 双镜像</text>
      <rect x="522" y="286" width="100" height="28" rx="14" fill="#ecbf7a" fill-opacity="0.12" stroke="#ecbf7a" stroke-opacity="0.4"/>
      <text x="572" y="304" text-anchor="middle" fill="#ecbf7a">国内直连</text>
    </g>

    <text x="1336" y="402" text-anchor="end" font-size="13" fill="#8e8da1">codexapp.agentsmirror.com</text>

    <rect width="1400" height="420" rx="24" fill="none" stroke="#f0eff7" stroke-opacity="0.1"/>
  </g>
</svg>
`;

writeFileSync(OUT, svg);
console.log(`${OUT}: ${(svg.length / 1024).toFixed(0)} KB · wordmark ${wordmark.width.toFixed(0)}px · tagline ${tagline.width.toFixed(0)}px`);
