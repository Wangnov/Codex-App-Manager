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
    let pattern = format!("^{}( |$)", ere_escape(&format!("{app}/Contents/MacOS/{exe}")));
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

/// Atomically replace `install_app` with `new_app`, preserving the previous
/// bundle at `backup_app`. On failure after the old bundle is moved aside, the
/// old bundle is restored before returning the error.
pub fn swap_in_place(
    install_app: &Path,
    new_app: &Path,
    backup_app: &Path,
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
    let had_old = install_app.exists();
    if had_old {
        std::fs::rename(install_app, backup_app)
            .map_err(|e| EngineError::Io(format!("move current bundle aside: {e}")))?;
    }

    match std::fs::rename(new_app, install_app) {
        Ok(()) => {
            let install_path = install_app.display();
            log::info!("atomic swap completed install_path={install_path}");
            Ok(())
        }
        Err(e) => {
            if had_old {
                let _ = std::fs::rename(backup_app, install_app);
            }
            let err = EngineError::Io(format!(
                "install new bundle failed (rolled back): {e}"
            ));
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

    // Sandbox swap + rollback round-trip. Never touches /Applications.
    #[test]
    fn swap_and_rollback_roundtrip() {
        let root = std::env::temp_dir().join(format!("codex-swap-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();

        let install = root.join("Codex.app");
        let new_app = root.join("new-Codex.app");
        let backup = root.join("backup-Codex.app");

        fs::create_dir_all(install.join("Contents")).unwrap();
        fs::write(install.join("Contents/ver"), "3511").unwrap();
        fs::create_dir_all(new_app.join("Contents")).unwrap();
        fs::write(new_app.join("Contents/ver"), "3575").unwrap();

        swap_in_place(&install, &new_app, &backup).unwrap();
        assert_eq!(fs::read_to_string(install.join("Contents/ver")).unwrap(), "3575");
        assert!(backup.join("Contents/ver").exists(), "old bundle preserved");
        assert!(!new_app.exists(), "new bundle moved into place");

        rollback(&install, &backup).unwrap();
        assert_eq!(fs::read_to_string(install.join("Contents/ver")).unwrap(), "3511");

        let _ = fs::remove_dir_all(&root);
    }
}
