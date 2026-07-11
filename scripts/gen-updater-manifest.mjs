#!/usr/bin/env node
// Build the Tauri updater manifest (latest.json) from collected release
// artifacts. Each macOS updater tarball is renamed with its arch during the
// CI "Collect artifacts" step and carries a sibling .sig; we read those sigs
// and point the urls at the GitHub release download path. The manifest is
// served as a release asset, matching the updater endpoints in tauri.conf.json.
//
// Usage: node scripts/gen-updater-manifest.mjs <tag> <artifacts-dir> <source-root>
import { createHash } from "node:crypto";
import { existsSync, readdirSync, readFileSync, writeFileSync } from "node:fs";
import { join } from "node:path";

const [, , tag, dir, sourceRoot] = process.argv;
if (!tag || !dir || !sourceRoot) {
  console.error(
    "usage: gen-updater-manifest.mjs <tag> <artifacts-dir> <source-root>",
  );
  process.exit(2);
}

const parseReleaseTag = (rawTag) => {
  const match =
    /^v(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?$/.exec(
      rawTag,
    );
  if (!match) return null;
  const prerelease = match[4];
  if (
    prerelease
      ?.split(".")
      .some(
        (identifier) => /^\d+$/.test(identifier) && /^0\d+/.test(identifier),
      )
  ) {
    return null;
  }
  return {
    version: rawTag.slice(1),
    channel: prerelease ? "prerelease" : "stable",
  };
};

const release = parseReleaseTag(tag);
if (!release) {
  console.error(
    `[manifest] tag must be a canonical SemVer tag prefixed with v: ${tag}`,
  );
  process.exit(1);
}
const { version, channel } = release;
const REPO = "Wangnov/Codex-App-Manager";
const releaseNotesPath = join(sourceRoot, "docs", "releases", `${tag}.md`);
const updaterNotes = (markdown) => {
  // Release pages start with a GitHub-only banner and end with the fixed
  // download/signature table. The in-app sheet needs the reviewed summary and
  // user-visible changes, not raw HTML or another set of install links.
  const withoutBanner = markdown.replace(
    /^\s*<p\s+align="center">[\s\S]*?<\/p>\s*/i,
    "",
  );
  return withoutBanner.split(/^##\s+📦\s+/m, 1)[0].trim();
};
const notes = existsSync(releaseNotesPath)
  ? updaterNotes(readFileSync(releaseNotesPath, "utf8")) ||
    `Codex App Manager ${tag}`
  : `Codex App Manager ${tag}`;
const downloadUrl = (file) =>
  `https://github.com/${REPO}/releases/download/${tag}/${encodeURIComponent(file)}`;

const files = readdirSync(dir);
const findSig = (re) => files.find((f) => re.test(f) && f.endsWith(".sig"));
const sha256File = (path) =>
  createHash("sha256").update(readFileSync(path)).digest("hex");

// Tauri updater platform keys → how to spot that platform's signed bundle.
const MATCHERS = [
  ["darwin-aarch64", /aarch64.*\.app\.tar\.gz\.sig$/],
  ["darwin-x86_64", /x86_64.*\.app\.tar\.gz\.sig$/],
  ["windows-x86_64", /(?:x64|x86_64).*-setup\.(?:exe|nsis\.zip)\.sig$/],
  ["windows-aarch64", /(?:arm64|aarch64).*-setup\.(?:exe|nsis\.zip)\.sig$/],
];
const REQUIRED_PLATFORMS = MATCHERS.map(([key]) => key);
const allowPartialRelease = process.env.ALLOW_PARTIAL_RELEASE === "1";

const platforms = {};
const resolved = [];
for (const [key, re] of MATCHERS) {
  const sig = findSig(re);
  if (!sig) {
    console.warn(`[manifest] no signed artifact for ${key} — skipping`);
    continue;
  }
  const bundle = sig.replace(/\.sig$/, "");
  const bundlePath = join(dir, bundle);
  if (!existsSync(bundlePath)) {
    console.error(
      `[manifest] signature ${sig} resolved ${bundle}, but the bundle is missing`,
    );
    process.exit(1);
  }
  const url = downloadUrl(bundle);
  const sha256 = sha256File(bundlePath);
  platforms[key] = {
    signature: readFileSync(join(dir, sig), "utf8").trim(),
    sha256,
    url,
  };
  resolved.push({
    platform: key,
    artifact: bundle,
    signature_file: sig,
    sha256,
    url,
  });
}

if (Object.keys(platforms).length === 0) {
  console.error("[manifest] no platforms resolved — check artifact globs");
  process.exit(1);
}

const missing = REQUIRED_PLATFORMS.filter((key) => !platforms[key]);
if (missing.length > 0) {
  const message = `[manifest] missing required platform artifacts: ${missing.join(", ")}`;
  if (!allowPartialRelease) {
    console.error(message);
    console.error(
      "[manifest] set ALLOW_PARTIAL_RELEASE=1 only for an intentional one-off partial release",
    );
    process.exit(1);
  }
  console.warn(
    `::warning::partial release allowed: missing ${missing.join(", ")}`,
  );
}

const manifest = {
  version,
  channel,
  // The same reviewed release note shown on GitHub powers the in-app details
  // view. Older/fallback builds still receive a short non-empty label.
  notes,
  pub_date: new Date().toISOString(),
  platforms,
};
if (missing.length > 0) {
  manifest.partial = true;
  manifest.missing = missing;
}

writeFileSync("latest.json", JSON.stringify(manifest, null, 2));

// `latest.json` remains URL-rewritten on the CN mirror for compatibility with
// already-shipped clients, so it cannot itself have one cross-provider
// signature. Sign this deterministic URL-free identity instead. New clients
// require the identity before trusting any mirror's version/artifact claim.
// Deliberately exclude pub_date/build time so a rerun for the same release
// produces byte-identical identity JSON.
const releaseIdentity = {
  schema: 1,
  version,
  channel,
  notes_sha256: createHash("sha256").update(notes, "utf8").digest("hex"),
  // The updater verifies the manifest's Minisign signature independently;
  // this authority binds the exact artifact bytes without duplicating it.
  platforms: Object.fromEntries(
    resolved.map(({ platform, artifact, sha256 }) => [
      platform,
      {
        artifact,
        sha256,
      },
    ]),
  ),
};
writeFileSync(
  "release-identity.json",
  JSON.stringify(releaseIdentity, null, 2) + "\n",
);
writeFileSync(
  "manifest-summary.json",
  JSON.stringify(
    {
      tag,
      version,
      allow_partial_release: allowPartialRelease,
      partial: missing.length > 0,
      required_platforms: REQUIRED_PLATFORMS.map((platform) => ({
        platform,
        present: Boolean(platforms[platform]),
        missing: !platforms[platform],
      })),
      missing,
      artifacts: resolved,
    },
    null,
    2,
  ) + "\n",
);
console.log("wrote latest.json:\n" + JSON.stringify(manifest, null, 2));
console.log(
  "wrote release-identity.json:\n" + JSON.stringify(releaseIdentity, null, 2),
);
