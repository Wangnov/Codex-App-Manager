import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";

import { requiredReleaseAssetNames } from "./check-release-reuse.mjs";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");

describe("release identity workflow wiring", () => {
  it("publishes one latest.json asset while retaining signed versioned identity assets", () => {
    const workflow = readFileSync(join(root, ".github", "workflows", "release.yml"), "utf8");
    const required = requiredReleaseAssetNames("v1.2.3");
    expect(required).toContain("release-identity.json");
    expect(required).toContain("release-identity.json.sig");
    expect(workflow).toContain("--pattern 'release-identity.json*'");
    expect(workflow).toContain(
      "if: ${{ steps.release_source.outputs.existing != 'true' }}",
    );
    expect(workflow).toContain(
      "if: ${{ steps.release_source.outputs.existing == 'true' }}",
    );
    expect(workflow).toContain("cp dist/release-identity.json release-identity.json");
    expect(workflow).toContain("cp dist/release-identity.json.sig release-identity.json.sig");
    expect(workflow).toContain("cp release-identity.json release-identity.json.sig dist/");
    expect(workflow).toContain('"$RELEASE_TAURI_CONFIG"');
    expect(workflow).not.toMatch(/cp latest\.json release-identity\.json/);
    expect(workflow).toMatch(/files:\s*\|[\s\S]*?dist\/\*[\s\S]*?\n\s+latest\.json/);
  });
});
