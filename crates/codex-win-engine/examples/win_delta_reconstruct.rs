//! Example / manual verification harness for the block-level delta engine
//! (`crate::delta`). NOT bundled into the Codex App Manager application --
//! see the comment at the top of `src-tauri/Cargo.toml` for why dev/test
//! harnesses live under a crate's `examples/` directory rather than
//! `src/bin/`: `cargo build`/`cargo run` (without `--example`) never
//! produces this binary, and Tauri's bundler never sees it.
//!
//! Reconstructs a real new MSIX from a local base file plus the new
//! package's URL, verifies the result's SHA-256 against an expected value,
//! and prints the numbers the delta feasibility report and this crate's PR
//! both care about: bytes actually fetched over the network, how many
//! requests that took, and whether the reconstructed file is byte-identical
//! to the real release (SHA-256 match).
//!
//! Usage:
//!   cargo run -p codex-win-engine --example win_delta_reconstruct -- \
//!     [--gap-kib N] [--min-savings PCT] \
//!     <base-msix-path> <new-package-url> <expected-sha256> [dest-path]
//!
//! `--gap-kib` is the range-coalescing gap (default 256): a larger gap means
//! fewer HTTP requests at the cost of re-fetching some reusable bytes that sit
//! between two changed regions. `--min-savings` is the planned-savings
//! percentage below which the run gives up (default 15).
//!
//! The package URL is resolved once (its redirect, e.g. the mirror router's
//! 302 to a presigned S3 URL, is followed by the length probe and every range
//! request goes to the final URL); 429/503 and transient curl failures are
//! retried within a bounded budget. On macOS the win engine's system-proxy
//! mode resolves nothing, so set `HTTPS_PROXY` when a proxy is needed.
//!
//! `dest-path` defaults to a file next to the base path; it is created (and
//! left in place unless you delete it -- this is example code, not the
//! production install flow) so its SHA-256 can be independently re-checked
//! with `shasum -a 256` after the fact.

use std::env;
use std::path::PathBuf;
use std::process::ExitCode;
use std::time::Instant;

use codex_win_engine::delta::executor::{execute_delta, CurlRangeFetcher};
use codex_win_engine::delta::planner::PlannerConfig;
use codex_win_engine::NetworkConfig;

fn main() -> ExitCode {
    let mut raw = env::args();
    let program = raw.next().unwrap_or_else(|| "win_delta_reconstruct".to_string());
    let mut gap_kib: u64 = 256;
    let mut min_savings: f64 = 15.0;
    let mut positional: Vec<String> = Vec::new();
    while let Some(arg) = raw.next() {
        match arg.as_str() {
            "--gap-kib" => match raw.next().and_then(|v| v.parse().ok()) {
                Some(v) => gap_kib = v,
                None => {
                    eprintln!("--gap-kib needs an integer");
                    return ExitCode::FAILURE;
                }
            },
            "--min-savings" => match raw.next().and_then(|v| v.parse().ok()) {
                Some(v) => min_savings = v,
                None => {
                    eprintln!("--min-savings needs a number");
                    return ExitCode::FAILURE;
                }
            },
            _ => positional.push(arg),
        }
    }
    // Shift so the indexes below stay `args[1..]`.
    let mut args = vec![program];
    args.extend(positional);
    if args.len() < 4 || args.len() > 5 {
        eprintln!(
            "usage: {} [--gap-kib N] [--min-savings PCT] <base-msix-path> <new-package-url> <expected-sha256> [dest-path]",
            args[0]
        );
        return ExitCode::FAILURE;
    }
    let base_path = PathBuf::from(&args[1]);
    let url = &args[2];
    let expected_sha256 = args[3].to_ascii_lowercase();
    let dest_path = args
        .get(4)
        .map(PathBuf::from)
        .unwrap_or_else(|| base_path.with_file_name("reconstructed-new.msix"));

    let base_size = match std::fs::metadata(&base_path) {
        Ok(meta) => meta.len(),
        Err(err) => {
            eprintln!("cannot stat base file {}: {err}", base_path.display());
            return ExitCode::FAILURE;
        }
    };

    println!("base:            {} ({base_size} bytes)", base_path.display());
    println!("new package url: {url}");
    println!("expected sha256: {expected_sha256}");
    println!("dest:            {}", dest_path.display());
    println!("coalesce gap:    {gap_kib} KiB, min planned savings {min_savings}%");
    println!();

    let network = NetworkConfig::system();
    let tmp_dir = std::env::temp_dir().join("codex-win-delta-example");
    let fetcher = CurlRangeFetcher::new(url, &network, &tmp_dir);

    let started = Instant::now();
    // Matches the feasibility report's recommended default (256 KiB
    // coalescing gap) and its explicit caveat (a): the worst observed real
    // pair saved only 8.38%, so anything below 15% falls back to a full
    // download rather than spending network round trips on a barely-useful
    // delta plan.
    let result = execute_delta(
        &base_path,
        &fetcher,
        &dest_path,
        &expected_sha256,
        &PlannerConfig {
            coalesce_gap: gap_kib * 1024,
        },
        min_savings,
    );
    let elapsed = started.elapsed();

    match result {
        Ok(outcome) => {
            println!("RECONSTRUCTION SUCCEEDED");
            println!("  new package size:  {} bytes", outcome.new_size);
            println!("  bytes fetched:      {} bytes", outcome.bytes_fetched);
            println!("  requests made:      {} (successful; includes the length probe)", outcome.request_count);
            println!(
                "  retries:            {} transient retries, {} URL re-resolves",
                outcome.retry_stats.retries, outcome.retry_stats.re_resolves
            );
            println!(
                "  curl invocations:   {}",
                outcome.request_count + outcome.retry_stats.retries + 2 * outcome.retry_stats.re_resolves
            );
            println!(
                "  blocks reused:      {} of {} (base blocks hash-verified before fetching: {})",
                outcome.reused_blocks, outcome.total_blocks, outcome.verified_base_blocks
            );
            println!("  planned savings:    {:.2}%", outcome.savings_pct);
            println!(
                "  actual savings:     {:.2}% (1 - bytes_fetched/new_size, includes layout-probing overhead)",
                100.0 * (1.0 - outcome.bytes_fetched as f64 / outcome.new_size as f64)
            );
            println!("  sha256:             {}", outcome.sha256);
            println!("  elapsed:            {:.1}s", elapsed.as_secs_f64());
            ExitCode::SUCCESS
        }
        Err(err) => {
            println!("RECONSTRUCTION FAILED (this is exactly the signal the real update flow");
            println!("would use to fall back to a full download): {err}");
            println!("  elapsed: {:.1}s", elapsed.as_secs_f64());
            ExitCode::FAILURE
        }
    }
}
