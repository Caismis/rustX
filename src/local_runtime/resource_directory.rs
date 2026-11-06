//! Small bounded directory IO shared by the fixed canonical resource readers.
use crate::runtime::resources::{RuntimeResourceLoadError, validate_project_resource_path};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

mod evidence;

/// Capture equality uses complete bytes, or the independently complete finite
/// domain fence carried by an incomplete observation. The incomplete tree's
/// fresh observation identity itself never certifies equality.
pub(crate) fn capture_token(observation: &str) -> Option<&str> {
    if !observation.starts_with("incomplete:") {
        return Some(observation);
    }
    let token = observation.split(':').nth(2)?;
    token
        .strip_prefix("accepted-")
        .filter(|digest| digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .map(|_| token)
}

pub(crate) fn same_capture(before: &str, after: &str) -> bool {
    capture_token(before).is_some_and(|token| Some(token) == capture_token(after))
}

/// The most authored entries of any kind a canonical resource directory
/// holds, besides rustX's one lock directory.
pub(crate) const MAX_DIRECTORY_ENTRIES: usize = 1024;

pub(crate) fn entries(
    boundary: &Path,
    root: &Path,
) -> Result<Vec<PathBuf>, RuntimeResourceLoadError> {
    let fail = |e: String| RuntimeResourceLoadError::new(e).at(root, "resources");
    validate_project_resource_path(boundary, root)?;
    match std::fs::symlink_metadata(root) {
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(fail(e.to_string())),
        Ok(meta) if meta.file_type().is_symlink() || !meta.is_dir() => {
            return Err(fail(
                "canonical resource root must be a directory, not a symlink".into(),
            ));
        }
        Ok(_) => {}
    }
    // rustX's lock directory is synchronization metadata, not an authored
    // entry: it is read past and never counted, so creating it on a first
    // write never changes what discovery admits. It is one fixed name, so the
    // read stays bounded at the authored bound plus that one entry.
    let paths = std::fs::read_dir(root)
        .map_err(|e| fail(e.to_string()))?
        .take(MAX_DIRECTORY_ENTRIES + 2)
        .map(|entry| entry.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| fail(e.to_string()))?;
    let paths: Vec<_> = paths
        .into_iter()
        .filter(|path| {
            !path
                .file_name()
                .is_some_and(super::settings::is_lock_directory)
        })
        .collect();
    if paths.len() > MAX_DIRECTORY_ENTRIES {
        return Err(fail("canonical resource root exceeds 1024 entries".into()));
    }
    let mut paths = paths;
    paths.sort();
    Ok(paths)
}

/// The most resources of one kind a canonical resource directory holds.
pub(crate) const MAX_RESOURCE_FILES: usize = 128;

pub(crate) fn files(
    boundary: &Path,
    root: &Path,
    extension: &str,
) -> Result<Vec<PathBuf>, RuntimeResourceLoadError> {
    let mut files = Vec::new();
    for path in entries(boundary, root)? {
        if path.extension().and_then(|v| v.to_str()) != Some(extension) {
            continue;
        }
        files.push(path);
    }
    if files.len() > MAX_RESOURCE_FILES {
        return Err(
            RuntimeResourceLoadError::new("canonical catalog exceeds 128 resources")
                .at(root, "resources"),
        );
    }
    Ok(files)
}

/// Read only a winning resource. Invalid Workspace identities still shadow User.
pub(crate) fn read_resource(
    boundary: &Path,
    path: &Path,
    byte_limit: u64,
) -> Result<Vec<u8>, RuntimeResourceLoadError> {
    validate_project_resource_path(boundary, path)?;
    let metadata = std::fs::symlink_metadata(path)
        .map_err(|e| RuntimeResourceLoadError::new(e.to_string()).at(path, "resources"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(RuntimeResourceLoadError::new(
            "resource must be a regular file, not a symlink",
        )
        .at(path, "resources"));
    }
    crate::bounded_file::read_with_limit(path, byte_limit)
        .map_err(|e| RuntimeResourceLoadError::new(e).at(path, "resources"))
}

/// The entries of a resource root (`.agents`) that configuration reads.
pub(crate) const SOURCE_ENTRIES: [&str; 5] = ["agents", "mcp.toml", "skills", "tools", "workflows"];

/// The names, directly under a resource root, that configuration reads: the
/// source entries and the guidance files. Anything else there — Session
/// uploads in particular — is not configuration input.
fn source_entry_names() -> impl Iterator<Item = &'static str> {
    SOURCE_ENTRIES
        .into_iter()
        .chain(crate::runtime::resources::PROJECT_CONTEXT_FILENAMES)
}

/// Revision of authored resource bytes, including malformed and shadowed entries.
/// This is change detection, not discovery or configuration authority. Symlinks
/// are hashed as links and never followed. Traversal is bounded even for invalid
/// unused trees, and writer locks/staging files do not count as authored input.
/// The root is read through the fixed names the loaders open
/// ([`source_entry_names`]), never by matching the spelling of what it lists:
/// each name resolves exactly as the loaders resolve it, on case-insensitive
/// volumes too, and Session uploads written beside them never count.
#[cfg(test)]
pub(crate) fn revision(root: &Path) -> String {
    revision_with_cancellation(
        root,
        crate::runtime::resources::SourceScope::User,
        &crate::runtime::CancellationSignal::new(),
    )
}

fn revision_walk(
    path: &Path,
    hash: &mut Sha256,
    remaining: &mut usize,
    bytes_left: &mut u64,
    depth: usize,
    cancellation: &crate::runtime::CancellationSignal,
) -> Result<(), String> {
    crate::package_io::check_cancelled(cancellation).map_err(|e| e.to_string())?;
    if *remaining == 0 || depth > 68 {
        return Err("resource traversal incomplete".into());
    }
    *remaining -= 1;
    let metadata = match std::fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            hash.update(b"absent");
            return Ok(());
        }
        Err(error) => return Err(error.to_string()),
    };
    if metadata.file_type().is_symlink() {
        hash.update(b"link:");
        hash.update(
            std::fs::read_link(path)
                .map_err(|e| e.to_string())?
                .as_os_str()
                .as_encoded_bytes(),
        );
    } else if metadata.is_dir() {
        hash.update(b"directory:");
        if depth == 0 {
            for name in source_entry_names() {
                hash.update(name.len().to_le_bytes());
                hash.update(name);
                revision_walk(
                    &path.join(name),
                    hash,
                    remaining,
                    bytes_left,
                    1,
                    cancellation,
                )?;
            }
            return Ok(());
        }
        let directory =
            crate::package_io::PackageDirectory::open(path).map_err(|e| e.to_string())?;
        // The global budget covers both 128-package collections at 4,096
        // entries each. It cannot hide a valid package at the old 16K cut.
        for name in directory.names(4096).map_err(|e| e.to_string())? {
            let name = name.as_os_str();
            if depth <= 1
                && (name.as_encoded_bytes().starts_with(b".tmp")
                    || super::settings::is_writer_artifact(name))
            {
                continue;
            }
            hash.update(name.as_encoded_bytes().len().to_le_bytes());
            hash.update(name.as_encoded_bytes());
            revision_walk(
                &path.join(name),
                hash,
                remaining,
                bytes_left,
                depth + 1,
                cancellation,
            )?;
        }
    } else if metadata.is_file() {
        if metadata.len() > *bytes_left {
            return Err("resource byte observation incomplete (64 MiB work budget)".into());
        }
        hash.update(b"file:");
        let parent = path.parent().ok_or("resource has no parent")?;
        let directory =
            crate::package_io::PackageDirectory::open(parent).map_err(|e| e.to_string())?;
        let evidence = crate::package_io::stream(
            &directory,
            Path::new(path.file_name().ok_or("resource has no name")?),
            cancellation,
            |bytes| {
                *bytes_left = bytes_left
                    .checked_sub(bytes.len() as u64)
                    .ok_or_else(|| std::io::Error::other("resource byte observation incomplete"))?;
                Ok(())
            },
        )
        .map_err(|e| e.to_string())?;
        hash.update(evidence.metadata.len().to_le_bytes());
        hash.update(evidence.digest);
    } else {
        hash.update(b"unsupported file type");
    }
    Ok(())
}

