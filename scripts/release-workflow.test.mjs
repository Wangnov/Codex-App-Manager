import { execFileSync } from "node:child_process";
import { mkdtemp, mkdir, readFile, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { describe, expect, it } from "vitest";

import {
  assertImmutableReleasesEnabled,
  verifyImmutableReleases,
} from "./check-immutable-releases.mjs";
import {
  assertReleaseTagCreationRuleset,
  assertReleaseTagCommit,
  assertReleaseTagRuleset,
  verifyReleaseTagProtection,
} from "./check-release-tag-protection.mjs";
import {
  REQUIRED_RELEASE_METADATA_ASSET_NAMES,
  requiredReleaseAssetNames,
} from "./check-release-reuse.mjs";
import {
  assertReleaseBindingAttestation,
  createReleaseBinding,
  RELEASE_BINDING_PREDICATE_TYPE,
  verifyReleaseBinding,
} from "./release-binding.mjs";
import {
  assertLocalReleaseArtifactNames,
  assertReleaseSourceVersions,
} from "./check-release-version.mjs";
import { verifyDraftReleaseAssets } from "./check-release-draft-assets.mjs";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "..");
const workflow = await readFile(
  join(repoRoot, ".github/workflows/release.yml"),
  "utf8",
);
const signalWorkflow = await readFile(
  join(repoRoot, ".github/workflows/release-source.yml"),
  "utf8",
);
const mirrorRelease = await readFile(
  join(repoRoot, "scripts/mirror-release.mjs"),
  "utf8",
);
const releaseJob = workflow.slice(workflow.indexOf("  release:\n"));

