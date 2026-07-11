import {
  createHash,
  generateKeyPairSync,
  sign as cryptoSign,
} from "node:crypto";
import { execFile } from "node:child_process";
import {
  mkdir,
  mkdtemp,
  readFile,
  rm,
  writeFile,
} from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { promisify } from "node:util";

import { afterEach, describe, expect, it } from "vitest";

import {
  AwsObjectStore,
  ConditionalWriteError,
  DowngradeBlockedError,
  assertCandidateMatchesRelease,
  candidateKeyFor,
  compareSemver,
  createMirrorManifest,
  downgradeOverrideFromEnv,
  promoteCandidateTransaction,
  promoteMirrors,
  stageMirrors,
  verifyBackendCandidate,
  verifyLocalUpdaterArtifacts,
  verifyLocalReleaseIdentity,
  verifyMirrors,
  verifyPublicMirrorRoute,
  verifyTauriUpdaterSignature,
} from "./mirror-release.mjs";
import {
  OPTIONAL_RELEASE_METADATA_ASSET_NAMES,
  REQUIRED_RELEASE_METADATA_ASSET_NAMES,
  inspectReleaseForReuse,
  requiredReleaseAssetNames,
} from "./check-release-reuse.mjs";

const roots = [];
const execFileAsync = promisify(execFile);
const TEST_IHEP_REDIRECT = Object.freeze({
  endpoint: "https://ihep.example/root",
  bucket: "mirror-bucket",
  prefix: "manager",
});

async function tempRoot(name) {
  const root = await mkdtemp(join(tmpdir(), `cam-${name}-`));
  roots.push(root);
  return root;
}

afterEach(async () => {
  await Promise.all(roots.splice(0).map((root) => rm(root, { force: true, recursive: true })));
});

function hash(body) {
  return createHash("sha256").update(body).digest("hex");
}

function manifest(version, signature = "test-signature") {
  return {
    version,
    notes: `release ${version}`,
    pub_date: "2026-01-01T00:00:00.000Z",
    platforms: {
      "windows-x86_64": {
        sha256: "a".repeat(64),
        signature,
        url: `https://mirror.example/manager/${version}/manager-${version}.exe`,
      },
    },
  };
}

function completeManifest(version, signature = "test-signature") {
  return {
    version,
    channel: "stable",
    notes: `release ${version}`,
    pub_date: "2026-01-01T00:00:00.000Z",
    platforms: Object.fromEntries(
      [
        "darwin-aarch64",
        "darwin-x86_64",
        "windows-x86_64",
        "windows-aarch64",
      ].map((platform) => [
        platform,
        {
          sha256: "a".repeat(64),
          signature,
          url: `https://mirror.example/manager/${version}/manager-${platform}-${version}.bin`,
        },
      ]),
    ),
  };
}

function completeRelease(releaseTag, overrides = {}) {
  const digest = `sha256:${"a".repeat(64)}`;
  return {
    draft: false,
    immutable: true,
    assets: [
      ...requiredReleaseAssetNames(releaseTag),
      ...REQUIRED_RELEASE_METADATA_ASSET_NAMES,
    ].map((name) => ({
      digest,
      name,
      size: 1,
    })),
    ...overrides,
  };
}

async function writeManifest(root, name, value) {
  const path = join(root, name);
  await mkdir(root, { recursive: true });
  await writeFile(path, `${JSON.stringify(value, null, 2)}\n`);
  return path;
}

function summaryFor(backends, override = overrideOff()) {
  return {
    backends: backends.map((backend) => ({
      name: backend.name,
      candidateVerification: "passed",
      promotion: "not-started",
      rollback: "not-needed",
    })),
    outcome: "running",
    override: { ...override },
  };
}

function overrideOff() {
  return {
    actor: "",
    eventName: "push",
    reason: "",
    requested: false,
    runUrl: "",
    used: false,
  };
}

class MemoryBackend {
  constructor(name, objects = {}) {
    this.name = name;
    this.endpoint = name === "ihep" ? TEST_IHEP_REDIRECT.endpoint : "https://r2.example";
    this.bucket = name === "ihep" ? TEST_IHEP_REDIRECT.bucket : "r2-bucket";
    this.prefix = name === "ihep" ? TEST_IHEP_REDIRECT.prefix : "";
    this.objects = new Map();
    this.counter = 0;
    this.latestPutAttempts = 0;
    this.conditionalLatestPutAttempts = 0;
    this.unconditionalLatestPutAttempts = 0;
    for (const [key, body] of Object.entries(objects)) this.set(key, body);
  }

  set(key, body, metadata = {}) {
    const bytes = Buffer.isBuffer(body) ? Buffer.from(body) : Buffer.from(body);
    this.counter += 1;
    this.objects.set(key, {
      body: bytes,
      etag: `"${hash(bytes)}-${this.counter}"`,
      metadata: { ...metadata },
    });
  }

  body(key) {
    return this.objects.get(key)?.body;
  }

  async head(key) {
    const object = this.objects.get(key);
    return object
      ? {
          etag: object.etag,
          metadata: { ...object.metadata },
          size: object.body.length,
        }
      : null;
  }

  async snapshot(key, destination) {
    const object = this.objects.get(key);
    if (!object) return { exists: false, etag: null, path: null, size: 0 };
    await mkdir(join(destination, ".."), { recursive: true });
    await writeFile(destination, object.body);
    return {
      exists: true,
      etag: object.etag,
      metadata: { ...object.metadata },
      path: destination,
      size: object.body.length,
    };
  }

  async putImmutable(localPath, key, _contentType, metadata = {}) {
    const existing = this.objects.get(key);
    if (existing) return { status: "existing", etag: existing.etag, size: existing.body.length };
    const body = await readFile(localPath);
    this.set(key, body, metadata);
    const stored = this.objects.get(key);
    return { status: "uploaded", etag: stored.etag, size: stored.body.length };
  }

  async putLatestConditional(localPath, expectedEtag, promotionToken = "") {
    this.latestPutAttempts += 1;
    this.conditionalLatestPutAttempts += 1;
    const current = this.objects.get("latest.json");
    const matches = expectedEtag ? current?.etag === expectedEtag : !current;
    if (!matches) throw new ConditionalWriteError(`${this.name}: stale latest ETag`);
    if (this.failLatestPut) throw new Error(`${this.name}: simulated write failure`);
    this.set(
      "latest.json",
      await readFile(localPath),
      promotionToken ? { "cam-promotion-token": promotionToken } : {},
    );
    const stored = this.objects.get("latest.json");
    return { etag: stored.etag, size: stored.body.length };
  }

  async putLatestUnconditional(localPath, promotionToken = "") {
    this.latestPutAttempts += 1;
    this.unconditionalLatestPutAttempts += 1;
    if (this.failLatestPut) throw new Error(`${this.name}: simulated write failure`);
    this.set(
      "latest.json",
      await readFile(localPath),
      promotionToken ? { "cam-promotion-token": promotionToken } : {},
    );
    const stored = this.objects.get("latest.json");
    return { etag: stored.etag, size: stored.body.length };
  }

  async putRootIdentityPointer(localPath, key, promotionToken = "") {
    this.onRootIdentityWrite?.(`${this.name}:${key}`);
    if (this.failRootIdentityKey === key) {
      throw new Error(`${this.name}: simulated root identity write failure for ${key}`);
    }
    this.set(
      key,
      await readFile(localPath),
      promotionToken ? { "cam-promotion-token": promotionToken } : {},
    );
    const stored = this.objects.get(key);
    return { etag: stored.etag, size: stored.body.length };
  }

  async deleteLatestConditional(expectedEtag) {
    const current = this.objects.get("latest.json");
    if (!current || current.etag !== expectedEtag) {
      throw new ConditionalWriteError(`${this.name}: stale delete ETag`);
    }
    this.objects.delete("latest.json");
  }
}

class ConditionIgnoringMemoryBackend extends MemoryBackend {
  async putLatestConditional(localPath, _expectedEtag, promotionToken = "") {
    // Model the real IHEP behavior: the request accepts If-Match but overwrites
    // regardless. Production promotion must therefore never call this method.
    this.latestPutAttempts += 1;
    this.conditionalLatestPutAttempts += 1;
    this.set(
      "latest.json",
      await readFile(localPath),
      promotionToken ? { "cam-promotion-token": promotionToken } : {},
    );
    const stored = this.objects.get("latest.json");
    return { etag: stored.etag, size: stored.body.length };
  }
}

function encodedUpdaterSignature(privateKey, keyId, artifact) {
  const digest = createHash("blake2b512").update(artifact).digest();
  const signature = cryptoSign(null, digest, privateKey);
  const trustedComment = "timestamp:1700000000\tfile:test-artifact";
  const globalSignature = cryptoSign(
    null,
    Buffer.concat([signature, Buffer.from(trustedComment)]),
    privateKey,
  );
  const primary = Buffer.concat([Buffer.from("ED"), keyId, signature]);
  const text = [
    "untrusted comment: signature from test key",
    primary.toString("base64"),
    `trusted comment: ${trustedComment}`,
    globalSignature.toString("base64"),
  ].join("\n");
  return Buffer.from(`${text}\n`).toString("base64");
}

function updaterFixture(artifact) {
  const { privateKey, publicKey } = generateKeyPairSync("ed25519");
  const publicDer = publicKey.export({ format: "der", type: "spki" });
  const rawPublicKey = publicDer.subarray(publicDer.length - 32);
  const keyId = Buffer.from("0102030405060708", "hex");
  const minisignPublic = Buffer.concat([Buffer.from("Ed"), keyId, rawPublicKey]);
  const publicText = [
    "untrusted comment: minisign public key: test",
    minisignPublic.toString("base64"),
  ].join("\n");
  return {
    publicKey: Buffer.from(`${publicText}\n`).toString("base64"),
    signature: encodedUpdaterSignature(privateKey, keyId, artifact),
    sign: (value) => encodedUpdaterSignature(privateKey, keyId, value),
  };
}

async function writeReleaseIdentity(
  dist,
  candidate,
  fixture,
  { channel = candidate.channel, mutate } = {},
) {
  const identity = {
    schema: 1,
    version: candidate.version,
    channel,
    notes_sha256: hash(Buffer.from(candidate.notes, "utf8")),
    platforms: {},
  };
  for (const [platform, entry] of Object.entries(candidate.platforms)) {
    const artifact = decodeURIComponent(
      new URL(entry.url).pathname.split("/").filter(Boolean).at(-1),
    );
    identity.platforms[platform] = {
      artifact,
      sha256: hash(await readFile(join(dist, artifact))),
    };
  }
  mutate?.(identity);
  const identityPath = await writeManifest(
    dist,
    "release-identity.json",
    identity,
  );
  const identityBytes = await readFile(identityPath);
  const signature = fixture.sign(identityBytes);
  const signaturePath = join(dist, "release-identity.json.sig");
  await writeFile(signaturePath, `${signature}\n`);
  return { identity, identityBytes, identityPath, signature, signaturePath };
}

function testIhepRedirect(workerUrl, overrides = {}) {
  const config = { ...TEST_IHEP_REDIRECT, ...overrides };
  const redirected = new URL(config.endpoint);
  const objectKey = decodeURIComponent(workerUrl.pathname.slice("/manager/".length));
  const prefix = config.prefix.replace(/^\/+|\/+$/g, "");
  redirected.pathname = [
    redirected.pathname.replace(/\/+$/g, ""),
    config.bucket,
    prefix,
    objectKey,
  ]
    .filter(Boolean)
    .join("/");
  if (!redirected.pathname.startsWith("/")) redirected.pathname = `/${redirected.pathname}`;
  redirected.searchParams.set("X-Amz-Algorithm", "AWS4-HMAC-SHA256");
  redirected.searchParams.set("X-Amz-Credential", "AKIATEST/20260711/auto/s3/aws4_request");
  redirected.searchParams.set("X-Amz-SignedHeaders", "host");
  redirected.searchParams.set("X-Amz-Signature", "a".repeat(64));
  return redirected.toString();
}

