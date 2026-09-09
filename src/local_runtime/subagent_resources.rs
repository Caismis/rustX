//! Canonical role authoring. This module only produces the existing native catalog.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;
use yaml_rust2::scanner::{Scanner, TokenType};

use super::config::{SubagentDocument, SubagentsDocument};
use crate::runtime::resources::ProjectContextFile;
use crate::runtime::resources::{RuntimeResourceLoadError, validate_project_resource_path};
use crate::runtime::subagent::{
    SubagentCatalog, SubagentDefinition, SubagentName, SubagentProjectInstructionPolicy,
};

/// Safe prospective source facts, never capability authority.
#[derive(Debug, Clone, Serialize)]
pub struct RoleSource {
    pub identity: SubagentName,
    pub selected: PathBuf,
    pub layer: &'static str,
    pub overridden: Option<PathBuf>,
}

/// LF or CRLF exact `---` lines delimit one frontmatter mapping at byte zero.
/// The body is retained verbatim. The shared reader bounds the entire file to 1 MiB.
pub(crate) fn parse(text: &str) -> Result<(SubagentDocument, String), String> {
    if text.len() > 1024 * 1024 {
        return Err("role resource exceeds 1 MiB".into());
    }
    let opening = text
        .strip_prefix("---\r\n")
        .or_else(|| text.strip_prefix("---\n"))
        .ok_or("required opening frontmatter delimiter --- at byte zero")?;
    let mut offset = 0;
    let mut boundary = None;
    for line in opening.split_inclusive('\n') {
        if line.trim_end_matches(['\r', '\n']) == "---" {
            boundary = Some((offset, offset + line.len()));
            break;
        }
        offset += line.len();
    }
    let (end, body) = boundary.ok_or("required closing frontmatter delimiter ---")?;
    let mut scanner = Scanner::new(opening[..end].chars());
    let mut depth = 0usize;
    for token in scanner.by_ref() {
        match token.1 {
            TokenType::BlockMappingStart
            | TokenType::BlockSequenceStart
            | TokenType::FlowMappingStart
            | TokenType::FlowSequenceStart => {
                depth += 1;
                if depth > 32 {
                    return Err("frontmatter nesting exceeds 32 levels".into());
                }
            }
            TokenType::BlockEnd | TokenType::FlowMappingEnd | TokenType::FlowSequenceEnd => {
                depth = depth.saturating_sub(1);
            }
            _ => {}
        }
        if matches!(
            token.1,
            TokenType::Alias(_)
                | TokenType::Anchor(_)
                | TokenType::Tag(..)
                | TokenType::TagDirective(..)
                | TokenType::VersionDirective(..)
                | TokenType::DocumentStart
                | TokenType::DocumentEnd
        ) {
            return Err("frontmatter tags, anchors, aliases, directives, and extra documents are unsupported".into());
        }
    }
    if let Some(error) = scanner.get_error() {
        return Err(error.to_string());
    }
    let value: serde_yaml::Value =
        serde_yaml::from_str(&opening[..end]).map_err(|e| e.to_string())?;
    if !value.is_mapping() {
        return Err("frontmatter must be a plain mapping".into());
    }
    validate_yaml(&value)?;
    let document: SubagentDocument = serde_yaml::from_value(value).map_err(|e| e.to_string())?;
    for selector in document.tools.selectors() {
        let empty = match selector {
            crate::capabilities::selection::ToolSelector::Builtin { name } => {
                name.trim().is_empty()
            }
            crate::capabilities::selection::ToolSelector::Mcp { server_id, name } => {
                server_id.as_str().is_empty() || name.trim().is_empty()
            }
        };
        if empty {
            return Err("tools must name nonempty source-qualified capability identities".into());
        }
    }
    document
        .execution_deadline()
        .map_err(|e| format!("timeoutMs: {e}"))?;
    Ok((document, opening[body..].to_owned()))
}

