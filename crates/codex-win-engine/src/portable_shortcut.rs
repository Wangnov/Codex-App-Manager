//! Match the upstream process ID and retain Unicode paths in native Shell links.
use std::{
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
};
use windows::{
    core::{Interface, GUID, PCWSTR},
    Win32::{
        Foundation::{PROPERTYKEY, RPC_E_CHANGED_MODE},
        System::Com::{
            CoCreateInstance, CoInitializeEx, CoUninitialize, IPersistFile,
            StructuredStorage::PROPVARIANT, CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED,
        },
        UI::Shell::{IShellLinkW, PropertiesSystem::IPropertyStore, ShellLink},
    },
};

// The supported stable Codex payload sets this process-wide AppUserModelID.
pub(crate) const APP_ID: &str = "com.openai.codex";
const APP_ID_KEY: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
    pid: 5,
};

struct Apartment(bool);
impl Apartment {
    fn initialize() -> windows::core::Result<Self> {
        // SAFETY: no reserved pointer; initialize the current thread.
        let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
        if initialized != RPC_E_CHANGED_MODE {
            initialized.ok()?;
        }
        Ok(Self(initialized.is_ok()))
    }
}
impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balance this thread's successful CoInitializeEx call.
            unsafe { CoUninitialize() };
        }
    }
}

fn wide(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(Some(0)).collect()
}

/// Everything about a Shell link that this crate owns and compares.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShortcutSpec {
    pub target: PathBuf,
    pub workdir: PathBuf,
    pub icon: PathBuf,
    pub icon_index: i32,
    pub arguments: String,
    pub app_id: String,
}

impl ShortcutSpec {
    pub(crate) fn new(target: &Path, workdir: &Path, icon: &Path, arguments: &str) -> Self {
        Self {
            target: target.to_path_buf(),
            workdir: workdir.to_path_buf(),
            icon: icon.to_path_buf(),
            icon_index: 0,
            arguments: arguments.to_string(),
            app_id: APP_ID.to_string(),
        }
    }

    fn matches(&self, other: &Self) -> bool {
        crate::same_windows_path(&self.target, &other.target)
            && crate::same_windows_path(&self.workdir, &other.workdir)
            && crate::same_windows_path(&self.icon, &other.icon)
            && self.icon_index == other.icon_index
            && self.arguments == other.arguments
            && self.app_id == other.app_id
    }
}

fn buffer_to_path(buffer: &[u16]) -> PathBuf {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    let end = buffer.iter().position(|&unit| unit == 0).unwrap_or(buffer.len());
    OsString::from_wide(&buffer[..end]).into()
}

pub(crate) fn read_spec(shortcut: &Path) -> windows::core::Result<ShortcutSpec> {
    use windows::{core::BSTR, Win32::System::Com::STGM_READ};
    let _apartment = Apartment::initialize()?;
    let shortcut = wide(shortcut);
    // SAFETY: load the saved link into a new object and read into initialized,
    // sized UTF-16 buffers. Interfaces drop before the COM apartment.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let persist: IPersistFile = link.cast()?;
        persist.Load(PCWSTR(shortcut.as_ptr()), STGM_READ)?;
        let mut target = vec![0u16; 32768];
        let mut workdir = vec![0u16; 32768];
        let mut icon = vec![0u16; 32768];
        let mut arguments = vec![0u16; 32768];
        let mut icon_index = 0i32;
        link.GetPath(&mut target, std::ptr::null_mut(), 0)?;
        link.GetWorkingDirectory(&mut workdir)?;
        link.GetIconLocation(&mut icon, &mut icon_index)?;
        link.GetArguments(&mut arguments)?;
        let store: IPropertyStore = link.cast()?;
        let app_id = store
            .GetValue(&APP_ID_KEY)
            .ok()
            .and_then(|value| BSTR::try_from(&value).ok())
            .map(|value| value.to_string())
            .unwrap_or_default();
        Ok(ShortcutSpec {
            target: buffer_to_path(&target),
            workdir: buffer_to_path(&workdir),
            icon: buffer_to_path(&icon),
            icon_index,
            arguments: String::from_utf16_lossy(
                &arguments[..arguments.iter().position(|&u| u == 0).unwrap_or(arguments.len())],
            ),
            app_id,
        })
    }
}

fn write_spec(shortcut: &Path, spec: &ShortcutSpec) -> windows::core::Result<()> {
    let _apartment = Apartment::initialize()?;
    let arguments: Vec<u16> = spec.arguments.encode_utf16().chain([0]).collect();
    let (shortcut, target, workdir, icon) = (
        wide(shortcut),
        wide(&spec.target),
        wide(&spec.workdir),
        wide(&spec.icon),
    );
    let value = PROPVARIANT::from(spec.app_id.as_str());
    // SAFETY: all paths are terminated and live through these calls. Interfaces
    // and the owned property value drop before the COM apartment, including on error.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(PCWSTR(target.as_ptr()))?;
        link.SetArguments(PCWSTR(arguments.as_ptr()))?;
        link.SetWorkingDirectory(PCWSTR(workdir.as_ptr()))?;
        link.SetIconLocation(PCWSTR(icon.as_ptr()), spec.icon_index)?;
        let store: IPropertyStore = link.cast()?;
        store.SetValue(&APP_ID_KEY, &value)?;
        store.Commit()?;
        let persist: IPersistFile = link.cast()?;
        persist.Save(PCWSTR(shortcut.as_ptr()), true)
    }
}

/// Make `shortcut` match `desired`, reading it first. Returns whether the file
/// was written: an identical link is left untouched (no disk write, no churn
/// for Explorer's taskbar/Start icon caches).
pub(crate) fn ensure(shortcut: &Path, desired: &ShortcutSpec) -> windows::core::Result<bool> {
    if shortcut.exists() {
        if let Ok(current) = read_spec(shortcut) {
            if current.matches(desired) {
                return Ok(false);
            }
        }
    }
    write_spec(shortcut, desired)?;
    Ok(true)
}

