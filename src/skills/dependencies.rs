//! rustX Skill dependency declarations (M6).
//!
//! M6 uses the Agent Skills standard `metadata` extension point with
//! exactly two rustX keys:
//!
//! ```yaml
//! metadata:
//!   rustx.python-dependencies: '{"pypdf":"5.9.0","pillow":"11.3.0"}'
//!   rustx.node-dependencies: '{"pdf-lib":"1.17.1","@scope/pkg":"2.0.0"}'
//! ```
//!
//! The metadata values are strings as required by the Agent Skills format;
//! each value is a JSON object mapping package name to an exact version
//! string. M6 supports direct exact versions only:
//!
//! - Python: `package == exact version`; distribution names are normalized
//!   deterministically (lowercase, `-`/`_`/`.` equivalence) before merging;
//! - Node: `package = exact version`, including scoped names such as
//!   `@scope/package`.
//!
//! M6 rejects version ranges, tags (`latest`, ...), extras, environment
//! markers, URLs, VCS dependencies, local path dependencies, editable
//! installs, and workspace references. rustX never builds a general semver
//! solver.
//!
//! # Merge/conflict semantics
//!
//! Across every active Skill, the same normalized package with the same
//! exact version coalesces; the same package with different exact versions
//! is a deterministic [`DependencyConflict`] reported before any
//! package-manager subprocess runs.

use std::collections::BTreeMap;

use crate::runtime::identity::SkillId;
use crate::skills::package::SkillPackage;

/// The rustX metadata key declaring Python dependencies.
pub const PYTHON_DEPENDENCIES_METADATA_KEY: &str = "rustx.python-dependencies";
/// The rustX metadata key declaring Node dependencies.
pub const NODE_DEPENDENCIES_METADATA_KEY: &str = "rustx.node-dependencies";

/// The dependency ecosystem.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Ecosystem {
    /// The Python ecosystem (`pip`/`PyPI`).
    Python,
    /// The Node ecosystem (`npm`).
    Node,
}

/// A parsed rustX dependency declaration failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyError {
    /// The metadata value is not a JSON object.
    NotAnObject(String),
    /// The package name is malformed for the ecosystem.
    InvalidPackageName { ecosystem: Ecosystem, name: String },
    /// The declared version is not a direct exact version.
    InvalidVersion {
        ecosystem: Ecosystem,
        package: String,
        version: String,
    },
}

impl core::fmt::Display for DependencyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::NotAnObject(value) => write!(
                f,
                "a rustX dependency declaration must be a JSON object of \
                 package-name to exact-version-string, got: {value:?}"
            ),
            Self::InvalidPackageName { ecosystem, name } => write!(
                f,
                "{ecosystem}: malformed package name {name:?} in a rustX dependency \
                 declaration"
            ),
            Self::InvalidVersion {
                ecosystem,
                package,
                version,
            } => write!(
                f,
                "{ecosystem}: package {package:?} declares unsupported version {version:?}: \
                 M6 supports direct exact versions only (no ranges, tags, extras, markers, \
                 URLs, VCS, local paths, editable installs, or workspace references)"
            ),
        }
    }
}

impl std::error::Error for DependencyError {}

/// A deterministic direct-declaration conflict across active Skills.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DependencyConflict {
    /// The ecosystem of the conflicting package.
    pub ecosystem: Ecosystem,
    /// The normalized package name.
    pub package: String,
    /// Every responsible Skill and its declared version, deterministically
    /// ordered by Skill name.
    pub declarations: Vec<(SkillId, String)>,
}

impl core::fmt::Display for DependencyConflict {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(
            f,
            "{} dependency conflict for package {:?}:",
            self.ecosystem, self.package
        )?;
        for (skill, version) in &self.declarations {
            write!(f, " skill {skill:?} declares {version:?};")?;
        }
        Ok(())
    }
}

impl std::error::Error for DependencyConflict {}

/// The merged, normalized dependency declarations of one Skill set.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct DependencyManifest {
    /// Sorted normalized package name -> exact version.
    pub python: BTreeMap<String, String>,
    /// Sorted package name -> exact version.
    pub node: BTreeMap<String, String>,
}

impl DependencyManifest {
    /// Whether any dependency is declared.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.python.is_empty() && self.node.is_empty()
    }

    /// Whether the ecosystem has declared dependencies.
    #[must_use]
    pub fn has_ecosystem(&self, ecosystem: Ecosystem) -> bool {
        match ecosystem {
            Ecosystem::Python => !self.python.is_empty(),
            Ecosystem::Node => !self.node.is_empty(),
        }
    }
}

impl core::fmt::Display for Ecosystem {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Python => write!(f, "python"),
            Self::Node => write!(f, "node"),
        }
    }
}

