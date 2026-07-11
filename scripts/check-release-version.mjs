#!/usr/bin/env node

import { existsSync, readFileSync, readdirSync, statSync } from "node:fs";
import { join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

import { requiredReleaseAssetNames } from "./check-release-reuse.mjs";

const RELEASE_TAG_PATTERN =
  /^v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?$/;

export function releaseVersionFromTag(releaseTag) {
  const tag = String(releaseTag);
  if (!RELEASE_TAG_PATTERN.test(tag)) {
    throw new Error(`release tag must be a semantic vX.Y.Z tag: ${releaseTag}`);
  }
  return tag.slice(1);
}

function readJson(path, label) {
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    throw new Error(`cannot read ${label}: ${detail}`);
  }
}

function quotedTomlValue(block, key, label) {
  const matches = [...block.matchAll(new RegExp(`^${key}\\s*=\\s*"([^"]+)"\\s*(?:#.*)?$`, "gm"))];
  if (matches.length !== 1) {
    throw new Error(`${label} must contain exactly one quoted ${key}`);
  }
  return matches[0][1];
}

function cargoPackageBlock(cargoToml, label) {
  const match = /^\[package\]\s*$/m.exec(cargoToml);
  if (!match) {
    throw new Error(`${label} has no [package] table`);
  }
  const start = match.index + match[0].length;
  const rest = cargoToml.slice(start);
  const nextTable = /^\s*\[/m.exec(rest);
  return nextTable ? rest.slice(0, nextTable.index) : rest;
}

function cargoLockPackageVersion(cargoLock, packageName) {
  const blocks = cargoLock.split(/^\[\[package\]\]\s*$/m).slice(1);
  const matches = blocks.filter((block) => {
    try {
      return quotedTomlValue(block, "name", "Cargo.lock package") === packageName;
    } catch {
      return false;
    }
  });
  if (matches.length !== 1) {
    throw new Error(`Cargo.lock must contain exactly one ${packageName} package block`);
  }
  return quotedTomlValue(matches[0], "version", `Cargo.lock ${packageName} package`);
}

export function readReleaseSourceVersions(sourceRoot) {
  const root = resolve(sourceRoot);
  const packageJson = readJson(join(root, "package.json"), "package.json");
  const packageLock = readJson(join(root, "package-lock.json"), "package-lock.json");
  const tauriConfig = readJson(
    join(root, "src-tauri", "tauri.conf.json"),
    "src-tauri/tauri.conf.json",
  );
  const cargoTomlPath = join(root, "src-tauri", "Cargo.toml");
  const cargoLockPath = join(root, "src-tauri", "Cargo.lock");
  const cargoToml = readFileSync(cargoTomlPath, "utf8");
  const cargoLock = readFileSync(cargoLockPath, "utf8");

  return [
    ["package.json#version", packageJson.version],
    ["package-lock.json#version", packageLock.version],
    ['package-lock.json#packages[""].version', packageLock.packages?.[""]?.version],
    ["src-tauri/tauri.conf.json#version", tauriConfig.version],
    [
      "src-tauri/Cargo.toml#[package].version",
      quotedTomlValue(
        cargoPackageBlock(cargoToml, "src-tauri/Cargo.toml"),
        "version",
        "src-tauri/Cargo.toml [package]",
      ),
    ],
    [
      'src-tauri/Cargo.lock#[[package]] name="codex-app-manager".version',
      cargoLockPackageVersion(cargoLock, "codex-app-manager"),
    ],
  ];
}

export function assertReleaseSourceVersions(releaseTag, sourceRoot) {
  const expected = releaseVersionFromTag(releaseTag);
  const entries = readReleaseSourceVersions(sourceRoot);
  const mismatches = entries.filter(([, version]) => version !== expected);
  if (mismatches.length > 0) {
    const detail = mismatches
      .map(([label, version]) => `${label}=${JSON.stringify(version)}`)
      .join(", ");
    throw new Error(
      `release tag ${releaseTag} requires application version ${expected}; mismatched source versions: ${detail}`,
    );
  }
  return { entries, version: expected };
}

export function assertLocalReleaseArtifactNames(releaseTag, artifactsDir) {
  const version = releaseVersionFromTag(releaseTag);
  // The updater manifest, signed channel identity, and trusted-main binding
  // are derived only after the platform matrix has been collected. This gate
  // validates the exact build artifact set that those files will authorize.
  const expected = requiredReleaseAssetNames(releaseTag).filter(
    (name) =>
      name !== "latest.json" &&
      name !== "release-binding.json" &&
      !name.startsWith("release-identity.json"),
  );
  const dir = resolve(artifactsDir);
  const files = readdirSync(dir, { withFileTypes: true })
    .filter((entry) => entry.isFile())
    .map((entry) => entry.name);
  const missing = expected.filter((name) => {
    const path = join(dir, name);
    return !existsSync(path) || !statSync(path).isFile() || statSync(path).size <= 0;
  });
  if (missing.length > 0) {
    throw new Error(
      `release ${releaseTag} is missing exact local artifact names: ${missing.join(", ")}`,
    );
  }

  const updaterInstallers = files.filter((name) =>
    /_(?:x64|arm64)-setup\.(?:exe|nsis\.zip)(?:\.sig)?$/.test(name),
  );
  const wrongVersion = updaterInstallers.filter(
    (name) => !name.startsWith(`CodexAppManager_${version}_`),
  );
  if (wrongVersion.length > 0) {
    throw new Error(
      `release ${releaseTag} contains installer artifacts for another version: ${wrongVersion.join(", ")}`,
    );
  }

  const unexpected = files.filter(
    (name) => name.startsWith("CodexAppManager") && !expected.includes(name),
  );
  if (unexpected.length > 0) {
    throw new Error(
      `release ${releaseTag} contains unexpected local artifact names: ${unexpected.join(", ")}`,
    );
  }

  return { expected, version };
}

const isCli = process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isCli) {
  const [, , mode, releaseTag, path] = process.argv;
  try {
    if (!mode || !releaseTag || !path || !["source", "artifacts"].includes(mode)) {
      throw new Error(
        "usage: check-release-version.mjs <source|artifacts> <vX.Y.Z> <source-root|artifacts-dir>",
      );
    }
    if (mode === "source") {
      const result = assertReleaseSourceVersions(releaseTag, path);
      console.log(
        `[version] ${releaseTag} matches all ${result.entries.length} application version declarations`,
      );
    } else {
      const result = assertLocalReleaseArtifactNames(releaseTag, path);
      console.log(
        `[version] ${releaseTag} matches all ${result.expected.length} exact local release artifact names`,
      );
    }
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    console.error(`::error::[version] ${detail}`);
    process.exitCode = 1;
  }
}
