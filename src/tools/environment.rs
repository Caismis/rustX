//! The explicit tool execution environment.
//!
//! Native subprocess tools construct an explicit child environment instead
//! of inheriting the parent process environment wholesale: only runtime-
//! approved basics plus entries explicitly authorized through the runtime's
//! tool environment configuration are visible to child processes. Parent-
//! process secrets are absent unless explicitly provided to the tool
//! environment. This is deliberately not a production secrets manager.
//!
//! # Reserved runtime-owned keys
//!
//! The runtime owns the baseline keys (`PATH`, `HOME`, `LANG`, `LC_ALL`)
//! and the Skill environment overlay keys (`VIRTUAL_ENV`, `NODE_PATH`).
//! These cannot be supplied as conflicting ordinary authorized entries:
//! [`ToolEnvironment::from_authorized`] rejects them with
//! [`ToolEnvironmentError::RuntimeOwnedKey`]. There is no ambiguous
//! "silently ignore the authorized override" behavior.
//!
//! # Skill environment overlay
//!
//! The attempt-level effective environment is the base authorized
//! environment plus the deterministic runtime-owned Skill environment
//! layer ([`ToolEnvironmentOverlay`]): Python and Node bin prefixes are
//! inserted before the baseline `PATH`, and `VIRTUAL_ENV` / `NODE_PATH`
//! are set when the corresponding environment exists. An ecosystem with no
//! dependencies adds no overlay.

use std::collections::BTreeMap;

/// The runtime-owned baseline keys: they are composed by
/// [`ToolEnvironment::child_environment`] and cannot be supplied as
/// conflicting authorized entries.
pub const RUNTIME_OWNED_KEYS: [&str; 6] = ["PATH", "HOME", "LANG", "LC_ALL", "VIRTUAL_ENV", "NODE_PATH"];

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

/// The deterministic runtime-owned Skill environment layer.
///
/// `path_prefixes` are inserted before the baseline `PATH` in order
/// (Python bin first, then Node `.bin`); `entries` are appended as
/// runtime-owned environment entries (`VIRTUAL_ENV`, `NODE_PATH`). An
/// ecosystem with no dependencies adds nothing.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolEnvironmentOverlay {
    path_prefixes: Vec<String>,
    entries: Vec<(String, String)>,
}

impl ToolEnvironmentOverlay {
    /// The Python environment overlay: `<root>/bin` on `PATH` and
    /// `VIRTUAL_ENV=<root>`.
    #[must_use]
    pub fn python(python_root: &std::path::Path) -> Self {
        Self {
            path_prefixes: vec![python_root.join("bin").display().to_string()],
            entries: vec![(
                "VIRTUAL_ENV".to_owned(),
                python_root.display().to_string(),
            )],
        }
    }

    /// The Node environment overlay: `<root>/node_modules/.bin` on `PATH`
    /// and `NODE_PATH=<root>/node_modules`.
    #[must_use]
    pub fn node(node_root: &std::path::Path) -> Self {
        Self {
            path_prefixes: vec![node_root
                .join("node_modules")
                .join(".bin")
                .display()
                .to_string()],
            entries: vec![(
                "NODE_PATH".to_owned(),
                node_root.join("node_modules").display().to_string(),
            )],
        }
    }

    /// Merges two overlays deterministically (Python overlay first when
    /// both exist).
    #[must_use]
    pub fn merge(mut self, other: Self) -> Self {
        self.path_prefixes.extend(other.path_prefixes);
        self.entries.extend(other.entries);
        self
    }

    /// Whether the overlay contributes anything.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.path_prefixes.is_empty() && self.entries.is_empty()
    }
}

/// The explicit tool execution environment of one conversation runtime.
///
/// The environment is deterministic: entries are stored sorted by key, so a
/// constructed child environment is reproducible. The Bash executor composes
/// the runtime-approved basics (`PATH`, `HOME`, `LANG`, `LC_ALL`) plus the
/// runtime-owned Skill environment overlay plus the explicitly authorized
/// entries; nothing from the parent process environment is inherited
/// wholesale.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolEnvironment {
    authorized: Vec<(String, String)>,
    overlay: ToolEnvironmentOverlay,
}

