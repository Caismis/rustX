//! The bounded startup argument contract of the `rustx` binary.
//!
//! Arguments are explicit and required. This is deliberately not M10
//! configuration discovery: there is no search path, no precedence, no
//! profile selection, and no interactive editor.
//!
//! ```text
//! rustx --models <path> --config <rustx.json> --workspace <dir> --runtime-root <dir>
//! ```

use std::path::PathBuf;

use super::composition::LocalRuntimePaths;

/// The usage text printed to **stderr** for an argument failure.
pub const USAGE: &str = "usage: rustx --models <models.json> --config <rustx.json> \
                         --workspace <dir> --runtime-root <dir> [tool/skill options]";

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
    let mut config: Option<PathBuf> = None;
    let mut workspace: Option<PathBuf> = None;
    let mut runtime_root: Option<PathBuf> = None;
    let mut skill_paths = Vec::new();
    let mut no_skills = false;
    let mut no_builtin_tools = false;
    let mut no_tools = false;
    let mut tools = None;
    let mut exclude_tools = None;

    let mut arguments = arguments.into_iter();
    while let Some(flag) = arguments.next() {
        match flag.as_str() {
            "--models" | "--config" | "--workspace" | "--runtime-root" | "--skill" | "--tools"
            | "--exclude-tools" => {
                let Some(value) = arguments.next() else {
                    return Err(ArgumentError::MissingValue { flag });
                };
                match flag.as_str() {
                    "--models" => set_path(&mut models, value.as_str(), flag.as_str())?,
                    "--config" => set_path(&mut config, value.as_str(), flag.as_str())?,
                    "--workspace" => set_path(&mut workspace, value.as_str(), flag.as_str())?,
                    "--runtime-root" => set_path(&mut runtime_root, value.as_str(), flag.as_str())?,
                    "--skill" => skill_paths.push(PathBuf::from(value)),
                    "--tools" => set_names(&mut tools, value.as_str(), flag.as_str())?,
                    "--exclude-tools" => {
                        set_names(&mut exclude_tools, value.as_str(), flag.as_str())?;
                    }
                    _ => unreachable!(),
                }
            }
            "--no-skills" => set_bool(&mut no_skills, flag.as_str())?,
            "--no-builtin-tools" => set_bool(&mut no_builtin_tools, flag.as_str())?,
            "--no-tools" => set_bool(&mut no_tools, flag.as_str())?,
            other => {
                return Err(ArgumentError::UnknownFlag {
                    flag: other.to_owned(),
                });
            }
        }
    }

    Ok(LocalRuntimePaths {
        models: required(models, "--models")?,
        config: required(config, "--config")?,
        skill_paths,
        no_skills,
        no_builtin_tools,
        no_tools,
        tools,
        exclude_tools: exclude_tools.unwrap_or_default(),
        workspace: required(workspace, "--workspace")?,
        runtime_root: required(runtime_root, "--runtime-root")?,
    })
}

fn set_path(slot: &mut Option<PathBuf>, value: &str, flag: &str) -> Result<(), ArgumentError> {
    if slot.is_some() {
        return Err(ArgumentError::Repeated {
            flag: flag.to_owned(),
        });
    }
    *slot = Some(PathBuf::from(value));
    Ok(())
}

fn set_names(slot: &mut Option<Vec<String>>, value: &str, flag: &str) -> Result<(), ArgumentError> {
    if slot.is_some() {
        return Err(ArgumentError::Repeated {
            flag: flag.to_owned(),
        });
    }
    let names = value
        .split(',')
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_owned)
        .collect::<Vec<_>>();
    if names.is_empty() {
        return Err(ArgumentError::InvalidValue {
            flag: flag.to_owned(),
        });
    }
    *slot = Some(names);
    Ok(())
}

fn set_bool(slot: &mut bool, flag: &str) -> Result<(), ArgumentError> {
    if *slot {
        return Err(ArgumentError::Repeated {
            flag: flag.to_owned(),
        });
    }
    *slot = true;
    Ok(())
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
    /// A value was syntactically present but empty or otherwise unusable.
    InvalidValue {
        /// The flag whose value was invalid.
        flag: String,
    },
}

impl std::fmt::Display for ArgumentError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownFlag { flag } => write!(f, "unknown argument {flag:?}"),
            Self::MissingValue { flag } => write!(f, "argument {flag} requires a value"),
            Self::Repeated { flag } => write!(f, "argument {flag} was supplied more than once"),
            Self::Missing { flag } => write!(f, "missing required argument {flag}"),
            Self::InvalidValue { flag } => write!(f, "argument {flag} requires a non-empty value"),
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
            "--config",
            "/r.json",
            "--workspace",
            "/ws",
            "--runtime-root",
            "/private",
        ]))
        .expect("valid");
        assert_eq!(paths.models.to_str(), Some("/m.json"));
        assert_eq!(paths.config.to_str(), Some("/r.json"));
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
        assert!(matches!(
            parse_arguments(args(&["--session", "old.json"])).expect_err("obsolete"),
            ArgumentError::UnknownFlag { .. }
        ));
    }

    #[test]
    fn tool_and_skill_startup_options_are_typed_and_repeatable_only_where_allowed() {
        let paths = parse_arguments(args(&[
            "--models",
            "m",
            "--config",
            "r",
            "--workspace",
            "w",
            "--runtime-root",
            "p",
            "--no-skills",
            "--skill",
            "one",
            "--skill",
            "two",
            "--no-builtin-tools",
            "--tools",
            "read, search",
            "--exclude-tools",
            "bash,grep",
        ]))
        .expect("options");
        assert!(paths.no_skills);
        assert_eq!(
            paths.skill_paths,
            vec![
                std::path::PathBuf::from("one"),
                std::path::PathBuf::from("two")
            ]
        );
        assert_eq!(
            paths.tools,
            Some(vec!["read".to_owned(), "search".to_owned()])
        );
        assert_eq!(
            paths.exclude_tools,
            vec!["bash".to_owned(), "grep".to_owned()]
        );
        assert!(paths.no_builtin_tools);
    }
}
