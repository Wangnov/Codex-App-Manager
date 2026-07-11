#!/usr/bin/env node

import { spawn } from "node:child_process";
import {
  createHash,
  createPublicKey,
  randomUUID,
  verify as cryptoVerify,
} from "node:crypto";
import { createReadStream, createWriteStream } from "node:fs";
import {
  mkdir,
  mkdtemp,
  readFile,
  readdir,
  rm,
  stat,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join, resolve } from "node:path";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { fileURLToPath } from "node:url";

const SCRIPT_DIR = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(SCRIPT_DIR, "..");
const DEFAULT_MIRROR_BASE = "https://codexapp.agentsmirror.com/manager";
const LATEST_KEY = "latest.json";
const ROOT_IDENTITY_KEY = "release-identity.json";
const ROOT_IDENTITY_SIGNATURE_KEY = "release-identity.json.sig";
const RELEASE_IDENTITY_SCHEMA_VERSION = 1;
const SUMMARY_SCHEMA_VERSION = 1;
const ED25519_SPKI_PREFIX = Buffer.from("302a300506032b6570032100", "hex");
export const REQUIRED_UPDATER_PLATFORMS = [
  "darwin-aarch64",
  "darwin-x86_64",
  "windows-x86_64",
  "windows-aarch64",
];

let interruptedBy = null;
let rollbackInProgress = false;
const activeChildren = new Set();

export class ConditionalWriteError extends Error {
  constructor(message) {
    super(message);
    this.name = "ConditionalWriteError";
  }
}

export class DowngradeBlockedError extends Error {
  constructor(message) {
    super(message);
    this.name = "DowngradeBlockedError";
  }
}

class WriteOutcomeUncertainError extends Error {
  constructor(message) {
    super(message);
    this.name = "WriteOutcomeUncertainError";
  }
}

class PublicRouteBindingError extends Error {
  constructor(message) {
    super(message);
    this.name = "PublicRouteBindingError";
  }
}

function errorText(error) {
  if (error instanceof Error) return error.message;
  return String(error);
}

function nowIso() {
  return new Date().toISOString();
}

function safeSummaryError(error) {
  return errorText(error).replace(/\s+/g, " ").trim().slice(0, 2_000);
}

function strictBase64(value, label) {
  const normalized = String(value).trim();
  if (
    normalized.length === 0 ||
    normalized.length % 4 !== 0 ||
    !/^[A-Za-z0-9+/]+={0,2}$/.test(normalized)
  ) {
    throw new Error(`${label} is not valid base64`);
  }
  const decoded = Buffer.from(normalized, "base64");
  if (decoded.toString("base64") !== normalized) {
    throw new Error(`${label} is not canonical base64`);
  }
  return decoded;
}

