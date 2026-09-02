//! Shared error type for the core engine.

use thiserror::Error;

/// Errors produced by the core engine.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("database error: {0}")]
    Db(#[from] rusqlite::Error),

    #[error("image error: {0}")]
    Image(#[from] image::ImageError),

    #[error("walk error: {0}")]
    Walk(#[from] walkdir::Error),

    #[error("serialization error: {0}")]
    Serde(#[from] serde_json::Error),

    #[error("{0}")]
    Other(String),
}

pub type CoreResult<T> = Result<T, CoreError>;

impl CoreError {
    /// Convenience constructor for ad-hoc errors.
    pub fn other<S: Into<String>>(msg: S) -> Self {
        CoreError::Other(msg.into())
    }
}
