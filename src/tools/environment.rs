//! The explicit tool execution environment.
//!
//! Excluded executors — Managed Python, manual terminals, generic
//! `command/exec`, and MCP — compose [`ToolEnvironment::child_environment`]:
//! a fixed `PATH` and a workspace `HOME`, plus authorized entries.
//! [`ToolEnvironment::from_authorized`] rejects `PATH`, `HOME`, `LANG`, and
//! `LC_ALL` so those callers cannot replace that baseline.
//!
//! Native Agent Bash, Workflow Bash, and Subagent Bash do not use that
//! baseline. They inherit the captured execution host, apply configured
//! `[environment]` overrides (including those four keys, via
//! [`ToolEnvironment::from_configured`]), then select the Workspace default
//! Python. The frozen result is stored on the invocation clone as
//! [`PreparedCommand`] and is omitted from [`std::fmt::Debug`].
//!
//! `ToolEnvironment` configures only the executed tool process. It never configures
//! supervisor ownership, cancellation, settlement, tracing, fault injection, or
//! physical authority. Internal-looking names remain ordinary child data; private
//! supervisor configuration travels through a separate runtime-owned channel.
//!
//! Skills do not contribute to this environment. Discovery, selection, and
//! catalog publication leave the child environment unchanged.

use std::collections::BTreeMap;

/// Keys owned by the runtime. Configured native Bash environments may override
/// `PATH`, `HOME`, `LANG`, and `LC_ALL`; Git execution defaults remain reserved.
pub const RUNTIME_OWNED_KEYS: [&str; 8] = [
    "PATH",
    "HOME",
    "LANG",
    "LC_ALL",
    "GIT_CONFIG_PARAMETERS",
    "GIT_CONFIG_COUNT",
    "GIT_PAGER",
    "GIT_EDITOR",
];

fn is_runtime_owned_git_key(key: &str) -> bool {
    matches!(
        key,
        "GIT_CONFIG_PARAMETERS" | "GIT_CONFIG_COUNT" | "GIT_PAGER" | "GIT_EDITOR"
    ) || key.starts_with("GIT_CONFIG_KEY_")
        || key.starts_with("GIT_CONFIG_VALUE_")
}

/// A command environment frozen for one native Bash invocation.
///
/// Debug output reports only the entry count. Values stay on the invocation
/// clone and are not written to journals or model context by this type.
#[derive(Clone, PartialEq, Eq)]
pub(crate) enum PreparedCommand {
    /// The exact environment the supervisor will apply.
    Entries(std::sync::Arc<Vec<(String, String)>>),
    /// A bounded composition failure; never proceed with an unguarded host PATH.
    Closed(&'static str),
}

impl std::fmt::Debug for PreparedCommand {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Entries(entries) => write!(formatter, "Entries({} redacted)", entries.len()),
            Self::Closed(reason) => write!(formatter, "Closed({reason})"),
        }
    }
}

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
/// The environment is deterministic: entries are stored sorted by key.
/// [`Self::child_environment`] is the excluded-executor baseline.
/// Native Bash uses [`PreparedCommand`] instead. Skill discovery never adds entries.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToolEnvironment {
    authorized: Vec<(String, String)>,
    /// Set only on a per-invocation clone, never on the shared capability snapshot.
    prepared: Option<PreparedCommand>,
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
            prepared: None,
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
        Self::collect(entries, false)
    }

    /// Configuration entries for native Agent Bash, including `PATH` and `HOME`.
    ///
    /// Generic `command/exec` keeps using [`Self::from_authorized`], which
    /// rejects those keys. [`Self::child_environment`] still installs its own
    /// baseline first, so excluded executors ignore a configured override.
    ///
    /// # Errors
    ///
    /// Returns the same key and duplicate errors as [`Self::from_authorized`].
    pub fn from_configured(
        entries: impl IntoIterator<Item = (String, String)>,
    ) -> Result<Self, ToolEnvironmentError> {
        Self::collect(entries, true)
    }

    fn collect(
        entries: impl IntoIterator<Item = (String, String)>,
        allow_baseline_keys: bool,
    ) -> Result<Self, ToolEnvironmentError> {
        let mut sorted: BTreeMap<String, String> = BTreeMap::new();
        for (key, value) in entries {
            if key.is_empty() || key.contains('=') || key.contains('\0') {
                return Err(ToolEnvironmentError::InvalidKey(key));
            }
            if is_runtime_owned_git_key(&key)
                || (!allow_baseline_keys && RUNTIME_OWNED_KEYS.contains(&key.as_str()))
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
            prepared: None,
            #[cfg(test)]
            path_prefixes: Vec::new(),
        })
    }

    /// Attaches a frozen Bash command environment to this invocation clone.
    #[must_use]
    pub(crate) fn with_prepared_command(mut self, prepared: PreparedCommand) -> Self {
        self.prepared = Some(prepared);
        self
    }

    pub(crate) fn prepared_command(&self) -> Option<&PreparedCommand> {
        self.prepared.as_ref()
    }

    #[cfg(test)]
    pub(crate) fn path_prefixes(&self) -> &[String] {
        &self.path_prefixes
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
        apply_git_execution_environment(&mut entries);
        entries
    }
}