export function parseSemver(value) {
  const text = String(value).trim().replace(/^v/, "");
  const match = /^(0|[1-9]\d*)\.(0|[1-9]\d*)\.(0|[1-9]\d*)(?:-([0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*))?(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$/.exec(
    text,
  );
  if (!match) throw new Error(`invalid semantic version: ${value}`);
  const prerelease = match[4] ? match[4].split(".") : [];
  for (const identifier of prerelease) {
    if (/^\d+$/.test(identifier) && identifier.length > 1 && identifier.startsWith("0")) {
      throw new Error(`invalid semantic version prerelease: ${value}`);
    }
  }
  return {
    raw: text,
    major: BigInt(match[1]),
    minor: BigInt(match[2]),
    patch: BigInt(match[3]),
    prerelease,
  };
}

export function compareSemver(left, right) {
  const a = parseSemver(left);
  const b = parseSemver(right);
  for (const key of ["major", "minor", "patch"]) {
    if (a[key] < b[key]) return -1;
    if (a[key] > b[key]) return 1;
  }
  if (a.prerelease.length === 0 && b.prerelease.length === 0) return 0;
  if (a.prerelease.length === 0) return 1;
  if (b.prerelease.length === 0) return -1;
  const count = Math.max(a.prerelease.length, b.prerelease.length);
  for (let index = 0; index < count; index += 1) {
    const av = a.prerelease[index];
    const bv = b.prerelease[index];
    if (av === undefined) return -1;
    if (bv === undefined) return 1;
    if (av === bv) continue;
    const aNumeric = /^\d+$/.test(av);
    const bNumeric = /^\d+$/.test(bv);
    if (aNumeric && bNumeric) return BigInt(av) < BigInt(bv) ? -1 : 1;
    if (aNumeric !== bNumeric) return aNumeric ? -1 : 1;
    return av < bv ? -1 : 1;
  }
  return 0;
}

function assertSafeSegment(value, label) {
  if (!/^[A-Za-z0-9][A-Za-z0-9._+-]{0,127}$/.test(value) || value.includes("..")) {
    throw new Error(`${label} contains unsafe characters: ${value}`);
  }
  return value;
}

function normalizeMirrorBase(value) {
  const parsed = new URL(String(value || DEFAULT_MIRROR_BASE).replace(/\/$/, ""));
  if (parsed.protocol !== "https:" || parsed.username || parsed.password || parsed.search || parsed.hash) {
    throw new Error("MIRROR_BASE_URL must be an HTTPS origin/path without credentials, query, or fragment");
  }
  return parsed.toString().replace(/\/$/, "");
}

export function candidateIdFromEnv(env = process.env) {
  const explicit = env.MIRROR_CANDIDATE_ID?.trim();
  const fallback = `${env.GITHUB_RUN_ID || "local"}-${env.GITHUB_RUN_ATTEMPT || "1"}`;
  return assertSafeSegment(explicit || fallback, "mirror candidate id");
}

export function candidateKeyFor(version, candidateId) {
  const parsed = parseSemver(version).raw;
  assertSafeSegment(parsed, "candidate version");
  assertSafeSegment(candidateId, "mirror candidate id");
  return `candidates/${parsed}/${candidateId}.json`;
}

function promotionTokenForCandidate(candidateKey) {
  return createHash("sha256").update(candidateKey).digest("hex");
}

async function readJson(path, label = path) {
  let value;
  try {
    value = JSON.parse(await readFile(path, "utf8"));
  } catch (error) {
    throw new Error(`cannot read ${label}: ${errorText(error)}`);
  }
  if (!value || typeof value !== "object" || Array.isArray(value)) {
    throw new Error(`${label} must contain a JSON object`);
  }
  return value;
}

async function writeJson(path, value) {
  await mkdir(dirname(resolve(path)), { recursive: true });
  await writeFile(path, `${JSON.stringify(value, null, 2)}\n`);
}

async function sha256File(path) {
  const hash = createHash("sha256");
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest("hex");
}

async function blake2bFile(path) {
  const hash = createHash("blake2b512");
  for await (const chunk of createReadStream(path)) hash.update(chunk);
  return hash.digest();
}

function parseUpdaterPublicKey(publicKeyBase64) {
  const text = strictBase64(publicKeyBase64, "Tauri updater public key").toString("utf8");
  const lines = text.trim().split(/\r?\n/);
  if (lines.length < 2) throw new Error("Tauri updater public key has invalid minisign encoding");
  const raw = strictBase64(lines[1], "minisign public key");
  if (raw.length !== 42 || !["Ed", "ED"].includes(raw.subarray(0, 2).toString("ascii"))) {
    throw new Error("Tauri updater public key has unsupported minisign encoding");
  }
  return { keyId: raw.subarray(2, 10), key: raw.subarray(10, 42) };
}

function parseUpdaterSignature(signatureBase64) {
  const text = strictBase64(signatureBase64, "Tauri updater signature").toString("utf8");
  const lines = text.trim().split(/\r?\n/);
  if (lines.length !== 4 || !lines[2].startsWith("trusted comment: ")) {
    throw new Error("Tauri updater signature has invalid minisign encoding");
  }
  const primary = strictBase64(lines[1], "minisign primary signature");
  const global = strictBase64(lines[3], "minisign global signature");
  if (primary.length !== 74 || global.length !== 64) {
    throw new Error("Tauri updater signature has invalid minisign lengths");
  }
  const algorithm = primary.subarray(0, 2).toString("ascii");
  if (!['Ed', 'ED'].includes(algorithm)) {
    throw new Error(`unsupported minisign signature algorithm: ${algorithm}`);
  }
  return {
    algorithm,
    keyId: primary.subarray(2, 10),
    signature: primary.subarray(10, 74),
    trustedComment: lines[2].slice("trusted comment: ".length),
    globalSignature: global,
  };
}

export async function verifyTauriUpdaterSignature(
  artifactPath,
  signatureBase64,
  publicKeyBase64,
) {
  const publicKey = parseUpdaterPublicKey(publicKeyBase64);
  const signature = parseUpdaterSignature(signatureBase64);
  if (!publicKey.keyId.equals(signature.keyId)) {
    throw new Error("Tauri updater signature key id does not match configured public key");
  }
  const spki = Buffer.concat([ED25519_SPKI_PREFIX, publicKey.key]);
  const key = createPublicKey({ key: spki, format: "der", type: "spki" });
  const payload =
    signature.algorithm === "ED"
      ? await blake2bFile(artifactPath)
      : await readFile(artifactPath);
  if (!cryptoVerify(null, payload, key, signature.signature)) {
    throw new Error("Tauri updater artifact signature is invalid");
  }
  const globalPayload = Buffer.concat([
    signature.signature,
    Buffer.from(signature.trustedComment, "utf8"),
  ]);
  if (!cryptoVerify(null, globalPayload, key, signature.globalSignature)) {
    throw new Error("Tauri updater trusted-comment signature is invalid");
  }
  return true;
}

function updaterArtifactsFromManifest(manifest, label) {
  assertManifestShape(manifest, label);
  const seenNames = new Set();
  return Object.entries(manifest.platforms)
    .sort(([left], [right]) => left.localeCompare(right))
    .map(([platform, entry]) => {
      const parsed = new URL(entry.url);
      const rawName = parsed.pathname.split("/").filter(Boolean).at(-1);
      const name = rawName ? decodeURIComponent(rawName) : "";
      assertSafeSegment(name, `${label} platform ${platform} artifact name`);
      if (seenNames.has(name)) {
        throw new Error(`multiple updater platforms resolve to the same artifact: ${name}`);
      }
      seenNames.add(name);
      return {
        name,
        platform,
        sha256: entry.sha256,
        signature: entry.signature,
        url: entry.url,
      };
    });
}

export async function verifyLocalUpdaterArtifacts({
  manifest,
  distDir,
  publicKey,
}) {
  const dist = resolve(distDir);
  const reports = [];
  for (const artifact of updaterArtifactsFromManifest(manifest, "release latest.json")) {
    const artifactPath = join(dist, artifact.name);
    const sidecarPath = `${artifactPath}.sig`;
    let sidecar;
    try {
      sidecar = (await readFile(sidecarPath, "utf8")).trim();
    } catch (error) {
      throw new Error(
        `local updater signature sidecar is missing for ${artifact.platform}: ${errorText(error)}`,
      );
    }
    if (sidecar !== artifact.signature) {
      throw new Error(
        `local updater signature sidecar does not match manifest for ${artifact.platform}`,
      );
    }
    const metadata = await stat(artifactPath).catch((error) => {
      throw new Error(
        `local updater artifact is missing for ${artifact.platform}: ${errorText(error)}`,
      );
    });
    if (!metadata.isFile() || metadata.size <= 0) {
      throw new Error(`local updater artifact is empty for ${artifact.platform}`);
    }
    const artifactSha256 = await sha256File(artifactPath);
    if (artifactSha256 !== artifact.sha256) {
      throw new Error(
        `local updater artifact sha256 does not match manifest for ${artifact.platform}`,
      );
    }
    await verifyTauriUpdaterSignature(artifactPath, artifact.signature, publicKey);
    reports.push({
      name: artifact.name,
      platform: artifact.platform,
      sha256: artifactSha256,
      size: metadata.size,
    });
  }
  return { artifactCount: reports.length, artifacts: reports, verified: true };
}

function manifestReleaseIdentity(manifest) {
  const channel = typeof manifest.channel === "string" ? manifest.channel : null;
  const notesSha256 =
    typeof manifest.notes === "string"
      ? createHash("sha256").update(manifest.notes, "utf8").digest("hex")
      : null;
  const platforms = Object.fromEntries(
    Object.entries(manifest.platforms || {})
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([platform, entry]) => [
        platform,
        {
          sha256: entry?.sha256 || "",
          signature: entry?.signature || "",
          url: entry?.url || "",
        },
      ]),
  );
  return JSON.stringify({ channel, notesSha256, version: manifest.version, platforms });
}

function assertManifestShape(manifest, label, { allowMissingSha256 = false } = {}) {
  const version = parseSemver(manifest.version).raw;
  if (!manifest.platforms || typeof manifest.platforms !== "object" || Array.isArray(manifest.platforms)) {
    throw new Error(`${label} has no platforms object`);
  }
  const entries = Object.entries(manifest.platforms);
  if (entries.length === 0) throw new Error(`${label} has no updater platforms`);
  for (const [platform, entry] of entries) {
    if (!entry || typeof entry.url !== "string" || typeof entry.signature !== "string") {
      throw new Error(`${label} platform ${platform} is missing url/signature`);
    }
    if (!entry.url.trim()) {
      throw new Error(`${label} platform ${platform} has an empty updater URL`);
    }
    if (!entry.signature.trim()) {
      throw new Error(`${label} platform ${platform} has an empty updater signature`);
    }
    if (!(allowMissingSha256 && entry.sha256 === undefined)) {
      assertSha256(entry.sha256, `${label} platform ${platform} sha256`);
    }
  }
  return version;
}

function assertRequiredUpdaterPlatforms(manifest, label) {
  const missing = REQUIRED_UPDATER_PLATFORMS.filter(
    (platform) => !Object.hasOwn(manifest.platforms, platform),
  );
  if (missing.length > 0) {
    throw new Error(`${label} is missing required updater platforms: ${missing.join(", ")}`);
  }
}

function assertSha256(value, label) {
  if (typeof value !== "string" || !/^[a-f0-9]{64}$/.test(value)) {
    throw new Error(`${label} must be a lowercase SHA-256 digest`);
  }
  return value;
}

export async function verifyLocalReleaseIdentity({
  candidateManifest,
  distDir,
  expectedChannel,
  publicKey,
}) {
  if (!["stable", "prerelease"].includes(expectedChannel)) {
    throw new Error(
      "release identity verification requires expectedChannel=stable or prerelease",
    );
  }
  const parsedCandidateVersion = parseSemver(
    assertManifestShape(candidateManifest, "release identity candidate"),
  );
  const candidateVersion = parsedCandidateVersion.raw;
  const semanticChannel =
    parsedCandidateVersion.prerelease.length > 0 ? "prerelease" : "stable";
  if (semanticChannel !== expectedChannel) {
    throw new Error(
      `release identity expected channel=${expectedChannel} but version ${candidateVersion} is ${semanticChannel}`,
    );
  }
  if (candidateManifest.channel !== expectedChannel) {
    throw new Error(
      `release identity candidate channel must be ${expectedChannel}, got ${candidateManifest.channel || "missing"}`,
    );
  }
  if (typeof candidateManifest.notes !== "string") {
    throw new Error("release identity candidate must contain release notes");
  }

  const identityPath = join(resolve(distDir), ROOT_IDENTITY_KEY);
  const signaturePath = join(resolve(distDir), ROOT_IDENTITY_SIGNATURE_KEY);
  const identity = await readJson(identityPath, ROOT_IDENTITY_KEY);
  if (identity.schema !== RELEASE_IDENTITY_SCHEMA_VERSION) {
    throw new Error(
      `${ROOT_IDENTITY_KEY} schema must be ${RELEASE_IDENTITY_SCHEMA_VERSION}`,
    );
  }
  if (identity.channel !== expectedChannel) {
    throw new Error(
      `${ROOT_IDENTITY_KEY} channel must be ${expectedChannel}, got ${identity.channel || "missing"}`,
    );
  }
  if (parseSemver(identity.version).raw !== candidateVersion) {
    throw new Error(
      `${ROOT_IDENTITY_KEY} version ${identity.version} does not match candidate ${candidateVersion}`,
    );
  }
  const expectedNotesSha256 = createHash("sha256")
    .update(candidateManifest.notes, "utf8")
    .digest("hex");
  if (
    assertSha256(identity.notes_sha256, `${ROOT_IDENTITY_KEY} notes_sha256`) !==
    expectedNotesSha256
  ) {
    throw new Error(`${ROOT_IDENTITY_KEY} notes_sha256 does not match candidate notes`);
  }
  if (
    !identity.platforms ||
    typeof identity.platforms !== "object" ||
    Array.isArray(identity.platforms)
  ) {
    throw new Error(`${ROOT_IDENTITY_KEY} has no platforms object`);
  }

  const candidatePlatforms = Object.keys(candidateManifest.platforms).sort();
  const identityPlatforms = Object.keys(identity.platforms).sort();
  if (JSON.stringify(identityPlatforms) !== JSON.stringify(candidatePlatforms)) {
    throw new Error(
      `${ROOT_IDENTITY_KEY} platforms do not exactly match the stable mirror candidate`,
    );
  }
  const seenArtifacts = new Set();
  for (const platform of candidatePlatforms) {
    const manifestEntry = candidateManifest.platforms[platform];
    const identityEntry = identity.platforms[platform];
    if (
      !identityEntry ||
      typeof identityEntry !== "object" ||
      Array.isArray(identityEntry)
    ) {
      throw new Error(`${ROOT_IDENTITY_KEY} platform ${platform} is invalid`);
    }
    if (typeof identityEntry.artifact !== "string") {
      throw new Error(
        `${ROOT_IDENTITY_KEY} platform ${platform} is missing artifact`,
      );
    }
    const artifact = assertSafeSegment(
      identityEntry.artifact,
      `${ROOT_IDENTITY_KEY} platform ${platform} artifact`,
    );
    if (seenArtifacts.has(artifact)) {
      throw new Error(`${ROOT_IDENTITY_KEY} reuses artifact ${artifact}`);
    }
    seenArtifacts.add(artifact);
    const manifestArtifact = decodeURIComponent(
      new URL(manifestEntry.url).pathname.split("/").filter(Boolean).at(-1) || "",
    );
    if (artifact !== manifestArtifact) {
      throw new Error(
        `${ROOT_IDENTITY_KEY} platform ${platform} artifact does not match candidate URL`,
      );
    }
    const artifactPath = join(resolve(distDir), artifact);
    const artifactMetadata = await stat(artifactPath).catch((error) => {
      throw new Error(
        `${ROOT_IDENTITY_KEY} platform ${platform} artifact is missing: ${errorText(error)}`,
      );
    });
    if (!artifactMetadata.isFile() || artifactMetadata.size <= 0) {
      throw new Error(`${ROOT_IDENTITY_KEY} platform ${platform} artifact is empty`);
    }
    const expectedArtifactSha256 = assertSha256(
      identityEntry.sha256,
      `${ROOT_IDENTITY_KEY} platform ${platform} sha256`,
    );
    const localArtifactSha256 = await sha256File(artifactPath);
    if (localArtifactSha256 !== expectedArtifactSha256) {
      throw new Error(
        `${ROOT_IDENTITY_KEY} platform ${platform} sha256 does not match local artifact`,
      );
    }
    if (manifestEntry.sha256 !== expectedArtifactSha256) {
      throw new Error(
        `${ROOT_IDENTITY_KEY} platform ${platform} sha256 does not match candidate manifest`,
      );
    }
  }

  let signature;
  try {
    signature = (await readFile(signaturePath, "utf8")).trim();
  } catch (error) {
    throw new Error(
      `${ROOT_IDENTITY_SIGNATURE_KEY} is missing: ${errorText(error)}`,
    );
  }
  if (!signature) throw new Error(`${ROOT_IDENTITY_SIGNATURE_KEY} is empty`);
  await verifyTauriUpdaterSignature(identityPath, signature, publicKey);
  const [identitySha256, signatureSha256, identityMetadata, signatureMetadata] =
    await Promise.all([
      sha256File(identityPath),
      sha256File(signaturePath),
      stat(identityPath),
      stat(signaturePath),
    ]);
  return {
    identity,
    identityPath,
    identitySha256,
    identitySize: identityMetadata.size,
    signature,
    signaturePath,
    signatureSha256,
    signatureSize: signatureMetadata.size,
  };
}

async function loadStableReleaseIdentity(options) {
  if (options.candidateManifest.channel !== "stable") {
    throw new Error(
      `root release identity accepts only channel=stable, got ${options.candidateManifest.channel || "missing"}`,
    );
  }
  assertRequiredUpdaterPlatforms(
    options.candidateManifest,
    "stable mirror candidate",
  );
  return await verifyLocalReleaseIdentity({
    ...options,
    expectedChannel: "stable",
  });
}

export function assertCandidateMatchesRelease(candidateManifest, artifactManifest, releaseTag) {
  const tag = String(releaseTag);
  if (!tag.startsWith("v")) {
    throw new Error(`release tag must start with v: ${releaseTag}`);
  }
  const expectedVersion = tag.slice(1);
  if (parseSemver(expectedVersion).raw !== expectedVersion) {
    throw new Error(`release tag must contain a canonical semantic version: ${releaseTag}`);
  }

  assertManifestShape(candidateManifest, "candidate latest.json");
  assertManifestShape(artifactManifest, "artifact-derived latest.json");
  assertRequiredUpdaterPlatforms(candidateManifest, "candidate latest.json");
  assertRequiredUpdaterPlatforms(artifactManifest, "artifact-derived latest.json");
  if (candidateManifest.version !== expectedVersion) {
    throw new Error(
      `candidate latest.json version ${candidateManifest.version} does not match release tag ${releaseTag}`,
    );
  }
  if (artifactManifest.version !== expectedVersion) {
    throw new Error(
      `artifact-derived latest.json version ${artifactManifest.version} does not match release tag ${releaseTag}`,
    );
  }
  if (manifestReleaseIdentity(candidateManifest) !== manifestReleaseIdentity(artifactManifest)) {
    throw new Error(
      "candidate latest.json channel/notes/platforms/signatures/sha256 do not match the artifact-derived manifest",
    );
  }
  return {
    platformCount: Object.keys(candidateManifest.platforms).length,
    version: expectedVersion,
  };
}

export async function createMirrorManifest(distDir, mirrorBaseValue = DEFAULT_MIRROR_BASE) {
  const dist = resolve(distDir);
  const sourcePath = join(dist, "latest.json");
  const manifest = await readJson(sourcePath, "release latest.json");
  const version = assertManifestShape(manifest, "release latest.json");
  const mirrorBase = normalizeMirrorBase(mirrorBaseValue);
  const rewritten = structuredClone(manifest);
  for (const [platform, entry] of Object.entries(rewritten.platforms)) {
    const original = new URL(entry.url);
    const rawName = original.pathname.split("/").filter(Boolean).at(-1);
    if (!rawName) throw new Error(`platform ${platform} URL has no artifact name`);
    const name = decodeURIComponent(rawName);
    assertSafeSegment(name, `platform ${platform} artifact name`);
    entry.url = `${mirrorBase}/${encodeURIComponent(version)}/${encodeURIComponent(name)}`;
  }
  const outputPath = join(dist, "latest.mirror.json");
  await writeJson(outputPath, rewritten);
  return { manifest: rewritten, mirrorBase, outputPath, version };
}

async function expectedArtifactsFromManifest(manifest, distDir, mirrorBase) {
  const version = assertManifestShape(manifest, "mirror candidate");
  const expectedBase = `${normalizeMirrorBase(mirrorBase)}/${encodeURIComponent(version)}/`;
  const updaterByName = new Map();
  for (const [platform, entry] of Object.entries(manifest.platforms).sort(([a], [b]) => a.localeCompare(b))) {
    if (!entry.url.startsWith(expectedBase)) {
      throw new Error(`platform ${platform} URL is not under ${expectedBase}`);
    }
    const parsed = new URL(entry.url);
    const rawName = parsed.pathname.split("/").filter(Boolean).at(-1);
    const name = rawName ? decodeURIComponent(rawName) : "";
    assertSafeSegment(name, `platform ${platform} artifact name`);
    if (updaterByName.has(name)) {
      throw new Error(`multiple updater platforms resolve to the same artifact: ${name}`);
    }
    updaterByName.set(name, {
      platform,
      sha256: entry.sha256,
      signature: entry.signature,
    });
  }

  for (const [name, updater] of updaterByName) {
    const sidecarPath = join(resolve(distDir), `${name}.sig`);
    let sidecar;
    try {
      sidecar = (await readFile(sidecarPath, "utf8")).trim();
    } catch (error) {
      throw new Error(
        `local updater signature sidecar is missing for ${updater.platform}: ${errorText(error)}`,
      );
    }
    if (sidecar !== updater.signature) {
      throw new Error(
        `local updater signature sidecar does not match manifest for ${updater.platform}`,
      );
    }
  }

  const artifacts = [];
  const names = (await readdir(distDir))
    .filter((name) => !["latest.json", "latest.mirror.json"].includes(name))
    .sort();
  for (const name of names) {
    assertSafeSegment(name, "release artifact name");
    const localPath = join(resolve(distDir), name);
    const metadata = await stat(localPath);
    if (!metadata.isFile()) continue;
    const updater = updaterByName.get(name);
    const localSha256 = await sha256File(localPath);
    if (updater && localSha256 !== updater.sha256) {
      throw new Error(
        `local updater artifact sha256 does not match manifest for ${updater.platform}`,
      );
    }
    artifacts.push({
      platform: updater?.platform || null,
      name,
      key: `${version}/${name}`,
      localPath,
      size: metadata.size,
      sha256: localSha256,
      signature: updater ? updater.signature : null,
    });
    updaterByName.delete(name);
  }
  if (updaterByName.size > 0) {
    throw new Error(
      `local release artifacts are missing: ${[...updaterByName.keys()].join(", ")}`,
    );
  }
  return artifacts;
}

function publicProbeUrl(value, candidateKey, backend) {
  const parsed = new URL(value);
  if (parsed.protocol !== "https:") {
    throw new Error(`public mirror probe requires HTTPS: ${value}`);
  }
  parsed.searchParams.set(
    "cam_probe",
    createHash("sha256").update(candidateKey).digest("hex").slice(0, 24),
  );
  parsed.searchParams.set("cam_backend", backend);
  return parsed.toString();
}

function normalizedObjectKey(value, label, { allowEmpty = false } = {}) {
  const text = String(value || "").replace(/^\/+|\/+$/g, "");
  if (!text) {
    if (allowEmpty) return "";
    throw new Error(`${label} is empty`);
  }
  return text
    .split("/")
    .map((segment) => assertSafeSegment(segment, label))
    .join("/");
}

function normalizeIhepRedirectExpectation(value) {
  if (!value || typeof value !== "object") {
    throw new Error("IHEP public verification requires an expected redirect target");
  }
  const endpoint = new URL(value.endpoint);
  if (
    endpoint.protocol !== "https:" ||
    endpoint.username ||
    endpoint.password ||
    endpoint.search ||
    endpoint.hash
  ) {
    throw new Error(
      "IHEP redirect endpoint must be HTTPS without credentials, query, or fragment",
    );
  }
  let endpointPath;
  try {
    endpointPath = decodeURIComponent(endpoint.pathname).replace(/\/+$/g, "");
  } catch {
    throw new Error("IHEP redirect endpoint path is not valid percent-encoding");
  }
  return {
    origin: endpoint.origin,
    endpointPath,
    bucket: assertSafeSegment(String(value.bucket || "").trim(), "IHEP redirect bucket"),
    prefix: normalizedObjectKey(value.prefix, "IHEP redirect prefix", { allowEmpty: true }),
  };
}

function assertExpectedIhepRedirect(redirected, expectation, objectKey) {
  const key = normalizedObjectKey(objectKey, "IHEP redirect object key");
  const suffix = [expectation.bucket, expectation.prefix, key].filter(Boolean).join("/");
  const expectedPath = `${expectation.endpointPath}/${suffix}` || "/";
  let actualPath;
  try {
    actualPath = decodeURIComponent(redirected.pathname);
  } catch {
    throw new PublicRouteBindingError(
      "Worker IHEP redirect path is not valid percent-encoding",
    );
  }
  if (
    redirected.protocol !== "https:" ||
    redirected.username ||
    redirected.password ||
    redirected.hash ||
    redirected.origin !== expectation.origin ||
    actualPath !== expectedPath
  ) {
    throw new PublicRouteBindingError(
      "Worker IHEP redirect target does not match the configured endpoint, bucket, prefix, and object",
    );
  }
  if (
    redirected.searchParams.get("X-Amz-Algorithm") !== "AWS4-HMAC-SHA256" ||
    !redirected.searchParams.get("X-Amz-Credential") ||
    redirected.searchParams.get("X-Amz-SignedHeaders") !== "host" ||
    !/^[0-9a-f]{64}$/i.test(redirected.searchParams.get("X-Amz-Signature") || "")
  ) {
    throw new PublicRouteBindingError(
      "Worker IHEP redirect is not a complete SigV4 presigned URL",
    );
  }
}

async function downloadPublicObject(
  fetchImpl,
  url,
  destination,
  label,
  backend,
  ihepRedirect,
) {
  await mkdir(dirname(destination), { recursive: true });
  let lastError;
  for (let attempt = 1; attempt <= 5; attempt += 1) {
    await rm(destination, { force: true });
    try {
      let response = await fetchImpl(url, {
        headers: {
          "Cache-Control": "no-cache",
          "User-Agent": "Codex-App-Manager release verifier",
        },
        redirect: backend === "ihep" ? "manual" : "follow",
        signal: AbortSignal.timeout(300_000),
      });
      const routedBackend = response.headers.get("X-Codex-Mirror-Backend");
      if (routedBackend !== backend) {
        await response.body?.cancel().catch(() => {});
        throw new Error(
          `Worker reported backend ${routedBackend || "missing"}, expected ${backend}`,
        );
      }
      if (backend === "ihep") {
        if (response.status !== 302) {
          await response.body?.cancel().catch(() => {});
          throw new Error(`Worker IHEP probe returned HTTP ${response.status}`);
        }
        const location = response.headers.get("Location");
        const redirected = location ? new URL(location, url) : null;
        if (!redirected) {
          throw new Error("Worker IHEP probe did not return a secure redirect");
        }
        assertExpectedIhepRedirect(redirected, ihepRedirect.expectation, ihepRedirect.objectKey);
        await response.body?.cancel().catch(() => {});
        response = await fetchImpl(redirected, {
          headers: { "User-Agent": "Codex-App-Manager release verifier" },
          // Do not let the expected IHEP endpoint redirect the verifier to a
          // different store after its Location has passed the binding check.
          redirect: "manual",
          signal: AbortSignal.timeout(300_000),
        });
      }
      if (!response.ok || !response.body) {
        await response.body?.cancel().catch(() => {});
        throw new Error(`HTTP ${response.status}`);
      }
      await pipeline(Readable.fromWeb(response.body), createWriteStream(destination));
      return { path: destination, size: (await stat(destination)).size, url };
    } catch (error) {
      await rm(destination, { force: true });
      if (error instanceof PublicRouteBindingError) throw error;
      lastError = error;
      if (attempt < 5) {
        await new Promise((resolveDelay) => setTimeout(resolveDelay, attempt * 1_000));
      }
    }
  }
  throw new Error(`${label} public ${backend} download failed: ${errorText(lastError)}`);
}

function bindExpectedUpdaterArtifacts(manifest, expectedArtifacts, label) {
  if (!Array.isArray(expectedArtifacts)) {
    throw new Error(`${label} expected artifacts must be an array`);
  }
  const expectedByPlatform = new Map();
  for (const artifact of expectedArtifacts) {
    if (!artifact?.platform) continue;
    if (expectedByPlatform.has(artifact.platform)) {
      throw new Error(`${label} has duplicate expected platform ${artifact.platform}`);
    }
    expectedByPlatform.set(artifact.platform, artifact);
  }
  const updaters = updaterArtifactsFromManifest(manifest, label);
  for (const updater of updaters) {
    const expected = expectedByPlatform.get(updater.platform);
    if (!expected || expected.name !== updater.name) {
      throw new Error(`${label} has no bound artifact for ${updater.platform}`);
    }
    if (expected.sha256 !== updater.sha256) {
      throw new Error(
        `${label} expected artifact sha256 does not match manifest for ${updater.platform}`,
      );
    }
  }
  if (expectedByPlatform.size !== updaters.length) {
    throw new Error(`${label} expected artifacts contain an unknown updater platform`);
  }
  return expectedByPlatform;
}

async function verifyPublicMirrorBackend({
  backend,
  candidateKey,
  candidateManifest,
  candidatePath,
  expectedByPlatform,
  mirrorBase,
  publicKey,
  publicDir,
  fetchImpl,
  ihepRedirectExpectation,
}) {
  const encodedCandidateKey = candidateKey
    .split("/")
    .map((segment) => encodeURIComponent(segment))
    .join("/");
  const candidateUrl = publicProbeUrl(
    `${normalizeMirrorBase(mirrorBase)}/${encodedCandidateKey}`,
    candidateKey,
    backend,
  );
  const downloadedCandidate = join(publicDir, backend, "candidate.json");
  const candidateResponse = await downloadPublicObject(
    fetchImpl,
    candidateUrl,
    downloadedCandidate,
    "mirror candidate",
    backend,
    backend === "ihep"
      ? { expectation: ihepRedirectExpectation, objectKey: candidateKey }
      : null,
  );
  const [localCandidateHash, publicCandidateHash] = await Promise.all([
    sha256File(candidatePath),
    sha256File(downloadedCandidate),
  ]);
  if (localCandidateHash !== publicCandidateHash) {
    throw new Error(`public mirror ${backend} candidate bytes do not match this release run`);
  }
  const publicManifest = await readJson(
    downloadedCandidate,
    `public mirror ${backend} candidate`,
  );
  if (
    manifestReleaseIdentity(publicManifest) !==
    manifestReleaseIdentity(candidateManifest)
  ) {
    throw new Error(
      `public mirror ${backend} candidate release identity does not match this release run`,
    );
  }

  const artifactReports = [];
  for (const updater of updaterArtifactsFromManifest(candidateManifest, "mirror candidate")) {
    const expected = expectedByPlatform.get(updater.platform);
    if (!expected || expected.name !== updater.name) {
      throw new Error(
        `public mirror ${backend} probe has no bound artifact for ${updater.platform}`,
      );
    }
    const publicPath = join(publicDir, backend, updater.name);
    const response = await downloadPublicObject(
      fetchImpl,
      publicProbeUrl(updater.url, candidateKey, backend),
      publicPath,
      `mirror artifact ${updater.name}`,
      backend,
      backend === "ihep"
        ? { expectation: ihepRedirectExpectation, objectKey: expected.key }
        : null,
    );
    if (response.size !== expected.size) {
      throw new Error(
        `public mirror ${backend} size mismatch for ${updater.name}: ${response.size} != ${expected.size}`,
      );
    }
    const publicHash = await sha256File(publicPath);
    if (publicHash !== expected.sha256) {
      throw new Error(`public mirror ${backend} sha256 mismatch for ${updater.name}`);
    }
    await verifyTauriUpdaterSignature(publicPath, updater.signature, publicKey);
    artifactReports.push({
      name: updater.name,
      platform: updater.platform,
      sha256: publicHash,
      size: response.size,
      url: updater.url,
    });
    await rm(publicPath, { force: true });
  }
  return {
    artifactCount: artifactReports.length,
    artifacts: artifactReports,
    backend,
    candidateSha256: publicCandidateHash,
    candidateSize: candidateResponse.size,
    candidateUrl,
    verified: true,
  };
}

export async function verifyPublicMirrorRoute({
  candidateKey,
  candidateManifest,
  candidatePath,
  expectedArtifacts,
  mirrorBase,
  publicKey,
  workDir,
  fetchImpl = fetch,
  ihepRedirect,
}) {
  const ihepRedirectExpectation = normalizeIhepRedirectExpectation(ihepRedirect);
  const expectedByPlatform = bindExpectedUpdaterArtifacts(
    candidateManifest,
    expectedArtifacts,
    "public mirror candidate",
  );
  const backends = {};
  for (const backend of ["r2", "ihep"]) {
    backends[backend] = await verifyPublicMirrorBackend({
      backend,
      candidateKey,
      candidateManifest,
      candidatePath,
      expectedByPlatform,
      mirrorBase,
      publicKey,
      publicDir: join(workDir, "public-route"),
      fetchImpl,
      ihepRedirectExpectation,
    });
  }
  return {
    artifactCount: updaterArtifactsFromManifest(candidateManifest, "mirror candidate").length,
    backendCount: Object.keys(backends).length,
    backends,
    verified: Object.values(backends).every((report) => report.verified),
  };
}

async function verifyPublicRootIdentityBackend({
  backend,
  bundle,
  candidateKey,
  fetchImpl,
  mirrorBase,
  publicDir,
  publicKey,
  ihepRedirectExpectation,
}) {
  const base = normalizeMirrorBase(mirrorBase);
  const backendDir = join(publicDir, backend);
  const publicIdentityPath = join(backendDir, ROOT_IDENTITY_KEY);
  const publicSignaturePath = join(backendDir, ROOT_IDENTITY_SIGNATURE_KEY);
  const identityResponse = await downloadPublicObject(
    fetchImpl,
    publicProbeUrl(`${base}/${ROOT_IDENTITY_KEY}`, candidateKey, backend),
    publicIdentityPath,
    "root release identity",
    backend,
    backend === "ihep"
      ? { expectation: ihepRedirectExpectation, objectKey: ROOT_IDENTITY_KEY }
      : null,
  );
  const signatureResponse = await downloadPublicObject(
    fetchImpl,
    publicProbeUrl(
      `${base}/${ROOT_IDENTITY_SIGNATURE_KEY}`,
      candidateKey,
      backend,
    ),
    publicSignaturePath,
    "root release identity signature",
    backend,
    backend === "ihep"
      ? { expectation: ihepRedirectExpectation, objectKey: ROOT_IDENTITY_SIGNATURE_KEY }
      : null,
  );
  const [identitySha256, signatureSha256] = await Promise.all([
    sha256File(publicIdentityPath),
    sha256File(publicSignaturePath),
  ]);
  if (
    identityResponse.size !== bundle.identitySize ||
    identitySha256 !== bundle.identitySha256
  ) {
    throw new Error(
      `public mirror ${backend} root release identity does not match this release run`,
    );
  }
  if (
    signatureResponse.size !== bundle.signatureSize ||
    signatureSha256 !== bundle.signatureSha256
  ) {
    throw new Error(
      `public mirror ${backend} root release identity signature does not match this release run`,
    );
  }
  const signature = (await readFile(publicSignaturePath, "utf8")).trim();
  await verifyTauriUpdaterSignature(publicIdentityPath, signature, publicKey);
  const publicIdentity = await readJson(
    publicIdentityPath,
    `public mirror ${backend} root release identity`,
  );
  if (
    publicIdentity.schema !== RELEASE_IDENTITY_SCHEMA_VERSION ||
    publicIdentity.channel !== "stable" ||
    publicIdentity.version !== bundle.identity.version
  ) {
    throw new Error(
      `public mirror ${backend} root release identity has an unexpected schema/channel/version`,
    );
  }
  return {
    backend,
    channel: publicIdentity.channel,
    identitySha256,
    identitySize: identityResponse.size,
    signatureSha256,
    signatureSize: signatureResponse.size,
    verified: true,
    version: publicIdentity.version,
  };
}

export async function verifyPublicRootIdentityPointers({
  bundle,
  candidateKey,
  fetchImpl = fetch,
  mirrorBase,
  publicKey,
  workDir,
  ihepRedirect,
}) {
  const ihepRedirectExpectation = normalizeIhepRedirectExpectation(ihepRedirect);
  const backends = {};
  for (const backend of ["r2", "ihep"]) {
    backends[backend] = await verifyPublicRootIdentityBackend({
      backend,
      bundle,
      candidateKey,
      fetchImpl,
      mirrorBase,
      publicDir: join(workDir, "root-identity"),
      publicKey,
      ihepRedirectExpectation,
    });
  }
  return {
    backendCount: Object.keys(backends).length,
    backends,
    verified: Object.values(backends).every((report) => report.verified),
    version: bundle.identity.version,
  };
}

function isNotFound(text) {
  return /(?:\b404\b|NoSuchKey|Not Found|does not exist)/i.test(text);
}

function isConditionalFailure(text) {
  return /(?:\b409\b|\b412\b|PreconditionFailed|ConditionalRequestConflict|pre-?condition|conditional request conflict)/i.test(
    text,
  );
}

async function runProcess(command, args, options = {}) {
  if (interruptedBy && !rollbackInProgress) {
    throw new Error(`release interrupted by ${interruptedBy}`);
  }
  return await new Promise((resolvePromise, rejectPromise) => {
    const child = spawn(command, args, {
      cwd: options.cwd,
      env: options.env,
      stdio: ["ignore", "pipe", "pipe"],
    });
    activeChildren.add(child);
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (chunk) => {
      if (stdout.length < 1_000_000) stdout += chunk.toString();
    });
    child.stderr.on("data", (chunk) => {
      if (stderr.length < 1_000_000) stderr += chunk.toString();
    });
    child.on("error", (error) => {
      activeChildren.delete(child);
      rejectPromise(error);
    });
    child.on("close", (code, signal) => {
      activeChildren.delete(child);
      const result = { code: code ?? 1, signal, stderr, stdout };
      if (code === 0 || options.allowFailure) resolvePromise(result);
      else rejectPromise(new Error(`${command} failed (${code ?? signal}): ${stderr || stdout}`));
    });
  });
}

