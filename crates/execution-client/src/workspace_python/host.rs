//! Host environment and executable discovery belong to the physical Client.

use super::{GUARD_MESSAGE_KEY, PythonSelection};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

const PYTHON_ACTIVATION: &[&str] = &[
    "PYTHONHOME",
    "PYTHONPATH",
    "PYTHONSTARTUP",
    "PYTHONEXECUTABLE",
    "__PYVENV_LAUNCHER__",
    "VIRTUAL_ENV",
    "CONDA_PREFIX",
    "CONDA_DEFAULT_ENV",
    "CONDA_PROMPT_MODIFIER",
    "CONDA_SHLVL",
    "CONDA_PYTHON_EXE",
    "_CE_CONDA",
    "_CE_M",
];
const UV_BOOTSTRAP_DROPS: &[&str] = &[
    "UV_VENV_CLEAR",
    "UV_VENV_SEED",
    "UV_PROJECT",
    "UV_PROJECT_ENVIRONMENT",
];
const SHELL_STARTUP: &[&str] = &["BASH_ENV", "ENV", "SHELLOPTS", "BASHOPTS"];

pub(super) struct ExecutionHost {
    entries: BTreeMap<String, String>,
    closed: bool,
}

impl ExecutionHost {
    pub(super) fn entries(&self) -> &BTreeMap<String, String> {
        &self.entries
    }

    pub(super) fn closed(&self) -> Option<&'static str> {
        self.closed.then_some("Client PATH or HOME is not Unicode")
    }
}

pub(super) fn home() -> Option<PathBuf> {
    let directory = std::env::var_os("HOME").map(PathBuf::from).or_else(|| {
        nix::unistd::User::from_uid(nix::unistd::geteuid())
            .ok()
            .flatten()
            .map(|user| user.dir)
    })?;
    directory.is_absolute().then_some(directory)
}

pub(super) fn current() -> Arc<ExecutionHost> {
    static HOST: OnceLock<Arc<ExecutionHost>> = OnceLock::new();
    Arc::clone(HOST.get_or_init(|| {
        let mut entries = BTreeMap::new();
        let mut closed = false;
        for (key, value) in std::env::vars_os() {
            let Some(key) = key.to_str() else {
                continue;
            };
            if let Some(value) = value.to_str() {
                entries.insert(key.to_owned(), value.to_owned());
            } else if matches!(key, "HOME" | "PATH") {
                closed = true;
            }
        }
        if let Some(home) = home() {
            entries
                .entry("HOME".to_owned())
                .or_insert_with(|| home.display().to_string());
        }
        entries.entry("PATH".to_owned()).or_insert_with(|| {
            let mut directories = Vec::new();
            if let Some(home) = home() {
                directories.push(home.join(".local/bin"));
            }
            directories.extend(["/usr/local/bin", "/usr/bin", "/bin"].map(PathBuf::from));
            std::env::join_paths(directories)
                .expect("default POSIX executable paths")
                .to_string_lossy()
                .into_owned()
        });
        Arc::new(ExecutionHost { entries, closed })
    }))
}

pub(super) fn bootstrap_environment(host: &ExecutionHost) -> Vec<(String, String)> {
    host.entries
        .iter()
        .filter(|(key, _)| {
            let key = key.as_str();
            !PYTHON_ACTIVATION.contains(&key)
                && !UV_BOOTSTRAP_DROPS.contains(&key)
                && !SHELL_STARTUP.contains(&key)
                && !key.starts_with("BASH_FUNC_")
                && !key.starts_with("RUSTX_")
        })
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}

pub(super) fn apply(
    entries: &mut Vec<(String, String)>,
    selection: &PythonSelection,
) -> Result<(), &'static str> {
    if selection.guard_unavailable {
        return Err(
            "Workspace Python is unavailable and no executable Python guard can be installed",
        );
    }
    entries
        .retain(|(key, _)| !PYTHON_ACTIVATION.contains(&key.as_str()) && key != GUARD_MESSAGE_KEY);
    let path = entries
        .iter_mut()
        .find(|(key, _)| key == "PATH")
        .ok_or("missing Client PATH")?;
    let prefix = selection.path_prefix.iter().map(PathBuf::as_path);
    let combined = std::env::join_paths(
        prefix
            .map(Path::to_path_buf)
            .chain(std::env::split_paths(&path.1)),
    )
    .map_err(|_| "cannot compose the Workspace Python PATH")?;
    combined
        .to_str()
        .ok_or("the Workspace Python PATH is not Unicode")?
        .clone_into(&mut path.1);
    if let Some(virtual_env) = &selection.virtual_env {
        entries.push(("VIRTUAL_ENV".to_owned(), virtual_env.display().to_string()));
    } else if !selection.diagnostic.is_empty() {
        entries.push((GUARD_MESSAGE_KEY.to_owned(), selection.diagnostic.clone()));
    }
    Ok(())
}

#[cfg(test)]
pub(super) fn host_from_entries(entries: Vec<(&str, &str)>) -> ExecutionHost {
    ExecutionHost {
        entries: entries
            .into_iter()
            .map(|(key, value)| (key.to_owned(), value.to_owned()))
            .collect(),
        closed: false,
    }
}

#[cfg(test)]
pub(super) fn compose(
    host: &ExecutionHost,
    base: &[(String, String)],
    selection: &PythonSelection,
) -> Result<Vec<(String, String)>, &'static str> {
    let mut entries = host.entries.clone();
    entries.extend(base.iter().cloned());
    let mut result: Vec<_> = entries.into_iter().collect();
    apply(&mut result, selection)?;
    Ok(result)
}
