//! Built and embedded by build.rs; no shell, runtime installer or global settings.
#![windows_subsystem = "windows"]
mod portable_command;

use portable_command::LAUNCH_TARGET_NAME;
use std::{env, fs, io, process::Command};

#[used]
#[link_section = ".camlnch"]
static LAUNCHER_ID: [u8; 33] = portable_command::LAUNCHER_MARKER;

fn run() -> io::Result<()> {
    std::hint::black_box(&LAUNCHER_ID);
    let own = env::current_exe()?;
    let root = own
        .parent()
        .ok_or_else(|| io::Error::other("missing launcher directory"))?;
    let config = fs::read_to_string(root.join(LAUNCH_TARGET_NAME))?;
    let (target, required) = portable_command::parse_launch_config(&config).ok_or_else(|| {
        io::Error::other("Invalid portable launch target. Reinstall with Codex App Manager.")
    })?;
    let exe = root.join(target);
    if fs::canonicalize(&exe)? == fs::canonicalize(&own)? {
        return Err(io::Error::other("Portable launcher cannot launch itself."));
    }
    let mut command = Command::new(&exe);
    portable_command::configure(&mut command, &exe, required)?;
    command.env("CODEX_SPARKLE_ENABLED", "false");
    command.args(env::args_os().skip(1));
    command.spawn()?;
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        #[link(name = "user32")]
        extern "system" {
            fn MessageBoxW(
                window: *mut std::ffi::c_void,
                text: *const u16,
                caption: *const u16,
                flags: u32,
            ) -> i32;
        }
        let text: Vec<u16> = format!("Codex could not start.\n\n{error}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let caption: Vec<u16> = "Codex portable launcher"
            .encode_utf16()
            .chain(Some(0))
            .collect();
        // SAFETY: both zero-terminated buffers live through the synchronous call.
        unsafe {
            MessageBoxW(std::ptr::null_mut(), text.as_ptr(), caption.as_ptr(), 0x10);
        }
        std::process::exit(1);
    }
}
