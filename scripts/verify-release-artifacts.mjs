#!/usr/bin/env node

import { readFile, writeFile } from "node:fs/promises";
import { resolve } from "node:path";

import {
  verifyLocalReleaseIdentity,
  verifyLocalUpdaterArtifacts,
} from "./mirror-release.mjs";

function usage() {
  return "usage: verify-release-artifacts.mjs <latest.json> <dist-dir> <tauri.conf.json>";
}

async function readJson(path, label) {
  try {
    return JSON.parse(await readFile(path, "utf8"));
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    throw new Error(`cannot read ${label}: ${detail}`);
  }
}

async function main() {
  const [manifestArg, distArg, configArg] = process.argv.slice(2);
  if (!manifestArg || !distArg || !configArg) throw new Error(usage());

  const manifest = await readJson(resolve(manifestArg), "updater manifest");
  const config = await readJson(resolve(configArg), "Tauri configuration");
  const publicKey = config.plugins?.updater?.pubkey;
  if (typeof publicKey !== "string" || !publicKey.trim()) {
    throw new Error("Tauri configuration has no updater public key");
  }

  const report = await verifyLocalUpdaterArtifacts({
    manifest,
    distDir: resolve(distArg),
    publicKey: publicKey.trim(),
  });
  const expectedChannel = String(manifest.version).split("+", 1)[0].includes("-")
    ? "prerelease"
    : "stable";
  const identity = await verifyLocalReleaseIdentity({
    candidateManifest: manifest,
    distDir: resolve(distArg),
    expectedChannel,
    publicKey: publicKey.trim(),
  });
  const summaryPath = resolve(
    process.env.UPDATER_SIGNATURE_SUMMARY_PATH ||
      "updater-signature-verification.json",
  );
  await writeFile(
    summaryPath,
    `${JSON.stringify(
      {
        ...report,
        identity: {
          channel: identity.identity.channel,
          sha256: identity.identitySha256,
          signatureSha256: identity.signatureSha256,
          version: identity.identity.version,
        },
        manifestVersion: manifest.version,
        verifiedAt: new Date().toISOString(),
      },
      null,
      2,
    )}\n`,
  );
  console.log(
    `[release] cryptographically verified ${report.artifactCount} updater artifacts`,
  );
}

main().catch((error) => {
  console.error(
    `::error::[release] ${error instanceof Error ? error.message : String(error)}`,
  );
  process.exitCode = 1;
});
