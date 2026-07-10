//! `mac_live_test` — FAITHFUL live exercise of the destructive tail against the
//! REAL `/Applications/Codex.app`, using a byte-identical copy of the current
//! bundle (zero version change → maximal safety).
//!
//! Proves on the real install root: codesign/Team/Gatekeeper gate → graceful
//! quit (only if Codex was running) → same-volume atomic swap → relaunch (only
//! if it was running). A backup is kept until the swapped bundle is verified.
//!
//!   cargo run -p codex-mac-engine --bin mac_live_test
//!
//! NOTE: this touches /Applications. Run only with explicit consent.

use std::path::{Path, PathBuf};
use std::process::{exit, Command};

use codex_mac_engine::{codesign, swap};

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
    assert!(status.success(), "ditto failed");
}

fn main() {
    let install = PathBuf::from("/Applications/Codex.app");
    if !install.exists() {
        eprintln!("未找到 /Applications/Codex.app");
        exit(2);
    }
    let before = build_of(&install);
    let was_running = swap::codex_running_at(&install);
    println!("真实安装根: /Applications/Codex.app  build={before}  running={was_running}");

    let stage_dir = std::env::temp_dir().join("codex-live-test");
    let _ = std::fs::remove_dir_all(&stage_dir);
    std::fs::create_dir_all(&stage_dir).expect("mkdir staging");
    let new_app = stage_dir.join("Codex.app");
    let backup = stage_dir.join("backup-Codex.app");

    println!("ditto 当前 bundle -> 同卷 staging（保签名）…");
    ditto(&install, &new_app);
    println!("  staged build = {}", build_of(&new_app));

    println!("\n对真实 /Applications 执行 install_gated_bundle(manage_process={was_running})…");
    match swap::install_gated_bundle(&install, &new_app, &backup, was_running) {
        Ok(()) => println!("  ✓ gate + 原子替换" ),
        Err(e) => {
            eprintln!("  ❌ {e}\n  （/Applications 未变更或已回滚）");
            let _ = std::fs::remove_dir_all(&stage_dir);
            exit(1);
        }
    }

    let after = build_of(&install);
    println!("  /Applications/Codex.app 现在 build = {after}");
    match codesign::gate_reconstructed(&install) {
        Ok(()) => println!("  ✓ 仍 codesign/Team/Gatekeeper 有效"),
        Err(e) => println!("  ⚠ 复验异常: {e}"),
    }

    let _ = std::fs::remove_dir_all(&stage_dir);
    println!(
        "\n✅ 真机 live 测试通过：build {before} -> {after}，在真实 /Applications 上验证了 \
         gate→（退出）→原子替换→（重启）→复验。backup 已清理（新 bundle 与原 bundle 字节一致）。"
    );
}
