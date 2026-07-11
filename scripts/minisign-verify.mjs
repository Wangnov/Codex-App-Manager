import { createHash, createPublicKey, timingSafeEqual, verify } from "node:crypto";
import { readFileSync } from "node:fs";
import { pathToFileURL } from "node:url";

const PUBLIC_KEY_PACKET_BYTES = 42;
const SIGNATURE_PACKET_BYTES = 74;
const ED25519_SIGNATURE_BYTES = 64;
const TRUSTED_COMMENT_PREFIX = "trusted comment: ";
const ED25519_SPKI_PREFIX = Buffer.from("302a300506032b6570032100", "hex");
const CANONICAL_BASE64 = /^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/;

const decodeBase64 = (value, label) => {
  const encoded = String(value);
  if (!encoded) throw new Error(`${label} is empty`);
  if (!CANONICAL_BASE64.test(encoded)) {
    throw new Error(`${label} is not canonical base64`);
  }
  const decoded = Buffer.from(encoded, "base64");
  if (decoded.toString("base64") !== encoded) {
    throw new Error(`${label} is not canonical base64`);
  }
  return decoded;
};

const decodeTauriPublicKey = (encodedPublicKey) => {
  const text = decodeBase64(encodedPublicKey, "Tauri updater public key").toString("utf8");
  const lines = text.trim().split(/\r?\n/);
  const packet = decodeBase64(lines.at(-1), "minisign public key packet");
  if (packet.length !== PUBLIC_KEY_PACKET_BYTES) {
    throw new Error("minisign public key packet has an invalid length");
  }
  const algorithm = packet.subarray(0, 2).toString("ascii");
  if (algorithm !== "Ed" && algorithm !== "ED") {
    throw new Error("minisign public key uses an unsupported algorithm");
  }
  return {
    keyId: packet.subarray(2, 10),
    key: createPublicKey({
      key: Buffer.concat([ED25519_SPKI_PREFIX, packet.subarray(10)]),
      format: "der",
      type: "spki",
    }),
  };
};

const decodeTauriSignature = (encodedSignature) => {
  const text = decodeBase64(encodedSignature, "Tauri updater signature").toString("utf8");
  const lines = text.trim().split(/\r?\n/);
  if (lines.length !== 4 || !lines[2].startsWith(TRUSTED_COMMENT_PREFIX)) {
    throw new Error("minisign signature has an invalid four-line envelope");
  }
  const packet = decodeBase64(lines[1], "minisign signature packet");
  const globalSignature = decodeBase64(lines[3], "minisign global signature");
  if (
    packet.length !== SIGNATURE_PACKET_BYTES ||
    globalSignature.length !== ED25519_SIGNATURE_BYTES
  ) {
    throw new Error("minisign signature packet has an invalid length");
  }
  const algorithm = packet.subarray(0, 2).toString("ascii");
  if (algorithm !== "Ed" && algorithm !== "ED") {
    throw new Error("minisign signature uses an unsupported algorithm");
  }
  return {
    algorithm,
    keyId: packet.subarray(2, 10),
    signature: packet.subarray(10),
    trustedComment: lines[2].slice(TRUSTED_COMMENT_PREFIX.length),
    globalSignature,
  };
};

export const verifyTauriMinisign = (bytes, encodedSignature, encodedPublicKey) => {
  const data = Buffer.isBuffer(bytes) ? bytes : Buffer.from(bytes);
  const publicKey = decodeTauriPublicKey(encodedPublicKey);
  const signature = decodeTauriSignature(encodedSignature);
  if (!timingSafeEqual(publicKey.keyId, signature.keyId)) {
    throw new Error("minisign signature key id does not match the updater public key");
  }

  const message =
    signature.algorithm === "ED" ? createHash("blake2b512").update(data).digest() : data;
  if (!verify(null, message, publicKey.key, signature.signature)) {
    throw new Error("minisign content signature verification failed");
  }
  const globalMessage = Buffer.concat([
    signature.signature,
    Buffer.from(signature.trustedComment, "utf8"),
  ]);
  if (!verify(null, globalMessage, publicKey.key, signature.globalSignature)) {
    throw new Error("minisign trusted-comment signature verification failed");
  }
  return true;
};

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  const [, , filePath, signaturePath, configPath = "src-tauri/tauri.conf.json"] = process.argv;
  if (!filePath || !signaturePath) {
    console.error("usage: minisign-verify.mjs <file> <file.sig> [tauri.conf.json]");
    process.exit(2);
  }
  try {
    const config = JSON.parse(readFileSync(configPath, "utf8"));
    verifyTauriMinisign(
      readFileSync(filePath),
      readFileSync(signaturePath, "utf8").trim(),
      config?.plugins?.updater?.pubkey ?? "",
    );
    console.log(`verified minisign signature: ${filePath}`);
  } catch (error) {
    console.error(error instanceof Error ? error.message : String(error));
    process.exit(1);
  }
}
