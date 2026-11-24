//! Shared by the engine and the dependency-free Windows GUI launcher.
use std::{env, io, path::Path, process::Command};

pub const APP_DIR_NAME: &str = "app";
pub const LAUNCHER_NAME: &str = "LaunchCodex.exe";
pub const LAUNCH_TARGET_NAME: &str = "codex-portable-launcher.txt";
pub const PROTOCOL_ARGUMENT: &str = "--codex-manager-protocol-url";
// Keep this identity stable across launcher versions. The native launcher puts
// it in its own PE section so an engine/test binary containing it is distinct.
pub const LAUNCHER_MARKER: [u8; 33] = *b"CAM_PORTABLE_LAUNCHER_V1_a91c7d3b";

/// Shell URI substitution can split a malicious URL into additional arguments.
/// Only the registered protocol entry uses this restricted mode; ordinary
/// launcher arguments keep their existing forwarding behavior.
#[allow(dead_code)] // Used by the separately compiled native launcher and unit tests.
pub fn launch_arguments(args: Vec<std::ffi::OsString>) -> io::Result<Vec<std::ffi::OsString>> {
    if args.first().is_none_or(|arg| arg != PROTOCOL_ARGUMENT) {
        return Ok(args);
    }
    let valid = args.len() == 2 && args[1].to_str().is_some_and(|url| {
        url.get(..6).is_some_and(|scheme| scheme.eq_ignore_ascii_case("codex:"))
            && url.len() > 6
            && !url.chars().any(|ch| ch.is_control() || ch.is_whitespace() || matches!(ch, '"' | '\\'))
    });
    if !valid {
        return Err(io::Error::other("Invalid Codex protocol URL."));
    }
    Ok(args.into_iter().skip(1).collect())
}

/// Both legacy root targets and the new app/<exe> layout are supported. Reject
/// traversal, absolute paths, and root launcher aliases that would recurse.
pub fn valid_launch_target(target: &str) -> bool {
    use std::path::Component;
    let normalized = target.replace('\\', "/");
    let parts: Vec<_> = Path::new(&normalized).components().collect();
    let normal = parts
        .iter()
        .all(|part| matches!(part, Component::Normal(_)));
    normal
        && !target.contains(':')
        && target.to_ascii_lowercase().ends_with(".exe")
        && match parts.as_slice() {
            [Component::Normal(name)] => {
                !name.to_string_lossy().eq_ignore_ascii_case(LAUNCHER_NAME)
            }
            [Component::Normal(dir), Component::Normal(_)] => *dir == APP_DIR_NAME,
            _ => false,
        }
}

/// One-line configs from older Managers required a CLI. New configs explicitly
/// carry the upstream metadata requirement so CLI-less legacy apps still run.
pub fn parse_launch_config(config: &str) -> Option<(&str, bool)> {
    let mut lines = config.lines();
    let target = lines.next()?.trim();
    if !valid_launch_target(target) {
        return None;
    }
    let required = match lines.next().map(str::trim) {
        None | Some("require-cli=1") => true,
        Some("require-cli=0") => false,
        _ => return None,
    };
    if lines.any(|line| !line.trim().is_empty()) {
        return None;
    }
    Some((target, required))
}

/// Prefer the user's home directory as Codex's working directory so dialogs and
/// new projects never default inside the managed portable install directory.
/// Falls back to `fallback` (the app directory) only if no valid home directory
/// is available in the environment.
#[allow(dead_code)] // Used by Windows shortcut creation, configure, and unit tests.
pub fn preferred_working_dir(fallback: &Path) -> std::path::PathBuf {
    resolve_working_dir(env::var_os("USERPROFILE"), env::var_os("HOME"), fallback)
}

fn resolve_working_dir(
    user_profile: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
    fallback: &Path,
) -> std::path::PathBuf {
    for candidate in [user_profile, home].into_iter().flatten() {
        if candidate.to_string_lossy().trim().is_empty() {
            continue;
        }
        let path = std::path::PathBuf::from(candidate);
        if path.is_dir() {
            return path;
        }
    }
    fallback.to_path_buf()
}

