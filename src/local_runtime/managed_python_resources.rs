//! Inert canonical Python identities. Package parsing and materialization remain
//! owned by the source lifecycle; directory discovery never enters those owners.
use super::configuration::settings::SourceScope;
use crate::capabilities::ToolSourceId;
use crate::runtime::resources::{
    ManagedPythonCatalog, RuntimeResourceLoadError, validate_project_resource_path,
};
use std::collections::BTreeMap;
use std::path::Path;

/// Captures the User root and the explicitly supplied Workspace, shadowing before parsing.
/// With no Workspace, discovery reads only User resources; it never infers a cwd.
/// This operation reads package bytes but never imports code or prepares Python.
///
/// # Errors
/// Returns a bounded resource error when discovery cannot produce a catalog.
pub fn discover(
    workspace: Option<&Path>,
    user_root: &Path,
) -> Result<ManagedPythonCatalog, RuntimeResourceLoadError> {
    let mut candidates = BTreeMap::new();
    let mut locations = BTreeMap::new();
    let mut diagnostics = Vec::new();
    for (boundary, root, scope) in
        std::iter::once((user_root, user_root.join("tools"), SourceScope::User)).chain(
            workspace.map(|workspace| {
                (
                    workspace,
                    workspace.join(".agents/tools"),
                    SourceScope::Workspace,
                )
            }),
        )
    {
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
            RuntimeResourceLoadError::new("Managed Python catalog exceeds 128 packages").at(
                &workspace.map_or_else(
                    || user_root.join("tools"),
                    |workspace| workspace.join(".agents/tools"),
                ),
                "tools",
            ),
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
    fn user_discovery_does_not_read_workspace_packages() {
        let dir = tempfile::tempdir().unwrap();
        // The workspace argument is canonical authority; model the production
        // binding point instead of passing an aliased temporary root.
        let root = dir.path().canonicalize().unwrap();
        let user = root.join("user");
        std::fs::create_dir_all(user.join("tools/user-package")).unwrap();
        std::fs::create_dir_all(root.join(".agents/tools/workspace-package")).unwrap();
        crate::tools::python::PACKAGE_PARSE_COUNT.with(|count| count.set(0));
        let (catalog, effects) =
            super::super::static_effects::measure(|| discover(None, &user).unwrap());
        assert_eq!(
            catalog
                .packages()
                .keys()
                .map(ToString::to_string)
                .collect::<Vec<_>>(),
            ["python:user-package"]
        );
        assert!(catalog.packages().values().all(Result::is_err));
        assert!(
            catalog
                .locations
                .values()
                .all(|location| location.scope == SourceScope::User)
        );
        crate::tools::python::PACKAGE_PARSE_COUNT.with(|count| assert_eq!(count.get(), 1));
        assert_eq!(effects, [0; 12]);
        assert_eq!(discover(Some(&root), &user).unwrap().packages().len(), 2);
    }

    #[test]
    fn discovery_is_inert_even_for_unprepared_packages() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path().canonicalize().unwrap();
        assert!(
            discover(Some(&workspace), &workspace.join("user/.agents"))
                .unwrap()
                .packages()
                .is_empty()
        );
        let root = workspace.join(".agents/tools");
        std::fs::create_dir_all(&root).unwrap();
        assert!(
            discover(Some(&workspace), &workspace.join("user/.agents"))
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
            discover(Some(&workspace), &workspace.join("user/.agents")).unwrap()
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
            discover(Some(&workspace), &workspace.join("user/.agents"))
                .unwrap()
                .packages()
                .len(),
            1
        );
    }

    #[test]
    fn managed_fastmcp_policy_applies_to_user_and_workspace_discovery() {
        let directory = tempfile::tempdir().unwrap();
        let workspace = directory.path().canonicalize().unwrap();
        let user = workspace.join("user/.agents");
        for (root, name, declaration) in [
            (&user, "user-tool", "FastMCP[cli]>=2"),
            (
                &workspace.join(".agents"),
                "workspace-tool",
                "fastmcp; python_version < '0'",
            ),
        ] {
            let package = root.join("tools").join(name);
            std::fs::create_dir_all(&package).unwrap();
            std::fs::write(package.join("server.py"), "mcp = None\n").unwrap();
            std::fs::write(package.join("requirements.txt"), declaration).unwrap();
        }
        let catalog = discover(Some(&workspace), &user).unwrap();
        for source in ["user-tool", "workspace-tool"] {
            assert!(
                matches!(
                    &catalog.packages()[&ToolSourceId::ManagedPython(source.into())],
                    Err(crate::tools::python::PythonToolError::InvalidPackage(message))
                        if message.contains("managed by rustX")
                ),
                "{source} applies the same normalized managed-dependency policy"
            );
        }
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
                discover(Some(&workspace), &workspace.join("user/.agents"))
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
        let catalog = discover(Some(&workspace), &workspace.join("user/.agents")).unwrap();
        assert!(catalog.packages()[&ToolSourceId::ManagedPython("escape".into())].is_err());
    }
}
