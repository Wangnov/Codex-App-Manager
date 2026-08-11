//! The destructive tail of a macOS update: gracefully quit Codex, atomically
//! swap the reconstructed bundle into the install root (same-volume rename),
//! relaunch, and roll back on failure.
//!
//! Safety posture:
//!   - NEVER force-kills Codex (an in-flight agent run must be allowed to finish
//!     / save). We ask it to quit and wait; if it refuses, we abort.
//!   - The swap keeps the previous bundle as a backup until the caller confirms
//!     the new version launched healthily, so rollback is always possible.
//!   - Atomic swap requires the staged bundle to live on the *same volume* as
//!     the install root (cross-volume rename is not atomic); we check and refuse
//!     otherwise.

use std::path::Path;
use std::process::Command;

use crate::EngineError;

const OPEN: &str = "/usr/bin/open";
const OSASCRIPT: &str = "/usr/bin/osascript";
const PGREP: &str = "/usr/bin/pgrep";

/// Escape a literal string for use inside a POSIX extended regular expression
/// (the pattern language `pgrep -f` matches against).
fn ere_escape(literal: &str) -> String {
    let mut escaped = String::with_capacity(literal.len());
    for ch in literal.chars() {
        if matches!(
            ch,
            '\\' | '^' | '$' | '.' | '[' | ']' | '|' | '(' | ')' | '*' | '+' | '?' | '{' | '}'
        ) {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

/// Escape a literal for embedding in a double-quoted AppleScript string.
fn applescript_quote(literal: &str) -> String {
    literal.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Is THIS install of Codex currently running?
///
/// Addressed by the install's own executable path, not by process name or
/// bundle id: the brand merge renamed the executable (`Codex` → `ChatGPT`,
/// same as ChatGPT Classic's process), and bundle-id addressing resolves via
/// LaunchServices, which may pick a DIFFERENT install of the same id when
/// several coexist. The main process's argv[0] is the bundle's
/// `Contents/MacOS/<CFBundleExecutable>` absolute path, so a prefix match on
/// that pins the exact instance we manage.
///
/// The path is canonicalized first: argv[0] holds the RESOLVED path, so a
/// symlinked install root (or parent) would otherwise never match and the
/// quit-before-swap protection would be silently skipped.
pub fn codex_running_at(install_app: &Path) -> bool {
    let canonical = install_app
        .canonicalize()
        .unwrap_or_else(|_| install_app.to_path_buf());
    let app = canonical.to_string_lossy();
    let Some(exe) = crate::sys::read_bundle_executable(&app) else {
        // No readable bundle at the path (e.g. already removed) — not running.
        return false;
    };
    let pattern = format!(
        "^{}( |$)",
        ere_escape(&format!("{app}/Contents/MacOS/{exe}"))
    );
    Command::new(PGREP)
        .args(["-f", &pattern])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Is a live process running out of an App Translocation mount for a bundle
/// with this name? Translocated instances do NOT move back when the original
/// bundle is de-quarantined or relocated, so they stay invisible to
/// `codex_running_at`'s path-scoped match. Matching is by bundle basename
/// (the tightest anchor a randomized mount allows), which can also hit a
/// translocated ChatGPT Classic — an acceptable false positive: callers only
/// use this to REFUSE managing until the instance exits.
pub fn translocated_instance_running(install_app: &Path) -> bool {
    let Some(bundle_name) = install_app.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let pattern = format!(
        "/AppTranslocation/.*/{}/Contents/MacOS/",
        ere_escape(bundle_name)
    );
    Command::new(PGREP)
        .args(["-f", &pattern])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Send `quit`/`activate` to the app bundle AT THIS PATH. AppleScript's
/// `tell application "<POSIX path>"` addresses that specific bundle, unlike
/// `tell application id`, which lets LaunchServices pick any install of the
/// id. Callers must gate on `codex_running_at` first: telling a non-running
/// app launches it.
fn tell_codex_at(install_app: &Path, verb: &str) -> std::io::Result<std::process::ExitStatus> {
    let script = format!(
        r#"tell application "{}" to {verb}"#,
        applescript_quote(&install_app.to_string_lossy())
    );
    Command::new(OSASCRIPT).args(["-e", &script]).status()
}

/// Ask the Codex install at `install_app` to quit gracefully (AppleScript),
/// polling up to `timeout_secs`. Never force-kills.
///
/// Codex may answer the quit event with its own in-app confirmation dialog
/// instead of quitting (e.g. "Quit Codex? Enabled automations won't run…"
/// when automations are enabled). When Codex isn't frontmost that dialog sits
/// on a window the user never sees, so the quit silently stalls. After a short
/// grace period we therefore `activate` Codex — bringing the pending dialog
/// frontmost so the user can answer it — and keep waiting until the timeout.
pub fn quit_codex_at(install_app: &Path, timeout_secs: u64) -> Result<(), EngineError> {
    // Resolve symlinks once so detection (argv[0] holds the real path) and the
    // AppleScript address agree on the same concrete bundle.
    let install_app = install_app
        .canonicalize()
        .unwrap_or_else(|_| install_app.to_path_buf());
    if !codex_running_at(&install_app) {
        return Ok(());
    }
    log::info!(
        "requesting Codex graceful quit timeout={timeout_secs} path={}",
        install_app.display()
    );
    let _ = tell_codex_at(&install_app, "quit");

    // 250ms ticks; if Codex is still running after ~5s it is most likely
    // waiting on its quit-confirmation dialog — surface it.
    let activate_tick = 5 * 4;
    for tick in 0..(timeout_secs * 4) {
        if !codex_running_at(&install_app) {
            return Ok(());
        }
        if tick == activate_tick {
            let _ = tell_codex_at(&install_app, "activate");
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    log::warn!("Codex graceful quit timed out");
    Err(EngineError::Io(
        "Codex did not quit within the timeout — it may be waiting on its own \
         quit-confirmation dialog (e.g. \"Quit Codex?\" when automations are \
         enabled); confirm the quit in Codex and retry (left running to \
         protect in-flight work)"
            .to_string(),
    ))
}

/// Are `a` and `b` on the same filesystem volume? This is the precondition for
/// an atomic `rename` swap. Exposed so callers can pre-flight it BEFORE taking
/// destructive steps (e.g. quitting the app) rather than discovering it mid-swap.
#[cfg(unix)]
pub fn same_volume(a: &Path, b: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;
    match (std::fs::metadata(a), std::fs::metadata(b)) {
        (Ok(ma), Ok(mb)) => ma.dev() == mb.dev(),
        _ => {
            log::warn!("same-volume preflight could not read metadata");
            false
        }
    }
}

// The crate is an unconditional dependency of the cross-platform Tauri app, so
// it must compile for Windows even though the macOS swap path is never invoked
// there (Windows has its own installer path).
#[cfg(not(unix))]
pub fn same_volume(_a: &Path, _b: &Path) -> bool {
    true
}

/// Rename boundary markers for crash-recovery callbacks and fault injection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapBoundary {
    /// About to perform the first destructive rename (old → backup).
    BeforeMoveOld,
    /// Old install is at backup; install path is empty.
    AfterMoveOld,
    /// About to move staged new → install.
    BeforeMoveNew,
    /// New payload is at install path.
    AfterMoveNew,
    /// A failure chose rollback; persist that intent before consuming backup.
    BeforeRollback,
}

/// Optional observer invoked at each rename boundary. Used by the app layer to
/// persist a transaction log before/after destructive steps. Returning `Err`
/// aborts the swap (and rolls back if the old bundle was already moved).
pub type SwapObserver<'a> = dyn FnMut(SwapBoundary) -> Result<(), EngineError> + 'a;

/// Injected failure points for tests (and only tests / explicit hooks).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SwapFault {
    /// Fail before moving the old install aside.
    BeforeMoveOld,
    /// Fail after old → backup succeeds, before new → install.
    AfterMoveOld,
    /// Fail the new → install rename (simulates disk error mid-commit).
    OnMoveNew,
}

thread_local! {
    static SWAP_FAULT: std::cell::Cell<Option<SwapFault>> = const { std::cell::Cell::new(None) };
}

/// Install a one-shot fault for the next `swap_in_place*` call (test-only path).
pub fn inject_swap_fault(fault: Option<SwapFault>) {
    SWAP_FAULT.with(|cell| cell.set(fault));
}

fn take_fault() -> Option<SwapFault> {
    SWAP_FAULT.with(|cell| cell.take())
}

fn fault_err(boundary: &str) -> EngineError {
    EngineError::Io(format!("injected swap fault at {boundary}"))
}

/// Atomically replace `install_app` with `new_app`, preserving the previous
/// bundle at `backup_app`. On failure after the old bundle is moved aside, the
/// old bundle is restored before returning the error.
pub fn swap_in_place(
    install_app: &Path,
    new_app: &Path,
    backup_app: &Path,
) -> Result<(), EngineError> {
    swap_in_place_with_observer(install_app, new_app, backup_app, &mut |_| Ok(()))
}

/// Like [`swap_in_place`], but notifies `observer` at each rename boundary so
/// callers can persist a crash-recovery transaction log.
pub fn swap_in_place_with_observer(
    install_app: &Path,
    new_app: &Path,
    backup_app: &Path,
    observer: &mut SwapObserver<'_>,
) -> Result<(), EngineError> {
    let install_parent = install_app.parent().unwrap_or(install_app);
    if new_app.exists() && install_parent.exists() && !same_volume(new_app, install_parent) {
        log::warn!("atomic swap preflight rejected cross-volume target");
        return Err(EngineError::Io(
            "staged bundle is on a different volume than the install root; \
             stage it on the same volume for an atomic swap"
                .to_string(),
        ));
    }

    if backup_app.exists() {
        std::fs::remove_dir_all(backup_app)
            .map_err(|e| EngineError::Io(format!("clear stale backup: {e}")))?;
    }

    let install_path = install_app.display();
    log::info!("atomic swap start install_path={install_path}");
    let fault = take_fault();
    let had_old = install_app.exists();

    observer(SwapBoundary::BeforeMoveOld)?;
    if fault == Some(SwapFault::BeforeMoveOld) {
        return Err(fault_err("before-move-old"));
    }

    if had_old {
        std::fs::rename(install_app, backup_app)
            .map_err(|e| EngineError::Io(format!("move current bundle aside: {e}")))?;
    }

    // Point of no return for "install missing": old is at backup (if any).
    // Observer must persist OldMoved here. On failure: roll old back when
    // possible so we never leave an empty install without a recovery path.
    if let Err(obs_err) = observer(SwapBoundary::AfterMoveOld) {
        if had_old {
            let _ = observer(SwapBoundary::BeforeRollback);
            if let Err(rb) = std::fs::rename(backup_app, install_app) {
                log::error!(
                    "observer failed after move-old and rollback also failed: obs={obs_err} rollback={rb}"
                );
                // Leave crash window (backup + new present, install empty) for
                // startup recovery; do not swallow the original error.
            }
        }
        return Err(obs_err);
    }
    if fault == Some(SwapFault::AfterMoveOld) {
        // Leave paths as-is for recovery tests (do NOT auto-rollback).
        return Err(fault_err("after-move-old"));
    }

    observer(SwapBoundary::BeforeMoveNew)?;
    if fault == Some(SwapFault::OnMoveNew) {
        if had_old {
            let _ = observer(SwapBoundary::BeforeRollback);
            let _ = std::fs::rename(backup_app, install_app);
        }
        return Err(fault_err("on-move-new"));
    }

    match std::fs::rename(new_app, install_app) {
        Ok(()) => {
            if let Err(obs_err) = observer(SwapBoundary::AfterMoveNew) {
                // New is already at install; leave NewInstalled-pending for
                // recovery rather than attempting a partial undo here.
                log::error!("observer failed after move-new: {obs_err}");
                return Err(obs_err);
            }
            let install_path = install_app.display();
            log::info!("atomic swap completed install_path={install_path}");
            Ok(())
        }
        Err(e) => {
            if had_old {
                let _ = observer(SwapBoundary::BeforeRollback);
                let _ = std::fs::rename(backup_app, install_app);
            }
            let err = EngineError::Io(format!("install new bundle failed (rolled back): {e}"));
            let install_path = install_app.display();
            log::error!("atomic swap failed install_path={install_path} error={err}");
            Err(err)
        }
    }
}

/// Restore the backup bundle over the install root (used when the new version
/// fails its post-launch health check).
pub fn rollback(install_app: &Path, backup_app: &Path) -> Result<(), EngineError> {
    let install_path = install_app.display();
    log::warn!("rollback start install_path={install_path}");
    if install_app.exists() {
        std::fs::remove_dir_all(install_app)
            .map_err(|e| EngineError::Io(format!("remove failed new bundle: {e}")))?;
    }
    std::fs::rename(backup_app, install_app)
        .map_err(|e| EngineError::Io(format!("rollback: {e}")))?;
    let install_path = install_app.display();
    log::warn!("rollback completed install_path={install_path}");
    Ok(())
}

/// Relaunch Codex from the install root.
pub fn relaunch(install_app: &Path) -> Result<(), EngineError> {
    log::info!("relaunching Codex");
    let status = Command::new(OPEN)
        .arg(install_app)
        .status()
        .map_err(|e| EngineError::Io(format!("open Codex: {e}")))?;
    if !status.success() {
        let err = EngineError::Io(format!("open Codex exited with {status}"));
        log::warn!("Codex relaunch failed error={err}");
        return Err(err);
    }
    log::info!("Codex relaunch requested");
    Ok(())
}

/// Capstone: gate (codesign/Team/Gatekeeper) `new_app`, then atomically install
/// it over `install_app` keeping `backup_app`. When `manage_process` is true,
/// quits a running Codex first and relaunches after (a real install). When
/// false (rehearsal against a sandbox), the host Codex process is left alone.
///
/// Rolling back on a failed post-launch health check is the caller's decision
/// (the backup is preserved here).
pub fn install_gated_bundle(
    install_app: &Path,
    new_app: &Path,
    backup_app: &Path,
    manage_process: bool,
) -> Result<(), EngineError> {
    crate::codesign::gate_reconstructed(new_app)?;
    if manage_process {
        quit_codex_at(install_app, 30)?;
    }
    swap_in_place(install_app, new_app, backup_app)?;
    if manage_process {
        relaunch(install_app)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

    fn test_root(name: &str) -> std::path::PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let root =
            std::env::temp_dir().join(format!("codex-swap-{name}-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn seed_bundles(root: &Path) -> (std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
        let install = root.join("Codex.app");
        let new_app = root.join("new-Codex.app");
        let backup = root.join("backup-Codex.app");
        fs::create_dir_all(install.join("Contents")).unwrap();
        fs::write(install.join("Contents/ver"), "3511").unwrap();
        fs::create_dir_all(new_app.join("Contents")).unwrap();
        fs::write(new_app.join("Contents/ver"), "3575").unwrap();
        (install, new_app, backup)
    }

    // Sandbox swap + rollback round-trip. Never touches /Applications.
    #[test]
    fn swap_and_rollback_roundtrip() {
        let root = test_root("roundtrip");
        let (install, new_app, backup) = seed_bundles(&root);

        swap_in_place(&install, &new_app, &backup).unwrap();
        assert_eq!(
            fs::read_to_string(install.join("Contents/ver")).unwrap(),
            "3575"
        );
        assert!(backup.join("Contents/ver").exists(), "old bundle preserved");
        assert!(!new_app.exists(), "new bundle moved into place");

        rollback(&install, &backup).unwrap();
        assert_eq!(
            fs::read_to_string(install.join("Contents/ver")).unwrap(),
            "3511"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn observer_sees_all_boundaries_on_success() {
        let root = test_root("observer");
        let (install, new_app, backup) = seed_bundles(&root);
        let mut seen = Vec::new();
        swap_in_place_with_observer(&install, &new_app, &backup, &mut |b| {
            seen.push(b);
            Ok(())
        })
        .unwrap();
        assert_eq!(
            seen,
            [
                SwapBoundary::BeforeMoveOld,
                SwapBoundary::AfterMoveOld,
                SwapBoundary::BeforeMoveNew,
                SwapBoundary::AfterMoveNew,
            ]
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fault_before_move_old_leaves_install_intact() {
        let root = test_root("fault-before-old");
        let (install, new_app, backup) = seed_bundles(&root);
        inject_swap_fault(Some(SwapFault::BeforeMoveOld));
        let err = swap_in_place(&install, &new_app, &backup).unwrap_err();
        assert!(err.to_string().contains("before-move-old"));
        assert_eq!(
            fs::read_to_string(install.join("Contents/ver")).unwrap(),
            "3511"
        );
        assert!(new_app.exists());
        assert!(!backup.exists());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fault_after_move_old_leaves_install_empty_for_recovery() {
        let root = test_root("fault-after-old");
        let (install, new_app, backup) = seed_bundles(&root);
        inject_swap_fault(Some(SwapFault::AfterMoveOld));
        let err = swap_in_place(&install, &new_app, &backup).unwrap_err();
        assert!(err.to_string().contains("after-move-old"));
        // Crash window: old at backup, new staged, install missing.
        assert!(!install.exists());
        assert!(backup.exists());
        assert!(new_app.exists());
        assert_eq!(
            fs::read_to_string(backup.join("Contents/ver")).unwrap(),
            "3511"
        );
        assert_eq!(
            fs::read_to_string(new_app.join("Contents/ver")).unwrap(),
            "3575"
        );
        // Manual continue (recovery matrix: continue).
        fs::rename(&new_app, &install).unwrap();
        assert_eq!(
            fs::read_to_string(install.join("Contents/ver")).unwrap(),
            "3575"
        );
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn fault_on_move_new_rolls_back_old_bundle() {
        let root = test_root("fault-on-new");
        let (install, new_app, backup) = seed_bundles(&root);
        inject_swap_fault(Some(SwapFault::OnMoveNew));
        let err = swap_in_place(&install, &new_app, &backup).unwrap_err();
        assert!(err.to_string().contains("on-move-new"));
        // In-process fault path restores old immediately (same as real rename fail).
        assert!(install.exists());
        assert_eq!(
            fs::read_to_string(install.join("Contents/ver")).unwrap(),
            "3511"
        );
        assert!(new_app.exists());
        let _ = fs::remove_dir_all(&root);
    }
}
