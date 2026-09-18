use std::{env, path::PathBuf, process::Command};

fn main() {
    println!("cargo:rerun-if-changed=src/portable_launcher.rs");
    println!("cargo:rerun-if-changed=src/portable_command.rs");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let target = env::var("TARGET").expect("Cargo TARGET");
    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR")).join("LaunchCodex.exe");
    let mut rustc = Command::new(env::var_os("RUSTC").expect("Cargo RUSTC"));
    rustc
        .args([
            "--edition=2021",
            "--crate-name=codex_portable_launcher",
            "--target",
            &target,
            "-C",
            "opt-level=s",
            "-C",
            "panic=abort",
            // A portable entry point must not require a separately installed
            // Visual C++ runtime. This rustc invocation does not inherit Tauri's flags.
            "-C",
            "target-feature=+crt-static",
            "src/portable_launcher.rs",
            "-o",
        ])
        .arg(output);
    if let Some(linker) = env::var_os("RUSTC_LINKER") {
        rustc
            .arg("-C")
            .arg(format!("linker={}", linker.to_string_lossy()));
    }
    assert!(
        rustc.status().expect("build portable launcher").success(),
        "portable launcher build failed for {target}"
    );
}
