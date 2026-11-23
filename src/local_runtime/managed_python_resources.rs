//! Inert canonical Python identities. Package parsing and materialization remain
//! owned by the source lifecycle; directory discovery never enters those owners.
use crate::capabilities::ToolSourceId;
use crate::runtime::resources::SourceScope;
use crate::runtime::resources::{
    ManagedPythonCatalog, RuntimeResourceLoadError, validate_project_resource_path,
};
use std::collections::BTreeMap;
use std::path::Path;

/// Captures package metadata and exact source identities through the admitted Client.
///
/// # Errors
/// Returns the typed source observation or authority failure.
#[doc(hidden)]
pub fn discover_bound(
    endpoint: &std::sync::Arc<super::session_runtime_manager::LocalExecutionEndpoint>,
    user_root: &Path,
    cancellation: &crate::runtime::CancellationSignal,
) -> Result<ManagedPythonCatalog, RuntimeResourceLoadError> {
    discover_bound_for(endpoint, user_root, true, cancellation)
}

pub(super) fn discover_bound_for(
    endpoint: &std::sync::Arc<super::session_runtime_manager::LocalExecutionEndpoint>,
    user_root: &Path,
    include_workspace: bool,
    cancellation: &crate::runtime::CancellationSignal,
) -> Result<ManagedPythonCatalog, RuntimeResourceLoadError> {
    use crate::runtime::resources::ResourceLocation;
    use rustx_execution_protocol::mcp::{
        PythonSourceReply as Reply, PythonSourceRequest as Request, PythonSourceScope,
    };
    use std::time::{Duration, Instant};

    let deadline = Instant::now() + Duration::from_secs(20);
    let mut candidates = BTreeMap::new();
    let mut locations = BTreeMap::new();
    let mut diagnostics = Vec::new();
    for (scope, physical, root) in [
        (
            SourceScope::User,
            PythonSourceScope::User,
            user_root.join("tools"),
        ),
        (
            SourceScope::Workspace,
            PythonSourceScope::Workspace,
            endpoint.workspace().join(".agents/tools"),
        ),
    ] {
        if scope == SourceScope::Workspace && !include_workspace {
            continue;
        }
        let names =
            match endpoint.managed_python_source(&Request::List { scope: physical }, deadline) {
                Ok(Reply::Names(names)) if names.len() <= 128 => names,
                Ok(Reply::CollectionInvalid) => {
                    candidates.clear();
                    locations.clear();
                    diagnostics.push(
                        RuntimeResourceLoadError::new(
                            "Client Managed Python collection must be a directory",
                        )
                        .at(&root, "tools"),
                    );
                    continue;
                }
                Err(error) if error.kind() == std::io::ErrorKind::Unsupported => {
                    return Ok(ManagedPythonCatalog::default());
                }
                Ok(_) | Err(_) => {
                    return Err(RuntimeResourceLoadError::new(
                        "cannot observe Client Managed Python sources",
                    )
                    .at(&root, "tools"));
                }
            };
        for name in names {
            if name.len() > 128 || crate::tools::python::validate_identifier(&name).is_err() {
                return Err(RuntimeResourceLoadError::new(
                    "invalid Client Managed Python identity",
                )
                .at(&root, "tools"));
            }
            let source = ToolSourceId::ManagedPython(name.clone());
            let path = root.join(&name);
            let shadowed = locations
                .remove(&source)
                .map(|owner: ResourceLocation| owner.path);
            locations.insert(
                source.clone(),
                ResourceLocation {
                    scope,
                    path: path.clone(),
                    shadowed,
                },
            );
            candidates.insert(source, (name, physical, path));
        }
    }
    if candidates.len() > 128 {
        return Err(
            RuntimeResourceLoadError::new("Managed Python catalog exceeds 128 packages")
                .at(user_root, "tools"),
        );
    }
    let packages = candidates
        .into_iter()
        .map(|(source, (name, physical, path))| {
            let result =
                capture_bound_package(endpoint, physical, name, path, deadline, cancellation);
            (source, result)
        })
        .collect();
    let mut catalog = ManagedPythonCatalog::new(packages);
    catalog.locations = locations;
    catalog.discovery_diagnostics = diagnostics;
    Ok(catalog)
}

