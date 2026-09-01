//! The immutable active capability snapshot.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::capabilities::tools::AvailableToolCatalog;
use crate::protocol::manifest::CapabilitiesManifest;
use crate::runtime::identity::{CapabilityRevision, ConversationId};
use crate::skills::SkillSnapshot;
use crate::skills::environments::{NodeEnvironment, PythonEnvironment};
use crate::tools::environment::ToolEnvironment;
use crate::tools::executor::ToolRegistry;
use crate::tools::mcp::{McpRuntimeLeaseAuthority, McpRuntimeLeaseSet, McpServerBindings};

/// The immutable capability snapshot observed by one attempt.
///
/// ```text
/// CapabilitySnapshot
/// ├── CapabilityRevision
/// ├── immutable ToolRegistry handle
/// ├── immutable Skill snapshot/catalog
/// ├── SkillId + SkillVersionId bindings
/// ├── Python environment identity/path when present
/// ├── Node environment identity/path when present
/// └── effective ToolEnvironment
/// ```
///
/// The snapshot is fully immutable: an attempt uses exactly this snapshot
/// for its complete lifetime, and Skill candidates never mutate it. The
/// The `ToolRegistry` handle is the immutable registry for this exact
/// capability revision; a later candidate owns a different registry handle.
#[derive(Clone)]
pub struct CapabilitySnapshot {
    conversation_id: ConversationId,
    workspace_root: PathBuf,
    revision: CapabilityRevision,
    tool_registry: Arc<ToolRegistry>,
    available_tools: Arc<AvailableToolCatalog>,
    skills: Arc<SkillSnapshot>,
    python_environment: Option<PythonEnvironment>,
    node_environment: Option<NodeEnvironment>,
    effective_environment: ToolEnvironment,
    /// The physical MCP lease authority paired with this exact immutable
    /// capability generation. It is deliberately part of the snapshot
    /// rather than read from mutable coordinator-current state at attempt
    /// admission.
    mcp_lease_authority: Arc<McpRuntimeLeaseAuthority>,
    /// The MCP server bindings of this exact generation (Issue #145): the
    /// configured set plus the synthesized managed-Python-package bindings
    /// (Issue #174).
    ///
    /// This is source authority, not live runtime state: it is what a
    /// subagent resolution freezes so a child can establish its own
    /// transport to exactly the servers its selected tools need. It is
    /// deliberately part of the immutable snapshot rather than read from
    /// mutable coordinator-current inputs, so a reload between the parent's
    /// freeze and the child's materialization cannot change the transport
    /// the child connects.
    mcp_servers: Arc<McpServerBindings>,
}

/// Two snapshots are equal when their capability content is equal: the
/// revision, ordered canonical tool definitions, Skill snapshot, environment
/// identities, and effective environment. Executor pointer identity is not
/// compared.
impl PartialEq for CapabilitySnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.conversation_id == other.conversation_id
            && self.workspace_root == other.workspace_root
            && self.revision == other.revision
            && self.tool_registry.definitions() == other.tool_registry.definitions()
            && self.available_tools == other.available_tools
            && self.skills == other.skills
            && self.python_environment == other.python_environment
            && self.node_environment == other.node_environment
            && self.effective_environment == other.effective_environment
    }
}

impl core::fmt::Debug for CapabilitySnapshot {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CapabilitySnapshot")
            .field("conversation_id", &self.conversation_id)
            .field("workspace_root", &self.workspace_root)
            .field("revision", &self.revision)
            .field("available_tools", &self.available_tools)
            .field("skills", &self.skills)
            .field("python_environment", &self.python_environment)
            .field("node_environment", &self.node_environment)
            .field("effective_environment", &self.effective_environment)
            .finish_non_exhaustive()
    }
}

