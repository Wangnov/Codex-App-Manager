//! Ed25519 (EdDSA) signature verification for Sparkle update artifacts.
//!
//! Sparkle signs each enclosure (full `.zip` or `.delta`) with Ed25519; the
//! base64 signature lives in the appcast's `sparkle:edSignature` attribute and
//! the public key in the app's `SUPublicEDKey`. We pin OpenAI's key so a
//! compromised mirror/CDN cannot substitute a payload it did not sign.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use ed25519_dalek::{Signature, VerifyingKey};

use crate::EngineError;

/// OpenAI's Sparkle EdDSA public key — from Codex.app's `SUPublicEDKey`
/// (a.k.a. `codexSparklePublicKey` in app.asar). Verified on a real DMG.
pub const SPARKLE_ED_PUBKEY_B64: &str = "mNfr1v9t63BfgDtlw4C8lRvSY6uMggIXABDOCi3tS6k=";

/// The pinned verifying key.
pub fn sparkle_pubkey() -> Result<VerifyingKey, EngineError> {
    verifying_key_from_b64(SPARKLE_ED_PUBKEY_B64)
}

fn verifying_key_from_b64(b64: &str) -> Result<VerifyingKey, EngineError> {
    let bytes = B64
        .decode(b64.trim())
        .map_err(|e| EngineError::Verify(format!("pubkey base64: {e}")))?;
    let arr: [u8; 32] = bytes
        .as_slice()
        .try_into()
        .map_err(|_| EngineError::Verify(format!("pubkey must be 32 bytes, got {}", bytes.len())))?;
    VerifyingKey::from_bytes(&arr).map_err(|e| EngineError::Verify(format!("pubkey: {e}")))
}

/// Verify `message` against `ed_signature_b64` using the pinned Sparkle key.
pub fn verify_sparkle(message: &[u8], ed_signature_b64: &str) -> Result<(), EngineError> {
    let bytes = message.len();
    log::info!("EdDSA verification start bytes={bytes}");
    match verify_with(&sparkle_pubkey()?, message, ed_signature_b64) {
        Ok(()) => {
            log::info!("EdDSA verification passed");
            Ok(())
        }
        Err(err) => {
            log::error!("EdDSA verification failed error={err}");
            Err(err)
        }
    }
}

/// Verify `message` against `ed_signature_b64` using an explicit key.
pub fn verify_with(
    key: &VerifyingKey,
    message: &[u8],
    ed_signature_b64: &str,
) -> Result<(), EngineError> {
    let sig_bytes = B64
        .decode(ed_signature_b64.trim())
        .map_err(|e| EngineError::Verify(format!("signature base64: {e}")))?;
    let arr: [u8; 64] = sig_bytes.as_slice().try_into().map_err(|_| {
        EngineError::Verify(format!("signature must be 64 bytes, got {}", sig_bytes.len()))
    })?;
    let sig = Signature::from_bytes(&arr);
    key.verify_strict(message, &sig)
        .map_err(|_| EngineError::Verify("EdDSA signature does not match".to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};

    #[test]
    fn pinned_pubkey_is_a_valid_ed25519_key() {
        assert!(sparkle_pubkey().is_ok());
    }

    #[test]
    fn verify_roundtrip_and_rejects_tamper() {
        // Deterministic key from fixed bytes (no RNG needed).
        let sk = SigningKey::from_bytes(&[7u8; 32]);
        let vk = sk.verifying_key();
        let msg = b"codex update payload bytes";
        let sig_b64 = B64.encode(sk.sign(msg).to_bytes());

        assert!(verify_with(&vk, msg, &sig_b64).is_ok());
        assert!(verify_with(&vk, b"tampered payload", &sig_b64).is_err());
    }

    #[test]
    fn rejects_malformed_signature() {
        let vk = sparkle_pubkey().unwrap();
        assert!(verify_with(&vk, b"x", "not-base64!!").is_err());
        assert!(verify_with(&vk, b"x", "aGVsbG8=").is_err()); // valid b64, wrong length
    }
}
