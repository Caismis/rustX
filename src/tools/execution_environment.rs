//! Captured execution-host environment for native Agent Bash.
//!
//! The snapshot is taken from the launched host, carried to a subagent child
//! through a mode-0600 file in that child's private runtime root, and composed
//! before permission approval. Debug output redacts values.
//!
//! Authorized commands can read ordinary user credentials that the host
//! environment already contains. This composition is not an operating-system
//! sandbox.

use std::collections::BTreeMap;
use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use super::environment::{PreparedCommand, ToolEnvironment};
use super::workspace::Workspace;
use super::workspace_python::{self, PythonSelection};

const SNAPSHOT_FILE: &str = "host-environment.json";

const SHELL_STARTUP: &[&str] = &["BASH_ENV", "ENV", "SHELLOPTS", "BASHOPTS"];

const CONTROL_KEYS: &[&str] = &[
    "RUSTX_SUPERVISOR_ROLE",
    "RUSTX_SUPERVISOR_COMMAND",
    "RUSTX_SUPERVISOR_ANCHOR_PID_FILE",
    "RUSTX_SUPERVISOR_PROGRAM",
    "RUSTX_COMMAND_ENVIRONMENT",
    "RUSTX_INTERACTIVE_CONTROL",
    "RUSTX_INTERACTIVE_INNER_CONTROL",
    "RUSTX_INTERACTIVE_ANCHOR_PID_FILE",
    "RUSTX_PHYSICAL_CONTINUATION",
];

const PYTHON_ACTIVATION: &[&str] = &[
    "PYTHONHOME",
    "PYTHONPATH",
    "PYTHONSTARTUP",
    "VIRTUAL_ENV",
    "CONDA_PREFIX",
    "CONDA_DEFAULT_ENV",
    "CONDA_PROMPT_MODIFIER",
    "CONDA_SHLVL",
    "CONDA_PYTHON_EXE",
    "_CE_CONDA",
    "_CE_M",
];

const UV_BOOTSTRAP_DROPS: &[&str] = &["UV_VENV_CLEAR", "UV_PROJECT_ENVIRONMENT"];

/// One captured host environment. Values are not part of [`std::fmt::Debug`].
#[derive(Clone, PartialEq, Eq)]
pub(crate) struct ExecutionHost {
    entries: BTreeMap<String, String>,
    omitted_non_unicode: u32,
    closed: Option<&'static str>,
}

impl std::fmt::Debug for ExecutionHost {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "ExecutionHost {{ entries: {}, omitted_non_unicode: {}, closed: {:?} }}",
            self.entries.len(),
            self.omitted_non_unicode,
            self.closed
        )
    }
}

impl ExecutionHost {
    pub(crate) fn entries(&self) -> &BTreeMap<String, String> {
        &self.entries
    }

    pub(crate) fn closed(&self) -> Option<&'static str> {
        self.closed
    }

    fn from_unicode(
        entries: BTreeMap<String, String>,
        omitted_non_unicode: u32,
        closed: Option<&'static str>,
    ) -> Self {
        Self {
            entries,
            omitted_non_unicode,
            closed,
        }
    }
}

fn host_slot() -> &'static OnceLock<Arc<ExecutionHost>> {
    static HOST: OnceLock<Arc<ExecutionHost>> = OnceLock::new();
    &HOST
}

/// The launched host, captured on first use.
pub(crate) fn current() -> Arc<ExecutionHost> {
    Arc::clone(host_slot().get_or_init(|| Arc::new(capture_process())))
}

/// Installs a parent snapshot before any child capture.
pub(crate) fn install_child_snapshot(runtime_root: &Path) -> bool {
    let path = runtime_root.join(SNAPSHOT_FILE);
    let Ok(bytes) = std::fs::read(&path) else {
        return false;
    };
    let Ok(host) = decode_snapshot(&bytes) else {
        return host_slot()
            .set(Arc::new(ExecutionHost::from_unicode(
                BTreeMap::new(),
                0,
                Some("the child host snapshot is unreadable"),
            )))
            .is_ok();
    };
    host_slot().set(Arc::new(host)).is_ok()
}

/// Writes the parent snapshot into the child runtime root before spawn.
pub(crate) fn publish_child_snapshot(runtime_root: &Path) -> Result<(), String> {
    let host = current();
    let bytes = encode_snapshot(&host)?;
    let path = runtime_root.join(SNAPSHOT_FILE);
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .map_err(|error| format!("could not store the child host snapshot: {error}"))?;
    file.write_all(&bytes)
        .map_err(|error| format!("could not store the child host snapshot: {error}"))?;
    Ok(())
}

