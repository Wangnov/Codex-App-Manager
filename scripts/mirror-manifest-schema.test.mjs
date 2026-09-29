// Proves the two vendored copies of the codex-app-mirror release-manifest.json
// contract -- the JSON Schema and the fixtures under
// docs/contracts/release-manifest/ -- cannot silently drift apart from each
// other. This is the schema-side half of the contract; the Rust half (that
// the *parser* also agrees with these fixtures) lives in
// crates/codex-win-engine/tests/mirror_manifest_contract.rs and
// crates/codex-mac-engine/tests/live_appcast_fixtures.rs.
//
// See docs/contracts/release-manifest/README.md for provenance (source repo,
// PR, commit) and refresh instructions.

import { readFile, readdir } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

import Ajv2020 from "ajv/dist/2020.js";
import addFormats from "ajv-formats";
import { describe, expect, it } from "vitest";

const here = path.dirname(fileURLToPath(import.meta.url));
const CONTRACT_ROOT = path.join(here, "..", "docs", "contracts", "release-manifest");
const SCHEMA_PATH = path.join(CONTRACT_ROOT, "schema", "release-manifest.schema.json");
const FIXTURES_ROOT = path.join(CONTRACT_ROOT, "fixtures");

const VALID_GROUPS = ["stable", "beta", "linux-preview"];

async function readJson(filePath) {
  return JSON.parse(await readFile(filePath, "utf8"));
}

async function listJsonFiles(dir) {
  const entries = await readdir(dir, { withFileTypes: true });
  return entries
    .filter((e) => e.isFile() && e.name.endsWith(".json"))
    .map((e) => path.join(dir, e.name))
    .sort();
}

function compileSchema(schema) {
  const ajv = new Ajv2020({ allErrors: true, strict: false });
  addFormats(ajv);
  return ajv.compile(schema);
}

describe("vendored release-manifest.schema.json contract", () => {
  it("is present and is valid JSON Schema (2020-12)", async () => {
    const schema = await readJson(SCHEMA_PATH);
    expect(schema.$schema).toContain("2020-12");
    // compileSchema throws if the schema itself doesn't compile.
    expect(() => compileSchema(schema)).not.toThrow();
  });

  for (const group of VALID_GROUPS) {
    it(`accepts every vendored fixtures/${group}/*.json fixture`, async () => {
      const schema = await readJson(SCHEMA_PATH);
      const validate = compileSchema(schema);
      const dir = path.join(FIXTURES_ROOT, group);
      const files = await listJsonFiles(dir);
      expect(files.length, `expected at least one fixture under ${dir}`).toBeGreaterThan(0);

      for (const file of files) {
        const data = await readJson(file);
        const valid = validate(data);
        expect(
          valid,
          `${path.relative(CONTRACT_ROOT, file)} should validate:\n${JSON.stringify(
            validate.errors,
            null,
            2,
          )}`,
        ).toBe(true);
      }
    });
  }

  it("rejects every vendored fixtures/invalid/*.json fixture", async () => {
    const schema = await readJson(SCHEMA_PATH);
    const validate = compileSchema(schema);
    const dir = path.join(FIXTURES_ROOT, "invalid");
    const files = await listJsonFiles(dir);
    expect(files.length, `expected at least one fixture under ${dir}`).toBeGreaterThan(0);

    for (const file of files) {
      const data = await readJson(file);
      const valid = validate(data);
      expect(
        valid,
        `${path.relative(CONTRACT_ROOT, file)} should be REJECTED by the schema but validated`,
      ).toBe(false);
    }
  });

  it("has at least one fixture in every group (catches an empty vendoring mistake)", async () => {
    for (const group of [...VALID_GROUPS, "invalid"]) {
      const files = await listJsonFiles(path.join(FIXTURES_ROOT, group));
      expect(files.length, `fixtures/${group}`).toBeGreaterThan(0);
    }
  });
});
