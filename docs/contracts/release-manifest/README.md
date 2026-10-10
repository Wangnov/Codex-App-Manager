# release-manifest.json contract (vendored)

This directory is a vendored copy of the `release-manifest.json` JSON Schema
contract and its real-sample fixtures, published by the mirror project this
Manager consumes. It exists so Codex App Manager can prove -- in its own CI,
without depending on the mirror repo at test time -- that
`crates/codex-win-engine/src/manifest.rs` (and `codex-mac-engine`'s appcast
parser) still agree with the shapes the mirror actually publishes.

## Source

- Repository: [`Wangnov/codex-app-mirror`](https://github.com/Wangnov/codex-app-mirror)
- Initial schema: [#71](https://github.com/Wangnov/codex-app-mirror/pull/71).
- Latest refresh: [#73](https://github.com/Wangnov/codex-app-mirror/pull/73),
  merged to `main` on 2026-10-10.
- Commit vendored: `91f0d5126b066cd9ed4c7ed3ce9dde6d851f6d03` (mirror `main`,
  schema and fixtures copied byte for byte).
- Upstream paths copied verbatim:
  - `schemas/release-manifest.schema.json` → [`schema/release-manifest.schema.json`](schema/release-manifest.schema.json)
  - `schemas/fixtures/{stable,beta,linux-preview,invalid}/*.json` → [`fixtures/`](fixtures/)
- Upstream docs describing the contract in full:
  [`docs/manifest-schema.md`](https://github.com/Wangnov/codex-app-mirror/blob/main/docs/manifest-schema.md)
  (mirror repo) and [`docs/manifest-contract.md`](../../manifest-contract.md)
  (this repo's own, pre-existing, hand-written description of the same
  contract from the Manager's point of view).

## What consumes this

- `crates/codex-win-engine/tests/mirror_manifest_contract.rs` parses every
  `fixtures/{stable,beta,linux-preview,invalid}/*.json` file with
  `manifest::parse_manifest_for_arch` and asserts the outcome (success with
  specific platform data, or a specific/any failure) matches what the
  fixture's name and the schema say it should be. It also documents two
  fixtures under `fixtures/invalid/` where the *schema* is intentionally
  stricter than the real parser (`downloadable-arch-missing-version`,
  `package-moniker-bad-chars`) -- fail-closed publishing is allowed to be
  more conservative than the bare minimum the parser needs.
- `crates/codex-mac-engine/tests/live_appcast_fixtures.rs` parses a live
  snapshot of both Sparkle feeds the Manager fetches
  (`crates/codex-mac-engine/tests/fixtures/live/appcast-arm64.xml` and
  `appcast-x64.xml`, fetched from `https://codexapp.agentsmirror.com/latest/appcast.xml`
  and `.../appcast-x64.xml` on 2026-09-28). These are separate from the
  release-manifest.json contract above (different file, different format)
  but are part of the same "does the Manager still agree with what the
  mirror actually serves" question.
- `scripts/mirror-manifest-schema.test.mjs` (run by `npm test`) compiles
  `schema/release-manifest.schema.json` with the same ajv version the mirror
  repo uses (`ajv` 8.20.0 + `ajv-formats` 3.0.1, see `package.json`) and
  re-validates every fixture here against it, so the schema and fixture
  files in this directory can never silently drift from each other even if
  someone edits one without the other.
- `.github/workflows/mirror-contract.yml` runs on a daily cron (and
  `workflow_dispatch`) and additionally: fetches the *live* manifest and
  both appcasts and runs the same parsers against them (catching drift in
  production, not just in these frozen fixtures), and diffs
  `schema/release-manifest.schema.json` against the mirror repo's own copy
  on its `main` branch, so this vendored copy doesn't quietly go stale. If
  the mirror repo's `main` branch is ever missing
  `schemas/release-manifest.schema.json` entirely (for example, this
  vendored copy is refreshed from a not-yet-merged mirror-side PR ahead of
  `main`), that comparison step fails loudly with a message explaining why,
  rather than silently passing or failing for the wrong reason -- see
  "Refreshing this vendored copy" below for what to do once the upstream
  change lands.

## Resolved schema gap

Mirror PR [#73](https://github.com/Wangnov/codex-app-mirror/pull/73) rejects
`architectures.x64.downloadable: false` even when the top-level Windows fallback
is complete, matching the Manager parser. The regression fixture now lives in
`fixtures/invalid/` and both the schema and Rust parser reject it through the
regular invalid-fixture tests.

## Refreshing this vendored copy

When the mirror repo's schema or fixtures change:

1. Copy `schemas/release-manifest.schema.json` from the mirror repo's
   `main` branch to [`schema/release-manifest.schema.json`](schema/release-manifest.schema.json)
   here, byte for byte.
2. Copy every file under the mirror repo's
   `schemas/fixtures/{stable,beta,linux-preview,invalid}/` to the matching
   directory under [`fixtures/`](fixtures/) here (add new files, remove ones
   the mirror deleted).
3. Update the "Commit vendored" line above to the new commit hash.
4. Run `npm test -- mirror-manifest-schema` (schema/fixture consistency),
   then `cargo test --manifest-path crates/codex-win-engine/Cargo.toml
   --test mirror_manifest_contract` and `cargo test --manifest-path
   crates/codex-mac-engine/Cargo.toml --all-targets`. A new fixture that
   represents a genuinely new manifest shape (not just a copy of an
   existing shape with different values) usually needs a new case added to
   `mirror_manifest_contract.rs`'s expectation tables -- the existing tests
   will not automatically pick it up, since they assert specific values
   per named fixture rather than iterating the directory blindly.
5. If the change affects a field `manifest.rs` reads (renamed, retyped,
   removed, or a new fallback/requirement), update `manifest.rs` and
   `docs/manifest-contract.md` in the same change, per the mirror's own
   "Versioning rule" in its `docs/manifest-schema.md`.

## Why vendor instead of fetching at test time

Fixture-based contract tests need to run offline, deterministically, and on
every PR (they're wired into `npm test` / `cargo test`, which run in the
default CI job on every push and pull request). Live-fetch verification
against the mirror's actual production endpoints and its `main` branch is a
separate, slower, network-dependent concern, handled by the scheduled
`mirror-contract.yml` workflow instead (see above).
