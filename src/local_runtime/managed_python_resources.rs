//! Inert canonical Python identities. Package parsing and materialization remain
//! owned by the source lifecycle; directory discovery never enters those owners.
use super::configuration::settings::SourceScope;
use crate::capabilities::ToolSourceId;
use crate::runtime::resources::{
    ManagedPythonCatalog, RuntimeResourceLoadError, validate_project_resource_path,
};
use std::collections::BTreeMap;
use std::path::Path;

/// Captures both explicitly bound resource roots, shadowing before parsing.
/// This operation reads package bytes but never imports code or prepares Python.
///
/// # Errors
/// Returns a bounded resource error when discovery cannot produce a catalog.
pub fn discover(
    workspace: &Path,
    user_root: &Path,
) -> Result<ManagedPythonCatalog, RuntimeResourceLoadError> {
    let mut candidates = BTreeMap::new();
    let mut locations = BTreeMap::new();
    let mut diagnostics = Vec::new();
    for (boundary, root, scope) in [
        (user_root, user_root.join("tools"), SourceScope::User),
        (
            workspace,
            workspace.join(".agents/tools"),
            SourceScope::Workspace,
        ),
    ] {
        let paths = match super::resource_directory::entries(boundary, &root) {
            Ok(paths) => paths,
            Err(error) => {
                candidates.clear();
                locations.clear();
                diagnostics.push(error);
                continue;
            }
        };
        for path in paths {
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
                diagnostics.push(
                    RuntimeResourceLoadError::new("invalid Managed Python filename identity")
                        .at(&path, "tools"),
                );
                continue;
            };
            if name.starts_with('.') {
                continue;
            }
            // Incidental documentation is not a package identity. A same-name
            // non-directory package still reserves the higher identity and is
            // diagnosed by the package reader rather than exposing a lower one.
            if !name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            {
                continue;
            }
            let shadowed = locations
                .remove(&ToolSourceId::ManagedPython(name.into()))
                .map(|lower: crate::runtime::resources::ResourceLocation| lower.path);
            locations.insert(
                ToolSourceId::ManagedPython(name.into()),
                crate::runtime::resources::ResourceLocation {
                    scope,
                    path: path.clone(),
                    shadowed,
                },
            );
            candidates.insert(name.to_owned(), (path, boundary.to_path_buf()));
        }
    }
    if candidates.len() > 128 {
        diagnostics.push(
            RuntimeResourceLoadError::new("Managed Python catalog exceeds 128 packages")
                .at(&workspace.join(".agents/tools"), "tools"),
        );
        candidates.clear();
        locations.clear();
    }
    let packages = candidates
        .into_iter()
        .map(|(name, (path, boundary))| {
            let package = validate_project_resource_path(&boundary, &path)
                .map_err(|error| {
                    crate::tools::python::PythonToolError::InvalidPackage(error.to_string())
                })
                .and_then(|()| crate::tools::python::discover_package(&path, &name));
            (ToolSourceId::ManagedPython(name), package)
        })
        .collect();
    let mut catalog = ManagedPythonCatalog::new(packages);
    catalog.locations = locations;
    catalog.discovery_diagnostics = diagnostics;
    Ok(catalog)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_is_inert_even_for_unprepared_packages() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        assert!(
            discover(&workspace, &workspace.join("user/.agents"))
                .unwrap()
                .packages()
                .is_empty()
        );
        let root = workspace.join(".agents/tools");
        std::fs::create_dir_all(&root).unwrap();
        assert!(
            discover(&workspace, &workspace.join("user/.agents"))
                .unwrap()
                .packages()
                .is_empty()
        );
        for name in ["zeta", "alpha"] {
            std::fs::create_dir(root.join(name)).unwrap();
        }
        std::fs::write(root.join("README.md"), "incidental").unwrap();
        crate::tools::python::PACKAGE_PARSE_COUNT.with(|count| count.set(0));
        let (catalog, effects) = crate::local_runtime::static_effects::measure(|| {
            discover(&workspace, &workspace.join("user/.agents")).unwrap()
        });
        assert_eq!(effects, [0; 12]);
        crate::tools::python::PACKAGE_PARSE_COUNT.with(|count| assert_eq!(count.get(), 2));
        assert_eq!(
            catalog
                .packages()
                .keys()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["python:alpha", "python:zeta"]
        );
        std::fs::remove_dir(root.join("alpha")).unwrap();
        assert_eq!(catalog.packages().len(), 2);
        assert_eq!(
            discover(&workspace, &workspace.join("user/.agents"))
                .unwrap()
                .packages()
                .len(),
            1
        );
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
                discover(&workspace, &workspace.join("user/.agents"))
                    .unwrap()
                    .packages()
                    .keys()
                    .map(ToString::to_string)
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
        let catalog = discover(&workspace, &workspace.join("user/.agents")).unwrap();
        assert!(catalog.packages()[&ToolSourceId::ManagedPython("escape".into())].is_err());
    }
}