export class AwsObjectStore {
  constructor({ name, endpoint, bucket, region, prefix = "", accessKeyId, secretAccessKey, configPath }) {
    this.name = name;
    this.endpoint = endpoint;
    this.bucket = bucket;
    this.region = region || "auto";
    this.prefix = prefix.replace(/^\/+|\/+$/g, "");
    this.configPath = configPath;
    this.processEnv = {
      ...process.env,
      AWS_ACCESS_KEY_ID: accessKeyId,
      AWS_SECRET_ACCESS_KEY: secretAccessKey,
      AWS_DEFAULT_REGION: this.region,
      AWS_EC2_METADATA_DISABLED: "true",
      AWS_CONFIG_FILE: configPath,
    };
  }

  objectKey(key) {
    return this.prefix ? `${this.prefix}/${key}` : key;
  }

  s3Uri(key) {
    return `s3://${this.bucket}/${this.objectKey(key)}`;
  }

  async aws(args, options = {}) {
    const { env: envOverride = {}, ...runOptions } = options;
    return await runProcess(
      "aws",
      [...args, "--endpoint-url", this.endpoint],
      {
        ...runOptions,
        env: { ...this.processEnv, ...envOverride },
      },
    );
  }

  async head(key) {
    const result = await this.aws(
      [
        "s3api",
        "head-object",
        "--bucket",
        this.bucket,
        "--key",
        this.objectKey(key),
        "--output",
        "json",
      ],
      { allowFailure: true },
    );
    if (result.code !== 0) {
      const detail = `${result.stderr}\n${result.stdout}`;
      if (isNotFound(detail)) return null;
      throw new Error(`${this.name}: cannot read object metadata for ${key}: ${detail.trim()}`);
    }
    const metadata = JSON.parse(result.stdout);
    if (!metadata.ETag || !Number.isFinite(metadata.ContentLength)) {
      throw new Error(`${this.name}: incomplete object metadata for ${key}`);
    }
    const objectMetadata = Object.fromEntries(
      Object.entries(metadata.Metadata || {}).map(([name, value]) => [
        name.toLowerCase(),
        String(value),
      ]),
    );
    return { etag: metadata.ETag, metadata: objectMetadata, size: metadata.ContentLength };
  }

