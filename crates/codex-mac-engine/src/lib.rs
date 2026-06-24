//! codex-mac-engine
//!
//! Pure logic for planning Codex macOS updates from a Sparkle appcast.
//! Deliberately free of any Tauri dependency so it compiles and tests fast
//! in isolation. The Tauri backend depends on this crate via a path dependency
//! and supplies the GUI / command surface.
//!
//! Scope of this slice (read-only, safe):
//!   - parse the Sparkle appcast (versions + full enclosure + binary deltas)
//!   - given the installed build number, compute an UpdatePlan (delta vs full)
//!
//! Out of scope here (later, destructive — guarded elsewhere):
//!   - download, EdDSA verify, BinaryDelta apply, atomic swap, relaunch

pub mod appcast;
pub mod apply;
pub mod codesign;
pub mod download;
pub mod limits;
pub mod network;
pub mod plan;
pub mod swap;
pub mod sys;
pub mod verify;

pub use appcast::{parse_appcast, Appcast, AppcastItem, Delta, Enclosure};
pub use apply::apply_delta;
pub use codesign::{gate_reconstructed, OPENAI_TEAM_ID};
pub use download::{
    download_to_with_network, download_to_with_progress_bounded_with_network,
    download_to_with_progress_with_network,
};
pub use network::NetworkConfig;
pub use plan::{plan_update, UpdatePlan, UpdateStrategy};
pub use swap::{install_gated_bundle, quit_codex, relaunch, rollback, swap_in_place};
pub use verify::{verify_sparkle, SPARKLE_ED_PUBKEY_B64};

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("failed to parse appcast: {0}")]
    Parse(String),
    #[error("appcast contained no usable items")]
    EmptyAppcast,
    #[error("signature verification error: {0}")]
    Verify(String),
    #[error("delta apply error: {0}")]
    Apply(String),
    #[error("io error: {0}")]
    Io(String),
}
