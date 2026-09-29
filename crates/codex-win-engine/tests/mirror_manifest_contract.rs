//! Contract tests proving `crates/codex-win-engine/src/manifest.rs` actually
//! parses the real `release-manifest.json` shapes vendored from
//! `Wangnov/codex-app-mirror` (see
//! `docs/contracts/release-manifest/README.md` for provenance).
//!
//! Every fixture here is a byte-for-byte copy of a fixture from that mirror
//! repo's `schemas/fixtures/` tree (PR #71, commit `2503834`). If the mirror
//! ever changes the manifest shape it emits without updating the schema, or
//! updates the schema without re-copying it here, `mirror_manifest_schema_drift`
//! below (and `scripts/mirror-manifest-contract.test.mjs`) catches it. This
//! file proves the other half of the contract: that the *parser* actually
//! agrees with what the schema (and the fixtures) say is a valid manifest.
//!
//! Kept engine-local (no `src-tauri` dependency) so it survives the pending
//! Cargo workspace migration (Codex-App-Manager#374).

use codex_win_engine::manifest::parse_manifest_for_arch;

// ---------------------------------------------------------------------------
// Fixture contents, embedded at compile time from the vendored copies.
// ---------------------------------------------------------------------------

const ARCH_X64_ONLY: &str = include_str!(
    "../../../docs/contracts/release-manifest/fixtures/stable/architectures-x64-only-no-top-level-fallback.json"
);
const ARM64_DRIFT: &str = include_str!(
    "../../../docs/contracts/release-manifest/fixtures/stable/fixture-prepare-arm64-drift.json"
);
const ARM64_PRESERVED: &str = include_str!(
    "../../../docs/contracts/release-manifest/fixtures/stable/fixture-prepare-arm64-preserved.json"
);
const BACKEND_UNAVAILABLE: &str = include_str!(
    "../../../docs/contracts/release-manifest/fixtures/stable/fixture-prepare-backend-unavailable.json"
);
const PREPARE_COMPLETE: &str = include_str!(
    "../../../docs/contracts/release-manifest/fixtures/stable/fixture-prepare-complete.json"
);
const LEGACY_V2_TOP_LEVEL: &str = include_str!(
    "../../../docs/contracts/release-manifest/fixtures/stable/legacy-schema-v2-top-level-only.json"
);
const LIVE_LATEST: &str =
    include_str!("../../../docs/contracts/release-manifest/fixtures/stable/live-latest.json");
const RELEASE_26_903: &str = include_str!(
    "../../../docs/contracts/release-manifest/fixtures/stable/release-26.903.71938.json"
);
const RELEASE_26_917_PRERELEASE: &str = include_str!(
    "../../../docs/contracts/release-manifest/fixtures/stable/release-26.917.61114-prerelease.json"
);
const RELEASE_26_924: &str = include_str!(
    "../../../docs/contracts/release-manifest/fixtures/stable/release-26.924.22138.json"
);

const BETA_SCHEMA_V1: &str = include_str!(
    "../../../docs/contracts/release-manifest/fixtures/beta/beta-emergency-schema-v1.json"
);

const LINUX_FINALIZE: &str = include_str!(
    "../../../docs/contracts/release-manifest/fixtures/linux-preview/fixture-linux-preview-finalize.json"
);
const LINUX_PREVIEW_RELEASE: &str = include_str!(
    "../../../docs/contracts/release-manifest/fixtures/linux-preview/linux-preview-26.803.81509.json"
);

const INVALID_ARCH_X64_MISSING: &str = include_str!(
    "../../../docs/contracts/release-manifest/fixtures/invalid/architectures-x64-missing.json"
);
const INVALID_X64_NOT_DOWNLOADABLE: &str = include_str!(
    "../../../docs/contracts/release-manifest/fixtures/invalid/architectures-x64-not-downloadable-no-fallback.json"
);
const INVALID_CONTENT_LENGTH_STRING: &str = include_str!(
    "../../../docs/contracts/release-manifest/fixtures/invalid/content-length-as-string.json"
);
const INVALID_DOWNLOADABLE_ARCH_MISSING_VERSION: &str = include_str!(
    "../../../docs/contracts/release-manifest/fixtures/invalid/downloadable-arch-missing-version.json"
);
const INVALID_MISSING_SCHEMA_VERSION: &str = include_str!(
    "../../../docs/contracts/release-manifest/fixtures/invalid/missing-schema-version.json"
);
const INVALID_MISSING_WINDOWS_SOURCE: &str = include_str!(
    "../../../docs/contracts/release-manifest/fixtures/invalid/missing-windows-source.json"
);
const INVALID_NO_PACKAGE_MONIKER: &str = include_str!(
    "../../../docs/contracts/release-manifest/fixtures/invalid/no-package-moniker-anywhere.json"
);
const INVALID_PACKAGE_MONIKER_BAD_CHARS: &str = include_str!(
    "../../../docs/contracts/release-manifest/fixtures/invalid/package-moniker-bad-chars.json"
);
const INVALID_SCHEMA_VERSION_NOT_INTEGER: &str = include_str!(
    "../../../docs/contracts/release-manifest/fixtures/invalid/schema-version-not-integer.json"
);