/// Point an existing link at `icon` while keeping its target, working
/// directory and the user's arguments. Returns whether anything was written.
pub(crate) fn ensure_icon(shortcut: &Path, icon: &Path) -> windows::core::Result<bool> {
    let mut spec = read_spec(shortcut)?;
    spec.icon = icon.to_path_buf();
    spec.icon_index = 0;
    ensure(shortcut, &spec)
}

#[cfg(test)]
pub(crate) fn create(
    shortcut: &Path,
    target: &Path,
    workdir: &Path,
    icon: &Path,
) -> windows::core::Result<()> {
    create_with_arguments(shortcut, target, workdir, icon, "")
}

#[cfg(test)]
pub(crate) fn create_with_arguments(
    shortcut: &Path,
    target: &Path,
    workdir: &Path,
    icon: &Path,
    arguments: &str,
) -> windows::core::Result<()> {
    write_spec(shortcut, &ShortcutSpec::new(target, workdir, icon, arguments))
}

#[cfg(test)]
pub(crate) fn inspect(shortcut: &Path) -> windows::core::Result<(std::path::PathBuf, String)> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    use windows::{core::BSTR, Win32::System::Com::STGM_READ};
    let _apartment = Apartment::initialize()?;
    let shortcut = wide(shortcut);
    // SAFETY: load the saved link into a new object, then read into an initialized
    // UTF-16 buffer; the optional find-data output is null. Interfaces drop before COM.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let persist: IPersistFile = link.cast()?;
        persist.Load(PCWSTR(shortcut.as_ptr()), STGM_READ)?;
        let mut target = vec![0u16; 32768];
        link.GetPath(&mut target, std::ptr::null_mut(), 0)?;
        let end = target
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(target.len());
        let store: IPropertyStore = link.cast()?;
        let id = store.GetValue(&APP_ID_KEY)?;
        Ok((
            OsString::from_wide(&target[..end]).into(),
            BSTR::try_from(&id)?.to_string(),
        ))
    }
}

pub(crate) fn is_manager_launcher(
    shortcut: &Path,
    manager_exe: &Path,
) -> windows::core::Result<bool> {
    use std::{ffi::OsString, os::windows::ffi::OsStringExt};
    use windows::Win32::System::Com::STGM_READ;
    let _apartment = Apartment::initialize()?;
    let shortcut = wide(shortcut);
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        let persist: IPersistFile = link.cast()?;
        persist.Load(PCWSTR(shortcut.as_ptr()), STGM_READ)?;
        let mut target = vec![0u16; 32768];
        let mut arguments = vec![0u16; 32768];
        link.GetPath(&mut target, std::ptr::null_mut(), 0)?;
        link.GetArguments(&mut arguments)?;
        let target_end = target
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(target.len());
        let arguments_end = arguments
            .iter()
            .position(|&unit| unit == 0)
            .unwrap_or(arguments.len());
        let target: std::path::PathBuf = OsString::from_wide(&target[..target_end]).into();
        Ok(crate::same_windows_path(&target, manager_exe)
            && String::from_utf16_lossy(&arguments[..arguments_end]) == "--launch-codex")
    }
}

#[cfg(test)]
mod launch_shortcut_tests {
    use super::*;
    #[test]
    fn identifies_only_manager_owned_launch_shortcuts() {
        let root = std::env::temp_dir().join(format!("codex-launch-link-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let manager = std::env::current_exe().unwrap();
        let shortcut = root.join("Codex.lnk");
        create_with_arguments(&shortcut, &manager, &root, &manager, "--launch-codex").unwrap();
        assert!(is_manager_launcher(&shortcut, &manager).unwrap());
        create_with_arguments(&shortcut, &manager, &root, &manager, "--other").unwrap();
        assert!(!is_manager_launcher(&shortcut, &manager).unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }

    fn temp_root(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("codex-{name}-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    #[test]
    fn ensure_is_idempotent_and_does_not_rewrite_identical_links() {
        let root = temp_root("ensure-idempotent");
        let exe = std::env::current_exe().unwrap();
        let shortcut = root.join("Codex.lnk");
        let spec = ShortcutSpec::new(&exe, &root, &exe, "--launch-codex");
        assert!(ensure(&shortcut, &spec).unwrap());
        let first = std::fs::read(&shortcut).unwrap();
        assert!(!ensure(&shortcut, &spec).unwrap());
        assert_eq!(std::fs::read(&shortcut).unwrap(), first);
        assert_eq!(read_spec(&shortcut).unwrap().arguments, "--launch-codex");
        let mut other = spec.clone();
        other.arguments = "--other".into();
        assert!(ensure(&shortcut, &other).unwrap());
        assert!(!ensure(&shortcut, &other).unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn ensure_icon_only_repairs_icon_and_keeps_user_arguments() {
        let root = temp_root("ensure-icon");
        let exe = std::env::current_exe().unwrap();
        let blank = root.join("blank.exe");
        let shortcut = root.join("Codex.lnk");
        create_with_arguments(&shortcut, &exe, &root, &blank, "--launch-codex --user-flag")
            .unwrap();
        assert!(ensure_icon(&shortcut, &exe).unwrap());
        let spec = read_spec(&shortcut).unwrap();
        assert!(crate::same_windows_path(&spec.icon, &exe));
        assert_eq!(spec.arguments, "--launch-codex --user-flag");
        assert!(crate::same_windows_path(&spec.target, &exe));
        assert!(!ensure_icon(&shortcut, &exe).unwrap());
        std::fs::remove_dir_all(root).unwrap();
    }
}
