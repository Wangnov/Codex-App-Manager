#!/usr/bin/env node
import { appendFileSync, existsSync, readFileSync } from "node:fs";

const env = process.env;
const tag = env.GITHUB_REF_NAME || env.RELEASE_TAG || "";
const repo = env.GITHUB_REPOSITORY || "Wangnov/Codex-App-Manager";
const mirrorBase = (env.MIRROR_BASE_URL || "https://codexapp.agentsmirror.com/manager").replace(/\/$/, "");
const summaryPath = env.GITHUB_STEP_SUMMARY;

if (!summaryPath) {
  console.warn("GITHUB_STEP_SUMMARY is not set; release summary was not written");
  process.exit(0);
}

const readJson = (path) => (existsSync(path) ? JSON.parse(readFileSync(path, "utf8")) : null);
const manifest = readJson("latest.json");
const manifestSummary = readJson("manifest-summary.json");
const version = manifest?.version || (tag ? tag.replace(/^v/, "") : "");
const releaseUrl = `https://github.com/${repo}/releases/tag/${tag}`;
const latestUrl = `${mirrorBase}/latest.json`;

const artifactsByPlatform = new Map(
  (manifestSummary?.artifacts || []).map((artifact) => [artifact.platform, artifact]),
);
const requiredPlatforms =
  manifestSummary?.required_platforms ||
  Object.keys(manifest?.platforms || {}).map((platform) => ({ platform, present: true, missing: false }));

const checksums = existsSync("SHA256SUMS")
  ? readFileSync("SHA256SUMS", "utf8")
      .trim()
      .split(/\r?\n/)
      .filter(Boolean)
      .map((line) => {
        const [sha256, ...nameParts] = line.trim().split(/\s+/);
        return { sha256, name: nameParts.join(" ") };
      })
  : [];

const rows = [];
rows.push(`# Release ${tag}`);
rows.push("");
rows.push(`GitHub Release: ${releaseUrl}`);
rows.push(`Mirror latest: ${latestUrl}`);
rows.push("");
rows.push("## Platform matrix");
rows.push("");
rows.push("| Platform | Status | Artifact | SHA-256 | Mirror URL |");
rows.push("| --- | --- | --- | --- | --- |");
for (const entry of requiredPlatforms) {
  const artifact = artifactsByPlatform.get(entry.platform);
  const status = artifact ? "present" : "missing";
  const mirrorUrl = artifact ? `${mirrorBase}/${version}/${artifact.artifact}` : "";
  rows.push(
    `| ${entry.platform} | ${status} | ${artifact?.artifact || ""} | ${artifact?.sha256 || ""} | ${mirrorUrl} |`,
  );
}

rows.push("");
rows.push("## Release asset checksums");
rows.push("");
if (checksums.length > 0) {
  rows.push("| SHA-256 | File |");
  rows.push("| --- | --- |");
  for (const checksum of checksums) {
    rows.push(`| ${checksum.sha256} | ${checksum.name} |`);
  }
} else {
  rows.push("_SHA256SUMS was not generated._");
}

rows.push("");
rows.push("## Job status");
rows.push("");
rows.push("| Item | Status |");
rows.push("| --- | --- |");
rows.push(`| GitHub Release | ${env.PUBLISH_OUTCOME || "unknown"} |`);
rows.push(`| Mirror stage | ${env.MIRROR_STAGE_OUTCOME || "skipped"} |`);
rows.push(`| Mirror promote | ${env.MIRROR_PROMOTE_OUTCOME || "skipped"} |`);
rows.push(`| winget dispatch | ${env.WINGET_OUTCOME || "skipped"} |`);
rows.push(`| SBOM | ${env.SBOM_OUTCOME || "skipped"} |`);
rows.push(`| provenance attestation | ${env.ATTESTATION_OUTCOME || "skipped"} |`);
rows.push("");
rows.push(
  "winget dispatch only starts the downstream submission workflow; until the first package is accepted in microsoft/winget-pkgs, downstream failures are expected noise.",
);

appendFileSync(summaryPath, `${rows.join("\n")}\n`);