fn capture_bound_package(
    endpoint: &std::sync::Arc<super::session_runtime_manager::LocalExecutionEndpoint>,
    scope: rustx_execution_protocol::mcp::PythonSourceScope,
    name: String,
    path: std::path::PathBuf,
    deadline: std::time::Instant,
    cancellation: &crate::runtime::CancellationSignal,
) -> Result<crate::tools::python::PythonToolPackage, crate::tools::python::PythonToolError> {
    use crate::tools::python::{ClientSource, PythonToolError, PythonToolPackage};
    use base64::{Engine as _, engine::general_purpose::STANDARD};
    use rustx_execution_protocol::{
        mcp::{PythonSourceReply as Reply, PythonSourceRequest as Request},
        process::OperationId,
    };
    let unavailable =
        || PythonToolError::Storage("the admitted Client could not capture the package".into());
    let key = endpoint
        .process_key(OperationId::new())
        .map_err(|_| unavailable())?;
    let owner = std::sync::Arc::new(ClientSource {
        endpoint: endpoint.clone(),
        key: key.clone(),
    });
    let mut request = Request::Capture {
        key: key.clone(),
        scope,
        name: name.clone(),
    };
    loop {
        if cancellation.is_cancelled() || std::time::Instant::now() >= deadline {
            return Err(unavailable());
        }
        match endpoint
            .managed_python_source(&request, deadline)
            .map_err(|_| unavailable())?
        {
            Reply::Capturing => {
                request = Request::State { key: key.clone() };
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
            Reply::Captured {
                digest,
                permissions_base64,
            } => {
                let valid = digest.strip_prefix("sha256:").is_some_and(|hash| {
                    hash.len() == 64
                        && hash
                            .bytes()
                            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
                });
                if !valid {
                    return Err(unavailable());
                }
                let permissions = permissions_base64
                    .as_deref()
                    .map(|encoded| STANDARD.decode(encoded))
                    .transpose()
                    .map_err(|_| unavailable())?;
                if permissions
                    .as_ref()
                    .is_some_and(|bytes| bytes.len() > 512 * 1024)
                {
                    return Err(unavailable());
                }
                return PythonToolPackage::from_client(
                    name,
                    path,
                    digest,
                    permissions.as_deref(),
                    owner,
                );
            }
            Reply::Refused(rustx_execution_protocol::ReadError::Invalid) => {
                return Err(PythonToolError::InvalidPackage(format!(
                    "the admitted Client rejected package {name:?}"
                )));
            }
            _ => return Err(unavailable()),
        }
    }
}

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
    discover_authoring(
        workspace,
        user_root,
        &crate::runtime::CancellationSignal::new(),
        true,
    )
}

pub(crate) fn discover_controlled(
    workspace: Option<&Path>,
    user_root: &Path,
    cancellation: &crate::runtime::CancellationSignal,
) -> Result<ManagedPythonCatalog, RuntimeResourceLoadError> {
    discover_authoring(workspace, user_root, cancellation, cfg!(test))
}

fn discover_authoring(
    workspace: Option<&Path>,
    user_root: &Path,
    cancellation: &crate::runtime::CancellationSignal,
    freeze_fixture_sources: bool,
) -> Result<ManagedPythonCatalog, RuntimeResourceLoadError> {
    crate::package_io::check_cancelled(cancellation)
        .map_err(|error| RuntimeResourceLoadError::new(error.to_string()).at(user_root, "tools"))?;
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
                .and_then(|()| {
                    if freeze_fixture_sources {
                        crate::tools::python::discover_package(&path, &name, cancellation)
                    } else {
                        crate::tools::python::PythonToolPackage::inspect_authoring(&path, &name)
                    }
                });
            (ToolSourceId::ManagedPython(name), package)
        })
        .collect();
    let mut catalog = ManagedPythonCatalog::new(packages);
    catalog.locations = locations;
    catalog.discovery_diagnostics = diagnostics;
    crate::package_io::check_cancelled(cancellation)
        .map_err(|error| RuntimeResourceLoadError::new(error.to_string()).at(user_root, "tools"))?;
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
        assert_eq!(effects, [0; 11]);
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
        assert_eq!(effects, [0; 11]);
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
