//! Thin macOS IO helpers for the read-only slice.
//!
//! NOTE: network fetch currently shells out to `curl` and version reading to
//! `PlistBuddy`. These are placeholders for the scaffold — the production
//! Tauri backend will inject a proper HTTP client adapter and may read the
//! plist with the `plist` crate. Keeping IO behind these functions means the
//! pure parsing/planning logic stays trivially testable.

use std::path::Path;
use std::process::Command;

use crate::limits::MAX_TEXT_BYTES;
use crate::network::NetworkConfig;
use crate::EngineError;

const CURL: &str = "/usr/bin/curl";
const LIPO: &str = "/usr/bin/lipo";

fn text_from_curl(url: &str, output: std::process::Output) -> Result<String, EngineError> {
    if !output.status.success() {
        // Keep the exit code in the message so the app-layer classifier can tell
        // a connect / timeout / TLS failure apart (mirrors the Windows engine).
        return Err(EngineError::Io(format!(
            "curl failed for {url} exit={}: stderr='{}'",
            output
                .status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string()),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    if output.stdout.len() > MAX_TEXT_BYTES as usize {
        return Err(EngineError::Io(format!(
            "text response exceeded {MAX_TEXT_BYTES} bytes"
        )));
    }
    String::from_utf8(output.stdout).map_err(|e| EngineError::Io(e.to_string()))
}

/// Fetch a small text resource (the appcast) over HTTPS via system `curl`.
pub fn fetch_text(url: &str) -> Result<String, EngineError> {
    fetch_text_with_network(url, &NetworkConfig::system())
}

pub fn fetch_text_with_network(url: &str, network: &NetworkConfig) -> Result<String, EngineError> {
    let max_text = MAX_TEXT_BYTES.to_string();
    let mut command = Command::new(CURL);
    network.apply_to_command(&mut command);
    let output = command
        .args([
            "-fsSL",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "20",
            "--max-time",
            "60",
            "--max-filesize",
            &max_text,
            url,
        ])
        .output()
        .map_err(|e| EngineError::Io(format!("spawn curl: {e}")))?;

    text_from_curl(url, output)
}

/// Like `fetch_text` but with a caller-set total timeout. Used to probe a
/// possibly-unreachable source (e.g. OpenAI's official appcast for users behind
/// a block) without stalling on the default long connect timeout.
pub fn fetch_text_timeout(url: &str, max_secs: u64) -> Result<String, EngineError> {
    fetch_text_timeout_with_network(url, max_secs, &NetworkConfig::system())
}

pub fn fetch_text_timeout_with_network(
    url: &str,
    max_secs: u64,
    network: &NetworkConfig,
) -> Result<String, EngineError> {
    let max_text = MAX_TEXT_BYTES.to_string();
    let mut command = Command::new(CURL);
    network.apply_to_command(&mut command);
    let output = command
        .args([
            "-fsSL",
            "--proto",
            "=https",
            "--proto-redir",
            "=https",
            "--connect-timeout",
            "5",
            "--max-time",
            &max_secs.to_string(),
            "--max-filesize",
            &max_text,
            url,
        ])
        .output()
        .map_err(|e| EngineError::Io(format!("spawn curl: {e}")))?;

    text_from_curl(url, output)
}

/// The Codex product's stable bundle identifier — the trust anchor for "is
/// this the Codex lineage?". The upstream ChatGPT-brand merge renamed the
/// bundle (`Codex.app` → `ChatGPT.app`) and its executable while keeping this
/// ID, so names can no longer identify the product. ChatGPT Classic
/// (`com.openai.chat`) shares the display name and signing team but is a
/// different product and must never match.
pub const CODEX_BUNDLE_ID: &str = "com.openai.codex";

/// Bundle names the Codex lineage ships under (pre/post the ChatGPT rebrand).
/// The canonical name comes first so, per root, a canonical install wins ties.
const CANDIDATE_BUNDLE_NAMES: [&str; 2] = ["Codex.app", "ChatGPT.app"];

/// Locate an installed Codex and read its `CFBundleVersion` (build number).
///
/// Candidates are identity-gated on `CFBundleIdentifier == com.openai.codex`,
/// so a ChatGPT Classic at `/Applications/ChatGPT.app` is never picked up.
/// Returns `(app_path, build)` for the first (canonical-order) match; when
/// several lineage installs coexist the canonical path wins and the ambiguity
/// is surfaced via `installed_codex_candidates` + a warning here.
pub fn installed_codex_build() -> Option<(String, u64)> {
    let candidates = installed_codex_candidates();
    if candidates.len() > 1 {
        let paths: Vec<&str> = candidates.iter().map(|(p, _)| p.as_str()).collect();
        log::warn!(
            "multiple Codex-lineage installs detected, preferring canonical path candidates={paths:?}"
        );
    }
    candidates.into_iter().next()
}

/// Every Codex-lineage install found at the known roots, canonical order.
/// More than one entry means the user has ambiguous installs (e.g. an old
/// `Codex.app` plus a hand-dragged `ChatGPT.app` from the official DMG).
pub fn installed_codex_candidates() -> Vec<(String, u64)> {
    candidate_app_paths()
        .into_iter()
        .filter_map(|app| installed_codex_build_at_path(&app))
        .collect()
}

pub fn installed_codex_build_at_path(app: &str) -> Option<(String, u64)> {
    if read_bundle_identifier(app).as_deref() != Some(CODEX_BUNDLE_ID) {
        return None;
    }
    read_bundle_build(app).map(|build| (app.to_string(), build))
}

fn candidate_app_paths() -> Vec<String> {
    let mut roots = vec!["/Applications".to_string()];
    if let Ok(home) = std::env::var("HOME") {
        roots.push(format!("{home}/Applications"));
    }
    roots
        .into_iter()
        .flat_map(|root| {
            CANDIDATE_BUNDLE_NAMES
                .iter()
                .map(move |name| format!("{root}/{name}"))
        })
        .collect()
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
    let output = Command::new(LIPO)
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

/// Would this bundle run through App Translocation (a randomized
/// `/private/var/folders/…` mount, where its running process cannot be
/// located from the bundle's own path)?
///
/// Translocation only applies to quarantined bundles Gatekeeper has NOT yet
/// user-approved. The quarantine xattr itself routinely SURVIVES on perfectly
/// normal installs — it stays after the Finder move to /Applications and
/// after first-launch approval — so its mere presence must not disqualify a
/// bundle; only the missing approval flag does.
#[cfg(target_os = "macos")]
pub fn is_translocation_risk(app: &str) -> bool {
    use std::ffi::CString;
    let Ok(cpath) = CString::new(app) else {
        return false;
    };
    let Ok(cname) = CString::new("com.apple.quarantine") else {
        return false;
    };
    let size = unsafe {
        libc::getxattr(
            cpath.as_ptr(),
            cname.as_ptr(),
            std::ptr::null_mut(),
            0,
            0,
            0,
        )
    };
    if size < 0 {
        // Only a definitive "attribute does not exist" is safe. Any other
        // failure (EACCES via an ACL, EIO, …) leaves the quarantine state
        // unknown — fail closed rather than let an 0083-quarantined bundle
        // through on a read error.
        return std::io::Error::last_os_error().raw_os_error() != Some(libc::ENOATTR);
    }
    let mut buf = vec![0_u8; size as usize];
    let read = unsafe {
        libc::getxattr(
            cpath.as_ptr(),
            cname.as_ptr(),
            buf.as_mut_ptr().cast(),
            buf.len(),
            0,
            0,
        )
    };
    if read < 0 {
        // Attribute exists but is unreadable — treat as risky (the adoption
        // error explains how to clear it).
        return true;
    }
    buf.truncate(read as usize);
    quarantine_flags_indicate_translocation(&String::from_utf8_lossy(&buf))
}

#[cfg(not(target_os = "macos"))]
pub fn is_translocation_risk(_app: &str) -> bool {
    false
}

/// The quarantine xattr value is `flags;timestamp;agent;uuid` with hex flags.
/// macOS translocates a bundle iff the TRANSLOCATE flag (0x0080) is set AND
/// the DO_NOT_TRANSLOCATE flag (0x0100) is clear. The user-approved flag
/// (0x0040) is irrelevant — `00c3` (approved, 0x0080 set) still translocates.
/// Verified empirically against `SecTranslocateURLShouldRunTranslocated` for
/// nine flag combinations (0083/00c3/0080 → translocate; 0381/0181/0143/0100/
/// 0043/0001 → not). Unparseable values fail closed (risky, with guidance).
fn quarantine_flags_indicate_translocation(value: &str) -> bool {
    const QTN_FLAG_TRANSLOCATE: u32 = 0x0080;
    const QTN_FLAG_DO_NOT_TRANSLOCATE: u32 = 0x0100;
    let flags_hex = value.split(';').next().unwrap_or("").trim();
    match u32::from_str_radix(flags_hex, 16) {
        Ok(flags) => {
            flags & QTN_FLAG_TRANSLOCATE != 0 && flags & QTN_FLAG_DO_NOT_TRANSLOCATE == 0
        }
        Err(_) => true,
    }
}

/// Read a bundle's `CFBundleExecutable` — the main binary's name under
/// `Contents/MacOS/`. Needed to address the RUNNING instance of a specific
/// install by executable path (the name changed `Codex` → `ChatGPT` in the
/// brand merge, so it must be read per-bundle, never assumed).
pub fn read_bundle_executable(app: &str) -> Option<String> {
    let plist = format!("{app}/Contents/Info.plist");
    if !Path::new(&plist).exists() {
        return None;
    }
    let output = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print :CFBundleExecutable", &plist])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let exe = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if exe.is_empty() {
        None
    } else {
        Some(exe)
    }
}

/// Read a bundle's `CFBundleIdentifier` — the product-identity anchor (see
/// `CODEX_BUNDLE_ID`). Returns `None` when the bundle or key is missing.
pub fn read_bundle_identifier(app: &str) -> Option<String> {
    let plist = format!("{app}/Contents/Info.plist");
    if !Path::new(&plist).exists() {
        return None;
    }
    let output = Command::new("/usr/libexec/PlistBuddy")
        .args(["-c", "Print :CFBundleIdentifier", &plist])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let id = String::from_utf8_lossy(&output.stdout).trim().to_string();
    if id.is_empty() {
        None
    } else {
        Some(id)
    }
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

#[cfg(test)]
mod quarantine_tests {
    use super::quarantine_flags_indicate_translocation;

    // Expected values below match SecTranslocateURLShouldRunTranslocated
    // observed on a real bundle for each flag combination.
    #[test]
    fn translocates_only_with_translocate_set_and_do_not_translocate_clear() {
        assert!(quarantine_flags_indicate_translocation("0083;t;a;u"));
        assert!(quarantine_flags_indicate_translocation("00c3;t;a;u")); // approved bit does NOT exempt
        assert!(quarantine_flags_indicate_translocation("0080;t;a;u"));
    }

    #[test]
    fn does_not_translocate_without_flag_or_with_exemption() {
        assert!(!quarantine_flags_indicate_translocation("0381;t;a;u")); // Finder-copy shape
        assert!(!quarantine_flags_indicate_translocation("0181;t;a;u"));
        assert!(!quarantine_flags_indicate_translocation("0143;t;a;u"));
        assert!(!quarantine_flags_indicate_translocation("0100;t;a;u"));
        assert!(!quarantine_flags_indicate_translocation("0043;t;a;u"));
        assert!(!quarantine_flags_indicate_translocation("0001;t;a;u"));
    }

    #[test]
    fn garbled_quarantine_fails_closed() {
        assert!(quarantine_flags_indicate_translocation("not-hex;x;y;z"));
        assert!(quarantine_flags_indicate_translocation(""));
    }
}

// PlistBuddy only exists on macOS, so the identity-gate tests are mac-only
// (the Windows CI job still compiles this module).
#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::fs;

    fn write_fake_app(root: &Path, name: &str, bundle_id: &str, build: u64) -> String {
        let app = root.join(name);
        fs::create_dir_all(app.join("Contents")).unwrap();
        let plist = format!(
            r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleIdentifier</key>
    <string>{bundle_id}</string>
    <key>CFBundleVersion</key>
    <string>{build}</string>
</dict>
</plist>
"#
        );
        fs::write(app.join("Contents/Info.plist"), plist).unwrap();
        app.to_string_lossy().into_owned()
    }

    #[test]
    fn identity_gate_accepts_codex_lineage_under_any_bundle_name() {
        let root = std::env::temp_dir().join(format!("codex-sys-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);

        // Post-rebrand shape: the bundle is named ChatGPT.app but carries the
        // Codex bundle id — it must be detected.
        let renamed = write_fake_app(&root, "ChatGPT.app", CODEX_BUNDLE_ID, 5059);
        assert_eq!(
            installed_codex_build_at_path(&renamed),
            Some((renamed.clone(), 5059))
        );
        assert_eq!(read_bundle_identifier(&renamed).as_deref(), Some(CODEX_BUNDLE_ID));

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn identity_gate_rejects_chatgpt_classic() {
        let root = std::env::temp_dir().join(format!("codex-sys-classic-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);

        // ChatGPT Classic: same bundle name the rebranded Codex uses, different
        // product identity — must never be treated as an install.
        let classic = write_fake_app(&root, "ChatGPT.app", "com.openai.chat", 42);
        assert_eq!(installed_codex_build_at_path(&classic), None);

        // A Codex-named impostor with a foreign id is rejected too.
        let impostor = write_fake_app(&root, "Codex.app", "com.example.fake", 7);
        assert_eq!(installed_codex_build_at_path(&impostor), None);

        let _ = fs::remove_dir_all(&root);
    }
}
