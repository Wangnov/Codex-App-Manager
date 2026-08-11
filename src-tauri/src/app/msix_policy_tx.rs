//! Crash-safe self-update-policy intent for historical MSIX deployment.
//!
//! Add-AppxPackage is an external registration transaction, so there is no
//! same-volume rename journal that can also carry the user's explicit
//! "block updates" choice. This small companion journal is written immediately
//! before deployment. Startup compares the exact registered PackageFullName with
//! the signed target: a landed target keeps the requested policy; anything else
//! restores the previous policy.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::app::atomic_file;
use crate::app::install_tx::SelfUpdatePolicyTransition;
use crate::app::paths;
use crate::errors::AppError;

const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct MsixPolicyJournal {
    schema_version: u32,
    id: String,
    target_package_full_name: String,
    transition: SelfUpdatePolicyTransition,
    started_unix: u64,
}

fn now_unix() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn journal_dir() -> Option<PathBuf> {
    paths::data_dir().map(|dir| dir.join("msix-policy-transactions"))
}

fn journal_path(id: &str) -> Option<PathBuf> {
    journal_dir().map(|dir| dir.join(format!("{id}.json")))
}

impl MsixPolicyJournal {
    fn begin(
        target_package_full_name: &str,
        transition: SelfUpdatePolicyTransition,
    ) -> Result<Self, AppError> {
        let target = target_package_full_name.trim();
        if target.is_empty()
            || target.len() > 512
            || !target.to_ascii_lowercase().starts_with("openai.codex_")
        {
            return Err(AppError::Internal(
                "MSIX 策略事务的目标包身份无效".to_string(),
            ));
        }
        if pending_journal_exists() {
            return Err(AppError::Internal(
                "仍有上一次 MSIX 安装策略等待恢复，请重启管理器后再试".to_string(),
            ));
        }
        let journal = Self {
            schema_version: SCHEMA_VERSION,
            id: uuid::Uuid::new_v4().to_string(),
            target_package_full_name: target.to_string(),
            transition,
            started_unix: now_unix(),
        };
        journal.persist()?;
        log::info!(
            "MSIX policy transaction prepared id={} target={}",
            journal.id,
            journal.target_package_full_name
        );
        Ok(journal)
    }

    fn persist(&self) -> Result<(), AppError> {
        let path = journal_path(&self.id)
            .ok_or_else(|| AppError::Internal("无法定位 MSIX 策略事务目录".to_string()))?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)
                .map_err(|e| AppError::Internal(format!("创建 MSIX 策略事务目录失败: {e}")))?;
        }
        let bytes = serde_json::to_vec_pretty(self)
            .map_err(|e| AppError::Internal(format!("序列化 MSIX 策略事务失败: {e}")))?;
        atomic_file::write_atomic(&path, &bytes)
            .map_err(|e| AppError::Internal(format!("写入 MSIX 策略事务失败: {e}")))
    }

    fn load(path: &Path) -> Result<Self, AppError> {
        let bytes = fs::read(path)
            .map_err(|e| AppError::Internal(format!("读取 MSIX 策略事务失败: {e}")))?;
        let journal: Self = serde_json::from_slice(&bytes)
            .map_err(|e| AppError::Internal(format!("解析 MSIX 策略事务失败: {e}")))?;
        if journal.schema_version != SCHEMA_VERSION {
            return Err(AppError::Internal(format!(
                "不支持的 MSIX 策略事务版本 {}",
                journal.schema_version
            )));
        }
        Ok(journal)
    }

    fn remove_checked(&self) -> Result<(), AppError> {
        let path = journal_path(&self.id)
            .ok_or_else(|| AppError::Internal("无法定位 MSIX 策略事务目录".to_string()))?;
        match fs::remove_file(&path) {
            Ok(()) => Ok(()),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(err) => Err(AppError::Internal(format!(
                "删除 MSIX 策略事务失败（{}）: {err}",
                path.display()
            ))),
        }
    }

    fn finish(self, target_landed: bool) -> Vec<String> {
        let disabled = if target_landed {
            self.transition.requested_disabled
        } else {
            self.transition.previous_disabled
        };
        let mut warnings = Vec::new();
        if let Err(err) = crate::app::codex_self_update::sync_and_persist_setting(disabled) {
            let warning =
                format!("MSIX 已处理，但自动更新策略暂未完成（{err}）；管理器下次启动会重试");
            log::warn!(
                "MSIX policy transaction finalization warning id={} error={err}",
                self.id
            );
            warnings.push(warning);
            return warnings;
        }
        if let Err(err) = self.remove_checked() {
            let warning = format!(
                "自动更新策略已生效，但 MSIX 策略事务日志暂未清理（{err}）；下次启动会自动清理"
            );
            log::warn!(
                "MSIX policy transaction cleanup warning id={} error={err}",
                self.id
            );
            warnings.push(warning);
        }
        warnings
    }
}

