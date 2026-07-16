//! In-process theme keeper. The studio ran a detached Node watcher polling a
//! state file; inside the manager the same loop is a tokio task fed by a
//! `watch` channel, reconciling *desired* state (the directive) against
//! *actual* state (the stamp each renderer carries):
//!
//! - every ~900 ms, list `app://` targets, connect/probe new ones, drop dead
//!   ones — quick-chat windows and prewarm shells get themed as they appear;
//! - per session, read `window.__CODEX_THEME_STUDIO__?.stamp` and re-inject /
//!   remove only on mismatch (the payload is itself idempotent, this just
//!   avoids pointless 30 MB evaluates). A page reload resets the stamp to
//!   null, so recovery is at most one tick behind — replacing the studio's
//!   `Page.loadEventFired` + 300 ms dance with something stateless.
//!
//! Dropping the directive sender stops the daemon **without** un-theming: the
//! manager quitting must not strip a running Codex (the injected runtime
//! keeps the theme alive until reload; only an explicit `None` directive
//! removes it).

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use serde::Serialize;
use tokio::sync::watch;

use crate::cdp::{list_app_targets, probe_session, CdpSession};
use crate::payload::{build_payload, BuiltPayload, CURRENT_STAMP_EXPRESSION, REMOVE_EXPRESSION};

const TICK: Duration = Duration::from_millis(900);

#[derive(Debug, Clone, Serialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DaemonStatus {
    pub running: bool,
    pub port: u16,
    pub theme_id: Option<String>,
    pub stamp: Option<String>,
    pub connected_targets: usize,
    pub last_error: Option<String>,
}

/// Desired state: `Some(theme_dir)` keeps that theme applied everywhere,
/// `None` keeps renderers stock. Send the same directory again to force a
/// payload rebuild (theme files changed on disk during development).
pub type Directive = Option<PathBuf>;

pub async fn run_daemon(
    port: u16,
    mut directive_rx: watch::Receiver<Directive>,
    status_tx: watch::Sender<DaemonStatus>,
) {
    let mut sessions: HashMap<String, CdpSession> = HashMap::new();
    let mut built: Option<BuiltPayload>;
    let mut last_error: Option<String> = None;

    // Build the initial payload for whatever directive we started with.
    let mut rebuild_for = directive_rx.borrow().clone();
    loop {
        if let Some(dir) = &rebuild_for {
            match build_payload(dir) {
                Ok(payload) => {
                    log::info!(
                        "theme payload built id={} bytes={} assets={}",
                        payload.theme.id,
                        payload.payload_bytes,
                        payload.asset_count
                    );
                    built = Some(payload);
                }
                Err(error) => {
                    log::warn!("theme payload build failed: {error}");
                    last_error = Some(error.to_string());
                    built = None;
                }
            }
        } else {
            built = None;
        }

        // Reconcile until the directive changes (or the sender goes away).
        loop {
            // 1. Target census.
            match list_app_targets(port).await {
                Ok(targets) => {
                    let active: std::collections::HashSet<&str> =
                        targets.iter().map(|t| t.id.as_str()).collect();
                    sessions.retain(|id, session| {
                        let keep = active.contains(id.as_str()) && !session.closed();
                        if !keep {
                            session.close();
                        }
                        keep
                    });
                    for target in targets {
                        if sessions.contains_key(&target.id) {
                            continue;
                        }
                        let id = target.id.clone();
                        match CdpSession::connect(target, port).await {
                            Ok(session) => match probe_session(&session).await {
                                Ok(probe) if probe.codex => {
                                    log::info!("theme daemon connected target {id}");
                                    sessions.insert(id, session);
                                }
                                Ok(_) => session.close(),
                                Err(error) => {
                                    session.close();
                                    last_error = Some(error.to_string());
                                }
                            },
                            Err(error) => last_error = Some(error.to_string()),
                        }
                    }
                }
                Err(error) => {
                    // Codex not running (or CDP gone): drop every session and
                    // report quietly — this is a normal resting state.
                    for session in sessions.values() {
                        session.close();
                    }
                    sessions.clear();
                    last_error = Some(error.to_string());
                }
            }

            // 2. Stamp reconciliation per session.
            let expected = built.as_ref().map(|b| b.stamp.clone());
            let mut dead = Vec::new();
            for (id, session) in &sessions {
                let current = match session.evaluate(CURRENT_STAMP_EXPRESSION).await {
                    Ok(value) => value.as_str().map(|s| s.to_string()),
                    Err(error) => {
                        log::debug!("stamp probe failed for {id}: {error}");
                        dead.push(id.clone());
                        continue;
                    }
                };
                let outcome = match (&expected, &current) {
                    (Some(stamp), Some(applied)) if stamp == applied => Ok(()),
                    (Some(_), _) => session
                        .evaluate(&built.as_ref().expect("built payload").payload)
                        .await
                        .map(|_| ()),
                    (None, Some(_)) => session.evaluate(REMOVE_EXPRESSION).await.map(|_| ()),
                    (None, None) => Ok(()),
                };
                if let Err(error) = outcome {
                    log::warn!("theme apply failed for {id}: {error}");
                    last_error = Some(error.to_string());
                    dead.push(id.clone());
                }
            }
            for id in dead {
                if let Some(session) = sessions.remove(&id) {
                    session.close();
                }
            }

            let _ = status_tx.send(DaemonStatus {
                running: true,
                port,
                theme_id: built.as_ref().map(|b| b.theme.id.clone()),
                stamp: expected,
                connected_targets: sessions.len(),
                last_error: last_error.clone(),
            });

            // 3. Wait one tick, reacting immediately to directive changes.
            tokio::select! {
                changed = directive_rx.changed() => {
                    if changed.is_err() {
                        // Manager shutting down: leave renderers as they are.
                        for session in sessions.values() {
                            session.close();
                        }
                        let _ = status_tx.send(DaemonStatus {
                            running: false,
                            port,
                            ..DaemonStatus::default()
                        });
                        return;
                    }
                    rebuild_for = directive_rx.borrow().clone();
                    last_error = None;
                    break; // outer loop rebuilds the payload
                }
                _ = tokio::time::sleep(TICK) => {}
            }
        }
    }
}
