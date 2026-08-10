//! Deterministic content-derived Skill package version identity (M6).
//!
//! [`package_version_id`] computes the `SkillVersionId` of an accepted
//! Skill package as SHA-256 over the complete package content in the stable
//! textual form `sha256:<64 lowercase hex characters>`.
//!
//! The digest covers every regular package file recursively:
//!
//! ```text
//! rustx-skill-v1
//! path=<workspace/package-relative path>
//! len=<file length>
//! <raw file bytes>
//! ```
//!
//! Files are processed in deterministic sorted workspace-relative path
//! order. Binary assets are hashed as raw bytes. The digest never includes
//! absolute host paths, directory/file mtimes, inode numbers, permissions,
//! wall-clock time, or filesystem enumeration order; because M6 rejects
//! package-internal symlinks, no symlink alias semantics are needed.
//!
//! `SkillVersionId` is distinct from `PythonEnvironmentDigest` and
//! `NodeEnvironmentDigest`: a description-only change yields a new Skill
//! version without changing environment identities when the dependency
//! inputs are unchanged.

use std::io::Read;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::runtime::identity::SkillVersionId;

/// The format/version domain separator of the Skill package digest.
const SKILL_VERSION_DOMAIN: &[u8] = b"rustx-skill-v1\n";

/// Computes the deterministic content-derived version identity of one
/// accepted Skill package.
///
/// `markdown_bytes` must be the exact bytes of `SKILL.md` read during
/// discovery, so the digest covers the exact accepted package state without
/// a second read of the primary instructions file.
///
/// # Errors
///
/// Returns a human-readable failure when a package file cannot be read.
pub fn package_version_id(
    package_root: &Path,
    files: &[PathBuf],
    markdown_bytes: &[u8],
) -> Result<SkillVersionId, String> {
    let mut hasher = Sha256::new();
    hasher.update(SKILL_VERSION_DOMAIN);
    for relative in files {
        let path = package_root.join(relative);
        let len = std::fs::metadata(&path)
            .map_err(|error| format!("cannot stat {path:?}: {error}"))?
            .len();
        hasher.update(format!("path={}\n", relative.display()));
        hasher.update(format!("len={len}\n"));
        if relative == Path::new(crate::skills::package::SKILL_MARKDOWN_FILE) {
            // The primary instructions file was already read during
            // discovery; hash those exact bytes so the digest covers the
            // exact accepted state.
            debug_assert_eq!(len, markdown_bytes.len() as u64);
            hasher.update(markdown_bytes);
            continue;
        }
        let mut file = std::fs::File::open(&path)
            .map_err(|error| format!("cannot open {path:?}: {error}"))?;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = file
                .read(&mut buffer)
                .map_err(|error| format!("cannot read {path:?}: {error}"))?;
            if read == 0 {
                break;
            }
            hasher.update(&buffer[..read]);
        }
    }
    let digest = hasher.finalize();
    let mut hex = String::with_capacity(71);
    hex.push_str("sha256:");
    for byte in digest {
        hex.push_str(&format!("{byte:02x}"));
    }
    Ok(SkillVersionId::new(hex))
}

#[cfg(test)]
mod tests {
    use super::package_version_id;
    use std::path::{Path, PathBuf};

    fn package_dir() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let package = dir.path().join("skills").join("pdf");
        std::fs::create_dir_all(&package).expect("package dir");
        std::fs::write(
            package.join("SKILL.md"),
            "---\nname: pdf\ndescription: A pdf skill\n---\nbody\n",
        )
        .expect("SKILL.md");
        (dir, package)
    }

    fn files_of(package: &Path) -> Vec<PathBuf> {
        let mut files = Vec::new();
        for entry in std::fs::read_dir(package).expect("read package") {
            let entry = entry.expect("entry");
            if entry.file_type().expect("type").is_file() {
                files.push(
                    entry
                        .path()
                        .strip_prefix(package)
                        .expect("relative")
                        .to_path_buf(),
                );
            }
        }
        files.sort();
        files
    }

    /// Same package bytes produce the same version identity.
    #[test]
    fn same_bytes_same_version_id() {
        let (_dir, package) = package_dir();
        let markdown = std::fs::read(package.join("SKILL.md")).expect("read");
        let first = package_version_id(&package, &files_of(&package), &markdown).expect("id");
        let second = package_version_id(&package, &files_of(&package), &markdown).expect("id");
        assert_eq!(first, second);
        assert!(first.as_str().starts_with("sha256:"));
        assert_eq!(first.as_str().len(), 7 + 64);
    }

    /// A body change changes the version identity.
    #[test]
    fn body_change_changes_version_id() {
        let (_dir, package) = package_dir();
        let markdown = std::fs::read(package.join("SKILL.md")).expect("read");
        let before = package_version_id(&package, &files_of(&package), &markdown).expect("id");
        std::fs::write(package.join("SKILL.md"), "---\nname: pdf\ndescription: A pdf skill\n---\nchanged body\n")
            .expect("write");
        let markdown = std::fs::read(package.join("SKILL.md")).expect("read");
        let after = package_version_id(&package, &files_of(&package), &markdown).expect("id");
        assert_ne!(before, after);
    }
}
