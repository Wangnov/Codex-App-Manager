//! Thin macOS IO helpers for the read-only slice.
//!
//! NOTE: network fetch currently shells out to `curl` and version reading to
//! `PlistBuddy`. These are placeholders for the scaffold — the production
//! Tauri backend will inject a proper HTTP client adapter and may read the
//! plist with the `plist` crate. Keeping IO behind these functions means the
//! pure parsing/planning logic stays trivially testable.

use std::path::Path;
use std::process::Command;

use crate::EngineError;

/// Fetch a small text resource (the appcast) over HTTPS via system `curl`.
pub fn fetch_text(url: &str) -> Result<String, EngineError> {
    let output = Command::new("curl")
        .args(["-fsSL", "--connect-timeout", "20", url])
        .output()
        .map_err(|e| EngineError::Io(format!("spawn curl: {e}")))?;

    if !output.status.success() {
        return Err(EngineError::Io(format!(
            "curl failed for {url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    String::from_utf8(output.stdout).map_err(|e| EngineError::Io(e.to_string()))
}

/// Like `fetch_text` but with a caller-set total timeout. Used to probe a
/// possibly-unreachable source (e.g. OpenAI's official appcast for users behind
/// a block) without stalling on the default long connect timeout.
pub fn fetch_text_timeout(url: &str, max_secs: u64) -> Result<String, EngineError> {
    let output = Command::new("curl")
        .args([
            "-fsSL",
            "--connect-timeout",
            "5",
            "--max-time",
            &max_secs.to_string(),
            url,
        ])
        .output()
        .map_err(|e| EngineError::Io(format!("spawn curl: {e}")))?;

    if !output.status.success() {
        return Err(EngineError::Io(format!(
            "curl failed for {url}: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }

    String::from_utf8(output.stdout).map_err(|e| EngineError::Io(e.to_string()))
}

/// Locate an installed `Codex.app` and read its `CFBundleVersion` (build number).
///
/// Returns `(app_path, build)` for the first candidate found, or `None`.
pub fn installed_codex_build() -> Option<(String, u64)> {
    candidate_app_paths()
        .into_iter()
        .find_map(|app| installed_codex_build_at_path(&app))
}

pub fn installed_codex_build_at_path(app: &str) -> Option<(String, u64)> {
    read_bundle_build(app).map(|build| (app.to_string(), build))
}

fn candidate_app_paths() -> Vec<String> {
    let mut paths = vec!["/Applications/Codex.app".to_string()];
    if let Ok(home) = std::env::var("HOME") {
        paths.push(format!("{home}/Applications/Codex.app"));
    }
    paths
}

/// Best-effort architecture of an installed Codex.app, read from its Mach-O
/// executable via `lipo`. Returns the host arch when the bundle is universal,
/// otherwise the bundle's single arch (e.g. an Intel/Rosetta install on Apple
/// Silicon reports `x86_64`). Values match `lipo` naming: `arm64` / `x86_64`.
pub fn app_arch(app: &str) -> Option<String> {
    let plist = format!("{app}/Contents/Info.plist");
    let exe = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print :CFBundleExecutable", &plist])
        .output()
        .ok()?;
    if !exe.status.success() {
        return None;
    }
    let exe_name = String::from_utf8_lossy(&exe.stdout).trim().to_string();
    if exe_name.is_empty() {
        return None;
    }
    let output = Command::new("lipo")
        .args(["-archs", &format!("{app}/Contents/MacOS/{exe_name}")])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let archs: Vec<String> = String::from_utf8_lossy(&output.stdout)
        .split_whitespace()
        .map(|s| s.to_string())
        .collect();
    if archs.is_empty() {
        return None;
    }
    let host = if std::env::consts::ARCH == "aarch64" {
        "arm64"
    } else {
        "x86_64"
    };
    if archs.iter().any(|a| a == host) {
        Some(host.to_string())
    } else {
        Some(archs[0].clone())
    }
}

fn read_bundle_build(app: &str) -> Option<u64> {
    let plist = format!("{app}/Contents/Info.plist");
    if !Path::new(&plist).exists() {
        return None;
    }
    let output = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print :CFBundleVersion", &plist])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    String::from_utf8_lossy(&output.stdout).trim().parse().ok()
}

/// Read the human-facing version string (`CFBundleShortVersionString`, e.g.
/// `26.602.40724`) of an installed bundle. This is what we show the user; the
/// build number (`CFBundleVersion`) is what Sparkle compares. Returns `None` if
/// the key is missing.
pub fn read_bundle_short_version(app: &str) -> Option<String> {
    let plist = format!("{app}/Contents/Info.plist");
    if !Path::new(&plist).exists() {
        return None;
    }
    let output = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print :CFBundleShortVersionString", &plist])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}
