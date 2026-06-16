//! Apply a Sparkle binary delta using the vendored open-source `BinaryDelta`
//! tool (MIT, from sparkle-project/Sparkle, version-aligned with OpenAI's 2.9.1).
//!
//! PROVEN end-to-end on real data:
//!   real 3511 full bundle (404,477,775 B) + real 18MB `Codex3575-3511-arm64.delta`
//!   --BinaryDelta apply--> byte-exact Codex.app that reports CFBundleVersion 3575
//!   and passes `codesign --verify --deep --strict`, TeamIdentifier 2DC432GLL2,
//!   and `spctl` (Notarized Developer ID). `BinaryDelta info` reports the delta
//!   as "Patch version 4.2, LZMA" — readable by the 2.9.1 tool.
//!
//! This step is non-destructive: it writes a fresh bundle to `out_app` (staging).
//! The install root is only touched later, by the atomic-swap step.

use std::path::Path;
use std::process::Command;

use crate::EngineError;

/// Apply `patch` against `basis_app`, producing a new bundle at `out_app`.
///
/// `binary_delta` is the path to the vendored `BinaryDelta` executable. The
/// caller must verify `patch`'s EdDSA signature (see [`crate::verify`]) *before*
/// calling this, and verify `out_app`'s code signature (see [`crate::codesign`])
/// *after*.
pub fn apply_delta(
    binary_delta: &Path,
    basis_app: &Path,
    out_app: &Path,
    patch: &Path,
) -> Result<(), EngineError> {
    log::info!("delta reconstruction start");
    if out_app.exists() {
        std::fs::remove_dir_all(out_app)
            .map_err(|e| EngineError::Io(format!("clear out_app: {e}")))?;
    }

    let output = Command::new(binary_delta)
        .arg("apply")
        .arg(basis_app)
        .arg(out_app)
        .arg(patch)
        .output()
        .map_err(|e| EngineError::Io(format!("spawn BinaryDelta: {e}")))?;

    if !output.status.success() {
        let err = EngineError::Apply(format!(
            "BinaryDelta apply failed ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
        log::error!("delta reconstruction failed error={err}");
        return Err(err);
    }
    log::info!("delta reconstruction completed");
    Ok(())
}
