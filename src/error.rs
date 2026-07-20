use std::path::PathBuf;

use thiserror::Error;

#[allow(dead_code)]
#[derive(Debug, Error)]
pub enum AppError {
    #[error("configuration error: {0}")]
    Config(String),
    #[error("missing required environment variable: {0}")]
    MissingEnv(&'static str),
    #[error("invalid source file {path}: {message}")]
    InvalidSourceFile { path: PathBuf, message: String },
    #[error("mdbtools command unavailable: {0}")]
    MdbToolsUnavailable(String),
    #[error("MDB schema error: {0}")]
    MdbSchema(String),
    #[error("MDB export error: {0}")]
    MdbExport(String),
    #[error("CSV parsing error: {0}")]
    Csv(#[from] csv::Error),
    #[error("validation failed: {0} blocking error(s)")]
    Validation(usize),
    #[error("authentication error: {0}")]
    Auth(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("DIGIweb processing error: {0}")]
    DigiwebProcessing(String),
    #[error("polling timed out after {0} seconds")]
    PollingTimeout(u64),
    #[error("logging error: {0}")]
    Logging(String),
    #[error("internal error: {0}")]
    Internal(String),
}

impl AppError {
    pub fn stage(&self) -> &'static str {
        match self {
            Self::Config(_) => "configuration",
            Self::MissingEnv(_) => "environment",
            Self::InvalidSourceFile { .. } => "source file verification",
            Self::MdbToolsUnavailable(_) => "mdbtools verification",
            Self::MdbSchema(_) => "MDB schema inspection",
            Self::MdbExport(_) | Self::Csv(_) => "MDB export parsing",
            Self::Validation(_) => "validation",
            Self::Auth(_) => "DIGIweb authentication",
            Self::Network(_) | Self::Http(_) => "DIGIweb connection",
            Self::DigiwebProcessing(_) | Self::PollingTimeout(_) => "DIGIweb processing",
            Self::Logging(_) => "logging",
            Self::Internal(_) => "internal",
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::Auth(_)
            | Self::Network(_)
            | Self::Http(_)
            | Self::DigiwebProcessing(_)
            | Self::PollingTimeout(_) => 3,
            Self::Internal(_) | Self::Logging(_) => 4,
            _ => 2,
        }
    }
}

impl From<std::io::Error> for AppError {
    fn from(value: std::io::Error) -> Self {
        Self::Internal(value.to_string())
    }
}
