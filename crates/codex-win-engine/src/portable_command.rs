//! Shared by the engine and the dependency-free Windows GUI launcher.
use std::{env, io, path::Path, process::Command};

#[cfg(windows)]
pub const LAUNCHER_NAME: &str = "LaunchCodex.exe";
#[cfg(windows)]
pub const LAUNCH_TARGET_NAME: &str = "codex-portable-launcher.txt";

/// Use the payload's CLI without changing the user's or the Manager's environment.
/// A nonempty explicit user override retains its normal upstream semantics.
pub fn configure(command: &mut Command, app_exe: &Path, required: bool) -> io::Result<()> {
    configure_with_override(command, app_exe, required, env::var_os("CODEX_CLI_PATH"))
}

fn configure_with_override(
    command: &mut Command,
    app_exe: &Path,
    required: bool,
    cli_override: Option<std::ffi::OsString>,
) -> io::Result<()> {
    let root = app_exe
        .parent()
        .ok_or_else(|| io::Error::other("missing app directory"))?;
    command.current_dir(root);
    command.env_remove("CODEX_WINDOWS_REGISTERED_CORE");
    let overridden = cli_override.is_some_and(|value| !value.to_string_lossy().trim().is_empty());
    if !overridden {
        let cli = [
            root.join("resources/codex.exe"),
            root.join("resources/bin/codex.exe"),
        ]
        .into_iter()
        .find(|path| path.is_file());
        if let Some(cli) = cli {
            command.env("CODEX_CLI_PATH", cli);
        } else if required {
            return Err(io::Error::new(io::ErrorKind::NotFound,
                "Portable Codex is missing its bundled CLI (resources/codex.exe). Reinstall it with Codex App Manager."));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bundled_cli_configuration_handles_missing_legacy_and_explicit_overrides() {
        let root = env::temp_dir().join(format!("portable-command-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("resources/bin")).unwrap();
        let exe = root.join("ChatGPT.exe");
        assert!(configure_with_override(&mut Command::new(&exe), &exe, true, None).is_err());
        assert!(configure_with_override(&mut Command::new(&exe), &exe, false, None).is_ok());
        let old_cli = root.join("resources/bin/codex.exe");
        std::fs::write(&old_cli, b"old").unwrap();
        let mut command = Command::new(&exe);
        configure_with_override(&mut command, &exe, true, Some("  ".into())).unwrap();
        assert!(command
            .get_envs()
            .any(|(k, v)| k == "CODEX_CLI_PATH" && v == Some(old_cli.as_os_str())));
        let cli = root.join("resources/codex.exe");
        std::fs::write(&cli, b"new").unwrap();
        let mut command = Command::new(&exe);
        configure_with_override(&mut command, &exe, true, None).unwrap();
        assert!(command
            .get_envs()
            .any(|(k, v)| k == "CODEX_CLI_PATH" && v == Some(cli.as_os_str())));
        assert!(command
            .get_envs()
            .any(|(k, v)| k == "CODEX_WINDOWS_REGISTERED_CORE" && v.is_none()));
        assert_eq!(command.get_current_dir(), Some(root.as_path()));
        let mut explicit = Command::new(&exe);
        configure_with_override(&mut explicit, &exe, true, Some("custom-cli".into())).unwrap();
        assert!(!explicit.get_envs().any(|(k, _)| k == "CODEX_CLI_PATH"));
        std::fs::remove_dir_all(root).unwrap();
    }
}
