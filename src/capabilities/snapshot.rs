//! The immutable active capability snapshot (M6).

use std::sync::Arc;

use crate::model::types::SkillCatalogAttachment;
use crate::protocol::manifest::CapabilitiesManifest;
use crate::runtime::identity::CapabilityRevision;
use crate::skills::environments::{NodeEnvironment, PythonEnvironment};
use crate::skills::{SkillSnapshot, SkillCatalogEntry};
use crate::tools::environment::ToolEnvironment;
use crate::tools::executor::ToolRegistry;

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
/// `ToolRegistry` handle is the same immutable registry of the attempt's
/// capability set (see `tools::executor::ToolRegistry`); M6 candidates
/// reuse the same handle, which is the M7 seam without implementing M7.
#[derive(Clone)]
pub struct CapabilitySnapshot {
    revision: CapabilityRevision,
    tool_registry: Arc<ToolRegistry>,
    skills: Arc<SkillSnapshot>,
    python_environment: Option<PythonEnvironment>,
    node_environment: Option<NodeEnvironment>,
    effective_environment: ToolEnvironment,
    skill_catalog: Option<SkillCatalogAttachment>,
}

/// Two snapshots are equal when their capability content is equal: the
/// revision, the Skill snapshot, the environment identities, and the
/// effective environment. The shared immutable ToolRegistry handle is not
/// part of the equality (it is the same handle across candidates).
impl PartialEq for CapabilitySnapshot {
    fn eq(&self, other: &Self) -> bool {
        self.revision == other.revision
            && self.skills == other.skills
            && self.python_environment == other.python_environment
            && self.node_environment == other.node_environment
            && self.effective_environment == other.effective_environment
    }
}

impl core::fmt::Debug for CapabilitySnapshot {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("CapabilitySnapshot")
            .field("revision", &self.revision)
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
    pub(crate) fn new(
        revision: CapabilityRevision,
        tool_registry: Arc<ToolRegistry>,
        skills: Arc<SkillSnapshot>,
        python_environment: Option<PythonEnvironment>,
        node_environment: Option<NodeEnvironment>,
        effective_environment: ToolEnvironment,
    ) -> Self {
        let skill_catalog = skills.render_catalog().map(|rendered| SkillCatalogAttachment {
            rendered,
        });
        Self {
            revision,
            tool_registry,
            skills,
            python_environment,
            node_environment,
            effective_environment,
            skill_catalog,
        }
    }

    /// The monotonic capability revision of this snapshot.
    #[must_use]
    pub fn revision(&self) -> CapabilityRevision {
        self.revision
    }

    /// The immutable ToolRegistry handle of the attempt's capability set.
    #[must_use]
    pub fn tool_registry(&self) -> &Arc<ToolRegistry> {
        &self.tool_registry
    }

    /// The immutable Skill snapshot/catalog.
    #[must_use]
    pub fn skills(&self) -> &Arc<SkillSnapshot> {
        &self.skills
    }

    /// The model-visible Skill catalog entries.
    #[must_use]
    pub fn catalog_entries(&self) -> &[SkillCatalogEntry] {
        self.skills.catalog_entries()
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

    /// The attempt's immutable effective ToolEnvironment: the base
    /// authorized environment plus the deterministic Skill environment
    /// overlay.
    #[must_use]
    pub fn effective_environment(&self) -> &ToolEnvironment {
        &self.effective_environment
    }

    /// The Layer-0 Skill catalog attachment, when any Skill is active.
    ///
    /// The attachment is projection-only: it is never canonical history,
    /// never checkpoint history, never returned in
    /// `AgentExecutionResult.messages`, and never emitted as a
    /// committed-message event.
    #[must_use]
    pub fn skill_catalog_attachment(&self) -> Option<&SkillCatalogAttachment> {
        self.skill_catalog.as_ref()
    }

    /// The deterministic `CapabilitiesManifest` data of this snapshot.
    #[must_use]
    pub fn to_capabilities_manifest(&self) -> CapabilitiesManifest {
        CapabilitiesManifest {
            revision: self.revision,
            skills: self.skills.bindings().to_vec(),
            tools: self
                .tool_registry
                .definitions()
                .iter()
                .map(|definition| crate::protocol::manifest::ToolBinding {
                    tool_id: definition.id.clone(),
                    name: definition.name.clone(),
                })
                .collect(),
            mcp: Vec::new(),
        }
    }
}
