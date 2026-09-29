//! Contract fixtures: real appcast feeds fetched from the live mirror
//! (`https://codexapp.agentsmirror.com/latest/appcast.xml` for arm64 and
//! `.../latest/appcast-x64.xml` for the Intel channel), captured on
//! 2026-09-28. See `docs/contracts/release-manifest/README.md` for how and
//! why these were vendored, and `.github/workflows/mirror-contract.yml` for
//! the CI job that re-fetches the live feeds and checks for drift.
//!
//! `tests/parse_and_plan.rs` already covers `fixtures/appcast.xml` (a
//! hand-assembled arm64 sample exercising delta selection edge cases). This
//! file adds real, currently-live data for *both* architectures, since the
//! Manager fetches two independent Sparkle feeds
//! (`src-tauri/src/app/mac_update.rs::PROD_ARM64_APPCAST` /
//! `PROD_X64_APPCAST`) and nothing previously exercised the x64 feed.

use codex_mac_engine::parse_appcast;

const LIVE_ARM64: &str = include_str!("fixtures/live/appcast-arm64.xml");
const LIVE_X64: &str = include_str!("fixtures/live/appcast-x64.xml");

#[test]
fn live_arm64_appcast_parses_with_expected_latest_build() {
    let appcast = parse_appcast(LIVE_ARM64).expect("live arm64 appcast should parse");
    let latest = appcast.latest().expect("at least one item");

    assert_eq!(latest.build, 11645);
    assert_eq!(latest.short_version, "26.924.22138");
    assert_eq!(latest.full.length, 653_348_759);
    assert!(latest.full.ed_signature.is_some());
    assert!(
        latest.full.url.contains("Codex-darwin-arm64-26.924.22138.zip"),
        "unexpected enclosure url: {}",
        latest.full.url
    );
    // The live feed ships five recent binary deltas.
    assert_eq!(latest.deltas.len(), 5);
    assert!(latest.deltas.iter().any(|d| d.from_build == 11431));
}

#[test]
fn live_x64_appcast_parses_with_expected_latest_build() {
    let appcast = parse_appcast(LIVE_X64).expect("live x64 appcast should parse");
    let latest = appcast.latest().expect("at least one item");

    assert_eq!(latest.build, 11645);
    assert_eq!(latest.short_version, "26.924.22138");
    assert_eq!(latest.full.length, 640_430_436);
    assert!(latest.full.ed_signature.is_some());
    assert!(
        latest.full.url.contains("Codex-darwin-x64-26.924.22138.zip"),
        "unexpected enclosure url: {}",
        latest.full.url
    );
    assert_eq!(latest.deltas.len(), 5);
    assert!(latest.deltas.iter().any(|d| d.from_build == 11431));
}

/// The two feeds are for different architectures but the same Codex release,
/// so they should always agree on build number and short version -- if they
/// ever don't, something is wrong with how the mirror generates them.
#[test]
fn live_arm64_and_x64_appcasts_agree_on_the_published_version() {
    let arm64 = parse_appcast(LIVE_ARM64).unwrap();
    let x64 = parse_appcast(LIVE_X64).unwrap();
    let arm64_latest = arm64.latest().unwrap();
    let x64_latest = x64.latest().unwrap();

    assert_eq!(arm64_latest.build, x64_latest.build);
    assert_eq!(arm64_latest.short_version, x64_latest.short_version);
}