describe("release workflow recovery invariants", () => {
  it("queues every release run instead of replacing an older pending tag", () => {
    expect(workflow).toMatch(
      /concurrency:\n\s+group: release-latest-\$\{\{ github\.repository \}\}\n\s+cancel-in-progress: false\n\s+queue: max/,
    );
  });

  it("separates the unprivileged tag signal from the default-branch credentialed workflow", () => {
    const dispatchInputs = workflow.slice(
      workflow.indexOf("  workflow_dispatch:\n"),
      workflow.indexOf("\npermissions:\n"),
    );
    const preflight = workflow.slice(
      workflow.indexOf("  preflight:\n"),
      workflow.indexOf("  prepare:\n"),
    );
    const prepare = workflow.slice(
      workflow.indexOf("  prepare:\n"),
      workflow.indexOf("  build:\n"),
    );
    const build = workflow.slice(
      workflow.indexOf("  build:\n"),
      workflow.indexOf("  select_artifacts:\n"),
    );

    expect(signalWorkflow).toContain("name: Release source");
    expect(signalWorkflow).toContain('tags: ["v*"]');
    expect(signalWorkflow.match(/permissions: \{\}/g)).toHaveLength(2);
    expect(signalWorkflow).not.toContain("actions/checkout@");
    expect(signalWorkflow).not.toContain("environment:");
    expect(signalWorkflow).not.toContain("${{ secrets.");
    expect(signalWorkflow).not.toMatch(/upload-artifact|download-artifact/);

    const releaseTriggers = workflow.slice(
      workflow.indexOf("on:\n"),
      workflow.indexOf("\npermissions:\n"),
    );
    expect(releaseTriggers).toContain("workflow_run:");
    expect(releaseTriggers).toContain('workflows: ["Release source"]');
    expect(releaseTriggers).toContain("types: [completed]");
    expect(releaseTriggers).not.toMatch(/\n\s+push:/);
    expect(dispatchInputs).toMatch(
      /target_tag:\n\s+description:.*\n\s+required: true/,
    );

    expect(preflight).toContain(
      '[[ "$UPSTREAM_EVENT" == "push" && "$UPSTREAM_CONCLUSION" == "success" ]]',
    );
    expect(preflight).toContain(
      '[[ "$UPSTREAM_PATH" == ".github/workflows/release-source.yml"',
    );
    expect(preflight).toContain('[[ "$UPSTREAM_HEAD_SHA" =~ ^[0-9a-f]{40}$ ]]');
    expect(preflight).toContain("UPSTREAM_ACTOR_LOGIN");
    expect(preflight).toContain("UPSTREAM_ACTOR_ID");
    expect(preflight).toContain("UPSTREAM_TRIGGERING_ACTOR_LOGIN");
    expect(preflight).toContain("UPSTREAM_TRIGGERING_ACTOR_ID");
    expect(preflight).toContain("CURRENT_TRIGGERING_ACTOR_LOGIN");
    expect(preflight).toContain("AUTHORIZED_RELEASE_ACTOR_LOGIN");
    expect(preflight).toContain("AUTHORIZED_RELEASE_ACTOR_ID");
    expect(preflight).toContain(
      '[[ "$DISPATCH_REF" == "refs/heads/$DEFAULT_BRANCH" ]]',
    );
    expect(preflight).toContain("DISPATCH_ACTOR_ID");
    expect(preflight).toContain(
      "workflow_dispatch requires a non-empty target_tag",
    );
    expect(preflight).not.toContain("actions/checkout@");
    expect(preflight).not.toContain("environment:");
    expect(preflight).not.toContain("${{ secrets.");
    expect(prepare).toContain("needs: preflight");
    expect(prepare).toContain("needs.preflight.result == 'success'");
    expect(prepare).toContain(
      "ref: ${{ steps.tag_source.outputs.release_source_sha }}",
    );
    expect(build).toContain(
      "ref: ${{ needs.prepare.outputs.release_source_sha }}",
    );
    expect(build).toContain("path: release-source");
    expect(build).toContain(
      "& ..\\scripts\\assert-signpath-foundation-ready.ps1",
    );
    expect(
      build.indexOf("- name: Re-check release source before build"),
    ).toBeLessThan(
      build.indexOf(
        "- name: Assert SignPath Foundation readiness (fail closed)",
      ),
    );
    expect(workflow).not.toContain("github.ref_type == 'tag'");
    expect(workflow).toContain(
      "needs.preflight.outputs.trusted_invocation == 'tag-signal'",
    );
  });

  it("re-authorizes the current rerun actor before every credentialed job does work", () => {
    const credentialedJobs = [
      workflow.slice(
        workflow.indexOf("  prepare:\n"),
        workflow.indexOf("  build:\n"),
      ),
      workflow.slice(
        workflow.indexOf("  build:\n"),
        workflow.indexOf("  select_artifacts:\n"),
      ),
      releaseJob,
    ];

    for (const job of credentialedJobs) {
      const steps = job.indexOf("    steps:\n");
      const authorization = job.indexOf(
        "- name: Authorize credentialed job rerun actor",
      );
      const firstAction = job.indexOf("- uses:");
      const firstSecret = job.indexOf("${{ secrets.");
      expect(steps).toBeGreaterThan(-1);
      expect(authorization).toBeGreaterThan(steps);
      expect(authorization).toBeLessThan(firstAction);
      expect(authorization).toBeLessThan(firstSecret);
      expect(job.slice(steps, authorization)).not.toMatch(
        /\n\s+- (?:name:|uses:)/,
      );
      expect(job.slice(authorization, firstAction)).toContain(
        "CURRENT_TRIGGERING_ACTOR_LOGIN: ${{ github.triggering_actor }}",
      );
      expect(job.slice(authorization, firstAction)).toContain(
        '[[ "$CURRENT_TRIGGERING_ACTOR_LOGIN" == "$AUTHORIZED_RELEASE_ACTOR_LOGIN" ]]',
      );
      if (job.includes("working-directory: release-source")) {
        expect(job.slice(authorization, firstAction)).toContain(
          "working-directory: ${{ github.workspace }}",
        );
      }
    }

    const buildAuthorization = credentialedJobs[1].indexOf(
      "- name: Authorize credentialed job rerun actor",
    );
    const buildFirstAction = credentialedJobs[1].indexOf("- uses:");
    expect(
      credentialedJobs[1].slice(buildAuthorization, buildFirstAction),
    ).toContain("working-directory: ${{ github.workspace }}");
  });

  it("requires the exact release commit to remain an ancestor of the live default branch", async () => {
    const root = await mkdtemp(join(tmpdir(), "cam-release-ancestor-"));
    const origin = join(root, "origin.git");
    const source = join(root, "source");
    const gate = join(repoRoot, "scripts/check-release-source-ancestor.sh");
    const git = (...args) =>
      execFileSync("git", args, { encoding: "utf8", stdio: "pipe" }).trim();
    try {
      await mkdir(source);
      git("init", "--bare", "--initial-branch=main", origin);
      git("-C", source, "init", "--initial-branch=main");
      git("-C", source, "config", "user.name", "Release Test");
      git("-C", source, "config", "user.email", "release-test@example.com");
      git("-C", source, "remote", "add", "origin", origin);
      await writeFile(join(source, "version.txt"), "base\n");
      git("-C", source, "add", "version.txt");
      git("-C", source, "commit", "-m", "base");
      await writeFile(join(source, "version.txt"), "release\n");
      git("-C", source, "commit", "-am", "release");
      const releaseSha = git("-C", source, "rev-parse", "HEAD");
      git("-C", source, "push", "-u", "origin", "main");
      await writeFile(join(source, "after.txt"), "main advanced\n");
      git("-C", source, "add", "after.txt");
      git("-C", source, "commit", "-m", "advance main");
      git("-C", source, "push", "origin", "main");

      expect(
        execFileSync("bash", [gate, source, releaseSha, "main"], {
          encoding: "utf8",
        }),
      ).toContain("is merged into origin/main");

      git("-C", source, "checkout", "-b", "unmerged", releaseSha);
      await writeFile(join(source, "unmerged.txt"), "not reviewed\n");
      git("-C", source, "add", "unmerged.txt");
      git("-C", source, "commit", "-m", "unmerged release");
      const unmergedSha = git("-C", source, "rev-parse", "HEAD");
      expect(() =>
        execFileSync("bash", [gate, source, unmergedSha, "main"], {
          encoding: "utf8",
          stdio: "pipe",
        }),
      ).toThrow(/not an ancestor/);

      const prepare = workflow.slice(
        workflow.indexOf("  prepare:\n"),
        workflow.indexOf("  build:\n"),
      );
      const build = workflow.slice(
        workflow.indexOf("  build:\n"),
        workflow.indexOf("  select_artifacts:\n"),
      );
      expect(prepare).toContain("check-release-source-ancestor.sh");
      expect(build).toContain("check-release-source-ancestor.sh");
      expect(
        releaseJob.match(/check-release-source-ancestor\.sh/g).length,
      ).toBeGreaterThanOrEqual(4);
    } finally {
      await rm(root, { force: true, recursive: true });
    }
  });

  it("binds target tag, peeled source, trusted signer digest, and every subject digest", async () => {
    const root = await mkdtemp(join(tmpdir(), "cam-release-binding-"));
    const artifact = join(root, "artifact.bin");
    const manifest = join(root, "latest.json");
    try {
      await writeFile(artifact, "artifact bytes");
      await writeFile(manifest, '{"version":"1.2.3"}\n');
      const releaseSourceSha = "a".repeat(40);
      const trustedWorkflowSignerSha = "b".repeat(40);
      const trustedWorkflowSourceSha = "c".repeat(40);
      const binding = createReleaseBinding({
        defaultBranch: "main",
        paths: [artifact, manifest],
        releaseSourceSha,
        releaseTag: "v1.2.3",
        repository: "owner/repo",
        trustedWorkflowSignerSha,
        trustedWorkflowSourceSha,
      });
      expect(
        verifyReleaseBinding(binding, {
          defaultBranch: "main",
          paths: [artifact, manifest],
          releaseSourceSha,
          releaseTag: "v1.2.3",
          repository: "owner/repo",
        }),
      ).toMatchObject({
        signerSha: trustedWorkflowSignerSha,
        sourceSha: trustedWorkflowSourceSha,
      });
      const statementSubjects = Object.entries(binding.subjectDigests).map(
        ([name, digest]) => ({
          name,
          digest: { sha256: digest.slice("sha256:".length) },
        }),
      );
      expect(
        assertReleaseBindingAttestation(
          [
            {
              verificationResult: {
                statement: {
                  predicateType: RELEASE_BINDING_PREDICATE_TYPE,
                  predicate: binding,
                  subject: statementSubjects,
                },
              },
            },
          ],
          binding,
        ),
      ).toEqual({ matched: true });
      expect(() =>
        assertReleaseBindingAttestation(
          [
            {
              verificationResult: {
                statement: {
                  predicateType: RELEASE_BINDING_PREDICATE_TYPE,
                  predicate: binding,
                  subject: [...statementSubjects, statementSubjects[0]],
                },
              },
            },
          ],
          binding,
        ),
      ).toThrow("exact release binding and subject set");
      expect(() =>
        assertReleaseBindingAttestation(
          [
            {
              verificationResult: {
                statement: {
                  predicateType: RELEASE_BINDING_PREDICATE_TYPE,
                  predicate: binding,
                  subject: [
                    ...statementSubjects,
                    { name: "extra.bin", digest: { sha256: "d".repeat(64) } },
                  ],
                },
              },
            },
          ],
          binding,
        ),
      ).toThrow("exact release binding and subject set");
      expect(() =>
        assertReleaseBindingAttestation(
          [
            {
              verificationResult: {
                statement: {
                  predicateType: RELEASE_BINDING_PREDICATE_TYPE,
                  predicate: binding,
                  subject: statementSubjects.map((subject, index) =>
                    index === 0
                      ? { ...subject, digest: { sha256: "e".repeat(64) } }
                      : subject,
                  ),
                },
              },
            },
          ],
          binding,
        ),
      ).toThrow("exact release binding and subject set");
      expect(() =>
        assertReleaseBindingAttestation(
          [
            {
              verificationResult: {
                statement: {
                  predicateType: RELEASE_BINDING_PREDICATE_TYPE,
                  predicate: binding,
                  subject: statementSubjects.slice(1),
                },
              },
            },
          ],
          binding,
        ),
      ).toThrow("exact release binding and subject set");
      expect(() =>
        verifyReleaseBinding(
          { ...binding, targetTag: "v9.9.9" },
          {
            defaultBranch: "main",
            paths: [artifact, manifest],
            releaseSourceSha,
            releaseTag: "v1.2.3",
            repository: "owner/repo",
          },
        ),
      ).toThrow("does not match repository, tag, and source SHA");
      await writeFile(artifact, "mutated artifact bytes");
      expect(() =>
        verifyReleaseBinding(binding, {
          defaultBranch: "main",
          paths: [artifact, manifest],
          releaseSourceSha,
          releaseTag: "v1.2.3",
          repository: "owner/repo",
        }),
      ).toThrow("subject digests do not match");
      expect(requiredReleaseAssetNames("v1.2.3")).toContain(
        "release-binding.json",
      );
    } finally {
      await rm(root, { force: true, recursive: true });
    }
  });

  it("requires the mutable Release draft to match the exact canonical local asset set", async () => {
    const root = await mkdtemp(join(tmpdir(), "cam-release-draft-assets-"));
    const releaseTag = "v1.2.3";
    const names = [
      ...requiredReleaseAssetNames(releaseTag),
      ...REQUIRED_RELEASE_METADATA_ASSET_NAMES,
    ];
    const paths = names.map((name) => join(root, name));
    try {
      await Promise.all(
        paths.map((path, index) => writeFile(path, `canonical asset ${index}`)),
      );
      const release = {
        assets: names.map((name, index) => ({
          id: 101 + index,
          name,
          size: Buffer.byteLength(`canonical asset ${index}`),
        })),
        draft: true,
        id: 77,
        immutable: false,
        tag_name: releaseTag,
      };
      expect(verifyDraftReleaseAssets(release, releaseTag, paths)).toMatchObject({
        assets: expect.arrayContaining([
          expect.objectContaining({
            name: "CodexAppManager_1.2.3_x64-setup.exe",
          }),
          expect.objectContaining({ name: "latest.json" }),
        ]),
        releaseId: 77,
      });

      expect(() =>
        verifyDraftReleaseAssets(
          {
            ...release,
            assets: [
              ...release.assets,
              {
                id: 103,
                name: "CodexAppManager_1.2.3_unsigned-debug.exe",
                size: 1,
              },
            ],
          },
          releaseTag,
          paths,
        ),
      ).toThrow("unexpected: CodexAppManager_1.2.3_unsigned-debug.exe");
      expect(() =>
        verifyDraftReleaseAssets(
          { ...release, assets: release.assets.slice(1) },
          releaseTag,
          paths,
        ),
      ).toThrow(`missing: ${names[0]}`);
      expect(() =>
        verifyDraftReleaseAssets(
          {
            ...release,
            assets: release.assets.map((asset) =>
              asset.name === "latest.json"
                ? { ...asset, digest: `sha256:${"f".repeat(64)}` }
                : asset,
            ),
          },
          releaseTag,
          paths,
        ),
      ).toThrow("draft release asset digest differs from local bytes: latest.json");
      expect(() =>
        verifyDraftReleaseAssets(
          { ...release, draft: false, immutable: true },
          releaseTag,
          paths,
        ),
      ).toThrow("no longer the expected mutable draft");

      const localExtra = join(root, "unsigned-debug.exe");
      await writeFile(localExtra, "unsigned debug bytes");
      expect(() =>
        verifyDraftReleaseAssets(release, releaseTag, [...paths, localExtra]),
      ).toThrow("local release asset is not in the canonical allowlist: unsigned-debug.exe");
    } finally {
      await rm(root, { force: true, recursive: true });
    }
  });

  it("resets stale draft assets and verifies an exact byte-for-byte draft before publish", () => {
    const reset = releaseJob.indexOf(
      "- name: Reset existing mutable Release draft assets",
    );
    const upload = releaseJob.indexOf("- name: Upload GitHub Release draft");
    const verify = releaseJob.indexOf(
      "- name: Verify exact GitHub Release draft assets",
    );
    const publish = releaseJob.indexOf("- name: Publish GitHub Release");
    expect(reset).toBeGreaterThan(-1);
    expect(reset).toBeLessThan(upload);
    expect(upload).toBeLessThan(verify);
    expect(verify).toBeLessThan(publish);

    const resetStep = releaseJob.slice(reset, upload);
    expect(resetStep).toContain("releases/assets/$asset_id");
    expect(resetStep).toContain("(.assets | length) == 0");
    expect(resetStep).toContain(".draft == true and .immutable != true");

    const verifyStep = releaseJob.slice(verify, publish);
    expect(verifyStep).toContain("check-release-draft-assets.mjs");
    expect(verifyStep).toContain(
      "canonical_assets=(dist/* latest.json release-binding.json SHA256SUMS release-assets/*)",
    );
    expect(verifyStep).toContain('Accept: application/octet-stream');
    expect(verifyStep).toContain('sha256sum "$downloaded"');
    expect(verifyStep).toContain('cmp -- "$local_path" "$downloaded"');
    expect(verifyStep).toContain('cmp -- "$draft_plan" "$post_plan"');
  });

  it("binds the release tag to all six source versions and exact local artifact names", async () => {
    const packageJson = JSON.parse(
      await readFile(join(repoRoot, "package.json"), "utf8"),
    );
    const releaseTag = `v${packageJson.version}`;
    const source = assertReleaseSourceVersions(releaseTag, repoRoot);
    expect(source.entries).toHaveLength(6);
    expect(() => assertReleaseSourceVersions("v9.9.9", repoRoot)).toThrow(
      "mismatched source versions",
    );

    const artifactsDir = await mkdtemp(
      join(tmpdir(), "cam-release-artifacts-"),
    );
    try {
      const expected = requiredReleaseAssetNames(releaseTag).filter(
        (name) =>
          name !== "latest.json" &&
          name !== "release-binding.json" &&
          !name.startsWith("release-identity.json"),
      );
      await Promise.all(
        expected.map((name) => writeFile(join(artifactsDir, name), "x")),
      );
      expect(
        assertLocalReleaseArtifactNames(releaseTag, artifactsDir).expected,
      ).toEqual(expected);

      await writeFile(
        join(artifactsDir, "CodexAppManager_9.9.9_x64-setup.exe"),
        "x",
      );
      expect(() =>
        assertLocalReleaseArtifactNames(releaseTag, artifactsDir),
      ).toThrow("installer artifacts for another version");
    } finally {
      await rm(artifactsDir, { force: true, recursive: true });
    }

    const prepare = workflow.slice(
      workflow.indexOf("  prepare:\n"),
      workflow.indexOf("  build:\n"),
    );
    expect(prepare).toContain(
      "ref: ${{ steps.tag_source.outputs.release_source_sha }}",
    );
    expect(prepare).toContain(
      'node scripts/check-release-version.mjs source "$RELEASE_TAG" release-source',
    );
    expect(releaseJob).toContain(
      'node scripts/check-release-version.mjs artifacts "$RELEASE_TAG" dist',
    );
    const manifestStep = releaseJob.slice(
      releaseJob.indexOf("- name: Generate updater manifest (latest.json)"),
      releaseJob.indexOf(
        "- name: Verify local updater signatures before immutable publication",
      ),
    );
    expect(manifestStep).toContain(
      "node scripts/validate-release-manifest.mjs",
    );
    expect(manifestStep).toContain(
      'node scripts/gen-updater-manifest.mjs "$RELEASE_TAG" dist release-source',
    );
    expect(manifestStep).not.toContain('if [[ "$RELEASE_TAG" != *-* ]]');
  });

  it("enforces the single-writer boundary for the unconditional IHEP follower", () => {
    expect(workflow).toContain(
      "correctness boundary for the unconditional IHEP follower",
    );
    expect(releaseJob).toContain("Enforce legacy mirror credential revocation");
    expect(releaseJob).toContain("MANAGER_R2_PROMOTION_ACCESS_KEY_ID");
    expect(releaseJob).toContain("MANAGER_IHEP_S3_PROMOTION_ACCESS_KEY_ID");
    expect(releaseJob).toContain(
      "MANAGER_IHEP_S3_ENDPOINT: ${{ vars.MANAGER_IHEP_S3_ENDPOINT }}",
    );
    expect(releaseJob).toContain(
      "MANAGER_IHEP_S3_BUCKET: ${{ vars.MANAGER_IHEP_S3_BUCKET }}",
    );
    expect(releaseJob).not.toContain(
      "MANAGER_IHEP_S3_ENDPOINT: ${{ secrets.MANAGER_IHEP_S3_ENDPOINT }}",
    );
    expect(mirrorRelease).toContain(
      "followerCommit = await ihep.putLatestUnconditional(candidatePath, promotionToken)",
    );
    expect(mirrorRelease).not.toContain("await ihep.putLatestConditional(");
  });

  it("refreshes immutable Release state inside the release job on failed-job reruns", () => {
    expect(releaseJob).toContain("id: live_release");
    expect(releaseJob).toContain(
      "if: ${{ steps.live_release.outputs.release_reusable != 'true' }}",
    );
    expect(releaseJob).toContain(
      "RELEASE_ASSET_DIGESTS: ${{ steps.live_release.outputs.release_asset_digests }}",
    );
    expect(releaseJob).toContain(
      'if [[ "${{ steps.live_release.outputs.release_reusable }}" == "true" ]]; then',
    );
    expect(releaseJob).not.toContain(
      "if: ${{ needs.prepare.outputs.release_reusable != 'true' }}",
    );
  });

  it("uses the target tag updater trust root without executing historical scripts", () => {
    const refresh = releaseJob.indexOf(
      "- name: Refresh immutable release state",
    );
    const resolveTrust = releaseJob.indexOf(
      "- name: Resolve updater trust root for release tag",
    );
    const download = releaseJob.indexOf(
      "- name: Download canonical build artifacts",
    );
    const localVerify = releaseJob.indexOf(
      "- name: Verify local updater signatures before immutable publication",
    );
    const stage = releaseJob.indexOf("- name: Stage CDN mirror candidate");
    const trustStep = releaseJob.slice(resolveTrust, download);
    const localVerifyStep = releaseJob.slice(localVerify, stage);

    expect(resolveTrust).toBeGreaterThan(refresh);
    expect(download).toBeGreaterThan(resolveTrust);
    expect(trustStep).toContain("gh api --method GET");
    expect(trustStep).toContain("application/vnd.github.raw+json");
    expect(trustStep).toContain('-f ref="$RELEASE_SOURCE_SHA"');
    expect(trustStep).toContain("RELEASE_TAURI_CONFIG=");
    expect(trustStep).toContain("MIRROR_UPDATER_PUBLIC_KEY=");
    expect(trustStep).toContain('>> "$GITHUB_ENV"');
    expect(localVerifyStep).toContain('"$RELEASE_TAURI_CONFIG"');
    expect(localVerifyStep).not.toContain("src-tauri/tauri.conf.json");
    expect(mirrorRelease).toContain("process.env.MIRROR_UPDATER_PUBLIC_KEY ||");
  });

  it("fails closed when immutable settings are disabled or cannot be queried", () => {
    expect(assertImmutableReleasesEnabled({ enabled: true })).toEqual({
      enabled: true,
    });
    expect(() => assertImmutableReleasesEnabled({ enabled: false })).toThrow(
      "GitHub Immutable Releases are disabled",
    );
    expect(() =>
      verifyImmutableReleases({
        repository: "owner/repo",
        token: "read-only-token",
        runner: () => ({ status: 1, stderr: "HTTP 403", stdout: "" }),
      }),
    ).toThrow("could not verify GitHub Immutable Releases");

    const prepare = workflow.slice(
      workflow.indexOf("  prepare:\n"),
      workflow.indexOf("  build:\n"),
    );
    expect(prepare).toContain("environment: release");
    expect(prepare).toContain(
      "GH_TOKEN: ${{ secrets.IMMUTABLE_RELEASES_READ_TOKEN }}",
    );
    expect(prepare).toContain("run: node scripts/check-immutable-releases.mjs");
    expect(releaseJob).toContain(
      "run: node scripts/check-immutable-releases.mjs",
    );
  });

  it("requires immutable release tags and rechecks the live peeled commit at publication", () => {
    const protectedRuleset = {
      id: 1,
      name: "immutable release tags",
      target: "tag",
      enforcement: "active",
      bypass_actors: [],
      conditions: { ref_name: { include: ["refs/tags/v*"], exclude: [] } },
      rules: [{ type: "update" }, { type: "deletion" }],
    };
    const creationRuleset = {
      id: 2,
      name: "authorized release tag creation",
      target: "tag",
      enforcement: "active",
      bypass_actors: [
        {
          actor_id: 48670012,
          actor_type: "User",
          bypass_mode: "always",
        },
      ],
      conditions: { ref_name: { include: ["refs/tags/v*"], exclude: [] } },
      rules: [{ type: "creation" }],
    };
    expect(assertReleaseTagRuleset([protectedRuleset])).toEqual({
      id: 1,
      name: "immutable release tags",
    });
    expect(() =>
      assertReleaseTagRuleset([
        { ...protectedRuleset, rules: [{ type: "deletion" }] },
      ]),
    ).toThrow("must protect refs/tags/v*");
    expect(() =>
      assertReleaseTagRuleset([
        {
          ...protectedRuleset,
          bypass_actors: [{ actor_type: "User", actor_id: 1 }],
        },
      ]),
    ).toThrow("visible bypass actors");
    expect(assertReleaseTagCreationRuleset([creationRuleset])).toEqual({
      id: 2,
      name: "authorized release tag creation",
    });
    expect(() =>
      assertReleaseTagCreationRuleset([
        {
          ...creationRuleset,
          bypass_actors: [
            {
              actor_id: 5,
              actor_type: "RepositoryRole",
              bypass_mode: "always",
            },
          ],
        },
      ]),
    ).toThrow("authorized release publisher");

    const expectedSha = "a".repeat(40);
    expect(assertReleaseTagCommit(expectedSha, expectedSha)).toEqual({
      sha: expectedSha,
    });
    expect(() => assertReleaseTagCommit("b".repeat(40), expectedSha)).toThrow(
      "release tag moved after validation",
    );

    const responses = new Map([
      [
        "repos/owner/repo/rulesets?targets=tag&per_page=100",
        [protectedRuleset, creationRuleset],
      ],
      ["repos/owner/repo/rulesets/1", protectedRuleset],
      ["repos/owner/repo/rulesets/2", creationRuleset],
      [
        "repos/owner/repo/git/ref/tags/v1.2.3",
        { object: { type: "tag", sha: "b".repeat(40) } },
      ],
      [
        `repos/owner/repo/git/tags/${"b".repeat(40)}`,
        { object: { type: "commit", sha: expectedSha } },
      ],
    ]);
    const runner = (_command, args) => ({
      status: 0,
      stderr: "",
      stdout: JSON.stringify(responses.get(args.at(-1))),
    });
    expect(
      verifyReleaseTagProtection({
        repository: "owner/repo",
        releaseTag: "v1.2.3",
        expectedSha,
        token: "read-token",
        runner,
      }),
    ).toEqual({
      commit: { sha: expectedSha },
      creationRuleset: { id: 2, name: "authorized release tag creation" },
      ruleset: { id: 1, name: "immutable release tags" },
    });
    expect(
      verifyReleaseTagProtection({
        allowResolve: true,
        expectedSha: "",
        releaseTag: "v1.2.3",
        repository: "owner/repo",
        runner,
        token: "read-token",
      }).commit,
    ).toEqual({ sha: expectedSha });
    expect(() =>
      verifyReleaseTagProtection({
        allowResolve: false,
        expectedSha: "",
        releaseTag: "v1.2.3",
        repository: "owner/repo",
        runner,
        token: "read-token",
      }),
    ).toThrow("expected release source SHA");

    const prepare = workflow.slice(
      workflow.indexOf("  prepare:\n"),
      workflow.indexOf("  build:\n"),
    );
    expect(prepare).toContain("node scripts/check-release-tag-protection.mjs");
    expect(prepare).toContain('echo "RELEASE_SOURCE_SHA=$release_source_sha"');

    const upload = releaseJob.indexOf("- name: Upload GitHub Release draft");
    const publish = releaseJob.indexOf("- name: Publish GitHub Release");
    const beforeUpload = releaseJob.lastIndexOf(
      "- name: Re-check protected release source before draft upload",
      upload,
    );
    const beforePublish = releaseJob.lastIndexOf(
      "- name: Re-check protected release source before publication",
      publish,
    );
    expect(beforeUpload).toBeGreaterThan(-1);
    expect(beforeUpload).toBeLessThan(upload);
    expect(beforePublish).toBeGreaterThan(upload);
    expect(beforePublish).toBeLessThan(publish);
    expect(releaseJob.slice(beforeUpload, upload)).toContain(
      'node scripts/check-release-tag-protection.mjs "$RELEASE_TAG" "$RELEASE_SOURCE_SHA"',
    );
    expect(releaseJob.slice(beforeUpload, upload)).toContain(
      "check-release-source-ancestor.sh",
    );
    expect(releaseJob.slice(beforePublish, publish)).toContain(
      'node scripts/check-release-tag-protection.mjs "$RELEASE_TAG" "$RELEASE_SOURCE_SHA"',
    );
    expect(releaseJob.slice(beforePublish, publish)).toContain(
      "check-release-source-ancestor.sh",
    );
  });

  it("uploads stable and prerelease assets to a draft before publishing", () => {
    const localVerify = releaseJob.indexOf(
      "- name: Verify local updater signatures before immutable publication",
    );
    const attest = releaseJob.indexOf("- name: Attest fresh build provenance");
    const verifyExisting = releaseJob.indexOf(
      "- name: Verify existing immutable release provenance",
    );
    const provenance = releaseJob.indexOf("- name: Resolve provenance gate");
    const stage = releaseJob.indexOf("- name: Stage CDN mirror candidate");
    const mirrorVerify = releaseJob.indexOf(
      "- name: Verify staged CDN mirror before immutable publication",
    );
    const upload = releaseJob.indexOf("- name: Upload GitHub Release draft");
    const publish = releaseJob.indexOf("- name: Publish GitHub Release");
    const publishedVerify = releaseJob.indexOf(
      "- name: Verify published immutable Release and asset digests",
    );
    const promote = releaseJob.indexOf("- name: Promote CDN mirror latest");
    const winget = releaseJob.indexOf("- name: Trigger winget submission");
    const summary = releaseJob.indexOf("- name: Write release summary");
    expect(localVerify).toBeGreaterThan(-1);
    expect(attest).toBeGreaterThan(localVerify);
    expect(verifyExisting).toBeGreaterThan(attest);
    expect(provenance).toBeGreaterThan(verifyExisting);
    expect(stage).toBeGreaterThan(provenance);
    expect(mirrorVerify).toBeGreaterThan(stage);
    expect(upload).toBeGreaterThan(mirrorVerify);
    expect(upload).toBeGreaterThan(-1);
    expect(publish).toBeGreaterThan(upload);
    expect(publishedVerify).toBeGreaterThan(publish);
    expect(promote).toBeGreaterThan(publishedVerify);

    const uploadStep = releaseJob.slice(upload, publish);
    const publishStep = releaseJob.slice(publish, publishedVerify);
    const verifyStep = releaseJob.slice(publishedVerify, promote);
    const attestStep = releaseJob.slice(attest, verifyExisting);
    const existingStep = releaseJob.slice(verifyExisting, provenance);
    const provenanceStep = releaseJob.slice(provenance, stage);
    const promoteStep = releaseJob.slice(promote, winget);
    const wingetStep = releaseJob.slice(winget, summary);
    const localVerifyStep = releaseJob.slice(localVerify, stage);
    const mirrorVerifyStep = releaseJob.slice(mirrorVerify, upload);
    expect(localVerifyStep).toContain(
      "node scripts/verify-release-artifacts.mjs",
    );
    expect(mirrorVerifyStep).toContain("MIRROR_PHASE: verify");
    expect(mirrorVerifyStep).toContain("bash scripts/sync-mirror.sh dist");
    expect(uploadStep).toContain("draft: true");
    expect(uploadStep).toContain("steps.provenance.outputs.ready == 'true'");
    expect(uploadStep).toContain(
      "prerelease: ${{ contains(env.RELEASE_TAG, '-') }}",
    );
    expect(uploadStep).toContain("files: |");
    expect(uploadStep).toContain("release-binding.json");
    expect(publishStep).not.toMatch(/^\s+draft:/m);
    expect(publishStep).not.toContain("files: |");
    expect(publishStep).toContain("steps.provenance.outputs.ready == 'true'");
    expect(verifyStep).toContain("node scripts/check-release-reuse.mjs");
    expect(verifyStep).toContain(
      "did not become immutable with canonical asset digests",
    );
    expect(verifyStep).toContain(
      "published immutable digest does not match attested local bytes",
    );
    expect(verifyStep).toContain('gh release verify "$RELEASE_TAG"');
    expect(verifyStep).toContain('gh release verify-asset "$RELEASE_TAG"');
    expect(attestStep).toContain(
      "steps.release_source.outputs.existing != 'true'",
    );
    expect(attestStep).toContain("actions/attest-build-provenance@");
    expect(attestStep).toContain("actions/attest@");
    expect(attestStep).toContain(
      "https://codexapp.agentsmirror.com/attestations/release-binding/v1",
    );
    expect(attestStep).toContain("predicate-path: release-binding.json");
    expect(attestStep).not.toContain("continue-on-error: true");
    expect(existingStep).toContain("gh attestation verify");
    expect(existingStep).toContain('gh release verify "$RELEASE_TAG"');
    expect(existingStep).toContain('gh release verify-asset "$RELEASE_TAG"');
    expect(existingStep).toContain('--signer-workflow "$signer_workflow"');
    expect(existingStep).toContain('--signer-digest "$RELEASE_SIGNER_SHA"');
    expect(existingStep).toContain('--source-ref "refs/heads/$DEFAULT_BRANCH"');
    expect(existingStep).toContain(
      '--source-digest "$RELEASE_WORKFLOW_SOURCE_SHA"',
    );
    expect(existingStep).toContain("release-binding.mjs attestation");
    expect(attestStep).toContain("steps.attest_binding.outputs.bundle-path");
    expect(attestStep).toContain('--signer-digest "$TRUSTED_WORKFLOW_SHA"');
    expect(attestStep).toContain(
      '--source-digest "$TRUSTED_WORKFLOW_SOURCE_SHA"',
    );
    expect(attestStep).toContain("release-binding.mjs attestation");
    expect(provenanceStep).toContain('echo "ready=true" >> "$GITHUB_OUTPUT"');
    expect(promoteStep).toContain("steps.provenance.outputs.ready == 'true'");
    expect(promoteStep).not.toContain("steps.attest_fresh.outcome");
    expect(wingetStep).toContain("steps.provenance.outputs.ready == 'true'");
    expect(wingetStep).not.toContain("steps.attest_fresh.outcome");
    expect(releaseJob.slice(0, upload)).toContain(
      "rm -f dist/latest.mirror.json dist/latest.json",
    );
  });

  it("verifies existing immutable provenance without minting a rerun attestation", () => {
    const source = releaseJob.indexOf(
      "- name: Resolve immutable release artifact source",
    );
    const validate = releaseJob.indexOf(
      "- name: Validate final release artifacts",
    );
    const attest = releaseJob.indexOf("- name: Attest fresh build provenance");
    const verifyExisting = releaseJob.indexOf(
      "- name: Verify existing immutable release provenance",
    );
    const provenance = releaseJob.indexOf("- name: Resolve provenance gate");
    const sourceStep = releaseJob.slice(source, validate);
    const attestStep = releaseJob.slice(attest, verifyExisting);
    const existingStep = releaseJob.slice(verifyExisting, provenance);

    expect(sourceStep).toContain("gh release download");
    expect(sourceStep).toContain("--pattern 'CodexAppManager*'");
    expect(sourceStep).toContain("--pattern 'latest.json'");
    expect(sourceStep).toMatch(
      /--pattern 'release-identity\.json\*' \\\s+--pattern 'release-binding\.json'/,
    );
    expect(sourceStep).toContain("--pattern 'release-binding.json'");
    expect(sourceStep).toContain('actual_digest="sha256:$(sha256sum "$file"');
    expect(sourceStep).toContain(
      'if [[ "$actual_digest" != "$expected_digest" ]]',
    );
    expect(attestStep).toContain(
      "steps.release_source.outputs.existing != 'true'",
    );
    expect(attestStep).not.toContain(
      "steps.release_source.outputs.existing == 'true'",
    );
    expect(existingStep).toContain(
      "steps.release_source.outputs.existing == 'true'",
    );
    expect(existingStep).toContain("for file in dist/* latest.json");
    expect(existingStep).toContain("gh attestation verify");
    expect(existingStep).toContain("--signer-digest");
    expect(existingStep).toContain("--predicate-type");
    expect(existingStep).toContain("release-binding.mjs attestation");
    expect(existingStep).toContain("--deny-self-hosted-runners");
  });
});
