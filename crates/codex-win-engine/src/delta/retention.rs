//! Retention of the last verified MSIX as the block-delta base.
//!
//! **Where the app currently keeps/deletes the downloaded MSIX**
//! (`src-tauri/src/app/win_update.rs`, `src-tauri/src/app/staging.rs`,
//! `src-tauri/src/commands.rs`). A download lands at
//! `staging::download_cache_path(url, name)`, a path keyed by an FNV-1a hash
//! of the download URL under `staging_root()/downloads/`. That file does
//! **not** outlive a successful install:
//!
//!   - `win_perform_update` (`commands.rs`, right after `report.success`) and
//!     `win_install_historical_release` (`commands.rs`, on success when the
//!     package was not a local file) both call
//!     `staging::clear_download_cache()` immediately. That
//!     `remove_dir_all`s the *whole* `downloads/` directory, so the
//!     just-installed MSIX is gone before the command returns. A failed or
//!     cancelled perform deliberately leaves it in place so a retry or
//!     resume can reuse it.
//!   - `discard_windows_download()` (paused-state cancel) clears it too.
//!   - As a backstop, `cleanup_stale_staging` removes any file directly under
//!     `downloads/` older than `STALE_AFTER` (30 minutes) when no operation
//!     is busy.
//!
//! The consequence for wiring: retention **must run before** that
//! `clear_download_cache()` call (with the cached MSIX path and the SHA-256
//! the download was verified against), and the retained copy **must live
//! outside `downloads/`** -- e.g. `staging_root()/delta-base/`, a directory
//! neither sweep touches (they only look at `update-*`/`inspect-*` directories
//! and at files directly inside `downloads/`). Calling
//! [`retain_verified_base`] after the command returned, or with a path under
//! `downloads/`, finds nothing: the file is already deleted and no base is
//! ever retained, so delta would silently never trigger.
//!
//! This module is the explicit, single-slot replacement for the file the
//! cache clear deletes: a fixed-name slot ([`retain_verified_base`] /
//! [`retained_base`] / [`clear_retained_base`]) the *caller* populates only
//! once it has verified a downloaded MSIX's SHA-256 against the mirror
//! manifest, and which a later update hands to
//! [`crate::delta::executor::execute_delta`] as the delta base. "At most
//! one" is enforced by always replacing the same fixed file name.
//!
//! **How the base is placed: hard link first, copy only as a fallback.**
//! Because the cache clear removes the source right after, the natural
//! placement is a same-volume hard link (`std::fs::hard_link`) of the
//! verified MSIX into the retention directory: it costs no data write and no
//! extra disk, and the bytes survive the cache clear (the data lives until the
//! last name is removed). Only if linking is impossible (different volume, a
//! filesystem without hard links such as FAT/exFAT) does it fall back to
//! `std::fs::copy`. The retained file therefore shares its bytes with
//! `verified_msix` until that name is deleted: never modify `verified_msix`
//! in place afterwards (the update flow never rewrites a completed download;
//! a new download is a different file).
//!
//! **Disk cost (be honest about it):**
//!   - *Steady state between updates:* one retained MSIX, currently about
//!     750-900 MB for the x64 build (see the feasibility report's `new_size`
//!     figures). This is new permanent usage compared with today, where the
//!     download cache is emptied after every install.
//!   - *During an update that uses the base:* the retained base plus the new
//!     package (delta assembly or full download in staging) coexist, so the
//!     transient peak is **two** MSIX instead of one, and the delta path also
//!     holds the base in memory (~900 MB resident).
//!   - *At retention time:* with the hard link there is no additional peak
//!     (old base + cached download coexist, the link adds nothing, and the
//!     old base's bytes are freed at the replacing rename). With the copy
//!     fallback the peak is **three** MSIX (old base, cached download, the
//!     new copy: about 2.7 GB for x64) plus the extra ~900 MB of write IO on
//!     every update. A copy failure (for example disk full) leaves the old
//!     base intact and removes the partial temp file, but by then the install
//!     already succeeded, so a caller on a constrained disk should treat
//!     retention as opt-in (gated by the same default-off setting as the rest
//!     of this feature) and call [`clear_retained_base`] when the user
//!     disables it.
//!
//! **Integrity of the base.** The sidecar SHA-256 records which release the
//! base is (identity hint, saves re-hashing ~1 GB); it is not compared with the
//! file's bytes on every use. What protects a delta run from a silently
//! corrupted base is [`crate::delta::executor::execute_delta`] verifying every
//! *reused* block against the base's own `AppxBlockMap.xml` hashes before any
//! bulk fetch. When that (or the base layout parse) fails, the error satisfies
//! [`crate::delta::executor::is_corrupt_base_error`]; the caller must then call
//! [`clear_retained_base`] (otherwise every later update repeats the same
//! failed attempt) and fall back to the full download.
//!
//! No wiring into `win_update.rs` happens in this PR: the functions here are
//! pure filesystem operations the caller drives explicitly.

