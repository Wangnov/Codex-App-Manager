//! Windows block-level delta update engine (prototype).
//!
//! Rationale and measurements live in the feasibility report produced before
//! this module existed (byte-identical reconstruction proven with a SHA-256
//! match; 8-55%, median ~29%, savings across 8 real consecutive
//! `codex-app-mirror` release pairs). This module turns that prototype
//! (originally a set of standalone Python scripts) into pure, testable Rust:
//!
//!   - [`zip_format`]: raw ZIP / ZIP64 central-directory + EOCD parsing,
//!     independent of the `zip` crate (which does not expose on-disk byte
//!     offsets, only decompressing reads).
//!   - `crate::appx_blockmap` (one level up, shared with `portable.rs`'s
//!     extractor so the XML traversal exists exactly once): parses
//!     `AppxBlockMap.xml` into per-file, per-block hash/size records.
//!   - [`layout`]: combines the two into one per-entry [`layout::PackageLayout`]
//!     the planner can walk directly (absolute byte offset of every block).
//!   - [`planner`]: pure copy-or-fetch planning with gap coalescing. No I/O,
//!     so it is unit-tested without a network or a real MSIX on disk.
//!   - [`http`]: the range transport. [`http::CurlRangeFetcher`] (curl-based,
//!     honors [`crate::network::NetworkConfig`]) resolves the package URL's
//!     redirect once and pins every range request to the final URL,
//!     re-resolving once on an expired presign (403), and retries 429/503 (with
//!     a capped `Retry-After`) and transient curl exits under a bounded budget.
//!   - [`executor`]: runs a plan against a real ([`http::CurlRangeFetcher`])
//!     or fake (tests) range source, verifies the base blocks it is about to
//!     reuse against their own block-map hashes, assembles the new package in a
//!     staging file, and
//!     requires the assembled file's streamed SHA-256 to equal the value the
//!     caller supplies (from the mirror manifest / `SHA256SUMS-windows.txt`)
//!     before ever returning success. Any failure -- a plan not worth using,
//!     a corrupt/unreadable base, a network error, or a SHA-256 mismatch --
//!     is surfaced as an `Err` so the caller falls back to the existing full
//!     download path; this module never partially "commits" a bad file.
//!   - [`retention`]: keeps at most one verified MSIX on disk as the delta
//!     base for the next update. Its module docs record where the app's update
//!     flow really deletes the downloaded MSIX (`clear_download_cache()`
//!     right after a successful install, so retention must hook in *before*
//!     that call and store the base outside `downloads/`), the hard-link
//!     placement, and the disk cost.
//!
//! **Not wired into the app's update flow in this PR.** `perform_windows_update*`
//! in `src-tauri/src/app/win_update.rs` still always does a full download; see
//! the PR description for what integrating this would require and why it is
//! deferred. Wiring prerequisites: hook retention before the post-install
//! `clear_download_cache()`; pass the app's cancel flag through
//! [`http::CurlRangeFetcher::with_cancel`] (without it a range request cannot
//! be aborted for up to 30 minutes) and treat [`http::is_cancelled_error`] as
//! "stop", not "fall back"; on [`executor::is_corrupt_base_error`] clear the
//! retained base before falling back.

pub mod executor;
pub mod http;
pub mod layout;
pub mod planner;
pub mod retention;
pub mod zip_format;
