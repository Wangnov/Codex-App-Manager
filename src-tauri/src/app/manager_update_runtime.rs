//! Renderer-independent state for the manager's own updater.
//!
//! Tauri commands outlive a WebView navigation. Keeping this state behind the
//! application `ManagerState` lets a freshly loaded renderer reattach to an
//! in-flight download, recover a terminal error, or relaunch after a successful
//! macOS install whose original JavaScript caller disappeared.

use std::sync::{Arc, Mutex};

use serde::Serialize;

use crate::errors::CommandError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum ManagerUpdateRuntimePhase {
    Downloading,
    Installing,
    Installed,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ManagerUpdateRuntimeSnapshot {
    pub revision: u64,
    pub version: String,
    pub current_version: String,
    pub body: Option<String>,
    pub phase: ManagerUpdateRuntimePhase,
    pub downloaded: u64,
    pub total: Option<u64>,
    pub failure: Option<CommandError>,
    pub handoff_started_at: Option<u64>,
}

#[derive(Debug, Default)]
struct Inner {
    revision: u64,
    snapshot: Option<ManagerUpdateRuntimeSnapshot>,
}

#[derive(Clone, Default)]
pub struct ManagerUpdateRuntime {
    inner: Arc<Mutex<Inner>>,
}

impl ManagerUpdateRuntime {
    pub fn snapshot(&self) -> Option<ManagerUpdateRuntimeSnapshot> {
        self.inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .snapshot
            .clone()
    }

    pub fn begin(
        &self,
        version: String,
        current_version: String,
        body: Option<String>,
    ) -> ManagerUpdateRuntimeSnapshot {
        self.replace(|revision| ManagerUpdateRuntimeSnapshot {
            revision,
            version,
            current_version,
            body,
            phase: ManagerUpdateRuntimePhase::Downloading,
            downloaded: 0,
            total: None,
            failure: None,
            handoff_started_at: None,
        })
    }

    pub fn downloading(
        &self,
        downloaded: u64,
        total: Option<u64>,
    ) -> Option<ManagerUpdateRuntimeSnapshot> {
        self.mutate(|snapshot| {
            snapshot.phase = ManagerUpdateRuntimePhase::Downloading;
            snapshot.downloaded = downloaded;
            snapshot.total = total;
            snapshot.failure = None;
            snapshot.handoff_started_at = None;
        })
    }

    pub fn installing(
        &self,
        handoff_started_at: Option<u64>,
    ) -> Option<ManagerUpdateRuntimeSnapshot> {
        self.mutate(|snapshot| {
            snapshot.phase = ManagerUpdateRuntimePhase::Installing;
            snapshot.failure = None;
            snapshot.handoff_started_at = handoff_started_at;
        })
    }

    pub fn recover_installing(
        &self,
        version: String,
        current_version: String,
        body: Option<String>,
        handoff_started_at: u64,
    ) -> ManagerUpdateRuntimeSnapshot {
        self.replace(|revision| ManagerUpdateRuntimeSnapshot {
            revision,
            version,
            current_version,
            body,
            phase: ManagerUpdateRuntimePhase::Installing,
            downloaded: 0,
            total: None,
            failure: None,
            handoff_started_at: Some(handoff_started_at),
        })
    }

    pub fn installed(&self) -> Option<ManagerUpdateRuntimeSnapshot> {
        self.mutate(|snapshot| {
            snapshot.phase = ManagerUpdateRuntimePhase::Installed;
            snapshot.failure = None;
            snapshot.handoff_started_at = None;
        })
    }

    pub fn failed(&self, failure: CommandError) -> Option<ManagerUpdateRuntimeSnapshot> {
        self.mutate(|snapshot| {
            snapshot.phase = ManagerUpdateRuntimePhase::Error;
            snapshot.failure = Some(failure);
            snapshot.handoff_started_at = None;
        })
    }

    pub fn acknowledge_terminal(
        &self,
        revision: u64,
        version: &str,
        current_version: &str,
    ) -> bool {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let should_clear = inner.snapshot.as_ref().is_some_and(|snapshot| {
            snapshot.revision == revision
                && snapshot.version == version
                && snapshot.current_version == current_version
                && matches!(
                    snapshot.phase,
                    ManagerUpdateRuntimePhase::Installed | ManagerUpdateRuntimePhase::Error
                )
        });
        if should_clear {
            inner.snapshot = None;
        }
        should_clear
    }

    fn replace(
        &self,
        create: impl FnOnce(u64) -> ManagerUpdateRuntimeSnapshot,
    ) -> ManagerUpdateRuntimeSnapshot {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.revision = inner.revision.saturating_add(1);
        let snapshot = create(inner.revision);
        inner.snapshot = Some(snapshot.clone());
        snapshot
    }

    fn mutate(
        &self,
        update: impl FnOnce(&mut ManagerUpdateRuntimeSnapshot),
    ) -> Option<ManagerUpdateRuntimeSnapshot> {
        let mut inner = self
            .inner
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        inner.revision = inner.revision.saturating_add(1);
        let revision = inner.revision;
        let snapshot = inner.snapshot.as_mut()?;
        update(snapshot);
        snapshot.revision = revision;
        Some(snapshot.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{ManagerUpdateRuntime, ManagerUpdateRuntimePhase};
    use crate::errors::CommandError;

    #[test]
    fn preserves_progress_and_terminal_state_across_handles() {
        let runtime = ManagerUpdateRuntime::default();
        let other_handle = runtime.clone();

        let started = runtime.begin(
            "2.0.0".to_string(),
            "1.0.0".to_string(),
            Some("notes".to_string()),
        );
        let progress = runtime.downloading(5, Some(10)).unwrap();
        let installing = runtime.installing(Some(123)).unwrap();
        let installed = runtime.installed().unwrap();

        assert!(started.revision < progress.revision);
        assert!(progress.revision < installing.revision);
        assert!(installing.revision < installed.revision);
        assert_eq!(installed.phase, ManagerUpdateRuntimePhase::Installed);
        assert_eq!(installing.handoff_started_at, Some(123));
        assert_eq!(installed.handoff_started_at, None);
        assert_eq!(installed.downloaded, 5);
        assert_eq!(other_handle.snapshot(), Some(installed));
    }

    #[test]
    fn records_structured_terminal_failure() {
        let runtime = ManagerUpdateRuntime::default();
        runtime.begin("2.0.0".into(), "1.0.0".into(), None);
        let failed = runtime
            .failed(CommandError {
                code: "network".into(),
                message: "offline".into(),
            })
            .unwrap();

        assert_eq!(failed.phase, ManagerUpdateRuntimePhase::Error);
        assert_eq!(failed.failure.unwrap().code, "network");
    }

    #[test]
    fn terminal_ack_is_compare_and_swap_safe() {
        let runtime = ManagerUpdateRuntime::default();
        runtime.begin("2.0.0".into(), "1.0.0".into(), None);
        let failed = runtime
            .failed(CommandError {
                code: "stale_expectation".into(),
                message: "changed".into(),
            })
            .unwrap();

        assert!(!runtime.acknowledge_terminal(
            failed.revision.saturating_sub(1),
            &failed.version,
            &failed.current_version,
        ));
        assert!(runtime.snapshot().is_some());
        assert!(runtime.acknowledge_terminal(
            failed.revision,
            &failed.version,
            &failed.current_version,
        ));
        assert!(runtime.snapshot().is_none());

        let active = runtime.begin("3.0.0".into(), "2.0.0".into(), None);
        assert!(!runtime.acknowledge_terminal(
            active.revision,
            &active.version,
            &active.current_version,
        ));
        assert_eq!(runtime.snapshot(), Some(active));
    }
}
