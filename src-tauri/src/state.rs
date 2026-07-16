use std::sync::atomic::AtomicBool;
use std::sync::Mutex;

use crate::adapters::host;
use crate::app::config_health::ConfigHealth;
use crate::app::oplock::OperationManager;
use crate::app::provenance::ProvenanceStore;
use crate::app::settings_store::AppSettings as PersistedAppSettings;
use crate::app::shell::FrontendGate;
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
    /// Windows release windows stay hidden until WebView2's native browser
    /// accelerators have been disabled. The single-instance focus path must not
    /// bypass that startup gate.
    #[cfg(target_os = "windows")]
    pub webview_safe_to_show: AtomicBool,
    /// Failure wins over readiness so startup recovery never begins after the
    /// release WebView safety gate has failed.
    #[cfg(target_os = "windows")]
    pub webview_gate_failed: AtomicBool,
    pub operations: OperationManager,
    pub config_health: Mutex<ConfigHealth>,
    pub frontend: FrontendGate,
    /// Codex UI theme orchestration (daemon handle + status).
    pub codex_theme: crate::app::codex_theme::ThemeService,
}

#[cfg(any(target_os = "windows", test))]
fn initial_webview_safe_to_show(is_dev: bool) -> bool {
    is_dev
}

#[cfg(any(target_os = "windows", test))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum WebviewStartupGate {
    Wait,
    Proceed,
    Abort,
}

#[cfg(any(target_os = "windows", test))]
fn webview_startup_gate(safe_to_show: bool, failed: bool) -> WebviewStartupGate {
    if failed {
        WebviewStartupGate::Abort
    } else if safe_to_show {
        WebviewStartupGate::Proceed
    } else {
        WebviewStartupGate::Wait
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

        Self {
            target,
            settings,
            endpoints,
            force_quit: AtomicBool::new(false),
            #[cfg(target_os = "windows")]
            webview_safe_to_show: AtomicBool::new(initial_webview_safe_to_show(cfg!(dev))),
            #[cfg(target_os = "windows")]
            webview_gate_failed: AtomicBool::new(false),
            operations,
            config_health,
            frontend: FrontendGate::default(),
            codex_theme: crate::app::codex_theme::ThemeService::default(),
        }
    }

    #[cfg(target_os = "windows")]
    pub(crate) fn webview_startup_gate(&self) -> WebviewStartupGate {
        webview_startup_gate(
            self.webview_safe_to_show
                .load(std::sync::atomic::Ordering::SeqCst),
            self.webview_gate_failed
                .load(std::sync::atomic::Ordering::SeqCst),
        )
    }
}

impl Default for ManagerState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::{initial_webview_safe_to_show, webview_startup_gate, WebviewStartupGate};

    #[test]
    fn windows_release_starts_with_the_webview_show_gate_closed() {
        assert!(!initial_webview_safe_to_show(false));
        assert!(initial_webview_safe_to_show(true));
    }

    #[test]
    fn failure_dominates_the_windows_startup_gate_state() {
        assert_eq!(webview_startup_gate(false, false), WebviewStartupGate::Wait);
        assert_eq!(
            webview_startup_gate(true, false),
            WebviewStartupGate::Proceed
        );
        assert_eq!(webview_startup_gate(false, true), WebviewStartupGate::Abort);
        assert_eq!(webview_startup_gate(true, true), WebviewStartupGate::Abort);
    }
}