use std::io;
use std::path::{Path, PathBuf};

/// Fixed file name for the retained base inside `base_dir` -- fixed, rather
/// than versioned, so a new call always replaces the previous base instead
/// of accumulating one file per update ("keep at most one").
pub const BASE_FILE_NAME: &str = "delta-base.msix";
/// Sidecar recording the base's own verified SHA-256, so a future update run
/// can identify what it has on disk without re-hashing a ~1 GB file just to
/// check whether it's usable as a base.
pub const BASE_SHA256_FILE_NAME: &str = "delta-base.sha256";

/// Replace the retained delta base (if any) with `verified_msix`.
///
/// `sha256` must already be the value the caller independently verified
/// `verified_msix` against (e.g. the mirror manifest's checksum for that
/// release) -- this function does not re-verify it, it only records it
/// alongside the base so a later reader does not have to re-hash the file.
///
/// The file is placed with a same-volume hard link, falling back to a copy
/// (see the module docs for the disk-cost consequences of each). It is
/// linked/copied to a temp path in `base_dir` first and only renamed onto the
/// fixed [`BASE_FILE_NAME`] once complete, so a crash or a disk-full error
/// never leaves a truncated file at the name a future update would trust. If
/// placing the temp file fails outright, whatever it left behind is removed
/// before returning the error --
/// `clear_retained_base` only ever removes the *final* base/sidecar names,
/// not this function's own temp names.
///
/// Both the base and the sidecar checksum are replaced the same way -- temp
/// path in `base_dir`, then [`std::fs::rename`] onto the fixed name, which
/// atomically replaces whatever was already there on both Unix and Windows
/// -- and, critically, the base is renamed into place *before* the sidecar is
/// touched at all, with no separate "remove the old sidecar first" step. That
/// ordering means a failure at either rename leaves a fully usable pair for
/// [`retained_base`] to hand back: a failed base rename leaves the *previous*
/// (base, sidecar) pair completely untouched, so a failed *replacement* never
/// silently downgrades a perfectly good previous base to "no usable base".
///
/// The remaining hazard is a failure *after* the new base is already in
/// place: writing or renaming the new sidecar then fails, and the old
/// sidecar would vouch for the wrong file. That path removes the retained
/// pair entirely (best effort) before returning the error, so
/// [`retained_base`] reports "no usable base" instead of the new file under
/// the previous release's checksum. (A literal process crash in the
/// sub-millisecond gap between the two renames could still leave that
/// pairing; the sidecar is an identity hint, never a content-integrity check
/// on the base -- [`crate::delta::executor::execute_delta`] verifies the
/// reused blocks' real bytes and the final whole-file SHA-256 gates trusting
/// a reconstruction.)
pub fn retain_verified_base(
    base_dir: &Path,
    verified_msix: &Path,
    sha256: &str,
) -> io::Result<PathBuf> {
    retain_verified_base_with(base_dir, verified_msix, sha256, &|src, dst| std::fs::hard_link(src, dst))
}

