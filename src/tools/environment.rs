//! The explicit tool execution environment.
//!
//! Native subprocess tools construct an explicit child environment instead
//! of inheriting the parent process environment wholesale: only runtime-
//! approved basics plus entries explicitly authorized through the runtime's
//! tool environment configuration are visible to child processes. Parent-
//! process secrets are absent unless explicitly provided to the tool
//! environment. This is deliberately not a production secrets manager.
//!
//! `ToolEnvironment` configures only the executed tool process. It never configures
//! supervisor ownership, cancellation, settlement, tracing, fault injection, or
//! physical authority. Internal-looking names remain ordinary child data; private
//! supervisor configuration travels through a separate runtime-owned channel.
//!
//! # Reserved runtime-owned keys
//!
//! The runtime owns the baseline keys (`PATH`, `HOME`, `LANG`, `LC_ALL`).
//! These cannot be supplied as conflicting ordinary authorized entries:
//! [`ToolEnvironment::from_authorized`] rejects them with
//! [`ToolEnvironmentError::RuntimeOwnedKey`]. There is no ambiguous
//! "silently ignore the authorized override" behavior.
//!
//! Skills do not contribute to this environment. Discovery, selection, and
//! catalog publication leave the child environment unchanged.

use std::collections::BTreeMap;

/// The runtime-owned baseline keys: they are composed by
/// [`ToolEnvironment::child_environment`] and cannot be supplied as
/// conflicting authorized entries.
pub const RUNTIME_OWNED_KEYS: [&str; 4] = ["PATH", "HOME", "LANG", "LC_ALL"];

/// An environment configuration failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolEnvironmentError {
    /// An environment key is empty or malformed.
    InvalidKey(String),
    /// The same key was authorized twice with different values.
    DuplicateKey(String),
    /// A runtime-owned key was supplied as a conflicting ordinary
    /// authorized entry.
    RuntimeOwnedKey(String),
}

impl core::fmt::Display for ToolEnvironmentError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidKey(key) => {
                write!(f, "tool environment key {key:?} is empty or malformed")
            }
            Self::DuplicateKey(key) => {
                write!(f, "tool environment key {key:?} was authorized twice")
            }
            Self::RuntimeOwnedKey(key) => write!(
                f,
                "tool environment key {key:?} is runtime-owned and cannot be authorized as a \
                 conflicting ordinary entry"
            ),
        }
    }
}

impl std::error::Error for ToolEnvironmentError {}

/// The explicit tool execution environment of one conversation runtime.
///
/// The environment is deterministic: entries are stored sorted by key, so a
/// constructed child environment is reproducible. The Bash executor composes
/// the runtime-approved basics (`PATH`, `HOME`, `LANG`, `LC_ALL`) plus the
/// explicitly authorized entries; nothing from the parent process environment
/// is inherited wholesale. Skill discovery never adds entries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolEnvironment {
    authorized: Vec<(String, String)>,
    /// Test-only interpreter locations outside the baseline `PATH`.
    /// Production Skill and capability preparation never populate this.
    #[cfg(test)]
    path_prefixes: Vec<String>,
}

impl ToolEnvironment {
    /// Creates an empty tool environment: only the runtime-approved basics
    /// are composed by subprocess executors.
    #[must_use]
    pub fn new() -> Self {
        Self {
            authorized: Vec::new(),
            #[cfg(test)]
            path_prefixes: Vec::new(),
        }
    }

