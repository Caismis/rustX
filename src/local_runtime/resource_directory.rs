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
        validate_project_resource_path(boundary, &path)?;
        let meta = std::fs::symlink_metadata(&path)
            .map_err(|e| RuntimeResourceLoadError::new(e.to_string()).at(&path, "resources"))?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err(RuntimeResourceLoadError::new(
                "canonical resource must be a regular file, not a symlink",
            )
            .at(&path, "resources"));
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
