//! Managed-Python package contracts shared with capability owners.

/// A package discovery/preparation failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PythonToolError {
    /// The package is malformed.
    #[error("invalid Python tool package: {0}")]
    InvalidPackage(String),
    /// Prepared state could not be read or published.
    #[error("Python tool storage failed: {0}")]
    Storage(String),
    /// The dependency environment could not be checked or materialized.
    #[error("Python tool environment failed: {0}")]
    Environment(String),
}

/// Validates one canonical Managed Python package identifier.
pub fn validate_identifier(identifier: &str) -> Result<(), PythonToolError> {
    if identifier.is_empty()
        || identifier.starts_with('-')
        || identifier.ends_with('-')
        || identifier.contains("--")
        || !identifier.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
    {
        return Err(PythonToolError::InvalidPackage(format!(
            "invalid package name {identifier:?}"
        )));
    }
    Ok(())
}
