use super::{
    SettingsError, SourceDirectory, SourceScope, SourceTarget, UserConfigManager, revision,
};
use crate::capabilities::{ToolSourceId, selection::SourceToolSelection};
use crate::local_runtime::{
    agent_resources, config::resolve_mcp_entry, mcp_resources::McpDocument, resource_directory,
    workflow_authoring, workflow_resources,
};
use crate::runtime::{identity::McpServerId, subagent::SubagentName, workflow::WorkflowId};
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
        }
    }

    fn selected(&self, document: super::RuntimeLayer) -> bool {
        let Some(agent) = document.agent else {
            return false;
        };
        match self {
            Self::Workflow { destination, .. } => {
                agent.workflows.is_some_and(|ids| ids.contains(destination))
            }
            Self::Agent { destination, .. } => {
                agent.agents.is_some_and(|ids| ids.contains(destination))
            }
            Self::Mcp { destination, .. } => {
                let id = ToolSourceId::Mcp(destination.clone());
                agent
                    .tools
                    .and_then(|tools| tools.sources)
                    .and_then(|mut sources| sources.remove(&id))
                    .is_some_and(|selection| match selection {
                        SourceToolSelection::All => true,
                        SourceToolSelection::Exact(names) => !names.is_empty(),
                    })
            }
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
        }
        if destination.is_some() {
            return Err(SettingsError::TransferRejected(
                TransferRejection::NameTaken,
            ));
        }
        Ok((None, source.to_vec()))
    }
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
            let (workspace, _) = self.capture_authored_settings(workspace, None)?;
            // Check both views: a Workspace may override the root Agent or an
            // MCP selection independently of what Global itself selects.
            for document in [global, workspace] {
                if resource.selected(document.ok_or(SettingsError::Invalid)?) {
                    return Err(SettingsError::TransferRejected(
                        TransferRejection::NameEnabled,
                    ));
                }
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
