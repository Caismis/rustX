use super::{
    SettingsError, SourceDirectory, SourceScope, SourceTarget, UserConfigManager, revision,
};
use crate::capabilities::{ToolSourceId, selection::SourceToolSelection};
use crate::local_runtime::{
    agent_resources, config::resolve_mcp_entry, mcp_resources::McpDocument, resource_directory,
    workflow_authoring, workflow_resources,
};
use crate::runtime::{
    identity::McpServerId,
    subagent::{AgentCatalog, NamedAgentDefinition, SubagentName},
    workflow::{WorkflowCatalog, WorkflowId, WorkflowProgram},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransferOperation {
    Copy,
    Move,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "family", rename_all = "snake_case", deny_unknown_fields)]
pub enum ResourceTransfer {
    Workflow {
        source: WorkflowId,
        destination: WorkflowId,
    },
    Agent {
        source: SubagentName,
        destination: SubagentName,
    },
    Mcp {
        source: McpServerId,
        destination: McpServerId,
    },
    Skill {
        source: String,
        destination: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum TransferRejection {
    NameTaken,
    NameEnabled,
    ProtectedMcpValues,
    SameDirectory,
}

impl ResourceTransfer {
    fn path(
        &self,
        manager: &UserConfigManager,
        target: &SourceTarget,
        destination: bool,
    ) -> PathBuf {
        match self {
            Self::Workflow {
                source,
                destination: id,
            } => workflow_authoring::file_path(
                manager,
                target,
                if destination { id } else { source },
            ),
            Self::Agent {
                source,
                destination: id,
            } => manager
                .resource_root(target)
                .join("agents")
                .join(format!("{}.toml", if destination { id } else { source })),
            Self::Mcp { .. } => manager.resource_root(target).join("mcp.toml"),
            Self::Skill { .. } => unreachable!(),
        }
    }

    fn same_name(&self) -> bool {
        match self {
            Self::Workflow {
                source,
                destination,
            } => source == destination,
            Self::Agent {
                source,
                destination,
            } => source == destination,
            Self::Mcp {
                source,
                destination,
            } => source == destination,
            Self::Skill {
                source,
                destination,
            } => source == destination,
        }
    }

    fn selected(&self, document: &super::RuntimeLayer) -> bool {
        let Some(agent) = document.agent.as_ref() else {
            return false;
        };
        match self {
            Self::Workflow { destination, .. } => agent
                .workflows
                .as_ref()
                .is_some_and(|ids| ids.contains(destination)),
            Self::Agent { destination, .. } => agent
                .agents
                .as_ref()
                .is_some_and(|ids| ids.contains(destination)),
            Self::Mcp { destination, .. } => {
                let id = ToolSourceId::Mcp(destination.clone());
                agent
                    .tools
                    .as_ref()
                    .and_then(|tools| tools.sources.as_ref())
                    .and_then(|sources| sources.get(&id))
                    .is_some_and(|selection| match selection {
                        SourceToolSelection::All => true,
                        SourceToolSelection::Exact(names) => !names.is_empty(),
                    })
            }
            Self::Skill { .. } => unreachable!(),
        }
    }

    /// Build both candidates before committing either. MCP documents contain
    /// unrelated entries and are edited as maps; standalone files keep their
    /// exact authored bytes, including comments and numeric lexemes.
    fn candidates(
        &self,
        source: &[u8],
        destination: Option<&[u8]>,
        source_path: &Path,
        to: SourceScope,
        moving: bool,
    ) -> Result<(Option<Vec<u8>>, Vec<u8>), SettingsError> {
        match self {
            Self::Workflow { source: id, .. } => {
                workflow_resources::parse_source(id, source_path, source).map_err(|error| {
                    SettingsError::InvalidWorkflow(Box::new(
                        workflow_authoring::WorkflowDiagnostic::from_resource(&error),
                    ))
                })?;
            }
            Self::Agent { .. } => {
                let text = std::str::from_utf8(source).map_err(|_| SettingsError::Invalid)?;
                agent_resources::parse(text).map_err(|_| SettingsError::Invalid)?;
            }
            Self::Mcp {
                source: id,
                destination: name,
            } => {
                let mut original: McpDocument =
                    crate::toml_authoring::parse(source).map_err(|_| SettingsError::Invalid)?;
                let mut target: McpDocument =
                    crate::toml_authoring::parse(destination.unwrap_or_default())
                        .map_err(|_| SettingsError::Invalid)?;
                if target.mcp_servers.contains_key(name)
                    || name != id && original.mcp_servers.contains_key(name)
                {
                    return Err(SettingsError::TransferRejected(
                        TransferRejection::NameTaken,
                    ));
                }
                let definition = original
                    .mcp_servers
                    .remove(id)
                    .ok_or(SettingsError::Invalid)?;
                // A Workspace file can be browsed, committed, or shared. Keep
                // redacted Global values inside their existing authority.
                if to == SourceScope::Workspace
                    && (!definition.env.is_empty() || !definition.headers.is_empty())
                {
                    return Err(SettingsError::TransferRejected(
                        TransferRejection::ProtectedMcpValues,
                    ));
                }
                resolve_mcp_entry(name, &definition.clone().resolve())
                    .map_err(|_| SettingsError::Invalid)?;
                if target.mcp_servers.len()
                    >= crate::local_runtime::mcp_resources::MAX_MCP_DEFINITIONS
                {
                    return Err(SettingsError::Invalid);
                }
                target.mcp_servers.insert(name.clone(), definition);
                let encode = |document: &McpDocument| {
                    crate::toml_authoring::write(document)
                        .map(String::into_bytes)
                        .map_err(|_| SettingsError::Invalid)
                };
                return Ok((
                    moving.then(|| encode(&original)).transpose()?,
                    encode(&target)?,
                ));
            }
            Self::Skill { .. } => unreachable!(),
        }
        if destination.is_some() {
            return Err(SettingsError::TransferRejected(
                TransferRejection::NameTaken,
            ));
        }
        Ok((None, source.to_vec()))
    }
}

fn selects_mcp(agent: &NamedAgentDefinition, id: &McpServerId) -> bool {
    let source = ToolSourceId::Mcp(id.clone());
    agent
        .tools()
        .iter()
        .any(|selector| selector.source() == Some(&source))
}

/// Programs whose winning file [`WorkflowCatalog::enabled_by_owning_scope`]
/// accepts. `get` still includes a disabled source, so the shared scope rule
/// decides; this only loads that program for inspection.
fn enabled_programs<'a>(
    catalog: &'a WorkflowCatalog,
    user_selected: &[WorkflowId],
    workspace_selected: &[WorkflowId],
) -> Vec<&'a WorkflowProgram> {
    catalog
        .locations
        .keys()
        .filter(|id| catalog.enabled_by_owning_scope(id, user_selected, workspace_selected))
        .filter_map(|id| catalog.get(id).map(AsRef::as_ref))
        .collect()
}

