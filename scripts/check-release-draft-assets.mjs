#!/usr/bin/env node

import { createHash } from "node:crypto";
import { readFileSync, statSync, writeFileSync } from "node:fs";
import { basename } from "node:path";
import { fileURLToPath } from "node:url";

import {
  REQUIRED_RELEASE_METADATA_ASSET_NAMES,
  allowedReleaseAssetNames,
  requiredReleaseAssetNames,
} from "./check-release-reuse.mjs";

const RELEASE_TAG_PATTERN =
  /^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;
const SAFE_ASSET_NAME = /^[A-Za-z0-9][A-Za-z0-9._+-]*$/;

function sha256File(path) {
  return createHash("sha256").update(readFileSync(path)).digest("hex");
}

export function verifyDraftReleaseAssets(release, releaseTag, paths) {
  if (!RELEASE_TAG_PATTERN.test(String(releaseTag))) {
    throw new Error("draft release tag must be semantic vX.Y.Z");
  }
  if (!release || typeof release !== "object" || Array.isArray(release)) {
    throw new Error("GitHub draft release response must be a JSON object");
  }
  if (release.tag_name !== releaseTag || release.draft !== true || release.immutable === true) {
    throw new Error("release is no longer the expected mutable draft");
  }
  if (!Number.isSafeInteger(release.id) || release.id <= 0) {
    throw new Error("GitHub draft release has no valid id");
  }
  if (!Array.isArray(paths) || paths.length === 0) {
    throw new Error("draft verification requires canonical local assets");
  }

  const localByName = new Map();
  const allowedNames = new Set(allowedReleaseAssetNames(releaseTag));
  for (const path of paths) {
    const metadata = statSync(path);
    if (!metadata.isFile() || metadata.size <= 0) {
      throw new Error(`canonical release asset is missing or empty: ${path}`);
    }
    const name = basename(path);
    if (!SAFE_ASSET_NAME.test(name)) {
      throw new Error(`canonical release asset name is unsafe: ${name}`);
    }
    if (!allowedNames.has(name)) {
      throw new Error(`local release asset is not in the canonical allowlist: ${name}`);
    }
    if (localByName.has(name)) {
      throw new Error(`canonical release assets contain duplicate basename: ${name}`);
    }
    localByName.set(name, {
      localPath: path,
      name,
      sha256: sha256File(path),
      size: metadata.size,
    });
  }
  const missingRequiredLocal = [
    ...requiredReleaseAssetNames(releaseTag),
    ...REQUIRED_RELEASE_METADATA_ASSET_NAMES,
  ].filter((name) => !localByName.has(name));
  if (missingRequiredLocal.length > 0) {
    throw new Error(
      `canonical local release assets are incomplete: ${missingRequiredLocal.join(", ")}`,
    );
  }

  const remoteAssets = Array.isArray(release.assets) ? release.assets : [];
  const remoteByName = new Map();
  for (const asset of remoteAssets) {
    const name = asset?.name;
    if (typeof name !== "string" || !SAFE_ASSET_NAME.test(name)) {
      throw new Error("draft release contains an unsafe asset name");
    }
    if (remoteByName.has(name)) {
      throw new Error(`draft release contains duplicate asset name: ${name}`);
    }
    if (!Number.isSafeInteger(asset?.id) || asset.id <= 0) {
      throw new Error(`draft release asset has no valid id: ${name}`);
    }
    if (!Number.isSafeInteger(asset?.size) || asset.size <= 0) {
      throw new Error(`draft release asset is empty: ${name}`);
    }
    remoteByName.set(name, asset);
  }

  const localNames = [...localByName.keys()].sort();
  const remoteNames = [...remoteByName.keys()].sort();
  if (JSON.stringify(localNames) !== JSON.stringify(remoteNames)) {
    const missing = localNames.filter((name) => !remoteByName.has(name));
    const unexpected = remoteNames.filter((name) => !localByName.has(name));
    throw new Error(
      `draft release asset set differs from canonical local assets (missing: ${missing.join(", ") || "none"}; unexpected: ${unexpected.join(", ") || "none"})`,
    );
  }

  const assets = localNames.map((name) => {
    const local = localByName.get(name);
    const remote = remoteByName.get(name);
    if (remote.size !== local.size) {
      throw new Error(`draft release asset size differs from local bytes: ${name}`);
    }
    if (
      remote.digest !== undefined &&
      remote.digest !== null &&
      remote.digest !== `sha256:${local.sha256}`
    ) {
      throw new Error(`draft release asset digest differs from local bytes: ${name}`);
    }
    return {
      id: remote.id,
      localPath: local.localPath,
      name,
      sha256: local.sha256,
      size: local.size,
    };
  });
  return { assets, releaseId: release.id, releaseTag };
}

if (process.argv[1] === fileURLToPath(import.meta.url)) {
  const [, , releaseTag, releaseJsonPath, outputPath, ...paths] = process.argv;
  try {
    if (!releaseTag || !releaseJsonPath || !outputPath) {
      throw new Error(
        "usage: check-release-draft-assets.mjs <vX.Y.Z> <release.json> <output.json> <asset...>",
      );
    }
    const result = verifyDraftReleaseAssets(
      JSON.parse(readFileSync(releaseJsonPath, "utf8")),
      releaseTag,
      paths,
    );
    writeFileSync(outputPath, `${JSON.stringify(result, null, 2)}\n`);
    console.log(`Verified exact mutable draft asset set for ${releaseTag}`);
  } catch (error) {
    console.error(`::error::${error instanceof Error ? error.message : String(error)}`);
    process.exitCode = 1;
  }
}
