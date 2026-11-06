//! Deterministic content-derived Skill package version identity (M6).
//!
//! [`inspect_package`] computes the `SkillVersionId` of an accepted
//! Skill package as SHA-256 over the complete package content in the stable
//! textual form `sha256:<64 lowercase hex characters>`.
//!
//! The digest covers every regular package file recursively:
//!
//! ```text
//! rustx-skill-v2
//! <u64 big-endian path byte length><raw package-relative OS path bytes>
//! <u64 big-endian file byte length>
//! <SHA-256 of complete file bytes, verified by a second content read>
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

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::runtime::identity::SkillVersionId;

/// The format/version domain separator of the Skill package digest.
const SKILL_VERSION_DOMAIN: &[u8] = b"rustx-skill-v2\n";

/// Computes identity over complete accepted bytes with fixed-buffer I/O.
/// No package or individual-file byte admission limit applies.
/// # Errors
/// Rejects unsafe paths, non-regular files, concurrent edits and I/O failures.
#[cfg(test)]
pub fn package_version_id(
    package_root: &Path,
    files: &[PathBuf],
) -> Result<SkillVersionId, String> {
    inspect_package(
        package_root,
        files,
        &crate::runtime::CancellationSignal::new(),
    )
    .map(|(identity, _)| identity)
}

/// Capture only the frontmatter while hashing the very same SKILL.md read.
/// Body bytes are validated as UTF-8 and hashed, never retained.
pub(crate) fn inspect_package(
    root: &Path,
    files: &[PathBuf],
    cancellation: &crate::runtime::CancellationSignal,
) -> Result<(SkillVersionId, Vec<u8>), String> {
    use crate::package_io::{PackageDirectory, stream};
    let directory = PackageDirectory::open(root).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    hasher.update(SKILL_VERSION_DOMAIN);
    let mut header = Vec::new();
    for relative in files {
        let markdown = relative == Path::new(crate::skills::package::SKILL_MARKDOWN_FILE);
        let mut delimiters = 0;
        let mut line_start = 0;
        let mut header_finished = false;
        let mut utf8 = Vec::new();
        let evidence = stream(&directory, relative, cancellation, |bytes| {
            if markdown {
                utf8.extend_from_slice(bytes);
                match std::str::from_utf8(&utf8) {
                    Ok(_) => utf8.clear(),
                    Err(error) if error.error_len().is_none() => {
                        utf8.drain(..error.valid_up_to());
                    }
                    Err(_) => return Err(std::io::Error::other("SKILL.md is not valid UTF-8")),
                }
                for &byte in bytes {
                    if header_finished {
                        break;
                    }
                    header.push(byte);
                    // The first line can only be one of these delimiters.
                    // Reject an impossible prefix before retaining an arbitrarily
                    // long malformed first line from an unselected candidate.
                    if line_start == 0
                        && !b"---\n".starts_with(&header)
                        && !b"---\r\n".starts_with(&header)
                    {
                        return Err(std::io::Error::other(
                            "SKILL.md must start with a frontmatter delimiter",
                        ));
                    }
                    if byte == b'\n' {
                        let line = &header[line_start..];
                        if line == b"---\n" || line == b"---\r\n" {
                            delimiters += 1;
                        }
                        if line_start == 0 && delimiters == 0 || delimiters == 2 {
                            header_finished = true;
                        }
                        line_start = header.len();
                    }
                }
            }
            Ok(())
        })
        .map_err(|error| format!("cannot inspect {}: {error}", root.join(relative).display()))?;
        let path_bytes = relative.as_os_str().as_encoded_bytes();
        hasher.update((path_bytes.len() as u64).to_be_bytes());
        hasher.update(path_bytes);
        hasher.update(evidence.metadata.len().to_be_bytes());
        hasher.update(evidence.digest);
        if !utf8.is_empty() {
            return Err("SKILL.md is not valid UTF-8".into());
        }
    }
    Ok((
        SkillVersionId::new(format!("sha256:{:x}", hasher.finalize())),
        header,
    ))
}

#[cfg(test)]
mod tests {
    use super::package_version_id;
    use std::path::{Path, PathBuf};