impl UserConfigManager {
    fn admit_agent_transfer(
        &self,
        workspace: &SourceTarget,
        to: SourceScope,
        name: &SubagentName,
    ) -> Result<(), SettingsError> {
        let mut names = BTreeSet::from([name.clone()]);
        for target in [&SourceTarget::User, workspace] {
            let root = self.resource_root(target);
            let entries = resource_directory::entries(&root, &root.join("agents"))
                .map_err(|_| SettingsError::Invalid)?;
            let profiles = entries
                .iter()
                .filter(|path| path.extension().is_some_and(|ext| ext == "toml"));
            if target.scope() == to
                && (entries.len() + 2 > resource_directory::MAX_DIRECTORY_ENTRIES
                    || profiles.clone().count() >= resource_directory::MAX_RESOURCE_FILES)
            {
                return Err(SettingsError::Invalid);
            }
            names.extend(
                profiles.filter_map(|path| SubagentName::parse(path.file_stem()?.to_str()?).ok()),
            );
        }
        if names.len() > crate::runtime::subagent::MAX_SUBAGENT_DEFINITIONS {
            return Err(SettingsError::Invalid);
        }
        Ok(())
    }

    /// Whether a copy would satisfy a consumer the root switch does not name.
    ///
    /// Named Agents selected by this layer contribute MCP selectors. `programs`
    /// are the Workflows whose winning file is enabled by that file's own scope,
    /// the same rule Session capture uses. Those programs contribute direct tool
    /// selectors and Agent-node profiles, including nodes nested in Loop and
    /// Iteration bodies.
    fn scope_consumers_select(
        document: &super::RuntimeLayer,
        agents: &AgentCatalog,
        programs: &[&WorkflowProgram],
        resource: &ResourceTransfer,
    ) -> bool {
        let Some(agent) = document.agent.as_ref() else {
            return false;
        };
        let selected_agents = agent.agents.as_deref().unwrap_or(&[]);
        match resource {
            ResourceTransfer::Mcp { destination, .. } => {
                selected_agents.iter().any(|name| {
                    agents
                        .get(name)
                        .is_some_and(|definition| selects_mcp(definition, destination))
                }) || programs.iter().any(|program| {
                    let inspection = program.inspect();
                    inspection.tools.iter().any(|selector| {
                        matches!(selector.source(), Some(ToolSourceId::Mcp(source)) if source == destination)
                    }) || inspection.profiles.iter().any(|name| {
                        agents
                            .get(name)
                            .is_some_and(|definition| selects_mcp(definition, destination))
                    })
                })
            }
            ResourceTransfer::Agent { destination, .. } => programs
                .iter()
                .any(|program| program.inspect().profiles.contains(destination)),
            ResourceTransfer::Workflow { .. } | ResourceTransfer::Skill { .. } => false,
        }
    }

