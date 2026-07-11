use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use crate::adapters::host;
use crate::app::config_health::ConfigHealth;
use crate::app::manager_update_handoff::{
    status_for_platform as manager_update_handoff_status, ManagerUpdateHandoff,
    ManagerUpdateHandoffStatus,
};
use crate::app::manager_update_runtime::ManagerUpdateRuntime;
use crate::app::oplock::{OperationGuard, OperationKind, OperationManager};
use crate::app::provenance::ProvenanceStore;
use crate::app::settings_store::AppSettings as PersistedAppSettings;
use crate::domain::manifest::MirrorEndpoints;
use crate::domain::settings::AppSettings;
use crate::domain::target::Target;

pub struct ManagerState {
    pub target: Target,
    pub settings: AppSettings,
    pub endpoints: MirrorEndpoints,
    /// Set once the user confirms quitting (or has the guard off) so the close /
    /// exit handlers stop intercepting and let the process go.
    pub force_quit: AtomicBool,
    pub operations: OperationManager,
    pub manager_update: ManagerUpdateRuntime,
    /// Holds the shared operation lock when an old Windows build is reopened
    /// while its updater-owned NSIS child is still replacing files.
    pub manager_update_handoff_guard: Mutex<Option<OperationGuard>>,
    pub config_health: Mutex<ConfigHealth>,
}

fn restore_manager_update_handoff(runtime: &ManagerUpdateRuntime, record: &ManagerUpdateHandoff) {
    runtime.recover_installing(
        record.version.clone(),
        record.current_version.clone(),
        record.body.clone(),
        record.started_at_unix_ms,
    );
}

pub(crate) fn manager_update_handoff_timeout() -> crate::errors::CommandError {
    crate::errors::CommandError {
        code: "timeout".to_string(),
        message: "manager updater handoff expired before the target version launched".to_string(),
    }
}

fn restore_manager_update_handoff_status(
    status: ManagerUpdateHandoffStatus,
    operations: &OperationManager,
    runtime: &ManagerUpdateRuntime,
) -> Option<OperationGuard> {
    match status {
        ManagerUpdateHandoffStatus::Active(record) => {
            restore_manager_update_handoff(runtime, &record);
            match operations.begin(OperationKind::ManagerUpdate) {
                Ok(guard) => Some(guard),
                Err(error) => {
                    // Another live manager may still own the same filesystem
                    // lock. The restored runtime still fences this renderer.
                    log::warn!("could not restore manager handoff operation lock: {error}");
                    None
                }
            }
        }
        ManagerUpdateHandoffStatus::Expired(record) => {
            restore_manager_update_handoff(runtime, &record);
            runtime.failed(manager_update_handoff_timeout());
            None
        }
        ManagerUpdateHandoffStatus::None => None,
    }
}

impl ManagerState {
    pub fn new() -> Self {
        let target = Target::current();
        let mirror_base_url = "https://codexapp.agentsmirror.com".to_string();
        let (saved, settings_health) = PersistedAppSettings::load_with_health();
        let (_, provenance_health) = ProvenanceStore::load_with_health();
        let config_health = Mutex::new(
            ConfigHealth::from_parts(settings_health, provenance_health).with_live_backup_flags(),
        );
        let install_root = if saved.install_root.trim().is_empty() {
            host::default_install_root(&target)
        } else {
            saved.install_root
        };
        let settings = AppSettings::new(mirror_base_url.clone(), install_root);
        let endpoints = MirrorEndpoints::from_base_url(&mirror_base_url);
        let lock_path = crate::app::paths::data_dir()
            .map(|dir| dir.join("operation.lock"))
            .unwrap_or_else(|| std::env::temp_dir().join("codex-app-manager-operation.lock"));
        let operations = OperationManager::new(lock_path);
        let manager_update = ManagerUpdateRuntime::default();

        Self {
            target,
            settings,
            endpoints,
            force_quit: AtomicBool::new(false),
            operations,
            manager_update,
            manager_update_handoff_guard: Mutex::new(None),
            config_health,
        }
    }

    pub(crate) fn restore_manager_update_handoff(&self, current_version: &str) {
        let guard = restore_manager_update_handoff_status(
            manager_update_handoff_status(current_version),
            &self.operations,
            &self.manager_update,
        );
        *self
            .manager_update_handoff_guard
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = guard;
    }
}

impl Default for ManagerState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        sync::atomic::{AtomicU64, Ordering},
    };

    use super::restore_manager_update_handoff_status;
    use crate::app::manager_update_handoff::{ManagerUpdateHandoff, ManagerUpdateHandoffStatus};
    use crate::app::manager_update_runtime::{ManagerUpdateRuntime, ManagerUpdateRuntimePhase};
    use crate::app::oplock::{OperationError, OperationKind, OperationManager};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

    fn operations(name: &str) -> (OperationManager, std::path::PathBuf) {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-data")
            .join(format!("state-handoff-{name}-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        (OperationManager::new(dir.join("operation.lock")), dir)
    }

    fn record() -> ManagerUpdateHandoff {
        ManagerUpdateHandoff {
            version: "2.0.0".into(),
            current_version: "1.0.0".into(),
            body: Some("notes".into()),
            started_at_unix_ms: 123,
        }
    }

    #[test]
    fn active_handoff_restores_runtime_and_blocks_codex_operations() {
        let (operations, dir) = operations("active");
        let runtime = ManagerUpdateRuntime::default();
        let guard = restore_manager_update_handoff_status(
            ManagerUpdateHandoffStatus::Active(record()),
            &operations,
            &runtime,
        )
        .unwrap();

        let snapshot = runtime.snapshot().unwrap();
        assert_eq!(snapshot.phase, ManagerUpdateRuntimePhase::Installing);
        assert_eq!(snapshot.handoff_started_at, Some(123));
        assert!(matches!(
            operations.begin(OperationKind::Update),
            Err(OperationError::BusySameProcess("manager-update"))
        ));

        drop(guard);
        assert!(operations.begin(OperationKind::Update).is_ok());
        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn expired_handoff_restores_a_retryable_terminal_without_a_lock() {
        let (operations, dir) = operations("expired");
        let runtime = ManagerUpdateRuntime::default();
        let guard = restore_manager_update_handoff_status(
            ManagerUpdateHandoffStatus::Expired(record()),
            &operations,
            &runtime,
        );

        assert!(guard.is_none());
        let snapshot = runtime.snapshot().unwrap();
        assert_eq!(snapshot.phase, ManagerUpdateRuntimePhase::Error);
        assert_eq!(snapshot.failure.unwrap().code, "timeout");
        assert!(operations.begin(OperationKind::Update).is_ok());
        let _ = fs::remove_dir_all(dir);
    }
}