  async download(key, destination) {
    await mkdir(dirname(destination), { recursive: true });
    await this.aws([
      "s3",
      "cp",
      this.s3Uri(key),
      destination,
      "--only-show-errors",
    ]);
  }

  async snapshot(key, destination) {
    for (let attempt = 1; attempt <= 3; attempt += 1) {
      const before = await this.head(key);
      if (!before) return { exists: false, etag: null, size: 0, path: null };
      await this.download(key, destination);
      const after = await this.head(key);
      const localSize = (await stat(destination)).size;
      if (after && before.etag === after.etag && after.size === localSize) {
        return {
          exists: true,
          etag: after.etag,
          metadata: after.metadata,
          size: localSize,
          path: destination,
        };
      }
      await rm(destination, { force: true });
      if (attempt === 3) {
        throw new Error(`${this.name}: ${key} changed repeatedly while being downloaded`);
      }
    }
    throw new Error(`${this.name}: could not snapshot ${key}`);
  }

  async putObject(
    localPath,
    key,
    { contentType, ifMatch, ifNoneMatch, metadata, singleAttempt = false } = {},
  ) {
    const args = [
      "s3api",
      "put-object",
      "--bucket",
      this.bucket,
      "--key",
      this.objectKey(key),
      "--body",
      localPath,
      "--content-type",
      contentType || "application/octet-stream",
      "--output",
      "json",
    ];
    if (ifMatch) args.push("--if-match", ifMatch);
    if (ifNoneMatch) args.push("--if-none-match", ifNoneMatch);
    if (metadata && Object.keys(metadata).length > 0) {
      args.push(
        "--metadata",
        Object.entries(metadata)
          .map(([name, value]) => `${name}=${value}`)
          .join(","),
      );
    }
    const result = await this.aws(args, {
      allowFailure: true,
      ...(singleAttempt ? { env: { AWS_MAX_ATTEMPTS: "1" } } : {}),
    });
    if (result.code !== 0) {
      const detail = `${result.stderr}\n${result.stdout}`.trim();
      if (isConditionalFailure(detail)) {
        throw new ConditionalWriteError(`${this.name}: conditional write rejected for ${key}`);
      }
      throw new Error(`${this.name}: upload failed for ${key}: ${detail}`);
    }
    let response;
    try {
      response = JSON.parse(result.stdout);
    } catch (error) {
      throw new WriteOutcomeUncertainError(
        `${this.name}: ${key} upload succeeded but its response was not valid JSON: ${errorText(error)}`,
      );
    }
    if (typeof response.ETag !== "string" || !response.ETag) {
      throw new WriteOutcomeUncertainError(
        `${this.name}: ${key} upload response did not include the committed ETag`,
      );
    }
    return { etag: response.ETag, size: (await stat(localPath)).size };
  }

  async putImmutable(localPath, key, contentType, metadata = {}) {
    const existing = await this.head(key);
    if (existing) return { status: "existing", ...existing };
    try {
      const uploaded = await this.putObject(localPath, key, {
        contentType,
        ifNoneMatch: "*",
        metadata,
      });
      return { status: "uploaded", ...uploaded };
    } catch (error) {
      if (!(error instanceof ConditionalWriteError)) throw error;
      const raced = await this.head(key);
      if (!raced) throw error;
      return { status: "existing-after-race", ...raced };
    }
  }

  async putLatestConditional(localPath, expectedEtag, promotionToken = "") {
    return await this.putObject(localPath, LATEST_KEY, {
      contentType: "application/json",
      ...(expectedEtag ? { ifMatch: expectedEtag } : { ifNoneMatch: "*" }),
      ...(promotionToken
        ? { metadata: { "cam-promotion-token": promotionToken } }
        : {}),
      singleAttempt: true,
    });
  }

  async putLatestUnconditional(localPath, promotionToken = "") {
    return await this.putObject(localPath, LATEST_KEY, {
      contentType: "application/json",
      ...(promotionToken
        ? { metadata: { "cam-promotion-token": promotionToken } }
        : {}),
      singleAttempt: true,
    });
  }

  async putRootIdentityPointer(localPath, key, promotionToken = "") {
    if (![ROOT_IDENTITY_KEY, ROOT_IDENTITY_SIGNATURE_KEY].includes(key)) {
      throw new Error(`unsupported root identity pointer key: ${key}`);
    }
    return await this.putObject(localPath, key, {
      contentType: contentType(key),
      ...(promotionToken
        ? { metadata: { "cam-promotion-token": promotionToken } }
        : {}),
      singleAttempt: true,
    });
  }
}

function contentType(name) {
  if (name.endsWith(".json")) return "application/json";
  if (name.endsWith(".tar.gz")) return "application/gzip";
  if (name.endsWith(".dmg")) return "application/x-apple-diskimage";
  return "application/octet-stream";
}

function requiredValue(env, name, backendName) {
  const value = env[name]?.trim();
  if (!value) throw new Error(`${backendName} backend requires ${name}`);
  return value;
}

export function backendConfigsFromEnv(env, configPath) {
  const required = (env.MIRROR_REQUIRED_BACKENDS || "r2,ihep")
    .split(",")
    .map((name) => name.trim().toLowerCase())
    .filter(Boolean);
  const unknown = required.filter((name) => !["r2", "ihep"].includes(name));
  if (unknown.length > 0) throw new Error(`unknown MIRROR_REQUIRED_BACKENDS: ${unknown.join(", ")}`);
  if (!required.includes("r2") || !required.includes("ihep")) {
    throw new Error("stable mirror publication requires both r2 and ihep backends");
  }
  return [
    new AwsObjectStore({
      name: "r2",
      endpoint: requiredValue(env, "MANAGER_R2_S3_ENDPOINT", "r2"),
      bucket: env.MANAGER_R2_BUCKET?.trim() || "codex-app-manager",
      region: "auto",
      accessKeyId: requiredValue(env, "MANAGER_R2_ACCESS_KEY_ID", "r2"),
      secretAccessKey: requiredValue(env, "MANAGER_R2_SECRET_ACCESS_KEY", "r2"),
      configPath,
    }),
    new AwsObjectStore({
      name: "ihep",
      endpoint: requiredValue(env, "MANAGER_IHEP_S3_ENDPOINT", "ihep"),
      bucket: requiredValue(env, "MANAGER_IHEP_S3_BUCKET", "ihep"),
      region: env.MANAGER_IHEP_S3_REGION?.trim() || "auto",
      prefix: env.MANAGER_IHEP_S3_PREFIX?.trim() || "",
      accessKeyId: requiredValue(env, "MANAGER_IHEP_S3_ACCESS_KEY_ID", "ihep"),
      secretAccessKey: requiredValue(env, "MANAGER_IHEP_S3_SECRET_ACCESS_KEY", "ihep"),
      configPath,
    }),
  ];
}