    #[allow(clippy::too_many_lines)] // One ordered, revision-fenced transfer.
    pub(super) fn transfer_resource(
        &self,
        workspace: &SourceTarget,
        from: SourceScope,
        operation: TransferOperation,
        resource: &ResourceTransfer,
        expected: &str,
    ) -> Result<String, SettingsError> {
        if workspace.scope() != SourceScope::Workspace {
            return Err(SettingsError::Invalid);
        }
        let moving = operation == TransferOperation::Move;
        if !moving && resource.same_name() {
            return Err(SettingsError::TransferRejected(
                TransferRejection::NameTaken,
            ));
        }
        let user = &SourceTarget::User;
        let (source_target, destination_target) = if from == SourceScope::User {
            (user, workspace)
        } else {
            (workspace, user)
        };
        if let ResourceTransfer::Skill {
            source,
            destination,
        } = resource
        {
            return self.transfer_skill(
                source_target,
                destination_target,
                operation,
                source,
                destination,
                expected,
            );
        }
        let capacity = match resource {
            ResourceTransfer::Workflow { destination, .. } => Some((
                workflow_authoring::Capacity::lock(self, destination_target)?,
                destination,
            )),
            _ => None,
        };
        let source_path = resource.path(self, source_target, false);
        let destination_path = resource.path(self, destination_target, true);
        let source_name = source_path.file_name().ok_or(SettingsError::Invalid)?;
        let destination_name = destination_path.file_name().ok_or(SettingsError::Invalid)?;
        let bounded = |directory: SourceDirectory| {
            if matches!(resource, ResourceTransfer::Workflow { .. }) {
                directory.with_byte_limit(crate::workflow_file::MAX_WORKFLOW_FILE_BYTES as u64)
            } else {
                directory
            }
        };
        let source = SourceDirectory::open(&source_path)
            .map_err(|_| SettingsError::Io)?
            .map(bounded)
            .ok_or(SettingsError::Invalid)?;
        let destination =
            bounded(SourceDirectory::create(&destination_path).map_err(|_| SettingsError::Io)?);
        if source.identity() == destination.identity() {
            return Err(SettingsError::TransferRejected(
                TransferRejection::SameDirectory,
            ));
        }
        // Order all transfers User before Workspace, including opposite moves.
        let (first, second) = if from == SourceScope::User {
            ((&source, source_name), (&destination, destination_name))
        } else {
            ((&destination, destination_name), (&source, source_name))
        };
        let _first = first.0.lock(first.1).map_err(|_| SettingsError::Io)?;
        let _second = second.0.lock(second.1).map_err(|_| SettingsError::Io)?;
        let original = source.read(source_name).map_err(|_| SettingsError::Io)?;
        let actual = revision(original.as_deref());
        if actual != expected {
            return Err(SettingsError::Conflict {
                scope: from,
                expected: expected.into(),
                actual,
            });
        }
        let original = original.ok_or(SettingsError::Invalid)?;
        let existing = destination
            .read(destination_name)
            .map_err(|_| SettingsError::Io)?;
        let (remainder, candidate) = resource.candidates(
            &original,
            existing.as_deref(),
            &source_path,
            destination_target.scope(),
            moving,
        )?;
        if source_name != destination_name
            && source
                .read(destination_name)
                .map_err(|_| SettingsError::Io)?
                .is_some()
        {
            return Err(SettingsError::TransferRejected(
                TransferRejection::NameTaken,
            ));
        }
        if let Some((capacity, id)) = &capacity {
            capacity.admit(id)?;
        }
        if let ResourceTransfer::Agent { destination, .. } = resource {
            self.admit_agent_transfer(workspace, destination_target.scope(), destination)?;
        }
        if !moving {
            let (global, _) = self.capture_authored_settings(user, None)?;
            let (workspace_layer, _) = self.capture_authored_settings(workspace, None)?;
            let global = global.ok_or(SettingsError::Invalid)?;
            let workspace_layer = workspace_layer.ok_or(SettingsError::Invalid)?;
            let user_agents_root = self.resource_root(user).join("agents");
            let user_agents = agent_resources::load_authorized(None, &user_agents_root)
                .map_err(|_| SettingsError::Invalid)?;
            let workspace_agents =
                agent_resources::load_authorized(workspace.workspace(), &user_agents_root)
                    .map_err(|_| SettingsError::Invalid)?;
            // Discovery shadows definitions: User files, then Workspace files
            // that replace them. That chooses the winning file, not whether it
            // runs. Enablement is the owning scope's raw selection.
            let (user_selected, workspace_selected) = self.scoped_workflow_selections(workspace)?;
            // Global's own catalog, then the layered catalog Session capture
            // uses. Each view applies the same owning-scope rule to its own
            // winning files; a Workspace shadow is not a Global consumer.
            let user_workflows = workflow_resources::load(None, &self.resource_root(user))
                .map_err(|_| SettingsError::Io)?;
            let workflows =
                workflow_resources::load(workspace.workspace(), &self.resource_root(user))
                    .map_err(|_| SettingsError::Io)?;
            let user_programs =
                enabled_programs(&user_workflows, &user_selected, &workspace_selected);
            let workspace_programs =
                enabled_programs(&workflows, &user_selected, &workspace_selected);
            if resource.selected(&global)
                || Self::scope_consumers_select(&global, &user_agents.0, &user_programs, resource)
                || resource.selected(&workspace_layer)
                || Self::scope_consumers_select(
                    &workspace_layer,
                    &workspace_agents.0,
                    &workspace_programs,
                    resource,
                )
            {
                return Err(SettingsError::TransferRejected(
                    TransferRejection::NameEnabled,
                ));
            }
        }
        if !source.holds() {
            return Err(SettingsError::Invalid);
        }
        let actual = revision(
            source
                .read(source_name)
                .map_err(|_| SettingsError::Io)?
                .as_deref(),
        );
        if actual != expected {
            return Err(SettingsError::Conflict {
                scope: from,
                expected: expected.into(),
                actual,
            });
        }
        self.publish_locked(
            &destination,
            destination_name,
            destination_target.scope(),
            &revision(existing.as_deref()),
            Some(&candidate),
            existing.is_some(),
        )?;
        if moving {
            // The destination is durable. Keep both definitions if the source
            // changed or deletion could not be confirmed, and require a reread.
            self.publish_locked(
                &source,
                source_name,
                from,
                expected,
                remainder.as_deref(),
                true,
            )
            .map_err(|_| SettingsError::Committed)?;
        }
        Ok(revision(Some(&candidate)))
    }
}

