use std::sync::atomic::AtomicBool;

use crate::adapters::host;
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
}

impl ManagerState {
    pub fn new() -> Self {
        let target = Target::current();
        let mirror_base_url = "https://codexapp.agentsmirror.com".to_string();
        let saved = PersistedAppSettings::load();
        let install_root = if saved.install_root.trim().is_empty() {
            host::default_install_root(&target)
        } else {
            saved.install_root
        };
        let settings = AppSettings::new(mirror_base_url.clone(), install_root);
        let endpoints = MirrorEndpoints::from_base_url(&mirror_base_url);

        Self {
            target,
            settings,
            endpoints,
            force_quit: AtomicBool::new(false),
        }
    }
}

impl Default for ManagerState {
    fn default() -> Self {
        Self::new()
    }
}
