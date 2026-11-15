//! Workspace Python pin parsing, requirement checks, and interpreter probes.

use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Mutex;

use pep440_rs::{Version, VersionSpecifier, VersionSpecifiers};

use super::guard::{create_private_directory, monotonic_nanos};
use super::owner::{PROBE_TIMEOUT, stamp};
use super::{Generation, Owner, PathStamp};
use crate::tools::execution_environment::ExecutionHost;

const UV_MINIMUM: &str = "0.10.0";

impl Owner {
    pub(super) async fn uv_version(
        &self,
        root: &Path,
        uv: &Path,
        host: &ExecutionHost,
        expected: &Generation,
    ) -> Result<String, String> {
        let output = self
            .run(
                root,
                vec![uv.display().to_string(), "--version".to_owned()],
                host,
                PROBE_TIMEOUT,
                expected,
            )
            .await?;
        if output.status != 0 {
            return Err("uv --version failed".to_owned());
        }
        output
            .stdout
            .split_whitespace()
            .nth(1)
            .map(str::to_owned)
            .ok_or_else(|| "uv --version could not be parsed".to_owned())
    }

    /// Resolve file/launcher and installation-directory requests identically
    /// for creation and adoption, without requiring uv. Validate the selected
    /// entry before target writes and retain its identity across capacity waits.
    pub(super) async fn check_fixed_interpreter(
        &self,
        root: &Path,
        host: &ExecutionHost,
        pin: Option<&str>,
        requires: Option<&str>,
        expected: &Generation,
    ) -> Result<Option<FixedInterpreter>, &'static str> {
        let Some(pin) = pin.filter(|pin| path_request(pin)) else {
            return Ok(None);
        };
        let path = if Path::new(pin).is_absolute() {
            PathBuf::from(pin)
        } else {
            root.join(pin)
        };
        let path_stamp = stamp(&path).ok_or("the Python interpreter cannot be inspected")?;
        let entry = fixed_interpreter_entry(&path)?;
        let entry_stamp = stamp(&entry).ok_or("the Python interpreter cannot be inspected")?;
        // File requests can name invocation-sensitive tool shims. Resolve the
        // launch path while retaining the authored entry's witness. Directory
        // entries keep their invocation path and virtual-environment context.
        let invocation = if entry == path {
            std::fs::canonicalize(&entry)
                .map_err(|_| "the Python interpreter cannot be inspected")?
        } else {
            entry.clone()
        };
        let interpreter = self
            .probe_interpreter(root, &invocation, host, expected)
            .await?;
        interpreter.check_requirements(Some(pin), requires)?;
        let executable = std::fs::canonicalize(&interpreter.executable)
            .map_err(|_| "the Python interpreter cannot be inspected")?;
        let executable_stamp =
            stamp(&executable).ok_or("the Python interpreter cannot be inspected")?;
        if std::fs::canonicalize(&entry).ok().as_ref() != Some(&executable) {
            // The launcher probe authorized metadata, not the later contents
            // of its reported executable path. Validate that actual file
            // within its own before/after stamp witness.
            let actual = self
                .probe_interpreter(root, &executable, host, expected)
                .await?;
            actual.check_requirements(Some(pin), requires)?;
            if std::fs::canonicalize(&actual.executable).ok().as_ref() != Some(&executable) {
                return Err("the Python interpreter changed during validation");
            }
        }
        let fixed = FixedInterpreter {
            path,
            stamp: path_stamp,
            entry,
            entry_stamp,
            executable,
            executable_stamp,
        };
        if !fixed.is_current() {
            return Err("the Python interpreter changed during validation");
        }
        Ok(Some(fixed))
    }

    pub(super) async fn check_interpreter(
        &self,
        root: &Path,
        program: &Path,
        host: &ExecutionHost,
        pin: Option<&str>,
        requires: Option<&str>,
        expected: &Generation,
    ) -> Result<Option<FixedInterpreter>, InterpreterError> {
        use InterpreterError::{Conflict, Unavailable};
        let interpreter = self
            .probe_interpreter(root, program, host, expected)
            .await
            .map_err(Conflict)?;
        {
            let venv = root.join(".venv");
            let Ok(actual) = std::fs::canonicalize(&interpreter.prefix) else {
                return Err(Conflict(
                    "Workspace .venv is not a valid virtual environment",
                ));
            };
            let Ok(expected) = std::fs::canonicalize(&venv) else {
                return Err(Conflict(
                    "Workspace .venv is not a valid virtual environment",
                ));
            };
            if actual != expected || interpreter.prefix == interpreter.base {
                return Err(Conflict(
                    "Workspace .venv is not a valid virtual environment",
                ));
            }
        }
        interpreter
            .check_requirements(pin, requires)
            .map_err(Conflict)?;
        let fixed = if let Some(pin) = pin.filter(|pin| path_request(pin)) {
            let fixed = self
                .check_fixed_interpreter(root, host, Some(pin), requires, expected)
                .await
                .map_err(Unavailable)?
                .expect("path request");
            // A launcher (notably /usr/bin/python3 on macOS) may exec a
            // different binary. Compare the interpreters' reported base
            // executables, rather than the launcher and venv symlink paths.
            if !fixed.is_current()
                || std::fs::canonicalize(&interpreter.executable).ok().as_ref()
                    != Some(&fixed.executable)
            {
                return Err(Unavailable(
                    "Workspace Python does not satisfy .python-version",
                ));
            }
            Some(fixed)
        } else {
            None
        };
        Ok(fixed)
    }

    async fn probe_interpreter(
        &self,
        root: &Path,
        program: &Path,
        host: &ExecutionHost,
        expected: &Generation,
    ) -> Result<Interpreter, &'static str> {
        let output = self
            .run(
                root,
                vec![
                    program.display().to_string(),
                    "-I".to_owned(),
                    "-c".to_owned(),
                    "import platform, sys; print(sys.prefix); print(sys.base_prefix); print(platform.python_version()); print(sys.implementation.name); print(getattr(sys, '_base_executable', sys.executable))".to_owned(),
                ],
                host,
                PROBE_TIMEOUT,
                expected,
            )
            .await
            .map_err(|_| "the Python interpreter could not be probed")?;
        if output.status != 0 {
            return Err("the Python interpreter could not be probed");
        }
        let mut lines = output.stdout.lines();
        let prefix = lines.next().unwrap_or("").trim().to_owned();
        let base = lines.next().unwrap_or("").trim().to_owned();
        let version_text = lines.next().unwrap_or("").trim();
        let implementation = lines.next().unwrap_or("").trim().to_owned();
        let executable = PathBuf::from(lines.next().unwrap_or("").trim());
        Ok(Interpreter {
            prefix,
            base,
            version: Version::from_str(version_text)
                .map_err(|_| "the Python version could not be verified")?,
            implementation,
            executable,
        })
    }
}

