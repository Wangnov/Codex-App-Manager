#!/usr/bin/env node

import { appendFileSync, readFileSync } from "node:fs";

const RELEASE_TAG_PATTERN =
  /^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;

export function requiredReleaseAssetNames(releaseTag) {
  const tag = String(releaseTag);
  if (!RELEASE_TAG_PATTERN.test(tag)) {
    throw new Error(`release tag must be a semantic vX.Y.Z tag: ${releaseTag}`);
  }
  const version = tag.slice(1);
  return [
    "latest.json",
    "release-binding.json",
    "CodexAppManager_aarch64.dmg",
    "CodexAppManager_x86_64.dmg",
    "CodexAppManager_aarch64.app.tar.gz",
    "CodexAppManager_aarch64.app.tar.gz.sig",
    "CodexAppManager_x86_64.app.tar.gz",
    "CodexAppManager_x86_64.app.tar.gz.sig",
    `CodexAppManager_${version}_x64-setup.exe`,
    `CodexAppManager_${version}_x64-setup.exe.sig`,
    `CodexAppManager_${version}_arm64-setup.exe`,
    `CodexAppManager_${version}_arm64-setup.exe.sig`,
  ];
}

export function inspectReleaseForReuse(release, releaseTag) {
  if (!release || typeof release !== "object" || Array.isArray(release)) {
    throw new Error("GitHub release response must be a JSON object");
  }
  const required = requiredReleaseAssetNames(releaseTag);
  const assets = Array.isArray(release.assets) ? release.assets : [];
  const missing = [];
  for (const name of required) {
    const matches = assets.filter((asset) => asset?.name === name);
    if (matches.length !== 1 || !Number.isFinite(matches[0]?.size) || matches[0].size <= 0) {
      missing.push(name);
    }
  }

  // Drafts are the only repairable state: their assets may still be replaced
  // before publication. Never route a published Release back through upload.
  if (release.draft !== false) {
    return {
      digests: {},
      missing,
      reason: "draft",
      reusable: false,
    };
  }
  if (release.immutable !== true) {
    throw new Error(
      `existing release ${releaseTag} is mutable; refusing to treat its current assets as canonical`,
    );
  }
  if (missing.length > 0) {
    throw new Error(
      `existing immutable release ${releaseTag} is missing required assets and cannot be repaired: ${missing.join(", ")}`,
    );
  }

  const selectedAssets = assets.filter(
    (asset) =>
      asset?.name === "latest.json" ||
      asset?.name === "release-binding.json" ||
      asset?.name?.startsWith("CodexAppManager"),
  );
  const selectedNames = new Set();
  const digestEntries = [];
  for (const asset of selectedAssets) {
    const name = asset.name;
    if (selectedNames.has(name)) {
      throw new Error(`immutable release has duplicate downloadable asset names: ${name}`);
    }
    selectedNames.add(name);
    if (!Number.isFinite(asset.size) || asset.size <= 0) {
      throw new Error(`immutable release downloadable asset is empty: ${name}`);
    }
    const digest = asset.digest;
    if (typeof digest !== "string" || !/^sha256:[0-9a-f]{64}$/.test(digest)) {
      throw new Error(`immutable release asset has no canonical SHA-256 digest: ${name}`);
    }
    digestEntries.push([name, digest]);
  }
  const digests = Object.fromEntries(digestEntries);
  return { digests, missing: [], reason: null, reusable: true };
}

const isCli = process.argv[1]?.endsWith("check-release-reuse.mjs");
if (isCli) {
  const [, , releaseTag, releaseJsonPath] = process.argv;
  const requestedTargetTag = process.env.REQUESTED_TARGET_TAG || "";
  const outputPath = process.env.GITHUB_OUTPUT;
  try {
    if (!releaseTag || !releaseJsonPath || !outputPath) {
      throw new Error(
        "usage: check-release-reuse.mjs <vX.Y.Z> <release.json> with GITHUB_OUTPUT set",
      );
    }
    const release = JSON.parse(readFileSync(releaseJsonPath, "utf8"));
    const result = inspectReleaseForReuse(release, releaseTag);
    if (!result.reusable && requestedTargetTag) {
      throw new Error(
        `target_tag ${requestedTargetTag} is draft or missing required release assets: ${result.missing.join(", ") || result.reason}`,
      );
    }
    appendFileSync(outputPath, `release_reusable=${result.reusable ? "true" : "false"}\n`);
    if (result.reusable) {
      appendFileSync(outputPath, `release_asset_digests=${JSON.stringify(result.digests)}\n`);
      console.log(`Existing immutable release ${releaseTag} has canonical digests for all assets`);
    } else {
      console.log(
        `Existing release ${releaseTag} is not reusable yet (${result.reason}; ${result.missing.join(", ")})`,
      );
    }
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    console.error(`::error::${detail}`);
    process.exitCode = 1;
  }
}
