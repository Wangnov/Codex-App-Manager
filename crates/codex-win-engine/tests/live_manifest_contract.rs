//! Live-fetch contract check.
//!
//! This test is `#[ignore]`d by default and is meant to run only from
//! `.github/workflows/mirror-contract.yml` (daily cron + `workflow_dispatch`),
//! which fetches `https://codexapp.agentsmirror.com/latest/manifest` to a
//! file and points `CODEX_MIRROR_LIVE_MANIFEST_PATH` at it before running
//! `cargo test -- --ignored`.
//!
//! It proves the Manager's parser still accepts whatever the mirror is
//! serving *right now*, complementing the frozen, offline fixtures exercised
//! by `mirror_manifest_contract.rs`.

use codex_win_engine::manifest::parse_manifest_for_arch;

#[test]
#[ignore = "network: fetched and run only by .github/workflows/mirror-contract.yml"]
fn live_manifest_parses_for_the_default_x64_update_check_path() {
    let path = std::env::var("CODEX_MIRROR_LIVE_MANIFEST_PATH").expect(
        "set CODEX_MIRROR_LIVE_MANIFEST_PATH to a file containing the live \
         https://codexapp.agentsmirror.com/latest/manifest response before \
         running this test with --ignored",
    );
    let text =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("failed to read {path}: {e}"));

    let x64 = parse_manifest_for_arch(&text, Some("x64"))
        .expect("live manifest must parse for the default x64 update-check path");
    assert!(!x64.package_version.is_empty(), "empty package_version");
    assert!(!x64.package_moniker.is_empty(), "empty package_moniker");

    // arm64 may legitimately be temporarily unavailable (rollout drift or a
    // catalog-only state) -- that specific, well-understood failure is fine;
    // any other failure (a JSON/shape error the schema should have caught
    // before publishing) is not.
    match parse_manifest_for_arch(&text, Some("arm64")) {
        Ok(arm64) => {
            assert!(!arm64.package_version.is_empty(), "empty arm64 package_version");
            assert!(!arm64.package_moniker.is_empty(), "empty arm64 package_moniker");
        }
        Err(e) => {
            let msg = e.to_string();
            assert!(
                msg.contains("Windows arm64 package is not available"),
                "unexpected arm64 parse failure on the live manifest: {msg}"
            );
        }
    }
}