#[cfg(test)]
mod tests {
    use super::super::super::super::launch::{HostEnvironment, LaunchRequest};
    use super::super::{SettingsError, SourceTarget, UserConfigManager};
    use super::{ResourceTransfer, TransferOperation, TransferRejection};
    use crate::runtime::resources::SourceScope;
    use crate::runtime::subagent::SubagentName;
    use crate::runtime::workflow::{WorkflowId, WorkflowProgram};
    use std::fs;
    use std::path::{Path, PathBuf};

    fn fixture() -> (tempfile::TempDir, UserConfigManager, SourceTarget) {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().canonicalize().unwrap();
        let home = root.join("home");
        let workspace = root.join("workspace");
        fs::create_dir_all(home.join("rustx")).unwrap();
        fs::create_dir_all(&workspace).unwrap();
        fs::write(
            home.join("rustx/rustx.toml"),
            r#"
[providers.local]
protocol = "openai_responses"
base_url = "http://127.0.0.1:9/v1"
api_key = "$RUSTX_TRANSFER_UNUSED"
[models.local]
provider = "local"
id = "local"
context_window = 128000
max_output_tokens = 8192
[models.local.capabilities]
input_modalities = ["text"]
output_modalities = ["text"]
[agent.model]
model = "local"
"#,
        )
        .unwrap();
        let host = HostEnvironment::from_paths(workspace.clone(), home).unwrap();
        let request = LaunchRequest {
            workspace: Some(workspace.clone()),
            ..LaunchRequest::default()
        };
        let manager = request.session_input(&host).unwrap().0;
        (
            temporary,
            manager,
            SourceTarget::Workspace {
                directory: workspace,
            },
        )
    }