pub(super) enum InterpreterError {
    Conflict(&'static str),
    Unavailable(&'static str),
}

#[derive(Debug)]
pub(super) struct FixedInterpreter {
    path: PathBuf,
    stamp: PathStamp,
    entry: PathBuf,
    entry_stamp: PathStamp,
    pub(super) executable: PathBuf,
    executable_stamp: PathStamp,
}

impl FixedInterpreter {
    pub(super) fn is_current(&self) -> bool {
        stamp(&self.path).as_ref() == Some(&self.stamp)
            && fixed_interpreter_entry(&self.path).ok().as_ref() == Some(&self.entry)
            && stamp(&self.entry).as_ref() == Some(&self.entry_stamp)
            && stamp(&self.executable).as_ref() == Some(&self.executable_stamp)
    }
}

/// Resolve Unix installations in bin/python, then bin/python3 order, independent
/// of the installed uv version (or its absence during adoption). Re-resolve:
/// adding bin/python must invalidate a prior bin/python3 selection too.
pub(super) fn fixed_interpreter_entry(path: &Path) -> Result<PathBuf, &'static str> {
    let metadata =
        std::fs::metadata(path).map_err(|_| "the Python interpreter cannot be inspected")?;
    if metadata.is_dir() {
        let python = path.join("bin/python");
        let entry = match std::fs::symlink_metadata(&python) {
            Ok(_) => python,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => path.join("bin/python3"),
            Err(_) => return Err("the preferred Python installation entry cannot be inspected"),
        };
        if std::fs::metadata(&entry).is_ok_and(|meta| meta.is_file() && meta.mode() & 0o111 != 0) {
            return Ok(entry);
        }
        return Err("the selected bin/python or bin/python3 is missing or not an executable file");
    }
    if !metadata.is_file() || metadata.mode() & 0o111 == 0 {
        return Err("the Python interpreter is not an executable file");
    }
    Ok(path.to_path_buf())
}

pub(super) struct Interpreter {
    prefix: String,
    base: String,
    version: Version,
    implementation: String,
    executable: PathBuf,
}

impl Interpreter {
    fn check_requirements(
        &self,
        pin: Option<&str>,
        requires: Option<&str>,
    ) -> Result<(), &'static str> {
        if let Some(requires) = requires {
            let specs = VersionSpecifiers::from_str(requires)
                .map_err(|_| "requires-python cannot be parsed")?;
            if !specs.contains(&self.version) {
                return Err("Workspace Python does not satisfy requires-python");
            }
        }
        if let Some(pin) = pin.filter(|pin| !path_request(pin)) {
            request_satisfied(pin, &self.version, &self.implementation)?;
        }
        Ok(())
    }
}