pub(crate) fn revision_with_cancellation(
    root: &Path,
    scope: crate::runtime::resources::SourceScope,
    cancellation: &crate::runtime::CancellationSignal,
) -> String {
    let mut hash = Sha256::new();
    // Bound arbitrary unused/malformed tree observation. Overflow is not an
    // admission refusal: accepted domain readers still inspect every accepted
    // byte for the independent complete fence below, without this work budget.
    match revision_walk(
        root,
        &mut hash,
        &mut 1_100_000,
        &mut (64 * 1024 * 1024),
        0,
        cancellation,
    ) {
        Ok(()) => format!("sha256:{:x}", hash.finalize()),
        // An incomplete inspection is explicitly not evidence of equality.
        // A later verification cannot certify an unchanged tree from a marker.
        Err(reason) => {
            let fence = evidence::capture(root, scope, cancellation)
                .unwrap_or_else(|_| "unavailable".into());
            format!("incomplete:{}:{fence}:{reason}", uuid::Uuid::now_v7())
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::os::unix::fs::MetadataExt;

    /// Session uploads and other entries configuration never reads leave the
    /// revision alone; a source entry or a guidance file changes it.
    #[test]
    fn only_configuration_entries_change_the_revision() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join(".agents");
        std::fs::create_dir_all(root.join("skills/guide")).unwrap();
        std::fs::write(root.join("skills/guide/SKILL.md"), b"A").unwrap();
        let initial = revision(&root);
        let uploads = root.join("uploads/ses_1/batch");
        std::fs::create_dir_all(&uploads).unwrap();
        std::fs::write(uploads.join("report.txt"), b"upload").unwrap();
        std::fs::write(root.join("notes.txt"), b"not read").unwrap();
        assert_eq!(revision(&root), initial, "uploads are not configuration");
        std::fs::write(root.join("AGENTS.md"), b"guidance").unwrap();
        let guided = revision(&root);
        assert_ne!(guided, initial, "a guidance file is configuration");
        std::fs::write(root.join("skills/guide/SKILL.md"), b"B").unwrap();
        assert_ne!(revision(&root), guided, "a source entry is configuration");
    }

    /// On a case-sensitive volume the loaders never open `Skills`, so its
    /// content is not configuration and leaves the revision alone.
    #[test]
    fn a_case_variant_the_loaders_never_open_is_not_configuration() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join(".agents");
        std::fs::create_dir_all(root.join("Skills/guide")).unwrap();
        std::fs::write(root.join("Skills/guide/SKILL.md"), b"A").unwrap();
        let initial = revision(&root);
        std::fs::write(root.join("Skills/guide/SKILL.md"), b"B").unwrap();
        assert_eq!(revision(&root), initial);
    }

    #[test]
    fn case_sensitive_discovery_counts_lock_directory_case_variants() {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().canonicalize().unwrap();
        let locks = root.join(".rustx-locks");
        std::fs::create_dir(&locks).unwrap();
        let variants = [root.join(".RUSTX-LOCKS"), root.join(".RuStX-LoCkS")];
        for path in &variants {
            std::fs::File::create_new(path)
                .expect("this regression requires a verified case-sensitive filesystem");
            let metadata = std::fs::symlink_metadata(path).unwrap();
            assert!(metadata.is_file());
            assert_ne!(metadata.ino(), std::fs::metadata(&locks).unwrap().ino());
        }
        assert_ne!(
            std::fs::metadata(&variants[0]).unwrap().ino(),
            std::fs::metadata(&variants[1]).unwrap().ino()
        );
        for index in 0..MAX_DIRECTORY_ENTRIES - variants.len() {
            std::fs::write(root.join(format!("ordinary-{index}")), b"ordinary").unwrap();
        }

        let discovered = entries(&root, &root).unwrap();
        assert_eq!(discovered.len(), MAX_DIRECTORY_ENTRIES);
        assert!(!discovered.contains(&locks));
        for path in &variants {
            assert!(discovered.contains(path));
        }

        std::fs::write(root.join("overflow"), b"ordinary").unwrap();
        let error = entries(&root, &root).unwrap_err();
        assert!(
            error.to_string().contains("exceeds 1024 entries"),
            "{error}"
        );
        assert_eq!(
            std::fs::read_dir(&root).unwrap().count(),
            MAX_DIRECTORY_ENTRIES + 2,
            "discovery preserves the exact metadata directory and all 1,025 authored entries"
        );
    }
}