impl ToolEnvironment {
    /// Creates an empty tool environment: only the runtime-approved basics
    /// are composed by subprocess executors.
    #[must_use]
    pub fn new() -> Self {
        Self {
            authorized: Vec::new(),
            overlay: ToolEnvironmentOverlay::default(),
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
            overlay: ToolEnvironmentOverlay::default(),
        })
    }

    /// The explicitly authorized entries in deterministic sorted order.
    #[must_use]
    pub fn authorized_entries(&self) -> &[(String, String)] {
        &self.authorized
    }

    /// The deterministic runtime-owned Skill environment overlay.
    #[must_use]
    pub fn overlay(&self) -> &ToolEnvironmentOverlay {
        &self.overlay
    }

    /// Composes the attempt-level effective environment: this base
    /// environment plus the deterministic runtime-owned Skill environment
    /// overlay.
    #[must_use]
    pub fn with_overlay(&self, overlay: &ToolEnvironmentOverlay) -> Self {
        let mut combined = self.clone();
        combined.overlay.path_prefixes.extend(overlay.path_prefixes.clone());
        combined.overlay.entries.extend(overlay.entries.clone());
        combined
    }

    /// The complete deterministic child environment for a workspace-rooted
    /// subprocess: the runtime-approved basics, the Skill environment
    /// overlay, and the authorized entries.
    ///
    /// ```text
    /// PATH=<python>/bin:<node>/node_modules/.bin:/usr/local/bin:/usr/bin:/bin
    /// HOME=<workspace root>
    /// LANG=C.UTF-8
    /// LC_ALL=C.UTF-8
    /// VIRTUAL_ENV=<python root>          (when a Python overlay exists)
    /// NODE_PATH=<node root>/node_modules (when a Node overlay exists)
    /// ```
    ///
    /// plus every explicitly authorized entry.
    #[must_use]
    pub fn child_environment(&self, workspace_root: &std::path::Path) -> Vec<(String, String)> {
        let mut path = String::new();
        for prefix in &self.overlay.path_prefixes {
            if !path.is_empty() {
                path.push(':');
            }
            path.push_str(prefix);
        }
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
        entries.extend(self.overlay.entries.clone());
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
    use super::{ToolEnvironment, ToolEnvironmentError, ToolEnvironmentOverlay};

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

    /// Runtime-owned baseline/overlay keys cannot be supplied as
    /// conflicting ordinary authorized entries.
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

    /// The overlay composes deterministically: Python bin first, then Node
    /// .bin, then the baseline; VIRTUAL_ENV and NODE_PATH are set; no
    /// ecosystem dependencies add no overlay.
    #[test]
    fn overlay_composes_deterministically() {
        let base = ToolEnvironment::from_authorized([("MY_VAR".to_owned(), "1".to_owned())])
            .expect("authorized");
        let python = ToolEnvironmentOverlay::python(std::path::Path::new("/env/python/abc"));
        let node = ToolEnvironmentOverlay::node(std::path::Path::new("/env/node/def"));
        let effective = base.with_overlay(&python.merge(node));
        let entries = effective.child_environment(std::path::Path::new("/ws"));
        assert_eq!(
            entries,
            vec![
                (
                    "PATH".to_owned(),
                    "/env/python/abc/bin:/env/node/def/node_modules/.bin:/usr/local/bin:/usr/bin:/bin"
                        .to_owned()
                ),
                ("HOME".to_owned(), "/ws".to_owned()),
                ("LANG".to_owned(), "C.UTF-8".to_owned()),
                ("LC_ALL".to_owned(), "C.UTF-8".to_owned()),
                ("VIRTUAL_ENV".to_owned(), "/env/python/abc".to_owned()),
                ("NODE_PATH".to_owned(), "/env/node/def/node_modules".to_owned()),
                ("MY_VAR".to_owned(), "1".to_owned()),
            ]
        );
        assert_eq!(
            base.child_environment(std::path::Path::new("/ws")),
            vec![
                ("PATH".to_owned(), "/usr/local/bin:/usr/bin:/bin".to_owned()),
                ("HOME".to_owned(), "/ws".to_owned()),
                ("LANG".to_owned(), "C.UTF-8".to_owned()),
                ("LC_ALL".to_owned(), "C.UTF-8".to_owned()),
                ("MY_VAR".to_owned(), "1".to_owned()),
            ],
            "no ecosystem dependencies means no unnecessary overlay"
        );
    }

    /// Equivalent capability snapshots construct identical child
    /// environments.
    #[test]
    fn equivalent_overlays_construct_identical_child_environments() {
        let base = ToolEnvironment::new();
        let first = base.with_overlay(&ToolEnvironmentOverlay::python(std::path::Path::new("/e/p")));
        let second =
            base.with_overlay(&ToolEnvironmentOverlay::python(std::path::Path::new("/e/p")));
        assert_eq!(first, second);
        assert_eq!(
            first.child_environment(std::path::Path::new("/ws")),
            second.child_environment(std::path::Path::new("/ws"))
        );
    }
}