export function downgradeOverrideFromEnv(env = process.env) {
  const requested = env.MIRROR_ALLOW_DOWNGRADE === "1" || env.MIRROR_ALLOW_DOWNGRADE === "true";
  const reason = env.MIRROR_DOWNGRADE_REASON?.trim() || "";
  const eventName = env.GITHUB_EVENT_NAME || "";
  const originalActor = env.GITHUB_ACTOR || "";
  const triggeringActor = env.GITHUB_TRIGGERING_ACTOR || "";
  const actor = triggeringActor || originalActor;
  const repository = env.GITHUB_REPOSITORY || "";
  const runId = env.GITHUB_RUN_ID || "";
  const workflowRefName = env.MIRROR_WORKFLOW_REF_NAME || "";
  const defaultBranch = env.MIRROR_DEFAULT_BRANCH || "";
  const runUrl = repository && runId ? `https://github.com/${repository}/actions/runs/${runId}` : "";
  if (requested) {
    if (eventName !== "workflow_dispatch") {
      throw new Error("mirror downgrade override is accepted only from workflow_dispatch");
    }
    if (!workflowRefName || !defaultBranch || workflowRefName !== defaultBranch) {
      throw new Error("mirror downgrade override must run from the repository default branch");
    }
    if (reason.length < 10) {
      throw new Error("mirror downgrade override requires an audit reason of at least 10 characters");
    }
    if (!actor || !runUrl) {
      throw new Error("mirror downgrade override requires GitHub actor and run audit metadata");
    }
  }
  return {
    actor,
    defaultBranch,
    eventName,
    originalActor,
    reason,
    requested,
    runUrl,
    used: false,
    triggeringActor,
    workflowRefName,
  };
}

async function updaterPublicKeyFromRepo() {
  const config = await readJson(join(REPO_ROOT, "src-tauri", "tauri.conf.json"), "tauri.conf.json");
  const pubkey = config.plugins?.updater?.pubkey;
  if (typeof pubkey !== "string" || !pubkey.trim()) {
    throw new Error("tauri.conf.json has no updater public key");
  }
  return pubkey.trim();
}

export async function verifyBackendCandidate({
  backend,
  candidateKey,
  candidatePath,
  candidateManifest,
  distDir,
  mirrorBase,
  publicKey,
  workDir,
  expectedArtifacts: preparedArtifacts,
  candidateSha256,
  promotionToken,
}) {
  const backendDir = join(workDir, backend.name);
  await mkdir(backendDir, { recursive: true });
  const downloadedCandidate = join(backendDir, "candidate.json");
  const remoteCandidate = await backend.snapshot(candidateKey, downloadedCandidate);
  if (!remoteCandidate.exists) throw new Error(`${backend.name}: candidate is missing at ${candidateKey}`);
  if (
    promotionToken &&
    remoteCandidate.metadata?.["cam-promotion-token"] !== promotionToken
  ) {
    throw new Error(
      `${backend.name}: candidate metadata does not preserve the promotion token`,
    );
  }
  const [localCandidateHash, remoteCandidateHash] = await Promise.all([
    candidateSha256 ? Promise.resolve(candidateSha256) : sha256File(candidatePath),
    sha256File(downloadedCandidate),
  ]);
  if (localCandidateHash !== remoteCandidateHash) {
    throw new Error(`${backend.name}: candidate bytes do not match this release run`);
  }
  const remoteManifest = await readJson(downloadedCandidate, `${backend.name} candidate`);
  const expectedVersion = assertManifestShape(candidateManifest, "local mirror candidate");
  const remoteVersion = assertManifestShape(remoteManifest, `${backend.name} candidate`);
  if (remoteVersion !== expectedVersion || manifestReleaseIdentity(remoteManifest) !== manifestReleaseIdentity(candidateManifest)) {
    throw new Error(`${backend.name}: candidate release identity does not match version ${expectedVersion}`);
  }

  const expectedArtifacts =
    preparedArtifacts ||
    (await expectedArtifactsFromManifest(candidateManifest, distDir, mirrorBase));
  bindExpectedUpdaterArtifacts(
    candidateManifest,
    expectedArtifacts,
    `${backend.name} candidate`,
  );
  const artifactReports = [];
  for (const artifact of expectedArtifacts) {
    const remotePath = join(backendDir, artifact.name);
    const remote = await backend.snapshot(artifact.key, remotePath);
    if (!remote.exists) throw new Error(`${backend.name}: artifact is missing: ${artifact.key}`);
    const remoteHash = await sha256File(remotePath);
    if (remote.size !== artifact.size) {
      throw new Error(
        `${backend.name}: size mismatch for ${artifact.name}: ${remote.size} != ${artifact.size}`,
      );
    }
    if (remoteHash !== artifact.sha256) {
      throw new Error(`${backend.name}: sha256 mismatch for ${artifact.name}`);
    }
    if (artifact.platform) {
      if (!artifact.signature) {
        throw new Error(`${backend.name}: updater artifact ${artifact.name} has no signature`);
      }
      await verifyTauriUpdaterSignature(remotePath, artifact.signature, publicKey);
    }
    artifactReports.push({
      name: artifact.name,
      platform: artifact.platform,
      sha256: remoteHash,
      signatureVerified: artifact.platform ? true : null,
      size: remote.size,
    });
    await rm(remotePath, { force: true });
  }
  return {
    artifactCount: artifactReports.length,
    artifacts: artifactReports,
    candidateEtag: remoteCandidate.etag,
    candidateSha256: remoteCandidateHash,
    verified: true,
    version: remoteVersion,
  };
}

function initialSummary(phase, version, candidateKey, override) {
  return {
    schemaVersion: SUMMARY_SCHEMA_VERSION,
    phase,
    candidateVersion: version,
    candidateKey,
    startedAt: nowIso(),
    finishedAt: null,
    outcome: "running",
    error: null,
    override: { ...override },
    backends: [],
  };
}

async function finishSummary(path, summary, outcome, error = null) {
  summary.finishedAt = nowIso();
  summary.outcome = outcome;
  summary.error = error ? safeSummaryError(error) : null;
  await writeJson(path, summary);
}

export async function stageMirrors({
  backends,
  candidateKey,
  candidatePath,
  distDir,
  summaryPath,
  version,
  override,
}) {
  const summary = initialSummary("stage", version, candidateKey, override);
  summary.backends = backends.map((backend) => ({
    name: backend.name,
    candidate: null,
    assets: [],
    status: "not-started",
  }));
  const promotionToken = promotionTokenForCandidate(candidateKey);
  let activeSummary = null;
  try {
    const names = (await readdir(distDir)).sort();
    const assets = names.filter((name) => !["latest.json", "latest.mirror.json"].includes(name));
    for (const backend of backends) {
      const backendSummary = summary.backends.find((entry) => entry.name === backend.name);
      activeSummary = backendSummary;
      backendSummary.status = "staging";
      for (const name of assets) {
        assertSafeSegment(name, "release artifact name");
        const localPath = join(distDir, name);
        const metadata = await stat(localPath);
        if (!metadata.isFile()) continue;
        const uploaded = await backend.putImmutable(
          localPath,
          `${version}/${name}`,
          contentType(name),
        );
        backendSummary.assets.push({ name, status: uploaded.status });
      }
      const candidate = await backend.putImmutable(
        candidatePath,
        candidateKey,
        "application/json",
        { "cam-promotion-token": promotionToken },
      );
      backendSummary.candidate = { key: candidateKey, status: candidate.status };
      backendSummary.status = "staged";
    }
    await finishSummary(summaryPath, summary, "staged");
    return summary;
  } catch (error) {
    if (activeSummary?.status === "staging") {
      activeSummary.status = "failed";
      activeSummary.error = safeSummaryError(error);
    }
    await finishSummary(summaryPath, summary, "failed", error);
    throw error;
  }
}

async function readCurrentState(backend, workDir, candidateManifest) {
  const path = join(workDir, `${backend.name}-previous-latest.json`);
  const snapshot = await backend.snapshot(LATEST_KEY, path);
  if (!snapshot.exists) {
    throw new Error(
      `${backend.name}: latest.json is absent; seed both backends with the same valid baseline before enabling single-writer promotion`,
    );
  }
  const manifest = await readJson(path, `${backend.name} current latest.json`);
  const currentLabel = `${backend.name} current latest.json`;
  const candidateVersion = assertManifestShape(candidateManifest, "candidate latest.json");
  // Releases published before per-platform digests were introduced are valid
  // only as a one-way migration baseline. Missing digests may be crossed by a
  // strictly newer, fully bound candidate, but may never participate in
  // same-version reuse or a downgrade (including an operator override).
  const currentVersion = assertManifestShape(manifest, currentLabel, {
    allowMissingSha256: true,
  });
  const comparison = compareSemver(currentVersion, candidateVersion);
  let decision;
  if (comparison < 0) decision = "promote-forward";
  else {
    // At equal or newer versions the baseline itself is part of the claimed
    // artifact identity, so legacy manifests must fail closed.
    assertManifestShape(manifest, currentLabel);
    if (comparison > 0) decision = "blocked-downgrade";
    else if (manifestReleaseIdentity(manifest) === manifestReleaseIdentity(candidateManifest)) {
      decision = "idempotent";
    } else {
      decision = "blocked-same-version-mismatch";
    }
  }
  return {
    backend,
    currentIdentity: manifestReleaseIdentity(manifest),
    currentManifest: manifest,
    currentVersion,
    decision,
    previous: snapshot,
  };
}

async function snapshotMatchesFile(snapshot, expectedPath) {
  if (!snapshot.exists) return false;
  const [actual, expected] = await Promise.all([
    sha256File(snapshot.path),
    sha256File(expectedPath),
  ]);
  return actual === expected;
}

async function observeAmbiguousWrite(
  state,
  candidatePath,
  workDir,
  promotionToken,
) {
  try {
    const observed = await state.backend.snapshot(
      LATEST_KEY,
      join(workDir, `${state.backend.name}-ambiguous-latest.json`),
    );
    if (
      observed.metadata?.["cam-promotion-token"] === promotionToken &&
      (await snapshotMatchesFile(observed, candidatePath))
    ) {
      return observed;
    }
  } catch {
    // The original write error remains the primary failure. Rollback reporting
    // will make any uncertainty visible in the audit summary.
  }
  return null;
}

async function withRollbackIo(callback) {
  const previous = rollbackInProgress;
  rollbackInProgress = true;
  try {
    return await callback();
  } finally {
    rollbackInProgress = previous;
  }
}

function promotionBackends(backends) {
  const r2 = backends.filter((backend) => backend.name === "r2");
  const ihep = backends.filter((backend) => backend.name === "ihep");
  if (backends.length !== 2 || r2.length !== 1 || ihep.length !== 1) {
    throw new Error("mirror promotion requires exactly one r2 authority and one ihep follower");
  }
  return { ihep: ihep[0], r2: r2[0] };
}

async function assertSnapshotUnchanged(state) {
  const current = await state.backend.head(LATEST_KEY);
  if (!current || current.etag !== state.previous.etag) {
    throw new ConditionalWriteError(
      `${state.backend.name}: latest.json changed after the promotion snapshot`,
    );
  }
}

async function classifyFollowerCoverage(snapshot, candidateManifest, label) {
  if (!snapshot.exists) throw new Error(`${label}: latest.json disappeared`);
  const manifest = await readJson(snapshot.path, `${label} latest.json`);
  const followerVersion = assertManifestShape(manifest, `${label} latest.json`);
  const candidateVersion = assertManifestShape(candidateManifest, "candidate latest.json");
  const comparison = compareSemver(followerVersion, candidateVersion);
  if (comparison > 0) {
    return { coverage: "higher-version", manifest, version: followerVersion };
  }
  if (
    comparison === 0 &&
    manifestReleaseIdentity(manifest) === manifestReleaseIdentity(candidateManifest)
  ) {
    return { coverage: "candidate", manifest, version: followerVersion };
  }
  if (comparison === 0) {
    throw new Error(
      `${label}: latest.json has the candidate version but different channel/notes/artifact/signature identity`,
    );
  }
  return { coverage: "behind", manifest, version: followerVersion };
}