fn pending_journal_exists() -> bool {
    let Some(dir) = journal_dir() else {
        return false;
    };
    fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .any(|entry| entry.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
}

fn target_landed(current: Option<&str>, target: &str) -> bool {
    current.is_some_and(|current| current.eq_ignore_ascii_case(target))
}

pub struct ActiveMsixPolicyTransition {
    inner: Option<MsixPolicyJournal>,
}

impl ActiveMsixPolicyTransition {
    pub fn begin(
        target_package_full_name: &str,
        transition: SelfUpdatePolicyTransition,
    ) -> Result<Self, AppError> {
        Ok(Self {
            inner: Some(MsixPolicyJournal::begin(
                target_package_full_name,
                transition,
            )?),
        })
    }

    pub fn commit_landed(mut self) -> Vec<String> {
        self.inner
            .take()
            .map(|journal| journal.finish(true))
            .unwrap_or_default()
    }

    pub fn rollback_not_landed(mut self) -> Vec<String> {
        self.inner
            .take()
            .map(|journal| journal.finish(false))
            .unwrap_or_default()
    }
}

impl Drop for ActiveMsixPolicyTransition {
    fn drop(&mut self) {
        if let Some(journal) = self.inner.take() {
            log::warn!(
                "MSIX policy transaction left pending for startup recovery id={} target={}",
                journal.id,
                journal.target_package_full_name
            );
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct MsixPolicyRecoverySummary {
    pub scanned: usize,
    pub committed: usize,
    pub rolled_back: usize,
    pub failed: usize,
}

/// Resolve pending policy intents from exact registered package identity. Called
/// during startup before normal installs are allowed to acquire the operation
/// lease.
pub fn recover_pending_msix_policy_transitions() -> MsixPolicyRecoverySummary {
    let mut summary = MsixPolicyRecoverySummary::default();
    let Some(dir) = journal_dir() else {
        return summary;
    };
    let Ok(entries) = fs::read_dir(dir) else {
        return summary;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        summary.scanned += 1;
        let result = (|| -> Result<bool, AppError> {
            let journal = MsixPolicyJournal::load(&path)?;
            let current = codex_win_engine::registered_msix_package_full_name()
                .map_err(|e| AppError::Engine(e.to_string()))?;
            let landed = target_landed(current.as_deref(), &journal.target_package_full_name);
            let disabled = if landed {
                journal.transition.requested_disabled
            } else {
                journal.transition.previous_disabled
            };
            crate::app::codex_self_update::sync_and_persist_setting(disabled)?;
            journal.remove_checked()?;
            Ok(landed)
        })();
        match result {
            Ok(true) => summary.committed += 1,
            Ok(false) => summary.rolled_back += 1,
            Err(err) => {
                summary.failed += 1;
                log::error!(
                    "MSIX policy transaction recovery failed path={} error={err}",
                    path.display()
                );
            }
        }
    }
    if summary.scanned > 0 {
        log::info!(
            "MSIX policy transaction recovery summary scanned={} committed={} rolled_back={} failed={}",
            summary.scanned,
            summary.committed,
            summary.rolled_back,
            summary.failed
        );
    }
    summary
}

#[cfg(test)]
mod tests {
    use super::target_landed;

    #[test]
    fn recovery_requires_the_exact_target_package_full_name() {
        let target = "OpenAI.Codex_26.803.10989.0_x64__2p2nqsd0c76g0";
        assert!(target_landed(Some(target), target));
        assert!(target_landed(
            Some("openai.codex_26.803.10989.0_X64__2P2NQSD0C76G0"),
            target
        ));
        assert!(!target_landed(
            Some("OpenAI.Codex_26.803.10000.0_x64__2p2nqsd0c76g0"),
            target
        ));
        assert!(!target_landed(None, target));
    }
}
