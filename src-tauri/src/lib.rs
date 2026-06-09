pub mod adapters;
pub mod app;
pub mod commands;
pub mod domain;
pub mod errors;

mod state;

use std::sync::atomic::Ordering;

use tauri::{Emitter, Manager, RunEvent, WindowEvent};

/// The "ask before closing" setting, read fresh from disk so a toggle in
/// Settings takes effect immediately (no restart).
fn confirm_close_enabled() -> bool {
    crate::app::settings_store::AppSettings::load().confirm_close
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
        .setup(|app| {
            #[cfg(target_os = "macos")]
            install_macos_menu(app)?;
            let _ = &app;
            Ok(())
        })
        // Our custom macOS Quit item lands here (Cmd+Q). Same guard as the
        // window close: confirm first unless already confirmed / guard off.
        .on_menu_event(|app, event| {
            if event.id().0.as_str() == "cam-quit" {
                let confirmed = app
                    .state::<state::ManagerState>()
                    .force_quit
                    .load(Ordering::SeqCst);
                if confirmed || !confirm_close_enabled() {
                    app.exit(0);
                } else {
                    let _ = app.emit("app://confirm-quit", ());
                }
            }
        })
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