async function refreshFollowerAfterAuthorityCas(state, candidateManifest, workDir) {
  const snapshot = await state.backend.snapshot(
    LATEST_KEY,
    join(workDir, "ihep-post-authority-cas.json"),
  );
  if (!snapshot.exists) throw new Error("ihep: latest.json disappeared after the R2 CAS");
  if (snapshot.etag === state.previous.etag) {
    return { coverage: "snapshot-unchanged", needsWrite: true, snapshot };
  }
  const classified = await classifyFollowerCoverage(
    snapshot,
    candidateManifest,
    "IHEP after R2 CAS",
  );
  if (classified.coverage !== "candidate") {
    throw new ConditionalWriteError(
      `ihep: latest.json changed after the R2 CAS to ${classified.version}; exact candidate identity is required`,
    );
  }
  return { ...classified, needsWrite: false, snapshot };
}

async function ownedCandidateSnapshot({
  state,
  candidatePath,
  expectedEtag,
  promotionToken,
  workDir,
  label,
}) {
  const snapshot = await state.backend.snapshot(
    LATEST_KEY,
    join(workDir, `${state.backend.name}-${label}.json`),
  );
  if (
    !snapshot.exists ||
    snapshot.etag !== expectedEtag ||
    snapshot.metadata?.["cam-promotion-token"] !== promotionToken ||
    !(await snapshotMatchesFile(snapshot, candidatePath))
  ) {
    throw new ConditionalWriteError(
      `${state.backend.name}: latest.json is no longer owned by this promotion`,
    );
  }
  return snapshot;
}

async function rollbackAuthoritativePromotion({
  r2State,
  ihepState,
  r2Commit,
  followerWriteAttempted,
  candidatePath,
  promotionToken,
  summaryByName,
  workDir,
}) {
  const failures = [];
  const r2Summary = summaryByName.get("r2");
  const ihepSummary = summaryByName.get("ihep");
  rollbackInProgress = true;
  try {
    try {
      await ownedCandidateSnapshot({
        state: r2State,
        candidatePath,
        expectedEtag: r2Commit.etag,
        promotionToken,
        workDir,
        label: "rollback-ownership",
      });
    } catch (error) {
      r2Summary.rollback = "skipped-concurrent-change";
      r2Summary.rollbackError = safeSummaryError(error);
      if (followerWriteAttempted) ihepSummary.rollback = "preserved-authority-lost";
      failures.push(`r2: ${errorText(error)}`);
      return { failures, rolledBack: false };
    }

    try {
      if (!r2State.previous.exists) {
        throw new Error("authoritative promotion cannot roll back an unseeded R2 baseline");
      }
      const restored = await r2State.backend.putLatestConditional(
        r2State.previous.path,
        r2Commit.etag,
        `rollback-${randomUUID()}`,
      );
      const check = await r2State.backend.snapshot(
        LATEST_KEY,
        join(workDir, "r2-rollback-check.json"),
      );
      if (
        check.etag !== restored.etag ||
        !(await snapshotMatchesFile(check, r2State.previous.path))
      ) {
        throw new Error("R2 rollback verification did not match the previous latest.json");
      }
      r2Summary.rollback = "restored";
    } catch (error) {
      r2Summary.rollback = "failed";
      r2Summary.rollbackError = safeSummaryError(error);
      failures.push(`r2: ${errorText(error)}`);
      if (followerWriteAttempted) ihepSummary.rollback = "preserved-authority-rollback-failed";
      return { failures, rolledBack: false };
    }

    if (followerWriteAttempted) {
      try {
        const current = await ihepState.backend.snapshot(
          LATEST_KEY,
          join(workDir, "ihep-rollback-current.json"),
        );
        if (await snapshotMatchesFile(current, ihepState.previous.path)) {
          ihepSummary.rollback = "preserved-previous";
        } else if (
          current.exists &&
          current.metadata?.["cam-promotion-token"] === promotionToken &&
          (await snapshotMatchesFile(current, candidatePath))
        ) {
          const restored = await ihepState.backend.putLatestUnconditional(
            ihepState.previous.path,
            `rollback-${randomUUID()}`,
          );
          const check = await ihepState.backend.snapshot(
            LATEST_KEY,
            join(workDir, "ihep-rollback-check.json"),
          );
          if (
            check.etag !== restored.etag ||
            !(await snapshotMatchesFile(check, ihepState.previous.path))
          ) {
            throw new Error("IHEP rollback verification did not match the previous latest.json");
          }
          ihepSummary.rollback = "restored-unconditionally";
        } else {
          // IHEP cannot enforce conditional writes. If its object is neither the
          // snapshot nor this transaction's token+bytes, never overwrite it.
          ihepSummary.rollback = "preserved-concurrent-change";
        }
      } catch (error) {
        ihepSummary.rollback = "failed";
        ihepSummary.rollbackError = safeSummaryError(error);
        failures.push(`ihep: ${errorText(error)}`);
      }
    }
  } finally {
    rollbackInProgress = false;
  }
  return { failures, rolledBack: true };
}

async function reconcileFollowerToCurrentAuthority({
  r2State,
  ihepState,
  candidateManifest,
  candidatePath,
  promotionToken,
  summaryByName,
  workDir,
}) {
  const authority = await r2State.backend.snapshot(
    LATEST_KEY,
    join(workDir, "r2-superseding-authority.json"),
  );
  if (!authority.exists) throw new Error("r2: superseding latest.json disappeared");
  const authorityManifest = await readJson(
    authority.path,
    "R2 superseding authoritative latest.json",
  );
  const authorityVersion = assertManifestShape(
    authorityManifest,
    "R2 superseding authoritative latest.json",
  );
  const candidateVersion = assertManifestShape(candidateManifest, "candidate latest.json");
  const comparison = compareSemver(authorityVersion, candidateVersion);
  if (
    comparison < 0 ||
    (comparison === 0 &&
      manifestReleaseIdentity(authorityManifest) !==
        manifestReleaseIdentity(candidateManifest))
  ) {
    throw new Error(
      `r2: superseding authority ${authorityVersion} is not a safe monotonic successor of ${candidateVersion}`,
    );
  }

  const currentFollower = await ihepState.backend.snapshot(
    LATEST_KEY,
    join(workDir, "ihep-supersession-current.json"),
  );
  if (!currentFollower.exists) {
    throw new Error("ihep: latest.json disappeared during supersession repair");
  }
  if (await snapshotMatchesFile(currentFollower, authority.path)) {
    summaryByName.get("ihep").supersession = "already-follows-r2";
    return authorityManifest;
  }

  const followerManifest = await readJson(
    currentFollower.path,
    "IHEP latest.json during supersession repair",
  );
  const followerVersion = assertManifestShape(
    followerManifest,
    "IHEP latest.json during supersession repair",
  );
  if (compareSemver(followerVersion, authorityVersion) > 0) {
    // Never let an older workflow overwrite a follower that has already moved
    // beyond the R2 snapshot it observed.
    summaryByName.get("ihep").supersession = "preserved-newer-follower";
    return followerManifest;
  }

  const ownsFollower =
    (currentFollower.metadata?.["cam-promotion-token"] === promotionToken &&
      (await snapshotMatchesFile(currentFollower, candidatePath))) ||
    (currentFollower.etag === ihepState.previous.etag &&
      (await snapshotMatchesFile(currentFollower, ihepState.previous.path)));
  if (!ownsFollower) {
    throw new ConditionalWriteError(
      "ihep: cannot safely repair a follower object owned by another writer",
    );
  }

  const repaired = await ihepState.backend.putLatestUnconditional(
    authority.path,
    `superseded-${randomUUID()}`,
  );
  const check = await ihepState.backend.snapshot(
    LATEST_KEY,
    join(workDir, "ihep-supersession-check.json"),
  );
  if (
    check.etag === repaired.etag &&
    (await snapshotMatchesFile(check, authority.path))
  ) {
    summaryByName.get("ihep").supersession = "repaired-to-r2";
    return authorityManifest;
  }

  if (check.exists) {
    const racedManifest = await readJson(
      check.path,
      "IHEP latest.json after supersession repair race",
    );
    const racedVersion = assertManifestShape(
      racedManifest,
      "IHEP latest.json after supersession repair race",
    );
    if (compareSemver(racedVersion, authorityVersion) >= 0) {
      summaryByName.get("ihep").supersession = "preserved-racing-successor";
      return racedManifest;
    }
  }
  throw new ConditionalWriteError(
    "ihep: follower changed to an older value during supersession repair",
  );
}