fn validate_yaml(value: &serde_yaml::Value) -> Result<(), String> {
    match value {
        serde_yaml::Value::Tagged(_) => return Err("YAML tags are unsupported".into()),
        serde_yaml::Value::Mapping(entries) => {
            for (key, value) in entries {
                let Some(key) = key.as_str() else {
                    return Err("mapping keys must be strings".into());
                };
                if key == "<<" {
                    return Err("YAML merges are unsupported".into());
                }
                validate_yaml(value)?;
            }
        }
        serde_yaml::Value::Sequence(values) => {
            for value in values {
                validate_yaml(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Resolve only explicitly registered identities against two pinned roots.
/// Callers supply launch-resolved canonical authority roots, never display aliases.
/// Project replacement is whole-resource: no field, body, or permission merge.
pub(crate) fn load(
    workspace: &Path,
    user_root: &Path,
    document: &SubagentsDocument,
) -> Result<(SubagentCatalog, BTreeMap<SubagentName, RoleSource>), RuntimeResourceLoadError> {
    let mut definitions = Vec::new();
    let mut sources = BTreeMap::new();
    for name in &document.definitions {
        let user = user_root.join(format!("{name}.md"));
        let project = workspace
            .join(".agents/subagents")
            .join(format!("{name}.md"));
        let field = format!("subagents.definitions.{name}");
        let fail = |message: String| RuntimeResourceLoadError::new(message).at(&project, &field);
        if sources.contains_key(name) {
            return Err(fail("duplicate registered role identity".into()));
        }
        validate_project_resource_path(workspace, &project).map_err(|e| e.at(&project, &field))?;
        let project_exists = project.try_exists().map_err(|e| fail(e.to_string()))?;
        let user_exists = user.try_exists().map_err(|e| fail(e.to_string()))?;
        let (path, boundary, layer) = if project_exists {
            (&project, workspace, "project")
        } else {
            (&user, user_root, "user")
        };
        let error = |message: String| RuntimeResourceLoadError::new(message).at(path, &field);
        // A user root is explicitly pinned, and must not redirect to project or
        // arbitrary filesystem authority through symlinks.
        if layer == "user" {
            validate_project_resource_path(user_root, path).map_err(|e| e.at(path, &field))?;
        }
        if std::fs::symlink_metadata(path).is_ok_and(|metadata| metadata.file_type().is_symlink()) {
            return Err(error("canonical role file must not be a symlink".into())
                .because("canonical role identity cannot be redirected by a file symlink"));
        }
        let bytes = crate::config_format::read_bounded(path).map_err(|e| {
            error(e).because(
                "canonical role file is missing, oversized, or not a readable regular file",
            )
        })?;
        let text = std::str::from_utf8(&bytes)
            .map_err(|e| error(e.to_string()).because("canonical role file is not UTF-8"))?;
        let (agent, instructions) = parse(text).map_err(|e| {
            error(e)
                .because("canonical role frontmatter violates its strict typed authoring contract")
        })?;
        if agent.agents_md.files.len()
            > crate::runtime::subagent::catalog::MAX_SUBAGENT_PROJECT_FILES
        {
            return Err(error(
                "agentsMd.files exceeds the native file-count bound".into(),
            ));
        }
        let mut files = Vec::new();
        for file in &agent.agents_md.files {
            let resolved = boundary.join(file);
            validate_project_resource_path(boundary, &resolved)
                .map_err(|e| e.at(path, format!("{field}.agentsMd.files")))?;
            let bytes = crate::config_format::read_bounded(&resolved).map_err(error)?;
            let content = String::from_utf8(bytes).map_err(|e| error(e.to_string()))?;
            files.push(ProjectContextFile {
                path: resolved,
                content,
            });
        }
        let deadline = agent.execution_deadline().map_err(error)?;
        definitions.push(
            SubagentDefinition::new(
                name.clone(),
                agent.description,
                instructions,
                path.clone(),
                agent.model.clone(),
                deadline,
                agent.tools.selectors(),
                agent.skills,
                SubagentProjectInstructionPolicy {
                    inherit: agent.agents_md.inherit,
                    files,
                },
                agent.worktree.to_policy(),
            )
            .map_err(|e| error(e.to_string()))?,
        );
        sources.insert(
            name.clone(),
            RoleSource {
                identity: name.clone(),
                selected: path.clone(),
                layer,
                overridden: (project_exists && user_exists).then_some(user),
            },
        );
    }
    let catalog = SubagentCatalog::new(definitions)
        .map_err(|e| RuntimeResourceLoadError::new(e.to_string()))?;
    Ok((catalog, sources))
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex, Weak};
    use tokio::sync::watch;

    pub(crate) struct Gate {
        entered: watch::Sender<bool>,
        release: watch::Sender<bool>,
    }
    static GATES: Mutex<Vec<(PathBuf, Weak<Gate>)>> = Mutex::new(Vec::new());
    pub(crate) fn arm(workspace: &Path) -> Arc<Gate> {
        let gate = Arc::new(Gate {
            entered: watch::channel(false).0,
            release: watch::channel(false).0,
        });
        let mut gates = GATES.lock().unwrap();
        gates.retain(|(path, weak)| path != workspace && weak.strong_count() > 0);
        gates.push((workspace.into(), Arc::downgrade(&gate)));
        gate
    }
    impl Gate {
        pub(crate) async fn entered(&self) {
            self.entered
                .subscribe()
                .wait_for(|entered| *entered)
                .await
                .unwrap();
        }
        pub(crate) fn release(&self) {
            self.release.send_replace(true);
        }
    }
    pub(crate) async fn before_publication(workspace: &Path) {
        let gate = GATES
            .lock()
            .unwrap()
            .iter()
            .find_map(|(path, weak)| (path == workspace).then(|| weak.upgrade()).flatten());
        if let Some(gate) = gate {
            let mut release = gate.release.subscribe();
            gate.entered.send_replace(true);
            release.wait_for(|released| *released).await.unwrap();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canonical_authority_roots(dir: &Path) -> (PathBuf, PathBuf) {
        let workspace = dir.join("project");
        let user = dir.join("user/subagents");
        std::fs::create_dir_all(workspace.join(".agents/subagents")).unwrap();
        std::fs::create_dir_all(&user).unwrap();
        // Match launch resolution before passing frozen authority to the loader.
        (
            workspace.canonicalize().unwrap(),
            user.canonicalize().unwrap(),
        )
    }

    #[test]
    fn cfg236_strict_frontmatter_rejects_unsupported_and_ambiguous_resources() {
        for text in [
            "no frontmatter",
            "---\ndescription: x\n",
            "---\n[]\n---\nbody",
            "---\ndescription: [\n---\nbody",
            "---\ndescription: x\ndescription: y\n---\nbody",
            "---\ndescription: x\nid: other\n---\nbody",
            "---\ndescription: x\ninstructionsFile: x.md\n---\nbody",
            "---\ndescription: 42\n---\nbody",
            "---\ndescription: x\ntimeoutMs: '20'\n---\nbody",
            "---\ndescription: x\nworktree: {enabled: 'true'}\n---\nbody",
            "---\ndescription: x\ntools: {mcp: {server: [read], server: [write]}}\n---\nbody",
            "---\ndescription: !custom x\n---\nbody",
            "---\ndescription: !!str x\n---\nbody",
            "---\ndescription: &macro x\n---\nbody",
            "---\ndescription: *macro\n---\nbody",
            "---\n%YAML 1.2\ndescription: x\n---\nbody",
            "---\ndescription: x\n<<: {}\n---\nbody",
            "---\ndescription: x\ninclude: other.md\n---\nbody",
            "---\ndescription: x\nmetadata: {}\n---\nbody",
            "---\ndescription: x\ntools: {unknown: []}\n---\nbody",
            "---\ndescription: x\ntools: {builtin: ['']}\n---\nbody",
            "---\ndescription: x\ntools: {mcp: {'': [read]}}\n---\nbody",
            "---\ndescription: x\ntools: {mcp: {disabled: [' ']}}\n---\nbody",
        ] {
            assert!(parse(text).is_err(), "accepted {text}");
        }
        assert!(
            parse(&"x".repeat(1024 * 1024 + 1))
                .unwrap_err()
                .contains("1 MiB")
        );
        assert_eq!(
            parse("---\r\ndescription: 'literal !tag &anchor'\r\n---\r\nBody\r\n")
                .unwrap()
                .1,
            "Body\r\n"
        );
    }

    #[test]
    fn cfg236_registered_resource_io_and_native_bounds_fail_with_source_context() {
        let dir = tempfile::tempdir().unwrap();
        let (workspace, user) = canonical_authority_roots(dir.path());
        let roles = workspace.join(".agents/subagents");
        let document = SubagentsDocument {
            definitions: vec![SubagentName::parse("reviewer").unwrap()],
            ..Default::default()
        };
        assert!(load(&workspace, &user, &document).is_err());
        let path = roles.join("reviewer.md");
        for bytes in [
            vec![0xff],
            vec![b'x'; 1024 * 1024 + 1],
            format!("---\ndescription: x\n---\n{}", "x".repeat(64 * 1024 + 1)).into_bytes(),
            format!("---\ndescription: {}\n---\nbody", "x".repeat(513)).into_bytes(),
        ] {
            std::fs::write(&path, bytes).unwrap();
            let error = load(&workspace, &user, &document).unwrap_err();
            assert_eq!(error.source_file, Some(path.clone()));
            assert_eq!(
                error.field_path.as_deref(),
                Some("subagents.definitions.reviewer")
            );
            assert!(error.message.len() < 4096);
        }
    }

    #[test]
    fn cfg236_canonical_resource_maps_every_native_field_and_freezes_body() {
        let dir = tempfile::tempdir().unwrap();
        let (workspace, user) = canonical_authority_roots(dir.path());
        let roles = workspace.join(".agents/subagents");
        std::fs::write(workspace.join("guidance.md"), "Supplemental").unwrap();
        let path = roles.join("reviewer.md");
        std::fs::write(&path, "---\ndescription: Review one request\nmodel: example/demo-model\ntimeoutMs: 3600000\ntools:\n  builtin: [read]\n  mcp:\n    service: [lookup]\nskills: [review-guidance]\nagentsMd:\n  inherit: false\n  files: [guidance.md]\nworktree:\n  enabled: true\n  requireCleanParent: true\n---\nYou are the reviewer.\n").unwrap();
        let name = SubagentName::parse("reviewer").unwrap();
        let document = SubagentsDocument {
            definitions: vec![name.clone()],
            ..Default::default()
        };
        let (catalog, sources) = load(&workspace, &user, &document).unwrap();
        let role = catalog.get(&name).unwrap();
        assert_eq!(role.instructions(), "You are the reviewer.\n");
        assert_eq!(role.description(), "Review one request");
        assert_eq!(role.model().unwrap().to_string(), "example/demo-model");
        assert_eq!(role.execution_deadline().unwrap().as_millis(), 3_600_000);
        assert_eq!(role.skills(), ["review-guidance"]);
        assert_eq!(role.tools().len(), 2);
        assert!(!role.project_instructions().inherit);
        assert_eq!(role.project_instructions().files[0].content, "Supplemental");
        assert_eq!(
            role.workspace_policy(),
            crate::runtime::workspace::WorkspacePolicy::GitWorktree {
                require_clean_parent: true
            }
        );
        assert_eq!(sources[&name].selected, path);
        std::fs::write(path, "changed invalid resource").unwrap();
        assert_eq!(role.instructions(), "You are the reviewer.\n");
        assert!(load(&workspace, &user, &document).is_err());
    }

    #[test]
    fn cfg236_whole_resource_replacement_registration_and_independent_admissions() {
        let dir = tempfile::tempdir().unwrap();
        let (workspace, user) = canonical_authority_roots(dir.path());
        let project = workspace.join(".agents/subagents");
        let name = SubagentName::parse("reviewer").unwrap();
        std::fs::write(user.join("reviewer.md"), "---\ndescription: User\ntools: {builtin: [write]}\nskills: [user-skill]\n---\nUser body").unwrap();
        std::fs::write(
            project.join("reviewer.md"),
            "---\ndescription: Project\ntools: {builtin: [read]}\n---\nProject body",
        )
        .unwrap();
        // Even malformed unregistered files are inert, not a second registry.
        std::fs::write(project.join("ignored.md"), "malformed").unwrap();
        let mut document = SubagentsDocument::default();
        assert!(
            load(&workspace, &user, &document)
                .unwrap()
                .0
                .get(&name)
                .is_none()
        );
        document.definitions.push(name.clone());
        let (catalog, sources) = load(&workspace, &user, &document).unwrap();
        let role = catalog.get(&name).unwrap();
        assert_eq!(role.instructions(), "Project body");
        assert!(role.skills().is_empty());
        assert_eq!(
            role.tools(),
            &[crate::capabilities::selection::ToolSelector::Builtin {
                name: "read".into()
            }]
        );
        assert_eq!(sources[&name].layer, "project");
        assert_eq!(sources[&name].overridden, Some(user.join("reviewer.md")));
        assert!(
            catalog
                .admitted(&document.main.iter().cloned().collect())
                .unwrap()
                .get(&name)
                .is_none()
        );
        document.workflow.push(name.clone());
        assert!(
            catalog
                .admitted(&document.workflow.iter().cloned().collect())
                .unwrap()
                .get(&name)
                .is_some()
        );
        assert!(
            catalog
                .admitted(&document.main.iter().cloned().collect())
                .unwrap()
                .get(&name)
                .is_none()
        );
        std::fs::remove_file(project.join("reviewer.md")).unwrap();
        let (catalog, sources) = load(&workspace, &user, &document).unwrap();
        assert_eq!(catalog.get(&name).unwrap().instructions(), "User body");
        assert_eq!(sources[&name].selected, user.join("reviewer.md"));
        assert_eq!(sources[&name].layer, "user");
        assert_eq!(sources[&name].overridden, None);
        document.definitions.push(name);
        assert!(
            load(&workspace, &user, &document)
                .unwrap_err()
                .to_string()
                .contains("duplicate")
        );
    }

    #[cfg(unix)]
    #[test]
    fn cfg236_role_and_supplemental_paths_cannot_escape_resource_authority() {
        let dir = tempfile::tempdir().unwrap();
        let (workspace, user) = canonical_authority_roots(dir.path());
        let project = workspace.join(".agents/subagents");
        let outside = dir.path().join("outside.md");
        std::fs::write(&outside, "---\ndescription: outside\n---\nOutside").unwrap();
        let document = SubagentsDocument {
            definitions: vec![SubagentName::parse("reviewer").unwrap()],
            ..Default::default()
        };
        let path = project.join("reviewer.md");
        std::os::unix::fs::symlink(&outside, &path).unwrap();
        let error = load(&workspace, &user, &document).unwrap_err();
        assert!(error.to_string().contains("outside trusted workspace"));
        assert_eq!(error.source_file, Some(path.clone()));
        std::fs::remove_file(&path).unwrap();
        let other_role = project.join("other.md");
        std::fs::write(&other_role, "---\ndescription: other\n---\nOther identity").unwrap();
        std::os::unix::fs::symlink(&other_role, &path).unwrap();
        assert!(
            load(&workspace, &user, &document)
                .unwrap_err()
                .to_string()
                .contains("must not be a symlink")
        );
        std::fs::remove_file(&path).unwrap();
        std::fs::write(
            &path,
            "---\ndescription: reviewer\nagentsMd: {files: [../outside.md]}\n---\nRole",
        )
        .unwrap();
        assert!(
            load(&workspace, &user, &document)
                .unwrap_err()
                .to_string()
                .contains("outside trusted workspace")
        );
        assert_eq!(
            std::fs::read_to_string(&outside).unwrap(),
            "---\ndescription: outside\n---\nOutside"
        );

        // Guidance may resolve through an in-boundary alias, unlike role identity.
        let guidance = workspace.join("guidance.md");
        std::os::unix::fs::symlink(&other_role, &guidance).unwrap();
        std::fs::write(
            &path,
            "---\ndescription: reviewer\nagentsMd: {files: [guidance.md]}\n---\nRole",
        )
        .unwrap();
        load(&workspace, &user, &document).unwrap();
        std::fs::remove_file(&guidance).unwrap();
        std::os::unix::fs::symlink(&outside, &guidance).unwrap();
        assert!(
            load(&workspace, &user, &document)
                .unwrap_err()
                .to_string()
                .contains("outside trusted workspace")
        );

        // A project replacement cannot read guidance from the user's authority.
        std::fs::write(user.join("guidance.md"), "User guidance").unwrap();
        std::fs::write(
            &path,
            format!(
                "---\ndescription: reviewer\nagentsMd: {{files: ['{}']}}\n---\nRole",
                user.join("guidance.md").display()
            ),
        )
        .unwrap();
        assert!(
            load(&workspace, &user, &document)
                .unwrap_err()
                .to_string()
                .contains("outside trusted workspace")
        );
        std::fs::remove_file(&path).unwrap();
        let user_role = user.join("reviewer.md");
        std::fs::write(
            &user_role,
            "---\ndescription: reviewer\nagentsMd: {files: [guidance.md]}\n---\nUser role",
        )
        .unwrap();
        load(&workspace, &user, &document).unwrap();
        std::fs::remove_file(user.join("guidance.md")).unwrap();
        std::os::unix::fs::symlink(&outside, user.join("guidance.md")).unwrap();
        assert!(
            load(&workspace, &user, &document)
                .unwrap_err()
                .to_string()
                .contains("outside trusted workspace")
        );
    }

    #[cfg(unix)]
    #[test]
    fn cfg236_missing_leaf_and_replaced_root_cannot_rebind_authority() {
        let dir = tempfile::tempdir().unwrap();
        let (workspace, user) = canonical_authority_roots(dir.path());
        std::fs::write(user.join("reviewer.md"), "Outside workspace").unwrap();
        // Resolving a missing leaf must still detect its escaped existing ancestor.
        std::os::unix::fs::symlink(&user, workspace.join("escape")).unwrap();
        assert!(
            validate_project_resource_path(&workspace, &workspace.join("escape/missing.md"))
                .unwrap_err()
                .to_string()
                .contains("outside trusted workspace")
        );
        // Capture precedes replacement: the validator must not follow a new root.
        std::fs::rename(&workspace, dir.path().join("retired-project")).unwrap();
        std::os::unix::fs::symlink(&user, &workspace).unwrap();
        assert!(
            validate_project_resource_path(&workspace, &workspace.join("reviewer.md"))
                .unwrap_err()
                .to_string()
                .contains("outside trusted workspace")
        );
    }
}