/// [`retain_verified_base`] with the link step injectable, so the copy
/// fallback is testable on a filesystem where linking works.
fn retain_verified_base_with(
    base_dir: &Path,
    verified_msix: &Path,
    sha256: &str,
    link: &dyn Fn(&Path, &Path) -> io::Result<()>,
) -> io::Result<PathBuf> {
    std::fs::create_dir_all(base_dir)?;
    let dest = base_dir.join(BASE_FILE_NAME);
    let tmp = base_dir.join(format!("{BASE_FILE_NAME}.tmp"));
    let sha_path = base_dir.join(BASE_SHA256_FILE_NAME);
    let sha_tmp = base_dir.join(format!("{BASE_SHA256_FILE_NAME}.tmp"));

    // A leftover temp from a crashed earlier run would make the link fail
    // with AlreadyExists (and is dead weight either way).
    match std::fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(err) if err.kind() == io::ErrorKind::NotFound => {}
        Err(err) => return Err(err),
    }
    // Prefer a hard link: no extra disk, no extra IO, and it survives the
    // caller's download-cache clear. Any link error (cross-volume, no
    // hard-link support) falls through to a real copy.
    if link(verified_msix, &tmp).is_err() {
        let _ = std::fs::remove_file(&tmp);
        if let Err(err) = std::fs::copy(verified_msix, &tmp) {
            let _ = std::fs::remove_file(&tmp);
            return Err(err);
        }
    }
    if let Err(err) = std::fs::rename(&tmp, &dest) {
        let _ = std::fs::remove_file(&tmp);
        return Err(err);
    }
    // If `verified_msix` already *was* the retained base (two hard links to
    // one inode), POSIX `rename` is a successful no-op that leaves `tmp`
    // behind as a second name; drop it.
    let _ = std::fs::remove_file(&tmp);
    // The new base is in place; from here on a failure must not leave the
    // previous sidecar vouching for it.
    if let Err(err) = std::fs::write(&sha_tmp, sha256.trim()) {
        let _ = std::fs::remove_file(&sha_tmp);
        let _ = clear_retained_base(base_dir);
        return Err(err);
    }
    if let Err(err) = std::fs::rename(&sha_tmp, &sha_path) {
        let _ = std::fs::remove_file(&sha_tmp);
        let _ = clear_retained_base(base_dir);
        return Err(err);
    }
    Ok(dest)
}

/// The currently retained base, if one is present and its sidecar checksum
/// is readable and non-empty. A base file with a missing/empty sidecar (an
/// interrupted [`retain_verified_base`], or manual tampering) is treated as
/// "no usable base" rather than handed to the planner unverified -- the
/// planner's own base-layout parse and the executor's final whole-file
/// SHA-256 check are the real safety net either way, but there is no reason
/// to spend a delta-plan attempt on a base this module cannot itself vouch
/// for the provenance of.
pub fn retained_base(base_dir: &Path) -> Option<(PathBuf, String)> {
    let path = base_dir.join(BASE_FILE_NAME);
    if !path.is_file() {
        return None;
    }
    let sha256 = std::fs::read_to_string(base_dir.join(BASE_SHA256_FILE_NAME))
        .ok()?
        .trim()
        .to_string();
    if sha256.is_empty() {
        return None;
    }
    Some((path, sha256))
}