// ---------------------------------------------------------------------------
// Every real (schema-valid) stable-channel fixture must parse for the
// Manager's default x64 update-check path, yielding the exact platform data
// the fixture carries.
// ---------------------------------------------------------------------------

struct ExpectedX64 {
    name: &'static str,
    text: &'static str,
    package_version: &'static str,
    package_moniker: &'static str,
    download_architecture: Option<&'static str>,
}

#[test]
fn stable_fixtures_parse_for_x64_default_update_path() {
    let cases = [
        ExpectedX64 {
            name: "architectures-x64-only-no-top-level-fallback",
            text: ARCH_X64_ONLY,
            package_version: "1.2.3.4",
            package_moniker: "OpenAI.Codex_1.2.3.4_x64__2p2nqsd0c76g0",
            download_architecture: Some("x64"),
        },
        ExpectedX64 {
            name: "fixture-prepare-arm64-drift",
            text: ARM64_DRIFT,
            package_version: "1.2.3.4",
            package_moniker: "OpenAI.Codex_1.2.3.4_x64__2p2nqsd0c76g0",
            download_architecture: Some("x64"),
        },
        ExpectedX64 {
            name: "fixture-prepare-arm64-preserved",
            text: ARM64_PRESERVED,
            package_version: "1.2.3.4",
            package_moniker: "OpenAI.Codex_1.2.3.4_x64__2p2nqsd0c76g0",
            download_architecture: Some("x64"),
        },
        ExpectedX64 {
            name: "fixture-prepare-backend-unavailable",
            text: BACKEND_UNAVAILABLE,
            package_version: "1.2.3.4",
            package_moniker: "OpenAI.Codex_1.2.3.4_x64__2p2nqsd0c76g0",
            download_architecture: Some("x64"),
        },
        ExpectedX64 {
            name: "fixture-prepare-complete",
            text: PREPARE_COMPLETE,
            package_version: "1.2.3.4",
            package_moniker: "OpenAI.Codex_1.2.3.4_x64__2p2nqsd0c76g0",
            download_architecture: Some("x64"),
        },
        ExpectedX64 {
            name: "legacy-schema-v2-top-level-only",
            text: LEGACY_V2_TOP_LEVEL,
            package_version: "26.602.3474.0",
            package_moniker: "OpenAI.Codex_26.602.3474.0_x64__2p2nqsd0c76g0",
            // No `architectures` map at all on this legacy schemaVersion-2
            // shape: the parser falls back to the top-level fields and never
            // selects a per-architecture entry.
            download_architecture: None,
        },
        ExpectedX64 {
            name: "live-latest",
            text: LIVE_LATEST,
            package_version: "26.924.2738.0",
            package_moniker: "OpenAI.Codex_26.924.2738.0_x64__2p2nqsd0c76g0",
            download_architecture: Some("x64"),
        },
        ExpectedX64 {
            name: "release-26.903.71938",
            text: RELEASE_26_903,
            package_version: "26.903.9818.0",
            package_moniker: "OpenAI.Codex_26.903.9818.0_x64__2p2nqsd0c76g0",
            download_architecture: Some("x64"),
        },
        ExpectedX64 {
            name: "release-26.917.61114-prerelease",
            text: RELEASE_26_917_PRERELEASE,
            package_version: "26.917.6896.0",
            package_moniker: "OpenAI.Codex_26.917.6896.0_x64__2p2nqsd0c76g0",
            download_architecture: Some("x64"),
        },
        ExpectedX64 {
            name: "release-26.924.22138",
            text: RELEASE_26_924,
            package_version: "26.924.2738.0",
            package_moniker: "OpenAI.Codex_26.924.2738.0_x64__2p2nqsd0c76g0",
            download_architecture: Some("x64"),
        },
    ];

    for case in cases {
        let release = parse_manifest_for_arch(case.text, Some("x64"))
            .unwrap_or_else(|e| panic!("{}: expected Ok, got {e}", case.name));
        assert_eq!(
            release.package_version, case.package_version,
            "{}: package_version",
            case.name
        );
        assert_eq!(
            release.package_moniker, case.package_moniker,
            "{}: package_moniker",
            case.name
        );
        assert_eq!(
            release.download_architecture.as_deref(),
            case.download_architecture,
            "{}: download_architecture",
            case.name
        );
    }
}

