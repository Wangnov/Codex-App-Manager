#!/usr/bin/env node
import { readFileSync, writeFileSync } from "node:fs";
import { pathToFileURL } from "node:url";
import { verifyTauriMinisign } from "./minisign-verify.mjs";

const IDENTITY_MAX_BYTES = 256 * 1024;
const SIGNATURE_MAX_BYTES = 16 * 1024;

export const fetchBounded = async (fetchImpl, url, maxBytes, label, options = {}) => {
  const response = await fetchImpl(url, { redirect: "follow", ...options });
  if (response.status === 404) return null;
  if (!response.ok) throw new Error(`${label} returned HTTP ${response.status}`);

  const announced = Number(response.headers.get("content-length"));
  if (Number.isFinite(announced) && announced > maxBytes) {
    throw new Error(`${label} exceeds ${maxBytes}-byte limit (announced ${announced})`);
  }
  if (!response.body) return Buffer.alloc(0);

  const chunks = [];
  let observed = 0;
  for await (const chunk of response.body) {
    const bytes = Buffer.from(chunk);
    observed += bytes.length;
    if (!Number.isSafeInteger(observed) || observed > maxBytes) {
      await response.body.cancel().catch(() => {});
      throw new Error(`${label} exceeds ${maxBytes}-byte limit (observed ${observed})`);
    }
    chunks.push(bytes);
  }
  return Buffer.concat(chunks, observed);
};

const verifyReusableSignature = ({ identityBytes, remoteIdentity, remoteSignature, publicKey }) => {
  if (remoteIdentity && !remoteIdentity.equals(identityBytes)) {
    throw new Error("prior immutable release identity differs from the local canonical bytes");
  }
  if (!remoteSignature) return null;
  const encodedSignature = remoteSignature.toString("utf8").trim();
  verifyTauriMinisign(identityBytes, encodedSignature, publicKey);
  // Verification accepts the text envelope after trimming surrounding
  // whitespace, but immutable reruns must preserve the already-published asset
  // byte-for-byte. Re-encoding here would change the SHA-256 when the original
  // Tauri signer output has no trailing newline.
  return Buffer.from(remoteSignature);
};

export const reuseGithubReleaseIdentitySignature = async ({
  identityBytes,
  version,
  repository,
  tag,
  token,
  publicKey,
  fetchImpl = fetch,
}) => {
  if (!repository || !tag || tag.replace(/^v/, "") !== version) return null;
  const headers = {
    accept: "application/vnd.github+json",
    "x-github-api-version": "2022-11-28",
    ...(token ? { authorization: `Bearer ${token}` } : {}),
  };
  const metadataBytes = await fetchBounded(
    fetchImpl,
    `https://api.github.com/repos/${repository}/releases/tags/${encodeURIComponent(tag)}`,
    1024 * 1024,
    "prior GitHub Release metadata",
    { headers },
  );
  if (!metadataBytes) return null;
  const release = JSON.parse(metadataBytes.toString("utf8"));
  const assets = Array.isArray(release.assets) ? release.assets : [];
  const identityAsset = assets.find((asset) => asset?.name === "release-identity.json");
  const signatureAsset = assets.find((asset) => asset?.name === "release-identity.json.sig");
  if (!identityAsset && !signatureAsset) return null;

  const assetHeaders = { ...headers, accept: "application/octet-stream" };
  const [remoteIdentity, remoteSignature] = await Promise.all([
    identityAsset
      ? fetchBounded(
          fetchImpl,
          identityAsset.url,
          IDENTITY_MAX_BYTES,
          "prior GitHub release identity",
          { headers: assetHeaders },
        )
      : null,
    signatureAsset
      ? fetchBounded(
          fetchImpl,
          signatureAsset.url,
          SIGNATURE_MAX_BYTES,
          "prior GitHub identity signature",
          { headers: assetHeaders },
        )
      : null,
  ]);
  return verifyReusableSignature({ identityBytes, remoteIdentity, remoteSignature, publicKey });
};

export const reuseReleaseIdentitySignature = async ({
  identityBytes,
  version,
  mirrorBase,
  publicKey,
  fetchImpl = fetch,
}) => {
  if (!/^[0-9A-Za-z.+-]+$/.test(version)) {
    throw new Error("release identity has an unsafe version");
  }
  const base = new URL(`${mirrorBase.replace(/\/+$/, "")}/${encodeURIComponent(version)}/`);
  const identityUrl = new URL("release-identity.json", base);
  const signatureUrl = new URL("release-identity.json.sig", base);
  const [remoteIdentity, remoteSignature] = await Promise.all([
    fetchBounded(fetchImpl, identityUrl, IDENTITY_MAX_BYTES, "prior release identity"),
    fetchBounded(fetchImpl, signatureUrl, SIGNATURE_MAX_BYTES, "prior identity signature"),
  ]);

  return verifyReusableSignature({ identityBytes, remoteIdentity, remoteSignature, publicKey });
};

const main = async () => {
  const [, , identityPath, signaturePath, configPath = "src-tauri/tauri.conf.json"] = process.argv;
  if (!identityPath || !signaturePath) {
    console.error(
      "usage: reuse-release-identity-signature.mjs <identity.json> <identity.json.sig> [tauri.conf.json]",
    );
    process.exit(2);
  }
  const identityBytes = readFileSync(identityPath);
  const identity = JSON.parse(identityBytes.toString("utf8"));
  const config = JSON.parse(readFileSync(configPath, "utf8"));
  const mirrorBase = process.env.MANAGER_MIRROR_BASE_URL ?? "https://codexapp.agentsmirror.com/manager";
  const publicKey = config?.plugins?.updater?.pubkey ?? "";
  let reused;
  try {
    const version = String(identity.version ?? "");
    try {
      reused = await reuseGithubReleaseIdentitySignature({
        identityBytes,
        version,
        repository: process.env.GITHUB_REPOSITORY,
        tag: process.env.GITHUB_REF_NAME,
        token: process.env.GITHUB_TOKEN ?? process.env.GH_TOKEN,
        publicKey,
      });
    } catch (error) {
      if (!(error instanceof TypeError && /fetch/i.test(error.message))) throw error;
      console.warn(`prior GitHub identity lookup unavailable: ${error.message}`);
    }
    if (!reused) {
      reused = await reuseReleaseIdentitySignature({
        identityBytes,
        version,
        mirrorBase,
        publicKey,
      });
    }
  } catch (error) {
    if (error instanceof TypeError && /fetch/i.test(error.message)) {
      console.warn(`prior mirror identity lookup unavailable: ${error.message}`);
      process.exit(3);
    }
    throw error;
  }
  if (!reused) process.exit(3);
  writeFileSync(signaturePath, reused);
  console.log(`reused verified immutable identity signature for ${identity.version}`);
};

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch((error) => {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  });
}
