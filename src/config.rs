//! Application configuration boundary.
//!
//! Responsible for loading, validating, and persisting user preferences
//! and platform-appropriate defaults.

use thiserror::Error;

/// Top-level application configuration.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AppConfig {}

/// Errors produced when loading or validating configuration.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum ConfigError {
    /// Configuration contains an invalid value or structure.
    #[error("invalid configuration: {0}")]
    Invalid(String),

    /// Underlying I/O error while reading or writing configuration.
    #[error("configuration I/O error: {0}")]
    Io(#[from] std::io::Error),
}
