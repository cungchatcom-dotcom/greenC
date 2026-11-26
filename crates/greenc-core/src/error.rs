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

impl GreenCError {
    /// Returns a privacy-safe diagnostic code identifying the error variant
    /// (plus numeric Win32 code for `Win32`), never including paths or messages.
    pub fn diagnostic_code(&self) -> String {
        match self {
            Self::InsufficientVaultSpace(_) => "InsufficientVaultSpace".to_string(),
            Self::FileLocked { .. } => "FileLocked".to_string(),
            Self::IntegrityMismatch { .. } => "IntegrityMismatch".to_string(),
            Self::PreconditionBlocked { .. } => "PreconditionBlocked".to_string(),
            Self::Cancelled => "Cancelled".to_string(),
            Self::Win32 { code, .. } => format!("Win32:{code}"),
            Self::Database(_) => "Database".to_string(),
            Self::Sqlite(_) => "Sqlite".to_string(),
            Self::Vault(_) => "Vault".to_string(),
            Self::Relocation(_) => "Relocation".to_string(),
            Self::Scanner(_) => "Scanner".to_string(),
            Self::I18n(_) => "I18n".to_string(),
            Self::NotFound { .. } => "NotFound".to_string(),
            Self::InvalidPath(_) => "InvalidPath".to_string(),
            Self::ConfigError(_) => "ConfigError".to_string(),
            Self::Io(_) => "Io".to_string(),
            Self::Json(_) => "Json".to_string(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostic_code_maps_every_variant_without_leaking_paths_or_messages() {
        let secret_path = PathBuf::from("C:\\Users\\Alice\\Documents\\secret.docx");
        let secret_msg = "C:\\Users\\Alice\\private\\token=12345";

        let cases: Vec<(GreenCError, &str)> = vec![
            (
                GreenCError::InsufficientVaultSpace(secret_msg.into()),
                "InsufficientVaultSpace",
            ),
            (
                GreenCError::FileLocked {
                    path: secret_path.clone(),
                    reason: secret_msg.into(),
                },
                "FileLocked",
            ),
            (
                GreenCError::IntegrityMismatch {
                    path: secret_path.clone(),
                    expected: "aaa".into(),
                    actual: "bbb".into(),
                },
                "IntegrityMismatch",
            ),
            (
                GreenCError::PreconditionBlocked {
                    operation: "relocate".into(),
                    reason: secret_msg.into(),
                },
                "PreconditionBlocked",
            ),
            (GreenCError::Cancelled, "Cancelled"),
            (
                GreenCError::Win32 {
                    code: 5,
                    message: secret_msg.into(),
                },
                "Win32:5",
            ),
            (
                GreenCError::Win32 {
                    code: 1234567890,
                    message: secret_msg.into(),
                },
                "Win32:1234567890",
            ),
            (GreenCError::Database(secret_msg.into()), "Database"),
            (GreenCError::Sqlite(secret_msg.into()), "Sqlite"),
            (GreenCError::Vault(secret_msg.into()), "Vault"),
            (GreenCError::Relocation(secret_msg.into()), "Relocation"),
            (GreenCError::Scanner(secret_msg.into()), "Scanner"),
            (GreenCError::I18n(secret_msg.into()), "I18n"),
            (
                GreenCError::NotFound {
                    entity: "vault".into(),
                    id: secret_msg.into(),
                },
                "NotFound",
            ),
            (GreenCError::InvalidPath(secret_path), "InvalidPath"),
            (GreenCError::ConfigError(secret_msg.into()), "ConfigError"),
            (
                GreenCError::Io(std::io::Error::new(
                    std::io::ErrorKind::PermissionDenied,
                    secret_msg,
                )),
                "Io",
            ),
            (
                GreenCError::Json(serde_json::from_str::<serde_json::Value>("not-json").unwrap_err()),
                "Json",
            ),
        ];

        for (err, expected) in cases {
            let code = err.diagnostic_code();
            assert_eq!(code, expected);
            assert!(!code.contains("Alice"));
            assert!(!code.contains('\\'));
            assert!(!code.contains('/'));
            assert!(!code.contains("secret"));
        }
    }
}