#[cfg(test)]
mod revision_tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};

    #[test]
    fn issue527_missing_required_files_skip_huge_assets_in_discovery_and_evidence() {
        use crate::capabilities::ToolSourceId;
        use crate::runtime::resources::SourceScope;
        for (server, requirements) in [(false, true), (true, false), (false, false)] {
            let temporary = tempfile::tempdir().unwrap();
            let workspace = temporary.path().canonicalize().unwrap();
            let user = workspace.join("user/.agents");
            let lower = user.join("tools/demo");
            let higher = workspace.join(".agents/tools/demo");
            std::fs::create_dir_all(&lower).unwrap();
            std::fs::create_dir_all(&higher).unwrap();
            std::fs::write(lower.join("server.py"), "# valid lower candidate\n").unwrap();
            std::fs::write(lower.join("requirements.txt"), "").unwrap();
            if server {
                std::fs::write(higher.join("server.py"), "# server\n").unwrap();
            }
            if requirements {
                std::fs::write(higher.join("requirements.txt"), "").unwrap();
            }
            std::fs::File::create(higher.join("asset.bin"))
                .unwrap()
                .set_len(1_u64 << 40)
                .unwrap();
            crate::package_io::STREAMED_FILE_BYTES.with(|count| count.set(0));
            assert!(
                crate::tools::python::discover_package(
                    &higher,
                    "demo",
                    &crate::runtime::CancellationSignal::new()
                )
                .is_err()
            );
            let catalog =
                crate::local_runtime::managed_python_resources::discover(Some(&workspace), &user)
                    .unwrap();
            let identity = ToolSourceId::ManagedPython("demo".into());
            assert!(catalog.packages()[&identity].is_err());
            assert_eq!(catalog.locations[&identity].shadowed.as_ref(), Some(&lower));
            crate::package_io::STREAMED_FILE_BYTES.with(|count| assert_eq!(count.get(), 0));
            let observation = revision_with_cancellation(
                &workspace.join(".agents"),
                SourceScope::Workspace,
                &crate::runtime::CancellationSignal::new(),
            );
            assert!(observation.starts_with("incomplete:"));
            assert!(capture_token(&observation).is_some());
            crate::package_io::STREAMED_FILE_BYTES.with(|count| count.set(0));
            evidence::capture(
                &workspace.join(".agents"),
                SourceScope::Workspace,
                &crate::runtime::CancellationSignal::new(),
            )
            .unwrap();
            crate::package_io::STREAMED_FILE_BYTES.with(|count| assert_eq!(count.get(), 0));
        }
    }
    #[test]
    fn issue527_invalid_collections_and_absent_collections_have_distinct_domain_fences() {
        use crate::runtime::{CancellationSignal, resources::SourceScope};
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap().join(".agents");
        let mut invalid = root.join("skills/unused");
        for _ in 0..70 {
            invalid.push("deep");
        }
        std::fs::create_dir_all(invalid).unwrap();
        for scope in [SourceScope::User, SourceScope::Workspace] {
            for family in ["agents", "tools", "workflows"] {
                let path = root.join(family);
                std::fs::write(&path, "malformed collection").unwrap();
                let invalid = revision_with_cancellation(&root, scope, &CancellationSignal::new());
                std::fs::remove_file(path).unwrap();
                let absent = revision_with_cancellation(&root, scope, &CancellationSignal::new());
                assert!(invalid.starts_with("incomplete:"));
                assert!(absent.starts_with("incomplete:"));
                assert!(capture_token(&invalid).is_some(), "{invalid}");
                assert!(capture_token(&absent).is_some(), "{absent}");
                assert!(!same_capture(&invalid, &absent), "{scope:?}/{family}");
            }
        }
    }

    #[test]
    fn issue527_unknown_huge_files_do_not_consume_unbounded_observation_io() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let file = std::fs::File::create(root.join("unrelated.bin")).unwrap();
        file.set_len(1024 * 1024 * 1024 * 1024).unwrap();
        crate::package_io::STREAMED_FILE_BYTES.with(|count| count.set(0));
        let first = revision(&root);
        let second = revision(&root);
        assert_eq!(first, second, "unread entries are not configuration");
        assert!(
            same_capture(&first, &second),
            "complete accepted domains remain stable"
        );
        assert_eq!(
            crate::package_io::STREAMED_FILE_BYTES.with(std::cell::Cell::get),
            0
        );
    }

    #[test]
    fn issue527_same_size_edits_beyond_old_file_and_directory_byte_budgets_change_revision() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        for folder in ["skills/large", "tools/large", "workflows"] {
            std::fs::create_dir_all(root.join(folder)).unwrap();
        }
        std::fs::write(
            root.join("skills/large/SKILL.md"),
            "---\nname: large\ndescription: Accepted bytes\n---\n",
        )
        .unwrap();
        std::fs::write(
            root.join("tools/large/server.py"),
            "from fastmcp import FastMCP\nmcp = FastMCP('large')\n",
        )
        .unwrap();
        std::fs::write(root.join("tools/large/requirements.txt"), "").unwrap();
        for name in [
            "skills/large/SKILL.md",
            "skills/large/asset.bin",
            "tools/large/asset.bin",
        ] {
            let path = root.join(name);
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create(true)
                .truncate(false)
                .open(path)
                .unwrap();
            file.set_len(65 * 1024 * 1024).unwrap();
            let before = revision(&root);
            assert!(before.starts_with("incomplete:"), "{before}");
            assert!(capture_token(&before).is_some(), "{before}");
            file.seek(SeekFrom::End(-1)).unwrap();
            file.write_all(b"x").unwrap();
            let after = revision(&root);
            assert!(
                !same_capture(&before, &after),
                "accepted content beyond the raw budget: {name}"
            );
        }
        let workflows = root.join("workflows/large.yaml");
        let mut bytes = b"description: Large workflow\nblock:\n  input: {type: object, properties: {}, additionalProperties: false}\n  nodes: {start: {type: start}, done: {type: return, outputs: []}}\n  edges: [{from: start, to: done}]\n#".to_vec();
        bytes.resize(crate::workflow_file::MAX_WORKFLOW_FILE_BYTES, b'x');
        std::fs::write(&workflows, bytes).unwrap();
        let before = revision(&root);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(workflows)
            .unwrap();
        file.seek(SeekFrom::End(-1)).unwrap();
        file.write_all(b"y").unwrap();
        assert!(
            !same_capture(&before, &revision(&root)),
            "accepted Workflow source bytes remain authoritative after observation overflow"
        );
    }
}

#[cfg(all(test, target_os = "macos"))]
mod case_insensitive_tests {
    use super::*;

    /// On a case-insensitive volume the loaders open `.agents/skills` and
    /// read the directory named `Skills`: editing a Skill there changes the
    /// revision exactly as it changes what they load.
    #[test]
    fn a_case_variant_the_loaders_open_is_configuration() {
        let volume = crate::local_runtime::case_insensitive_volume::CaseInsensitiveVolume::new();
        let root = volume.path().join(".agents");
        std::fs::create_dir_all(root.join("Skills/guide")).unwrap();
        std::fs::write(root.join("Skills/guide/SKILL.md"), b"A").unwrap();
        assert_eq!(
            std::fs::read(root.join("skills/guide/SKILL.md")).unwrap(),
            b"A",
            "the loaders' fixed path resolves to `Skills`"
        );
        let initial = revision(&root);
        std::fs::write(root.join("Skills/guide/SKILL.md"), b"B").unwrap();
        assert_ne!(revision(&root), initial);
    }
}
