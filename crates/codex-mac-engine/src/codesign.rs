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

/// OpenAI's Apple Developer Team ID — verified on a real notarized Codex.app
/// (`Developer ID Application: OpenAI OpCo, LLC (2DC432GLL2)`).
pub const OPENAI_TEAM_ID: &str = "2DC432GLL2";

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
    let output = Command::new(SPCTL)
        .args(["--assess", "--type", "execute"])
        .arg(app)
        .output()
        .map_err(|e| EngineError::Io(format!("spawn spctl: {e}")))?;
    if !output.status.success() {
        return Err(EngineError::Verify(format!(
            "Gatekeeper rejected bundle: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(())
}

/// Full post-apply gate: signature intact + correct team + Gatekeeper accepts.
pub fn gate_reconstructed(app: &Path) -> Result<(), EngineError> {
    verify_signature(app)?;
    require_team(app, OPENAI_TEAM_ID)?;
    assess_gatekeeper(app)?;
    Ok(())
}
