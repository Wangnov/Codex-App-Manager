// Post-build: pins every inline <script> of every page into the CSP, then
// verifies the security headers. Inline scripts are the JSON-LD block and the
// first-visit language redirect; their hashes change whenever they do.
//
//   node scripts/finalize-headers.mjs   (runs as part of npm run build)

import { createHash } from "node:crypto";
import { readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const dist = path.join(path.dirname(path.dirname(fileURLToPath(import.meta.url))), "dist");
const PLACEHOLDER = "__INLINE_SCRIPT_HASHES__";

async function htmlFiles(dir) {
  const out = [];
  for (const e of await readdir(dir, { withFileTypes: true })) {
    const p = path.join(dir, e.name);
    if (e.isDirectory()) out.push(...(await htmlFiles(p)));
    else if (e.name.endsWith(".html")) out.push(p);
  }
  return out;
}

const hashes = new Set();
for (const file of await htmlFiles(dist)) {
  const html = await readFile(file, "utf8");
  for (const [, body] of html.matchAll(/<script(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/gi)) {
    hashes.add(`'sha256-${createHash("sha256").update(body).digest("base64")}'`);
  }
}

const headersFile = path.join(dist, "_headers");
let headers = await readFile(headersFile, "utf8");
if (!headers.includes(PLACEHOLDER)) throw new Error("_headers is missing the inline script hash placeholder");
headers = headers.replace(PLACEHOLDER, [...hashes].sort().join(" "));
await writeFile(headersFile, headers);

for (const name of ["Content-Security-Policy", "Permissions-Policy", "Referrer-Policy", "X-Content-Type-Options", "X-Frame-Options"]) {
  if (!headers.includes(`${name}:`)) throw new Error(`website security header missing: ${name}`);
}
console.log(`pinned ${hashes.size} inline script hash(es) into dist/_headers`);
