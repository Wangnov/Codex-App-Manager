//! Match the upstream process ID and retain Unicode paths in native Shell links.
use std::{os::windows::ffi::OsStrExt, path::Path};
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

pub(crate) fn create(
    shortcut: &Path,
    target: &Path,
    workdir: &Path,
    icon: &Path,
) -> windows::core::Result<()> {
    let _apartment = Apartment::initialize()?;
    let (shortcut, target, workdir, icon) =
        (wide(shortcut), wide(target), wide(workdir), wide(icon));
    let value = PROPVARIANT::from(APP_ID);
    // SAFETY: all paths are terminated and live through these calls. Interfaces
    // and the owned property value drop before the COM apartment, including on error.
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(PCWSTR(target.as_ptr()))?;
        link.SetWorkingDirectory(PCWSTR(workdir.as_ptr()))?;
        link.SetIconLocation(PCWSTR(icon.as_ptr()), 0)?;
        let store: IPropertyStore = link.cast()?;
        store.SetValue(&APP_ID_KEY, &value)?;
        store.Commit()?;
        let persist: IPersistFile = link.cast()?;
        persist.Save(PCWSTR(shortcut.as_ptr()), true)
    }
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