pub(super) enum Requires {
    Absent,
    Spec(String),
    Invalid,
}

pub(super) fn read_pin(root: &Path) -> Result<Option<String>, &'static str> {
    let Some(text) = read_optional_file(&root.join(".python-version"))
        .map_err(|()| ".python-version cannot be read")?
    else {
        return Ok(None);
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        // uv can write a bare relative directory name (without ./). Versions
        // keep their meaning even when a same-named directory exists.
        if !path_request(line) && parse_request(line).is_err() && root.join(line).is_dir() {
            // Preserve authored directory links for the fixed witness.
            return Ok(Some(root.join(line).display().to_string()));
        }
        return Ok(Some(line.to_owned()));
    }
    Ok(None)
}

pub(super) fn read_optional_file(path: &Path) -> Result<Option<String>, ()> {
    if std::fs::symlink_metadata(path).is_err_and(|e| e.kind() == std::io::ErrorKind::NotFound) {
        return Ok(None);
    }
    // Shared bounded reads open with O_NONBLOCK and validate the opened FD.
    let bytes = crate::bounded_file::read_bounded(path).map_err(|_| ())?;
    String::from_utf8(bytes).map(Some).map_err(|_| ())
}

pub(super) fn read_requires(root: &Path) -> Requires {
    let text = match read_optional_file(&root.join("pyproject.toml")) {
        Ok(Some(text)) => text,
        Ok(None) => return Requires::Absent,
        Err(()) => return Requires::Invalid,
    };
    let Ok(document) = text.parse::<toml_edit::DocumentMut>() else {
        return Requires::Invalid;
    };
    let Some(project) = document.get("project") else {
        return Requires::Absent;
    };
    match project.get("requires-python") {
        None => Requires::Absent,
        Some(value) => value
            .as_str()
            .filter(|spec| VersionSpecifiers::from_str(spec).is_ok())
            .map_or(Requires::Invalid, |spec| Requires::Spec(spec.to_owned())),
    }
}

pub(super) fn find_uv(host: &ExecutionHost, workspace: &Path) -> Option<PathBuf> {
    let path = host.entries().get("PATH")?;
    let venv_bin = workspace.join(".venv").join("bin");
    for directory in path.split(':') {
        if directory.is_empty() {
            continue;
        }
        let directory = PathBuf::from(directory);
        if directory == venv_bin {
            continue;
        }
        let candidate = directory.join("uv");
        let Ok(meta) = std::fs::metadata(&candidate) else {
            continue;
        };
        if meta.is_file() && meta.permissions().mode() & 0o111 != 0 {
            return Some(candidate);
        }
    }
    None
}

