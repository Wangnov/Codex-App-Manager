import { createHash } from "node:crypto";
import {
  mkdtempSync,
  mkdirSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { spawnSync } from "node:child_process";
import { afterEach, describe, expect, it } from "vitest";

const script = join(
  dirname(fileURLToPath(import.meta.url)),
  "gen-updater-manifest.mjs",
);
const tempDirs = [];

afterEach(() => {
  for (const dir of tempDirs.splice(0))
    rmSync(dir, { recursive: true, force: true });
});

describe("release identity generation", () => {
  const prepareRelease = (tag = "v1.2.3") => {
    const root = mkdtempSync(join(tmpdir(), "cam-release-identity-"));
    tempDirs.push(root);
    mkdirSync(join(root, "dist"));
    mkdirSync(join(root, "docs", "releases"), { recursive: true });
    mkdirSync(join(root, "release-source", "docs", "releases"), {
      recursive: true,
    });
    writeFileSync(
      join(root, "docs", "releases", `${tag}.md`),
      "Drifted notes from the current default branch\n",
    );
    writeFileSync(
      join(root, "release-source", "docs", "releases", `${tag}.md`),
      "Reviewed notes\n",
    );

    const fixtures = [
      ["CodexAppManager_aarch64.app.tar.gz", "mac-arm"],
      ["CodexAppManager_x86_64.app.tar.gz", "mac-x64"],
      ["CodexAppManager_1.2.3_x64-setup.exe", "win-x64"],
      ["CodexAppManager_1.2.3_arm64-setup.exe", "win-arm"],
    ];
    for (const [name, bytes] of fixtures) {
      writeFileSync(join(root, "dist", name), bytes);
      writeFileSync(join(root, "dist", `${name}.sig`), `signature-${name}`);
    }

    return {
      root,
      run: (releaseTag = tag) => {
        const result = spawnSync(
          process.execPath,
          [script, releaseTag, "dist", "release-source"],
          {
            cwd: root,
            encoding: "utf8",
          },
        );
        return result;
      },
    };
  };

  it("is byte-stable across reruns and binds a stable channel and artifact digests", () => {
    const { root, run } = prepareRelease();
    const generateIdentity = () => {
      const result = run();
      expect(result.status, result.stderr).toBe(0);
      return readFileSync(join(root, "release-identity.json"));
    };
    const first = generateIdentity();
    const second = generateIdentity();
    expect(second.equals(first)).toBe(true);

    const identity = JSON.parse(first.toString("utf8"));
    const manifest = JSON.parse(
      readFileSync(join(root, "latest.json"), "utf8"),
    );
    expect(identity).not.toHaveProperty("pub_date");
    expect(identity.channel).toBe("stable");
    expect(manifest.channel).toBe("stable");
    expect(manifest.notes).toBe("Reviewed notes");
    expect(identity.notes_sha256).toBe(
      createHash("sha256").update("Reviewed notes", "utf8").digest("hex"),
    );
    expect(identity.platforms["windows-x86_64"]).toEqual({
      artifact: "CodexAppManager_1.2.3_x64-setup.exe",
      sha256: createHash("sha256").update("win-x64").digest("hex"),
    });
    expect(manifest.platforms["windows-x86_64"]).toMatchObject({
      signature: "signature-CodexAppManager_1.2.3_x64-setup.exe",
      sha256: identity.platforms["windows-x86_64"].sha256,
    });
  });

  it("derives prerelease channel from a canonical SemVer tag", () => {
    const { root, run } = prepareRelease("v1.2.3-rc.1");
    const result = run();
    expect(result.status, result.stderr).toBe(0);

    const identity = JSON.parse(
      readFileSync(join(root, "release-identity.json"), "utf8"),
    );
    const manifest = JSON.parse(
      readFileSync(join(root, "latest.json"), "utf8"),
    );
    expect(identity).toMatchObject({
      version: "1.2.3-rc.1",
      channel: "prerelease",
    });
    expect(manifest).toMatchObject({
      version: "1.2.3-rc.1",
      channel: "prerelease",
    });
  });

  it.each(["1.2.3", "v1.2", "v01.2.3", "v1.2.3-01"])(
    "rejects non-canonical release tag %s",
    (tag) => {
      const { run } = prepareRelease();
      const result = run(tag);
      expect(result.status).toBe(1);
      expect(result.stderr).toContain("canonical SemVer tag");
    },
  );
});
