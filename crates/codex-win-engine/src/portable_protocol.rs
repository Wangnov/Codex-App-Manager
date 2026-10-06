//! Register a portable URI handler as a selectable default; never change UserChoice.
use std::path::Path;
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{ERROR_FILE_NOT_FOUND, ERROR_PATH_NOT_FOUND},
        System::Registry::{
            RegCloseKey, RegCreateKeyExW, RegDeleteTreeW, RegDeleteValueW, RegGetValueW,
            RegOpenKeyExW, RegSetValueExW, HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE,
            REG_CREATED_NEW_KEY, REG_CREATE_KEY_DISPOSITION, REG_OPTION_NON_VOLATILE,
            REG_SAM_FLAGS, REG_SZ, RRF_RT_REG_SZ,
        },
        UI::Shell::{SHChangeNotify, SHCNE_ASSOCCHANGED, SHCNF_IDLIST},
    },
};

const PROG_ID: &str = "CodexAppManager.Portable";
#[cfg(not(test))]
const CLASS: &str = r"Software\Classes\CodexAppManager.Portable";
#[cfg(not(test))]
const CAP_ROOT: &str = r"Software\CodexAppManager\Portable";
#[cfg(not(test))]
const REGISTERED: &str = r"Software\RegisteredApplications";
const OWNER: PCWSTR = w!("CodexAppManagerProtocol");
const COMMAND_KEY: &str = r"shell\open\command";

struct Key(HKEY);
impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: this key was opened by this module and is closed exactly once.
        unsafe {
            let _ = RegCloseKey(self.0);
        }
    }
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn open(path: &str, access: REG_SAM_FLAGS) -> windows::core::Result<Option<Key>> {
    let name = wide(path);
    let mut key = HKEY::default();
    // SAFETY: name is terminated and the output lives through the call.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(name.as_ptr()),
            Some(0),
            access,
            &mut key,
        )
    };
    if status == ERROR_FILE_NOT_FOUND || status == ERROR_PATH_NOT_FOUND {
        return Ok(None);
    }
    status.ok()?;
    Ok(Some(Key(key)))
}

fn create(path: &str) -> windows::core::Result<Key> {
    create_key(path).map(|(key, _)| key)
}

fn create_key(path: &str) -> windows::core::Result<(Key, bool)> {
    let name = wide(path);
    let mut key = HKEY::default();
    let mut disposition = REG_CREATE_KEY_DISPOSITION::default();
    // SAFETY: valid terminated input, no security descriptor, valid output.
    unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            PCWSTR(name.as_ptr()),
            Some(0),
            None,
            REG_OPTION_NON_VOLATILE,
            KEY_READ | KEY_WRITE,
            None,
            &mut key,
            Some(&mut disposition),
        )
        .ok()?;
    }
    Ok((Key(key), disposition == REG_CREATED_NEW_KEY))
}

fn read(key: &Key, subkey: Option<&str>, name: PCWSTR) -> windows::core::Result<Option<String>> {
    let subkey = subkey.map(wide);
    let subkey = subkey
        .as_ref()
        .map_or(PCWSTR::null(), |s| PCWSTR(s.as_ptr()));
    let mut size = 0;
    // SAFETY: query the size first, then allocate enough space for the value.
    let status = unsafe {
        RegGetValueW(
            key.0,
            subkey,
            name,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    if status == ERROR_FILE_NOT_FOUND || status == ERROR_PATH_NOT_FOUND {
        return Ok(None);
    }
    status.ok()?;
    let mut data = vec![0u16; (size as usize).div_ceil(2)];
    unsafe {
        RegGetValueW(
            key.0,
            subkey,
            name,
            RRF_RT_REG_SZ,
            None,
            Some(data.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .ok()?;
    }
    let end = data.iter().position(|&c| c == 0).unwrap_or(data.len());
    Ok(Some(String::from_utf16_lossy(&data[..end])))
}

fn write(key: &Key, name: PCWSTR, value: &str) -> windows::core::Result<()> {
    let value = wide(value);
    // SAFETY: view the initialized UTF-16 buffer as bytes for this synchronous call.
    let bytes = unsafe { std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), value.len() * 2) };
    unsafe { RegSetValueExW(key.0, name, Some(0), REG_SZ, Some(bytes)).ok() }
}

fn command(launcher: &Path) -> String {
    format!(
        "\"{}\" {} \"%1\"",
        launcher.display(),
        crate::portable_command::PROTOCOL_ARGUMENT
    )
}

fn owned(key: &Key) -> windows::core::Result<bool> {
    Ok(read(key, None, OWNER)?.as_deref() == Some("1")
        && read(key, Some(COMMAND_KEY), PCWSTR::null())?.is_none_or(|value| {
            value.ends_with(&format!(
                " {} \"%1\"",
                crate::portable_command::PROTOCOL_ARGUMENT
            ))
        }))
}

fn delete_tree(path: &str) -> windows::core::Result<()> {
    let path = wide(path);
    // SAFETY: the caller supplies an exact owned registry subtree.
    let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr())) };
    if status == ERROR_FILE_NOT_FOUND || status == ERROR_PATH_NOT_FOUND {
        return Ok(());
    }
    status.ok()
}