pub(super) fn isolated_discovery_args() -> [String; 3] {
    [
        "--no-project".to_owned(),
        "--directory".to_owned(),
        discovery_directory().display().to_string(),
    ]
}

pub(super) fn discovery_directory() -> PathBuf {
    static DIRECTORY: Mutex<Option<PathBuf>> = Mutex::new(None);
    let mut cached = DIRECTORY.lock().expect("workspace python discovery");
    if let Some(directory) = fresh_discovery(&mut cached, &discovery_parents()) {
        return directory;
    }
    std::env::temp_dir().join(format!(
        "rustx-uv-discovery-{}-{}",
        std::process::id(),
        monotonic_nanos()
    ))
}

fn discovery_parents() -> Vec<PathBuf> {
    let mut parents = vec![std::env::temp_dir()];
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(runtime);
        if path.is_absolute() && path.is_dir() {
            parents.push(path);
        }
    }
    let shm = PathBuf::from("/dev/shm");
    if shm.is_dir() {
        parents.push(shm);
    }
    parents
}

/// Keeps a discovery directory only after it exists. A failed create is not
/// remembered, and a deleted directory is replaced on the next use.
pub(super) fn fresh_discovery(
    cached: &mut Option<PathBuf>,
    parents: &[PathBuf],
) -> Option<PathBuf> {
    if let Some(directory) = cached.as_ref().filter(|directory| directory.is_dir()) {
        return Some(directory.clone());
    }
    let created = create_private_directory(parents, "rustx-uv-discovery")?;
    *cached = Some(created.clone());
    Some(created)
}

pub(super) fn uv_can_create(text: &str) -> bool {
    let Ok(version) = Version::from_str(text) else {
        return false;
    };
    let Ok(minimum) = Version::from_str(UV_MINIMUM) else {
        return false;
    };
    version >= minimum
}

/// Translate partial release requests to PEP 440 equality patterns and let uv
/// perform selection over the conjunction. This does not implement a solver.
pub(super) fn creation_request(
    root: &Path,
    pin: Option<&str>,
    requires: Option<&str>,
) -> Result<String, &'static str> {
    let Some(pin) = pin else {
        return Ok(requires
            .filter(|spec| !spec.trim().is_empty())
            .unwrap_or("default")
            .to_owned());
    };
    if path_request(pin) {
        return Ok(absolute_pin(root, pin));
    }
    let Some(requires) = requires else {
        return Ok(pin.to_owned());
    };
    let parsed = parse_request(pin).map_err(|()| ".python-version cannot be verified")?;
    let (implementation, pinned): (_, VersionSpecifiers) = match parsed {
        ParsedRequest::Spec {
            implementation,
            spec,
        } => (
            implementation,
            VersionSpecifiers::from_str(&spec).map_err(|_| ".python-version cannot be verified")?,
        ),
        ParsedRequest::Release {
            implementation,
            version,
        } => {
            let specifier =
                if version.release().len() < 3 && version == Version::new(version.release()) {
                    VersionSpecifier::equals_star_version(version)
                } else {
                    VersionSpecifier::equals_version(version)
                };
            (implementation, std::iter::once(specifier).collect())
        }
    };
    let additional =
        VersionSpecifiers::from_str(requires).map_err(|_| "requires-python cannot be parsed")?;
    let combined: VersionSpecifiers = pinned.into_iter().chain(additional).collect();
    Ok(implementation.map_or_else(|| combined.to_string(), |name| format!("{name}@{combined}")))
}

pub(super) fn exact_pin_conflicts(pin: &str, requires: &str) -> bool {
    let Ok(ParsedRequest::Release { version, .. }) = parse_request(pin) else {
        return false;
    };
    if version.release().len() < 3 {
        return false;
    }
    VersionSpecifiers::from_str(requires).is_ok_and(|specs| !specs.contains(&version))
}

pub(super) fn path_request(pin: &str) -> bool {
    pin.contains('/') || pin.contains('\\')
}