function publicRouteFetch(
  objects,
  requests = [],
  { ihepObjects = objects, ihepRedirectOverrides = {} } = {},
) {
  return async (value) => {
    const url = new URL(value);
    requests.push(url);
    if (url.hostname === "ihep.example") {
      const objectPrefix = "/root/mirror-bucket/manager/";
      const objectKey = url.pathname.startsWith(objectPrefix)
        ? decodeURIComponent(url.pathname.slice(objectPrefix.length))
        : "";
      const body = ihepObjects.get(`/manager/${objectKey}`);
      return body === undefined
        ? new Response("not found", { status: 404 })
        : new Response(body, { status: 200 });
    }
    const backend = url.searchParams.get("cam_backend");
    if (backend === "ihep") {
      return new Response(null, {
        status: 302,
        headers: {
          Location: testIhepRedirect(url, ihepRedirectOverrides),
          "X-Codex-Mirror-Backend": "ihep",
        },
      });
    }
    const body = objects.get(url.pathname);
    return body === undefined
      ? new Response("not found", { status: 404 })
      : new Response(body, {
          status: 200,
          headers: { "X-Codex-Mirror-Backend": "r2" },
        });
  };
}

function liveBackendPublicRouteFetch(r2, ihep, requests = []) {
  const keyFromUrl = (url) => {
    const pathname = url.pathname
      .replace(/^\/root\/mirror-bucket\/manager\//, "")
      .replace(/^\/manager\//, "");
    return pathname
      .split("/")
      .map((segment) => decodeURIComponent(segment))
      .join("/");
  };
  return async (value) => {
    const url = new URL(value);
    requests.push(url);
    if (url.hostname === "ihep.example") {
      const body = ihep.body(keyFromUrl(url));
      return body === undefined
        ? new Response("not found", { status: 404 })
        : new Response(body, { status: 200 });
    }
    const backend = url.searchParams.get("cam_backend");
    if (backend === "ihep") {
      return new Response(null, {
        status: 302,
        headers: {
          Location: testIhepRedirect(url),
          "X-Codex-Mirror-Backend": "ihep",
        },
      });
    }
    const body = r2.body(keyFromUrl(url));
    return body === undefined
      ? new Response("not found", { status: 404 })
      : new Response(body, {
          status: 200,
          headers: { "X-Codex-Mirror-Backend": "r2" },
        });
  };
}

async function prepareMirrorPromotionFixture(
  root,
  {
    channel = "stable",
    preexistingR2 = {},
    version = "1.2.3",
  } = {},
) {
  const dist = join(root, "dist");
  await mkdir(dist, { recursive: true });
  const artifact = Buffer.from(`stable updater payload for ${version}`);
  const fixture = updaterFixture(artifact);
  const candidate = completeManifest(version, fixture.signature);
  for (const entry of Object.values(candidate.platforms)) {
    entry.sha256 = hash(artifact);
  }
  candidate.channel = channel;
  for (const entry of Object.values(candidate.platforms)) {
    const name = new URL(entry.url).pathname.split("/").at(-1);
    await writeFile(join(dist, name), artifact);
    await writeFile(join(dist, `${name}.sig`), `${fixture.signature}\n`);
  }
  const releaseIdentity = await writeReleaseIdentity(dist, candidate, fixture, {
    channel,
  });
  const candidatePath = await writeManifest(dist, "latest.mirror.json", candidate);
  const candidateKey = candidateKeyFor(version, "root-identity-run");
  const current = Buffer.from(`${JSON.stringify(manifest("0.9.0"))}\n`);
  const r2 = new MemoryBackend("r2", {
    "latest.json": current,
    ...preexistingR2,
  });
  const ihep = new MemoryBackend("ihep", { "latest.json": current });
  await stageMirrors({
    backends: [r2, ihep],
    candidateKey,
    candidatePath,
    distDir: dist,
    summaryPath: join(root, "stage-summary.json"),
    version,
    override: overrideOff(),
  });
  return {
    artifact,
    candidate,
    candidateKey,
    candidatePath,
    dist,
    fixture,
    ihep,
    r2,
    releaseIdentity,
  };
}

describe("semantic release ordering", () => {
  it("orders stable and prerelease versions without lexical mistakes", () => {
    expect(compareSemver("1.10.0", "1.9.9")).toBe(1);
    expect(compareSemver("1.0.0-rc.2", "1.0.0-rc.10")).toBe(-1);
    expect(compareSemver("1.0.0", "1.0.0-rc.10")).toBe(1);
    expect(compareSemver("v2.0.0+build.1", "2.0.0+build.2")).toBe(0);
  });
});

describe("release candidate binding", () => {
  it("binds the candidate version and complete platform identity to the release tag", () => {
    const derived = completeManifest("1.2.3", "artifact-derived-signature");
    const candidate = structuredClone(derived);

    expect(assertCandidateMatchesRelease(candidate, derived, "v1.2.3")).toEqual({
      platformCount: 4,
      version: "1.2.3",
    });

    const wrongVersion = structuredClone(candidate);
    wrongVersion.version = "9.9.9";
    expect(() => assertCandidateMatchesRelease(wrongVersion, derived, "v1.2.3")).toThrow(
      "does not match release tag",
    );

    const missingPlatform = structuredClone(candidate);
    delete missingPlatform.platforms["windows-aarch64"];
    expect(() => assertCandidateMatchesRelease(missingPlatform, derived, "v1.2.3")).toThrow(
      "missing required updater platforms: windows-aarch64",
    );

    const changedSignature = structuredClone(candidate);
    changedSignature.platforms["windows-x86_64"].signature = "different-signature";
    expect(() => assertCandidateMatchesRelease(changedSignature, derived, "v1.2.3")).toThrow(
      "channel/notes/platforms/signatures/sha256 do not match",
    );

    const missingSha256 = structuredClone(candidate);
    delete missingSha256.platforms["windows-x86_64"].sha256;
    expect(() => assertCandidateMatchesRelease(missingSha256, derived, "v1.2.3")).toThrow(
      "sha256 must be a lowercase SHA-256 digest",
    );

    const invalidSha256 = structuredClone(candidate);
    invalidSha256.platforms["windows-x86_64"].sha256 = "A".repeat(64);
    expect(() => assertCandidateMatchesRelease(invalidSha256, derived, "v1.2.3")).toThrow(
      "sha256 must be a lowercase SHA-256 digest",
    );

    const changedSha256 = structuredClone(candidate);
    changedSha256.platforms["windows-x86_64"].sha256 = "b".repeat(64);
    expect(() => assertCandidateMatchesRelease(changedSha256, derived, "v1.2.3")).toThrow(
      "channel/notes/platforms/signatures/sha256 do not match",
    );
  });
});

describe("existing GitHub Release reuse", () => {
  it("requires GitHub immutability and canonical SHA-256 asset digests", () => {
    const releaseTag = "v1.2.3";
    const valid = inspectReleaseForReuse(completeRelease(releaseTag), releaseTag);
    expect(valid.reusable).toBe(true);
    expect(Object.keys(valid.digests)).toHaveLength(
      requiredReleaseAssetNames(releaseTag).length,
    );

    expect(() =>
      inspectReleaseForReuse(completeRelease(releaseTag, { immutable: false }), releaseTag),
    ).toThrow("is mutable");

    const mutableAndIncomplete = completeRelease(releaseTag, { immutable: false });
    mutableAndIncomplete.assets.pop();
    expect(() => inspectReleaseForReuse(mutableAndIncomplete, releaseTag)).toThrow("is mutable");

    const immutableAndIncomplete = completeRelease(releaseTag);
    immutableAndIncomplete.assets.pop();
    expect(() => inspectReleaseForReuse(immutableAndIncomplete, releaseTag)).toThrow(
      "is missing required assets and cannot be repaired",
    );

    const repairableDraft = completeRelease(releaseTag, { draft: true, immutable: false });
    repairableDraft.assets.pop();
    expect(inspectReleaseForReuse(repairableDraft, releaseTag)).toMatchObject({
      reason: "draft",
      reusable: false,
    });

    const missingDigest = completeRelease(releaseTag);
    delete missingDigest.assets[0].digest;
    expect(() => inspectReleaseForReuse(missingDigest, releaseTag)).toThrow(
      "has no canonical SHA-256 digest",
    );

    const unexpectedExecutable = completeRelease(releaseTag);
    unexpectedExecutable.assets.push({
      digest: `sha256:${"b".repeat(64)}`,
      name: "CodexAppManager_1.2.3_unsigned-debug.exe",
      size: 10,
    });
    expect(() => inspectReleaseForReuse(unexpectedExecutable, releaseTag)).toThrow(
      "has unexpected assets and cannot be trusted",
    );

    const optionalSbom = completeRelease(releaseTag);
    optionalSbom.assets.push({
      digest: `sha256:${"c".repeat(64)}`,
      name: OPTIONAL_RELEASE_METADATA_ASSET_NAMES[0],
      size: 10,
    });
    expect(inspectReleaseForReuse(optionalSbom, releaseTag)).toMatchObject({
      reusable: true,
    });
  });
});

describe("Tauri updater verification", () => {
  it("accepts the configured minisign format and rejects changed bytes", async () => {
    const root = await tempRoot("updater-signature");
    const artifact = Buffer.from("signed updater bytes");
    const fixture = updaterFixture(artifact);
    const path = join(root, "artifact.bin");
    await writeFile(path, artifact);

    await expect(
      verifyTauriUpdaterSignature(path, fixture.signature, fixture.publicKey),
    ).resolves.toBe(true);

    await writeFile(path, Buffer.from("tampered updater bytes"));
    await expect(
      verifyTauriUpdaterSignature(path, fixture.signature, fixture.publicKey),
    ).rejects.toThrow("artifact signature is invalid");
  });

  it("verifies local manifest payloads and sidecars before publication", async () => {
    const root = await tempRoot("local-updater-signatures");
    const artifact = Buffer.from("locally signed updater bytes");
    const fixture = updaterFixture(artifact);
    const candidate = manifest("1.2.3", fixture.signature);
    candidate.platforms["windows-x86_64"].sha256 = hash(artifact);
    const artifactName = "manager-1.2.3.exe";
    await writeFile(join(root, artifactName), artifact);
    await writeFile(join(root, `${artifactName}.sig`), `${fixture.signature}\n`);

    await expect(
      verifyLocalUpdaterArtifacts({
        manifest: candidate,
        distDir: root,
        publicKey: fixture.publicKey,
      }),
    ).resolves.toMatchObject({ artifactCount: 1, verified: true });

    const wrongKey = updaterFixture(Buffer.from("different release")).publicKey;
    await expect(
      verifyLocalUpdaterArtifacts({
        manifest: candidate,
        distDir: root,
        publicKey: wrongKey,
      }),
    ).rejects.toThrow("artifact signature is invalid");

    candidate.platforms["windows-x86_64"].sha256 = "b".repeat(64);
    await expect(
      verifyLocalUpdaterArtifacts({
        manifest: candidate,
        distDir: root,
        publicKey: fixture.publicKey,
      }),
    ).rejects.toThrow("artifact sha256 does not match manifest");
  });
});

describe("local signed release identity verification", () => {
  it("verifies an explicitly expected prerelease identity without making it root-promotable", async () => {
    const root = await tempRoot("local-prerelease-identity");
    const artifact = Buffer.from("prerelease updater payload");
    const fixture = updaterFixture(artifact);
    const candidate = completeManifest("1.2.3-rc.1", fixture.signature);
    for (const entry of Object.values(candidate.platforms)) {
      entry.sha256 = hash(artifact);
    }
    candidate.channel = "prerelease";
    for (const entry of Object.values(candidate.platforms)) {
      const name = new URL(entry.url).pathname.split("/").at(-1);
      await writeFile(join(root, name), artifact);
    }
    const releaseIdentity = await writeReleaseIdentity(root, candidate, fixture, {
      channel: "prerelease",
    });

    await expect(
      verifyLocalReleaseIdentity({
        candidateManifest: candidate,
        distDir: root,
        expectedChannel: "prerelease",
        publicKey: fixture.publicKey,
      }),
    ).resolves.toMatchObject({
      identity: releaseIdentity.identity,
      identityPath: releaseIdentity.identityPath,
    });
    await expect(
      verifyLocalReleaseIdentity({
        candidateManifest: candidate,
        distDir: root,
        expectedChannel: "stable",
        publicKey: fixture.publicKey,
      }),
    ).rejects.toThrow("version 1.2.3-rc.1 is prerelease");
  });

  it.each([
    ["missing", undefined],
    ["invalid", "A".repeat(64)],
  ])(
    "rejects historical immutable reuse with a %s manifest sha256 even when its signed identity is valid",
    async (_case, manifestSha256) => {
      const root = await tempRoot(`historical-${_case}-manifest-sha256`);
      const artifact = Buffer.from("historical immutable updater payload");
      const fixture = updaterFixture(artifact);
      const candidate = completeManifest("1.2.3", fixture.signature);
      for (const entry of Object.values(candidate.platforms)) {
        entry.sha256 = hash(artifact);
        const name = new URL(entry.url).pathname.split("/").at(-1);
        await writeFile(join(root, name), artifact);
        await writeFile(join(root, `${name}.sig`), `${fixture.signature}\n`);
      }
      await writeReleaseIdentity(root, candidate, fixture);

      const publishedManifest = structuredClone(candidate);
      if (manifestSha256 === undefined) {
        delete publishedManifest.platforms["windows-x86_64"].sha256;
      } else {
        publishedManifest.platforms["windows-x86_64"].sha256 = manifestSha256;
      }

      await expect(
        verifyLocalUpdaterArtifacts({
          manifest: publishedManifest,
          distDir: root,
          publicKey: fixture.publicKey,
        }),
      ).rejects.toThrow("sha256 must be a lowercase SHA-256 digest");
      await expect(
        verifyLocalReleaseIdentity({
          candidateManifest: publishedManifest,
          distDir: root,
          expectedChannel: "stable",
          publicKey: fixture.publicKey,
        }),
      ).rejects.toThrow("sha256 must be a lowercase SHA-256 digest");
    },
  );

  it("rejects a valid-looking manifest sha256 that differs from its signed identity", async () => {
    const root = await tempRoot("historical-mismatched-manifest-sha256");
    const artifact = Buffer.from("historical updater payload with signed identity");
    const fixture = updaterFixture(artifact);
    const candidate = completeManifest("1.2.3", fixture.signature);
    for (const entry of Object.values(candidate.platforms)) {
      entry.sha256 = hash(artifact);
      const name = new URL(entry.url).pathname.split("/").at(-1);
      await writeFile(join(root, name), artifact);
    }
    await writeReleaseIdentity(root, candidate, fixture);
    candidate.platforms["windows-x86_64"].sha256 = "b".repeat(64);

    await expect(
      verifyLocalReleaseIdentity({
        candidateManifest: candidate,
        distDir: root,
        expectedChannel: "stable",
        publicKey: fixture.publicKey,
      }),
    ).rejects.toThrow("sha256 does not match candidate manifest");
  });
});

describe("public mirror route verification", () => {
  it("downloads the candidate and every updater payload through public URLs", async () => {
    const root = await tempRoot("public-route");
    const artifact = Buffer.from("public updater payload");
    const fixture = updaterFixture(artifact);
    const candidate = completeManifest("1.2.3", fixture.signature);
    for (const entry of Object.values(candidate.platforms)) {
      entry.sha256 = hash(artifact);
    }
    const candidateKey = candidateKeyFor("1.2.3", "public-run");
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const objects = new Map([
      [`/manager/${candidateKey}`, await readFile(candidatePath)],
    ]);
    const expectedArtifacts = Object.entries(candidate.platforms).map(
      ([platform, entry]) => {
        const name = new URL(entry.url).pathname.split("/").at(-1);
        objects.set(`/manager/1.2.3/${name}`, artifact);
        return {
          key: `1.2.3/${name}`,
          localPath: join(root, name),
          name,
          platform,
          sha256: hash(artifact),
          signature: fixture.signature,
          size: artifact.length,
        };
      },
    );
    const requests = [];

    const report = await verifyPublicMirrorRoute({
      candidateKey,
      candidateManifest: candidate,
      candidatePath,
      expectedArtifacts,
      ihepRedirect: TEST_IHEP_REDIRECT,
      mirrorBase: "https://mirror.example/manager",
      publicKey: fixture.publicKey,
      workDir: join(root, "verify"),
      fetchImpl: publicRouteFetch(objects, requests),
    });

    expect(report).toMatchObject({ artifactCount: 4, backendCount: 2, verified: true });
    expect(report.backends.r2).toMatchObject({ artifactCount: 4, verified: true });
    expect(report.backends.ihep).toMatchObject({ artifactCount: 4, verified: true });
    expect(requests).toHaveLength(15);
    const workerRequests = requests.filter((url) => url.hostname === "mirror.example");
    expect(workerRequests).toHaveLength(10);
    expect(workerRequests.every((url) => url.searchParams.has("cam_probe"))).toBe(true);
    expect(new Set(workerRequests.map((url) => url.searchParams.get("cam_backend")))).toEqual(
      new Set(["r2", "ihep"]),
    );

    const corruptedName = expectedArtifacts[0].name;
    const corruptedIhep = new Map(objects);
    corruptedIhep.set(`/manager/1.2.3/${corruptedName}`, Buffer.from("corrupt"));
    await expect(
      verifyPublicMirrorRoute({
        candidateKey,
        candidateManifest: candidate,
        candidatePath,
        expectedArtifacts,
        ihepRedirect: TEST_IHEP_REDIRECT,
        mirrorBase: "https://mirror.example/manager",
        publicKey: fixture.publicKey,
        workDir: join(root, "corrupt"),
        fetchImpl: publicRouteFetch(objects, [], { ihepObjects: corruptedIhep }),
      }),
    ).rejects.toThrow(`public mirror ihep size mismatch for ${corruptedName}`);
  });

  it.each([
    ["origin", { endpoint: "https://r2.example/root" }],
    ["bucket", { bucket: "wrong-bucket" }],
    ["prefix", { prefix: "wrong-prefix" }],
  ])("rejects an IHEP redirect with the wrong %s even when bytes are identical", async (_name, overrides) => {
    const root = await tempRoot("public-route-binding");
    const artifact = Buffer.from("identical public updater payload");
    const fixture = updaterFixture(artifact);
    const candidate = completeManifest("1.2.3", fixture.signature);
    for (const entry of Object.values(candidate.platforms)) {
      entry.sha256 = hash(artifact);
    }
    const candidateKey = candidateKeyFor("1.2.3", "binding-run");
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const objects = new Map([[`/manager/${candidateKey}`, await readFile(candidatePath)]]);
    const expectedArtifacts = Object.entries(candidate.platforms).map(([platform, entry]) => {
      const name = new URL(entry.url).pathname.split("/").at(-1);
      objects.set(`/manager/1.2.3/${name}`, artifact);
      return {
        key: `1.2.3/${name}`,
        localPath: join(root, name),
        name,
        platform,
        sha256: hash(artifact),
        signature: fixture.signature,
        size: artifact.length,
      };
    });

    await expect(
      verifyPublicMirrorRoute({
        candidateKey,
        candidateManifest: candidate,
        candidatePath,
        expectedArtifacts,
        ihepRedirect: TEST_IHEP_REDIRECT,
        mirrorBase: "https://mirror.example/manager",
        publicKey: fixture.publicKey,
        workDir: join(root, "verify"),
        fetchImpl: publicRouteFetch(objects, [], { ihepRedirectOverrides: overrides }),
      }),
    ).rejects.toThrow(
      "Worker IHEP redirect target does not match the configured endpoint, bucket, prefix, and object",
    );
  });
});

describe("AWS latest writes", () => {
  function objectStore() {
    return new AwsObjectStore({
      name: "r2",
      endpoint: "https://r2.example.invalid",
      bucket: "manager",
      region: "auto",
      accessKeyId: "test-access-key",
      secretAccessKey: "test-secret-key",
      configPath: "/tmp/test-aws-config",
    });
  }

  it("uses one attempt and trusts the committing PutObject response ETag", async () => {
    const root = await tempRoot("conditional-put");
    const candidatePath = await writeManifest(root, "candidate.json", manifest("1.2.3"));
    const backend = objectStore();
    let call;
    backend.aws = async (args, options) => {
      call = { args, options };
      return { code: 0, stderr: "", stdout: '{"ETag":"\\"committed-etag\\""}' };
    };
    backend.head = async () => {
      throw new Error("post-write HEAD must not determine write ownership");
    };

    const result = await backend.putLatestConditional(
      candidatePath,
      '"previous-etag"',
      "promotion-token",
    );

    expect(result.etag).toBe('"committed-etag"');
    expect(call.options.env).toEqual({ AWS_MAX_ATTEMPTS: "1" });
    expect(call.args).toEqual(
      expect.arrayContaining([
        "--if-match",
        '"previous-etag"',
        "--metadata",
        "cam-promotion-token=promotion-token",
      ]),
    );
  });

  it("reports a final 412 as a conditional conflict without claiming ownership", async () => {
    const root = await tempRoot("conditional-conflict");
    const candidatePath = await writeManifest(root, "candidate.json", manifest("1.2.3"));
    const backend = objectStore();
    backend.aws = async () => ({
      code: 1,
      stderr: "PreconditionFailed (412)",
      stdout: "",
    });

    await expect(
      backend.putLatestConditional(candidatePath, '"stale-etag"', "promotion-token"),
    ).rejects.toBeInstanceOf(ConditionalWriteError);
  });

  it("omits conditional headers for the serialized IHEP follower write", async () => {
    const root = await tempRoot("unconditional-follower-put");
    const candidatePath = await writeManifest(root, "candidate.json", manifest("1.2.3"));
    const backend = objectStore();
    let call;
    backend.aws = async (args, options) => {
      call = { args, options };
      return { code: 0, stderr: "", stdout: '{"ETag":"\\"follower-etag\\""}' };
    };

    const result = await backend.putLatestUnconditional(candidatePath, "promotion-token");

    expect(result.etag).toBe('"follower-etag"');
    expect(call.options.env).toEqual({ AWS_MAX_ATTEMPTS: "1" });
    expect(call.args).toEqual(
      expect.arrayContaining([
        "--metadata",
        "cam-promotion-token=promotion-token",
      ]),
    );
    expect(call.args).not.toContain("--if-match");
    expect(call.args).not.toContain("--if-none-match");
  });
});

describe("backend candidate verification", () => {
  it("downloads candidate and artifact and verifies version, size, hash, and signature", async () => {
    const root = await tempRoot("candidate-verify");
    const dist = join(root, "dist");
    await mkdir(dist, { recursive: true });
    const artifact = Buffer.from("release artifact from object storage");
    const fixture = updaterFixture(artifact);
    const candidate = manifest("1.2.3", fixture.signature);
    candidate.platforms["windows-x86_64"].sha256 = hash(artifact);
    const artifactName = "manager-1.2.3.exe";
    const dmgName = "CodexAppManager_aarch64.dmg";
    const signatureName = `${artifactName}.sig`;
    const dmg = Buffer.from("macOS installer bytes");
    const signatureSidecar = Buffer.from(`${fixture.signature}\n`);
    await writeFile(join(dist, artifactName), artifact);
    await writeFile(join(dist, dmgName), dmg);
    await writeFile(join(dist, signatureName), signatureSidecar);
    const candidatePath = await writeManifest(dist, "latest.mirror.json", candidate);
    const key = candidateKeyFor("1.2.3", "run-1");
    const backend = new MemoryBackend("r2", {
      [key]: await readFile(candidatePath),
      [`1.2.3/${artifactName}`]: artifact,
      [`1.2.3/${dmgName}`]: dmg,
      [`1.2.3/${signatureName}`]: signatureSidecar,
    });

    const report = await verifyBackendCandidate({
      backend,
      candidateKey: key,
      candidatePath,
      candidateManifest: candidate,
      distDir: dist,
      mirrorBase: "https://mirror.example/manager",
      publicKey: fixture.publicKey,
      workDir: join(root, "verify"),
    });

    expect(report.verified).toBe(true);
    expect(report.artifactCount).toBe(3);
    expect(report.artifacts).toEqual(
      expect.arrayContaining([
        expect.objectContaining({
          name: artifactName,
          sha256: hash(artifact),
          signatureVerified: true,
          size: artifact.length,
        }),
        expect.objectContaining({
          name: dmgName,
          sha256: hash(dmg),
          signatureVerified: null,
          size: dmg.length,
        }),
        expect.objectContaining({
          name: signatureName,
          sha256: hash(signatureSidecar),
          signatureVerified: null,
          size: signatureSidecar.length,
        }),
      ]),
    );
  });

  it("rejects an empty platform signature instead of silently skipping verification", async () => {
    const root = await tempRoot("candidate-empty-signature");
    const dist = join(root, "dist");
    await mkdir(dist, { recursive: true });
    const candidate = manifest("1.2.3", "");
    const candidatePath = await writeManifest(dist, "latest.mirror.json", candidate);
    const key = candidateKeyFor("1.2.3", "run-empty-signature");
    const backend = new MemoryBackend("r2", {
      [key]: await readFile(candidatePath),
    });

    await expect(
      verifyBackendCandidate({
        backend,
        candidateKey: key,
        candidatePath,
        candidateManifest: candidate,
        distDir: dist,
        mirrorBase: "https://mirror.example/manager",
        publicKey: "deliberately-invalid-public-key",
        workDir: join(root, "verify"),
      }),
    ).rejects.toThrow("empty updater signature");
  });

  it("requires each manifest signature to match its local sidecar", async () => {
    const root = await tempRoot("candidate-sidecar-mismatch");
    const dist = join(root, "dist");
    await mkdir(dist, { recursive: true });
    const artifact = Buffer.from("release artifact from object storage");
    const fixture = updaterFixture(artifact);
    const candidate = manifest("1.2.3", fixture.signature);
    candidate.platforms["windows-x86_64"].sha256 = hash(artifact);
    const artifactName = "manager-1.2.3.exe";
    await writeFile(join(dist, artifactName), artifact);
    await writeFile(join(dist, `${artifactName}.sig`), "different-signature\n");
    const candidatePath = await writeManifest(dist, "latest.mirror.json", candidate);
    const key = candidateKeyFor("1.2.3", "run-sidecar-mismatch");
    const backend = new MemoryBackend("r2", {
      [key]: await readFile(candidatePath),
    });

    await expect(
      verifyBackendCandidate({
        backend,
        candidateKey: key,
        candidatePath,
        candidateManifest: candidate,
        distDir: dist,
        mirrorBase: "https://mirror.example/manager",
        publicKey: fixture.publicKey,
        workDir: join(root, "verify"),
      }),
    ).rejects.toThrow("sidecar does not match manifest");
  });
});

describe("pre-publication mirror verification", () => {
  it("verifies both origins and the public route without writing latest.json", async () => {
    const root = await tempRoot("prepublish-verify");
    const dist = join(root, "dist");
    await mkdir(dist, { recursive: true });
    const artifact = Buffer.from("stable updater payload");
    const fixture = updaterFixture(artifact);
    const candidate = completeManifest("1.2.3", fixture.signature);
    for (const entry of Object.values(candidate.platforms)) {
      entry.sha256 = hash(artifact);
    }
    const candidatePath = await writeManifest(dist, "latest.mirror.json", candidate);
    const candidateKey = candidateKeyFor("1.2.3", "verify-run");
    const candidateBytes = await readFile(candidatePath);
    const stagedObjects = {};
    const publicObjects = new Map([[`/manager/${candidateKey}`, candidateBytes]]);
    for (const entry of Object.values(candidate.platforms)) {
      const name = new URL(entry.url).pathname.split("/").at(-1);
      const sidecar = Buffer.from(`${fixture.signature}\n`);
      await writeFile(join(dist, name), artifact);
      await writeFile(join(dist, `${name}.sig`), sidecar);
      stagedObjects[`1.2.3/${name}`] = artifact;
      stagedObjects[`1.2.3/${name}.sig`] = sidecar;
      publicObjects.set(`/manager/1.2.3/${name}`, artifact);
    }
    const releaseIdentity = await writeReleaseIdentity(dist, candidate, fixture);
    stagedObjects["1.2.3/release-identity.json"] = releaseIdentity.identityBytes;
    stagedObjects["1.2.3/release-identity.json.sig"] = await readFile(
      releaseIdentity.signaturePath,
    );
    const current = Buffer.from(`${JSON.stringify(manifest("1.0.0"))}\n`);
    const r2 = new MemoryBackend("r2", {
      "latest.json": current,
      [candidateKey]: candidateBytes,
      ...stagedObjects,
    });
    const ihep = new MemoryBackend("ihep", {
      "latest.json": current,
      [candidateKey]: candidateBytes,
      ...stagedObjects,
    });
    const promotionToken = hash(Buffer.from(candidateKey));
    r2.objects.get(candidateKey).metadata["cam-promotion-token"] = promotionToken;
    ihep.objects.get(candidateKey).metadata["cam-promotion-token"] = promotionToken;
    const summaryPath = join(root, "mirror-verification-summary.json");

    const summary = await verifyMirrors({
      backends: [r2, ihep],
      candidateKey,
      candidateManifest: candidate,
      candidatePath,
      distDir: dist,
      mirrorBase: "https://mirror.example/manager",
      override: overrideOff(),
      publicKey: fixture.publicKey,
      summaryPath,
      tempRoot: join(root, "verify"),
      fetchImpl: publicRouteFetch(publicObjects),
    });

    expect(summary.outcome).toBe("verified");
    expect(summary.publicRouteVerification).toBe("passed");
    expect(Object.keys(summary.publicRoute.backends)).toEqual(["r2", "ihep"]);
    expect(summary.backends.map((backend) => backend.candidateVerification)).toEqual([
      "passed",
      "passed",
    ]);
    expect([r2.latestPutAttempts, ihep.latestPutAttempts]).toEqual([0, 0]);
    expect(JSON.parse(await readFile(summaryPath, "utf8")).outcome).toBe("verified");

    const corruptedName = new URL(Object.values(candidate.platforms)[0].url).pathname
      .split("/")
      .at(-1);
    const corruptedIhep = new Map(publicObjects);
    corruptedIhep.set(`/manager/1.2.3/${corruptedName}`, Buffer.from("corrupt"));
    await expect(
      promoteMirrors({
        backends: [r2, ihep],
        candidateKey,
        candidateManifest: candidate,
        candidatePath,
        distDir: dist,
        mirrorBase: "https://mirror.example/manager",
        override: overrideOff(),
        publicKey: fixture.publicKey,
        summaryPath: join(root, "failed-promotion-summary.json"),
        tempRoot: join(root, "failed-promote"),
        fetchImpl: publicRouteFetch(publicObjects, [], { ihepObjects: corruptedIhep }),
      }),
    ).rejects.toThrow(`public mirror ihep size mismatch for ${corruptedName}`);
    expect([r2.latestPutAttempts, ihep.latestPutAttempts]).toEqual([0, 0]);
  });
});

describe("stable root release identity promotion", () => {
  it("writes both signatures before either JSON pointer and verifies direct and public readback", async () => {
    const root = await tempRoot("root-identity-success");
    const setup = await prepareMirrorPromotionFixture(root);
    const writes = [];
    setup.r2.onRootIdentityWrite = (entry) => writes.push(entry);
    setup.ihep.onRootIdentityWrite = (entry) => writes.push(entry);
    const requests = [];
    const summaryPath = join(root, "promotion-summary.json");

    const summary = await promoteMirrors({
      backends: [setup.r2, setup.ihep],
      candidateKey: setup.candidateKey,
      candidateManifest: setup.candidate,
      candidatePath: setup.candidatePath,
      distDir: setup.dist,
      mirrorBase: "https://mirror.example/manager",
      override: overrideOff(),
      publicKey: setup.fixture.publicKey,
      summaryPath,
      tempRoot: join(root, "promote"),
      fetchImpl: liveBackendPublicRouteFetch(setup.r2, setup.ihep, requests),
    });

    expect(summary.outcome).toBe("promoted");
    expect(writes).toEqual([
      "r2:release-identity.json.sig",
      "ihep:release-identity.json.sig",
      "r2:release-identity.json",
      "ihep:release-identity.json",
    ]);
    for (const backend of [setup.r2, setup.ihep]) {
      expect(backend.body("release-identity.json")).toEqual(
        setup.releaseIdentity.identityBytes,
      );
      expect(backend.body("release-identity.json.sig")).toEqual(
        await readFile(setup.releaseIdentity.signaturePath),
      );
    }
    expect(summary.rootIdentity).toMatchObject({
      channel: "stable",
      localVerification: "passed",
      publicRouteVerification: "passed",
      version: "1.2.3",
    });
    expect(summary.backends.map((backend) => backend.rootIdentity)).toEqual([
      expect.objectContaining({
        directVerification: "passed",
        jsonWrite: "written-and-verified",
        publicVerification: "passed",
        signatureWrite: "written-and-verified",
      }),
      expect.objectContaining({
        directVerification: "passed",
        jsonWrite: "written-and-verified",
        publicVerification: "passed",
        signatureWrite: "written-and-verified",
      }),
    ]);
    const rootRequests = requests.filter((url) =>
      url.pathname.includes("release-identity.json"),
    );
    expect(rootRequests.some((url) => url.searchParams.get("cam_backend") === "r2")).toBe(
      true,
    );
    expect(
      rootRequests.some((url) => url.searchParams.get("cam_backend") === "ihep"),
    ).toBe(true);
    expect(JSON.parse(await readFile(summaryPath, "utf8")).outcome).toBe("promoted");
  });

  it("never overwrites a mismatched versioned identity and fails before latest promotion", async () => {
    const root = await tempRoot("versioned-identity-mismatch");
    const staleBytes = Buffer.from("pre-existing mismatched identity bytes");
    const setup = await prepareMirrorPromotionFixture(root, {
      preexistingR2: { "1.2.3/release-identity.json": staleBytes },
    });

    expect(setup.r2.body("1.2.3/release-identity.json")).toEqual(staleBytes);
    await expect(
      promoteMirrors({
        backends: [setup.r2, setup.ihep],
        candidateKey: setup.candidateKey,
        candidateManifest: setup.candidate,
        candidatePath: setup.candidatePath,
        distDir: setup.dist,
        mirrorBase: "https://mirror.example/manager",
        override: overrideOff(),
        publicKey: setup.fixture.publicKey,
        summaryPath: join(root, "promotion-summary.json"),
        tempRoot: join(root, "promote"),
      }),
    ).rejects.toThrow(/(?:size|sha256) mismatch for release-identity\.json/);
    expect(setup.r2.body("1.2.3/release-identity.json")).toEqual(staleBytes);
    expect([setup.r2.latestPutAttempts, setup.ihep.latestPutAttempts]).toEqual([0, 0]);
    expect(setup.r2.body("release-identity.json")).toBeUndefined();
    expect(setup.ihep.body("release-identity.json")).toBeUndefined();
  });

  it("rejects prerelease identities before any mutable pointer write", async () => {
    const root = await tempRoot("prerelease-root-identity");
    const setup = await prepareMirrorPromotionFixture(root, {
      channel: "prerelease",
      version: "1.2.3-rc.1",
    });

    await expect(
      promoteMirrors({
        backends: [setup.r2, setup.ihep],
        candidateKey: setup.candidateKey,
        candidateManifest: setup.candidate,
        candidatePath: setup.candidatePath,
        distDir: setup.dist,
        mirrorBase: "https://mirror.example/manager",
        override: overrideOff(),
        publicKey: setup.fixture.publicKey,
        summaryPath: join(root, "promotion-summary.json"),
        tempRoot: join(root, "promote"),
      }),
    ).rejects.toThrow("root release identity accepts only channel=stable");
    expect([setup.r2.latestPutAttempts, setup.ihep.latestPutAttempts]).toEqual([0, 0]);
    expect(setup.r2.body("release-identity.json")).toBeUndefined();
    expect(setup.ihep.body("release-identity.json.sig")).toBeUndefined();
  });

  it("fails on stale public pointer bytes and a retry repairs the same signed pair", async () => {
    const root = await tempRoot("root-identity-public-readback");
    const setup = await prepareMirrorPromotionFixture(root);
    const liveFetch = liveBackendPublicRouteFetch(setup.r2, setup.ihep);
    const stalePublicFetch = async (value) => {
      const url = new URL(value);
      const response = await liveFetch(value);
      if (
        url.hostname === "ihep.example" &&
        url.pathname.endsWith("/release-identity.json")
      ) {
        await response.body?.cancel().catch(() => {});
        return new Response("stale cached identity", { status: 200 });
      }
      return response;
    };
    const failedSummaryPath = join(root, "failed-summary.json");

    await expect(
      promoteMirrors({
        backends: [setup.r2, setup.ihep],
        candidateKey: setup.candidateKey,
        candidateManifest: setup.candidate,
        candidatePath: setup.candidatePath,
        distDir: setup.dist,
        mirrorBase: "https://mirror.example/manager",
        override: overrideOff(),
        publicKey: setup.fixture.publicKey,
        summaryPath: failedSummaryPath,
        tempRoot: join(root, "failed-promote"),
        fetchImpl: stalePublicFetch,
      }),
    ).rejects.toThrow(
      "public mirror ihep root release identity does not match this release run",
    );
    expect(JSON.parse(await readFile(failedSummaryPath, "utf8"))).toMatchObject({
      outcome: "failed",
      rootIdentity: { publicRouteVerification: "failed" },
    });

    const repaired = await promoteMirrors({
      backends: [setup.r2, setup.ihep],
      candidateKey: setup.candidateKey,
      candidateManifest: setup.candidate,
      candidatePath: setup.candidatePath,
      distDir: setup.dist,
      mirrorBase: "https://mirror.example/manager",
      override: overrideOff(),
      publicKey: setup.fixture.publicKey,
      summaryPath: join(root, "repaired-summary.json"),
      tempRoot: join(root, "repaired-promote"),
      fetchImpl: liveBackendPublicRouteFetch(setup.r2, setup.ihep),
    });
    expect(repaired.outcome).toBe("idempotent");
    expect(repaired.rootIdentity.publicRouteVerification).toBe("passed");
  });
});

describe("monotonic mirror promotion", () => {
  it("blocks an old tag rerun without writing either backend", async () => {
    const root = await tempRoot("old-rerun");
    const current = manifest("2.0.0");
    const candidate = manifest("1.9.0");
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const initial = `${JSON.stringify(current)}\n`;
    const backends = [
      new MemoryBackend("r2", { "latest.json": initial }),
      new MemoryBackend("ihep", { "latest.json": initial }),
    ];

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        summary: summaryFor(backends),
        workDir: join(root, "transaction"),
      }),
    ).rejects.toBeInstanceOf(DowngradeBlockedError);

    expect(backends.map((backend) => backend.latestPutAttempts)).toEqual([0, 0]);
    expect(backends.map((backend) => JSON.parse(backend.body("latest.json")).version)).toEqual([
      "2.0.0",
      "2.0.0",
    ]);
  });

  it("never lets an old rerun fill a lagging IHEP follower behind newer R2", async () => {
    const root = await tempRoot("old-rerun-lagging-follower");
    const candidate = manifest("1.9.0");
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const r2 = new MemoryBackend("r2", {
      "latest.json": `${JSON.stringify(manifest("2.0.0"))}\n`,
    });
    const ihep = new ConditionIgnoringMemoryBackend("ihep", {
      "latest.json": `${JSON.stringify(manifest("1.0.0"))}\n`,
    });
    const backends = [r2, ihep];

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        summary: summaryFor(backends),
        workDir: join(root, "transaction"),
      }),
    ).rejects.toBeInstanceOf(DowngradeBlockedError);

    expect(r2.latestPutAttempts).toBe(0);
    expect(ihep.latestPutAttempts).toBe(0);
    expect(JSON.parse(r2.body("latest.json")).version).toBe("2.0.0");
    expect(JSON.parse(ihep.body("latest.json")).version).toBe("1.0.0");
  });

  it("uses IHEP only as an unconditional follower even when it ignores conditions", async () => {
    const root = await tempRoot("ihep-ignores-conditions");
    const candidate = manifest("1.1.0");
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const initial = `${JSON.stringify(manifest("1.0.0"))}\n`;
    const r2 = new MemoryBackend("r2", { "latest.json": initial });
    const ihep = new ConditionIgnoringMemoryBackend("ihep", { "latest.json": initial });
    const backends = [r2, ihep];

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        promotionToken: "single-writer",
        summary: summaryFor(backends),
        workDir: join(root, "transaction"),
      }),
    ).resolves.toEqual(expect.objectContaining({ outcome: "promoted" }));

    expect(r2.conditionalLatestPutAttempts).toBe(1);
    expect(ihep.conditionalLatestPutAttempts).toBe(0);
    expect(ihep.unconditionalLatestPutAttempts).toBe(1);
    expect(JSON.parse(ihep.body("latest.json")).version).toBe("1.1.0");
  });

  it("migrates a strictly older legacy baseline that predates manifest sha256", async () => {
    const root = await tempRoot("legacy-baseline-forward-migration");
    const legacy = manifest("1.0.0");
    delete legacy.platforms["windows-x86_64"].sha256;
    const candidate = manifest("1.1.0");
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const initial = `${JSON.stringify(legacy)}\n`;
    const r2 = new MemoryBackend("r2", { "latest.json": initial });
    const ihep = new ConditionIgnoringMemoryBackend("ihep", { "latest.json": initial });
    const backends = [r2, ihep];

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        promotionToken: "legacy-forward-migration",
        summary: summaryFor(backends),
        workDir: join(root, "transaction"),
      }),
    ).resolves.toEqual(expect.objectContaining({ outcome: "promoted" }));

    expect(JSON.parse(r2.body("latest.json"))).toEqual(candidate);
    expect(JSON.parse(ihep.body("latest.json"))).toEqual(candidate);
  });

  it("rejects same-version reuse of a legacy baseline without sha256", async () => {
    const root = await tempRoot("legacy-baseline-same-version");
    const legacy = manifest("1.0.0");
    delete legacy.platforms["windows-x86_64"].sha256;
    const candidate = manifest("1.0.0");
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const original = Buffer.from(`${JSON.stringify(legacy)}\n`);
    const backends = [
      new MemoryBackend("r2", { "latest.json": original }),
      new MemoryBackend("ihep", { "latest.json": original }),
    ];

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        summary: summaryFor(backends),
        workDir: join(root, "transaction"),
      }),
    ).rejects.toThrow("sha256 must be a lowercase SHA-256 digest");
    expect(backends.map((backend) => backend.latestPutAttempts)).toEqual([0, 0]);
    expect(backends.every((backend) => backend.body("latest.json").equals(original))).toBe(true);
  });

  it("rejects an invalid legacy digest even for a forward promotion", async () => {
    const root = await tempRoot("legacy-baseline-invalid-digest");
    const invalid = manifest("1.0.0");
    invalid.platforms["windows-x86_64"].sha256 = "A".repeat(64);
    const candidate = manifest("1.1.0");
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const original = Buffer.from(`${JSON.stringify(invalid)}\n`);
    const backends = [
      new MemoryBackend("r2", { "latest.json": original }),
      new MemoryBackend("ihep", { "latest.json": original }),
    ];

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        summary: summaryFor(backends),
        workDir: join(root, "transaction"),
      }),
    ).rejects.toThrow("sha256 must be a lowercase SHA-256 digest");
    expect(backends.map((backend) => backend.latestPutAttempts)).toEqual([0, 0]);
  });

  it("does not let an R2 CAS loser touch condition-ignoring IHEP", async () => {
    const root = await tempRoot("r2-cas-loser");
    const candidate = manifest("1.1.0");
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const initial = `${JSON.stringify(manifest("1.0.0"))}\n`;
    const r2 = new MemoryBackend("r2", { "latest.json": initial });
    const ihep = new ConditionIgnoringMemoryBackend("ihep", { "latest.json": initial });
    const realPut = r2.putLatestConditional.bind(r2);
    let raced = false;
    r2.putLatestConditional = async (...args) => {
      if (!raced) {
        raced = true;
        r2.set(
          "latest.json",
          `${JSON.stringify(manifest("1.2.0"))}\n`,
          { "cam-promotion-token": "newer-winner" },
        );
      }
      return await realPut(...args);
    };
    const backends = [r2, ihep];

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        summary: summaryFor(backends),
        workDir: join(root, "transaction"),
      }),
    ).rejects.toBeInstanceOf(ConditionalWriteError);

    expect(ihep.conditionalLatestPutAttempts).toBe(0);
    expect(ihep.unconditionalLatestPutAttempts).toBe(0);
    expect(JSON.parse(r2.body("latest.json")).version).toBe("1.2.0");
    expect(JSON.parse(ihep.body("latest.json")).version).toBe("1.0.0");
  });

  it("treats a same-version rerun as a no-write idempotent success", async () => {
    const root = await tempRoot("same-version");
    const current = manifest("2.0.0");
    current.pub_date = "2026-01-01T00:00:00Z";
    const candidate = manifest("2.0.0");
    candidate.pub_date = "2026-02-01T00:00:00Z";
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const original = Buffer.from(`${JSON.stringify(current)}\n`);
    const backends = [
      new MemoryBackend("r2", { "latest.json": original }),
      new MemoryBackend("ihep", { "latest.json": original }),
    ];

    const result = await promoteCandidateTransaction({
      backends,
      candidateManifest: candidate,
      candidatePath,
      override: overrideOff(),
      summary: summaryFor(backends),
      workDir: join(root, "transaction"),
    });

    expect(result.outcome).toBe("idempotent");
    expect(backends.map((backend) => backend.latestPutAttempts)).toEqual([0, 0]);
    expect(backends.every((backend) => backend.body("latest.json").equals(original))).toBe(true);
  });

  it("does not treat a same-version manifest with a different sha256 as idempotent", async () => {
    const root = await tempRoot("same-version-sha256-mismatch");
    const current = manifest("2.0.0");
    const candidate = structuredClone(current);
    candidate.platforms["windows-x86_64"].sha256 = "b".repeat(64);
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const original = Buffer.from(`${JSON.stringify(current)}\n`);
    const backends = [
      new MemoryBackend("r2", { "latest.json": original }),
      new MemoryBackend("ihep", { "latest.json": original }),
    ];

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        summary: summaryFor(backends),
        workDir: join(root, "transaction"),
      }),
    ).rejects.toThrow("different channel/notes/artifact/signature/sha256 identity");
    expect(backends.map((backend) => backend.latestPutAttempts)).toEqual([0, 0]);
  });

  it.each([
    ["release notes", (current) => { current.notes = "tampered release notes"; }],
    ["channel", (current) => { current.channel = "prerelease"; }],
  ])("does not treat same-version %s drift as idempotent", async (_field, mutate) => {
    const root = await tempRoot(`same-version-${_field.replaceAll(" ", "-")}-mismatch`);
    const candidate = manifest("2.0.0");
    candidate.channel = "stable";
    const current = structuredClone(candidate);
    mutate(current);
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const original = Buffer.from(`${JSON.stringify(current)}\n`);
    const backends = [
      new MemoryBackend("r2", { "latest.json": original }),
      new MemoryBackend("ihep", { "latest.json": original }),
    ];

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        summary: summaryFor(backends),
        workDir: join(root, "transaction"),
      }),
    ).rejects.toThrow("different channel/notes/artifact/signature/sha256 identity");
    expect(backends.map((backend) => backend.latestPutAttempts)).toEqual([0, 0]);
    expect(backends.every((backend) => backend.body("latest.json").equals(original))).toBe(true);
  });

  it("rejects a concurrent latest change before returning idempotent", async () => {
    const root = await tempRoot("same-version-race");
    const candidate = manifest("2.0.0");
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const original = Buffer.from(`${JSON.stringify(candidate)}\n`);
    const r2 = new MemoryBackend("r2", { "latest.json": original });
    const ihep = new MemoryBackend("ihep", { "latest.json": original });
    const backends = [r2, ihep];

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        summary: summaryFor(backends),
        workDir: join(root, "transaction"),
        hooks: {
          afterSnapshots: () => {
            ihep.set("latest.json", `${JSON.stringify(manifest("2.1.0"))}\n`);
          },
        },
      }),
    ).rejects.toBeInstanceOf(ConditionalWriteError);

    expect([r2.latestPutAttempts, ihep.latestPutAttempts]).toEqual([0, 0]);
    expect(JSON.parse(r2.body("latest.json")).version).toBe("2.0.0");
    expect(JSON.parse(ihep.body("latest.json")).version).toBe("2.1.0");
  });

  it("uses R2 CAS so a concurrent newer release wins without mixed latest pointers", async () => {
    const root = await tempRoot("concurrent");
    const initial = `${JSON.stringify(manifest("1.0.0"))}\n`;
    const backends = [
      new MemoryBackend("r2", { "latest.json": initial }),
      new MemoryBackend("ihep", { "latest.json": initial }),
    ];
    const older = manifest("1.1.0");
    const newer = manifest("1.2.0");
    const olderPath = await writeManifest(root, "older.json", older);
    const newerPath = await writeManifest(root, "newer.json", newer);
    let releaseOlderSnapshots;
    let olderSnapshotsReached;
    const holdOlder = new Promise((resolvePromise) => {
      releaseOlderSnapshots = resolvePromise;
    });
    const olderReady = new Promise((resolvePromise) => {
      olderSnapshotsReached = resolvePromise;
    });

    const olderRun = promoteCandidateTransaction({
      backends,
      candidateManifest: older,
      candidatePath: olderPath,
      override: overrideOff(),
      summary: summaryFor(backends),
      workDir: join(root, "older-transaction"),
      hooks: {
        afterSnapshots: async () => {
          olderSnapshotsReached();
          await holdOlder;
        },
      },
    });
    await olderReady;

    await promoteCandidateTransaction({
      backends,
      candidateManifest: newer,
      candidatePath: newerPath,
      override: overrideOff(),
      summary: summaryFor(backends),
      workDir: join(root, "newer-transaction"),
    });
    releaseOlderSnapshots();

    await expect(olderRun).rejects.toBeInstanceOf(ConditionalWriteError);
    expect(backends.map((backend) => JSON.parse(backend.body("latest.json")).version)).toEqual([
      "1.2.0",
      "1.2.0",
    ]);
  });

  it("repairs IHEP when an older writer is superseded after its pre-write ownership check", async () => {
    const root = await tempRoot("cross-version-follower-race");
    const initial = `${JSON.stringify(manifest("1.0.0"))}\n`;
    const r2 = new MemoryBackend("r2", { "latest.json": initial });
    const ihep = new ConditionIgnoringMemoryBackend("ihep", { "latest.json": initial });
    const backends = [r2, ihep];
    const older = manifest("2.0.0");
    const newer = manifest("3.0.0");
    const olderPath = await writeManifest(root, "older.json", older);
    const newerPath = await writeManifest(root, "newer.json", newer);
    const olderSummary = summaryFor(backends);
    let releaseOlderFollower;
    let olderAtFollower;
    const holdOlderFollower = new Promise((resolvePromise) => {
      releaseOlderFollower = resolvePromise;
    });
    const olderFollowerReady = new Promise((resolvePromise) => {
      olderAtFollower = resolvePromise;
    });

    const olderRun = promoteCandidateTransaction({
      backends,
      candidateManifest: older,
      candidatePath: olderPath,
      override: overrideOff(),
      promotionToken: "older-writer",
      summary: olderSummary,
      workDir: join(root, "older-transaction"),
      hooks: {
        beforeWrite: async (state) => {
          if (state.backend.name === "ihep") {
            olderAtFollower();
            await holdOlderFollower;
          }
        },
      },
    });
    await olderFollowerReady;

    await promoteCandidateTransaction({
      backends,
      candidateManifest: newer,
      candidatePath: newerPath,
      override: overrideOff(),
      promotionToken: "newer-writer",
      summary: summaryFor(backends),
      workDir: join(root, "newer-transaction"),
    });
    releaseOlderFollower();

    await expect(olderRun).rejects.toBeInstanceOf(ConditionalWriteError);
    expect(JSON.parse(r2.body("latest.json")).version).toBe("3.0.0");
    expect(JSON.parse(ihep.body("latest.json")).version).toBe("3.0.0");
    expect(ihep.conditionalLatestPutAttempts).toBe(0);
    expect(ihep.unconditionalLatestPutAttempts).toBe(3);
    expect(olderSummary.backends.find((backend) => backend.name === "ihep").supersession).toBe(
      "repaired-to-r2",
    );
  });

  it("keeps B's R2 v3 CAS when A repairs B's changed follower before B resumes", async () => {
    const root = await tempRoot("post-cas-follower-repaired-by-older-writer");
    const initial = `${JSON.stringify(manifest("1.0.0"))}\n`;
    const r2 = new MemoryBackend("r2", { "latest.json": initial });
    const ihep = new ConditionIgnoringMemoryBackend("ihep", { "latest.json": initial });
    const backends = [r2, ihep];
    const aManifest = manifest("2.0.0");
    const bManifest = manifest("3.0.0");
    const aPath = await writeManifest(root, "a-v2.json", aManifest);
    const bPath = await writeManifest(root, "b-v3.json", bManifest);
    const bSummary = summaryFor(backends);

    let releaseA;
    let signalAReady;
    const holdA = new Promise((resolvePromise) => {
      releaseA = resolvePromise;
    });
    const aReady = new Promise((resolvePromise) => {
      signalAReady = resolvePromise;
    });
    const aRun = promoteCandidateTransaction({
      backends,
      candidateManifest: aManifest,
      candidatePath: aPath,
      override: overrideOff(),
      promotionToken: "writer-a-v2",
      summary: summaryFor(backends),
      workDir: join(root, "a-transaction"),
      hooks: {
        beforeWrite: async (state) => {
          if (state.backend.name === "ihep") {
            signalAReady();
            await holdA;
          }
        },
      },
    });
    await aReady;

    let releaseB;
    let signalBCasCommitted;
    const holdB = new Promise((resolvePromise) => {
      releaseB = resolvePromise;
    });
    const bCasCommitted = new Promise((resolvePromise) => {
      signalBCasCommitted = resolvePromise;
    });
    const bRun = promoteCandidateTransaction({
      backends,
      candidateManifest: bManifest,
      candidatePath: bPath,
      override: overrideOff(),
      promotionToken: "writer-b-v3",
      summary: bSummary,
      workDir: join(root, "b-transaction"),
      hooks: {
        afterWrite: async (state) => {
          if (state.backend.name === "r2") {
            signalBCasCommitted();
            await holdB;
          }
        },
      },
    });
    await bCasCommitted;

    releaseA();
    await expect(aRun).rejects.toBeInstanceOf(ConditionalWriteError);
    expect(JSON.parse(ihep.body("latest.json")).version).toBe("3.0.0");

    releaseB();
    await expect(bRun).resolves.toEqual(expect.objectContaining({ outcome: "promoted" }));

    expect(JSON.parse(r2.body("latest.json")).version).toBe("3.0.0");
    expect(JSON.parse(ihep.body("latest.json")).version).toBe("3.0.0");
    expect(r2.conditionalLatestPutAttempts).toBe(2);
    expect(ihep.conditionalLatestPutAttempts).toBe(0);
    expect(ihep.unconditionalLatestPutAttempts).toBe(2);
    expect(bSummary.backends.find((backend) => backend.name === "r2").rollback).toBe(
      "not-needed",
    );
    expect(bSummary.backends.find((backend) => backend.name === "ihep").promotion).toBe(
      "already-current-after-authority-cas",
    );
  });

  it("fails closed on a higher post-CAS follower without rolling R2 back", async () => {
    const root = await tempRoot("post-cas-safe-successor");
    const candidate = manifest("2.0.0");
    const successor = manifest("3.0.0");
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const initial = `${JSON.stringify(manifest("1.0.0"))}\n`;
    const r2 = new MemoryBackend("r2", { "latest.json": initial });
    const ihep = new ConditionIgnoringMemoryBackend("ihep", { "latest.json": initial });
    const backends = [r2, ihep];
    const summary = summaryFor(backends);

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        promotionToken: "candidate-v2",
        summary,
        workDir: join(root, "transaction"),
        hooks: {
          afterWrite: (state) => {
            if (state.backend.name === "r2") {
              ihep.set(
                "latest.json",
                `${JSON.stringify(successor)}\n`,
                { "cam-promotion-token": "successor-v3" },
              );
            }
          },
        },
      }),
    ).rejects.toBeInstanceOf(ConditionalWriteError);

    expect(JSON.parse(r2.body("latest.json")).version).toBe("2.0.0");
    expect(JSON.parse(ihep.body("latest.json")).version).toBe("3.0.0");
    expect(r2.conditionalLatestPutAttempts).toBe(1);
    expect(ihep.unconditionalLatestPutAttempts).toBe(0);
    expect(summary.authorityPreserved).toBe(true);
    expect(summary.backends.find((backend) => backend.name === "r2").rollback).toBe(
      "preserved-authoritative-cas",
    );
    expect(summary.backends.find((backend) => backend.name === "ihep").promotion).toBe(
      "conflict-after-authority-cas",
    );
  });

  it("fails closed on an unknown post-CAS identity without rolling R2 back", async () => {
    const root = await tempRoot("post-cas-unknown-identity");
    const candidate = manifest("2.0.0");
    const unknown = structuredClone(candidate);
    unknown.platforms["windows-x86_64"].signature = "unknown-signature";
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const initial = `${JSON.stringify(manifest("1.0.0"))}\n`;
    const r2 = new MemoryBackend("r2", { "latest.json": initial });
    const ihep = new ConditionIgnoringMemoryBackend("ihep", { "latest.json": initial });
    const backends = [r2, ihep];
    const summary = summaryFor(backends);

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        promotionToken: "candidate-v2",
        summary,
        workDir: join(root, "transaction"),
        hooks: {
          afterWrite: (state) => {
            if (state.backend.name === "r2") {
              ihep.set(
                "latest.json",
                `${JSON.stringify(unknown)}\n`,
                { "cam-promotion-token": "unknown-v2" },
              );
            }
          },
        },
      }),
    ).rejects.toBeInstanceOf(ConditionalWriteError);

    expect(JSON.parse(r2.body("latest.json")).version).toBe("2.0.0");
    expect(JSON.parse(ihep.body("latest.json")).platforms["windows-x86_64"].signature).toBe(
      "unknown-signature",
    );
    expect(r2.conditionalLatestPutAttempts).toBe(1);
    expect(ihep.unconditionalLatestPutAttempts).toBe(0);
    expect(summary.authorityPreserved).toBe(true);
  });

  it("fails closed when the final follower becomes higher after this run writes it", async () => {
    const root = await tempRoot("final-follower-safe-successor");
    const candidate = manifest("2.0.0");
    const successor = manifest("3.0.0");
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const initial = `${JSON.stringify(manifest("1.0.0"))}\n`;
    const r2 = new MemoryBackend("r2", { "latest.json": initial });
    const ihep = new ConditionIgnoringMemoryBackend("ihep", { "latest.json": initial });
    const backends = [r2, ihep];
    const summary = summaryFor(backends);

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        promotionToken: "candidate-v2",
        summary,
        workDir: join(root, "transaction"),
        hooks: {
          afterWrite: (state) => {
            if (state.backend.name === "ihep") {
              ihep.set(
                "latest.json",
                `${JSON.stringify(successor)}\n`,
                { "cam-promotion-token": "successor-v3" },
              );
            }
          },
        },
      }),
    ).rejects.toBeInstanceOf(ConditionalWriteError);

    expect(JSON.parse(r2.body("latest.json")).version).toBe("2.0.0");
    expect(JSON.parse(ihep.body("latest.json")).version).toBe("3.0.0");
    expect(r2.conditionalLatestPutAttempts).toBe(1);
    expect(ihep.unconditionalLatestPutAttempts).toBe(1);
    expect(summary.authorityPreserved).toBe(true);
    expect(summary.backends.find((backend) => backend.name === "r2").rollback).toBe(
      "preserved-authoritative-cas",
    );
  });

  it("reclaims R2 CAS and heals a hard-terminated R2-only promotion", async () => {
    const root = await tempRoot("partial-terminated-transaction");
    const current = manifest("1.0.0");
    const candidate = manifest("1.1.0");
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const initial = `${JSON.stringify(current)}\n`;
    const r2 = new MemoryBackend("r2", { "latest.json": initial });
    const ihep = new MemoryBackend("ihep", { "latest.json": initial });
    // Model SIGKILL/OOM after the first conditional write committed but before
    // the process could enter its rollback handler.
    const r2Previous = await r2.head("latest.json");
    await r2.putLatestConditional(candidatePath, r2Previous.etag, "terminated-run");
    const r2AttemptsAfterTermination = r2.latestPutAttempts;
    const backends = [r2, ihep];
    const failedResumeSummary = summaryFor(backends);
    const realIhepPut = ihep.putLatestUnconditional.bind(ihep);
    let failFirstResume = true;
    ihep.putLatestUnconditional = async (...args) => {
      if (failFirstResume) {
        failFirstResume = false;
        ihep.latestPutAttempts += 1;
        ihep.unconditionalLatestPutAttempts += 1;
        throw new Error("ihep: simulated follower outage");
      }
      return await realIhepPut(...args);
    };

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        promotionToken: "this-run",
        summary: failedResumeSummary,
        workDir: join(root, "failed-resume"),
      }),
    ).rejects.toThrow("simulated follower outage");

    expect(r2.latestPutAttempts).toBe(r2AttemptsAfterTermination + 2);
    expect(backends.map((backend) => JSON.parse(backend.body("latest.json")).version)).toEqual([
      "1.1.0",
      "1.0.0",
    ]);
    expect(failedResumeSummary.rollback).toEqual(
      expect.objectContaining({ attempted: true, complete: true }),
    );

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        promotionToken: "fresh-run",
        summary: summaryFor(backends),
        workDir: join(root, "successful-resume"),
      }),
    ).resolves.toEqual(expect.objectContaining({ outcome: "promoted" }));

    expect(r2.latestPutAttempts).toBe(r2AttemptsAfterTermination + 3);
    expect(ihep.latestPutAttempts).toBe(2);
    expect(backends.map((backend) => JSON.parse(backend.body("latest.json")).version)).toEqual([
      "1.1.0",
      "1.1.0",
    ]);
    expect(r2.objects.get("latest.json").metadata["cam-promotion-token"]).toBe(
      "fresh-run",
    );
    expect(ihep.objects.get("latest.json").metadata["cam-promotion-token"]).toBe("fresh-run");
  });

  it("lets a newer rerun converge after a hard-terminated R2-only promotion", async () => {
    const root = await tempRoot("newer-after-hard-termination");
    const previous = manifest("1.0.0");
    const interrupted = manifest("1.1.0");
    const newer = manifest("1.2.0");
    const interruptedPath = await writeManifest(root, "interrupted.json", interrupted);
    const newerPath = await writeManifest(root, "newer.json", newer);
    const initial = `${JSON.stringify(previous)}\n`;
    const r2 = new MemoryBackend("r2", { "latest.json": initial });
    const ihep = new ConditionIgnoringMemoryBackend("ihep", { "latest.json": initial });
    const before = await r2.head("latest.json");
    await r2.putLatestConditional(interruptedPath, before.etag, "terminated-run");
    const backends = [r2, ihep];

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: newer,
        candidatePath: newerPath,
        override: overrideOff(),
        promotionToken: "newer-rerun",
        summary: summaryFor(backends),
        workDir: join(root, "transaction"),
      }),
    ).resolves.toEqual(expect.objectContaining({ outcome: "promoted" }));

    expect(JSON.parse(r2.body("latest.json")).version).toBe("1.2.0");
    expect(JSON.parse(ihep.body("latest.json")).version).toBe("1.2.0");
    expect(ihep.conditionalLatestPutAttempts).toBe(0);
    expect(ihep.unconditionalLatestPutAttempts).toBe(1);
  });

  it("fails closed before writing when either backend has no baseline latest.json", async () => {
    const root = await tempRoot("unseeded-backend");
    const candidate = manifest("1.1.0");
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const r2 = new MemoryBackend("r2", {
      "latest.json": `${JSON.stringify(manifest("1.0.0"))}\n`,
    });
    const ihep = new MemoryBackend("ihep");
    const backends = [r2, ihep];

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        summary: summaryFor(backends),
        workDir: join(root, "transaction"),
      }),
    ).rejects.toThrow("seed both backends");

    expect(backends.map((backend) => backend.latestPutAttempts)).toEqual([0, 0]);
    expect(ihep.body("latest.json")).toBeUndefined();
  });

  it("CAS-rolls R2 back and preserves IHEP when the follower fails before writing", async () => {
    const root = await tempRoot("follower-failure-before-write");
    const candidate = manifest("1.1.0");
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const initial = Buffer.from(`${JSON.stringify(manifest("1.0.0"))}\n`);
    const r2 = new MemoryBackend("r2", { "latest.json": initial });
    const ihep = new ConditionIgnoringMemoryBackend("ihep", { "latest.json": initial });
    ihep.failLatestPut = true;
    const backends = [r2, ihep];
    const summary = summaryFor(backends);

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        promotionToken: "failed-follower",
        summary,
        workDir: join(root, "transaction"),
      }),
    ).rejects.toThrow("simulated write failure");

    expect(r2.body("latest.json").equals(initial)).toBe(true);
    expect(ihep.body("latest.json").equals(initial)).toBe(true);
    expect(r2.conditionalLatestPutAttempts).toBe(2);
    expect(ihep.conditionalLatestPutAttempts).toBe(0);
    expect(ihep.unconditionalLatestPutAttempts).toBe(1);
    expect(summary.backends.find((backend) => backend.name === "r2").rollback).toBe("restored");
    expect(summary.backends.find((backend) => backend.name === "ihep").rollback).toBe(
      "preserved-previous",
    );
  });

  it("restores only its own IHEP follower write after a later local failure", async () => {
    const root = await tempRoot("follower-failure-after-write");
    const candidate = manifest("1.1.0");
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const initial = Buffer.from(`${JSON.stringify(manifest("1.0.0"))}\n`);
    const r2 = new MemoryBackend("r2", { "latest.json": initial });
    const ihep = new ConditionIgnoringMemoryBackend("ihep", { "latest.json": initial });
    const backends = [r2, ihep];
    const summary = summaryFor(backends);

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        promotionToken: "restore-follower",
        summary,
        workDir: join(root, "transaction"),
        hooks: {
          afterWrite: (state) => {
            if (state.backend.name === "ihep") throw new Error("simulated post-write failure");
          },
        },
      }),
    ).rejects.toThrow("simulated post-write failure");

    expect(r2.body("latest.json").equals(initial)).toBe(true);
    expect(ihep.body("latest.json").equals(initial)).toBe(true);
    expect(ihep.conditionalLatestPutAttempts).toBe(0);
    expect(ihep.unconditionalLatestPutAttempts).toBe(2);
    expect(summary.backends.find((backend) => backend.name === "ihep").rollback).toBe(
      "restored-unconditionally",
    );
  });

  it("does not roll R2 back when IHEP fails after a newer authority takes ownership", async () => {
    const root = await tempRoot("follower-failure-authority-lost");
    const candidate = manifest("1.1.0");
    const successor = manifest("1.2.0");
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const initial = `${JSON.stringify(manifest("1.0.0"))}\n`;
    const r2 = new MemoryBackend("r2", { "latest.json": initial });
    const ihep = new ConditionIgnoringMemoryBackend("ihep", { "latest.json": initial });
    ihep.putLatestUnconditional = async () => {
      ihep.latestPutAttempts += 1;
      ihep.unconditionalLatestPutAttempts += 1;
      r2.set(
        "latest.json",
        `${JSON.stringify(successor)}\n`,
        { "cam-promotion-token": "newer-authority" },
      );
      throw new Error("simulated IHEP outage after R2 supersession");
    };
    const backends = [r2, ihep];
    const summary = summaryFor(backends);

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        promotionToken: "older-authority",
        summary,
        workDir: join(root, "transaction"),
      }),
    ).rejects.toThrow("rollback incomplete");

    expect(JSON.parse(r2.body("latest.json")).version).toBe("1.2.0");
    expect(JSON.parse(ihep.body("latest.json")).version).toBe("1.0.0");
    expect(r2.conditionalLatestPutAttempts).toBe(1);
    expect(ihep.conditionalLatestPutAttempts).toBe(0);
    expect(summary.backends.find((backend) => backend.name === "r2").rollback).toBe(
      "skipped-concurrent-change",
    );
  });

  it("preserves a concurrent newer IHEP value while rolling back owned R2", async () => {
    const root = await tempRoot("preserve-concurrent-follower");
    const candidate = manifest("1.1.0");
    const followerSuccessor = manifest("1.2.0");
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const initial = `${JSON.stringify(manifest("1.0.0"))}\n`;
    const r2 = new MemoryBackend("r2", { "latest.json": initial });
    const ihep = new ConditionIgnoringMemoryBackend("ihep", { "latest.json": initial });
    const backends = [r2, ihep];
    const summary = summaryFor(backends);

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        promotionToken: "owned-follower",
        summary,
        workDir: join(root, "transaction"),
        hooks: {
          afterWrite: (state) => {
            if (state.backend.name === "ihep") {
              ihep.set(
                "latest.json",
                `${JSON.stringify(followerSuccessor)}\n`,
                { "cam-promotion-token": "newer-follower" },
              );
              throw new Error("simulated failure after concurrent IHEP advance");
            }
          },
        },
      }),
    ).rejects.toThrow("simulated failure after concurrent IHEP advance");

    expect(JSON.parse(r2.body("latest.json")).version).toBe("1.0.0");
    expect(JSON.parse(ihep.body("latest.json")).version).toBe("1.2.0");
    expect(summary.backends.find((backend) => backend.name === "r2").rollback).toBe("restored");
    expect(summary.backends.find((backend) => backend.name === "ihep").rollback).toBe(
      "preserved-concurrent-change",
    );
  });

  it("never rolls back an ETag that a concurrent writer owns", async () => {
    const root = await tempRoot("etag-ownership");
    const current = manifest("1.0.0");
    const candidate = manifest("1.1.0");
    const external = manifest("1.2.0");
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const r2 = new MemoryBackend("r2", {
      "latest.json": `${JSON.stringify(current)}\n`,
    });
    const ihep = new MemoryBackend("ihep", {
      "latest.json": `${JSON.stringify(current)}\n`,
    });
    const realPut = r2.putLatestConditional.bind(r2);
    r2.putLatestConditional = async (...args) => {
      const committed = await realPut(...args);
      r2.set(
        "latest.json",
        `${JSON.stringify(external)}\n`,
        { "cam-promotion-token": "external-run" },
      );
      return committed;
    };
    const backends = [r2, ihep];
    const summary = summaryFor(backends);

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        promotionToken: "this-run",
        summary,
        workDir: join(root, "transaction"),
      }),
    ).rejects.toThrow("rollback incomplete");

    expect(JSON.parse(r2.body("latest.json")).version).toBe("1.2.0");
    expect(JSON.parse(ihep.body("latest.json")).version).toBe("1.0.0");
    expect(summary.backends.find((backend) => backend.name === "r2").rollback).toBe(
      "skipped-concurrent-change",
    );
  });

  it("observes and rolls back a committed write whose process response was lost", async () => {
    const root = await tempRoot("ambiguous-commit");
    const current = manifest("1.0.0");
    const candidate = manifest("1.1.0");
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const initial = Buffer.from(`${JSON.stringify(current)}\n`);
    const r2 = new MemoryBackend("r2", { "latest.json": initial });
    const ihep = new MemoryBackend("ihep", { "latest.json": initial });
    const realPut = r2.putLatestConditional.bind(r2);
    let firstWrite = true;
    r2.putLatestConditional = async (localPath, expectedEtag, promotionToken) => {
      if (!firstWrite) return await realPut(localPath, expectedEtag, promotionToken);
      firstWrite = false;
      r2.latestPutAttempts += 1;
      const currentObject = r2.objects.get("latest.json");
      if (currentObject?.etag !== expectedEtag) {
        throw new ConditionalWriteError("r2: stale latest ETag");
      }
      r2.set(
        "latest.json",
        await readFile(localPath),
        { "cam-promotion-token": promotionToken },
      );
      throw new Error("simulated process termination after server commit");
    };
    const backends = [r2, ihep];
    const summary = summaryFor(backends);

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        promotionToken: "interrupted-run",
        summary,
        workDir: join(root, "transaction"),
      }),
    ).rejects.toThrow("simulated process termination");

    expect(backends.every((backend) => backend.body("latest.json").equals(initial))).toBe(true);
    expect(summary.rollback).toEqual(
      expect.objectContaining({ attempted: true, complete: true }),
    );
  });

  it("rolls back the first backend when execution is interrupted between writes", async () => {
    const root = await tempRoot("interrupted");
    const current = manifest("1.0.0");
    const candidate = manifest("1.1.0");
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const initial = Buffer.from(`${JSON.stringify(current)}\n`);
    const backends = [
      new MemoryBackend("r2", { "latest.json": initial }),
      new MemoryBackend("ihep", { "latest.json": initial }),
    ];
    const summary = summaryFor(backends);

    await expect(
      promoteCandidateTransaction({
        backends,
        candidateManifest: candidate,
        candidatePath,
        override: overrideOff(),
        summary,
        workDir: join(root, "transaction"),
        hooks: {
          afterWrite: (state) => {
            if (state.backend.name === "r2") throw new Error("simulated SIGTERM");
          },
        },
      }),
    ).rejects.toThrow("simulated SIGTERM");

    expect(backends.every((backend) => backend.body("latest.json").equals(initial))).toBe(true);
    expect(summary.backends.find((backend) => backend.name === "r2").rollback).toBe("restored");
    expect(summary.backends.find((backend) => backend.name === "ihep").promotion).toBe(
      "not-started",
    );
  });

  it("does not write latest when one backend candidate verification fails", async () => {
    const root = await tempRoot("one-backend-fails");
    const dist = join(root, "dist");
    await mkdir(dist, { recursive: true });
    const artifact = Buffer.from("valid release artifact");
    const fixture = updaterFixture(artifact);
    const candidate = completeManifest("1.1.0", fixture.signature);
    for (const entry of Object.values(candidate.platforms)) {
      entry.sha256 = hash(artifact);
    }
    const artifactNames = Object.values(candidate.platforms).map((entry) =>
      new URL(entry.url).pathname.split("/").at(-1),
    );
    for (const name of artifactNames) {
      await writeFile(join(dist, name), artifact);
      await writeFile(join(dist, `${name}.sig`), `${fixture.signature}\n`);
    }
    const releaseIdentity = await writeReleaseIdentity(dist, candidate, fixture);
    const candidatePath = await writeManifest(dist, "latest.mirror.json", candidate);
    const candidateKey = candidateKeyFor("1.1.0", "run-2");
    const current = Buffer.from(`${JSON.stringify(manifest("1.0.0"))}\n`);
    const stagedObjects = Object.fromEntries(
      artifactNames.flatMap((name) => [
        [`1.1.0/${name}`, artifact],
        [`1.1.0/${name}.sig`, Buffer.from(`${fixture.signature}\n`)],
      ]),
    );
    stagedObjects["1.1.0/release-identity.json"] = releaseIdentity.identityBytes;
    stagedObjects["1.1.0/release-identity.json.sig"] = await readFile(
      releaseIdentity.signaturePath,
    );
    const r2 = new MemoryBackend("r2", {
      "latest.json": current,
      [candidateKey]: await readFile(candidatePath),
      ...stagedObjects,
    });
    const missingArtifact = artifactNames.at(-1);
    const ihepObjects = { ...stagedObjects };
    delete ihepObjects[`1.1.0/${missingArtifact}`];
    const ihep = new MemoryBackend("ihep", {
      "latest.json": current,
      [candidateKey]: await readFile(candidatePath),
      // Deliberately missing one required platform artifact.
      ...ihepObjects,
    });
    const promotionToken = hash(Buffer.from(candidateKey));
    r2.objects.get(candidateKey).metadata["cam-promotion-token"] = promotionToken;
    ihep.objects.get(candidateKey).metadata["cam-promotion-token"] = promotionToken;
    const summaryPath = join(root, "promotion-summary.json");

    await expect(
      promoteMirrors({
        backends: [r2, ihep],
        candidateKey,
        candidateManifest: candidate,
        candidatePath,
        distDir: dist,
        mirrorBase: "https://mirror.example/manager",
        override: overrideOff(),
        publicKey: fixture.publicKey,
        summaryPath,
        tempRoot: join(root, "promote"),
      }),
    ).rejects.toThrow("ihep: artifact is missing");

    expect([r2.latestPutAttempts, ihep.latestPutAttempts]).toEqual([0, 0]);
    const audit = JSON.parse(await readFile(summaryPath, "utf8"));
    expect(audit.outcome).toBe("failed");
    expect(audit.backends.map((backend) => backend.candidateVerification)).toEqual([
      "passed",
      "failed",
    ]);
  });

  it("requires a workflow-dispatch audit trail for emergency downgrade override", () => {
    expect(() =>
      downgradeOverrideFromEnv({
        MIRROR_ALLOW_DOWNGRADE: "1",
        MIRROR_DOWNGRADE_REASON: "urgent rollback after launch regression",
        GITHUB_EVENT_NAME: "push",
        GITHUB_ACTOR: "release-admin",
        GITHUB_REPOSITORY: "owner/repo",
        GITHUB_RUN_ID: "42",
      }),
    ).toThrow("only from workflow_dispatch");

    expect(() =>
      downgradeOverrideFromEnv({
        MIRROR_ALLOW_DOWNGRADE: "1",
        MIRROR_DEFAULT_BRANCH: "main",
        MIRROR_DOWNGRADE_REASON: "urgent rollback after launch regression",
        MIRROR_WORKFLOW_REF_NAME: "release-experiment",
        GITHUB_EVENT_NAME: "workflow_dispatch",
        GITHUB_ACTOR: "release-admin",
        GITHUB_REPOSITORY: "owner/repo",
        GITHUB_RUN_ID: "42",
      }),
    ).toThrow("default branch");

    const override = downgradeOverrideFromEnv({
      MIRROR_ALLOW_DOWNGRADE: "1",
      MIRROR_DEFAULT_BRANCH: "main",
      MIRROR_DOWNGRADE_REASON: "urgent rollback after launch regression",
      MIRROR_WORKFLOW_REF_NAME: "main",
      GITHUB_EVENT_NAME: "workflow_dispatch",
      GITHUB_ACTOR: "release-admin",
      GITHUB_TRIGGERING_ACTOR: "incident-commander",
      GITHUB_REPOSITORY: "owner/repo",
      GITHUB_RUN_ID: "42",
    });
    expect(override).toEqual(
      expect.objectContaining({
        actor: "incident-commander",
        originalActor: "release-admin",
        reason: "urgent rollback after launch regression",
        requested: true,
        runUrl: "https://github.com/owner/repo/actions/runs/42",
      }),
    );
  });

  it("uses an audited override to downgrade both backends consistently", async () => {
    const root = await tempRoot("audited-downgrade");
    const current = manifest("3.0.0");
    const candidate = manifest("2.5.0");
    const candidatePath = await writeManifest(root, "candidate.json", candidate);
    const initial = `${JSON.stringify(current)}\n`;
    const backends = [
      new MemoryBackend("r2", { "latest.json": initial }),
      new MemoryBackend("ihep", { "latest.json": initial }),
    ];
    const override = {
      actor: "release-admin",
      eventName: "workflow_dispatch",
      reason: "rollback production crash regression",
      requested: true,
      runUrl: "https://github.com/owner/repo/actions/runs/42",
      used: false,
    };
    const summary = summaryFor(backends, override);

    const result = await promoteCandidateTransaction({
      backends,
      candidateManifest: candidate,
      candidatePath,
      override,
      summary,
      workDir: join(root, "transaction"),
    });

    expect(result.outcome).toBe("downgrade-override-promoted");
    expect(summary.override.used).toBe(true);
    expect(backends.map((backend) => JSON.parse(backend.body("latest.json")).version)).toEqual([
      "2.5.0",
      "2.5.0",
    ]);
  });
});

