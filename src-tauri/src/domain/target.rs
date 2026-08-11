use serde::Serialize;

#[cfg(target_os = "macos")]
use std::process::{Command, Stdio};

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum OperatingSystem {
    Windows,
    Macos,
    Linux,
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Architecture {
    X64,
    Arm64,
    Unknown,
}

impl Architecture {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::X64 => "x86_64",
            Self::Arm64 => "aarch64",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct Target {
    pub os: OperatingSystem,
    pub arch: Architecture,
    pub label: String,
}

impl Target {
    pub fn current() -> Self {
        let os = if cfg!(target_os = "windows") {
            OperatingSystem::Windows
        } else if cfg!(target_os = "macos") {
            OperatingSystem::Macos
        } else if cfg!(target_os = "linux") {
            OperatingSystem::Linux
        } else {
            OperatingSystem::Unknown
        };

        // `std::env::consts::ARCH` is the process architecture. An x64 manager
        // can run through Rosetta 2 / Windows emulation on an ARM64 host, where
        // using the process value would hide and reject the native ARM64 Codex
        // package. Prefer an OS-native host probe and fall back to the process
        // architecture only when the platform cannot report it.
        let arch = native_architecture().unwrap_or_else(process_architecture);

        let label = format!("{os:?} / {arch:?}");

        Self { os, arch, label }
    }
}

fn process_architecture() -> Architecture {
    architecture_from_runtime(std::env::consts::ARCH)
}

fn architecture_from_runtime(value: &str) -> Architecture {
    match value {
        "x86_64" | "x64" | "amd64" => Architecture::X64,
        "aarch64" | "arm64" => Architecture::Arm64,
        _ => Architecture::Unknown,
    }
}

#[cfg(target_os = "macos")]
fn native_architecture() -> Option<Architecture> {
    if process_architecture() == Architecture::Arm64 {
        return Some(Architecture::Arm64);
    }

    // Apple documents `sysctl.proc_translated == 1` as the Rosetta 2 signal.
    // `hw.optional.arm64` is a second native-hardware fallback for environments
    // where the translation key is unavailable.
    if sysctl_i32(b"sysctl.proc_translated\0") == Some(1)
        || sysctl_i32(b"hw.optional.arm64\0") == Some(1)
    {
        Some(Architecture::Arm64)
    } else {
        Some(process_architecture())
    }
}

#[cfg(target_os = "macos")]
fn sysctl_i32(name: &'static [u8]) -> Option<i32> {
    let mut value = 0_i32;
    let mut len = std::mem::size_of::<i32>();
    let result = unsafe {
        libc::sysctlbyname(
            name.as_ptr().cast(),
            (&mut value as *mut i32).cast(),
            &mut len,
            std::ptr::null_mut(),
            0,
        )
    };
    (result == 0 && len == std::mem::size_of::<i32>()).then_some(value)
}

/// Probe the actual x86_64 execution path instead of inferring Rosetta from the
/// host architecture. On a native Apple Silicon process `sysctl.proc_translated`
/// is normally absent/zero even when Rosetta is installed, while successfully
/// starting `/usr/bin/true` through `arch -x86_64` proves that the selected Intel
/// Codex build can execute. The probe neither installs Rosetta nor opens UI.
#[cfg(target_os = "macos")]
pub fn macos_x64_compatibility_available() -> bool {
    Command::new("/usr/bin/arch")
        .args(["-x86_64", "/usr/bin/true"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(not(target_os = "macos"))]
pub fn macos_x64_compatibility_available() -> bool {
    false
}

/// Ask Windows whether the native host can execute an AMD64 guest binary.
/// ARM64 support for x64 emulation varies by Windows release, so the host
/// architecture alone is not enough to accept an x64 historical package.
#[cfg(target_os = "windows")]
pub fn windows_x64_compatibility_available() -> bool {
    use windows_sys::Win32::System::SystemInformation::{
        IsWow64GuestMachineSupported, IMAGE_FILE_MACHINE_AMD64,
    };

    let mut supported = 0;
    let result = unsafe { IsWow64GuestMachineSupported(IMAGE_FILE_MACHINE_AMD64, &mut supported) };
    result >= 0 && supported != 0
}

#[cfg(not(target_os = "windows"))]
pub fn windows_x64_compatibility_available() -> bool {
    false
}

#[cfg(target_os = "windows")]
fn native_architecture() -> Option<Architecture> {
    use windows_sys::Win32::System::SystemInformation::{
        GetNativeSystemInfo, PROCESSOR_ARCHITECTURE_AMD64, PROCESSOR_ARCHITECTURE_ARM64,
        SYSTEM_INFO,
    };

    let mut info = SYSTEM_INFO::default();
    unsafe { GetNativeSystemInfo(&mut info) };
    let architecture = unsafe { info.Anonymous.Anonymous.wProcessorArchitecture };
    match architecture {
        PROCESSOR_ARCHITECTURE_AMD64 => Some(Architecture::X64),
        PROCESSOR_ARCHITECTURE_ARM64 => Some(Architecture::Arm64),
        _ => None,
    }
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn native_architecture() -> Option<Architecture> {
    None
}

#[cfg(test)]
mod tests {
    use super::{architecture_from_runtime, Architecture};

    #[test]
    fn runtime_architecture_aliases_are_normalized() {
        assert_eq!(architecture_from_runtime("x86_64"), Architecture::X64);
        assert_eq!(architecture_from_runtime("amd64"), Architecture::X64);
        assert_eq!(architecture_from_runtime("aarch64"), Architecture::Arm64);
        assert_eq!(architecture_from_runtime("arm64"), Architecture::Arm64);
        assert_eq!(architecture_from_runtime("mips"), Architecture::Unknown);
    }
}
