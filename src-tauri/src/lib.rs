pub mod adapters;
pub mod app;
pub mod commands;
pub mod domain;
pub mod errors;
pub mod ports;

mod state;

use std::sync::atomic::Ordering;

use tauri::{Emitter, Manager, RunEvent, WindowEvent};

/// The "ask before closing" setting, read fresh from disk so a toggle in
/// Settings takes effect immediately (no restart).
fn confirm_close_enabled() -> bool {
    crate::app::settings_store::AppSettings::load().confirm_close
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_dialog::init())
        // Launch-at-login support. Off by default — the user opts in from
        // Settings; we only register the plugin so the toggle can flip it.
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .manage(state::ManagerState::new())
        .invoke_handler(tauri::generate_handler![
            commands::check_payload_updates,
            commands::get_app_snapshot,
            commands::plan_install,
            commands::plan_uninstall,
            commands::run_health_check,
            commands::mac_plan_update,
            commands::mac_stage_update,
            commands::mac_perform_update,
            commands::mac_status,
            commands::mac_adopt,
            commands::mac_install,
            commands::mac_launch_codex,
            commands::mac_uninstall,
            commands::get_settings,
            commands::set_settings,
            commands::confirm_quit,
            commands::win_default_install_root,
            commands::win_pick_install_dir,
            commands::win_set_install_root,
            commands::win_reset_install_root,
            commands::get_autostart,
            commands::set_autostart,
            commands::open_url,
            commands::win_plan_update,
            commands::win_stage_update,
            commands::win_auto_stage_update,
            commands::win_cancel_download,
            commands::win_status,
            commands::win_adopt,
            commands::win_launch_codex,
            commands::win_perform_update,
            commands::win_uninstall,
        ])
        // A normal "open it when you need it" app — NOT a menu-bar resident.
        // Closing the window quits the process so nothing lingers in the
        // background; the Dock icon is the only entry point, and login launch is
        // an explicit, off-by-default opt-in (see Settings).
        //
        // The window has no system chrome, so every window-close path — the
        // in-app ✕, Alt+F4, the macOS window close — arrives here. Unless the
        // user already confirmed (or turned the guard off) we hold the close and
        // ask the UI to raise the confirm dialog instead of quitting.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                let confirmed = app
                    .state::<state::ManagerState>()
                    .force_quit
                    .load(Ordering::SeqCst);
                if confirmed || !confirm_close_enabled() {
                    app.exit(0);
                } else {
                    api.prevent_close();
                    let _ = window.emit("app://confirm-quit", ());
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to build Codex App Manager")
        // Cmd+Q (and any other app-level quit) lands as ExitRequested rather than
        // a window CloseRequested — gate it the same way so the close-confirm
        // setting is honored there too.
        .run(|app, event| {
            if let RunEvent::ExitRequested { api, .. } = event {
                let confirmed = app
                    .state::<state::ManagerState>()
                    .force_quit
                    .load(Ordering::SeqCst);
                if !confirmed && confirm_close_enabled() {
                    api.prevent_exit();
                    let _ = app.emit("app://confirm-quit", ());
                }
            }
        });
}