/// arm64 requests must follow each fixture's own documented scenario:
/// rollout-drift (downloadable:false) hard-fails, a preserved-from-latest
/// entry (downloadable:true, just an older/newer build) still succeeds.
#[test]
fn arm64_request_matches_each_fixtures_documented_scenario() {
    let err = parse_manifest_for_arch(ARM64_DRIFT, Some("arm64")).unwrap_err();
    assert!(
        err.to_string()
            .contains("Windows arm64 package is not available"),
        "unexpected error: {err}"
    );

    let ok = parse_manifest_for_arch(ARM64_PRESERVED, Some("arm64")).unwrap();
    assert_eq!(ok.package_version, "1.2.4.4");
    assert_eq!(
        ok.package_moniker,
        "OpenAI.Codex_1.2.4.4_arm64__2p2nqsd0c76g0"
    );
    assert_eq!(ok.download_architecture.as_deref(), Some("arm64"));
}

/// Codex App Manager only ever fetches the stable channel's
/// `/latest/manifest` (see `docs/manifest-contract.md` and the upstream
/// `docs/manifest-schema.md`). This real, still-published schemaVersion-1
/// beta fixture predates the Manager's own minimum (`schemaVersion >= 2`)
/// and is kept in the mirror's fixture set only to prove the *schema* still
/// accepts historical GitHub Release assets as structurally sound. Pin that
/// the parser correctly still refuses it, so nobody "fixes" this by quietly
/// lowering `schemaVersion >= 2` in `manifest.rs`.
#[test]
fn beta_channel_fixture_is_intentionally_unparseable_by_the_stable_only_manager() {
    let err = parse_manifest_for_arch(BETA_SCHEMA_V1, Some("x64")).unwrap_err();
    assert!(
        err.to_string().contains("unsupported schemaVersion"),
        "unexpected error: {err}"
    );
}

/// linux-preview manifests have no `sources.windows` at all (Linux is not a
/// supported install target); the Windows parser must reject them outright
/// rather than silently returning nonsense.
#[test]
fn linux_preview_fixtures_are_not_consumed_by_the_windows_parser() {
    for (name, text) in [
        ("fixture-linux-preview-finalize", LINUX_FINALIZE),
        ("linux-preview-26.803.81509", LINUX_PREVIEW_RELEASE),
    ] {
        if let Ok(release) = parse_manifest_for_arch(text, Some("x64")) {
            panic!("{name}: expected an Err (no sources.windows), got Ok({release:?})");
        }
    }
}

// ---------------------------------------------------------------------------
// Fixtures the vendored schema rejects (`fixtures/invalid/`) must also fail
// the real parser -- proving the schema isn't rejecting shapes the Manager
// would in fact handle fine.
// ---------------------------------------------------------------------------

#[test]
fn invalid_fixtures_that_the_parser_also_rejects() {
    // Fixtures whose rejection reason is a specific, stable manifest.rs
    // error message worth pinning.
    let with_message: &[(&str, &str, &str)] = &[
        (
            "architectures-x64-missing",
            INVALID_ARCH_X64_MISSING,
            "missing Windows version",
        ),
        (
            "architectures-x64-not-downloadable-no-fallback",
            INVALID_X64_NOT_DOWNLOADABLE,
            "Windows x64 package is not available",
        ),
        (
            "no-package-moniker-anywhere",
            INVALID_NO_PACKAGE_MONIKER,
            "missing Windows packageMoniker",
        ),
    ];
    for (name, text, expect_contains) in with_message {
        match parse_manifest_for_arch(text, Some("x64")) {
            Ok(release) => panic!("{name}: expected Err, got Ok({release:?})"),
            Err(e) => assert!(
                e.to_string().contains(expect_contains),
                "{name}: expected error containing {expect_contains:?}, got {e}"
            ),
        }
    }

    // Fixtures that fail for structural/JSON-typing reasons (the message
    // text is serde-specific and not worth pinning); just prove they don't
    // parse at all.
    let without_message: &[(&str, &str)] = &[
        ("content-length-as-string", INVALID_CONTENT_LENGTH_STRING),
        ("missing-schema-version", INVALID_MISSING_SCHEMA_VERSION),
        ("missing-windows-source", INVALID_MISSING_WINDOWS_SOURCE),
        (
            "schema-version-not-integer",
            INVALID_SCHEMA_VERSION_NOT_INTEGER,
        ),
    ];
    for (name, text) in without_message {
        if let Ok(release) = parse_manifest_for_arch(text, Some("x64")) {
            panic!("{name}: expected Err, got Ok({release:?})");
        }
    }
}

