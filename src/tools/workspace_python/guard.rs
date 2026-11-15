//! Guard scripts, staging, and read-only executable-sibling validation.

use std::fs::OpenOptions;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use sha2::{Digest, Sha256};

use super::PythonSelection;
use super::interpreter::read_optional_file;
use super::owner::state_dir;

pub(super) fn create_private_directory(parents: &[PathBuf], prefix: &str) -> Option<PathBuf> {
    for parent in parents {
        if !parent.is_dir() {
            continue;
        }
        for _ in 0..4 {
            let directory = parent.join(format!(
                "{prefix}-{}-{}",
                std::process::id(),
                monotonic_nanos()
            ));
            if claim_fresh_directory(&directory).is_some() {
                return Some(directory);
            }
        }
    }
    None
}

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

pub(super) const GUARD_PYTHON: &[u8] = include_bytes!("../../../assets/python-guard/python");
pub(super) const GUARD_PYTHON3: &[u8] = include_bytes!("../../../assets/python-guard/python3");

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

pub(crate) const GUARD_MESSAGE_KEY: &str = "RUSTX_PYTHON_GUARD_MESSAGE";

pub(super) fn fallback_guard() -> Option<PathBuf> {
    static GUARD: Mutex<Option<PathBuf>> = Mutex::new(None);
    cached_guard(
        &mut GUARD.lock().expect("workspace python guard"),
        &fallback_parents(),
    )
}

pub(super) fn cached_guard(cached: &mut Option<PathBuf>, parents: &[PathBuf]) -> Option<PathBuf> {
    if let Some(valid) = cached.as_deref().and_then(validated_guard_directory) {
        return Some(valid);
    }
    *cached = install_process_guard(parents, "Workspace Python is not ready");
    cached.clone()
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

pub(super) fn install_process_guard(parents: &[PathBuf], diagnostic: &str) -> Option<PathBuf> {
    for parent in parents {
        if !parent.is_dir() {
            continue;
        }
        for _ in 0..4 {
            let nanos = monotonic_nanos();
            let directory =
                parent.join(format!("rustx-python-guard-{}-{nanos}", std::process::id()));
            if claim_fresh_directory(&directory).is_none() {
                continue;
            }
            if let Some(directory) = install_guard(&directory, diagnostic) {
                return Some(directory);
            }
        }
    }
    None
}

pub(super) fn claim_fresh_directory(directory: &Path) -> Option<PathBuf> {
    std::fs::create_dir(directory).ok()?;
    let _ = std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700));
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