/// Parses the rustX dependency declaration keys of one Skill's metadata.
///
/// # Errors
///
/// Returns [`DependencyError`] for a malformed or unsupported declaration.
pub fn parse_dependency_map(
    metadata: &BTreeMap<String, String>,
) -> Result<DependencyManifest, DependencyError> {
    let python = match metadata.get(PYTHON_DEPENDENCIES_METADATA_KEY) {
        Some(value) => parse_python_dependencies(value)?,
        None => BTreeMap::new(),
    };
    let node = match metadata.get(NODE_DEPENDENCIES_METADATA_KEY) {
        Some(value) => parse_node_dependencies(value)?,
        None => BTreeMap::new(),
    };
    Ok(DependencyManifest { python, node })
}

/// Parses one `rustx.python-dependencies` metadata value.
///
/// # Errors
///
/// Returns [`DependencyError`] for a malformed declaration.
pub fn parse_python_dependencies(value: &str) -> Result<BTreeMap<String, String>, DependencyError> {
    parse_json_object(value, Ecosystem::Python, normalize_python_package)
}

/// Parses one `rustx.node-dependencies` metadata value.
///
/// # Errors
///
/// Returns [`DependencyError`] for a malformed declaration.
pub fn parse_node_dependencies(value: &str) -> Result<BTreeMap<String, String>, DependencyError> {
    parse_json_object(value, Ecosystem::Node, validate_node_package)
}

/// Parses a JSON object of package name -> exact version string for one
/// ecosystem, normalizing/validating every key and validating every value.
fn parse_json_object(
    value: &str,
    ecosystem: Ecosystem,
    normalize: fn(&str, Ecosystem) -> Result<String, String>,
) -> Result<BTreeMap<String, String>, DependencyError> {
    let object: serde_json::Value =
        serde_json::from_str(value).map_err(|_| DependencyError::NotAnObject(value.to_owned()))?;
    let serde_json::Value::Object(map) = object else {
        return Err(DependencyError::NotAnObject(value.to_owned()));
    };
    let mut normalized = BTreeMap::new();
    for (package, version) in map {
        let package =
            normalize(&package, ecosystem).map_err(|_| DependencyError::InvalidPackageName {
                ecosystem,
                name: package.clone(),
            })?;
        let serde_json::Value::String(version) = version else {
            return Err(DependencyError::InvalidVersion {
                ecosystem,
                package,
                version: version.to_string(),
            });
        };
        validate_exact_version(&version, ecosystem).map_err(|()| {
            DependencyError::InvalidVersion {
                ecosystem,
                package: package.clone(),
                version: version.clone(),
            }
        })?;
        normalized.insert(package, version);
    }
    Ok(normalized)
}

/// Normalizes a Python distribution name deterministically: lowercase,
/// with `-`, `_`, and `.` treated as equivalent separators (collapsing
/// runs), per PEP 503.
fn normalize_python_package(name: &str, ecosystem: Ecosystem) -> Result<String, String> {
    if name.is_empty()
        || name.chars().any(|character| {
            !(character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
        })
    {
        return Err(format!("malformed package name {name:?}"));
    }
    let mut normalized = String::with_capacity(name.len());
    let mut last_separator = false;
    for character in name.to_ascii_lowercase().chars() {
        if matches!(character, '-' | '_' | '.') {
            if !last_separator && !normalized.is_empty() {
                normalized.push('-');
            }
            last_separator = true;
        } else {
            normalized.push(character);
            last_separator = false;
        }
    }
    if normalized.is_empty()
        || normalized.starts_with('-')
        || normalized.ends_with('-')
        || !normalized.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        })
    {
        return Err(format!("malformed package name {name:?}"));
    }
    let _ = ecosystem;
    Ok(normalized)
}

/// Validates a Node package name, including scoped names such as
/// `@scope/package`.
fn validate_node_package(name: &str, _ecosystem: Ecosystem) -> Result<String, String> {
    let scoped = name.strip_prefix('@');
    let (scope, rest) = match scoped {
        Some(remainder) => {
            let Some((scope, rest)) = remainder.split_once('/') else {
                return Err(format!("malformed scoped package name {name:?}"));
            };
            (Some(scope), rest)
        }
        None => (None, name),
    };
    let valid_segment = |segment: &str| {
        !segment.is_empty()
            && segment.chars().all(|character| {
                character.is_ascii_lowercase()
                    || character.is_ascii_digit()
                    || matches!(character, '.' | '_' | '-' | '~')
            })
    };
    if !valid_segment(rest) || scope.is_some_and(|scope| !valid_segment(scope)) {
        return Err(format!("malformed package name {name:?}"));
    }
    Ok(name.to_owned())
}