export async function promoteCandidateTransaction({
  backends,
  candidateManifest,
  candidatePath,
  override,
  summary,
  workDir,
  hooks = {},
  promotionToken = randomUUID(),
}) {
  const { r2, ihep } = promotionBackends(backends);
  const summaryByName = new Map(summary.backends.map((entry) => [entry.name, entry]));
  const states = [];
  for (const backend of [r2, ihep]) {
    try {
      states.push(await readCurrentState(backend, workDir, candidateManifest));
    } catch (error) {
      const backendSummary = summaryByName.get(backend.name);
      if (backendSummary) backendSummary.error = safeSummaryError(error);
      throw error;
    }
  }
  await hooks.afterSnapshots?.(states);

  for (const state of states) {
    const backendSummary = summaryByName.get(state.backend.name);
    backendSummary.currentVersion = state.currentVersion;
    backendSummary.decision = state.decision;
  }

  const mismatch = states.find((state) => state.decision === "blocked-same-version-mismatch");
  if (mismatch) {
    throw new Error(
      `${mismatch.backend.name}: current latest has the candidate version but different channel/notes/artifact/signature/sha256 identity`,
    );
  }
  const downgradeStates = states.filter((state) => state.decision === "blocked-downgrade");
  if (downgradeStates.length > 0 && !override.requested) {
    throw new DowngradeBlockedError(
      `candidate ${candidateManifest.version} is older than ${downgradeStates
        .map((state) => `${state.backend.name}=${state.currentVersion}`)
        .join(", ")}`,
    );
  }
  if (downgradeStates.length > 0) {
    override.used = true;
    summary.override.used = true;
    for (const state of downgradeStates) {
      state.decision = "promote-downgrade-override";
      summaryByName.get(state.backend.name).decision = state.decision;
    }
  }

  const r2State = states.find((state) => state.backend === r2);
  const ihepState = states.find((state) => state.backend === ihep);
  let followerNeedsWrite = ihepState.decision !== "idempotent";
  const authorityNeedsWrite = r2State.decision !== "idempotent";
  if (!authorityNeedsWrite && !followerNeedsWrite) {
    for (const state of states) {
      const backendSummary = summaryByName.get(state.backend.name);
      try {
        const finalSnapshot = await state.backend.snapshot(
          LATEST_KEY,
          join(workDir, `${state.backend.name}-idempotent-final.json`),
        );
        if (!finalSnapshot.exists || finalSnapshot.etag !== state.previous.etag) {
          throw new ConditionalWriteError(
            `${state.backend.name}: latest.json changed after the idempotent snapshot`,
          );
        }
        const finalManifest = await readJson(
          finalSnapshot.path,
          `${state.backend.name} idempotent final latest.json`,
        );
        if (
          manifestReleaseIdentity(finalManifest) !==
          manifestReleaseIdentity(candidateManifest)
        ) {
          throw new ConditionalWriteError(
            `${state.backend.name}: latest.json no longer exposes the idempotent candidate`,
          );
        }
        backendSummary.finalVersion = finalManifest.version;
      } catch (error) {
        backendSummary.error = safeSummaryError(error);
        throw error;
      }
    }
    return { outcome: "idempotent", states };
  }

  if (!authorityNeedsWrite && followerNeedsWrite) {
    r2State.decision = override.used
      ? "claim-follower-downgrade-override"
      : "claim-follower-reconciliation";
    summaryByName.get("r2").decision = r2State.decision;
  }

  let activeState = r2State;
  let r2Commit = null;
  let followerWriteAttempted = false;
  let followerCommit = null;
  let supersededByAuthority = false;
  let supersessionFailures = [];
  let followerConflictAfterAuthority = false;
  try {
    // R2 is the only linearization authority. The follower snapshot must still
    // be current before this workflow attempts the authoritative CAS.
    await assertSnapshotUnchanged(ihepState);
    await hooks.beforeWrite?.(r2State);
    try {
      r2Commit = await r2.putLatestConditional(
        candidatePath,
        r2State.previous.etag,
        promotionToken,
      );
      summaryByName.get("r2").promotion = "written-authoritative-cas";
    } catch (error) {
      if (error instanceof ConditionalWriteError) throw error;
      const ambiguous = await withRollbackIo(() =>
        observeAmbiguousWrite(
          r2State,
          candidatePath,
          workDir,
          promotionToken,
        ),
      );
      if (!ambiguous) {
        summaryByName.get("r2").promotion = "write-outcome-unresolved";
        summaryByName.get("r2").rollback = "uncertain";
        throw new WriteOutcomeUncertainError(
          `r2: write failed and ownership could not be resolved after ${errorText(error)}`,
        );
      }
      r2Commit = ambiguous;
      summaryByName.get("r2").promotion = "write-outcome-ambiguous";
      throw error;
    }

    await ownedCandidateSnapshot({
      state: r2State,
      candidatePath,
      expectedEtag: r2Commit.etag,
      promotionToken,
      workDir,
      label: "authoritative-check",
    });
    summaryByName.get("r2").promotion = "verified-authoritative-cas";
    await hooks.afterWrite?.(r2State);

    if (followerNeedsWrite) {
      await ownedCandidateSnapshot({
        state: r2State,
        candidatePath,
        expectedEtag: r2Commit.etag,
        promotionToken,
        workDir,
        label: "pre-follower-ownership",
      });
      activeState = ihepState;
      let refreshedFollower;
      try {
        refreshedFollower = await refreshFollowerAfterAuthorityCas(
          ihepState,
          candidateManifest,
          workDir,
        );
      } catch (error) {
        followerConflictAfterAuthority = true;
        summaryByName.get("r2").rollback = "preserved-authoritative-cas";
        summaryByName.get("ihep").promotion = "conflict-after-authority-cas";
        summaryByName.get("ihep").rollback = "preserved-unowned-follower";
        if (error instanceof ConditionalWriteError) throw error;
        throw new ConditionalWriteError(
          `ihep: cannot prove exact candidate identity after the R2 CAS: ${errorText(error)}`,
        );
      }
      if (!refreshedFollower.needsWrite) {
        followerNeedsWrite = false;
        summaryByName.get("ihep").promotion = "already-current-after-authority-cas";
      }
    }

    if (followerNeedsWrite) {
      activeState = ihepState;
      await hooks.beforeWrite?.(ihepState);
      followerWriteAttempted = true;
      try {
        followerCommit = await ihep.putLatestUnconditional(candidatePath, promotionToken);
        summaryByName.get("ihep").promotion = "written-unconditional-follower";
      } catch (error) {
        const observed = await withRollbackIo(() =>
          observeAmbiguousWrite(
            ihepState,
            candidatePath,
            workDir,
            promotionToken,
          ),
        );
        if (!observed) {
          summaryByName.get("ihep").promotion = "write-outcome-unresolved";
          throw error;
        }
        followerCommit = observed;
        summaryByName.get("ihep").promotion = "write-outcome-confirmed";
      }
      await ownedCandidateSnapshot({
        state: ihepState,
        candidatePath,
        expectedEtag: followerCommit.etag,
        promotionToken,
        workDir,
        label: "follower-check",
      });
      summaryByName.get("ihep").promotion = "verified-unconditional-follower";
      await hooks.afterWrite?.(ihepState);
    } else if (summaryByName.get("ihep").promotion === "not-started") {
      summaryByName.get("ihep").promotion = "already-current";
    }

    activeState = r2State;
    try {
      await ownedCandidateSnapshot({
        state: r2State,
        candidatePath,
        expectedEtag: r2Commit.etag,
        promotionToken,
        workDir,
        label: "final-authority",
      });
    } catch (ownershipError) {
      // The protected release workflow is serialized, but keep a fencing repair
      // for accidental/manual overlap. An older writer that was superseded
      // between its pre-IHEP check and its unconditional IHEP write repairs only
      // an object it still owns, using the newer stable R2 snapshot.
      supersededByAuthority = true;
      summaryByName.get("r2").rollback = "skipped-superseded";
      try {
        await withRollbackIo(() =>
          reconcileFollowerToCurrentAuthority({
            r2State,
            ihepState,
            candidateManifest,
            candidatePath,
            promotionToken,
            summaryByName,
            workDir,
          }),
        );
      } catch (repairError) {
        supersessionFailures = [`ihep: ${errorText(repairError)}`];
      }
      throw ownershipError;
    }
    summaryByName.get("r2").finalVersion = candidateManifest.version;

    activeState = ihepState;
    let finalFollower;
    let finalFollowerCoverage;
    try {
      finalFollower = await ihep.snapshot(
        LATEST_KEY,
        join(workDir, "ihep-final-latest.json"),
      );
      finalFollowerCoverage = await classifyFollowerCoverage(
        finalFollower,
        candidateManifest,
        "IHEP final",
      );
    } catch (error) {
      followerConflictAfterAuthority = true;
      summaryByName.get("r2").rollback = "preserved-authoritative-cas";
      summaryByName.get("ihep").rollback = "preserved-unowned-follower";
      throw new ConditionalWriteError(
        `ihep: cannot prove exact final candidate identity: ${errorText(error)}`,
      );
    }
    if (finalFollowerCoverage.coverage !== "candidate") {
      followerConflictAfterAuthority = true;
      summaryByName.get("r2").rollback = "preserved-authoritative-cas";
      summaryByName.get("ihep").rollback = "preserved-unowned-follower";
      throw new ConditionalWriteError(
        `ihep: final latest.json is ${finalFollowerCoverage.version}; exact candidate identity is required`,
      );
    }
    if (
      followerCommit &&
      (finalFollower.etag !== followerCommit.etag ||
        finalFollower.metadata?.["cam-promotion-token"] !== promotionToken)
    ) {
      summaryByName.get("ihep").supersession =
        "already-follows-r2";
    }
    summaryByName.get("ihep").finalVersion = finalFollowerCoverage.manifest.version;
    return { outcome: override.used ? "downgrade-override-promoted" : "promoted", states };
  } catch (error) {
    if (activeState) {
      summaryByName.get(activeState.backend.name).error = safeSummaryError(error);
    }
    let failures = [];
    let rolledBack = false;
    if (followerConflictAfterAuthority) {
      summary.authorityPreserved = true;
    } else if (supersededByAuthority) {
      failures = supersessionFailures;
    } else if (r2Commit) {
      ({ failures, rolledBack } = await rollbackAuthoritativePromotion({
        r2State,
        ihepState,
        r2Commit,
        followerWriteAttempted,
        candidatePath,
        promotionToken,
        summaryByName,
        workDir,
      }));
    } else if (error instanceof WriteOutcomeUncertainError) {
      failures = ["r2: write ownership unresolved"];
    }
    summary.rollback = {
      attempted:
        Boolean(r2Commit) && !supersededByAuthority && !followerConflictAfterAuthority,
      complete: failures.length === 0,
      failures,
    };
    if (failures.length > 0) {
      throw new Error(
        `${errorText(error)}; rollback incomplete: ${failures.join("; ")}`,
      );
    }
    if (rolledBack) summary.outcome = "rolled-back";
    throw error;
  }
}

async function assertBackendLatestIsStableCandidate({
  backend,
  candidateManifest,
  destination,
}) {
  const snapshot = await backend.snapshot(LATEST_KEY, destination);
  if (!snapshot.exists) {
    throw new Error(`${backend.name}: latest.json disappeared before root identity promotion`);
  }
  const latest = await readJson(
    snapshot.path,
    `${backend.name} latest.json before root identity promotion`,
  );
  if (
    latest.channel !== "stable" ||
    latest.version !== candidateManifest.version ||
    manifestReleaseIdentity(latest) !== manifestReleaseIdentity(candidateManifest)
  ) {
    throw new ConditionalWriteError(
      `${backend.name}: latest.json no longer exposes the stable candidate before root identity promotion`,
    );
  }
  return snapshot;
}

async function assertRootPointerSnapshot({
  backend,
  destination,
  expectedPath,
  key,
  promotionToken,
}) {
  const snapshot = await backend.snapshot(key, destination);
  if (!snapshot.exists || !(await snapshotMatchesFile(snapshot, expectedPath))) {
    throw new Error(`${backend.name}: root identity pointer readback mismatch for ${key}`);
  }
  if (snapshot.metadata?.["cam-promotion-token"] !== promotionToken) {
    throw new Error(`${backend.name}: root identity pointer lost promotion metadata for ${key}`);
  }
  return snapshot;
}

async function verifyBackendRootIdentityPointer({
  backend,
  bundle,
  candidateManifest,
  promotionToken,
  publicKey,
  workDir,
}) {
  const backendDir = join(workDir, backend.name);
  await mkdir(backendDir, { recursive: true });
  await assertBackendLatestIsStableCandidate({
    backend,
    candidateManifest,
    destination: join(backendDir, "latest.json"),
  });
  const identitySnapshot = await assertRootPointerSnapshot({
    backend,
    destination: join(backendDir, ROOT_IDENTITY_KEY),
    expectedPath: bundle.identityPath,
    key: ROOT_IDENTITY_KEY,
    promotionToken,
  });
  const signatureSnapshot = await assertRootPointerSnapshot({
    backend,
    destination: join(backendDir, ROOT_IDENTITY_SIGNATURE_KEY),
    expectedPath: bundle.signaturePath,
    key: ROOT_IDENTITY_SIGNATURE_KEY,
    promotionToken,
  });
  const signature = (await readFile(signatureSnapshot.path, "utf8")).trim();
  await verifyTauriUpdaterSignature(identitySnapshot.path, signature, publicKey);
  const remoteIdentity = await readJson(
    identitySnapshot.path,
    `${backend.name} root release identity`,
  );
  if (
    remoteIdentity.schema !== RELEASE_IDENTITY_SCHEMA_VERSION ||
    remoteIdentity.channel !== "stable" ||
    remoteIdentity.version !== candidateManifest.version
  ) {
    throw new Error(
      `${backend.name}: root release identity has an unexpected schema/channel/version`,
    );
  }
  return {
    channel: remoteIdentity.channel,
    identityEtag: identitySnapshot.etag,
    identitySha256: bundle.identitySha256,
    signatureEtag: signatureSnapshot.etag,
    signatureSha256: bundle.signatureSha256,
    verified: true,
    version: remoteIdentity.version,
  };
}

export async function promoteStableIdentityPointers({
  backends,
  bundle,
  candidateKey,
  candidateManifest,
  fetchImpl = fetch,
  mirrorBase,
  promotionToken,
  publicKey,
  summary,
  workDir,
}) {
  const summaryByName = new Map(summary.backends.map((entry) => [entry.name, entry]));
  for (const backend of backends) {
    const backendSummary = summaryByName.get(backend.name);
    if (!backendSummary) throw new Error(`missing summary row for ${backend.name}`);
    backendSummary.rootIdentity = {
      directVerification: "not-started",
      jsonWrite: "not-started",
      publicVerification: "not-started",
      signatureWrite: "not-started",
    };
    if (typeof backend.putRootIdentityPointer !== "function") {
      throw new Error(`${backend.name}: backend cannot write root identity pointers`);
    }
  }

  const directRoot = join(workDir, "direct");
  for (const backend of backends) {
    await assertBackendLatestIsStableCandidate({
      backend,
      candidateManifest,
      destination: join(directRoot, backend.name, "pre-signature-latest.json"),
    });
  }

  // Write both signatures first. Until the matching JSON lands, clients either
  // still verify the old pair or reject the mismatched pair and fall back to
  // GitHub. Never roll these mutable pointers back: a retry safely rewrites the
  // same signed pair after latest.json has already committed monotonically.
  for (const backend of backends) {
    const pointerSummary = summaryByName.get(backend.name).rootIdentity;
    try {
      await backend.putRootIdentityPointer(
        bundle.signaturePath,
        ROOT_IDENTITY_SIGNATURE_KEY,
        promotionToken,
      );
      await assertRootPointerSnapshot({
        backend,
        destination: join(
          directRoot,
          backend.name,
          `signature-phase-${ROOT_IDENTITY_SIGNATURE_KEY}`,
        ),
        expectedPath: bundle.signaturePath,
        key: ROOT_IDENTITY_SIGNATURE_KEY,
        promotionToken,
      });
      pointerSummary.signatureWrite = "written-and-verified";
    } catch (error) {
      pointerSummary.signatureWrite = "failed";
      pointerSummary.error = safeSummaryError(error);
      throw error;
    }
  }

  for (const backend of backends) {
    const pointerSummary = summaryByName.get(backend.name).rootIdentity;
    try {
      await assertBackendLatestIsStableCandidate({
        backend,
        candidateManifest,
        destination: join(directRoot, backend.name, "pre-json-latest.json"),
      });
      await backend.putRootIdentityPointer(
        bundle.identityPath,
        ROOT_IDENTITY_KEY,
        promotionToken,
      );
      pointerSummary.jsonWrite = "written";
      pointerSummary.directVerification = "running";
      pointerSummary.direct = await verifyBackendRootIdentityPointer({
        backend,
        bundle,
        candidateManifest,
        promotionToken,
        publicKey,
        workDir: directRoot,
      });
      pointerSummary.directVerification = "passed";
      pointerSummary.jsonWrite = "written-and-verified";
    } catch (error) {
      if (pointerSummary.jsonWrite === "not-started") {
        pointerSummary.jsonWrite = "failed";
      }
      pointerSummary.directVerification = "failed";
      pointerSummary.error = safeSummaryError(error);
      throw error;
    }
  }

  summary.rootIdentity.publicRouteVerification = "running";
  try {
    summary.rootIdentity.publicRoute = await verifyPublicRootIdentityPointers({
      bundle,
      candidateKey,
      fetchImpl,
      mirrorBase,
      publicKey,
      workDir: join(workDir, "public"),
      ihepRedirect: ihepRedirectFromBackends(backends),
    });
    summary.rootIdentity.publicRouteVerification = "passed";
    for (const backend of backends) {
      const pointerSummary = summaryByName.get(backend.name).rootIdentity;
      pointerSummary.publicVerification = "passed";
      pointerSummary.public =
        summary.rootIdentity.publicRoute.backends[backend.name];
    }
  } catch (error) {
    summary.rootIdentity.publicRouteVerification = "failed";
    summary.rootIdentity.publicRouteError = safeSummaryError(error);
    throw error;
  }
  return summary.rootIdentity;
}

