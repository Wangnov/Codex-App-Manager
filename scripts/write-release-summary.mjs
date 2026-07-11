#!/usr/bin/env node
import { appendFileSync, existsSync, readFileSync } from "node:fs";

const env = process.env;
const tag = env.RELEASE_TAG || env.GITHUB_REF_NAME || "";
const repo = env.GITHUB_REPOSITORY || "Wangnov/Codex-App-Manager";
const mirrorBase = (env.MIRROR_BASE_URL || "https://codexapp.agentsmirror.com/manager").replace(/\/$/, "");
const summaryPath = env.GITHUB_STEP_SUMMARY;

if (!summaryPath) {
  console.warn("GITHUB_STEP_SUMMARY is not set; release summary was not written");
  process.exit(0);
}

const readJson = (path) => {
  if (!existsSync(path)) return null;
  try {
    return JSON.parse(readFileSync(path, "utf8"));
  } catch (error) {
    return { readError: error instanceof Error ? error.message : String(error) };
  }
};
const cell = (value) =>
  String(value ?? "")
    .replace(/\s+/g, " ")
    .replaceAll("|", "\\|")
    .replaceAll("`", "'")
    .replaceAll("<", "&lt;")
    .replaceAll(">", "&gt;")
    .slice(0, 500);
const manifest = readJson("latest.json");
const manifestSummary = readJson("manifest-summary.json");
const mirrorStage = readJson("mirror-stage-summary.json");
const mirrorVerification = readJson("mirror-verification-summary.json");
const mirrorPromotion = readJson("mirror-promotion-summary.json");
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
rows.push("## Mirror publication audit");
rows.push("");
if (mirrorStage || mirrorVerification || mirrorPromotion) {
  const audit = mirrorPromotion || mirrorVerification || mirrorStage;
  rows.push(`Candidate: \`${cell(audit?.candidateVersion || version)}\``);
  rows.push(`Candidate object: \`${cell(audit?.candidateKey || "unknown")}\``);
  rows.push(`Stage outcome: **${cell(mirrorStage?.outcome || "missing")}**`);
  rows.push(
    `Pre-publish verification: **${cell(mirrorVerification?.outcome || "not-run")}**`,
  );
  rows.push(`Promotion outcome: **${cell(mirrorPromotion?.outcome || "not-run")}**`);
  rows.push(
    `Public route verification: **${cell(
      mirrorPromotion?.publicRouteVerification ||
        mirrorVerification?.publicRouteVerification ||
        "not-run",
    )}**`,
  );
  if (mirrorPromotion?.rootIdentity) {
    rows.push(
      `Stable root identity: local=**${cell(
        mirrorPromotion.rootIdentity.localVerification || "not-run",
      )}**, latest transaction=**${cell(
        mirrorPromotion.rootIdentity.latestTransactionOutcome || "not-run",
      )}**, public readback=**${cell(
        mirrorPromotion.rootIdentity.publicRouteVerification || "not-run",
      )}**`,
    );
  }
  if (mirrorPromotion?.error || mirrorVerification?.error || mirrorStage?.error) {
    rows.push(
      `Mirror error: \`${cell(
        mirrorPromotion?.error || mirrorVerification?.error || mirrorStage?.error,
      )}\``,
    );
  }
  rows.push("");
  rows.push("| Backend | Candidate verification | Previous | Decision | Promotion | Root sig | Root JSON | Root direct | Root public | Supersession | Rollback | Final | Error |");
  rows.push("| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |");
  const backendRows =
    mirrorPromotion?.backends || mirrorVerification?.backends || mirrorStage?.backends || [];
  for (const backend of backendRows) {
    rows.push(
      `| ${cell(backend.name)} | ${cell(backend.candidateVerification || backend.status)} | ${cell(
        backend.currentVersion || "absent",
      )} | ${cell(backend.decision)} | ${cell(backend.promotion)} | ${cell(
        backend.rootIdentity?.signatureWrite,
      )} | ${cell(backend.rootIdentity?.jsonWrite)} | ${cell(
        backend.rootIdentity?.directVerification,
      )} | ${cell(backend.rootIdentity?.publicVerification)} | ${cell(
        backend.supersession,
      )} | ${cell(
        backend.rollback,
      )} | ${cell(backend.finalVersion)} | ${cell(
        backend.error || backend.rollbackError,
      )} |`,
    );
  }
  const override = audit?.override;
  rows.push("");
  if (override?.requested) {
    const originalActor =
      override.originalActor && override.originalActor !== override.actor
        ? ` (original workflow actor: \`${cell(override.originalActor)}\`)`
        : "";
    rows.push(
      `Emergency downgrade override: **requested=${cell(override.requested)}, used=${cell(
        override.used,
      )}** by \`${cell(override.actor)}\`${originalActor}; reason: ${cell(override.reason)}; [workflow audit](${cell(
        override.runUrl,
      )}).`,
    );
  } else {
    rows.push("Emergency downgrade override: not requested.");
  }
  if (mirrorPromotion?.authorityPreserved) {
    rows.push(
      "**Authority preserved:** R2 kept its verified CAS commit, but IHEP did not expose the exact canonical candidate identity. The follower was not overwritten and this run failed closed.",
    );
  } else if (mirrorPromotion?.rollback?.complete === false) {
    rows.push(
      "**Manual intervention required:** promotion rollback was incomplete; inspect the per-backend errors before another release.",
    );
  } else if (
    mirrorPromotion?.outcome === "failed" &&
    mirrorPromotion?.rootIdentity?.latestTransactionOutcome
  ) {
    rows.push(
      "**Retry required:** latest.json completed its monotonic transaction, but stable root identity publication or readback failed. Any mismatched identity/signature transition is rejected by clients, which fall back to GitHub; rerun the same release to repair both root pointers.",
    );
  } else if (["failed", "blocked-downgrade", "rolled-back"].includes(mirrorPromotion?.outcome)) {
    rows.push(
      "This run left no owned mirror-pointer advance in place; any completed rollback is shown per backend above.",
    );
  }
} else {
  rows.push("_No mirror audit record was produced (pre-release, skipped job, or failure before staging)._ ");
}

rows.push("");
rows.push("## Job status");
rows.push("");
rows.push("| Item | Status |");
rows.push("| --- | --- |");
rows.push(`| GitHub Release | ${env.PUBLISH_OUTCOME || "unknown"} |`);
rows.push(`| Mirror stage | ${env.MIRROR_STAGE_OUTCOME || "skipped"} |`);
rows.push(`| Mirror pre-publish verify | ${env.MIRROR_VERIFY_OUTCOME || "skipped"} |`);
rows.push(`| Mirror promote | ${env.MIRROR_PROMOTE_OUTCOME || "skipped"} |`);
rows.push(`| winget dispatch | ${env.WINGET_OUTCOME || "skipped"} |`);
rows.push(`| SBOM | ${env.SBOM_OUTCOME || "skipped"} |`);
rows.push(`| fresh provenance attestation | ${env.ATTESTATION_OUTCOME || "skipped"} |`);
rows.push(`| provenance mode | ${env.PROVENANCE_MODE || "blocked"} |`);
rows.push("");
rows.push(
  "winget dispatch only starts the downstream submission workflow; until the first package is accepted in microsoft/winget-pkgs, downstream failures are expected noise.",
);

appendFileSync(summaryPath, `${rows.join("\n")}\n`);
