//! Errors as Swift sees them: a typed enum, never a panic.

use metla_core::engine::EngineError;
use metla_core::index::IndexError;

#[derive(Clone, Debug, PartialEq, Eq, uniffi::Error)]
pub enum MetlaError {
    /// The root or the data directory cannot be used.
    Setup { reason: String },
    /// The disk has no room for the index.
    NoSpace,
    /// The index cannot be read or written.
    Index { reason: String },
    /// A cleanup cannot run (for example: not a fixture).
    Clean { reason: String },
    /// The picture has no such directory.
    NotInPicture,
    /// The engine has stopped.
    Stopped,
    /// Not an absolute path without `.` or `..`.
    InvalidPath { path: String },
}

impl From<EngineError> for MetlaError {
    fn from(error: EngineError) -> MetlaError {
        match error {
            EngineError::Setup(reason) => MetlaError::Setup { reason },
            EngineError::Index(IndexError::NoSpace) => MetlaError::NoSpace,
            EngineError::Index(error) => MetlaError::Index {
                reason: error.to_string(),
            },
            EngineError::Clean(error) => MetlaError::Clean {
                reason: error.to_string(),
            },
            EngineError::NotInPicture => MetlaError::NotInPicture,
            EngineError::Stopped => MetlaError::Stopped,
        }
    }
}

impl std::fmt::Display for MetlaError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
    }
}

impl std::error::Error for MetlaError {}
