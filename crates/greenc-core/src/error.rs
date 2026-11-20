use std::path::PathBuf;
use thiserror::Error;

pub type Result<T> = std::result::Result<T, GreenCError>;

#[derive(Error, Debug)]
pub enum GreenCError {
    #[error("Insufficient vault drive space: {0}")]
    InsufficientVaultSpace(String),

    #[error("File is locked by an active process: {path:?} ({reason})")]
    FileLocked { path: PathBuf, reason: String },

    #[error("Integrity mismatch for {path:?}: expected SHA-256 {expected}, calculated {actual}")]
    IntegrityMismatch {
        path: PathBuf,
        expected: String,
        actual: String,
    },

    #[error("Precondition blocked for {operation}: {reason}")]
    PreconditionBlocked { operation: String, reason: String },

    #[error("Task cancelled by user or cooperative yield")]
    Cancelled,

    #[error("Win32 error (code {code}): {message}")]
    Win32 { code: u32, message: String },

    #[error("Database error: {0}")]
    Database(String),

    #[error("SQLite error: {0}")]
    Sqlite(String),

    #[error("Vault error: {0}")]
    Vault(String),

    #[error("Relocation error: {0}")]
    Relocation(String),

    #[error("Scanner error: {0}")]
    Scanner(String),

    #[error("Localization error: {0}")]
    I18n(String),

    #[error("Entity not found: {entity} with ID {id}")]
    NotFound { entity: String, id: String },

    #[error("Invalid path: {0:?}")]
    InvalidPath(PathBuf),

    #[error("Configuration error: {0}")]
    ConfigError(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
}