/// Use the payload's CLI without changing the user's or the Manager's environment.
/// A nonempty explicit user override retains its normal upstream semantics.
pub fn configure(command: &mut Command, app_exe: &Path, required: bool) -> io::Result<()> {
    configure_with_env(
        command,
        app_exe,
        required,
        env::var_os("CODEX_CLI_PATH"),
        env::var_os("USERPROFILE"),
        env::var_os("HOME"),
    )
}

fn configure_with_env(
    command: &mut Command,
    app_exe: &Path,
    required: bool,
    cli_override: Option<std::ffi::OsString>,
    user_profile: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> io::Result<()> {
    let root = app_exe
        .parent()
        .ok_or_else(|| io::Error::other("missing app directory"))?;
    let working_dir = resolve_working_dir(user_profile, home, root);
    command.current_dir(working_dir);
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
    fn protocol_entry_rejects_argument_injection() {
        let args = |url: &str| vec![PROTOCOL_ARGUMENT.into(), url.into()];
        for url in ["codex://settings", "CODEX://threads/123?x=%22quoted%22", "codex://test/参数"] {
            assert_eq!(launch_arguments(args(url)).unwrap(), vec![std::ffi::OsString::from(url)]);
        }
        for url in ["", "codex:", "https://example.com", "codex://test/\" --inspect=1", "codex://test/space here", "codex://test/\\", "codex://test/\n"] {
            assert!(launch_arguments(args(url)).is_err(), "{url:?}");
        }
        assert!(launch_arguments(vec![PROTOCOL_ARGUMENT.into()]).is_err());
        assert!(launch_arguments(vec![PROTOCOL_ARGUMENT.into(), "codex://test".into(), "--inspect=1".into()]).is_err());
        let direct = vec!["--test-flag".into(), "codex://test/with spaces".into()];
        assert_eq!(launch_arguments(direct.clone()).unwrap(), direct);
    }
    #[test]
    fn launcher_targets_stay_in_the_payload_or_legacy_root() {
        for target in [
            "ChatGPT.exe",
            "Codex.exe",
            "app/ChatGPT.exe",
            r"app\Codex.exe",
        ] {
            assert!(valid_launch_target(target), "{target}");
        }
        for target in [
            "",
            "LaunchCodex.exe",
            "launchcodex.EXE",
            "app",
            "app/file.txt",
            "../ChatGPT.exe",
            "app/../Codex.exe",
            "other/ChatGPT.exe",
            "app/sub/ChatGPT.exe",
            r"C:\app\ChatGPT.exe",
            r"\app\ChatGPT.exe",
            r"\\server\app\ChatGPT.exe",
        ] {
            assert!(!valid_launch_target(target), "{target}");
        }
    }

    #[test]
    fn launch_config_preserves_legacy_defaults_and_explicit_cli_requirements() {
        assert_eq!(
            parse_launch_config("ChatGPT.exe"),
            Some(("ChatGPT.exe", true))
        );
        assert_eq!(
            parse_launch_config("app/ChatGPT.exe\nrequire-cli=1"),
            Some(("app/ChatGPT.exe", true))
        );
        assert_eq!(
            parse_launch_config("app/Codex.exe\r\nrequire-cli=0\r\n"),
            Some(("app/Codex.exe", false))
        );
        for config in [
            "",
            "../app.exe\nrequire-cli=0",
            "app/Codex.exe\nrequire-cli=2",
            "app/Codex.exe\nrequire-cli=0\nunknown",
        ] {
            assert_eq!(parse_launch_config(config), None, "{config}");
        }
    }
    #[test]
    fn bundled_cli_configuration_handles_missing_legacy_and_explicit_overrides() {
        let base = env::temp_dir().join(format!("portable-command-{}", uuid::Uuid::new_v4()));
        let root = base.join("Codex").join("app");
        let home = base.join("UserHome");
        std::fs::create_dir_all(root.join("resources/bin")).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        let exe = root.join("ChatGPT.exe");
        assert!(configure_with_env(
            &mut Command::new(&exe),
            &exe,
            true,
            None,
            Some(home.as_os_str().to_os_string()),
            None,
        )
        .is_err());
        assert!(configure_with_env(
            &mut Command::new(&exe),
            &exe,
            false,
            None,
            Some(home.as_os_str().to_os_string()),
            None,
        )
        .is_ok());
        let old_cli = root.join("resources/bin/codex.exe");
        std::fs::write(&old_cli, b"old").unwrap();
        let mut command = Command::new(&exe);
        configure_with_env(
            &mut command,
            &exe,
            true,
            Some("  ".into()),
            Some(home.as_os_str().to_os_string()),
            None,
        )
        .unwrap();
        assert!(command
            .get_envs()
            .any(|(k, v)| k == "CODEX_CLI_PATH" && v == Some(old_cli.as_os_str())));
        let cli = root.join("resources/codex.exe");
        std::fs::write(&cli, b"new").unwrap();
        let mut command = Command::new(&exe);
        configure_with_env(
            &mut command,
            &exe,
            true,
            None,
            Some(home.as_os_str().to_os_string()),
            None,
        )
        .unwrap();
        assert!(command
            .get_envs()
            .any(|(k, v)| k == "CODEX_CLI_PATH" && v == Some(cli.as_os_str())));
        assert!(command
            .get_envs()
            .any(|(k, v)| k == "CODEX_WINDOWS_REGISTERED_CORE" && v.is_none()));
        assert_eq!(command.get_current_dir(), Some(home.as_path()));
        let mut explicit = Command::new(&exe);
        configure_with_env(
            &mut explicit,
            &exe,
            true,
            Some("custom-cli".into()),
            Some(home.as_os_str().to_os_string()),
            None,
        )
        .unwrap();
        assert!(!explicit.get_envs().any(|(k, _)| k == "CODEX_CLI_PATH"));
        assert_eq!(explicit.get_current_dir(), Some(home.as_path()));
        std::fs::remove_dir_all(base).unwrap();
    }

    #[test]
    fn working_dir_prefers_userprofile_then_home_and_falls_back_to_app_dir() {
        let base = env::temp_dir().join(format!("portable-cwd-{}", uuid::Uuid::new_v4()));
        let root = base.join("Codex").join("app");
        let user_profile = base.join("UserProfile");
        let home = base.join("HomeFallback");
        let not_a_dir = base.join("not-a-dir.txt");
        let missing = base.join("does-not-exist");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(&user_profile).unwrap();
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(&not_a_dir, b"file").unwrap();
        let exe = root.join("ChatGPT.exe");

        // Valid USERPROFILE wins over HOME and the app directory.
        let mut cmd = Command::new(&exe);
        configure_with_env(
            &mut cmd,
            &exe,
            false,
            None,
            Some(user_profile.as_os_str().to_os_string()),
            Some(home.as_os_str().to_os_string()),
        )
        .unwrap();
        assert_eq!(cmd.get_current_dir(), Some(user_profile.as_path()));

        // Missing, blank, nonexistent, or file USERPROFILE falls back to valid HOME.
        for bad_profile in [
            None,
            Some(std::ffi::OsString::from("")),
            Some(std::ffi::OsString::from("   ")),
            Some(missing.as_os_str().to_os_string()),
            Some(not_a_dir.as_os_str().to_os_string()),
        ] {
            let mut cmd = Command::new(&exe);
            configure_with_env(
                &mut cmd,
                &exe,
                false,
                None,
                bad_profile,
                Some(home.as_os_str().to_os_string()),
            )
            .unwrap();
            assert_eq!(cmd.get_current_dir(), Some(home.as_path()));
        }

        // If neither USERPROFILE nor HOME resolves to an existing directory, fall back to root.
        let mut fallback_cmd = Command::new(&exe);
        configure_with_env(
            &mut fallback_cmd,
            &exe,
            false,
            None,
            Some(missing.as_os_str().to_os_string()),
            Some(not_a_dir.as_os_str().to_os_string()),
        )
        .unwrap();
        assert_eq!(fallback_cmd.get_current_dir(), Some(root.as_path()));

        std::fs::remove_dir_all(base).unwrap();
    }
}
