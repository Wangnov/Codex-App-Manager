use std::{
    env,
    path::{Path, PathBuf},
    process::Command,
};

fn main() {
    println!("cargo:rerun-if-changed=src/portable_launcher.rs");
    println!("cargo:rerun-if-changed=src/portable_command.rs");
    println!("cargo:rerun-if-changed=assets/launcher.rc");
    println!("cargo:rerun-if-changed=assets/launcher.ico");
    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let target = env::var("TARGET").expect("Cargo TARGET");
    let output =
        PathBuf::from(env::var_os("OUT_DIR").expect("Cargo OUT_DIR")).join("LaunchCodex.exe");
    let resource = compile_icon_resource(&target, &output);
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
    // Embed the icon so shortcuts and taskbar pins that point at this stub
    // (it has no icon of its own otherwise) still show the Codex icon.
    rustc.arg("-C").arg(format!("link-arg={}", resource.display()));
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

/// Compile `assets/launcher.rc` into something the linker accepts as a plain
/// input: a `.res` file for MSVC, a COFF object (via windres) for GNU.
fn compile_icon_resource(target: &str, launcher_output: &Path) -> PathBuf {
    let assets = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"))
        .join("assets");
    let out_dir = launcher_output.parent().expect("OUT_DIR").to_path_buf();
    let script = assets.join("launcher.rc");
    if target.ends_with("-msvc") {
        let res = out_dir.join("launcher.res");
        let status = Command::new(find_rc())
            .current_dir(&assets)
            .arg("/nologo")
            .arg("/fo")
            .arg(&res)
            .arg(&script)
            .status()
            .expect("run rc.exe");
        assert!(status.success(), "rc.exe failed for {target}");
        res
    } else {
        let object = out_dir.join("launcher_res.o");
        let windres = env::var_os("WINDRES").unwrap_or_else(|| {
            if target.starts_with("aarch64") {
                "aarch64-w64-mingw32-windres".into()
            } else if target.starts_with("i686") {
                "i686-w64-mingw32-windres".into()
            } else {
                "x86_64-w64-mingw32-windres".into()
            }
        });
        let status = Command::new(windres)
            .current_dir(&assets)
            .args(["-O", "coff", "-i"])
            .arg(&script)
            .arg("-o")
            .arg(&object)
            .status()
            .expect("run windres");
        assert!(status.success(), "windres failed for {target}");
        object
    }
}

/// Prefer `rc.exe` on PATH (Developer Command Prompt / CI), otherwise the
/// newest Windows SDK installation.
fn find_rc() -> PathBuf {
    if let Some(path) = env::var_os("PATH") {
        if let Some(found) = env::split_paths(&path)
            .map(|dir| dir.join("rc.exe"))
            .find(|candidate| candidate.is_file())
        {
            return found;
        }
    }
    let kits = env::var_os("ProgramFiles(x86)")
        .or_else(|| env::var_os("ProgramFiles"))
        .map(|root| PathBuf::from(root).join("Windows Kits").join("10").join("bin"))
        .expect("Program Files is unavailable; run from a Visual Studio build environment");
    let mut versions: Vec<_> = std::fs::read_dir(&kits)
        .unwrap_or_else(|error| panic!("Windows SDK not found at {}: {error}", kits.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    versions.sort();
    for version in versions.iter().rev() {
        for arch in ["x64", "arm64", "x86"] {
            let candidate = version.join(arch).join("rc.exe");
            if candidate.is_file() {
                return candidate;
            }
        }
    }
    panic!("rc.exe not found; install the Windows SDK");
}
