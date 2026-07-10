//! Structured outcomes for install/uninstall (and similar) operations.
//!
//! Primary work can succeed while ancillary steps fail — e.g. the app is on
//! disk but provenance could not be saved, or the app is gone but shortcut
//! cleanup failed. The UI must show disk truth and only retry the failed
//! ancillary steps, never blindly re-run a full destructive op.

use serde::{Deserialize, Serialize};

/// Machine keys for recovery CTAs. Keep stable; the frontend maps them to copy
/// and command arguments.
pub mod recovery {
    pub const RECORD_PROVENANCE: &str = "record_provenance";
    pub const CLEAR_PROVENANCE: &str = "clear_provenance";
    pub const CLEANUP_METADATA: &str = "cleanup_metadata";
    pub const PURGE_USER_DATA: &str = "purge_user_data";
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct StepOutcome {
    /// `"ok"` | `"failed"` | `"skipped"` | `"not_applicable"`
    pub state: String,
    pub detail: Option<String>,
}

impl StepOutcome {
    pub fn ok() -> Self {
        Self {
            state: "ok".to_string(),
            detail: None,
        }
    }

    pub fn ok_detail(detail: impl Into<String>) -> Self {
        Self {
            state: "ok".to_string(),
            detail: Some(detail.into()),
        }
    }

    pub fn failed(detail: impl Into<String>) -> Self {
        Self {
            state: "failed".to_string(),
            detail: Some(detail.into()),
        }
    }

    pub fn skipped(detail: impl Into<String>) -> Self {
        Self {
            state: "skipped".to_string(),
            detail: Some(detail.into()),
        }
    }

    pub fn not_applicable() -> Self {
        Self {
            state: "not_applicable".to_string(),
            detail: None,
        }
    }

    pub fn is_failed(&self) -> bool {
        self.state == "failed"
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct OperationOutcome {
    /// True when the primary op landed (app installed / removed as requested).
    pub primary_ok: bool,
    /// Disk truth after the op: `"present"` | `"absent"` | `"unknown"`.
    pub app_state: String,
    /// Classification when known: `"managed"` | `"external"` | `"none"`.
    pub install_class: Option<String>,
    /// Install path context for targeted recovery (e.g. clear a specific record).
    /// Prefer this over encoding paths into `warnings`.
    pub path: Option<String>,
    pub provenance: StepOutcome,
    pub cleanup: StepOutcome,
    pub warnings: Vec<String>,
    /// Stable recovery action keys (see [`recovery`]).
    pub recovery_actions: Vec<String>,
}

impl OperationOutcome {
    pub fn full_success(app_state: &str, install_class: Option<&str>) -> Self {
        Self {
            primary_ok: true,
            app_state: app_state.to_string(),
            install_class: install_class.map(str::to_string),
            path: None,
            provenance: StepOutcome::ok(),
            cleanup: StepOutcome::ok(),
            warnings: Vec::new(),
            recovery_actions: Vec::new(),
        }
    }

    pub fn primary_failed(app_state: &str, detail: impl Into<String>) -> Self {
        Self {
            primary_ok: false,
            app_state: app_state.to_string(),
            install_class: None,
            path: None,
            provenance: StepOutcome::not_applicable(),
            cleanup: StepOutcome::not_applicable(),
            warnings: vec![detail.into()],
            recovery_actions: Vec::new(),
        }
    }

    pub fn with_path(mut self, path: impl Into<String>) -> Self {
        self.path = Some(path.into());
        self
    }

    pub fn push_warning(&mut self, warning: impl Into<String>) {
        self.warnings.push(warning.into());
    }

    pub fn push_recovery(&mut self, action: &str) {
        if !self.recovery_actions.iter().any(|a| a == action) {
            self.recovery_actions.push(action.to_string());
        }
    }

    /// True when the primary op succeeded but at least one ancillary step failed.
    pub fn is_partial(&self) -> bool {
        self.primary_ok && (self.provenance.is_failed() || self.cleanup.is_failed())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AncillaryRetryRequest {
    /// Recovery action keys to run (subset of those reported by an outcome).
    pub actions: Vec<String>,
    /// Optional install path context (clear a specific provenance record, etc.).
    pub path: Option<String>,
    /// When retrying `purge_user_data`, whether to actually purge.
    #[serde(default)]
    pub purge_user_data: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub struct AncillaryRetryReport {
    pub outcome: OperationOutcome,
    pub message: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn partial_when_primary_ok_and_ancillary_failed() {
        let mut outcome = OperationOutcome::full_success("present", Some("external"));
        outcome.provenance = StepOutcome::failed("disk full");
        outcome.push_recovery(recovery::RECORD_PROVENANCE);
        assert!(outcome.is_partial());
        assert!(outcome.primary_ok);
        assert_eq!(
            outcome.recovery_actions,
            vec![recovery::RECORD_PROVENANCE.to_string()]
        );
    }

    #[test]
    fn not_partial_when_primary_failed() {
        let outcome = OperationOutcome::primary_failed("unknown", "network");
        assert!(!outcome.is_partial());
        assert!(!outcome.primary_ok);
    }

    #[test]
    fn recovery_actions_dedup() {
        let mut outcome = OperationOutcome::default();
        outcome.push_recovery(recovery::CLEANUP_METADATA);
        outcome.push_recovery(recovery::CLEANUP_METADATA);
        assert_eq!(outcome.recovery_actions.len(), 1);
    }
}
