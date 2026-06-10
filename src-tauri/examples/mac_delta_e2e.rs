//! `mac_delta_e2e` — REAL end-to-end macOS **delta** update against the live
//! `/Applications/Codex.app`, through the exact production path
//! (`plan_macos_update` → `perform_macos_update`) with the vendored
//! `BinaryDelta` from `src-tauri/resources/` (the same file release bundles
//! ship). Lives in `examples/` so it is never built into the .app (see the
//! Cargo.toml note); `win_real_smoke.rs` is the Windows counterpart.
//!
//! DESTRUCTIVE — it gracefully quits Codex and atomically swaps the installed
//! bundle (that IS the behavior under test), so it refuses to run without
//! `CAM_MAC_DELTA_E2E=1`. The installed build must be one the appcast still
//! publishes a delta FROM (downgrade first by unpacking an older full zip):
//!
//! ```sh
//! CAM_MAC_DELTA_E2E=1 cargo run --example mac_delta_e2e
//! ```
//!
//! Pass/fail is decided by:
//!   * the plan strategy is `delta-from-<installed>` — NOT full;
//!   * every download progress callback reports the DELTA's byte size as its
//!     total — the moment a full-package fallback kicks in, the total flips to
//!     the full size and the run fails;
//!   * the perform report is verified, not rolled back, strategy still delta
//!     (a fallback rewrites it to `full (…回退…)`);
//!   * the installed bundle ends at the appcast's latest build.

use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicU64, Ordering};

use codex_app_manager_lib::app::mac_update::{
    perform_macos_update, plan_macos_update, DownloadProgress, PerformExpectation,
};
use codex_mac_engine::UpdateStrategy;

fn fail(msg: &str) -> ExitCode {
    eprintln!("FAIL: {msg}");
    ExitCode::FAILURE
}

fn main() -> ExitCode {
    if std::env::var("CAM_MAC_DELTA_E2E").as_deref() != Ok("1") {
        eprintln!(
            "refusing to run: set CAM_MAC_DELTA_E2E=1 — this quits Codex and swaps \
             /Applications/Codex.app (the real production update)"
        );
        return ExitCode::FAILURE;
    }

    // The vendored helper exactly as the release bundle ships it.
    let tool = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("resources/BinaryDelta");
    if !tool.is_file() {
        return fail("missing src-tauri/resources/BinaryDelta — run scripts/vendor-binary-delta.sh");
    }

    let report = match plan_macos_update(None) {
        Ok(r) => r,
        Err(e) => return fail(&format!("plan: {e}")),
    };
    let Some(installed) = report.installed else {
        return fail("no Codex installed");
    };
    let Some(plan) = report.plan else {
        return fail("appcast had no items");
    };
    println!(
        "installed: build {} ({}) at {}",
        installed.build, installed.short_version, installed.path
    );
    println!(
        "plan: latest build {} ({}) | download {} B | full {} B | saves {:.1}%",
        plan.latest_build, plan.latest_short_version, plan.download_size, plan.full_size, plan.savings_pct
    );

    if plan.up_to_date {
        return fail("already up to date — downgrade Codex to a delta-covered build first");
    }
    let UpdateStrategy::Delta { from_build } = plan.strategy.clone() else {
        return fail(&format!(
            "plan strategy is FULL — the appcast has no delta from build {}",
            installed.build
        ));
    };
    if from_build != installed.build {
        return fail("delta from_build != installed build");
    }
    let delta_size = plan.download_size;
    if delta_size == 0 || delta_size * 2 >= plan.full_size {
        return fail("delta is not meaningfully smaller than the full package — wrong plan?");
    }

    // Every callback's `total` must equal the delta size; a fallback download
    // (full zip) would report the full size and trip this.
    let bad_total = AtomicU64::new(0);
    let last_logged = AtomicU64::new(0);
    let progress = |p: DownloadProgress| {
        if p.total != delta_size {
            bad_total.store(p.total, Ordering::SeqCst);
        }
        // Log at most every ~4 MiB so a slow link doesn't spam.
        let prev = last_logged.load(Ordering::SeqCst);
        if p.downloaded >= prev + 4 * 1024 * 1024 || p.downloaded == p.total {
            last_logged.store(p.downloaded, Ordering::SeqCst);
            println!("  ↓ {}/{} bytes from {}", p.downloaded, p.total, p.source);
        }
    };

    let perf = match perform_macos_update(
        Some(tool),
        PerformExpectation {
            from_build: installed.build,
            to_build: plan.latest_build,
            install_path: installed.path.clone(),
        },
        &progress,
    ) {
        Ok(r) => r,
        Err(e) => return fail(&format!("perform: {e}")),
    };

    println!(
        "perform: strategy={} verified={} rolled_back={} relaunched={} warning={:?}",
        perf.strategy, perf.verified, perf.rolled_back, perf.relaunched, perf.warning
    );
    println!("perform message: {}", perf.message);

    let expected_strategy = format!("delta-from-{from_build}");
    if perf.strategy != expected_strategy {
        return fail(&format!(
            "strategy was '{}' (expected '{expected_strategy}') — delta fell back to full",
            perf.strategy
        ));
    }
    if !perf.verified || perf.rolled_back {
        return fail("update not verified or rolled back");
    }
    let bad = bad_total.load(Ordering::SeqCst);
    if bad != 0 {
        return fail(&format!(
            "a download reported total {bad} B (expected the delta's {delta_size} B) — full package was fetched"
        ));
    }

    // Reality check on disk: the installed bundle must now BE the new build.
    match plan_macos_update(None) {
        Ok(after) => match after.installed {
            Some(i) if i.build == plan.latest_build => {
                println!(
                    "OK — delta update applied on the real install: build {} → {} ({} B delta, full would be {} B)",
                    from_build, i.build, delta_size, plan.full_size
                );
                ExitCode::SUCCESS
            }
            Some(i) => fail(&format!(
                "installed build is {} after update (expected {})",
                i.build, plan.latest_build
            )),
            None => fail("no Codex detected after update"),
        },
        Err(e) => fail(&format!("post-update plan: {e}")),
    }
}