    fn package_dir() -> (tempfile::TempDir, std::path::PathBuf) {
        let dir = tempfile::tempdir().expect("temp dir");
        let package = dir
            .path()
            .canonicalize()
            .unwrap()
            .join("skills")
            .join("pdf");
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
        let first = package_version_id(&package, &files_of(&package)).expect("id");
        let second = package_version_id(&package, &files_of(&package)).expect("id");
        assert_eq!(first, second);
        assert!(first.as_str().starts_with("sha256:"));
        assert_eq!(first.as_str().len(), 7 + 64);
    }

    /// A body change changes the version identity.
    #[test]
    fn body_change_changes_version_id() {
        let (_dir, package) = package_dir();
        let before = package_version_id(&package, &files_of(&package)).expect("id");
        std::fs::write(
            package.join("SKILL.md"),
            "---\nname: pdf\ndescription: A pdf skill\n---\nchanged body\n",
        )
        .expect("write");
        let after = package_version_id(&package, &files_of(&package)).expect("id");
        assert_ne!(before, after);
    }
}

#[cfg(test)]
mod large_tests {
    use super::*;
    use crate::runtime::CancellationSignal;
    use std::io::Write;

    #[test]
    fn issue527_invalid_newline_free_skill_stops_at_the_first_chunk() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let package = root.join("invalid");
        std::fs::create_dir(&package).unwrap();
        let mut file = std::fs::File::create(package.join("SKILL.md")).unwrap();
        file.write_all(b"x").unwrap();
        file.set_len(256 * 1024 * 1024).unwrap();
        crate::package_io::STREAMED_FILE_BYTES.with(|count| count.set(0));
        let outcome = crate::skills::SkillDiscovery::user_root(root).discover();
        assert!(outcome.packages.is_empty());
        assert_eq!(outcome.diagnostics.len(), 1);
        crate::package_io::STREAMED_FILE_BYTES.with(|count| {
            assert_eq!(count.get(), crate::package_io::CHUNK_BYTES as u64);
        });
    }

    #[test]
    fn issue527_large_assets_and_markdown_stream_through_discovery_and_materialization() {
        for size_mib in [17_u64, 64, 256] {
            let temporary = tempfile::tempdir().unwrap();
            let root = temporary.path().canonicalize().unwrap();
            let package = root.join("large");
            std::fs::create_dir(&package).unwrap();
            let mut markdown = std::fs::File::create(package.join("SKILL.md")).unwrap();
            markdown
                .write_all(b"---\nname: large\ndescription: Large local resources\n---\n")
                .unwrap();
            if size_mib == 17 {
                let mut body = vec![b'x'; 64 * 1024];
                *body.last_mut().unwrap() = b'\n';
                for _ in 0..17 * 16 {
                    markdown.write_all(&body).unwrap();
                }
            }
            drop(markdown);
            std::fs::File::create(package.join("asset.bin"))
                .unwrap()
                .set_len(size_mib * 1024 * 1024)
                .unwrap();
            let start = std::time::Instant::now();
            let outcome = crate::skills::SkillDiscovery::user_root(root.clone()).discover();
            assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
            let accepted = &outcome.packages[0];
            let (identity, header) =
                inspect_package(&package, accepted.files(), &CancellationSignal::new()).unwrap();
            assert_eq!(&identity, accepted.version_id());
            assert!(header.len() < 128, "discovery retains frontmatter only");
            let destination = root.join("child/large");
            let binding = crate::protocol::manifest::SkillBinding {
                skill_id: accepted.id().clone(),
                version_id: identity.clone(),
            };
            crate::skills::materialization::materialize_skill(
                &binding,
                &package,
                accepted.files(),
                &destination,
                &CancellationSignal::new(),
            )
            .unwrap();
            assert_eq!(
                std::fs::metadata(destination.join("asset.bin"))
                    .unwrap()
                    .len(),
                size_mib * 1024 * 1024
            );
            std::fs::OpenOptions::new()
                .write(true)
                .open(package.join("asset.bin"))
                .unwrap()
                .write_all(b"changed")
                .unwrap();
            assert_ne!(
                inspect_package(&package, accepted.files(), &CancellationSignal::new())
                    .unwrap()
                    .0,
                identity
            );
            eprintln!(
                "issue527 Skill {size_mib} MiB: {:?}; retained header={} bytes; I/O buffer={} bytes",
                start.elapsed(),
                header.len(),
                crate::package_io::CHUNK_BYTES
            );
        }
    }
}
