//! Small bounded directory IO shared by the fixed canonical resource readers.
use crate::runtime::resources::{RuntimeResourceLoadError, validate_project_resource_path};
use std::path::{Path, PathBuf};

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
    let paths = std::fs::read_dir(root)
        .map_err(|e| fail(e.to_string()))?
        .take(1025)
        .map(|entry| entry.map(|e| e.path()))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| fail(e.to_string()))?;
    if paths.len() > 1024 {
        return Err(fail("canonical resource root exceeds 1024 entries".into()));
    }
    let mut paths = paths;
    paths.sort();
    Ok(paths)
}

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
    if files.len() > 128 {
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
    crate::bounded_file::read_bounded(path)
        .map_err(|e| RuntimeResourceLoadError::new(e).at(path, "resources"))
}

/// Revision of authored resource bytes, including malformed and shadowed entries.
/// This is change detection, not discovery or configuration authority. Symlinks
/// are hashed as links and never followed. Traversal is bounded even for invalid
/// unused trees, and writer locks/staging files do not count as authored input.
pub(crate) fn revision(root: &Path) -> String {
    use sha2::{Digest, Sha256};
    fn walk(
        path: &Path,
        hash: &mut Sha256,
        remaining: &mut usize,
        bytes_left: &mut u64,
        depth: usize,
    ) {
        if *remaining == 0 || depth > 32 {
            hash.update(b"resource traversal bound");
            return;
        }
        *remaining -= 1;
        let metadata = match std::fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) => {
                hash.update(format!("missing-or-unreadable:{:?}", error.kind()));
                return;
            }
        };
        if metadata.file_type().is_symlink() {
            hash.update(b"link:");
            if let Ok(target) = std::fs::read_link(path) {
                hash.update(target.as_os_str().as_encoded_bytes());
            }
        } else if metadata.is_dir() {
            hash.update(b"directory:");
            let Ok(entries) = std::fs::read_dir(path) else {
                hash.update(b"unreadable");
                return;
            };
            let mut entries: Vec<_> = entries.take(16_385).filter_map(Result::ok).collect();
            if entries.len() > 16_384 {
                hash.update(b"directory entry bound");
                return;
            }
            entries.sort_by_key(std::fs::DirEntry::file_name);
            for entry in entries {
                let name = entry.file_name();
                let text = name.to_string_lossy();
                if depth <= 1
                    && (text.starts_with(".tmp")
                        || (text.starts_with('.') && text.ends_with(".toml.lock")))
                {
                    continue;
                }
                let bytes = name.as_encoded_bytes();
                hash.update(bytes.len().to_le_bytes());
                hash.update(bytes);
                walk(&entry.path(), hash, remaining, bytes_left, depth + 1);
            }
        } else if metadata.is_file() {
            hash.update(b"file:");
            if metadata.len() > *bytes_left {
                hash.update(b"resource byte bound");
                hash.update(metadata.len().to_le_bytes());
                if let Ok(time) = metadata.modified() {
                    hash.update(format!("{time:?}"));
                }
                return;
            }
            *bytes_left -= metadata.len();
            if let Ok(bytes) = crate::bounded_file::read_bounded(path) {
                hash.update(bytes.len().to_le_bytes());
                hash.update(bytes);
            } else {
                hash.update(b"unreadable-or-oversized");
                hash.update(metadata.len().to_le_bytes());
            }
        } else {
            hash.update(b"unsupported file type");
        }
    }
    let mut hash = Sha256::new();
    walk(root, &mut hash, &mut 16_384, &mut (64 * 1024 * 1024), 0);
    format!("sha256:{:x}", hash.finalize())
}
