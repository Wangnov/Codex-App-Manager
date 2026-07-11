import { describe, expect, it } from "vitest";
import { verifyTauriMinisign } from "./minisign-verify.mjs";
import {
  reuseGithubReleaseIdentitySignature,
  reuseReleaseIdentitySignature,
} from "./reuse-release-identity-signature.mjs";

const RAW_PUBLIC_KEY =
  "RWQf6LRCGA9i53mlYecO4IzT51TGPpvWucNSCh1CBM0QTaLn73Y7GFO3";
const RAW_SIGNATURE = `untrusted comment: signature from minisign secret key
RUQf6LRCGA9i559r3g7V1qNyJDApGip8MfqcadIgT9CuhV3EMhHoN1mGTkUidF/z7SrlQgXdy8ofjb7bNJJylDOocrCo8KLzZwo=
trusted comment: timestamp:1633700835\tfile:test\tprehashed
wLMDjy9FLAuxZ3q4NlEvkgtyhrr0gtTu6KC4KBJdITbbOeAi1zBIYo0v4iTgt8jJpIidRJnp94ABQkJAgAooBQ==`;
const TAURI_PUBLIC_KEY = Buffer.from(
  `untrusted comment: minisign public key\n${RAW_PUBLIC_KEY}\n`,
).toString("base64");
const TAURI_SIGNATURE = Buffer.from(`${RAW_SIGNATURE}\n`).toString("base64");

const encodePublicKeyEnvelope = (packet) =>
  Buffer.from(`untrusted comment: minisign public key\n${packet}\n`).toString("base64");

const encodeSignatureEnvelope = ({ packet, globalSignature }) =>
  Buffer.from(
    `untrusted comment: signature from minisign secret key\n${packet}\n` +
      `trusted comment: timestamp:1633700835\tfile:test\tprehashed\n${globalSignature}\n`,
  ).toString("base64");

const SIGNATURE_LINES = RAW_SIGNATURE.split("\n");
const RAW_SIGNATURE_PACKET = SIGNATURE_LINES[1];
const RAW_GLOBAL_SIGNATURE = SIGNATURE_LINES[3];

const replaceLastDataCharacter = (value, replacement) =>
  value.replace(/([A-Za-z0-9+/])=+$/, `${replacement}${value.match(/=+$/)?.[0] ?? ""}`);

const response = (body, status = 200) =>
  new Response(body, {
    status,
    headers: body == null ? {} : { "content-length": String(Buffer.byteLength(body)) },
  });