fn capture_process() -> ExecutionHost {
    let mut entries = BTreeMap::new();
    let mut omitted = 0u32;
    let mut closed = None;
    for (key, value) in std::env::vars_os() {
        let Some(key) = key.to_str() else {
            omitted = omitted.saturating_add(1);
            continue;
        };
        if key.is_empty() || key.contains('=') || key.contains('\0') {
            omitted = omitted.saturating_add(1);
            continue;
        }
        let Some(value) = value.to_str() else {
            omitted = omitted.saturating_add(1);
            if key == "PATH" || key == "HOME" {
                closed = Some("PATH or HOME is not Unicode");
            }
            continue;
        };
        entries.insert(key.to_owned(), value.to_owned());
    }
    ExecutionHost::from_unicode(entries, omitted, closed)
}

fn encode_snapshot(host: &ExecutionHost) -> Result<Vec<u8>, String> {
    serde_json::to_vec(&SnapshotFile {
        entries: host
            .entries
            .iter()
            .map(|(k, v)| (k.clone(), v.clone()))
            .collect(),
        omitted_non_unicode: host.omitted_non_unicode,
        closed: host.closed.map(str::to_owned),
    })
    .map_err(|error| error.to_string())
}

fn decode_snapshot(bytes: &[u8]) -> Result<ExecutionHost, String> {
    let file: SnapshotFile = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
    let mut entries = BTreeMap::new();
    for (key, value) in file.entries {
        if key.is_empty() || key.contains('=') || key.contains('\0') || value.contains('\0') {
            return Err("malformed host snapshot".into());
        }
        entries.insert(key, value);
    }
    let closed = match file.closed.as_deref() {
        Some("PATH or HOME is not Unicode") => Some("PATH or HOME is not Unicode"),
        Some("the child host snapshot is unreadable") => {
            Some("the child host snapshot is unreadable")
        }
        Some(_) => Some("the child host snapshot is unreadable"),
        None => None,
    };
    Ok(ExecutionHost::from_unicode(
        entries,
        file.omitted_non_unicode,
        closed,
    ))
}

#[derive(serde::Serialize, serde::Deserialize)]
struct SnapshotFile {
    entries: Vec<(String, String)>,
    omitted_non_unicode: u32,
    closed: Option<String>,
}