    /// Creates a tool environment from explicitly authorized entries.
    ///
    /// # Errors
    ///
    /// Returns [`ToolEnvironmentError::InvalidKey`] for an empty or
    /// malformed key, [`ToolEnvironmentError::DuplicateKey`] for a key
    /// authorized twice with a different value, and
    /// [`ToolEnvironmentError::RuntimeOwnedKey`] for a runtime-owned key
    /// supplied as a conflicting ordinary authorized entry.
    ///
    /// # Panics
    ///
    /// Panics only if the internal sorted map is inconsistent, which is
    /// impossible by construction.
    pub fn from_authorized(
        entries: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, ToolEnvironmentError> {
        let mut sorted: BTreeMap<String, String> = BTreeMap::new();
        for (key, value) in entries {
            if key.is_empty() || key.contains('=') || key.contains('\0') {
                return Err(ToolEnvironmentError::InvalidKey(key));
            }
            if RUNTIME_OWNED_KEYS.contains(&key.as_str()) {
                return Err(ToolEnvironmentError::RuntimeOwnedKey(key));
            }
            if let Some(existing) = sorted.insert(key.clone(), value.clone())
                && existing != value
            {
                return Err(ToolEnvironmentError::DuplicateKey(key));
            }
        }
        Ok(Self {
            authorized: sorted.into_iter().collect(),
            #[cfg(test)]
            path_prefixes: Vec::new(),
        })
    }

    /// The explicitly authorized entries in deterministic sorted order.
    #[must_use]
    pub fn authorized_entries(&self) -> &[(String, String)] {
        &self.authorized
    }

    /// Prepends one executable directory for a crate test whose interpreter
    /// is installed outside the baseline `PATH`.
    #[cfg(test)]
    #[must_use]
    pub fn with_path_prefix(&self, prefix: &std::path::Path) -> Self {
        let mut combined = self.clone();
        combined.path_prefixes.push(prefix.display().to_string());
        combined
    }

    /// The complete deterministic child environment for a workspace-rooted
    /// subprocess: the runtime-approved basics and the authorized entries.
    ///
    /// ```text
    /// PATH=/usr/local/bin:/usr/bin:/bin
    /// HOME=<workspace root>
    /// LANG=C.UTF-8
    /// LC_ALL=C.UTF-8
    /// ```
    ///
    /// plus every explicitly authorized entry.
    #[must_use]
    pub fn child_environment(&self, workspace_root: &std::path::Path) -> Vec<(String, String)> {
        let mut path = String::new();
        #[cfg(test)]
        for prefix in &self.path_prefixes {
            if !path.is_empty() {
                path.push(':');
            }
            path.push_str(prefix);
        }
        #[cfg(test)]
        if !path.is_empty() {
            path.push(':');
        }
        path.push_str("/usr/local/bin:/usr/bin:/bin");
        let mut entries = vec![
            ("PATH".to_owned(), path),
            ("HOME".to_owned(), workspace_root.display().to_string()),
            ("LANG".to_owned(), String::from("C.UTF-8")),
            ("LC_ALL".to_owned(), String::from("C.UTF-8")),
        ];
        for (key, value) in &self.authorized {
            if !entries.iter().any(|(existing, _)| existing == key) {
                entries.push((key.clone(), value.clone()));
            }
        }
        entries
    }
}

#[cfg(test)]
mod tests {
    use super::{ToolEnvironment, ToolEnvironmentError};

    #[test]
    fn empty_environment_composes_only_approved_basics() {
        let environment = ToolEnvironment::new();
        let entries = environment.child_environment(std::path::Path::new("/ws"));
        assert_eq!(
            entries,
            vec![
                ("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned()),
                ("HOME".to_owned(), "/ws".to_owned()),
                ("LANG".to_owned(), String::from("C.UTF-8")),
                ("LC_ALL".to_owned(), String::from("C.UTF-8")),
            ]
        );
    }

    #[test]
    fn authorized_entries_are_sorted_deterministically() {
        let environment = ToolEnvironment::from_authorized([
            ("ZED".to_owned(), "z".to_owned()),
            ("ALPHA".to_owned(), "a".to_owned()),
        ])
        .expect("authorized");
        assert_eq!(
            environment.authorized_entries(),
            &[
                ("ALPHA".to_owned(), "a".to_owned()),
                ("ZED".to_owned(), "z".to_owned()),
            ]
        );
    }

    #[test]
    fn malformed_and_duplicate_entries_are_rejected() {
        assert_eq!(
            ToolEnvironment::from_authorized([(String::new(), "x".to_owned())]),
            Err(ToolEnvironmentError::InvalidKey(String::new()))
        );
        assert_eq!(
            ToolEnvironment::from_authorized([("A=B".to_owned(), "x".to_owned())]),
            Err(ToolEnvironmentError::InvalidKey("A=B".to_owned()))
        );
        let duplicate = ToolEnvironment::from_authorized([
            ("K".to_owned(), "1".to_owned()),
            ("K".to_owned(), "1".to_owned()),
        ]);
        assert!(duplicate.is_ok(), "identical values are idempotent");
        assert_eq!(
            ToolEnvironment::from_authorized([
                ("K".to_owned(), "1".to_owned()),
                ("K".to_owned(), "2".to_owned()),
            ]),
            Err(ToolEnvironmentError::DuplicateKey("K".to_owned()))
        );
    }

    /// Runtime-owned baseline keys cannot be supplied as conflicting
    /// ordinary authorized entries.
    #[test]
    fn runtime_owned_keys_are_rejected_as_authorized_entries() {
        for key in super::RUNTIME_OWNED_KEYS {
            let result = ToolEnvironment::from_authorized([(key.to_owned(), "value".to_owned())]);
            assert_eq!(
                result,
                Err(ToolEnvironmentError::RuntimeOwnedKey(key.to_owned())),
                "{key} must be runtime-owned"
            );
        }
    }

    /// Authorized entries are visible, and Skills have no path into `PATH`.
    #[test]
    fn authorized_entries_compose_without_a_skill_overlay() {
        let environment = ToolEnvironment::from_authorized([("MY_VAR".to_owned(), "1".to_owned())])
            .expect("authorized");
        assert_eq!(
            environment.child_environment(std::path::Path::new("/ws")),
            vec![
                ("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned()),
                ("HOME".to_owned(), "/ws".to_owned()),
                ("LANG".to_owned(), "C.UTF-8".to_owned()),
                ("LC_ALL".to_owned(), "C.UTF-8".to_owned()),
                ("MY_VAR".to_owned(), "1".to_owned()),
            ]
        );
    }
}
