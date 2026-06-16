use serde::Serialize;
use thiserror::Error;

use crate::app::oplock::OperationError;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("the current platform is not supported yet")]
    UnsupportedPlatform,
    #[error("update engine error: {0}")]
    Engine(String),
    /// Reality (installed bundle / feed target) no longer matches the snapshot
    /// the user confirmed — the TOCTOU guard before a destructive step. The
    /// message is already user-facing; the UI reacts to the code by silently
    /// re-checking and asking the user to confirm the fresh plan.
    #[error("{0}")]
    StaleExpectation(String),
    #[error("{0}")]
    Busy(String),
    #[error("{0}")]
    Internal(String),
}

impl AppError {
    fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedPlatform => "unsupported_platform",
            Self::Engine(_) => "engine_error",
            Self::StaleExpectation(_) => "stale_expectation",
            Self::Busy(_) => "operation_busy",
            Self::Internal(_) => "internal_error",
        }
    }
}

impl From<OperationError> for AppError {
    fn from(value: OperationError) -> Self {
        Self::Busy(value.to_string())
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CommandError {
    pub code: String,
    pub message: String,
}

impl From<AppError> for CommandError {
    fn from(value: AppError) -> Self {
        Self {
            code: value.code().to_string(),
            message: value.to_string(),
        }
    }
}