/// The environment a uv or interpreter probe inherits from the host.
pub(crate) fn bootstrap_environment(host: &ExecutionHost) -> Vec<(String, String)> {
    host.entries
        .iter()
        .filter(|(key, _)| !excluded_from_bootstrap(key))
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

fn excluded_from_bootstrap(key: &str) -> bool {
    UV_BOOTSTRAP_DROPS.contains(&key)
        || PYTHON_ACTIVATION.contains(&key)
        || SHELL_STARTUP.contains(&key)
        || key.starts_with("BASH_FUNC_")
        || CONTROL_KEYS.contains(&key)
        || key.starts_with("RUSTX_TEST_")
}

/// Freezes native Bash before approval. The returned value is an invocation clone.
pub(crate) fn freeze_bash(base: &ToolEnvironment, workspace: &Workspace) -> ToolEnvironment {
    let selection = workspace_python::observe(workspace.root());
    match compose(current().as_ref(), base, &selection) {
        Ok(entries) => base
            .clone()
            .with_prepared_command(PreparedCommand::Entries(Arc::new(entries))),
        Err(()) => base.clone().with_prepared_command(PreparedCommand::Closed),
    }
}

/// Resolves the command environment the Bash supervisor will apply.
pub(crate) fn command_entries(
    environment: &ToolEnvironment,
    workspace: &Workspace,
) -> Result<Vec<(String, String)>, String> {
    if let Some(prepared) = environment.prepared_command() {
        return prepared.entries().map(<[_]>::to_vec).ok_or_else(|| {
            "the execution environment could not be composed because PATH or HOME is not Unicode"
                .to_owned()
        });
    }
    let selection = workspace_python::observe(workspace.root());
    compose(current().as_ref(), environment, &selection).map_err(|()| {
        "the execution environment could not be composed because PATH or HOME is not Unicode"
            .to_owned()
    })
}

/// Composes one native Bash environment from an explicit host snapshot.
pub(crate) fn compose(
    host: &ExecutionHost,
    base: &ToolEnvironment,
    selection: &PythonSelection,
) -> Result<Vec<(String, String)>, ()> {
    if host.closed.is_some() {
        return Err(());
    }
    let mut entries = host.entries.clone();
    for (key, value) in base.authorized_entries() {
        entries.insert(key.clone(), value.clone());
    }
    entries.retain(|key, _| !excluded_after_overrides(key));
    for key in PYTHON_ACTIVATION {
        entries.remove(*key);
    }
    let mut path = String::new();
    for directory in &selection.path_prefix {
        push_path(&mut path, &directory.display().to_string());
    }
    #[cfg(test)]
    {
        for prefix in base.path_prefixes() {
            push_path(&mut path, prefix);
        }
    }
    if let Some(host_path) = entries.get("PATH") {
        let inherited = host_path.clone();
        push_path(&mut path, &inherited);
    }
    entries.insert("PATH".to_owned(), path);
    if let Some(virtual_env) = &selection.virtual_env {
        entries.insert("VIRTUAL_ENV".to_owned(), virtual_env.display().to_string());
    }
    if !entries.contains_key("LANG") {
        entries.insert("LANG".to_owned(), "C.UTF-8".to_owned());
    }
    if !entries.contains_key("LC_ALL") {
        entries.insert("LC_ALL".to_owned(), "C.UTF-8".to_owned());
    }
    Ok(entries.into_iter().collect())
}

fn excluded_after_overrides(key: &str) -> bool {
    SHELL_STARTUP.contains(&key)
        || key.starts_with("BASH_FUNC_")
        || CONTROL_KEYS.contains(&key)
        || key.starts_with("RUSTX_TEST_")
}

fn push_path(path: &mut String, addition: &str) {
    if addition.is_empty() {
        return;
    }
    if !path.is_empty() {
        path.push(':');
    }
    path.push_str(addition);
}

#[cfg(test)]
pub(crate) fn host_from_entries(entries: Vec<(&str, &str)>) -> ExecutionHost {
    ExecutionHost::from_unicode(
        entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect(),
        0,
        None,
    )
}

#[cfg(test)]
pub(crate) fn closed_host() -> ExecutionHost {
    ExecutionHost::from_unicode(BTreeMap::new(), 1, Some("PATH or HOME is not Unicode"))
}

#[cfg(test)]
mod tests {
    use super::{
        PYTHON_ACTIVATION, command_entries, compose, excluded_after_overrides, host_from_entries,
    };
    use crate::tools::environment::{PreparedCommand, ToolEnvironment};
    use crate::tools::workspace::Workspace;
    use crate::tools::workspace_python::PythonSelection;
    use std::path::PathBuf;
    use std::sync::Arc;

    fn selection(ready: bool) -> PythonSelection {
        if ready {
            PythonSelection {
                path_prefix: vec![
                    PathBuf::from("/work/.venv/bin"),
                    PathBuf::from("/guards/ready"),
                ],
                virtual_env: Some(PathBuf::from("/work/.venv")),
                diagnostic: String::new(),
            }
        } else {
            PythonSelection {
                path_prefix: vec![PathBuf::from("/guards/missing")],
                virtual_env: None,
                diagnostic: "Workspace Python is not ready".to_owned(),
            }
        }
    }

    #[test]
    fn u01_host_home_and_path_survive_and_workspace_python_is_prefixed() {
        let host = host_from_entries(vec![
            ("HOME", "/users/ada"),
            ("PATH", "/usr/bin:/bin"),
            ("HTTPS_PROXY", "http://proxy.example"),
            ("LC_ALL", "en_US.UTF-8"),
        ]);
        let entries = compose(&host, &ToolEnvironment::new(), &selection(true)).expect("composed");
        let map: std::collections::BTreeMap<_, _> = entries.into_iter().collect();
        assert_eq!(map.get("HOME").map(String::as_str), Some("/users/ada"));
        assert_eq!(
            map.get("PATH").map(String::as_str),
            Some("/work/.venv/bin:/guards/ready:/usr/bin:/bin")
        );
        assert_eq!(
            map.get("VIRTUAL_ENV").map(String::as_str),
            Some("/work/.venv")
        );
        assert_eq!(
            map.get("HTTPS_PROXY").map(String::as_str),
            Some("http://proxy.example")
        );
        assert_eq!(map.get("LC_ALL").map(String::as_str), Some("en_US.UTF-8"));
        assert!(!format!("{host:?}").contains("/users/ada"));
    }

    #[test]
    fn u02_configured_path_is_the_base_and_the_venv_still_wins() {
        let host = host_from_entries(vec![("HOME", "/users/ada"), ("PATH", "/usr/bin")]);
        let configured = ToolEnvironment::from_configured([
            ("PATH".to_owned(), "/custom/bin".to_owned()),
            ("HOME".to_owned(), "/users/override".to_owned()),
        ])
        .expect("configured");
        let map: std::collections::BTreeMap<_, _> = compose(&host, &configured, &selection(true))
            .expect("composed")
            .into_iter()
            .collect();
        assert_eq!(map.get("HOME").map(String::as_str), Some("/users/override"));
        assert_eq!(
            map.get("PATH").map(String::as_str),
            Some("/work/.venv/bin:/guards/ready:/custom/bin")
        );
    }

    #[test]
    fn u03_shell_startup_and_control_keys_are_removed_after_overrides() {
        let host = host_from_entries(vec![
            ("PATH", "/usr/bin"),
            ("HOME", "/users/ada"),
            ("BASH_ENV", "/tmp/early"),
            ("RUSTX_SUPERVISOR_ROLE", "parent"),
            ("RUSTX_TEST_INJECT", "1"),
            ("UV_OFFLINE", "1"),
        ]);
        let configured = ToolEnvironment::from_configured([
            ("BASH_ENV".to_owned(), "/tmp/again".to_owned()),
            ("ENV".to_owned(), "/tmp/env".to_owned()),
            (
                "BASH_FUNC_python%%".to_owned(),
                "() { /usr/bin/python; }".to_owned(),
            ),
            ("RUSTX_COMMAND_ENVIRONMENT".to_owned(), "secret".to_owned()),
        ])
        .expect("configured");
        let map: std::collections::BTreeMap<_, _> = compose(&host, &configured, &selection(false))
            .expect("composed")
            .into_iter()
            .collect();
        for key in [
            "BASH_ENV",
            "ENV",
            "BASH_FUNC_python%%",
            "RUSTX_COMMAND_ENVIRONMENT",
            "RUSTX_SUPERVISOR_ROLE",
            "RUSTX_TEST_INJECT",
        ] {
            assert!(!map.contains_key(key), "{key} must be absent");
        }
        assert_eq!(map.get("UV_OFFLINE").map(String::as_str), Some("1"));
        assert!(!map.contains_key("UV_PROJECT_ENVIRONMENT"));
        assert!(excluded_after_overrides("BASH_FUNC_python%%"));
    }

    #[test]
    fn u13_python_activation_keys_are_removed_until_the_workspace_env_is_ready() {
        let mut pairs = vec![("PATH", "/usr/bin"), ("HOME", "/users/ada")];
        let mut owned = Vec::new();
        for key in PYTHON_ACTIVATION {
            owned.push((*key, "inherited"));
        }
        pairs.extend(owned.iter().copied());
        let host = host_from_entries(pairs);
        let waiting: std::collections::BTreeMap<_, _> =
            compose(&host, &ToolEnvironment::new(), &selection(false))
                .expect("composed")
                .into_iter()
                .collect();
        for key in PYTHON_ACTIVATION {
            assert!(!waiting.contains_key(*key), "{key}");
        }
        assert_eq!(
            waiting.get("PATH").map(String::as_str),
            Some("/guards/missing:/usr/bin")
        );
        let ready: std::collections::BTreeMap<_, _> =
            compose(&host, &ToolEnvironment::new(), &selection(true))
                .expect("composed")
                .into_iter()
                .collect();
        assert_eq!(
            ready.get("VIRTUAL_ENV").map(String::as_str),
            Some("/work/.venv")
        );
        assert!(!ready.contains_key("PYTHONPATH"));
        assert!(!ready.contains_key("CONDA_PREFIX"));
    }

    #[test]
    fn issue_u16_prepared_entries_are_reused_without_recomposition() {
        let root = tempfile::tempdir().expect("workspace");
        let workspace = Workspace::new(root.path()).expect("workspace");
        let frozen = vec![
            ("HOME".to_owned(), "/users/ada".to_owned()),
            ("PATH".to_owned(), "/frozen/bin".to_owned()),
        ];
        let environment = ToolEnvironment::new()
            .with_prepared_command(PreparedCommand::Entries(Arc::new(frozen.clone())));
        let entries = command_entries(&environment, &workspace).expect("entries");
        assert_eq!(entries, frozen);
        assert!(!format!("{environment:?}").contains("/users/ada"));
        assert!(!format!("{environment:?}").contains("/frozen/bin"));
    }

    #[test]
    fn u14_non_unicode_path_fails_closed() {
        let host = super::closed_host();
        assert!(compose(&host, &ToolEnvironment::new(), &selection(true)).is_err());
    }
}
