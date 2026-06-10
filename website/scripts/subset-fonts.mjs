// Subsets the self-hosted display fonts to exactly the glyphs the site uses.
// Source Han Serif SC is ~24 MB; the subset ships at a few dozen KB.
//
//   node scripts/subset-fonts.mjs
//
// Inputs : assets/fonts-src/*.otf|ttf  (downloaded, git-ignored)
// Outputs: public/fonts/*.woff2

import { readFile, writeFile, mkdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import subsetFont from "subset-font";

const root = path.dirname(path.dirname(fileURLToPath(import.meta.url)));
const SRC = path.join(root, "assets/fonts-src");
const OUT = path.join(root, "public/fonts");

// Every file whose text can end up rendered in the display face.
const TEXT_SOURCES = [
  "index.html",
  "src/locales/zh.ts",
  "src/locales/en.ts",
];

const ASCII = Array.from({ length: 95 }, (_, i) => String.fromCharCode(32 + i)).join("");
// CJK punctuation & typographic marks used by the design even if a locale
// string is edited later.
const SAFETY = "「」『』《》〈〉、。,;:?!·—…··％℃①②③→←↑↓×✓✕“”‘’";

async function collectText() {
  let chars = new Set((ASCII + SAFETY).split(""));
  for (const rel of TEXT_SOURCES) {
    const body = await readFile(path.join(root, rel), "utf8");
    for (const ch of body) {
      if (ch.charCodeAt(0) > 0x2000) chars.add(ch);
    }
  }
  return Array.from(chars).join("");
}

async function subset(srcFile, outFile, text) {
  const buf = await readFile(path.join(SRC, srcFile));
  const woff2 = await subsetFont(buf, text, { targetFormat: "woff2" });
  await writeFile(path.join(OUT, outFile), woff2);
  console.log(
    `${outFile}: ${(buf.length / 1024 / 1024).toFixed(1)} MB -> ${(woff2.length / 1024).toFixed(1)} KB`
  );
}

await mkdir(OUT, { recursive: true });
const text = await collectText();
console.log(`glyph set: ${text.length} chars`);

await subset("SourceHanSerifSC-Heavy.otf", "shs-heavy.woff2", text);
await subset("SourceHanSerifSC-Bold.otf", "shs-bold.woff2", text);
// Fraunces only ever renders Latin.
await subset("Fraunces-VF.ttf", "fraunces.woff2", ASCII + "“”‘’—–…·");