    fn revision_of(path: &Path) -> String {
        super::super::revision(Some(&fs::read(path).unwrap()))
    }

    fn copy(
        manager: &UserConfigManager,
        workspace: &SourceTarget,
        resource: &ResourceTransfer,
        expected: &str,
    ) -> Result<String, SettingsError> {
        manager.transfer_resource(
            workspace,
            SourceScope::User,
            TransferOperation::Copy,
            resource,
            expected,
        )
    }

    const NESTED_WORKFLOW: &str = r"
description: Nested consumers.
block:
  input:
    type: object
    properties:
      files:
        type: array
        items: {type: string}
      proposal: {type: string}
    required: [files, proposal]
    additionalProperties: false
  nodes:
    workflow_start: {type: start}
    repair:
      type: loop
      loop_count: 2
      loop_variables:
      - {label: passed, var_type: boolean, value: {type: literal, value: false}}
      body:
        nodes:
          repair_start: {type: start}
          review:
            type: agent
            profile: review-copy
            task:
            - {type: literal, value: Review}
            output:
              type: json_schema
              schema:
                type: object
                properties: {summary: {type: string}}
                required: [summary]
                additionalProperties: false
        edges:
        - {from: repair_start, to: review}
    each:
      type: iteration
      iterator: {type: reference, path: [args, files]}
      output: {type: reference, path: [each, item]}
      body:
        nodes:
          each_start: {type: start}
          inspect:
            type: tool
            selector: {origin: source, source_id: review-copy, name: inspect}
            arguments: {type: literal, value: {}}
            result: {type: text}
        edges:
        - {from: each_start, to: inspect}
    done: {type: return, outputs: []}
  edges:
  - {from: workflow_start, to: repair}
  - {from: repair, to: each}
  - {from: each, to: done}
";

    fn install_workflow_at(path: &Path, yaml: &str) {
        let document: crate::workflow_file::WorkflowFileDocument =
            serde_yaml_ng::from_slice(yaml.as_bytes()).unwrap();
        WorkflowProgram::compile_document(WorkflowId::parse("pipeline").unwrap(), "test", document)
            .unwrap_or_else(|error| panic!("workflow fixture does not compile: {error}"));
        let _ = path;
    }

