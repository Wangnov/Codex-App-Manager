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

use codex_mac_engine::parse_appcast;

#[test]
#[ignore = "network: fetched and run only by .github/workflows/mirror-contract.yml"]
fn live_appcasts_parse_for_both_architectures() {
    for (env_var, label) in [
        ("CODEX_MAC_LIVE_APPCAST_ARM64_PATH", "arm64"),
        ("CODEX_MAC_LIVE_APPCAST_X64_PATH", "x64"),
    ] {
        let path = std::env::var(env_var)
            .unwrap_or_else(|_| panic!("set {env_var} before running this test with --ignored"));
        let text = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("failed to read {path} ({label}): {e}"));
        let appcast = parse_appcast(&text)
            .unwrap_or_else(|e| panic!("live {label} appcast failed to parse: {e}"));
        let latest = appcast
            .latest()
            .unwrap_or_else(|| panic!("live {label} appcast has no items"));
        assert!(latest.build > 0, "{label}: build should be > 0");
        assert!(!latest.short_version.is_empty(), "{label}: empty short_version");
        assert!(!latest.full.url.is_empty(), "{label}: empty enclosure url");
        assert!(latest.full.length > 0, "{label}: enclosure length should be > 0");
    }
}