/// Validates one direct exact version declaration for the ecosystem.
///
/// M6 supports direct exact versions only. Ranges, tags such as `latest`,
/// URLs, VCS dependencies, local paths, and workspace references are
/// rejected.
fn validate_exact_version(version: &str, ecosystem: Ecosystem) -> Result<(), ()> {
    if version.is_empty() || version.chars().any(char::is_whitespace) {
        return Err(());
    }
    match ecosystem {
        Ecosystem::Python => validate_python_exact_version(version),
        Ecosystem::Node => validate_node_exact_version(version),
    }
}

/// PEP 440-shaped direct exact Python version: digits, letters, `.`, `+`
/// (local version), `!` (epoch) only; no operators, no markers, no extras.
fn validate_python_exact_version(version: &str) -> Result<(), ()> {
    if version.chars().any(|character| {
        !(character.is_ascii_alphanumeric() || matches!(character, '.' | '+' | '!'))
    }) || !version.chars().any(|character| character.is_ascii_digit())
    {
        return Err(());
    }
    Ok(())
}

/// npm-shaped direct exact Node version: digits, letters, `.`, `-`, `+`
/// only; must start with a digit (rejecting tags such as `latest`); no
/// range wildcards (`*`, `x`, `X`); no `:`, `/`, `#`, `?`, `&` (URLs,
/// git, workspace references).
fn validate_node_exact_version(version: &str) -> Result<(), ()> {
    if version.chars().any(|character| {
        !(character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '+'))
    }) || !version
        .chars()
        .next()
        .is_some_and(|character| character.is_ascii_digit())
        || version
            .chars()
            .any(|character| matches!(character, 'x' | 'X' | '*'))
    {
        return Err(());
    }
    Ok(())
}

/// Merges the dependency declarations of every active Skill.
///
/// Across every active Skill, the same normalized package with the same
/// exact version coalesces; the same package with different exact versions
/// is a deterministic [`DependencyConflict`] naming the ecosystem, the
/// normalized package, every responsible Skill, and every declared
/// version. The check runs before any package-manager subprocess.
///
/// # Errors
///
/// Returns [`DependencyConflict`] on the first deterministic conflict
/// (deterministically ordered by package, then by Skill name).
pub fn merge_dependency_manifests(
    packages: &[SkillPackage],
) -> Result<DependencyManifest, DependencyConflict> {
    let mut merged = DependencyManifest::default();
    let mut candidates: Vec<(&SkillPackage, Ecosystem, &BTreeMap<String, String>)> = Vec::new();
    for package in packages {
        candidates.push((package, Ecosystem::Python, &package.dependencies().python));
        candidates.push((package, Ecosystem::Node, &package.dependencies().node));
    }
    for (package, ecosystem, map) in candidates {
        let _ = package;
        for (name, version) in map {
            let slot = match ecosystem {
                Ecosystem::Python => &mut merged.python,
                Ecosystem::Node => &mut merged.node,
            };
            match slot.get(name) {
                Some(existing) if existing == version => {}
                Some(_) => {
                    let mut declarations = packages
                        .iter()
                        .filter_map(|candidate| {
                            let candidate_map = match ecosystem {
                                Ecosystem::Python => &candidate.dependencies().python,
                                Ecosystem::Node => &candidate.dependencies().node,
                            };
                            candidate_map
                                .get(name)
                                .map(|declared| (candidate.id().clone(), declared.clone()))
                        })
                        .collect::<Vec<_>>();
                    declarations.sort_by(|left, right| left.0.cmp(&right.0));
                    return Err(DependencyConflict {
                        ecosystem,
                        package: name.clone(),
                        declarations,
                    });
                }
                None => {
                    slot.insert(name.clone(), version.clone());
                }
            }
        }
    }
    Ok(merged)
}

#[cfg(test)]
mod tests {
    use super::{DependencyError, Ecosystem, merge_dependency_manifests, normalize_python_package};
    use crate::runtime::identity::SkillId;
    use crate::skills::dependencies::{parse_node_dependencies, parse_python_dependencies};

    /// Python distribution names normalize deterministically with
    /// `-`/`_`/`.` equivalence and lowercasing.
    #[test]
    fn python_names_normalize_deterministically() {
        let cases = [
            ("pypdf", "pypdf"),
            ("PyPDF", "pypdf"),
            ("Pillow", "pillow"),
            ("python_ldap", "python-ldap"),
            ("zope.interface", "zope-interface"),
            ("foo..bar", "foo-bar"),
            ("foo_-bar", "foo-bar"),
        ];
        for (input, expected) in cases {
            assert_eq!(
                normalize_python_package(input, Ecosystem::Python).expect("normalize"),
                expected
            );
        }
    }