    fn install_workflow(workspace: &Path, yaml: &str) -> PathBuf {
        let path = workspace.join(".agents/workflows/pipeline.yaml");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, yaml).unwrap();
        install_workflow_at(&path, yaml);
        path
    }

    fn mcp_transfer() -> ResourceTransfer {
        ResourceTransfer::Mcp {
            source: crate::runtime::identity::McpServerId::new("review"),
            destination: crate::runtime::identity::McpServerId::new("review-copy"),
        }
    }

    fn agent_transfer() -> ResourceTransfer {
        ResourceTransfer::Agent {
            source: SubagentName::parse("original").unwrap(),
            destination: SubagentName::parse("review-copy").unwrap(),
        }
    }

    fn assert_name_enabled(result: &Result<String, SettingsError>, label: &str) {
        assert!(
            matches!(
                result,
                Err(SettingsError::TransferRejected(
                    TransferRejection::NameEnabled
                ))
            ),
            "{label}: {result:?}"
        );
    }

    #[test]
    fn copy_rejects_an_enabled_named_agent_mcp_selector_without_publishing() {
        let (_temporary, manager, workspace) = fixture();
        let workspace_dir = workspace.workspace().unwrap();
        let user_mcp = manager.resource_root(&SourceTarget::User).join("mcp.toml");
        fs::create_dir_all(user_mcp.parent().unwrap()).unwrap();
        fs::write(
            &user_mcp,
            "[mcp_servers.review]\nurl = 'https://example.invalid/mcp'\n",
        )
        .unwrap();
        let reviewer = workspace_dir.join(".agents/agents/reviewer.toml");
        fs::create_dir_all(reviewer.parent().unwrap()).unwrap();
        fs::write(
            &reviewer,
            "description = 'Review'\ninstructions = 'Review input'\n[tools.sources]\nreview-copy = ['inspect']\n",
        )
        .unwrap();
        fs::write(
            workspace_dir.join("rustx.toml"),
            "[agent]\nagents = ['reviewer']\n",
        )
        .unwrap();
        let destination = workspace_dir.join(".agents/mcp.toml");
        let revision = revision_of(&user_mcp);
        assert_name_enabled(
            &copy(&manager, &workspace, &mcp_transfer(), &revision),
            "enabled named agent",
        );
        assert!(
            !fs::read_to_string(&destination)
                .unwrap_or_default()
                .contains("review-copy")
        );

        fs::write(
            &reviewer,
            "description = 'Review'\ninstructions = 'Review input'\n[tools.sources]\n",
        )
        .unwrap();
        copy(&manager, &workspace, &mcp_transfer(), &revision)
            .expect("clearing the named-agent selector allows the copy");
        assert!(
            fs::read_to_string(&destination)
                .unwrap()
                .contains("review-copy")
        );
    }

    #[test]
    fn copy_rejects_selected_workflow_profile_and_tool_consumers_without_publishing() {
        let (_temporary, manager, workspace) = fixture();
        let workspace_dir = workspace.workspace().unwrap().to_path_buf();
        let user_agents = manager.resource_root(&SourceTarget::User).join("agents");
        fs::create_dir_all(&user_agents).unwrap();
        let source_agent = user_agents.join("original.toml");
        fs::write(
            &source_agent,
            "description = 'Review'\ninstructions = 'Review input'\n",
        )
        .unwrap();
        let user_mcp = manager.resource_root(&SourceTarget::User).join("mcp.toml");
        fs::write(
            &user_mcp,
            "[mcp_servers.review]\nurl = 'https://example.invalid/mcp'\n",
        )
        .unwrap();
        fs::write(
            workspace_dir.join("rustx.toml"),
            "[agent]\nworkflows = ['pipeline']\n",
        )
        .unwrap();
        let workflow = install_workflow(&workspace_dir, NESTED_WORKFLOW);
        let agent_destination = workspace_dir.join(".agents/agents/review-copy.toml");
        let mcp_destination = workspace_dir.join(".agents/mcp.toml");
        let agent_revision = revision_of(&source_agent);
        let mcp_revision = revision_of(&user_mcp);

        assert_name_enabled(
            &copy(&manager, &workspace, &agent_transfer(), &agent_revision),
            "nested loop agent profile",
        );
        assert!(!agent_destination.exists());
        assert_name_enabled(
            &copy(&manager, &workspace, &mcp_transfer(), &mcp_revision),
            "nested iteration tool",
        );
        assert!(
            !fs::read_to_string(&mcp_destination)
                .unwrap_or_default()
                .contains("review-copy")
        );

        let cleared_tool = NESTED_WORKFLOW.replace(
            "selector: {origin: source, source_id: review-copy, name: inspect}",
            "selector: {origin: builtin, name: read}",
        );
        fs::write(&workflow, &cleared_tool).unwrap();
        install_workflow(&workspace_dir, &cleared_tool);
        copy(&manager, &workspace, &mcp_transfer(), &mcp_revision)
            .expect("clearing the nested tool dependency allows the MCP copy");
        assert!(
            fs::read_to_string(&mcp_destination)
                .unwrap()
                .contains("review-copy")
        );

        let cleared_profile = cleared_tool.replace("profile: review-copy", "profile: reviewer");
        fs::create_dir_all(workspace_dir.join(".agents/agents")).unwrap();
        fs::write(
            workspace_dir.join(".agents/agents/reviewer.toml"),
            "description = 'Review'\ninstructions = 'Review input'\n",
        )
        .unwrap();
        fs::write(&workflow, &cleared_profile).unwrap();
        install_workflow(&workspace_dir, &cleared_profile);
        copy(&manager, &workspace, &agent_transfer(), &agent_revision)
            .expect("clearing the nested profile reference allows the agent copy");
        assert!(agent_destination.is_file());
    }

    #[test]
    fn copy_allows_a_global_workflow_selected_only_by_the_workspace() {
        let (_temporary, manager, workspace) = fixture();
        let workspace_dir = workspace.workspace().unwrap();
        let user_root = manager.resource_root(&SourceTarget::User);
        fs::create_dir_all(user_root.join("agents")).unwrap();
        let source_agent = user_root.join("agents/original.toml");
        fs::write(
            &source_agent,
            "description = 'Review'\ninstructions = 'Review input'\n",
        )
        .unwrap();
        let user_mcp = user_root.join("mcp.toml");
        fs::write(
            &user_mcp,
            "[mcp_servers.review]\nurl = 'https://example.invalid/mcp'\n",
        )
        .unwrap();
        let global_workflow = user_root.join("workflows/pipeline.yaml");
        fs::create_dir_all(global_workflow.parent().unwrap()).unwrap();
        fs::write(&global_workflow, NESTED_WORKFLOW).unwrap();
        install_workflow_at(&global_workflow, NESTED_WORKFLOW);
        fs::write(
            workspace_dir.join("rustx.toml"),
            "[agent]\nworkflows = ['pipeline']\n",
        )
        .unwrap();
        let agent_destination = workspace_dir.join(".agents/agents/review-copy.toml");
        let mcp_destination = workspace_dir.join(".agents/mcp.toml");
        copy(
            &manager,
            &workspace,
            &agent_transfer(),
            &revision_of(&source_agent),
        )
        .expect("a Workspace list does not enable the Global file it does not own");
        assert!(agent_destination.is_file());
        copy(
            &manager,
            &workspace,
            &mcp_transfer(),
            &revision_of(&user_mcp),
        )
        .expect("a Workspace list does not enable the Global file it does not own");
        assert!(
            fs::read_to_string(&mcp_destination)
                .unwrap()
                .contains("review-copy")
        );

        fs::remove_file(&agent_destination).unwrap();
        fs::remove_file(&mcp_destination).unwrap();
        let user_config = user_root.parent().unwrap().join("rustx.toml");
        let config = fs::read_to_string(&user_config).unwrap().replace(
            "[agent.model]\nmodel = \"local\"\n",
            "[agent]\nworkflows = ['pipeline']\n\n[agent.model]\nmodel = \"local\"\n",
        );
        fs::write(&user_config, config).unwrap();
        assert_name_enabled(
            &copy(
                &manager,
                &workspace,
                &agent_transfer(),
                &revision_of(&source_agent),
            ),
            "the Global file's own scope enables its profile",
        );
        assert!(!agent_destination.exists());
        assert_name_enabled(
            &copy(
                &manager,
                &workspace,
                &mcp_transfer(),
                &revision_of(&user_mcp),
            ),
            "the Global file's own scope enables its tool",
        );
        assert!(
            !fs::read_to_string(&mcp_destination)
                .unwrap_or_default()
                .contains("review-copy")
        );
    }

    /// Global `pipeline` is enabled and does not reference the copy. The
    /// Workspace file of the same name does, and is not selected.
    fn disabled_workspace_shadow() -> (
        tempfile::TempDir,
        UserConfigManager,
        SourceTarget,
        PathBuf,
        PathBuf,
    ) {
        let (temporary, manager, workspace) = fixture();
        let workspace_dir = workspace.workspace().unwrap();
        let user_root = manager.resource_root(&SourceTarget::User);
        let user_config = user_root.parent().unwrap().join("rustx.toml");
        let config = fs::read_to_string(&user_config).unwrap().replace(
            "[agent.model]\nmodel = \"local\"\n",
            "[agent]\nworkflows = ['pipeline']\n\n[agent.model]\nmodel = \"local\"\n",
        );
        fs::write(&user_config, config).unwrap();
        fs::create_dir_all(user_root.join("agents")).unwrap();
        let source_agent = user_root.join("agents/original.toml");
        fs::write(
            &source_agent,
            "description = 'Review'\ninstructions = 'Review input'\n",
        )
        .unwrap();
        let user_mcp = user_root.join("mcp.toml");
        fs::write(
            &user_mcp,
            "[mcp_servers.review]\nurl = 'https://example.invalid/mcp'\n",
        )
        .unwrap();
        let quiet = NESTED_WORKFLOW
            .replace("profile: review-copy", "profile: reviewer")
            .replace(
                "selector: {origin: source, source_id: review-copy, name: inspect}",
                "selector: {origin: builtin, name: read}",
            );
        let global_workflow = user_root.join("workflows/pipeline.yaml");
        fs::create_dir_all(global_workflow.parent().unwrap()).unwrap();
        fs::write(&global_workflow, &quiet).unwrap();
        install_workflow_at(&global_workflow, &quiet);
        fs::create_dir_all(workspace_dir.join(".agents/agents")).unwrap();
        fs::write(
            workspace_dir.join(".agents/agents/reviewer.toml"),
            "description = 'Review'\ninstructions = 'Review input'\n",
        )
        .unwrap();
        install_workflow(workspace_dir, NESTED_WORKFLOW);
        fs::write(
            workspace_dir.join("rustx.toml"),
            "[agent]\nworkflows = []\n",
        )
        .unwrap();
        (temporary, manager, workspace, source_agent, user_mcp)
    }

    #[test]
    fn copy_allows_a_disabled_workspace_shadow() {
        let (_temporary, manager, workspace, source_agent, user_mcp) = disabled_workspace_shadow();
        let workspace_dir = workspace.workspace().unwrap();
        let agent_destination = workspace_dir.join(".agents/agents/review-copy.toml");
        let mcp_destination = workspace_dir.join(".agents/mcp.toml");
        copy(
            &manager,
            &workspace,
            &agent_transfer(),
            &revision_of(&source_agent),
        )
        .expect("a disabled Workspace shadow is not a consumer");
        assert!(agent_destination.is_file());
        copy(
            &manager,
            &workspace,
            &mcp_transfer(),
            &revision_of(&user_mcp),
        )
        .expect("a disabled Workspace shadow is not a consumer");
        assert!(
            fs::read_to_string(&mcp_destination)
                .unwrap()
                .contains("review-copy")
        );
        assert!(
            !fs::read_to_string(workspace_dir.join("rustx.toml"))
                .unwrap()
                .contains("pipeline")
        );
    }

    #[test]
    fn copy_rejects_a_workspace_shadow_enabled_by_its_owner() {
        let (_temporary, manager, workspace, source_agent, user_mcp) = disabled_workspace_shadow();
        let workspace_dir = workspace.workspace().unwrap();
        fs::write(
            workspace_dir.join("rustx.toml"),
            "[agent]\nworkflows = ['pipeline']\n",
        )
        .unwrap();
        let agent_destination = workspace_dir.join(".agents/agents/review-copy.toml");
        let mcp_destination = workspace_dir.join(".agents/mcp.toml");
        assert_name_enabled(
            &copy(
                &manager,
                &workspace,
                &agent_transfer(),
                &revision_of(&source_agent),
            ),
            "owning scope enables the shadowing profile",
        );
        assert!(!agent_destination.exists());
        assert_name_enabled(
            &copy(
                &manager,
                &workspace,
                &mcp_transfer(),
                &revision_of(&user_mcp),
            ),
            "owning scope enables the shadowing tool",
        );
        assert!(
            !fs::read_to_string(&mcp_destination)
                .unwrap_or_default()
                .contains("review-copy")
        );
    }

    #[test]
    fn copy_rejects_an_inherited_named_agent_whose_workspace_profile_selects_the_mcp() {
        let (_temporary, manager, workspace) = fixture();
        let workspace_dir = workspace.workspace().unwrap();
        let user_root = manager.resource_root(&SourceTarget::User);
        let user_config = user_root.parent().unwrap().join("rustx.toml");
        let config = fs::read_to_string(&user_config).unwrap().replace(
            "[agent.model]\nmodel = \"local\"\n",
            "[agent]\nagents = ['reviewer']\n\n[agent.model]\nmodel = \"local\"\n",
        );
        assert!(
            config.contains("agents = ['reviewer']"),
            "fixture agent table was not found"
        );
        fs::write(&user_config, config).unwrap();
        fs::create_dir_all(user_root.join("agents")).unwrap();
        fs::write(
            user_root.join("agents/reviewer.toml"),
            "description = 'Review'\ninstructions = 'Review input'\n[tools.sources]\n",
        )
        .unwrap();
        let user_mcp = user_root.join("mcp.toml");
        fs::write(
            &user_mcp,
            "[mcp_servers.review]\nurl = 'https://example.invalid/mcp'\n",
        )
        .unwrap();
        let override_profile = workspace_dir.join(".agents/agents/reviewer.toml");
        fs::create_dir_all(override_profile.parent().unwrap()).unwrap();
        fs::write(
            &override_profile,
            "description = 'Review'\ninstructions = 'Review input'\n[tools.sources]\nreview-copy = ['inspect']\n",
        )
        .unwrap();
        let destination = workspace_dir.join(".agents/mcp.toml");
        let revision = revision_of(&user_mcp);
        assert_name_enabled(
            &copy(&manager, &workspace, &mcp_transfer(), &revision),
            "inherited named agent with workspace profile override",
        );
        assert!(
            !fs::read_to_string(&destination)
                .unwrap_or_default()
                .contains("review-copy")
        );
        fs::write(
            &override_profile,
            "description = 'Review'\ninstructions = 'Review input'\n[tools.sources]\n",
        )
        .unwrap();
        copy(&manager, &workspace, &mcp_transfer(), &revision)
            .expect("clearing the overriding selector allows the copy");
    }
}
