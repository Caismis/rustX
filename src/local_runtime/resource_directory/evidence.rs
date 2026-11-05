//! A finite discovery-domain fence when arbitrary unused trees cannot be walked.
//! Uses the existing domain readers, including their candidate/shadowing rules.
use sha2::{Digest, Sha256};
use std::path::Path;

use crate::runtime::CancellationSignal;

struct HashWriter<'a>(&'a mut Sha256);
impl std::io::Write for HashWriter<'_> {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.update(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn include(hash: &mut Sha256, value: &impl serde::Serialize) -> Result<(), String> {
    serde_json::to_writer(HashWriter(hash), value).map_err(|error| error.to_string())?;
    hash.update(b"\n");
    Ok(())
}

fn include_diagnostics(
    hash: &mut Sha256,
    diagnostics: &[crate::runtime::resources::RuntimeResourceLoadError],
) -> Result<(), String> {
    include(
        hash,
        &diagnostics
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
    )
}

pub(super) fn capture(
    root: &Path,
    scope: crate::runtime::resources::SourceScope,
    signal: &CancellationSignal,
) -> Result<String, String> {
    crate::package_io::check_cancelled(signal).map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    hash.update(b"rustx-accepted-resource-domain-v1\n");
    let skills = crate::skills::SkillDiscovery::user_root(root.join("skills"))
        .with_cancellation(signal.clone())
        .discover();
    include(
        &mut hash,
        &(&skills.provenance, &skills.invalid, &skills.diagnostics),
    )?;
    for package in &skills.packages {
        include(&mut hash, &(package.id(), package.version_id()))?;
    }
    let python = super::super::managed_python_resources::discover_controlled(None, root, signal)
        .map_err(|error| error.to_string())?;
    include(&mut hash, &python.locations)?;
    include_diagnostics(&mut hash, &python.discovery_diagnostics)?;
    for (id, package) in python.packages() {
        include(
            &mut hash,
            &(
                id,
                package
                    .as_ref()
                    .map(crate::tools::python::PythonToolPackage::source_digest)
                    .map_err(ToString::to_string),
            ),
        )?;
    }
    let workspace = (scope == crate::runtime::resources::SourceScope::Workspace)
        .then(|| root.parent())
        .flatten();
    let agent_root = root.join("agents");
    let (catalog, agents) = super::super::agent_resources::load_authorized(workspace, &agent_root)
        .map_err(|error| error.to_string())?;
    include(&mut hash, &agents)?;
    include_diagnostics(&mut hash, &catalog.discovery_diagnostics)?;
    include(
        &mut hash,
        &catalog
            .invalid()
            .iter()
            .map(|(id, error)| (id, error.to_string()))
            .collect::<Vec<_>>(),
    )?;
    for definition in catalog.definitions() {
        for file in &definition.profile().project_instructions.files {
            include_source(
                &mut hash,
                workspace.unwrap_or(&agent_root),
                &file.path,
                crate::bounded_file::MAX_RESOURCE_BYTES,
            )?;
        }
    }
    include_source(
        &mut hash,
        root,
        &root.join("AGENTS.md"),
        crate::bounded_file::MAX_RESOURCE_BYTES,
    )?;
    let workflows =
        super::super::workflow_resources::load(None, root).map_err(|error| error.to_string())?;
    include(&mut hash, &workflows.locations)?;
    include_diagnostics(&mut hash, &workflows.discovery_diagnostics)?;
    include(
        &mut hash,
        &workflows
            .invalid()
            .iter()
            .map(|(id, error)| (id, error.to_string()))
            .collect::<Vec<_>>(),
    )?;
    // A Workflow's source also includes presentation/formatting bytes. Read it
    // under its actual source policy, rather than hashing only compiled nodes.
    for location in workflows.locations.values() {
        include_source(
            &mut hash,
            root,
            &location.path,
            crate::workflow_file::MAX_WORKFLOW_FILE_BYTES as u64,
        )?;
    }
    include_source(
        &mut hash,
        root,
        &root.join("mcp.toml"),
        crate::bounded_file::MAX_RESOURCE_BYTES,
    )?;
    crate::package_io::check_cancelled(signal).map_err(|error| error.to_string())?;
    Ok(format!("accepted-{:x}", hash.finalize()))
}

fn include_source(hash: &mut Sha256, root: &Path, path: &Path, limit: u64) -> Result<(), String> {
    // The source owner bounds malformed/oversized files before allocation.
    // Invalid candidates still contribute their location and diagnostic; a
    // transition into an admitted source contributes all actual content bytes.
    let source = super::read_resource(root, path, limit);
    match source {
        Ok(bytes) => include(hash, &(path, format!("{:x}", Sha256::digest(&bytes)))),
        Err(error) => include(hash, &(path, error.to_string())),
    }
}
