// Subsets the self-hosted fonts to exactly the glyphs the site renders.
//
//   node scripts/subset-fonts.mjs
//
// Inputs : assets/fonts-src/SourceHanSansCN-{Heavy,Bold}.otf (git-ignored,
//          Adobe Source Han Sans, OFL) and the Fontsource packages for Mona Sans
//          and Monaspace Neon (OFL, devDependencies).
// Outputs: public/fonts/*.woff2
//
// CJK only renders in display type (headings, numbers), so only those strings
// feed the Source Han subsets; body copy uses the system CJK face.

import { readFile, writeFile, mkdir, rm } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import subsetFont from "subset-font";
import { displayStrings } from "../site/render.mjs";
import zh from "../site/locales/zh.mjs";
import en from "../site/locales/en.mjs";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const OUT = path.join(root, "public/fonts");
const SRC = path.join(root, "assets/fonts-src");
const pkg = (p) => path.join(root, "node_modules", p);

const ASCII = Array.from({ length: 95 }, (_, i) => String.fromCharCode(32 + i)).join("");
const LATIN_EXTRA = "“”‘’…·×→←↗↓✓✕•–";
const CJK_SAFETY = "，。、；：？！「」『』（）《》·…—0123456789";

function cjkChars() {
  const set = new Set(CJK_SAFETY);
  for (const dict of [zh, en]) {
    for (const s of displayStrings(dict)) for (const ch of s) if (ch.codePointAt(0) > 0x2e7f) set.add(ch);
  }
  return [...set].join("");
}

async function subset(src, out, text, opts = {}) {
  const buf = await readFile(src);
  const woff2 = await subsetFont(buf, text, { targetFormat: "woff2", ...opts });
  await writeFile(path.join(OUT, out), woff2);
  console.log(`${out.padEnd(22)} ${(buf.length / 1024).toFixed(0).padStart(6)} KB -> ${(woff2.length / 1024).toFixed(1)} KB`);
}

await rm(OUT, { recursive: true, force: true });
await mkdir(OUT, { recursive: true });

const cjk = cjkChars();
console.log(`display CJK glyphs: ${cjk.length}`);
await subset(path.join(SRC, "SourceHanSansCN-Heavy.otf"), "shs-heavy.woff2", cjk);
await subset(path.join(SRC, "SourceHanSansCN-Bold.otf"), "shs-bold.woff2", cjk);

// Mona Sans keeps both variation axes (wght 200-900, wdth 75-125%).
await subset(
  pkg("@fontsource-variable/mona-sans/files/mona-sans-latin-standard-normal.woff2"),
  "mona-sans.woff2",
  ASCII + LATIN_EXTRA
);
for (const w of [400, 600]) {
  await subset(
    pkg(`@fontsource/monaspace-neon/files/monaspace-neon-latin-${w}-normal.woff2`),
    `monaspace-${w}.woff2`,
    ASCII + LATIN_EXTRA
  );
}
