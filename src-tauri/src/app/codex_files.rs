//! Read/write Codex CLI/desktop user files under `~/.codex`
//! (`auth.json`, `config.toml`) so users can paste sub2api-style relay configs.

use std::fs;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::app::atomic_file;
use crate::app::paths;
use crate::errors::AppError;

/// Soft cap so a paste cannot fill the disk or lock the UI with multi‑MB blobs.
const MAX_BYTES: usize = 512 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexFileKind {
    Auth,
    Config,
}

impl CodexFileKind {
    pub fn parse(which: &str) -> Result<Self, AppError> {
        match which.trim() {
            "auth" => Ok(Self::Auth),
            "config" => Ok(Self::Config),
            other => Err(AppError::Internal(format!(
                "unknown codex file kind: {other}"
            ))),
        }
    }

    pub fn file_name(self) -> &'static str {
        match self {
            Self::Auth => "auth.json",
            Self::Config => "config.toml",
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auth => "auth",
            Self::Config => "config",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexFileSnapshot {
    pub which: String,
    pub path: String,
    pub content: String,
    pub exists: bool,
    pub bytes: u64,
}

fn codex_home() -> Result<PathBuf, AppError> {
    paths::codex_home_dir()
        .ok_or_else(|| AppError::Internal("无法定位 ~/.codex 目录".to_string()))
}

fn file_path(kind: CodexFileKind) -> Result<PathBuf, AppError> {
    Ok(codex_home()?.join(kind.file_name()))
}

fn validate_content(kind: CodexFileKind, content: &str) -> Result<(), AppError> {
    if content.len() > MAX_BYTES {
        return Err(AppError::Internal(format!(
            "{} 内容过大（上限 {} KB）",
            kind.file_name(),
            MAX_BYTES / 1024
        )));
    }
    if content.bytes().any(|b| b == 0) {
        return Err(AppError::Internal(format!(
            "{} 不能包含空字节",
            kind.file_name()
        )));
    }
    match kind {
        CodexFileKind::Auth => {
            let trimmed = content.trim();
            if trimmed.is_empty() {
                return Ok(());
            }
            let value: serde_json::Value = serde_json::from_str(trimmed).map_err(|e| {
                AppError::Internal(format!("auth.json 不是合法 JSON: {e}"))
            })?;
            if !value.is_object() {
                return Err(AppError::Internal(
                    "auth.json 顶层必须是 JSON 对象 {{ … }}".to_string(),
                ));
            }
            Ok(())
        }
        CodexFileKind::Config => {
            // config.toml is free-form TOML used by Codex + third-party relays
            // (sub2api, custom providers). We only enforce size/encoding safety;
            // Codex itself reports semantic errors on next launch.
            Ok(())
        }
    }
}

fn normalize_for_write(kind: CodexFileKind, content: &str) -> String {
    match kind {
        CodexFileKind::Auth => {
            let trimmed = content.trim();
            if trimmed.is_empty() {
                // Empty paste → empty object so the file stays valid JSON.
                "{}\n".to_string()
            } else {
                // Pretty-print for readability after paste (preserve keys).
                match serde_json::from_str::<serde_json::Value>(trimmed) {
                    Ok(value) => {
                        let mut out = serde_json::to_string_pretty(&value)
                            .unwrap_or_else(|_| trimmed.to_string());
                        if !out.ends_with('\n') {
                            out.push('\n');
                        }
                        out
                    }
                    Err(_) => content.to_string(),
                }
            }
        }
        CodexFileKind::Config => {
            let mut out = content.to_string();
            // Normalize bare CR endings from some Windows pastes.
            if out.contains('\r') {
                out = out.replace("\r\n", "\n").replace('\r', "\n");
            }
            out
        }
    }
}

/// Restrict mode bits on secret-bearing paths (auth.json and its .bak).
/// Failures are logged, not fatal — the write already succeeded.
#[cfg(unix)]
fn tighten_secret_permissions(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    match fs::metadata(path) {
        Ok(meta) => {
            let mut perms = meta.permissions();
            perms.set_mode(0o600);
            if let Err(e) = fs::set_permissions(path, perms) {
                log::warn!(
                    "could not set 0o600 on {}: {e}",
                    path.display()
                );
            }
        }
        Err(e) => {
            log::warn!(
                "could not stat {} for permission tighten: {e}",
                path.display()
            );
        }
    }
}

#[cfg(not(unix))]
fn tighten_secret_permissions(path: &Path) {
    // Windows ACLs are user-profile local by default for %USERPROFILE%\.codex;
    // no portable equivalent without extra crates — log for audit only.
    log::debug!(
        "skip mode tighten on non-unix path={}",
        path.display()
    );
}

#[cfg(unix)]
fn ensure_codex_home_private(home: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = fs::metadata(home) {
        let mode = meta.permissions().mode() & 0o777;
        if mode & 0o077 != 0 {
            let mut perms = meta.permissions();
            perms.set_mode(0o700);
            if let Err(e) = fs::set_permissions(home, perms) {
                log::warn!(
                    "could not set 0o700 on {}: {e}",
                    home.display()
                );
            }
        }
    }
}

#[cfg(not(unix))]
fn ensure_codex_home_private(_home: &Path) {}

pub fn read_file(kind: CodexFileKind) -> Result<CodexFileSnapshot, AppError> {
    let path = file_path(kind)?;
    let path_display = path.display().to_string();
    if !path.exists() {
        return Ok(CodexFileSnapshot {
            which: kind.as_str().to_string(),
            path: path_display,
            content: String::new(),
            exists: false,
            bytes: 0,
        });
    }
    let bytes = fs::read(&path).map_err(|e| {
        AppError::Internal(format!("读取 {} 失败: {e}", kind.file_name()))
    })?;
    if bytes.len() > MAX_BYTES {
        return Err(AppError::Internal(format!(
            "{} 过大（{} bytes），请用外部编辑器处理",
            kind.file_name(),
            bytes.len()
        )));
    }
    let content = String::from_utf8(bytes).map_err(|e| {
        AppError::Internal(format!("{} 不是合法 UTF-8: {e}", kind.file_name()))
    })?;
    let len = content.len() as u64;
    Ok(CodexFileSnapshot {
        which: kind.as_str().to_string(),
        path: path_display,
        content,
        exists: true,
        bytes: len,
    })
}

pub fn write_file(kind: CodexFileKind, content: &str) -> Result<CodexFileSnapshot, AppError> {
    validate_content(kind, content)?;
    let normalized = normalize_for_write(kind, content);
    validate_content(kind, &normalized)?;

    let home = codex_home()?;
    fs::create_dir_all(&home).map_err(|e| {
        AppError::Internal(format!("创建 ~/.codex 失败: {e}"))
    })?;
    ensure_codex_home_private(&home);

    let path = home.join(kind.file_name());
    atomic_file::write_atomic(&path, normalized.as_bytes()).map_err(|e| {
        AppError::Internal(format!("写入 {} 失败: {e}", kind.file_name()))
    })?;

    if kind == CodexFileKind::Auth {
        // Final file + leftover .bak both hold prior/current secrets.
        tighten_secret_permissions(&path);
        let bak = atomic_file::backup_path(&path);
        if bak.exists() {
            tighten_secret_permissions(&bak);
        }
    }

    log::info!(
        "wrote codex file which={} path={} bytes={}",
        kind.as_str(),
        path.display(),
        normalized.len()
    );

    read_file(kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_kind() {
        assert_eq!(CodexFileKind::parse("auth").unwrap(), CodexFileKind::Auth);
        assert_eq!(CodexFileKind::parse("config").unwrap(), CodexFileKind::Config);
        assert!(CodexFileKind::parse("nope").is_err());
    }

    #[test]
    fn auth_requires_json_object() {
        assert!(validate_content(CodexFileKind::Auth, r#"{"OPENAI_API_KEY":"x"}"#).is_ok());
        assert!(validate_content(CodexFileKind::Auth, "[]").is_err());
        assert!(validate_content(CodexFileKind::Auth, "not-json").is_err());
        assert!(validate_content(CodexFileKind::Auth, "").is_ok());
    }

    #[test]
    fn config_accepts_relay_style_toml() {
        let sample = r#"
model_provider = "custom"
model = "gpt-5"

[model_providers.custom]
name = "sub2api"
base_url = "https://example.com/v1"
wire_api = "responses"
"#;
        assert!(validate_content(CodexFileKind::Config, sample).is_ok());
    }

    #[test]
    fn normalize_auth_pretty_prints() {
        let out = normalize_for_write(CodexFileKind::Auth, r#"{"a":1}"#);
        assert!(out.contains("\n"));
        assert!(out.contains("\"a\""));
    }

    #[test]
    fn rejects_oversized_payload() {
        let big = "x".repeat(MAX_BYTES + 1);
        assert!(validate_content(CodexFileKind::Config, &big).is_err());
    }
}
