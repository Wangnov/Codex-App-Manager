pub mod adapters;
pub mod app;
pub mod commands;
pub mod domain;
pub mod errors;

mod state;

use std::sync::atomic::Ordering;

use tauri::{Emitter, Manager, RunEvent, WindowEvent};

use crate::app::op_phase::QuitPolicy;

/// The "ask before closing" setting, read fresh from disk so a toggle in
/// Settings takes effect immediately (no restart).
fn confirm_close_enabled() -> bool {
    crate::app::settings_store::AppSettings::load().confirm_close
}

/// Unified quit/close policy: phase-aware + confirm_close setting.
fn quit_policy_for(app: &tauri::AppHandle) -> QuitPolicy {
    let state = app.state::<state::ManagerState>();
    let force = state.force_quit.load(Ordering::SeqCst);
    state
        .operations
        .quit_policy(force, confirm_close_enabled())
}

/// Apply a quit policy decision for window/menu/exit paths.
/// Returns `true` when the caller should proceed to exit.
fn apply_quit_policy(app: &tauri::AppHandle, policy: &QuitPolicy) -> bool {
    match policy {
        QuitPolicy::Allow => true,
        QuitPolicy::Confirm => {
            let _ = app.emit("app://confirm-quit", ());
            false
        }
        QuitPolicy::Block {
            phase,
            reason_code,
            reason,
            kind,
        } => {
            log::warn!(
                "quit blocked phase={} reason_code={reason_code} kind={:?} reason={reason}",
                phase.as_str(),
                kind
            );
            let _ = app.emit("app://quit-blocked", policy);
            false
        }
    }
}