/// Deterministic child defaults, not a proof that a Git command is read-only.
/// In particular, clean filters, textconv, external diff and signing helpers
/// remain repository-configurable and require normal permission admission.
pub(crate) fn apply_git_execution_environment(entries: &mut Vec<(String, String)>) {
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
    entries.retain(|(key, _)| !is_runtime_owned_git_key(key));
    for (key, value) in GUARDS {
        entries.push(((*key).to_owned(), (*value).to_owned()));
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

    #[test]
    fn configured_git_execution_keys_remain_reserved() {
        for key in [
            "GIT_CONFIG_PARAMETERS",
            "GIT_CONFIG_COUNT",
            "GIT_PAGER",
            "GIT_EDITOR",
            "GIT_CONFIG_KEY_0",
            "GIT_CONFIG_VALUE_0",
            "GIT_CONFIG_KEY_17",
            "GIT_CONFIG_VALUE_17",
        ] {
            assert_eq!(
                ToolEnvironment::from_configured([(key.to_owned(), "value".to_owned())]),
                Err(ToolEnvironmentError::RuntimeOwnedKey(key.to_owned()))
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
            self.run_with_environment(args, ToolEnvironment::new().child_environment(&self.repo))
        }

        fn run_with_environment(
            &self,
            args: &[&str],
            environment: Vec<(String, String)>,
        ) -> std::process::Output {
            std::process::Command::new("git")
                .args(args)
                .current_dir(&self.repo)
                .env_clear()
                .envs(environment)
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

    #[test]
    fn inherited_git_controls_cannot_replace_native_or_bootstrap_defaults() {
        let fixture = GitFixture::new();
        fixture.configure_helper("core.fsmonitor");
        let helper = fixture.helper.to_str().unwrap();
        let parameters = format!("'core.fsmonitor={helper}'");
        let host = crate::tools::execution_environment::host_from_entries(vec![
            ("PATH", "/usr/local/bin:/usr/bin:/bin"),
            ("GIT_CONFIG_COUNT", "18"),
            ("GIT_CONFIG_KEY_0", "core.fsmonitor"),
            ("GIT_CONFIG_VALUE_0", helper),
            ("GIT_CONFIG_KEY_17", "core.editor"),
            ("GIT_CONFIG_VALUE_17", helper),
            ("GIT_CONFIG_PARAMETERS", &parameters),
            ("GIT_PAGER", helper),
            ("GIT_EDITOR", helper),
        ]);
        let selection = crate::tools::workspace_python::PythonSelection {
            path_prefix: Vec::new(),
            virtual_env: None,
            diagnostic: String::new(),
            guard_unavailable: false,
        };
        for environment in [
            crate::tools::execution_environment::bootstrap_environment(&host),
            crate::tools::execution_environment::compose(
                &host,
                &ToolEnvironment::new(),
                &selection,
            )
            .unwrap(),
        ] {
            assert!(
                !environment
                    .iter()
                    .any(|(key, _)| key == "GIT_CONFIG_KEY_17" || key == "GIT_CONFIG_VALUE_17")
            );
            let config = fixture
                .run_with_environment(&["config", "--get", "core.fsmonitor"], environment.clone());
            assert!(
                config.status.success(),
                "{}",
                String::from_utf8_lossy(&config.stderr)
            );
            assert_eq!(String::from_utf8_lossy(&config.stdout).trim(), "false");
            let status = fixture.run_with_environment(&["status", "--porcelain"], environment);
            assert!(
                status.status.success(),
                "{}",
                String::from_utf8_lossy(&status.stderr)
            );
            assert!(
                !fixture.marker.exists(),
                "inherited controls must not invoke a helper"
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
    #[test]
    fn configured_baseline_keys_do_not_change_the_excluded_child_baseline() {
        let environment = ToolEnvironment::from_configured([
            ("PATH".to_owned(), "/custom/bin".to_owned()),
            ("HOME".to_owned(), "/users/ada".to_owned()),
            ("MY_VAR".to_owned(), "1".to_owned()),
        ])
        .expect("configured");
        assert_eq!(
            environment.child_environment(std::path::Path::new("/ws")),
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
            ]
        );
        let frozen = environment.with_prepared_command(super::PreparedCommand::Entries(
            std::sync::Arc::new(vec![("SECRET_TOKEN".to_owned(), "super-secret".to_owned())]),
        ));
        assert!(!format!("{frozen:?}").contains("super-secret"));
    }
}