/// Drop the retained base entirely (both the MSIX and its sidecar). Used
/// when the user disables delta updates, clears the cache, or a base fails
/// its final verification and should not be reused as-is for the next
/// attempt. Missing files are not an error -- this is also how a "nothing
/// retained yet" state gets normalized after a manual cleanup.
pub fn clear_retained_base(base_dir: &Path) -> io::Result<()> {
    for name in [BASE_FILE_NAME, BASE_SHA256_FILE_NAME] {
        match std::fs::remove_file(base_dir.join(name)) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "codex-win-engine-retention-test-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn retains_and_reads_back_a_base() {
        let base_dir = temp_dir("retain");
        let source_dir = temp_dir("source");
        let source = source_dir.join("Codex.msix");
        std::fs::write(&source, b"fake msix bytes").unwrap();

        assert!(retained_base(&base_dir).is_none());
        let dest = retain_verified_base(&base_dir, &source, "abc123").unwrap();
        assert_eq!(dest, base_dir.join(BASE_FILE_NAME));
        assert_eq!(std::fs::read(&dest).unwrap(), b"fake msix bytes");

        let (path, sha256) = retained_base(&base_dir).unwrap();
        assert_eq!(path, dest);
        assert_eq!(sha256, "abc123");

        std::fs::remove_dir_all(&base_dir).ok();
        std::fs::remove_dir_all(&source_dir).ok();
    }

    #[test]
    fn the_retained_base_survives_deleting_the_source_like_the_download_cache_clear_does() {
        // The app empties `downloads/` right after a successful install; the
        // retained base must not depend on the source name staying around.
        let base_dir = temp_dir("survives-clear");
        let source_dir = temp_dir("survives-clear-source");
        let source = source_dir.join("Codex.msix");
        std::fs::write(&source, b"installed msix bytes").unwrap();

        retain_verified_base(&base_dir, &source, "hash").unwrap();
        std::fs::remove_dir_all(&source_dir).unwrap(); // clear_download_cache()

        let (path, sha256) = retained_base(&base_dir).expect("base must outlive its source");
        assert_eq!(std::fs::read(&path).unwrap(), b"installed msix bytes");
        assert_eq!(sha256, "hash");
        std::fs::remove_dir_all(&base_dir).ok();
    }

    #[cfg(unix)]
    #[test]
    fn retention_hard_links_instead_of_copying_when_it_can() {
        use std::os::unix::fs::MetadataExt;
        let base_dir = temp_dir("hardlink");
        let source_dir = temp_dir("hardlink-source");
        let source = source_dir.join("Codex.msix");
        std::fs::write(&source, b"bytes").unwrap();

        let dest = retain_verified_base(&base_dir, &source, "h").unwrap();
        let (a, b) = (std::fs::metadata(&source).unwrap(), std::fs::metadata(&dest).unwrap());
        assert_eq!(a.ino(), b.ino(), "same inode: no second copy of the data");
        assert_eq!(a.nlink(), 2);
        assert!(!base_dir.join(format!("{BASE_FILE_NAME}.tmp")).exists());

        std::fs::remove_dir_all(&base_dir).ok();
        std::fs::remove_dir_all(&source_dir).ok();
    }

    #[test]
    fn it_falls_back_to_a_copy_when_hard_linking_is_impossible() {
        let base_dir = temp_dir("copy-fallback");
        let source_dir = temp_dir("copy-fallback-source");
        let source = source_dir.join("Codex.msix");
        std::fs::write(&source, b"copied bytes").unwrap();

        let no_links = |_: &Path, _: &Path| -> io::Result<()> {
            Err(io::Error::other("cross-device link"))
        };
        let dest = retain_verified_base_with(&base_dir, &source, "h", &no_links).unwrap();
        assert_eq!(std::fs::read(&dest).unwrap(), b"copied bytes");
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(std::fs::metadata(&source).unwrap().nlink(), 1, "a real copy, not a link");
        }
        assert_eq!(retained_base(&base_dir).unwrap().1, "h");

        std::fs::remove_dir_all(&base_dir).ok();
        std::fs::remove_dir_all(&source_dir).ok();
    }

    #[test]
    fn re_retaining_the_file_that_is_already_the_base_leaves_no_stray_temp() {
        let base_dir = temp_dir("re-retain");
        let source_dir = temp_dir("re-retain-source");
        let source = source_dir.join("Codex.msix");
        std::fs::write(&source, b"bytes").unwrap();
        let dest = retain_verified_base(&base_dir, &source, "h1").unwrap();
        // Retaining the retained base itself (same inode under two names).
        retain_verified_base(&base_dir, &dest, "h1").unwrap();
        let entries = std::fs::read_dir(&base_dir).unwrap().count();
        assert_eq!(entries, 2, "only the base and its sidecar");
        assert_eq!(std::fs::read(&dest).unwrap(), b"bytes");
        std::fs::remove_dir_all(&base_dir).ok();
        std::fs::remove_dir_all(&source_dir).ok();
    }

    #[test]
    fn a_second_retain_call_replaces_rather_than_accumulates() {
        let base_dir = temp_dir("replace");
        let source_dir = temp_dir("replace-source");
        let first = source_dir.join("first.msix");
        let second = source_dir.join("second.msix");
        std::fs::write(&first, b"version one").unwrap();
        std::fs::write(&second, b"version two, longer content").unwrap();

        retain_verified_base(&base_dir, &first, "hash-one").unwrap();
        retain_verified_base(&base_dir, &second, "hash-two").unwrap();

        // Exactly one base file + one sidecar -- never one per version.
        let entries: Vec<_> = std::fs::read_dir(&base_dir)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(entries.len(), 2, "{entries:?}");

        let (path, sha256) = retained_base(&base_dir).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"version two, longer content");
        assert_eq!(sha256, "hash-two");

        std::fs::remove_dir_all(&base_dir).ok();
        std::fs::remove_dir_all(&source_dir).ok();
    }

    #[test]
    fn a_replace_interrupted_before_the_new_sidecar_is_written_is_not_usable() {
        let base_dir = temp_dir("interrupted-replace");
        let source_dir = temp_dir("interrupted-replace-source");
        let first = source_dir.join("first.msix");
        let second = source_dir.join("second.msix");
        std::fs::write(&first, b"version one").unwrap();
        std::fs::write(&second, b"version two").unwrap();

        retain_verified_base(&base_dir, &first, "hash-one").unwrap();

        // Directly simulate a base file that exists with no sidecar at all
        // -- one possible shape an interruption could in principle leave
        // behind, and the one `retained_base` must always treat as unusable
        // regardless of how it was reached (this does not depend on
        // `retain_verified_base`'s own internal step ordering, which is
        // covered on its own by the tests below).
        std::fs::copy(&second, base_dir.join(BASE_FILE_NAME)).unwrap();
        std::fs::remove_file(base_dir.join(BASE_SHA256_FILE_NAME)).unwrap();

        assert!(
            retained_base(&base_dir).is_none(),
            "a base with no sidecar at all must never be treated as usable"
        );

        std::fs::remove_dir_all(&base_dir).ok();
        std::fs::remove_dir_all(&source_dir).ok();
    }

    #[test]
    fn a_failed_replacement_leaves_the_previous_base_usable_and_cleans_up_the_temp_file() {
        let base_dir = temp_dir("failed-replace");
        let source_dir = temp_dir("failed-replace-source");
        let first = source_dir.join("first.msix");
        std::fs::write(&first, b"version one").unwrap();
        let missing_source = source_dir.join("does-not-exist.msix");

        retain_verified_base(&base_dir, &first, "hash-one").unwrap();

        // A second call whose source can't even be read (standing in for
        // any failure during the copy -- a full disk behaves the same way
        // from this function's point of view: the copy step fails) must
        // leave the previous, still-good (base, sha256) pair exactly as it
        // was, and must not leave its own `.tmp` file behind for nothing to
        // ever clean up (`clear_retained_base` only knows the final names).
        let err = retain_verified_base(&base_dir, &missing_source, "hash-two").unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);

        assert!(
            !base_dir.join(format!("{BASE_FILE_NAME}.tmp")).exists(),
            "a failed copy must not leave its temp file behind"
        );
        let (path, sha256) = retained_base(&base_dir).expect("previous base must still be usable");
        assert_eq!(std::fs::read(&path).unwrap(), b"version one");
        assert_eq!(sha256, "hash-one");

        std::fs::remove_dir_all(&base_dir).ok();
        std::fs::remove_dir_all(&source_dir).ok();
    }

    #[test]
    fn a_failed_sidecar_write_after_the_base_swap_does_not_leave_a_mismatched_pair() {
        let base_dir = temp_dir("sidecar-fails");
        let source_dir = temp_dir("sidecar-fails-source");
        let first = source_dir.join("first.msix");
        let second = source_dir.join("second.msix");
        std::fs::write(&first, b"version one").unwrap();
        std::fs::write(&second, b"version two").unwrap();
        retain_verified_base(&base_dir, &first, "hash-one").unwrap();

        // A directory squatting on the sidecar's temp name makes the sidecar
        // write fail after the new base has already replaced the old one.
        std::fs::create_dir(base_dir.join(format!("{BASE_SHA256_FILE_NAME}.tmp"))).unwrap();
        assert!(retain_verified_base(&base_dir, &second, "hash-two").is_err());

        assert!(
            retained_base(&base_dir).is_none(),
            "the new base must never be reported under the previous release's checksum"
        );
        assert!(!base_dir.join(BASE_FILE_NAME).exists(), "the unusable base is reclaimed");

        std::fs::remove_dir_all(&base_dir).ok();
        std::fs::remove_dir_all(&source_dir).ok();
    }

    #[test]
    fn a_base_with_no_sidecar_checksum_is_not_usable() {
        let base_dir = temp_dir("no-sidecar");
        std::fs::write(base_dir.join(BASE_FILE_NAME), b"orphaned base").unwrap();
        assert!(retained_base(&base_dir).is_none());
        std::fs::remove_dir_all(&base_dir).ok();
    }

    #[test]
    fn clear_removes_both_files_and_tolerates_being_called_twice() {
        let base_dir = temp_dir("clear");
        let source_dir = temp_dir("clear-source");
        let source = source_dir.join("Codex.msix");
        std::fs::write(&source, b"bytes").unwrap();
        retain_verified_base(&base_dir, &source, "deadbeef").unwrap();

        clear_retained_base(&base_dir).unwrap();
        assert!(retained_base(&base_dir).is_none());
        assert!(!base_dir.join(BASE_FILE_NAME).exists());
        assert!(!base_dir.join(BASE_SHA256_FILE_NAME).exists());

        // Idempotent: clearing an already-empty directory is not an error.
        clear_retained_base(&base_dir).unwrap();

        std::fs::remove_dir_all(&base_dir).ok();
        std::fs::remove_dir_all(&source_dir).ok();
    }
}