describe("mirror manifest rewriting", () => {
  it("uses an immutable version path and a per-run candidate key", async () => {
    const root = await tempRoot("manifest-rewrite");
    const dist = join(root, "dist");
    await mkdir(dist, { recursive: true });
    const source = manifest("3.4.5");
    source.platforms["windows-x86_64"].url =
      "https://github.com/owner/repo/releases/download/v3.4.5/manager-3.4.5.exe";
    await writeManifest(dist, "latest.json", source);

    const result = await createMirrorManifest(dist, "https://mirror.example/manager");

    expect(result.manifest.platforms["windows-x86_64"].url).toBe(
      "https://mirror.example/manager/3.4.5/manager-3.4.5.exe",
    );
    expect(candidateKeyFor("3.4.5", "987-2")).toBe("candidates/3.4.5/987-2.json");
  });
});

describe("release audit summary", () => {
  it("renders per-backend verification, monotonic decisions, rollback, and override audit", async () => {
    const root = await tempRoot("release-summary");
    const summaryPath = join(root, "github-summary.md");
    await writeManifest(root, "latest.json", manifest("4.0.0"));
    await writeManifest(root, "mirror-stage-summary.json", {
      candidateKey: "candidates/4.0.0/42-1.json",
      candidateVersion: "4.0.0",
      outcome: "staged",
    });
    await writeManifest(root, "mirror-verification-summary.json", {
      candidateKey: "candidates/4.0.0/42-1.json",
      candidateVersion: "4.0.0",
      outcome: "verified",
      publicRouteVerification: "passed",
    });
    await writeManifest(root, "mirror-promotion-summary.json", {
      candidateKey: "candidates/4.0.0/42-1.json",
      candidateVersion: "4.0.0",
      outcome: "downgrade-override-promoted",
      rootIdentity: {
        latestTransactionOutcome: "downgrade-override-promoted",
        localVerification: "passed",
        publicRouteVerification: "passed",
      },
      override: {
        actor: "incident-commander",
        originalActor: "release-admin",
        reason: "rollback broken production release",
        requested: true,
        runUrl: "https://github.com/owner/repo/actions/runs/42",
        used: true,
      },
      backends: [
        {
          name: "r2",
          candidateVerification: "passed",
          currentVersion: "5.0.0",
          decision: "promote-downgrade-override",
          promotion: "verified",
          rootIdentity: {
            directVerification: "passed",
            jsonWrite: "written-and-verified",
            publicVerification: "passed",
            signatureWrite: "written-and-verified",
          },
          rollback: "not-needed",
          finalVersion: "4.0.0",
        },
        {
          name: "ihep",
          candidateVerification: "passed",
          currentVersion: "5.0.0",
          decision: "promote-downgrade-override",
          promotion: "verified",
          rootIdentity: {
            directVerification: "passed",
            jsonWrite: "written-and-verified",
            publicVerification: "passed",
            signatureWrite: "written-and-verified",
          },
          rollback: "not-needed",
          finalVersion: "4.0.0",
        },
      ],
    });

    await execFileAsync(process.execPath, [join(process.cwd(), "scripts/write-release-summary.mjs")], {
      cwd: root,
      env: {
        ...process.env,
        GITHUB_REF_NAME: "v4.0.0",
        GITHUB_REPOSITORY: "owner/repo",
        GITHUB_STEP_SUMMARY: summaryPath,
      },
    });

    const rendered = await readFile(summaryPath, "utf8");
    expect(rendered).toContain("Promotion outcome: **downgrade-override-promoted**");
    expect(rendered).toContain("Pre-publish verification: **verified**");
    expect(rendered).toContain("Public route verification: **passed**");
    expect(rendered).toContain(
      "Stable root identity: local=**passed**, latest transaction=**downgrade-override-promoted**, public readback=**passed**",
    );
    expect(rendered).toContain("| r2 | passed | 5.0.0 | promote-downgrade-override | verified");
    expect(rendered).toContain("rollback broken production release");
    expect(rendered).toContain("by `incident-commander` (original workflow actor: `release-admin`)");
    expect(rendered).toContain("https://github.com/owner/repo/actions/runs/42");
  });

  it("reports a committed latest transaction as retryable when root identity readback fails", async () => {
    const root = await tempRoot("release-summary-root-retry");
    const summaryPath = join(root, "github-summary.md");
    await writeManifest(root, "latest.json", manifest("4.0.0"));
    await writeManifest(root, "mirror-promotion-summary.json", {
      candidateKey: "candidates/4.0.0/42-1.json",
      candidateVersion: "4.0.0",
      error: "public identity readback mismatch",
      outcome: "failed",
      rootIdentity: {
        latestTransactionOutcome: "promoted",
        localVerification: "passed",
        publicRouteVerification: "failed",
      },
      backends: [],
    });

    await execFileAsync(process.execPath, [join(process.cwd(), "scripts/write-release-summary.mjs")], {
      cwd: root,
      env: {
        ...process.env,
        GITHUB_REF_NAME: "v4.0.0",
        GITHUB_REPOSITORY: "owner/repo",
        GITHUB_STEP_SUMMARY: summaryPath,
      },
    });

    const rendered = await readFile(summaryPath, "utf8");
    expect(rendered).toContain("**Retry required:** latest.json completed its monotonic transaction");
    expect(rendered).toContain("clients, which fall back to GitHub");
    expect(rendered).not.toContain("left no owned mirror-pointer advance");
  });
});
