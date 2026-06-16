//! `mac_rehearse` — capstone rehearsal of the full install sequence against a
//! SANDBOX path, using the REAL reconstructed bundles. Never touches
//! /Applications and never quits the real Codex.
//!
//! Demonstrates: codesign/Team/Gatekeeper gate → same-volume atomic swap →
//! rollback, on actual signed bundles.
//!
//! Inputs (produced by the earlier BinaryDelta proof step):
//!   /tmp/codex-bd/basis/Codex.app   real 3511
//!   /tmp/codex-bd/out.app           real reconstructed 3575
//!
//!   cargo run -p codex-mac-engine --bin mac_rehearse

use std::path::Path;
use std::process::Command;

use codex_mac_engine::swap;

const BASIS: &str = "/tmp/codex-bd/basis/Codex.app";
const OUT: &str = "/tmp/codex-bd/out.app";
const SANDBOX: &str = "/tmp/codex-rehearse";

fn build_of(app: &Path) -> String {
    Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print :CFBundleVersion"])
        .arg(app.join("Contents/Info.plist"))
        .output()
        .ok()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "?".to_string())
}

fn ditto(src: &Path, dst: &Path) {
    let status = Command::new("/usr/bin/ditto")
        .arg(src)
        .arg(dst)
        .status()
        .expect("spawn ditto");
    assert!(status.success(), "ditto {src:?} -> {dst:?} failed");
}

fn main() {
    let basis = Path::new(BASIS);
    let out = Path::new(OUT);
    if !basis.exists() || !out.exists() {
        eprintln!(
            "缺少素材：需要 {} 和 {}（先跑 BinaryDelta 证明步骤生成）",
            BASIS, OUT
        );
        std::process::exit(2);
    }

    let sandbox = Path::new(SANDBOX);
    let install = sandbox.join("Codex.app");
    let new_app = sandbox.join("staged-Codex.app");
    let backup = sandbox.join("backup-Codex.app");

    let _ = std::fs::remove_dir_all(sandbox);
    std::fs::create_dir_all(sandbox).expect("mkdir sandbox");

    println!("沙盒: {SANDBOX}（绝不碰 /Applications）");
    println!("准备素材：basis(3511) -> install；out(3575) -> staged …");
    ditto(basis, &install);
    ditto(out, &new_app);
    println!("  install = build {}", build_of(&install));
    println!("  staged  = build {}", build_of(&new_app));

    println!("\n执行 install_gated_bundle(manage_process=false)…");
    match swap::install_gated_bundle(&install, &new_app, &backup, false) {
        Ok(()) => println!("  ✓ codesign/Team/Gatekeeper 闸通过 + 同卷原子替换 成功"),
        Err(e) => {
            eprintln!("  ❌ {e}");
            std::process::exit(1);
        }
    }
    println!("  install = build {} (期望 3575)", build_of(&install));
    println!("  backup  = build {} (期望 3511)", build_of(&backup));

    println!("\n演练回滚…");
    swap::rollback(&install, &backup).expect("rollback");
    println!("  install = build {} (期望 3511)", build_of(&install));

    let _ = std::fs::remove_dir_all(sandbox);
    println!("\n✅ 全链路彩排成功（gate → swap → rollback），沙盒已清理，/Applications 未触碰。");
}