impl CapabilitySnapshot {
    /// Builds the immutable snapshot from the prepared candidate pieces.
    #[must_use]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        conversation_id: ConversationId,
        workspace_root: PathBuf,
        revision: CapabilityRevision,
        tool_registry: Arc<ToolRegistry>,
        available_tools: Arc<AvailableToolCatalog>,
        skills: Arc<SkillSnapshot>,
        python_environment: Option<PythonEnvironment>,
        node_environment: Option<NodeEnvironment>,
        effective_environment: ToolEnvironment,
        mcp_lease_authority: Arc<McpRuntimeLeaseAuthority>,
        mcp_servers: Arc<McpServerBindings>,
    ) -> Self {
        Self {
            conversation_id,
            workspace_root,
            revision,
            tool_registry,
            available_tools,
            skills,
            python_environment,
            node_environment,
            effective_environment,
            mcp_lease_authority,
            mcp_servers,
        }
    }

    /// The configured MCP server bindings of this immutable generation.
    #[must_use]
    pub fn mcp_servers(&self) -> &McpServerBindings {
        &self.mcp_servers
    }

    /// The conversation owner of this immutable capability snapshot.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        &self.conversation_id
    }

    /// The canonical Workspace owner of this immutable capability snapshot.
    #[must_use]
    pub fn workspace_root(&self) -> &Path {
        &self.workspace_root
    }

    /// The monotonic capability revision of this snapshot.
    #[must_use]
    pub fn revision(&self) -> CapabilityRevision {
        self.revision
    }

    /// The immutable `ToolRegistry` handle of the attempt's capability set.
    #[must_use]
    pub fn tool_registry(&self) -> &Arc<ToolRegistry> {
        &self.tool_registry
    }

    /// The immutable available Tool catalog, including inactive Tools.
    #[must_use]
    pub fn available_tools(&self) -> &AvailableToolCatalog {
        &self.available_tools
    }

    /// The immutable Skill snapshot/catalog.
    #[must_use]
    pub fn skills(&self) -> &Arc<SkillSnapshot> {
        &self.skills
    }

    /// The shared Python environment, when the merged Skill set declares
    /// Python dependencies.
    #[must_use]
    pub fn python_environment(&self) -> Option<&PythonEnvironment> {
        self.python_environment.as_ref()
    }

    /// The shared Node environment, when the merged Skill set declares
    /// Node dependencies.
    #[must_use]
    pub fn node_environment(&self) -> Option<&NodeEnvironment> {
        self.node_environment.as_ref()
    }

    /// The attempt's immutable effective `ToolEnvironment`: the base
    /// authorized environment plus the deterministic Skill environment
    /// overlay.
    #[must_use]
    pub fn effective_environment(&self) -> &ToolEnvironment {
        &self.effective_environment
    }

    /// Acquires the physical MCP leases paired with this immutable
    /// capability generation.
    pub(crate) fn acquire_mcp_leases(&self) -> Option<McpRuntimeLeaseSet> {
        self.mcp_lease_authority.acquire()
    }

    /// The exact rendered Skill capability guidance of this immutable
    /// capability snapshot. Context Assembly renders it into the request-time
    /// Effective System Prompt; it is not canonical conversation history.
    #[must_use]
    pub fn skill_catalog(&self) -> Option<String> {
        let entries = self.skills.catalog_entries();
        (!entries.is_empty()).then(|| crate::skills::render_skill_catalog(entries))
    }

    /// The deterministic `CapabilitiesManifest` data of this snapshot.
    #[must_use]
    pub fn to_capabilities_manifest(&self) -> CapabilitiesManifest {
        CapabilitiesManifest {
            revision: self.revision,
            // Manifest provenance is the complete immutable capability set.
            // Model visibility is projected separately through the Skill
            // snapshot's Skill-level visible catalog; `disable-model-
            // invocation` must not erase runtime ownership.
            skills: self.skills.bindings().to_vec(),
            tools: self
                .tool_registry
                .definitions()
                .iter()
                .map(|definition| crate::protocol::manifest::ToolBinding {
                    tool_id: definition.id.clone(),
                    name: definition.name.clone(),
                    origin: definition.origin.clone(),
                })
                .collect(),
            mcp: self
                .tool_registry
                .definitions()
                .into_iter()
                .filter_map(|definition| match definition.origin {
                    crate::tools::types::ToolOrigin::Mcp { server_id } => Some(server_id),
                    crate::tools::types::ToolOrigin::Builtin => None,
                })
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .map(|server_id| crate::protocol::manifest::McpBinding { server_id })
                .collect(),
        }
    }
}
