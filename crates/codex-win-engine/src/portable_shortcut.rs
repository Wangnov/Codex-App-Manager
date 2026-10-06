//! Match the stable upstream process ID so Windows pins the launcher shortcut.
use std::{os::windows::ffi::OsStrExt, path::Path};

use windows::{
    core::{GUID, PCWSTR},
    Win32::{
        Foundation::{PROPERTYKEY, RPC_E_CHANGED_MODE},
        System::Com::{
            CoInitializeEx, CoUninitialize, StructuredStorage::PROPVARIANT,
            COINIT_APARTMENTTHREADED,
        },
        UI::Shell::PropertiesSystem::{
            IPropertyStore, SHGetPropertyStoreFromParsingName, GPS_READWRITE,
        },
    },
};

// The supported stable Codex payload sets this process-wide AppUserModelID.
pub(crate) const APP_ID: &str = "com.openai.codex";
const APP_ID_KEY: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
    pid: 5,
};

struct Apartment(bool);
impl Drop for Apartment {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: balance this thread's successful CoInitializeEx call.
            unsafe { CoUninitialize() };
        }
    }
}

pub(crate) fn set_app_id(shortcut: &Path) -> windows::core::Result<()> {
    // SAFETY: no reserved pointer; this call initializes the current thread.
    let initialized = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    if initialized != RPC_E_CHANGED_MODE {
        initialized.ok()?;
    }
    let _apartment = Apartment(initialized.is_ok());
    let path: Vec<u16> = shortcut.as_os_str().encode_wide().chain(Some(0)).collect();
    let value = PROPVARIANT::from(APP_ID);
    // SAFETY: path is terminated and value owns its string through these calls.
    // The store drops before the apartment, including on an error return.
    unsafe {
        let store: IPropertyStore =
            SHGetPropertyStoreFromParsingName(PCWSTR(path.as_ptr()), None, GPS_READWRITE)?;
        store.SetValue(&APP_ID_KEY, &value)?;
        store.Commit()
    }
}
