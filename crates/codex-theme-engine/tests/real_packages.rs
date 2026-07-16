//! Real-package smoke test against a local codex-theme-studio checkout.
//! Ignored by default (CI has no checkout); run manually with
//! `cargo test -p codex-theme-engine -- --ignored`.

use std::path::PathBuf;

fn studio_themes() -> Option<PathBuf> {
    let root = PathBuf::from(std::env::var("HOME").ok()?).join("codex-theme-studio/themes");
    root.is_dir().then_some(root)
}

#[test]
#[ignore = "requires ~/codex-theme-studio (developer machine only)"]
fn studio_packages_load_and_build() {
    let root = studio_themes().expect("~/codex-theme-studio/themes not found");
    let listed = codex_theme_engine::theme::list_themes(&root);
    let ids: Vec<&str> = listed.iter().map(|t| t.id.as_str()).collect();
    assert!(ids.contains(&"guts-terminal"), "listed: {ids:?}");
    assert!(ids.contains(&"asuka-eva02"), "listed: {ids:?}");

    for summary in &listed {
        let theme = codex_theme_engine::theme::load_theme(&summary.dir).expect("load");
        assert!(
            theme.codex_theme.is_some(),
            "{}: native block expected",
            summary.id
        );
        assert!(
            !theme.config.colors.is_empty(),
            "{}: colors expected",
            summary.id
        );
        let built = codex_theme_engine::payload::build_payload(&summary.dir).expect("payload");
        assert!(
            built.asset_count >= 30,
            "{}: {} assets",
            summary.id,
            built.asset_count
        );
        assert!(
            built.payload_bytes > 1_000_000,
            "{}: suspiciously small payload",
            summary.id
        );
        assert!(
            !built.payload.contains("__CTS_"),
            "{}: unsubstituted placeholder",
            summary.id
        );
        println!(
            "{}: payload {:.1} MB, {} assets, stamp {}",
            summary.id,
            built.payload_bytes as f64 / 1e6,
            built.asset_count,
            built.stamp
        );
    }
}