describe("release identity minisign", () => {
  it("verifies the Tauri base64 minisign envelope and rejects changed bytes", () => {
    expect(verifyTauriMinisign(Buffer.from("test"), TAURI_SIGNATURE, TAURI_PUBLIC_KEY)).toBe(true);
    expect(() =>
      verifyTauriMinisign(Buffer.from("tampered"), TAURI_SIGNATURE, TAURI_PUBLIC_KEY),
    ).toThrow(/content signature verification failed/);
  });

  it.each([
    ["public-key prefix", `!${TAURI_PUBLIC_KEY}`, TAURI_SIGNATURE],
    [
      "public-key whitespace",
      `${TAURI_PUBLIC_KEY.slice(0, 12)} ${TAURI_PUBLIC_KEY.slice(12)}`,
      TAURI_SIGNATURE,
    ],
    ["public-key trailing garbage", `${TAURI_PUBLIC_KEY}!`, TAURI_SIGNATURE],
    ["public-key leading whitespace", ` ${TAURI_PUBLIC_KEY}`, TAURI_SIGNATURE],
    ["signature prefix", TAURI_PUBLIC_KEY, `!${TAURI_SIGNATURE}`],
    [
      "signature whitespace",
      TAURI_PUBLIC_KEY,
      `${TAURI_SIGNATURE.slice(0, 12)} ${TAURI_SIGNATURE.slice(12)}`,
    ],
    ["signature trailing garbage", TAURI_PUBLIC_KEY, `${TAURI_SIGNATURE}!`],
    ["signature trailing whitespace", TAURI_PUBLIC_KEY, `${TAURI_SIGNATURE}\n`],
    ["signature missing padding", TAURI_PUBLIC_KEY, TAURI_SIGNATURE.slice(0, -1)],
    [
      "signature non-zero pad bits",
      TAURI_PUBLIC_KEY,
      replaceLastDataCharacter(TAURI_SIGNATURE, "p"),
    ],
  ])("rejects non-canonical outer Tauri base64: %s", (_label, publicKey, signature) => {
    expect(() => verifyTauriMinisign(Buffer.from("test"), signature, publicKey)).toThrow(
      /not canonical base64/,
    );
  });

  it.each([
    ["prefix", `!${RAW_PUBLIC_KEY}`],
    ["whitespace", `${RAW_PUBLIC_KEY.slice(0, 12)} ${RAW_PUBLIC_KEY.slice(12)}`],
    ["trailing garbage", `${RAW_PUBLIC_KEY}!`],
  ])("rejects non-canonical minisign public-key packet base64: %s", (_label, packet) => {
    expect(() =>
      verifyTauriMinisign(Buffer.from("test"), TAURI_SIGNATURE, encodePublicKeyEnvelope(packet)),
    ).toThrow(/minisign public key packet is not canonical base64/);
  });

  it.each([
    ["prefix", `!${RAW_SIGNATURE_PACKET}`],
    ["whitespace", `${RAW_SIGNATURE_PACKET.slice(0, 12)} ${RAW_SIGNATURE_PACKET.slice(12)}`],
    ["trailing garbage", `${RAW_SIGNATURE_PACKET}!`],
    ["non-zero pad bits", replaceLastDataCharacter(RAW_SIGNATURE_PACKET, "p")],
  ])("rejects non-canonical minisign signature-packet base64: %s", (_label, packet) => {
    const signature = encodeSignatureEnvelope({
      packet,
      globalSignature: RAW_GLOBAL_SIGNATURE,
    });
    expect(() => verifyTauriMinisign(Buffer.from("test"), signature, TAURI_PUBLIC_KEY)).toThrow(
      /minisign signature packet is not canonical base64/,
    );
  });

  it.each([
    ["prefix", `!${RAW_GLOBAL_SIGNATURE}`],
    ["whitespace", `${RAW_GLOBAL_SIGNATURE.slice(0, 12)} ${RAW_GLOBAL_SIGNATURE.slice(12)}`],
    ["trailing garbage", `${RAW_GLOBAL_SIGNATURE}!`],
    ["non-zero pad bits", replaceLastDataCharacter(RAW_GLOBAL_SIGNATURE, "R")],
  ])("rejects non-canonical minisign global-signature base64: %s", (_label, globalSignature) => {
    const signature = encodeSignatureEnvelope({
      packet: RAW_SIGNATURE_PACKET,
      globalSignature,
    });
    expect(() => verifyTauriMinisign(Buffer.from("test"), signature, TAURI_PUBLIC_KEY)).toThrow(
      /minisign global signature is not canonical base64/,
    );
  });

  it("reuses the exact first-stage signature on a release rerun", async () => {
    const identityBytes = Buffer.from("test");
    const publishedSignature = Buffer.from(TAURI_SIGNATURE, "utf8");
    const missingFetch = async () => response(null, 404);
    await expect(
      reuseReleaseIdentitySignature({
        identityBytes,
        version: "1.0.0",
        mirrorBase: "https://mirror.example/manager",
        publicKey: TAURI_PUBLIC_KEY,
        fetchImpl: missingFetch,
      }),
    ).resolves.toBeNull();

    const rerunFetch = async (url) =>
      response(url.pathname.endsWith(".sig") ? publishedSignature : identityBytes);
    const reused = await reuseReleaseIdentitySignature({
      identityBytes,
      version: "1.0.0",
      mirrorBase: "https://mirror.example/manager",
      publicKey: TAURI_PUBLIC_KEY,
      fetchImpl: rerunFetch,
    });

    expect(reused).toEqual(publishedSignature);
  });

  it("reuses an already-published GitHub Release signature before consulting mirrors", async () => {
    const identityBytes = Buffer.from("test");
    const calls = [];
    const fetchImpl = async (url) => {
      calls.push(String(url));
      if (String(url).includes("/releases/tags/v1.0.0")) {
        return response(
          JSON.stringify({
            assets: [
              { name: "release-identity.json", url: "https://api.github.test/assets/1" },
              { name: "release-identity.json.sig", url: "https://api.github.test/assets/2" },
            ],
          }),
        );
      }
      if (String(url).endsWith("/assets/1")) return response(identityBytes);
      if (String(url).endsWith("/assets/2")) return response(TAURI_SIGNATURE);
      throw new Error(`unexpected URL ${url}`);
    };

    const reused = await reuseGithubReleaseIdentitySignature({
      identityBytes,
      version: "1.0.0",
      repository: "owner/repo",
      tag: "v1.0.0",
      token: "test-token",
      publicKey: TAURI_PUBLIC_KEY,
      fetchImpl,
    });

    expect(reused).toEqual(Buffer.from(TAURI_SIGNATURE, "utf8"));
    expect(calls).toEqual([
      "https://api.github.com/repos/owner/repo/releases/tags/v1.0.0",
      "https://api.github.test/assets/1",
      "https://api.github.test/assets/2",
    ]);
  });

  it("fails closed when an immutable prior identity differs", async () => {
    const fetchImpl = async (url) =>
      response(url.pathname.endsWith(".sig") ? TAURI_SIGNATURE : "different");
    await expect(
      reuseReleaseIdentitySignature({
        identityBytes: Buffer.from("test"),
        version: "1.0.0",
        mirrorBase: "https://mirror.example/manager",
        publicKey: TAURI_PUBLIC_KEY,
        fetchImpl,
      }),
    ).rejects.toThrow(/immutable release identity differs/);
  });
});