fn notify() {
    // SAFETY: association-change notifications have no item pointers.
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) }
}

pub(crate) fn register(launcher: &Path, icon: &Path) -> windows::core::Result<bool> {
    #[cfg(test)]
    {
        let _ = (launcher, icon);
        Ok(false)
    } // Installation fixtures never alter real shell metadata.
    #[cfg(not(test))]
    register_at(CLASS, CAP_ROOT, REGISTERED, launcher, icon)
}

fn register_at(
    class_path: &str,
    cap_root: &str,
    registered_path: &str,
    launcher: &Path,
    icon: &Path,
) -> windows::core::Result<bool> {
    let caps_path = format!(r"{cap_root}\Capabilities");
    // Check every ownership boundary before changing any registration.
    if let Some(key) = open(class_path, KEY_READ)? {
        if !owned(&key)? {
            return Ok(false);
        }
    }
    if let Some(cap) = open(cap_root, KEY_READ)? {
        if read(&cap, None, OWNER)?.as_deref() != Some("1") {
            return Ok(false);
        }
    }
    if let Some(registered) = open(registered_path, KEY_READ)? {
        if read(&registered, None, w!("CodexAppManager.Portable"))?
            .is_some_and(|value| value != caps_path)
        {
            return Ok(false);
        }
    }
    let (key, fresh) = create_key(class_path)?;
    if !fresh && !owned(&key)? {
        return Ok(false);
    }
    let mut cap_created = false;
    let result: windows::core::Result<bool> = (|| {
        write(&key, OWNER, "1")?;
        write(&key, PCWSTR::null(), "Codex (Portable)")?;
        write(&key, w!("URL Protocol"), "")?;
        let icon_key = create(&format!(r"{class_path}\DefaultIcon"))?;
        let icon = format!("\"{}\",0", icon.display());
        write(&icon_key, PCWSTR::null(), &icon)?;
        let application = create(&format!(r"{class_path}\Application"))?;
        write(&application, w!("ApplicationName"), "Codex (Portable)")?;
        write(&application, w!("ApplicationIcon"), &icon)?;
        write(
            &application,
            w!("AppUserModelID"),
            crate::portable_shortcut::APP_ID,
        )?;
        let cmd = create(&format!(r"{class_path}\{COMMAND_KEY}"))?;
        // Commit command last, keeping a previous launch command on write failure.
        write(&cmd, PCWSTR::null(), &command(launcher))?;
        let (cap, fresh) = create_key(cap_root)?;
        cap_created = fresh;
        if !fresh && read(&cap, None, OWNER)?.as_deref() != Some("1") {
            return Ok(false);
        }
        write(&cap, OWNER, "1")?;
        let caps = create(&caps_path)?;
        write(&caps, w!("ApplicationName"), "Codex (Portable)")?;
        write(
            &caps,
            w!("ApplicationDescription"),
            "Open Codex links with portable Codex.",
        )?;
        write(&caps, w!("ApplicationIcon"), &icon)?;
        let associations = create(&format!(r"{caps_path}\UrlAssociations"))?;
        write(&associations, w!("codex"), PROG_ID)?;
        let registered = create(registered_path)?;
        if read(&registered, None, w!("CodexAppManager.Portable"))?
            .is_some_and(|value| value != caps_path)
        {
            return Ok(false);
        }
        write(&registered, w!("CodexAppManager.Portable"), &caps_path)?;
        Ok(true)
    })();
    drop(key);
    if !matches!(result, Ok(true)) {
        if fresh {
            let _ = delete_tree(class_path);
        }
        if cap_created {
            let _ = delete_tree(cap_root);
        }
    }
    let registered = result?;
    notify();
    Ok(registered)
}

