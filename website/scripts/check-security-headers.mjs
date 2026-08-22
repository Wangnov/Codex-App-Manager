import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";

const [html, headers] = await Promise.all([
  readFile(new URL("../dist/index.html", import.meta.url), "utf8"),
  readFile(new URL("../dist/_headers", import.meta.url), "utf8"),
]);

const requiredHeaders = [
  "Content-Security-Policy",
  "Permissions-Policy",
  "Referrer-Policy",
  "X-Content-Type-Options",
  "X-Frame-Options",
];
for (const name of requiredHeaders) {
  if (!headers.includes(`${name}:`)) {
    throw new Error(`website security header missing: ${name}`);
  }
}

const csp = headers
  .split("\n")
  .find((line) => line.trimStart().startsWith("Content-Security-Policy:"));
if (!csp) {
  throw new Error("website Content-Security-Policy is missing");
}

const inlineScripts = [...html.matchAll(/<script(?![^>]*\bsrc=)[^>]*>([\s\S]*?)<\/script>/gi)];
for (const [, body] of inlineScripts) {
  const hash = `sha256-${createHash("sha256").update(body).digest("base64")}`;
  if (!csp.includes(`'${hash}'`)) {
    throw new Error(`website CSP is missing inline script hash: ${hash}`);
  }
}

console.log(`verified website security headers and ${inlineScripts.length} inline script hash(es)`);