    /// Different JSON key ordering produces the same canonical set.
    #[test]
    fn json_key_order_does_not_matter() {
        let first = parse_python_dependencies(r#"{"a":"1.0","b":"2.0"}"#).expect("parse");
        let second = parse_python_dependencies(r#"{"b":"2.0","a":"1.0"}"#).expect("parse");
        assert_eq!(first, second);
        assert_eq!(
            first,
            [
                ("a".to_owned(), "1.0".to_owned()),
                ("b".to_owned(), "2.0".to_owned()),
            ]
            .into_iter()
            .collect()
        );
    }

    /// Scoped Node package names parse.
    #[test]
    fn scoped_node_packages_parse() {
        let map =
            parse_node_dependencies(r#"{"pdf-lib":"1.17.1","@scope/pkg":"2.0.0"}"#).expect("parse");
        assert_eq!(map.get("@scope/pkg").expect("scoped"), "2.0.0");
        assert_eq!(map.get("pdf-lib").expect("plain"), "1.17.1");
    }

    /// Unsupported M6 declarations are rejected for both ecosystems.
    #[test]
    fn unsupported_declarations_are_rejected() {
        for bad_python in [
            ">=1.0",
            "~=1.0",
            "==1.0",
            "1.0.*",
            "1.0; python_version<'3.9'",
            "1.0[extra]",
            "git+https://example.com/repo.git",
            "https://example.com/pkg-1.0.tar.gz",
            "../local/path",
            "-e .",
            "latest",
        ] {
            let json = format!(r#"{{"pkg":"{bad_python}"}}"#);
            let result = parse_python_dependencies(&json);
            assert!(
                matches!(result, Err(DependencyError::InvalidVersion { .. })),
                "python {bad_python:?} must be rejected, got {result:?}"
            );
        }
        for bad_node in [
            "^1.0.0",
            "~1.0.0",
            ">=1.0.0",
            "1.0.x",
            "*",
            "latest",
            "1.0.0 || 2.0.0",
            "git+https://example.com/repo.git",
            "https://example.com/pkg.tgz",
            "file:../pkg",
            "workspace:*",
            "1.0.0-beta.1",
        ] {
            // 1.0.0-beta.1 is a valid prerelease exact version.
            if bad_node == "1.0.0-beta.1" {
                assert!(parse_node_dependencies(&format!(r#"{{"pkg":"{bad_node}"}}"#)).is_ok());
                continue;
            }
            let json = format!(r#"{{"pkg":"{bad_node}"}}"#);
            let result = parse_node_dependencies(&json);
            assert!(
                matches!(result, Err(DependencyError::InvalidVersion { .. })),
                "node {bad_node:?} must be rejected, got {result:?}"
            );
        }
    }

    /// Duplicate compatible declarations coalesce; incompatible ones
    /// report every responsible Skill.
    #[test]
    fn merge_coalesces_and_reports_conflicts() {
        let packages = vec![
            package("skill-a", r#"{"pypdf":"5.9.0"}"#, r#"{"pdf-lib":"1.17.1"}"#),
            package("skill-b", r#"{"PyPDF":"5.9.0"}"#, r#"{"pdf-lib":"1.17.1"}"#),
        ];
        let merged = merge_dependency_manifests(&packages).expect("merge");
        assert_eq!(merged.python.len(), 1);
        assert_eq!(merged.python.get("pypdf").expect("coalesced"), "5.9.0");
        assert_eq!(merged.node.len(), 1);
        assert_eq!(merged.node.get("pdf-lib").expect("coalesced"), "1.17.1");

        let packages = vec![
            package("skill-a", r#"{"pypdf":"5.9.0"}"#, r"{}"),
            package("skill-b", r#"{"pypdf":"5.10.0"}"#, r"{}"),
        ];
        let conflict = merge_dependency_manifests(&packages).expect_err("conflict");
        assert_eq!(conflict.ecosystem, Ecosystem::Python);
        assert_eq!(conflict.package, "pypdf");
        assert_eq!(
            conflict.declarations,
            vec![
                (SkillId::new("skill-a"), "5.9.0".to_owned()),
                (SkillId::new("skill-b"), "5.10.0".to_owned()),
            ]
        );
    }

    fn package(name: &str, python: &str, node: &str) -> crate::skills::package::SkillPackage {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().join(".agents").join("skills").join(name);
        std::fs::create_dir_all(&root).expect("create");
        std::fs::write(
            root.join("SKILL.md"),
            format!(
                "---\nname: {name}\ndescription: a skill\nmetadata:\n  \
                 rustx.python-dependencies: '{python}'\n  \
                 rustx.node-dependencies: '{node}'\n---\nbody\n"
            ),
        )
        .expect("write");
        let workspace = crate::tools::workspace::Workspace::new(dir.path()).expect("workspace");
        let mut discovered = crate::skills::package::SkillDiscovery::new(&workspace)
            .discover()
            .expect("discover");
        discovered.remove(0)
    }
}
