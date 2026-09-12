//! Inert canonical Python identities. Package parsing and materialization remain
//! owned by the source lifecycle; directory discovery never enters those owners.
use crate::runtime::identity::McpServerId;
use crate::runtime::resources::{
    ManagedPythonCatalog, RuntimeResourceLoadError, validate_project_resource_path,
};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub(crate) fn discover(workspace: &Path) -> Result<ManagedPythonCatalog, RuntimeResourceLoadError> {
    let root = workspace.join(".agents/tools");
    let mut packages = BTreeMap::<McpServerId, PathBuf>::new();
    for path in super::resource_directory::entries(workspace, &root)? {
        let fail = |detail: String| RuntimeResourceLoadError::new(detail).at(&path, "tools");
        let meta = std::fs::symlink_metadata(&path).map_err(|e| fail(e.to_string()))?;
        if meta.file_type().is_symlink() {
            return Err(fail("managed Python package must not be a symlink".into()));
        }
        if !meta.is_dir() {
            continue;
        }
        let name = path
            .file_name()
            .and_then(|v| v.to_str())
            .ok_or_else(|| fail("package name must be UTF-8".into()))?;
        if name.starts_with('.') {
            continue;
        }
        crate::tools::python::validate_identifier(name).map_err(|e| fail(e.to_string()))?;
        validate_project_resource_path(workspace, &path)?;
        packages.insert(crate::tools::python::python_server_id(name), path);
    }
    if packages.len() > 128 {
        return Err(
            RuntimeResourceLoadError::new("Managed Python catalog exceeds 128 packages")
                .at(&root, "tools"),
        );
    }
    Ok(ManagedPythonCatalog::new(packages))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_is_inert_even_for_unprepared_packages() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        assert!(discover(&workspace).unwrap().packages().is_empty());
        let root = workspace.join(".agents/tools");
        std::fs::create_dir_all(&root).unwrap();
        assert!(discover(&workspace).unwrap().packages().is_empty());
        for name in ["zeta", "alpha"] {
            std::fs::create_dir(root.join(name)).unwrap();
        }
        std::fs::write(root.join("README.md"), "incidental").unwrap();
        crate::tools::python::PACKAGE_PARSE_COUNT.with(|count| count.set(0));
        let (catalog, effects) =
            crate::local_runtime::static_effects::measure(|| discover(&workspace).unwrap());
        assert_eq!(effects, [0; 13]);
        crate::tools::python::PACKAGE_PARSE_COUNT.with(|count| assert_eq!(count.get(), 0));
        assert_eq!(
            catalog
                .packages()
                .keys()
                .map(McpServerId::as_str)
                .collect::<Vec<_>>(),
            ["python:alpha", "python:zeta"]
        );
        std::fs::remove_dir(root.join("alpha")).unwrap();
        assert_eq!(catalog.packages().len(), 2);
        assert_eq!(discover(&workspace).unwrap().packages().len(), 1);
    }
    #[test]
    fn python_catalog_order_does_not_depend_on_creation_order() {
        for names in [["zeta", "alpha"], ["alpha", "zeta"]] {
            let dir = tempfile::tempdir().unwrap();
            let workspace = dir.path().canonicalize().unwrap();
            for name in names {
                std::fs::create_dir_all(workspace.join(".agents/tools").join(name)).unwrap();
            }
            assert_eq!(
                discover(&workspace)
                    .unwrap()
                    .packages()
                    .keys()
                    .map(McpServerId::as_str)
                    .collect::<Vec<_>>(),
                ["python:alpha", "python:zeta"]
            );
        }
    }
    #[cfg(unix)]
    #[test]
    fn redirected_packages_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        std::fs::create_dir_all(workspace.join(".agents/tools")).unwrap();
        std::os::unix::fs::symlink("/tmp", workspace.join(".agents/tools/escape")).unwrap();
        assert!(discover(&workspace).is_err());
    }
}