function mirrorVerificationRows(backends, includeTransaction = false) {
  return backends.map((backend) => ({
    name: backend.name,
    candidateVerification: "not-started",
    ...(includeTransaction
      ? {
          currentVersion: null,
          decision: null,
          promotion: "not-started",
          rollback: "not-needed",
        }
      : {}),
  }));
}

function ihepRedirectFromBackends(backends) {
  const ihep = backends.find((backend) => backend.name === "ihep");
  if (!ihep) throw new Error("public mirror verification requires the IHEP backend");
  return {
    endpoint: ihep.endpoint,
    bucket: ihep.bucket,
    prefix: ihep.prefix || "",
  };
}

async function verifyMirrorCandidates({
  backends,
  candidateKey,
  candidateManifest,
  candidatePath,
  distDir,
  mirrorBase,
  publicKey,
  summary,
  tempRoot,
  fetchImpl = fetch,
}) {
  const [expectedArtifacts, candidateSha256] = await Promise.all([
    expectedArtifactsFromManifest(candidateManifest, distDir, mirrorBase),
    sha256File(candidatePath),
  ]);
  const promotionToken = promotionTokenForCandidate(candidateKey);

  // Both storage origins must finish a complete candidate + artifact readback
  // before any mutable latest.json write. Verification is sequential to cap
  // runner disk use.
  for (const backend of backends) {
    const backendSummary = summary.backends.find((entry) => entry.name === backend.name);
    backendSummary.candidateVerification = "running";
    try {
      backendSummary.candidate = await verifyBackendCandidate({
        backend,
        candidateKey,
        candidatePath,
        candidateManifest,
        distDir,
        mirrorBase,
        publicKey,
        workDir: join(tempRoot, "backend"),
        expectedArtifacts,
        candidateSha256,
        promotionToken,
      });
      backendSummary.candidateVerification = "passed";
    } catch (error) {
      backendSummary.candidateVerification = "failed";
      backendSummary.error = safeSummaryError(error);
      throw error;
    }
  }

  // Direct S3 readback does not prove that the Worker route used by clients is
  // healthy or bound to the expected bucket. Fetch the run-specific candidate
  // and every updater payload through their real public HTTPS URLs as a separate
  // fail-closed gate.
  summary.publicRouteVerification = "running";
  try {
    summary.publicRoute = await verifyPublicMirrorRoute({
      candidateKey,
      candidateManifest,
      candidatePath,
      expectedArtifacts,
      mirrorBase,
      publicKey,
      workDir: join(tempRoot, "public"),
      fetchImpl,
      ihepRedirect: ihepRedirectFromBackends(backends),
    });
    summary.publicRouteVerification = "passed";
  } catch (error) {
    summary.publicRouteVerification = "failed";
    summary.publicRouteError = safeSummaryError(error);
    throw error;
  }

  return { candidateSha256, expectedArtifacts, promotionToken };
}

export async function verifyMirrors({
  backends,
  candidateKey,
  candidateManifest,
  candidatePath,
  distDir,
  mirrorBase,
  override,
  publicKey,
  summaryPath,
  tempRoot,
  fetchImpl,
}) {
  const version = assertManifestShape(candidateManifest, "candidate latest.json");
  assertRequiredUpdaterPlatforms(candidateManifest, "candidate latest.json");
  const summary = initialSummary("verify", version, candidateKey, override);
  summary.backends = mirrorVerificationRows(backends);
  summary.publicRouteVerification = "not-started";
  try {
    await verifyMirrorCandidates({
      backends,
      candidateKey,
      candidateManifest,
      candidatePath,
      distDir,
      mirrorBase,
      publicKey,
      summary,
      tempRoot,
      fetchImpl,
    });
    await finishSummary(summaryPath, summary, "verified");
    return summary;
  } catch (error) {
    await finishSummary(summaryPath, summary, "failed", error);
    throw error;
  }
}

export async function promoteMirrors({
  backends,
  candidateKey,
  candidateManifest,
  candidatePath,
  distDir,
  mirrorBase,
  override,
  publicKey,
  summaryPath,
  tempRoot,
  hooks,
  fetchImpl,
}) {
  const version = assertManifestShape(candidateManifest, "candidate latest.json");
  assertRequiredUpdaterPlatforms(candidateManifest, "candidate latest.json");
  const summary = initialSummary("promote", version, candidateKey, override);
  summary.backends = mirrorVerificationRows(backends, true);
  summary.publicRouteVerification = "not-started";
  summary.rootIdentity = {
    channel: "stable",
    localVerification: "not-started",
    publicRouteVerification: "not-started",
    version,
  };
  try {
    summary.rootIdentity.localVerification = "running";
    const identityBundle = await loadStableReleaseIdentity({
      candidateManifest,
      distDir,
      publicKey,
    });
    summary.rootIdentity.localVerification = "passed";
    summary.rootIdentity.identitySha256 = identityBundle.identitySha256;
    summary.rootIdentity.signatureSha256 = identityBundle.signatureSha256;

    const { promotionToken } = await verifyMirrorCandidates({
      backends,
      candidateKey,
      candidateManifest,
      candidatePath,
      distDir,
      mirrorBase,
      publicKey,
      summary,
      tempRoot: join(tempRoot, "verify"),
      fetchImpl,
    });

    const transaction = await promoteCandidateTransaction({
      backends,
      candidateManifest,
      candidatePath,
      override,
      summary,
      workDir: join(tempRoot, "transaction"),
      hooks,
      promotionToken,
    });
    summary.rootIdentity.latestTransactionOutcome = transaction.outcome;
    await promoteStableIdentityPointers({
      backends,
      bundle: identityBundle,
      candidateKey,
      candidateManifest,
      fetchImpl,
      mirrorBase,
      promotionToken,
      publicKey,
      summary,
      workDir: join(tempRoot, "root-identity"),
    });
    await finishSummary(summaryPath, summary, transaction.outcome);
    return summary;
  } catch (error) {
    if (summary.rootIdentity.localVerification === "running") {
      summary.rootIdentity.localVerification = "failed";
      summary.rootIdentity.error = safeSummaryError(error);
    }
    const outcome =
      error instanceof DowngradeBlockedError
        ? "blocked-downgrade"
        : summary.outcome === "rolled-back"
          ? "rolled-back"
          : "failed";
    await finishSummary(summaryPath, summary, outcome, error);
    throw error;
  }
}

async function createAwsConfig(tempRoot) {
  const path = join(tempRoot, "aws-config");
  await writeFile(path, "[default]\nregion = auto\ns3 =\n    addressing_style = path\n");
  return path;
}

function installSignalHandlers() {
  const handler = (signal) => {
    if (interruptedBy) return;
    interruptedBy = signal;
    for (const child of activeChildren) child.kill("SIGTERM");
  };
  process.on("SIGINT", handler);
  process.on("SIGTERM", handler);
  return () => {
    process.off("SIGINT", handler);
    process.off("SIGTERM", handler);
  };
}

async function main() {
  const distArg = process.argv[2];
  if (!distArg) throw new Error("usage: sync-mirror.sh <dist-dir>");
  const distDir = resolve(distArg);
  const phase = process.env.MIRROR_PHASE || "all";
  if (!["all", "stage", "verify", "promote"].includes(phase)) {
    throw new Error(
      `unsupported MIRROR_PHASE=${phase} (expected all, stage, verify, or promote)`,
    );
  }
  const override = downgradeOverrideFromEnv(process.env);
  if (override.requested) {
    console.log(
      `::warning::[mirror] emergency downgrade override requested by ${safeSummaryError(
        override.actor,
      )}: ${safeSummaryError(override.reason)} (${override.runUrl})`,
    );
  }
  const candidateId = candidateIdFromEnv(process.env);
  const mirror = await createMirrorManifest(
    distDir,
    process.env.MIRROR_BASE_URL || DEFAULT_MIRROR_BASE,
  );
  const candidateKey = candidateKeyFor(mirror.version, candidateId);
  const tempRoot = await mkdtemp(join(tmpdir(), "cam-mirror-release-"));
  const removeSignalHandlers = installSignalHandlers();
  try {
    const configPath = await createAwsConfig(tempRoot);
    const backends = backendConfigsFromEnv(process.env, configPath);
    if (phase === "all" || phase === "stage") {
      const stageSummaryPath = resolve(
        process.env.MIRROR_STAGE_SUMMARY_PATH || "mirror-stage-summary.json",
      );
      console.log(`::group::[mirror] stage ${mirror.version} candidate=${candidateKey}`);
      await stageMirrors({
        backends,
        candidateKey,
        candidatePath: mirror.outputPath,
        distDir,
        summaryPath: stageSummaryPath,
        version: mirror.version,
        override,
      });
      console.log("::endgroup::");
    }
    if (phase === "verify") {
      const verificationSummaryPath = resolve(
        process.env.MIRROR_VERIFICATION_SUMMARY_PATH ||
          "mirror-verification-summary.json",
      );
      const publicKey = process.env.MIRROR_UPDATER_PUBLIC_KEY || (await updaterPublicKeyFromRepo());
      console.log(`::group::[mirror] verify ${mirror.version}`);
      await verifyMirrors({
        backends,
        candidateKey,
        candidateManifest: mirror.manifest,
        candidatePath: mirror.outputPath,
        distDir,
        mirrorBase: mirror.mirrorBase,
        override,
        publicKey,
        summaryPath: verificationSummaryPath,
        tempRoot,
      });
      console.log("::endgroup::");
    }
    if (phase === "all" || phase === "promote") {
      const promotionSummaryPath = resolve(
        process.env.MIRROR_PROMOTION_SUMMARY_PATH || "mirror-promotion-summary.json",
      );
      const publicKey = process.env.MIRROR_UPDATER_PUBLIC_KEY || (await updaterPublicKeyFromRepo());
      console.log(`::group::[mirror] verify and promote ${mirror.version}`);
      await promoteMirrors({
        backends,
        candidateKey,
        candidateManifest: mirror.manifest,
        candidatePath: mirror.outputPath,
        distDir,
        mirrorBase: mirror.mirrorBase,
        override,
        publicKey,
        summaryPath: promotionSummaryPath,
        tempRoot,
      });
      console.log("::endgroup::");
    }
  } finally {
    removeSignalHandlers();
    await rm(tempRoot, { force: true, recursive: true });
  }
  if (interruptedBy) throw new Error(`release interrupted by ${interruptedBy}`);
}

const isCli = process.argv[1] && resolve(process.argv[1]) === fileURLToPath(import.meta.url);
if (isCli) {
  main().catch((error) => {
    console.error(`::error::[mirror] ${safeSummaryError(error)}`);
    process.exitCode = interruptedBy === "SIGINT" ? 130 : interruptedBy === "SIGTERM" ? 143 : 1;
  });
}