/// macOS routes Cmd+Q through the app-menu Quit item, which terminates *below*
/// the RunEvent loop (so ExitRequested can't hold it). Replace the default menu
/// with one whose Quit item is ours — its activation lands in `on_menu_event`
/// where we can confirm first. The Edit submenu is preserved so the standard
/// copy/paste/select-all shortcuts keep working in text fields.
#[cfg(target_os = "macos")]
fn install_macos_menu(app: &tauri::App) -> tauri::Result<()> {
    use tauri::menu::{AboutMetadata, MenuBuilder, MenuItemBuilder, SubmenuBuilder};

    let quit = MenuItemBuilder::with_id("cam-quit", "Quit Codex App 管理器")
        .accelerator("Cmd+Q")
        .build(app)?;
    let app_menu = SubmenuBuilder::new(app, "Codex App 管理器")
        .about(Some(AboutMetadata::default()))
        .separator()
        .services()
        .separator()
        .hide()
        .hide_others()
        .show_all()
        .separator()
        .item(&quit)
        .build()?;
    let edit_menu = SubmenuBuilder::new(app, "Edit")
        .undo()
        .redo()
        .separator()
        .cut()
        .copy()
        .paste()
        .select_all()
        .build()?;
    let window_menu = SubmenuBuilder::new(app, "Window")
        .minimize()
        .close_window()
        .build()?;
    let menu = MenuBuilder::new(app)
        .items(&[&app_menu, &edit_menu, &window_menu])
        .build()?;
    app.set_menu(menu)?;
    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.unminimize();
                let _ = window.show();
                let _ = window.set_focus();
            }
        }))
        .plugin(
            tauri_plugin_log::Builder::new()
                .targets([
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::Stdout),
                    tauri_plugin_log::Target::new(tauri_plugin_log::TargetKind::LogDir {
                        file_name: Some("codex-app-manager".to_string()),
                    }),
                ])
                .level(if cfg!(debug_assertions) {
                    log::LevelFilter::Debug
                } else {
                    log::LevelFilter::Info
                })
                .level_for("tao", log::LevelFilter::Warn)
                .level_for("wry", log::LevelFilter::Warn)
                .max_file_size(crate::app::logging::MAX_LOG_FILE_BYTES)
                .rotation_strategy(tauri_plugin_log::RotationStrategy::KeepAll)
                .timezone_strategy(tauri_plugin_log::TimezoneStrategy::UseLocal)
                .format(|out, message, record| {
                    out.finish(format_args!(
                        "[{}] [{}] [{}:{}] {}",
                        record.level(),
                        record.target(),
                        record.file().unwrap_or("?"),
                        record.line().unwrap_or(0),
                        message
                    ))
                })
                .build(),
        )
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
            commands::mac_plan_update,
            commands::mac_stage_update,
            commands::mac_perform_update,
            commands::mac_status,
            commands::mac_adopt,
            commands::mac_pick_existing_install,
            commands::mac_adopt_path,
            commands::mac_install,
            commands::mac_pause_download,
            commands::mac_cancel_download,
            commands::mac_discard_download,
            commands::mac_launch_codex,
            commands::mac_uninstall,
            commands::manager_check_update,
            commands::manager_install_update,
            commands::get_settings,
            commands::set_settings,
            commands::get_config_health,
            commands::restore_config_backup,
            commands::reset_config,
            commands::retry_ancillary,
            commands::begin_operation,
            commands::arm_destructive,
            commands::end_operation,
            commands::get_operation_snapshot,
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
            commands::win_pause_download,
            commands::win_cancel_download,
            commands::win_discard_download,
            commands::win_status,
            commands::win_adopt,
            commands::win_pick_existing_install,
            commands::win_adopt_path,
            commands::win_launch_codex,
            commands::win_perform_update,
            commands::win_uninstall,
            commands::get_diagnostics,
            commands::open_logs_dir,
            commands::open_codex_home,
            commands::log_frontend_error,
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            install_macos_menu(app)?;
            log::info!(
                "Codex App Manager v{} starting (os={}, arch={})",
                app.package_info().version,
                std::env::consts::OS,
                std::env::consts::ARCH
            );
            if let Some(logs_dir) = crate::app::logging::logs_dir(app.handle()) {
                tauri::async_runtime::spawn_blocking(move || {
                    crate::app::logging::prune_old_logs(
                        &logs_dir,
                        crate::app::logging::KEEP_LOG_FILES,
                    );
                });
            }
            let operations = app.state::<state::ManagerState>().operations.clone();
            tauri::async_runtime::spawn_blocking(move || {
                // Crash-safe install recovery MUST run before ordinary staging
                // cleanup so recovery materials (backup / staged new) are not
                // deleted out from under an incomplete swap.
                let recovery =
                    crate::app::install_tx::recover_pending_transactions(Some(&operations));
                if recovery.failed > 0 || recovery.kept_manual > 0 {
                    log::warn!(
                        "install transaction recovery finished scanned={} continued={} rolled_back={} completed={} cleared={} kept_manual={} failed={}",
                        recovery.scanned,
                        recovery.continued,
                        recovery.rolled_back,
                        recovery.completed,
                        recovery.cleared,
                        recovery.kept_manual,
                        recovery.failed
                    );
                }
                let summary = crate::app::staging::cleanup_stale_staging(&operations);
                if summary.failed > 0 {
                    log::warn!(
                        "staging cleanup completed with failures scanned={} removed={} failed={}",
                        summary.scanned,
                        summary.removed,
                        summary.failed
                    );
                }
            });
            let health = app
                .state::<state::ManagerState>()
                .config_health
                .lock()
                .unwrap_or_else(|poison| poison.into_inner())
                .clone();
            if !health.is_ok() {
                log::warn!(
                    "config health not ok: settings={} provenance={} unknown_source={:?} detail={:?}",
                    health.settings_status,
                    health.provenance_status,
                    health.unknown_source,
                    health.detail
                );
                let _ = app.emit("app://config-health", health);
            }
            Ok(())
        })
        // Our custom macOS Quit item lands here (Cmd+Q). Same phase-aware policy
        // as window close / ExitRequested.
        .on_menu_event(|app, event| {
            if event.id().0.as_str() == "cam-quit" {
                let policy = quit_policy_for(app);
                if apply_quit_policy(app, &policy) {
                    app.exit(0);
                }
            }
        })
        // A normal "open it when you need it" app — NOT a menu-bar resident.
        // Closing the window quits the process so nothing lingers in the
        // background; the Dock icon is the only entry point, and login launch is
        // an explicit, off-by-default opt-in (see Settings).
        //
        // The window has no system chrome, so every window-close path — the
        // in-app ✕, Alt+F4, the macOS window close — arrives here. Policy is
        // phase-aware: point-of-no-return install steps block quit; otherwise
        // the confirm_close setting may raise a dialog.
        .on_window_event(|window, event| {
            if let WindowEvent::CloseRequested { api, .. } = event {
                let app = window.app_handle();
                let policy = quit_policy_for(app);
                if apply_quit_policy(app, &policy) {
                    app.exit(0);
                } else {
                    api.prevent_close();
                }
            }
        })
        .build(tauri::generate_context!())
        .expect("failed to build Codex App Manager")
        // Cmd+Q (and any other app-level quit) lands as ExitRequested rather than
        // a window CloseRequested — gate it with the same phase-aware policy.
        .run(|app, event| {
            if let RunEvent::ExitRequested { api, .. } = event {
                let policy = quit_policy_for(app);
                if !apply_quit_policy(app, &policy) {
                    api.prevent_exit();
                }
            }
        });
}
