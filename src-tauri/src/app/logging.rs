use std::path::{Path, PathBuf};
use std::time::SystemTime;

use tauri::Manager;
use url::Url;

pub const MAX_LOG_FILE_BYTES: u128 = 2 * 1024 * 1024;
pub const KEEP_LOG_FILES: usize = 5;

pub fn logs_dir(app: &tauri::AppHandle) -> Option<PathBuf> {
    app.path().app_log_dir().ok()
}

pub fn redact_url(raw: &str) -> String {
    let Ok(url) = Url::parse(raw.trim()) else {
        return "<invalid-url>".to_string();
    };
    let Some(host) = url.host_str() else {
        return "<invalid-url>".to_string();
    };
    let mut redacted = format!("{}://{}", url.scheme(), host);
    if let Some(port) = url.port() {
        redacted.push(':');
        redacted.push_str(&port.to_string());
    }
    redacted
}

/// Parsed host (plus an explicit non-default port) for user-facing source
/// labels. Reuses the origin redactor so credentials, path, query, and fragment
/// can never enter progress events or logs.
pub fn redact_url_host(raw: &str) -> String {
    let origin = redact_url(raw);
    origin
        .split_once("://")
        .map(|(_, host)| host.to_string())
        .unwrap_or(origin)
}

pub fn prune_old_logs(dir: &Path, keep: usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut logs = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("codex-app-manager") && name.contains(".log"))
        })
        .map(|path| {
            let modified = std::fs::metadata(&path)
                .and_then(|metadata| metadata.modified())
                .unwrap_or(SystemTime::UNIX_EPOCH);
            (modified, path)
        })
        .collect::<Vec<_>>();
    logs.sort_by(|(mtime_a, path_a), (mtime_b, path_b)| {
        mtime_b
            .cmp(mtime_a)
            .then_with(|| path_b.file_name().cmp(&path_a.file_name()))
    });
    for (_, path) in logs.into_iter().skip(keep) {
        match std::fs::remove_file(&path) {
            Ok(()) => {
                let path = path.display();
                log::debug!("pruned old log file path={path}");
            }
            Err(err) => log::warn!(
                "failed to prune old log file path={} error={err}",
                path.display()
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{prune_old_logs, redact_url, redact_url_host};

    fn temp_dir(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("{name}-{}", std::process::id()))
    }

    #[test]
    fn redact_url_keeps_only_origin() {
        assert_eq!(
            redact_url("https://u:p@example.com:8443/a/b?x=1#frag"),
            "https://example.com:8443"
        );
        assert_eq!(redact_url("http://127.0.0.1/path"), "http://127.0.0.1");
        assert_eq!(redact_url("127.0.0.1/path"), "<invalid-url>");
        assert_eq!(redact_url("not a url"), "<invalid-url>");
        assert_eq!(
            redact_url_host("https://u:p@example.com:8443/a/b?x=1#frag"),
            "example.com:8443"
        );
        assert_eq!(redact_url_host("not a url"), "<invalid-url>");
    }

    #[test]
    fn prune_old_logs_keeps_newest_by_mtime_then_name() {
        let dir = temp_dir("codex-manager-log-prune");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        for idx in 0..7 {
            std::fs::write(dir.join(format!("codex-app-manager.{idx}.log")), b"log").unwrap();
        }
        std::fs::write(dir.join("other.log"), b"keep").unwrap();

        prune_old_logs(&dir, 5);

        let mut names = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(Result::ok)
            .filter_map(|entry| entry.file_name().into_string().ok())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(
            names,
            vec![
                "codex-app-manager.2.log",
                "codex-app-manager.3.log",
                "codex-app-manager.4.log",
                "codex-app-manager.5.log",
                "codex-app-manager.6.log",
                "other.log",
            ]
        );

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn prune_old_logs_tolerates_missing_dir() {
        prune_old_logs(&temp_dir("codex-manager-log-missing"), 5);
    }
}
