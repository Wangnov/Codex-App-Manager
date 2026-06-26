use std::process::Command;

use crate::errors::AppError;

pub const DISABLE_ENV_KEY: &str = "CODEX_SPARKLE_ENABLED";
pub const DISABLE_ENV_VALUE: &str = "false";

#[cfg(target_os = "macos")]
const LAUNCH_AGENT_LABEL: &str = "io.github.wangnov.codex-app-manager.codex-self-update-env";

pub fn apply_to_command(command: &mut Command, disabled: bool) {
    if disabled {
        command.env(DISABLE_ENV_KEY, DISABLE_ENV_VALUE);
    }
}

pub fn sync_setting(disabled: bool) -> Result<(), AppError> {
    sync_current_process(disabled);
    sync_platform(disabled)
}

fn sync_current_process(disabled: bool) {
    if disabled {
        std::env::set_var(DISABLE_ENV_KEY, DISABLE_ENV_VALUE);
    } else {
        std::env::remove_var(DISABLE_ENV_KEY);
    }
}

#[cfg(target_os = "macos")]
fn sync_platform(disabled: bool) -> Result<(), AppError> {
    if disabled {
        launchctl_required(&["setenv", DISABLE_ENV_KEY, DISABLE_ENV_VALUE])?;
        install_macos_launch_agent()?;
    } else {
        launchctl_required(&["unsetenv", DISABLE_ENV_KEY])?;
        remove_macos_launch_agent()?;
    }
    Ok(())
}

#[cfg(windows)]
fn sync_platform(disabled: bool) -> Result<(), AppError> {
    set_windows_user_env(disabled)?;
    broadcast_windows_environment_change();
    Ok(())
}

#[cfg(not(any(target_os = "macos", windows)))]
fn sync_platform(_disabled: bool) -> Result<(), AppError> {
    Ok(())
}

#[cfg(target_os = "macos")]
fn launch_agent_path() -> Result<std::path::PathBuf, AppError> {
    let home = std::env::var_os("HOME")
        .ok_or_else(|| AppError::Internal("无法定位用户主目录".to_string()))?;
    Ok(std::path::PathBuf::from(home)
        .join("Library")
        .join("LaunchAgents")
        .join(format!("{LAUNCH_AGENT_LABEL}.plist")))
}

#[cfg(target_os = "macos")]
fn launchctl_required(args: &[&str]) -> Result<(), AppError> {
    let output = Command::new("/bin/launchctl")
        .args(args)
        .output()
        .map_err(|e| AppError::Engine(format!("launchctl {}: {e}", args.join(" "))))?;
    if output.status.success() {
        return Ok(());
    }
    Err(AppError::Engine(format!(
        "launchctl {} failed: {}",
        args.join(" "),
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

#[cfg(target_os = "macos")]
fn launchctl_best_effort(args: &[String]) {
    if let Ok(output) = Command::new("/bin/launchctl").args(args).output() {
        if !output.status.success() {
            log::debug!(
                "launchctl best-effort command failed args={} stderr={}",
                args.join(" "),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }
}

#[cfg(target_os = "macos")]
fn launchctl_gui_target() -> String {
    format!("gui/{}", unsafe { libc::getuid() })
}

#[cfg(target_os = "macos")]
fn install_macos_launch_agent() -> Result<(), AppError> {
    let path = launch_agent_path()?;
    let parent = path
        .parent()
        .ok_or_else(|| AppError::Internal("无法定位 LaunchAgents 目录".to_string()))?;
    std::fs::create_dir_all(parent)
        .map_err(|e| AppError::Internal(format!("create LaunchAgents dir: {e}")))?;
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key>
  <string>{LAUNCH_AGENT_LABEL}</string>
  <key>ProgramArguments</key>
  <array>
    <string>/bin/launchctl</string>
    <string>setenv</string>
    <string>{DISABLE_ENV_KEY}</string>
    <string>{DISABLE_ENV_VALUE}</string>
  </array>
  <key>RunAtLoad</key>
  <true/>
</dict>
</plist>
"#
    );
    std::fs::write(&path, plist)
        .map_err(|e| AppError::Internal(format!("write LaunchAgent: {e}")))?;

    // The current GUI session is already updated by `launchctl setenv` before
    // this function runs. Leave the LaunchAgent on disk for the next login
    // instead of bootstrapping it immediately: immediate bootstrap/kickstart is
    // noticeably slow and makes macOS show a "launchctl can run in background"
    // notification for a one-shot helper.
    Ok(())
}

#[cfg(target_os = "macos")]
fn remove_macos_launch_agent() -> Result<(), AppError> {
    let path = launch_agent_path()?;
    if path.exists() {
        let target = launchctl_gui_target();
        launchctl_best_effort(&[
            "bootout".to_string(),
            target,
            path.to_string_lossy().to_string(),
        ]);
        std::fs::remove_file(&path)
            .map_err(|e| AppError::Internal(format!("remove LaunchAgent: {e}")))?;
    }
    Ok(())
}

#[cfg(windows)]
fn set_windows_user_env(disabled: bool) -> Result<(), AppError> {
    let script = if disabled {
        format!(
            "[Environment]::SetEnvironmentVariable('{DISABLE_ENV_KEY}','{DISABLE_ENV_VALUE}','User')"
        )
    } else {
        format!("[Environment]::SetEnvironmentVariable('{DISABLE_ENV_KEY}',$null,'User')")
    };
    let output = hidden_command(powershell_exe())
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .map_err(|e| AppError::Engine(format!("spawn powershell: {e}")))?;
    if output.status.success() {
        return Ok(());
    }
    Err(AppError::Engine(format!(
        "set user environment failed: {}",
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}

#[cfg(windows)]
fn broadcast_windows_environment_change() {
    use windows_sys::Win32::Foundation::{LPARAM, WPARAM};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        SendMessageTimeoutW, HWND_BROADCAST, SMTO_ABORTIFHUNG, WM_SETTINGCHANGE,
    };

    let value = "Environment\0".encode_utf16().collect::<Vec<_>>();
    let mut result = 0usize;
    let sent = unsafe {
        SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0 as WPARAM,
            value.as_ptr() as LPARAM,
            SMTO_ABORTIFHUNG,
            5000,
            &mut result,
        )
    };
    if sent == 0 {
        log::warn!("broadcast Windows environment change failed");
    }
}

#[cfg(windows)]
fn powershell_exe() -> std::path::PathBuf {
    std::env::var_os("WINDIR")
        .map(std::path::PathBuf::from)
        .map(|windir| {
            windir
                .join("System32")
                .join("WindowsPowerShell")
                .join("v1.0")
                .join("powershell.exe")
        })
        .filter(|path| path.exists())
        .unwrap_or_else(|| std::path::PathBuf::from("powershell.exe"))
}

#[cfg(windows)]
fn hidden_command(program: impl AsRef<std::ffi::OsStr>) -> Command {
    use std::os::windows::process::CommandExt;

    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}
