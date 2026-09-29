//! Live-fetch contract check.
//!
//! This test is `#[ignore]`d by default and is meant to run only from
//! `.github/workflows/mirror-contract.yml` (daily cron + `workflow_dispatch`),
//! which fetches both live Sparkle feeds
//! (`https://codexapp.agentsmirror.com/latest/appcast.xml` and
//! `.../appcast-x64.xml`) to files and points
//! `CODEX_MAC_LIVE_APPCAST_ARM64_PATH` / `CODEX_MAC_LIVE_APPCAST_X64_PATH`
//! at them before running `cargo test -- --ignored`.
//!
//! It proves the appcast parser still accepts whatever the mirror is serving
//! *right now*, complementing the frozen fixtures in
//! `tests/fixtures/live/{appcast-arm64,appcast-x64}.xml` exercised by
//! `live_appcast_fixtures.rs`.

use codex_mac_engine::{parse_appcast, AppcastItem};

fn read_live_appcast(env_var: &str) -> AppcastItem {
    let path = std::env::var(env_var)
        .unwrap_or_else(|_| panic!("set {env_var} before running this test with --ignored"));
    let text = std::fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read {path}: {e}"));
    let appcast = parse_appcast(&text)
        .unwrap_or_else(|e| panic!("{env_var}: live appcast failed to parse: {e}"));
    appcast
        .latest()
        .unwrap_or_else(|| panic!("{env_var}: live appcast has no items"))
        .clone()
}

#[test]
#[ignore = "network: fetched and run only by .github/workflows/mirror-contract.yml"]
fn live_appcasts_parse_for_both_architectures() {
    for (env_var, label) in [
        ("CODEX_MAC_LIVE_APPCAST_ARM64_PATH", "arm64"),
        ("CODEX_MAC_LIVE_APPCAST_X64_PATH", "x64"),
    ] {
        let latest = read_live_appcast(env_var);
        assert!(latest.build > 0, "{label}: build should be > 0");
        assert!(!latest.short_version.is_empty(), "{label}: empty short_version");
        assert!(!latest.full.url.is_empty(), "{label}: empty enclosure url");
        assert!(latest.full.length > 0, "{label}: enclosure length should be > 0");
        // src-tauri/src/app/mac_update.rs (both the direct-update and the
        // delta-apply paths) hard-rejects a full enclosure with no
        // `edSignature` ("appcast enclosure missing edSignature" /
        // "appcast full enclosure missing edSignature") before ever
        // attempting a download. A live feed that dropped the signature
        // would still satisfy every assertion above, so check it
        // explicitly -- this is exactly the kind of production drift this
        // scheduled workflow exists to catch, not just "does it parse".
        assert!(
            latest.full.ed_signature.as_deref().is_some_and(|s| !s.is_empty()),
            "{label}: latest full enclosure has no edSignature; \
             src-tauri's update paths would reject it"
        );
    }
}

/// The two feeds are for different architectures but the same Codex release,
/// so their latest items should always agree on build number and short
/// version. Read both feeds independently (rather than reusing
/// `live_appcasts_parse_for_both_architectures`'s per-architecture loop) so
/// this specific cross-feed invariant is checked even when each feed passes
/// its own assertions individually.
#[test]
#[ignore = "network: fetched and run only by .github/workflows/mirror-contract.yml"]
fn live_appcasts_agree_on_the_published_version() {
    let arm64 = read_live_appcast("CODEX_MAC_LIVE_APPCAST_ARM64_PATH");
    let x64 = read_live_appcast("CODEX_MAC_LIVE_APPCAST_X64_PATH");

    assert_eq!(
        arm64.build, x64.build,
        "arm64 and x64 live appcasts disagree on the latest build number"
    );
    assert_eq!(
        arm64.short_version, x64.short_version,
        "arm64 and x64 live appcasts disagree on the latest short version"
    );
}
