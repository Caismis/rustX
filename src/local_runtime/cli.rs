//! The bounded startup argument contract of the `rustx` binary.
//!
//! Arguments are explicit and required. This is deliberately not M10
//! configuration discovery: there is no search path, no precedence, no
//! profile selection, and no interactive editor.
//!
//! ```text
//! rustx --models <path> --session <path> --workspace <dir> --runtime-root <dir>
//! ```

use std::path::PathBuf;

use super::composition::LocalRuntimePaths;

/// The usage text printed to **stderr** for an argument failure.
pub const USAGE: &str = "usage: rustx --models <models.json> --session <session.json> \
                         --workspace <dir> --runtime-root <dir>";

/// Parses the bounded startup arguments.
///
/// # Errors
///
/// Returns a bounded diagnostic for an unknown flag, a missing value, or a
/// missing required path.
pub fn parse_arguments(
    arguments: impl IntoIterator<Item = String>,
) -> Result<LocalRuntimePaths, ArgumentError> {
    let mut models: Option<PathBuf> = None;
    let mut session: Option<PathBuf> = None;
    let mut workspace: Option<PathBuf> = None;
    let mut runtime_root: Option<PathBuf> = None;

    let mut arguments = arguments.into_iter();
    while let Some(flag) = arguments.next() {
        let slot = match flag.as_str() {
            "--models" => &mut models,
            "--session" => &mut session,
            "--workspace" => &mut workspace,
            "--runtime-root" => &mut runtime_root,
            other => {
                return Err(ArgumentError::UnknownFlag {
                    flag: other.to_owned(),
                });
            }
        };
        let Some(value) = arguments.next() else {
            return Err(ArgumentError::MissingValue { flag });
        };
        if slot.is_some() {
            return Err(ArgumentError::Repeated { flag });
        }
        *slot = Some(PathBuf::from(value));
    }

    Ok(LocalRuntimePaths {
        models: required(models, "--models")?,
        session: required(session, "--session")?,
        workspace: required(workspace, "--workspace")?,
        runtime_root: required(runtime_root, "--runtime-root")?,
    })
}

fn required(value: Option<PathBuf>, flag: &'static str) -> Result<PathBuf, ArgumentError> {
    value.ok_or(ArgumentError::Missing { flag })
}

/// A bounded startup argument failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ArgumentError {
    /// An unrecognized flag was supplied.
    UnknownFlag {
        /// The offending flag.
        flag: String,
    },
    /// A flag was supplied without its value.
    MissingValue {
        /// The flag whose value is missing.
        flag: String,
    },
    /// A flag was supplied more than once.
    Repeated {
        /// The repeated flag.
        flag: String,
    },
    /// A required flag was not supplied.
    Missing {
        /// The missing flag.
        flag: &'static str,
    },
}

impl std::fmt::Display for ArgumentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownFlag { flag } => write!(f, "unknown argument {flag:?}"),
            Self::MissingValue { flag } => write!(f, "argument {flag} requires a value"),
            Self::Repeated { flag } => write!(f, "argument {flag} was supplied more than once"),
            Self::Missing { flag } => write!(f, "missing required argument {flag}"),
        }
    }
}

impl std::error::Error for ArgumentError {}

#[cfg(test)]
mod tests {
    use super::{ArgumentError, parse_arguments};

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    /// The complete argument set parses into explicit paths.
    #[test]
    fn complete_arguments_parse() {
        let paths = parse_arguments(args(&[
            "--models",
            "/m.json",
            "--session",
            "/s.json",
            "--workspace",
            "/ws",
            "--runtime-root",
            "/private",
        ]))
        .expect("valid");
        assert_eq!(paths.models.to_str(), Some("/m.json"));
        assert_eq!(paths.artifacts_root().to_str(), Some("/private/artifacts"));
        assert_eq!(
            paths.environment_store_root().to_str(),
            Some("/private/environments")
        );
    }

    /// Unknown, repeated, valueless, and missing arguments all fail
    /// explicitly.
    #[test]
    fn malformed_arguments_fail() {
        assert!(matches!(
            parse_arguments(args(&["--future"])).expect_err("unknown"),
            ArgumentError::UnknownFlag { .. }
        ));
        assert!(matches!(
            parse_arguments(args(&["--models"])).expect_err("no value"),
            ArgumentError::MissingValue { .. }
        ));
        assert!(matches!(
            parse_arguments(args(&["--models", "a", "--models", "b"])).expect_err("repeated"),
            ArgumentError::Repeated { .. }
        ));
        assert!(matches!(
            parse_arguments(args(&["--models", "a"])).expect_err("incomplete"),
            ArgumentError::Missing { .. }
        ));
    }
}
