use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use crate::adapters::host;
use crate::app::config_health::ConfigHealth;
use crate::app::oplock::OperationManager;
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
    pub config_health: Mutex<ConfigHealth>,
}

impl ManagerState {
    pub fn new() -> Self {
        let target = Target::current();
        let mirror_base_url = "https://codexapp.agentsmirror.com".to_string();
        let (saved, settings_health) = PersistedAppSettings::load_with_health();
        let (_, provenance_health) = ProvenanceStore::load_with_health();
        let config_health =
            Mutex::new(ConfigHealth::from_parts(settings_health, provenance_health));
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

        Self {
            target,
            settings,
            endpoints,
            force_quit: AtomicBool::new(false),
            operations,
            config_health,
        }
    }
}

impl Default for ManagerState {
    fn default() -> Self {
        Self::new()
    }
}
