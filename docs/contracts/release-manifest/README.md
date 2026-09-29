# release-manifest.json contract (vendored)

This directory is a vendored copy of the `release-manifest.json` JSON Schema
contract and its real-sample fixtures, published by the mirror project this
Manager consumes. It exists so Codex App Manager can prove -- in its own CI,
without depending on the mirror repo at test time -- that
`crates/codex-win-engine/src/manifest.rs` (and `codex-mac-engine`'s appcast
parser) still agree with the shapes the mirror actually publishes.

## Source

- Repository: [`Wangnov/codex-app-mirror`](https://github.com/Wangnov/codex-app-mirror)
- Pull request: [#71 "feat(manifest): add machine-checkable schema for
  release-manifest.json"](https://github.com/Wangnov/codex-app-mirror/pull/71)
  (merged to `main` as commit `83273c9cd3c51e1514202f51b3c5afa07a9ec969` on
  2026-09-29)
- Commit vendored: `2503834911a3ceafbaed7c7de67e1060c5a0f5e7` (the tip of PR
  #71's branch, `feat/release-manifest-schema`, immediately before merge --
  byte-for-byte identical to what landed on `main`, confirmed by diff at
  vendoring time)
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

## Known gaps

This contract-test suite found a real mismatch between the vendored schema
(as merged upstream) and the actual Rust parser, which is not yet fixed
upstream:

- **`architectures.x64` with `downloadable: false`, plus a top-level
  fallback, still parses to an error.** `select_architecture()` in
  `crates/codex-win-engine/src/manifest.rs` looks up the requested
  architecture key ("x64" by default) in `sources.windows.architectures`
  *before* anything consults the top-level `version`/`packageMoniker`
  fallback. If that key is present with `downloadable: false`, it returns
  `Err(...)` immediately -- there is no code path where a top-level fallback
  rescues a present-but-non-downloadable per-architecture entry. The
  vendored schema's `windowsSource.anyOf` only encodes the "not
  `downloadable: false`" constraint on its *second* branch (the
  `architectures`-only one, added in PR #71's second review-fix commit,
  `2503834`); its *first* branch (top-level `packageMoniker` + `version`)
  does not also forbid `architectures.x64.downloadable === false`. A
  manifest satisfying only the first branch, with an explicitly
  non-downloadable `architectures.x64`, therefore validates against the
  schema today even though the Manager will always reject it for the
  default x64 update-check path.

  See [`known-gaps/architectures-x64-not-downloadable-with-top-level-fallback.json`](known-gaps/architectures-x64-not-downloadable-with-top-level-fallback.json)
  (a minimal reproduction, kept outside `fixtures/invalid/` specifically
  *because* it currently validates when it shouldn't -- putting it in
  `fixtures/invalid/` would make `scripts/mirror-manifest-schema.test.mjs`'s
  blind per-directory loop fail for a reason that looks like vendoring
  breakage rather than an upstream schema gap), pinned by:
  - `scripts/mirror-manifest-schema.test.mjs`'s dedicated "KNOWN GAP" test
    (currently asserts the schema wrongly validates it; flip to `false` and
    move the fixture into `fixtures/invalid/` once fixed upstream).
  - `crates/codex-win-engine/tests/mirror_manifest_contract.rs::known_upstream_schema_gap_x64_not_downloadable_with_top_level_fallback_still_hard_fails`
    (asserts the real parser rejects it regardless).

  **The fix belongs upstream**, in `Wangnov/codex-app-mirror`: tighten the
  first `anyOf` branch of `windowsSource` to also forbid
  `architectures.x64.downloadable === false` when `architectures.x64` is
  present, the same way the second branch already does. Once that lands and
  this vendored copy is refreshed, delete both of the pinning tests above
  (or, better, watch them start failing -- that's the point) and move the
  fixture into `fixtures/invalid/`.

## Refreshing this vendored copy

When the mirror repo's schema or fixtures change (most recently: any change
after commit `2503834` above):

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
