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
//! The runtime owns the baseline keys (`PATH`, `HOME`, `LANG`, `LC_ALL`)
//! and the Skill environment overlay keys (`VIRTUAL_ENV`, `NODE_PATH`).
//! The Git configuration namespace (`GIT_CONFIG_COUNT`, `GIT_CONFIG_KEY_*`,
//! `GIT_CONFIG_VALUE_*`, `GIT_CONFIG_PARAMETERS`) and Git pager/editor keys
//! are also runtime-owned so authorized entries cannot bypass or be silently
//! discarded by the composed Git defaults.
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
pub const RUNTIME_OWNED_KEYS: [&str; 10] = [
    "PATH",
    "HOME",
    "LANG",
    "LC_ALL",
    "VIRTUAL_ENV",
    "NODE_PATH",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_PAGER",
    "GIT_EDITOR",
];

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
            entries: vec![("VIRTUAL_ENV".to_owned(), python_root.display().to_string())],
        }
    }

    /// The Node environment overlay: `<root>/node_modules/.bin` on `PATH`
    /// and `NODE_PATH=<root>/node_modules`.
    #[must_use]
    pub fn node(node_root: &std::path::Path) -> Self {
        Self {
            path_prefixes: vec![
                node_root
                    .join("node_modules")
                    .join(".bin")
                    .display()
                    .to_string(),
            ],
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
            if RUNTIME_OWNED_KEYS.contains(&key.as_str())
                || key.starts_with("GIT_CONFIG_KEY_")
                || key.starts_with("GIT_CONFIG_VALUE_")
            {
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
        combined
            .overlay
            .path_prefixes
            .extend(overlay.path_prefixes.clone());
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
        // Keep ordinary Git output intact while controlling the shared child
        // environment. Git still requires authorization: argv alone cannot
        // exclude repository-configured helpers.
        apply_git_execution_environment(&mut entries);
        entries
    }
}

/// Deterministic child defaults, not a proof that a Git command is read-only.
/// In particular, clean filters, textconv, external diff and signing helpers
/// remain repository-configurable and require normal permission admission.
fn apply_git_execution_environment(entries: &mut Vec<(String, String)>) {
    const GUARDS: &[(&str, &str)] = &[
        ("GIT_CONFIG_COUNT", "5"),
        ("GIT_CONFIG_KEY_0", "core.useBuiltinFSMonitor"),
        ("GIT_CONFIG_VALUE_0", "false"),
        ("GIT_CONFIG_KEY_1", "diff.orderFile"),
        ("GIT_CONFIG_VALUE_1", "/dev/null"),
        ("GIT_CONFIG_KEY_2", "core.editor"),
        ("GIT_CONFIG_VALUE_2", "true"),
        ("GIT_CONFIG_KEY_3", "core.pager"),
        ("GIT_CONFIG_VALUE_3", "cat"),
        ("GIT_CONFIG_KEY_4", "core.fsmonitor"),
        ("GIT_CONFIG_VALUE_4", "false"),
        ("GIT_PAGER", "cat"),
        ("GIT_EDITOR", "true"),
        ("GIT_CONFIG_PARAMETERS", ""),
    ];
    for (key, value) in GUARDS {
        entries.retain(|(existing, _)| existing != key);
        entries.push(((*key).to_owned(), (*value).to_owned()));
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
                ("GIT_CONFIG_COUNT".to_owned(), "5".to_owned()),
                (
                    "GIT_CONFIG_KEY_0".to_owned(),
                    "core.useBuiltinFSMonitor".to_owned()
                ),
                ("GIT_CONFIG_VALUE_0".to_owned(), "false".to_owned()),
                ("GIT_CONFIG_KEY_1".to_owned(), "diff.orderFile".to_owned()),
                ("GIT_CONFIG_VALUE_1".to_owned(), "/dev/null".to_owned()),
                ("GIT_CONFIG_KEY_2".to_owned(), "core.editor".to_owned()),
                ("GIT_CONFIG_VALUE_2".to_owned(), "true".to_owned()),
                ("GIT_CONFIG_KEY_3".to_owned(), "core.pager".to_owned()),
                ("GIT_CONFIG_VALUE_3".to_owned(), "cat".to_owned()),
                ("GIT_CONFIG_KEY_4".to_owned(), "core.fsmonitor".to_owned()),
                ("GIT_CONFIG_VALUE_4".to_owned(), "false".to_owned()),
                ("GIT_PAGER".to_owned(), "cat".to_owned()),
                ("GIT_EDITOR".to_owned(), "true".to_owned()),
                ("GIT_CONFIG_PARAMETERS".to_owned(), String::new()),
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

    #[test]
    fn indexed_git_configuration_is_rejected_even_beyond_composed_indices() {
        for key in [
            "GIT_CONFIG_KEY_0",
            "GIT_CONFIG_VALUE_0",
            "GIT_CONFIG_KEY_4",
            "GIT_CONFIG_VALUE_4",
            "GIT_CONFIG_KEY_17",
            "GIT_CONFIG_VALUE_17",
        ] {
            assert_eq!(
                ToolEnvironment::from_authorized([(key.to_owned(), "value".to_owned())]),
                Err(ToolEnvironmentError::RuntimeOwnedKey(key.to_owned())),
                "{key} belongs to the runtime-owned Git configuration namespace"
            );
        }
    }

    /// The overlay composes deterministically: Python bin first, then Node
    /// .bin, then the baseline; `VIRTUAL_ENV` and `NODE_PATH` are set; no
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
                ("GIT_CONFIG_COUNT".to_owned(), "5".to_owned()),
                ("GIT_CONFIG_KEY_0".to_owned(), "core.useBuiltinFSMonitor".to_owned()),
                ("GIT_CONFIG_VALUE_0".to_owned(), "false".to_owned()),
                ("GIT_CONFIG_KEY_1".to_owned(), "diff.orderFile".to_owned()),
                ("GIT_CONFIG_VALUE_1".to_owned(), "/dev/null".to_owned()),
                ("GIT_CONFIG_KEY_2".to_owned(), "core.editor".to_owned()),
                ("GIT_CONFIG_VALUE_2".to_owned(), "true".to_owned()),
                ("GIT_CONFIG_KEY_3".to_owned(), "core.pager".to_owned()),
                ("GIT_CONFIG_VALUE_3".to_owned(), "cat".to_owned()),
                ("GIT_CONFIG_KEY_4".to_owned(), "core.fsmonitor".to_owned()),
                ("GIT_CONFIG_VALUE_4".to_owned(), "false".to_owned()),
                ("GIT_PAGER".to_owned(), "cat".to_owned()),
                ("GIT_EDITOR".to_owned(), "true".to_owned()),
                ("GIT_CONFIG_PARAMETERS".to_owned(), String::new()),
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
                ("GIT_CONFIG_COUNT".to_owned(), "5".to_owned()),
                (
                    "GIT_CONFIG_KEY_0".to_owned(),
                    "core.useBuiltinFSMonitor".to_owned()
                ),
                ("GIT_CONFIG_VALUE_0".to_owned(), "false".to_owned()),
                ("GIT_CONFIG_KEY_1".to_owned(), "diff.orderFile".to_owned()),
                ("GIT_CONFIG_VALUE_1".to_owned(), "/dev/null".to_owned()),
                ("GIT_CONFIG_KEY_2".to_owned(), "core.editor".to_owned()),
                ("GIT_CONFIG_VALUE_2".to_owned(), "true".to_owned()),
                ("GIT_CONFIG_KEY_3".to_owned(), "core.pager".to_owned()),
                ("GIT_CONFIG_VALUE_3".to_owned(), "cat".to_owned()),
                ("GIT_CONFIG_KEY_4".to_owned(), "core.fsmonitor".to_owned()),
                ("GIT_CONFIG_VALUE_4".to_owned(), "false".to_owned()),
                ("GIT_PAGER".to_owned(), "cat".to_owned()),
                ("GIT_EDITOR".to_owned(), "true".to_owned()),
                ("GIT_CONFIG_PARAMETERS".to_owned(), String::new()),
            ],
            "no ecosystem dependencies means no unnecessary overlay"
        );
    }

    /// Equivalent capability snapshots construct identical child
    /// environments.
    #[test]
    fn equivalent_overlays_construct_identical_child_environments() {
        let base = ToolEnvironment::new();
        let first = base.with_overlay(&ToolEnvironmentOverlay::python(std::path::Path::new(
            "/e/p",
        )));
        let second = base.with_overlay(&ToolEnvironmentOverlay::python(std::path::Path::new(
            "/e/p",
        )));
        assert_eq!(first, second);
        assert_eq!(
            first.child_environment(std::path::Path::new("/ws")),
            second.child_environment(std::path::Path::new("/ws"))
        );
    }

    struct GitFixture {
        _root: tempfile::TempDir,
        repo: std::path::PathBuf,
        marker: std::path::PathBuf,
        helper: std::path::PathBuf,
    }

    impl GitFixture {
        fn new() -> Self {
            use std::os::unix::fs::PermissionsExt;
            let root = tempfile::tempdir().expect("temporary Git fixture");
            let repo = root.path().join("repo");
            std::fs::create_dir(&repo).expect("repo");
            let marker = root.path().join("helper-fired");
            let helper = root.path().join("helper.sh");
            std::fs::write(
                &helper,
                format!("#!/bin/sh\necho fired > '{}'\ncat\n", marker.display()),
            )
            .expect("helper");
            std::fs::set_permissions(&helper, std::fs::Permissions::from_mode(0o755))
                .expect("executable helper");
            let fixture = Self {
                _root: root,
                repo,
                marker,
                helper,
            };
            assert!(fixture.run(&["init", "-q"]).status.success());
            std::fs::write(fixture.repo.join("file.txt"), "hello\n").expect("file");
            assert!(fixture.run(&["add", "file.txt"]).status.success());
            assert!(
                fixture
                    .run(&[
                        "-c",
                        "user.name=Test",
                        "-c",
                        "user.email=test@example.com",
                        "commit",
                        "-qm",
                        "initial",
                    ])
                    .status
                    .success()
            );
            fixture
        }

        fn run(&self, args: &[&str]) -> std::process::Output {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&self.repo)
                .env_clear()
                .envs(ToolEnvironment::new().child_environment(&self.repo))
                .env("GIT_CONFIG_GLOBAL", "/dev/null")
                .env("GIT_CONFIG_NOSYSTEM", "1")
                .output()
                .unwrap_or_else(|error| panic!("git {args:?}: {error}"))
        }

        fn configure_helper(&self, key: &str) {
            assert!(
                self.run(&["config", key, &self.helper.display().to_string()])
                    .status
                    .success()
            );
        }
    }

    /// Ordinary authorized Git commands retain real patches. Explicit helper
    /// flags can preserve output, but never establish readonly authorization.
    #[test]
    fn git_environment_preserves_real_patches_without_claiming_readonly() {
        let fixture = GitFixture::new();
        fixture.configure_helper("core.fsmonitor");
        std::fs::write(fixture.repo.join("file.txt"), "updated\n").unwrap();
        let status = fixture.run(&["status", "--porcelain"]);
        assert!(
            status.status.success(),
            "{}",
            String::from_utf8_lossy(&status.stderr)
        );
        assert!(String::from_utf8_lossy(&status.stdout).contains(" M file.txt"));
        let diff = fixture.run(&["diff"]);
        assert!(
            diff.status.success(),
            "{}",
            String::from_utf8_lossy(&diff.stderr)
        );
        let patch = String::from_utf8_lossy(&diff.stdout);
        assert!(
            patch.contains("a/file.txt") && patch.contains("-hello") && patch.contains("+updated")
        );
        std::fs::write(fixture.repo.join(".gitattributes"), "file.txt diff=probe\n").unwrap();
        for key in ["diff.external", "diff.probe.textconv"] {
            fixture.configure_helper(key);
        }
        for args in [
            vec!["diff", "--no-ext-diff", "--no-textconv"],
            vec!["show", "--no-ext-diff", "--no-textconv", "HEAD"],
            vec!["log", "--no-ext-diff", "--no-textconv", "-p", "-1"],
        ] {
            let command = format!("git {}", args.join(" "));
            assert!(matches!(
                crate::runtime::permission::analyze_bash_readonly(&command).class,
                crate::runtime::permission::BashReadonlyClass::Unproven { .. }
            ));
            let result = fixture.run(&args);
            assert!(
                result.status.success(),
                "{command}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            let patch = String::from_utf8_lossy(&result.stdout);
            assert!(
                patch.contains("a/file.txt") && patch.contains("hello"),
                "{command}: {patch}"
            );
        }
        assert!(
            !fixture.marker.exists(),
            "explicit diff flags and fsmonitor default suppress these particular helpers"
        );
    }

    /// Clean conversion remains configurable even when external diff and
    /// textconv are disabled. An argv-only Git proof would authorize this code.
    #[test]
    fn git_clean_filters_require_authorization_even_with_diff_helper_flags() {
        let fixture = GitFixture::new();
        fixture.configure_helper("filter.probe.clean");
        std::fs::write(
            fixture.repo.join(".gitattributes"),
            "file.txt filter=probe\n",
        )
        .unwrap();
        let path = fixture.repo.join("file.txt");
        std::fs::write(&path, "world\n").unwrap();
        std::fs::OpenOptions::new()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2))
            .unwrap();
        for args in [
            vec!["status", "--porcelain"],
            vec!["diff", "--no-ext-diff", "--no-textconv"],
        ] {
            let command = format!("git {}", args.join(" "));
            assert!(matches!(
                crate::runtime::permission::analyze_bash_readonly(&command).class,
                crate::runtime::permission::BashReadonlyClass::Unproven { .. }
            ));
            let result = fixture.run(&args);
            assert!(
                result.status.success(),
                "{command}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert!(
                fixture.marker.exists(),
                "{command} invokes configured clean conversion"
            );
            std::fs::remove_file(&fixture.marker).unwrap();
        }
    }
}
