use std::path::{Path, PathBuf};

use crate::errors::AppError;

/// Available bytes for the current user on the volume containing `path`.
/// `Ok(None)` means the platform cannot measure it and should not block work.
pub fn available_space(path: &Path) -> Result<Option<u64>, AppError> {
    platform_available_space(&nearest_existing_dir(path))
}

fn nearest_existing_dir(path: &Path) -> PathBuf {
    let mut cur = path;
    loop {
        if cur.is_dir() {
            return cur.to_path_buf();
        }
        match cur.parent() {
            Some(parent) => cur = parent,
            None => return cur.to_path_buf(),
        }
    }
}

#[cfg(windows)]
fn platform_available_space(path: &Path) -> Result<Option<u64>, AppError> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;

    let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
    wide.push(0);
    let mut free_to_caller = 0_u64;
    let mut total = 0_u64;
    let mut total_free = 0_u64;
    let ok = unsafe {
        GetDiskFreeSpaceExW(
            wide.as_ptr(),
            &mut free_to_caller,
            &mut total,
            &mut total_free,
        )
    };
    if ok == 0 {
        return Err(AppError::Internal(format!(
            "读取磁盘剩余空间失败: {}",
            std::io::Error::last_os_error()
        )));
    }
    Ok(Some(free_to_caller))
}

#[cfg(unix)]
fn platform_available_space(path: &Path) -> Result<Option<u64>, AppError> {
    use std::ffi::CString;
    use std::mem::MaybeUninit;
    use std::os::unix::ffi::OsStrExt;

    let c_path = CString::new(path.as_os_str().as_bytes())
        .map_err(|e| AppError::Internal(format!("读取磁盘剩余空间失败: {e}")))?;
    let mut stat = MaybeUninit::<libc::statvfs>::uninit();
    let rc = unsafe { libc::statvfs(c_path.as_ptr(), stat.as_mut_ptr()) };
    if rc != 0 {
        return Err(AppError::Internal(format!(
            "读取磁盘剩余空间失败: {}",
            std::io::Error::last_os_error()
        )));
    }
    let stat = unsafe { stat.assume_init() };
    let fragment_size = if stat.f_frsize == 0 {
        stat.f_bsize as u128
    } else {
        stat.f_frsize as u128
    };
    let bytes = (stat.f_bavail as u128).saturating_mul(fragment_size);
    Ok(Some(bytes.min(u64::MAX as u128) as u64))
}

#[cfg(not(any(windows, unix)))]
fn platform_available_space(_path: &Path) -> Result<Option<u64>, AppError> {
    Ok(None)
}

#[cfg(test)]
mod tests {
    #[cfg(unix)]
    use super::available_space;

    #[cfg(unix)]
    #[test]
    fn unix_available_space_handles_nonexistent_child_path() {
        let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("missing")
            .join("child");
        assert!(available_space(&path).unwrap().unwrap() > 0);
    }
}