/// A path request is bound to the Workspace root. uv's `--directory` must not
/// reinterpret `./pinned-python` against the private discovery directory.
pub(super) fn absolute_pin(root: &Path, pin: &str) -> String {
    if !path_request(pin) {
        return pin.to_owned();
    }
    let requested = Path::new(pin);
    let absolute = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        root.join(requested)
    };
    std::fs::canonicalize(&absolute)
        .unwrap_or(absolute)
        .display()
        .to_string()
}

const IMPLEMENTATIONS: &[&str] = &["cpython", "pypy", "pyston", "graalpy", "graalpython"];

pub(super) enum ParsedRequest {
    Spec {
        implementation: Option<String>,
        spec: String,
    },
    Release {
        implementation: Option<String>,
        version: Version,
    },
}

pub(super) fn parse_request(pin: &str) -> Result<ParsedRequest, ()> {
    let (implementation, rest) = split_implementation(pin)?;
    if rest
        .chars()
        .any(|c| matches!(c, '>' | '<' | '=' | '~' | ','))
    {
        if VersionSpecifiers::from_str(rest).is_err() {
            return Err(());
        }
        return Ok(ParsedRequest::Spec {
            implementation,
            spec: rest.to_owned(),
        });
    }
    let version = Version::from_str(rest).map_err(|_| ())?;
    Ok(ParsedRequest::Release {
        implementation,
        version,
    })
}

fn split_implementation(pin: &str) -> Result<(Option<String>, &str), ()> {
    let mut best: Option<&str> = None;
    for name in IMPLEMENTATIONS {
        let Some(tail) = pin.get(name.len()..) else {
            continue;
        };
        if !pin[..name.len()].eq_ignore_ascii_case(name) {
            continue;
        }
        if !(tail.starts_with('-') || tail.starts_with('@')) {
            continue;
        }
        if best.is_none_or(|current| name.len() > current.len()) {
            best = Some(name);
        }
    }
    if let Some(name) = best {
        let tail = pin[name.len()..]
            .strip_prefix('-')
            .or_else(|| pin[name.len()..].strip_prefix('@'))
            .unwrap_or("");
        if tail.is_empty() {
            return Err(());
        }
        return Ok((Some((*name).to_owned()), tail));
    }
    if let Some(head_end) = pin.find(|character: char| {
        character.is_ascii_digit() || matches!(character, '>' | '<' | '=' | '~' | ',')
    }) {
        let head = pin[..head_end].trim_end_matches(['-', '@']);
        if !head.is_empty()
            && head
                .chars()
                .all(|character| character.is_ascii_alphabetic())
        {
            return Err(());
        }
    }
    Ok((None, pin))
}

fn request_satisfied(
    pin: &str,
    version: &Version,
    implementation: &str,
) -> Result<(), &'static str> {
    let request = parse_request(pin).map_err(|()| ".python-version cannot be verified")?;
    match request {
        ParsedRequest::Spec {
            implementation: required,
            spec,
        } => {
            if let Some(required) = required
                && !implementation.eq_ignore_ascii_case(&required)
            {
                return Err("Workspace Python does not satisfy .python-version");
            }
            let specs = VersionSpecifiers::from_str(&spec)
                .map_err(|_| ".python-version cannot be verified")?;
            if specs.contains(version) {
                Ok(())
            } else {
                Err("Workspace Python does not satisfy .python-version")
            }
        }
        ParsedRequest::Release {
            implementation: required,
            version: requested,
        } => {
            if let Some(required) = required {
                if implementation.is_empty() {
                    return Err("the Python implementation could not be verified");
                }
                if !implementation.eq_ignore_ascii_case(&required) {
                    return Err("Workspace Python does not satisfy .python-version");
                }
            }
            let matches = if requested.release().len() >= 3
                || requested != Version::new(requested.release())
            {
                requested == *version
            } else {
                version.release().starts_with(requested.release())
            };
            if matches {
                Ok(())
            } else {
                Err("Workspace Python does not satisfy .python-version")
            }
        }
    }
}

#[cfg(test)]
pub(super) fn pin_matches(pin: &str, actual: &Version) -> bool {
    request_satisfied(pin, actual, "cpython").is_ok()
}
