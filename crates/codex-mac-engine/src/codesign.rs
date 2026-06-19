//! Native macOS code-signature / notarization checks via `codesign` & `spctl`.
//!
//! This is the *strongest* trust anchor: even a fully compromised mirror cannot
//! forge OpenAI's Apple Developer ID signature. Used to gate a reconstructed
//! bundle (after a delta apply) before it is allowed near the install root.

use std::path::Path;
use std::process::Command;

use crate::EngineError;

const CODESIGN: &str = "/usr/bin/codesign";
const SPCTL: &str = "/usr/sbin/spctl";
const MIN_GATEKEEPER_NOFILE_LIMIT: u64 = 32_768;

/// OpenAI's Apple Developer Team ID — verified on a real notarized Codex.app
/// (`Developer ID Application: OpenAI OpCo, LLC (2DC432GLL2)`).
pub const OPENAI_TEAM_ID: &str = "2DC432GLL2";

#[cfg(unix)]
fn try_raise_nofile_limit(min_soft_limit: u64) -> Result<Option<(u64, u64)>, String> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    let rc = unsafe { libc::getrlimit(libc::RLIMIT_NOFILE, &mut limit) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }

    let desired = min_soft_limit as libc::rlim_t;
    let target = if limit.rlim_max == libc::RLIM_INFINITY {
        desired
    } else {
        desired.min(limit.rlim_max)
    };
    if limit.rlim_cur >= target {
        return Ok(None);
    }

    let previous = limit.rlim_cur as u64;
    limit.rlim_cur = target;
    let rc = unsafe { libc::setrlimit(libc::RLIMIT_NOFILE, &limit) };
    if rc != 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(Some((previous, target as u64)))
}

#[cfg(not(unix))]
fn try_raise_nofile_limit(_min_soft_limit: u64) -> Result<Option<(u64, u64)>, String> {
    Ok(None)
}

fn prepare_gatekeeper_process_limits() {
    match try_raise_nofile_limit(MIN_GATEKEEPER_NOFILE_LIMIT) {
        Ok(Some((previous, current))) => log::info!(
            "raised process file descriptor soft limit for Gatekeeper previous={previous} current={current}"
        ),
        Ok(None) => log::debug!("process file descriptor soft limit already sufficient for Gatekeeper"),
        Err(err) => log::warn!("could not raise process file descriptor soft limit: {err}"),
    }
}

fn is_too_many_open_files(text: &str) -> bool {
    text.to_ascii_lowercase().contains("too many open files")
}

fn gatekeeper_failure_message(app: &Path, stderr: &str) -> String {
    let stderr = stderr.trim();
    if is_too_many_open_files(stderr) {
        format!(
            "Gatekeeper assessment could not complete because macOS reported too many open files while checking {}. No files were replaced; raise the macOS maxfiles limit or close file-heavy apps and retry. raw='{}'",
            app.display(),
            stderr
        )
    } else {
        format!("Gatekeeper rejected bundle: {stderr}")
    }
}

/// `codesign --verify --deep --strict` — fails if any sealed byte changed.
pub fn verify_signature(app: &Path) -> Result<(), EngineError> {
    let output = Command::new(CODESIGN)
        .args(["--verify", "--deep", "--strict"])
        .arg(app)
        .output()
        .map_err(|e| EngineError::Io(format!("spawn codesign: {e}")))?;
    if !output.status.success() {
        return Err(EngineError::Verify(format!(
            "codesign verify failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// Read the Team Identifier from a bundle's signature.
pub fn team_identifier(app: &Path) -> Result<String, EngineError> {
    // `codesign -dv` prints its fields to stderr.
    let output = Command::new(CODESIGN)
        .args(["-dv", "--verbose=2"])
        .arg(app)
        .output()
        .map_err(|e| EngineError::Io(format!("spawn codesign: {e}")))?;
    let text = String::from_utf8_lossy(&output.stderr);
    text.lines()
        .find_map(|l| l.strip_prefix("TeamIdentifier="))
        .map(|s| s.trim().to_string())
        .ok_or_else(|| EngineError::Verify("no TeamIdentifier in signature".to_string()))
}

/// Assert the bundle is signed by the expected team (defaults to OpenAI).
pub fn require_team(app: &Path, expected: &str) -> Result<(), EngineError> {
    let got = team_identifier(app)?;
    if got == expected {
        Ok(())
    } else {
        Err(EngineError::Verify(format!(
            "TeamIdentifier mismatch: got {got}, expected {expected}"
        )))
    }
}

/// `spctl --assess --type execute` — Gatekeeper's verdict (notarization).
/// Passes offline when the notarization ticket is stapled (Codex's is).
pub fn assess_gatekeeper(app: &Path) -> Result<(), EngineError> {
    prepare_gatekeeper_process_limits();
    let output = Command::new(SPCTL)
        .args(["--assess", "--type", "execute"])
        .arg(app)
        .output()
        .map_err(|e| EngineError::Io(format!("spawn spctl: {e}")))?;
    if !output.status.success() {
        return Err(EngineError::Verify(gatekeeper_failure_message(
            app,
            &String::from_utf8_lossy(&output.stderr),
        )));
    }
    Ok(())
}

/// Full post-apply gate: signature intact + correct team + Gatekeeper accepts.
pub fn gate_reconstructed(app: &Path) -> Result<(), EngineError> {
    let app_name = app
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("Codex.app");
    log::info!("codesign Gatekeeper gate start path={app_name}");
    if let Err(err) = verify_signature(app) {
        log::error!("codesign gate failed path={app_name} error={err}");
        return Err(err);
    }
    let team = match team_identifier(app) {
        Ok(team) => team,
        Err(err) => {
            log::error!("codesign gate failed path={app_name} error={err}");
            return Err(err);
        }
    };
    if team != OPENAI_TEAM_ID {
        let err = EngineError::Verify(format!(
            "TeamIdentifier mismatch: got {team}, expected {OPENAI_TEAM_ID}"
        ));
        log::error!("codesign gate failed path={app_name} error={err}");
        return Err(err);
    }
    if let Err(err) = assess_gatekeeper(app) {
        log::error!("Gatekeeper gate failed path={app_name} error={err}");
        return Err(err);
    }
    log::info!("codesign Gatekeeper gate passed path={app_name} team={team}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_gatekeeper_file_descriptor_exhaustion() {
        assert!(is_too_many_open_files(
            "/tmp/Codex.app: Too many open files"
        ));
        assert!(is_too_many_open_files(
            "gatekeeper rejected bundle: too many open files"
        ));
        assert!(!is_too_many_open_files(
            "/tmp/Codex.app: rejected (the code is valid but does not seem to be an app)"
        ));
    }

    #[test]
    fn explains_gatekeeper_resource_failure_without_calling_it_rejected() {
        let message = gatekeeper_failure_message(
            Path::new("/tmp/Codex.app"),
            "/tmp/Codex.app: Too many open files",
        );

        assert!(message.contains("could not complete"));
        assert!(message.contains("No files were replaced"));
        assert!(!message.contains("rejected bundle"));
    }

    #[test]
    fn preserves_rejected_wording_for_real_gatekeeper_rejections() {
        let message = gatekeeper_failure_message(Path::new("/tmp/Codex.app"), "rejected");

        assert_eq!(message, "Gatekeeper rejected bundle: rejected");
    }
}
