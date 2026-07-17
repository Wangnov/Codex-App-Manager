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

/// Full live loop against a debuggable Codex on 9345: inject guts-terminal,
/// structurally verify, remove, verify removed. Only proceeds when every
/// renderer is currently stock (never clobbers an active studio session);
/// net effect on the running app is zero.
#[test]
#[ignore = "requires a running Codex with --remote-debugging-port=9345"]
fn live_inject_verify_revert_when_stock() {
    let root = studio_themes().expect("~/codex-theme-studio/themes not found");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let connected =
            codex_theme_engine::cdp::connect_codex_targets(9345, std::time::Duration::from_secs(5))
                .await
                .expect("no verified Codex renderer on 9345");
        println!("connected {} verified target(s)", connected.len());

        for target in &connected {
            let stamp = target
                .session
                .evaluate(codex_theme_engine::payload::CURRENT_STAMP_EXPRESSION)
                .await
                .expect("stamp probe");
            if !stamp.is_null() {
                println!("SKIP: renderer already themed ({stamp}) — not touching it");
                return;
            }
        }

        let built =
            codex_theme_engine::payload::build_payload(&root.join("guts-terminal")).unwrap();
        let verify = codex_theme_engine::payload::verify_expression(
            codex_theme_engine::ENGINE_VERSION,
        )
        .unwrap();
        for target in &connected {
            target.session.evaluate(&built.payload).await.expect("inject");
            let report = target.session.evaluate(&verify).await.expect("verify");
            println!(
                "target {} verify: pass={} themeId={:?}",
                target.probe.title,
                report["pass"],
                report["themeId"]
            );
            assert_eq!(report["installed"], true, "theme must be installed");
            assert_eq!(report["stylePresent"], true, "style must be present");

            target
                .session
                .evaluate(codex_theme_engine::payload::REMOVE_EXPRESSION)
                .await
                .expect("remove");
            let removed = target
                .session
                .evaluate(codex_theme_engine::payload::VERIFY_REMOVED_EXPRESSION)
                .await
                .expect("verify removed");
            assert_eq!(removed, true, "renderer must be stock again");
        }
        println!("live loop OK: inject → verify → revert, renderers back to stock");
    });
}

/// Import verification for a real packed `.codexskin` (path via CODEXSKIN
/// env). Pairs with the studio's `pack` command as the delivery loop's
/// receiving end.
#[test]
#[ignore = "provide CODEXSKIN=/path/to/pkg.codexskin"]
fn imports_env_provided_codexskin() {
    let path = std::env::var("CODEXSKIN").expect("set CODEXSKIN=/path/to/pkg.codexskin");
    let tmp = std::env::temp_dir().join(format!("codexskin-import-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&tmp);
    let summary =
        codex_theme_engine::import::import_codexskin(std::path::Path::new(&path), &tmp)
            .expect("import");
    println!(
        "imported id={} version={:?} preview={:?} verified={:?}",
        summary.id, summary.meta.version, summary.preview, summary.meta.codex_verified
    );
    assert!(summary.meta.version.is_some(), "packed skins carry a version");
    assert!(summary.preview.is_some(), "packed skins carry a cover preview");
    let _ = std::fs::remove_dir_all(&tmp);
}

/// Hot native-settings round trip against a live debuggable Codex: discover
/// the renderer's settings API, read the five managed keys, write the SAME
/// values back (visually a no-op) and re-read to confirm equality. Proves the
/// hot-first apply path end to end without disturbing the user's setup.
#[test]
#[ignore = "requires a running Codex with --remote-debugging-port=9345"]
fn live_hot_settings_same_value_round_trip() {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .expect("runtime");
    runtime.block_on(async {
        let port = 9345;
        assert!(
            codex_theme_engine::cdp::cdp_http_ready(port).await,
            "no CDP endpoint on {port} — launch Codex with --remote-debugging-port"
        );
        let mut targets =
            codex_theme_engine::cdp::connect_codex_targets(port, std::time::Duration::from_secs(8))
                .await
                .expect("connect");
        let session = targets.remove(0).session;
        for extra in targets {
            extra.session.close();
        }

        let before = codex_theme_engine::native_hot::read_snapshot(&session, None)
            .await
            .expect("read snapshot");
        println!(
            "live settings: appearance={:?} darkId={:?} lightId={:?}",
            before.appearance_theme, before.dark_code_id, before.light_code_id
        );
        let entries = codex_theme_engine::native_hot::snapshot_write_entries(&before);
        assert!(!entries.is_empty(), "effective reads should yield values");
        codex_theme_engine::native_hot::write_values(&session, &entries, None)
            .await
            .expect("same-value write");
        let after = codex_theme_engine::native_hot::read_snapshot(&session, None)
            .await
            .expect("re-read snapshot");
        assert_eq!(
            serde_json::to_value(&before).unwrap(),
            serde_json::to_value(&after).unwrap(),
            "same-value write must not change effective settings"
        );
        session.close();
        println!("live hot settings round trip OK");
    });
}