pub(crate) fn unregister() -> windows::core::Result<bool> {
    #[cfg(test)]
    {
        Ok(false)
    }
    #[cfg(not(test))]
    unregister_at(CLASS, CAP_ROOT, REGISTERED)
}

fn unregister_at(
    class_path: &str,
    cap_root: &str,
    registered_path: &str,
) -> windows::core::Result<bool> {
    let mut removed = false;
    if let Some(key) = open(class_path, KEY_READ)? {
        if !owned(&key)? {
            return Ok(false);
        }
        drop(key);
        delete_tree(class_path)?;
        removed = true;
    }
    if let Some(cap) = open(cap_root, KEY_READ)? {
        if read(&cap, None, OWNER)?.as_deref() == Some("1")
            && read(&cap, Some(r"Capabilities\UrlAssociations"), w!("codex"))?.as_deref()
                == Some(PROG_ID)
        {
            if let Some(registered) = open(registered_path, KEY_READ | KEY_WRITE)? {
                if read(&registered, None, w!("CodexAppManager.Portable"))?.as_deref()
                    == Some(&format!(r"{cap_root}\Capabilities"))
                {
                    // SAFETY: remove only our named value, preserving all other apps.
                    unsafe {
                        RegDeleteValueW(registered.0, w!("CodexAppManager.Portable")).ok()?;
                    }
                }
            }
            drop(cap);
            delete_tree(cap_root)?;
            removed = true;
        }
    }
    notify();
    Ok(removed)
}

#[cfg(not(test))]
pub(crate) fn uninstall_script(launcher: &Path) -> String {
    uninstall_script_at(CLASS, CAP_ROOT, REGISTERED, launcher)
}