/// These two fixtures live under the vendored `fixtures/invalid/` (the
/// upstream JSON Schema deliberately rejects them) but the real Rust parser
/// still accepts them today, via fallbacks/lenience the schema does not rely
/// on:
///   - `downloadable-arch-missing-version`: the x64 architecture entry omits
///     `version`, but `parse_manifest_for_arch` falls back to the top-level
///     `sources.windows.version` for the *version* field even when a
///     per-architecture entry was selected (it does the same for
///     `packageMoniker` only when no architecture entry is selected).
///   - `package-moniker-bad-chars`: the parser never validates
///     `packageMoniker`'s character set; it is stored and returned as-is.
///
/// This is intentional (fail-closed publishing is safer than the bare
/// minimum the parser needs), but pinning it here means a future change
/// that makes the parser *also* reject these shapes -- or stops falling
/// back the way this test expects -- is a visible, deliberate decision
/// instead of silent drift between the two repos.
#[test]
fn invalid_fixtures_where_the_schema_is_deliberately_stricter_than_the_parser() {
    let release = parse_manifest_for_arch(INVALID_DOWNLOADABLE_ARCH_MISSING_VERSION, Some("x64"))
        .expect("parser falls back to the top-level `version` when the x64 entry omits it");
    assert_eq!(release.package_version, "26.924.2738.0");

    let release = parse_manifest_for_arch(INVALID_PACKAGE_MONIKER_BAD_CHARS, Some("x64"))
        .expect("parser does not validate packageMoniker's character set");
    assert_eq!(release.package_moniker, "not a valid moniker!!");
}

/// KNOWN UPSTREAM SCHEMA GAP (tracked, not yet fixed in
/// `Wangnov/codex-app-mirror`; see
/// `docs/contracts/release-manifest/README.md`'s "Known gaps" section).
///
/// Unlike the two cases above, this is the *dangerous* direction: the
/// vendored schema currently **validates** a manifest that
/// `select_architecture()` will **always** hard-reject for the default x64
/// update-check path, regardless of any top-level fallback.
///
/// `select_architecture()` looks up the requested key ("x64" by default) in
/// `architectures` *before* anything ever consults the top-level
/// `version`/`packageMoniker` fallback. If that key exists with
/// `downloadable: false`, it returns `Err(...)` immediately -- there is no
/// code path where a present-but-non-downloadable `architectures.x64` entry
/// is rescued by a top-level fallback. The schema's `windowsSource.anyOf`
/// only encodes this `not downloadable:false` constraint on its *second*
/// branch (the `architectures`-only one); its *first* branch (top-level
/// `packageMoniker` + `version`) does not also forbid
/// `architectures.x64.downloadable === false`, so a manifest satisfying
/// the first branch alone still validates even though the real parser
/// rejects it outright.
///
/// This was NOT introduced by this vendoring pass -- it reproduces against
/// the schema exactly as merged upstream (`Wangnov/codex-app-mirror` main
/// commit `83273c9`, the tip of PR #71) -- but it is exactly the class of
/// bug this contract-test suite exists to catch, so it is pinned here
/// rather than silently left for the next person to rediscover. The
/// upstream fix is to also forbid `downloadable: false` on
/// `architectures.x64` (when present) in the first `anyOf` branch, the same
/// way the second branch already does.
#[test]
fn known_upstream_schema_gap_x64_not_downloadable_with_top_level_fallback_still_hard_fails() {
    // Equivalent in shape to `fixtures/invalid/architectures-x64-not-downloadable-no-fallback.json`
    // (which correctly fails schema validation) except this ALSO carries a
    // fully-populated top-level `version` + `packageMoniker` fallback. Per
    // the upstream schema's first anyOf branch, this currently validates;
    // per `select_architecture()`, it always fails to parse anyway.
    const MANIFEST_WITH_TOP_LEVEL_FALLBACK_AND_NON_DOWNLOADABLE_X64: &str = r#"{
        "schemaVersion": 5,
        "sources": {
            "windows": {
                "version": "1.2.3.4",
                "packageMoniker": "OpenAI.Codex_1.2.3.4_x64__2p2nqsd0c76g0",
                "architectures": {
                    "x64": { "architecture": "x64", "status": "catalog-only", "downloadable": false }
                }
            },
            "macos": {
                "arm64": { "bundleShortVersion": "1.2.3", "bundleVersion": "5" },
                "x64": { "bundleShortVersion": "1.2.3", "bundleVersion": "5" }
            }
        }
    }"#;

    let err = parse_manifest_for_arch(
        MANIFEST_WITH_TOP_LEVEL_FALLBACK_AND_NON_DOWNLOADABLE_X64,
        Some("x64"),
    )
    .expect_err(
        "select_architecture() always errors on a present, non-downloadable \
         x64 entry -- it never falls back to the top-level fields, so this \
         must stay an Err even though a schema-valid-today manifest of this \
         exact shape exists",
    );
    assert!(
        err.to_string()
            .contains("Windows x64 package is not available"),
        "unexpected error: {err}"
    );
}
