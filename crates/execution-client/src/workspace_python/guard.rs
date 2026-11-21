//! Guard scripts, staging, and read-only executable-sibling validation.

use std::fs::OpenOptions;
use std::io::Write;
#[cfg(test)]
use std::os::unix::fs::DirBuilderExt;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sha2::{Digest, Sha256};

use super::PythonSelection;
use super::interpreter::read_optional_file;
use super::owner::state_dir;

pub(super) fn monotonic_nanos() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| duration.as_nanos())
}

pub(super) fn bounded_diagnostic(summary: &str, stderr: &str) -> String {
    let detail = shell_line(stderr);
    if detail.is_empty() {
        summary.to_owned()
    } else {
        format!("{summary}: {detail}")
    }
}

pub(super) fn guarded(diagnostic: &str) -> PythonSelection {
    selection_from_guard(ensure_guard(diagnostic), diagnostic)
}

pub(super) fn selection_from_guard(specific: Option<PathBuf>, diagnostic: &str) -> PythonSelection {
    finish_guard(specific.or_else(fallback_guard), diagnostic)
}

pub(super) fn finish_guard(created: Option<PathBuf>, diagnostic: &str) -> PythonSelection {
    directory_guard(created.or_else(provisioned_guard), diagnostic)
}

pub(super) fn directory_guard(directory: Option<PathBuf>, diagnostic: &str) -> PythonSelection {
    match directory {
        Some(directory) => PythonSelection {
            path_prefix: vec![directory],
            virtual_env: None,
            diagnostic: diagnostic.to_owned(),
            guard_unavailable: false,
        },
        None => PythonSelection {
            path_prefix: Vec::new(),
            virtual_env: None,
            diagnostic: diagnostic.to_owned(),
            guard_unavailable: true,
        },
    }
}

pub(super) const GUARD_PYTHON: &[u8] = include_bytes!("../../assets/python-guard/python");
pub(super) const GUARD_PYTHON3: &[u8] = include_bytes!("../../assets/python-guard/python3");

/// `python-guard` installed beside the running executable. The source checkout
/// is not consulted: a relocated binary keeps this directory next to itself,
/// and both scripts must match the bytes embedded in that binary.
pub(super) fn provisioned_guard() -> Option<PathBuf> {
    let executable = std::env::current_exe().ok()?;
    provisioned_guard_for(&executable)
}

pub(super) fn provisioned_guard_for(executable: &Path) -> Option<PathBuf> {
    if let Some(directory) = guard_beside_executable(executable) {
        return Some(directory);
    }
    let canonical = std::fs::canonicalize(executable).ok()?;
    if canonical == executable {
        return None;
    }
    guard_beside_executable(&canonical)
}

pub(super) fn guard_beside_executable(executable: &Path) -> Option<PathBuf> {
    validated_guard_directory(&executable.parent()?.join("python-guard"))
}

pub(super) fn validated_guard_directory(directory: &Path) -> Option<PathBuf> {
    for (name, expected) in [("python", GUARD_PYTHON), ("python3", GUARD_PYTHON3)] {
        let path = directory.join(name);
        let meta = std::fs::metadata(&path).ok()?;
        if !meta.is_file() || meta.permissions().mode() & 0o111 == 0 {
            return None;
        }
        if read_optional_file(&path).ok()??.as_bytes() != expected {
            return None;
        }
    }
    Some(directory.to_path_buf())
}

/// The physical guard receives its one diagnostic through this environment entry.
pub const GUARD_MESSAGE_KEY: &str = "RUSTX_PYTHON_GUARD_MESSAGE";

static GUARD: Mutex<Option<tempfile::TempDir>> = Mutex::new(None);

pub(super) fn fallback_guard() -> Option<PathBuf> {
    cached_guard(
        &mut GUARD.lock().expect("workspace python guard"),
        &fallback_parents(),
    )
}

pub(super) fn cleanup_fallback() -> std::io::Result<()> {
    GUARD
        .lock()
        .expect("workspace python guard")
        .take()
        .map_or(Ok(()), tempfile::TempDir::close)
}

pub(super) fn cached_guard(
    cached: &mut Option<tempfile::TempDir>,
    parents: &[PathBuf],
) -> Option<PathBuf> {
    if let Some(valid) = cached
        .as_ref()
        .and_then(|dir| validated_guard_directory(dir.path()))
    {
        return Some(valid);
    }
    *cached = install_process_guard(parents, "Workspace Python is not ready");
    cached.as_ref().map(|directory| directory.path().to_owned())
}

fn fallback_parents() -> Vec<PathBuf> {
    let mut parents = vec![std::env::temp_dir()];
    if let Some(runtime) = std::env::var_os("XDG_RUNTIME_DIR") {
        let path = PathBuf::from(runtime);
        if path.is_absolute() {
            parents.push(path);
        }
    }
    let shm = PathBuf::from("/dev/shm");
    if shm.is_dir() {
        parents.push(shm);
    }
    if let Some(parent) = std::env::current_exe()
        .ok()
        .as_deref()
        .and_then(Path::parent)
    {
        parents.push(parent.to_path_buf());
    }
    parents
}

pub(super) fn install_process_guard(
    parents: &[PathBuf],
    diagnostic: &str,
) -> Option<tempfile::TempDir> {
    parents.iter().find_map(|parent| {
        let directory = tempfile::Builder::new()
            .prefix("rustx-python-guard-")
            .tempdir_in(parent)
            .ok()?;
        install_guard(directory.path(), diagnostic)?;
        Some(directory)
    })
}

#[cfg(test)]
pub(super) fn claim_fresh_directory(directory: &Path) -> Option<PathBuf> {
    std::fs::DirBuilder::new()
        .mode(0o700)
        .create(directory)
        .ok()?;
    Some(directory.to_path_buf())
}

pub(super) fn ensure_guard(diagnostic: &str) -> Option<PathBuf> {
    let directory = state_dir()?.join("guards").join(guard_name(diagnostic));
    install_guard(&directory, diagnostic)
}

pub(super) fn install_guard(directory: &Path, diagnostic: &str) -> Option<PathBuf> {
    std::fs::create_dir_all(directory).ok()?;
    let _ = diagnostic; // The immutable script reads the per-command diagnostic.
    for (name, bytes) in [("python", GUARD_PYTHON), ("python3", GUARD_PYTHON3)] {
        let path = directory.join(name);
        if path.exists() {
            continue;
        }
        let mut file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o755)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(_) => return None,
        };
        file.write_all(bytes).ok()?;
    }
    validated_guard_directory(directory)
}

fn guard_name(diagnostic: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(diagnostic.as_bytes());
    format!("{:x}", hasher.finalize())[..32].to_owned()
}

fn shell_line(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || " .,:-_".contains(c) {
                c
            } else {
                ' '
            }
        })
        .take(180)
        .collect()
}