fn uninstall_script_at(
    class_path: &str,
    cap_root: &str,
    registered_path: &str,
    launcher: &Path,
) -> String {
    let quote = |value: &str| value.replace('\'', "''");
    let class = quote(class_path);
    let cap = quote(cap_root);
    let apps = quote(registered_path);
    format!("$Protocol = Get-Item -LiteralPath 'HKCU:\\{class}' -ErrorAction SilentlyContinue; $ProtocolCommand = Get-Item -LiteralPath 'HKCU:\\{class}\\shell\\open\\command' -ErrorAction SilentlyContinue; if ($Protocol -and $ProtocolCommand -and $Protocol.GetValue('CodexAppManagerProtocol') -eq '1' -and $ProtocolCommand.GetValue('') -eq '{}') {{ Remove-Item -LiteralPath 'HKCU:\\{class}' -Recurse -Force -ErrorAction SilentlyContinue; $Capabilities = Get-Item -LiteralPath 'HKCU:\\{cap}' -ErrorAction SilentlyContinue; $UrlAssociations = Get-Item -LiteralPath 'HKCU:\\{cap}\\Capabilities\\UrlAssociations' -ErrorAction SilentlyContinue; if ($Capabilities -and $UrlAssociations -and $Capabilities.GetValue('CodexAppManagerProtocol') -eq '1' -and $UrlAssociations.GetValue('codex') -eq 'CodexAppManager.Portable') {{ $Registered = Get-Item -LiteralPath 'HKCU:\\{apps}' -ErrorAction SilentlyContinue; if ($Registered -and $Registered.GetValue('CodexAppManager.Portable') -eq '{cap}\\Capabilities') {{ Remove-ItemProperty -LiteralPath 'HKCU:\\{apps}' -Name CodexAppManager.Portable -ErrorAction SilentlyContinue }}; Remove-Item -LiteralPath 'HKCU:\\{cap}' -Recurse -Force -ErrorAction SilentlyContinue }} }}", quote(&command(launcher)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ownership_conflicts_leave_registration_unchanged() {
        let root = format!(r"Software\CodexAppManagerTests\{}", uuid::Uuid::new_v4());
        let class = format!(r"{root}\Class");
        let cap = format!(r"{root}\CapabilitiesRoot");
        let apps = format!(r"{root}\RegisteredApplications");
        let launcher = Path::new(r"C:\original\Codex.exe");
        let foreign_cap = create(&cap).unwrap();
        write(&foreign_cap, w!("foreign"), "untouched").unwrap();
        drop(foreign_cap);
        assert!(!register_at(&class, &cap, &apps, launcher, launcher).unwrap());
        assert!(open(&class, KEY_READ).unwrap().is_none());
        assert!(open(&apps, KEY_READ).unwrap().is_none());
        delete_tree(&cap).unwrap();
        assert!(register_at(&class, &cap, &apps, launcher, launcher).unwrap());
        let registered = open(&apps, KEY_READ | KEY_WRITE).unwrap().unwrap();
        write(
            &registered,
            w!("CodexAppManager.Portable"),
            "foreign-capabilities",
        )
        .unwrap();
        drop(registered);
        assert!(!register_at(
            &class,
            &cap,
            &apps,
            Path::new(r"C:\replacement\Codex.exe"),
            launcher
        )
        .unwrap());
        let key = open(&class, KEY_READ).unwrap().unwrap();
        assert_eq!(
            read(&key, Some(COMMAND_KEY), PCWSTR::null())
                .unwrap()
                .as_deref(),
            Some(command(launcher).as_str())
        );
        drop(key);
        delete_tree(&root).unwrap();
    }
    #[test]
    fn registration_and_both_uninstall_paths_preserve_other_apps() {
        let root = format!(r"Software\CodexAppManagerTests\{}", uuid::Uuid::new_v4());
        let class = format!(r"{root}\Class");
        let cap = format!(r"{root}\CapabilitiesRoot");
        let apps = format!(r"{root}\RegisteredApplications");
        let launcher = Path::new(r"C:\便携 O'Neil & space\Codex.exe");
        let registered = create(&apps).unwrap();
        write(&registered, w!("other-app"), "foreign-capabilities").unwrap();
        drop(registered);
        for external in [false, true] {
            assert!(register_at(&class, &cap, &apps, launcher, launcher).unwrap());
            let key = open(&class, KEY_READ).unwrap().unwrap();
            assert_eq!(
                read(&key, Some(COMMAND_KEY), PCWSTR::null())
                    .unwrap()
                    .as_deref(),
                Some(command(launcher).as_str())
            );
            drop(key);
            let registered = open(&apps, KEY_READ).unwrap().unwrap();
            assert_eq!(
                read(&registered, None, w!("CodexAppManager.Portable"))
                    .unwrap()
                    .as_deref(),
                Some(format!(r"{cap}\Capabilities").as_str())
            );
            drop(registered);
            if external {
                assert!(crate::process::hidden_command("powershell.exe")
                    .args([
                        "-NoProfile",
                        "-NonInteractive",
                        "-Command",
                        &uninstall_script_at(&class, &cap, &apps, launcher)
                    ])
                    .status()
                    .unwrap()
                    .success());
            } else {
                assert!(unregister_at(&class, &cap, &apps).unwrap());
            }
            assert!(open(&class, KEY_READ).unwrap().is_none());
            assert!(open(&cap, KEY_READ).unwrap().is_none());
            let registered = open(&apps, KEY_READ).unwrap().unwrap();
            assert_eq!(
                read(&registered, None, w!("other-app")).unwrap().as_deref(),
                Some("foreign-capabilities")
            );
            assert!(read(&registered, None, w!("CodexAppManager.Portable"))
                .unwrap()
                .is_none());
        }
        assert!(register_at(&class, &cap, &apps, launcher, launcher).unwrap());
        let cmd = create(&format!(r"{class}\{COMMAND_KEY}")).unwrap();
        write(&cmd, PCWSTR::null(), "foreign.exe %1").unwrap();
        drop(cmd);
        assert!(!register_at(&class, &cap, &apps, launcher, launcher).unwrap());
        assert!(!unregister_at(&class, &cap, &apps).unwrap());
        assert!(crate::process::hidden_command("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-Command",
                &uninstall_script_at(&class, &cap, &apps, launcher)
            ])
            .status()
            .unwrap()
            .success());
        let key = open(&class, KEY_READ).unwrap().unwrap();
        assert_eq!(
            read(&key, Some(COMMAND_KEY), PCWSTR::null())
                .unwrap()
                .as_deref(),
            Some("foreign.exe %1")
        );
        drop(key);
        delete_tree(&root).unwrap();
    }
}
