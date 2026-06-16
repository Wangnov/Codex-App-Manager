use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use serde::de::DeserializeOwned;

static TMP_COUNTER: AtomicU64 = AtomicU64::new(1);
static WRITE_MUTEX: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoadOutcome {
    Ok,
    RecoveredFromBak,
    Corrupt,
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let _guard = WRITE_MUTEX
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let parent = path
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "atomic path has no parent"))?;
    fs::create_dir_all(parent)?;

    let tmp = tmp_path(path);
    let result = write_atomic_inner(path, &tmp, bytes);
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn write_atomic_inner(path: &Path, tmp: &Path, bytes: &[u8]) -> io::Result<()> {
    {
        let mut file = File::create(tmp)?;
        file.write_all(bytes)?;
        file.flush()?;
        file.sync_all()?;
    }

    let bak = backup_path(path);
    if path.exists() {
        let _ = fs::remove_file(&bak);
        match fs::rename(path, &bak) {
            Ok(()) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
    }

    if let Err(err) = fs::rename(tmp, path) {
        if !path.exists() && bak.exists() {
            let _ = fs::rename(&bak, path);
        }
        return Err(err);
    }

    if let Some(parent) = path.parent() {
        sync_parent_dir(parent)?;
    }
    Ok(())
}

pub fn read_with_recovery<T: DeserializeOwned>(path: &Path) -> (Option<T>, LoadOutcome) {
    if let Some(value) = read_json(path) {
        return (Some(value), LoadOutcome::Ok);
    }
    if let Some(value) = read_json(&backup_path(path)) {
        return (Some(value), LoadOutcome::RecoveredFromBak);
    }
    (None, LoadOutcome::Corrupt)
}

fn read_json<T: DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn tmp_path(path: &Path) -> PathBuf {
    let counter = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    path_with_added_extension(path, &format!("tmp-{}-{counter}", std::process::id()))
}

pub fn backup_path(path: &Path) -> PathBuf {
    path_with_added_extension(path, "bak")
}

fn path_with_added_extension(path: &Path, added: &str) -> PathBuf {
    match path.extension().and_then(|ext| ext.to_str()) {
        Some(ext) if !ext.is_empty() => path.with_extension(format!("{ext}.{added}")),
        _ => path.with_extension(added),
    }
}

#[cfg(unix)]
fn sync_parent_dir(parent: &Path) -> io::Result<()> {
    File::open(parent)?.sync_all()
}

#[cfg(not(unix))]
fn sync_parent_dir(_parent: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{read_with_recovery, write_atomic, LoadOutcome};
    use serde::{Deserialize, Serialize};
    use std::fs;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEST_COUNTER: AtomicU64 = AtomicU64::new(1);

    #[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
    struct Payload {
        value: u64,
    }

    fn test_dir(name: &str) -> std::path::PathBuf {
        let id = TEST_COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("test-data")
            .join(format!("{name}-{}-{id}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn write_atomic_replaces_main_and_preserves_backup() {
        let dir = test_dir("atomic-replace");
        let path = dir.join("settings.json");
        write_atomic(&path, br#"{"value":1}"#).unwrap();
        write_atomic(&path, br#"{"value":2}"#).unwrap();

        let (value, outcome) = read_with_recovery::<Payload>(&path);
        assert_eq!(outcome, LoadOutcome::Ok);
        assert_eq!(value.unwrap(), Payload { value: 2 });
        assert_eq!(
            serde_json::from_slice::<Payload>(&fs::read(path.with_extension("json.bak")).unwrap())
                .unwrap(),
            Payload { value: 1 }
        );

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn read_with_recovery_uses_backup_for_corrupt_main() {
        let dir = test_dir("atomic-recover");
        let path = dir.join("provenance.json");
        fs::write(&path, b"{").unwrap();
        fs::write(path.with_extension("json.bak"), br#"{"value":7}"#).unwrap();

        let (value, outcome) = read_with_recovery::<Payload>(&path);
        assert_eq!(outcome, LoadOutcome::RecoveredFromBak);
        assert_eq!(value.unwrap(), Payload { value: 7 });

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn read_with_recovery_reports_corrupt_when_main_and_backup_fail() {
        let dir = test_dir("atomic-corrupt");
        let path = dir.join("settings.json");
        fs::write(&path, b"").unwrap();
        fs::write(path.with_extension("json.bak"), b"{").unwrap();

        let (value, outcome) = read_with_recovery::<Payload>(&path);
        assert_eq!(outcome, LoadOutcome::Corrupt);
        assert!(value.is_none());

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn concurrent_writes_leave_complete_json() {
        let dir = test_dir("atomic-concurrent");
        let path = dir.join("settings.json");
        write_atomic(&path, br#"{"value":0}"#).unwrap();

        let handles: Vec<_> = (1_u64..=8)
            .map(|value| {
                let path = path.clone();
                std::thread::spawn(move || {
                    for _ in 0..10 {
                        let bytes = serde_json::to_vec(&Payload { value }).unwrap();
                        write_atomic(&path, &bytes).unwrap();
                    }
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }

        let (value, outcome) = read_with_recovery::<Payload>(&path);
        assert_eq!(outcome, LoadOutcome::Ok);
        assert!(matches!(value.unwrap().value, 1..=8));

        let _ = fs::remove_dir_all(dir);
    }
}
