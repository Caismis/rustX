//! The single rustX-owned Context Assembly contract (Issue #55).
//!
//! Contributors only produce transient, bounded proposals against one finite
//! immutable invocation snapshot.  This module assigns semantic lanes,
//! stable contributor identities, trusted provenance, deterministic order,
//! and the exact system-section family.  It never owns canonical history,
//! Surface mutation, admission, cancellation, or provider dispatch.

use std::path::PathBuf;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::conversation::SurfaceRevision;
use crate::message::types::{ContextKind, MessageBlock, UserContentBlock, UserSource};
use crate::runtime::identity::{
    AttemptId, CapabilityRevision, CertifiedExtensionIdentity, ContextContributorIdentity,
    ConversationId, NativeContextContributor,
};

/// The ABI version of the native context contribution contract.
pub const CONTEXT_COMPATIBILITY_ABI_VERSION: u32 = 1;

/// The finite user-context semantic lanes owned by rustX.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserContextLane {
    /// Claimed inbound input. This is not extension-controlled.
    ClaimedInbound,
    /// Workspace/project instructions, with one semantic owner.
    WorkspaceInstructions,
    /// Generic certified-extension/environment context.
    ExtensionEnvironment,
    /// Native capability/Skill guidance.
    SkillGuidance,
    /// Native runtime/Agent Status context.
    AgentStatus,
}

impl UserContextLane {
    /// The contract's deterministic total order.
    pub const ALL: [Self; 5] = [
        Self::ClaimedInbound,
        Self::WorkspaceInstructions,
        Self::ExtensionEnvironment,
        Self::SkillGuidance,
        Self::AgentStatus,
    ];

    /// Stable manifest spelling of one user-context lane.
    #[must_use]
    pub const fn manifest_name(self) -> &'static str {
        match self {
            Self::ClaimedInbound => "claimed_inbound",
            Self::WorkspaceInstructions => "workspace_instructions",
            Self::ExtensionEnvironment => "extension_environment",
            Self::SkillGuidance => "skill_guidance",
            Self::AgentStatus => "agent_status",
        }
    }
}

/// The finite request-time system-section lanes owned by rustX.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SystemSectionLane {
    /// Core runtime/system identity. Single-owner and native-reserved.
    CoreRuntimeIdentity,
    /// Agent profile/persona. Single-owner and native-reserved.
    AgentProfile,
    /// Certified-extension sections, ordered by logical contributor identity.
    CertifiedExtension,
    /// Native capability guidance sections, when a future native owner needs
    /// a system section rather than a conversational User context fact.
    NativeCapabilityGuidance,
}

impl SystemSectionLane {
    /// The contract's deterministic total order.
    pub const ALL: [Self; 4] = [
        Self::CoreRuntimeIdentity,
        Self::AgentProfile,
        Self::CertifiedExtension,
        Self::NativeCapabilityGuidance,
    ];

    /// Stable manifest spelling of one system-section lane.
    #[must_use]
    pub const fn manifest_name(self) -> &'static str {
        match self {
            Self::CoreRuntimeIdentity => "core_runtime_identity",
            Self::AgentProfile => "agent_profile",
            Self::CertifiedExtension => "certified_extension",
            Self::NativeCapabilityGuidance => "native_capability_guidance",
        }
    }
}

/// The kind of transient proposal a contributor may return.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextProposalKind {
    /// A bounded model-visible canonical User context fact.
    UserMessage,
    /// A bounded request-time effective-system-prompt section.
    SystemPromptSection,
}

impl ContextProposalKind {
    /// Every proposal kind accepted by the core assembly contract.
    pub const ALL: [Self; 2] = [Self::UserMessage, Self::SystemPromptSection];
}

/// The finite immutable input visible to one contributor invocation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ContributorInputSnapshot {
    /// The admitted attempt identity.
    pub attempt_id: AttemptId,
    /// The owning conversation identity.
    pub conversation_id: ConversationId,
    /// The primary model turn number.
    pub turn: u32,
    /// The exact current Surface revision/reference.
    pub surface_revision: SurfaceRevision,
    /// The finite active Surface identities at invocation time.
    pub surface_ids: Vec<crate::runtime::identity::MessageId>,
    /// The claimed inbound batch, already committed by the host and copied
    /// into this immutable invocation value.
    pub claimed_inbound: Vec<MessageBlock>,
    /// The canonical workspace identity where a contributor needs it.
    pub workspace_root: PathBuf,
    /// The immutable capability generation observed by this attempt.
    pub capability_revision: CapabilityRevision,
}

/// Native values already sampled by rustX before assembly.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NativeContextInput {
    /// Workspace/project instructions, when the native workspace owner has
    /// one for this request.
    pub workspace_instructions: Option<String>,
    /// Native Skill/capability guidance rendered from the attempt's immutable
    /// capability snapshot.
    pub skill_guidance: Option<String>,
    /// The canonical rendered Agent Status snapshot.
    pub agent_status: Option<String>,
    /// Core runtime/system identity content for the effective system prompt.
    pub core_runtime_identity: Option<String>,
    /// Agent profile/persona content for the effective system prompt.
    pub agent_profile: Option<String>,
}

/// A transient User context proposal. It contains no id, source, kind, lane,
/// or mutable runtime handle; rustX supplies all of those at admission.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMessageProposal {
    /// Complete bounded User content.
    pub content: Vec<UserContentBlock>,
}

/// A transient request-time system-section proposal. Its family, identity,
/// and ordering are supplied by the registered contributor, not by this
/// value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SystemPromptSectionProposal {
    /// The section text before rustX's final deterministic rendering.
    pub content: String,
}

/// One transient proposal returned by a contributor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ContextProposal {
    /// A normal model-visible canonical User context fact.
    UserMessage(UserMessageProposal),
    /// A request-time effective-system-prompt section.
    SystemPromptSection(SystemPromptSectionProposal),
}

impl ContextProposal {
    /// The proposal kind.
    #[must_use]
    pub const fn kind(&self) -> ContextProposalKind {
        match self {
            Self::UserMessage(_) => ContextProposalKind::UserMessage,
            Self::SystemPromptSection(_) => ContextProposalKind::SystemPromptSection,
        }
    }
}

/// The contributor API. Implementations receive only a finite immutable
/// snapshot and return transient typed proposals.
pub trait ContextContributor: Send + Sync {
    /// Produce bounded proposals for one primary model step.
    ///
    /// # Errors
    ///
    /// Returns a context assembly error when the contributor cannot produce
    /// a bounded proposal batch.
    fn contribute(
        &self,
        input: &ContributorInputSnapshot,
    ) -> Result<Vec<ContextProposal>, ContextAssemblyError>;
}

impl<F> ContextContributor for F
where
    F: Fn(&ContributorInputSnapshot) -> Result<Vec<ContextProposal>, ContextAssemblyError>
        + Send
        + Sync,
{
    fn contribute(
        &self,
        input: &ContributorInputSnapshot,
    ) -> Result<Vec<ContextProposal>, ContextAssemblyError> {
        self(input)
    }
}

/// The independent attestation/content generation recorded beside a logical
/// contributor identity.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ContributorGeneration {
    /// Stable logical identity used for deterministic ordering.
    pub identity: ContextContributorIdentity,
    /// Optional attestation/package/content generation. It does not affect
    /// ordering.
    pub attestation: Option<String>,
}

/// One frozen context assembly generation accepted for a primary step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextGeneration {
    /// Monotonic per-attempt generation assigned by the Agent Loop.
    pub id: u64,
    /// The active contributor identities and their attestation generations.
    pub contributors: Vec<ContributorGeneration>,
}

impl ContextGeneration {
    /// Sets the Agent Loop-owned generation number.
    #[must_use]
    pub const fn with_id(mut self, id: u64) -> Self {
        self.id = id;
        self
    }
}

/// A canonical User message draft after assembly validation. It still has no
/// `MessageId`; the conversation admission owner allocates one exactly once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedUserContext {
    /// Trusted provenance assigned by rustX.
    pub source: UserSource,
    /// Trusted semantic family assigned by rustX.
    pub kind: ContextKind,
    /// Complete User content.
    pub content: Vec<UserContentBlock>,
}

/// One accepted system section after lane/identity assignment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AcceptedSystemSection {
    /// The semantic system lane.
    pub lane: SystemSectionLane,
    /// The contributor that owns the section.
    pub contributor: ContextContributorIdentity,
    /// The exact section content.
    pub content: String,
}

/// The validated result of one assembly invocation. This value is transient
/// until the Agent Loop crosses its admission boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcceptedContext {
    /// Canonical User context facts awaiting core `MessageId` allocation.
    pub user_messages: Vec<AcceptedUserContext>,
    /// Request-time system sections awaiting effective-prompt rendering.
    pub system_sections: Vec<AcceptedSystemSection>,
    /// The identity/generation explanation of the accepted assembly.
    pub generation: ContextGeneration,
}

/// A machine-readable projection of the real native contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ContextCompatibilityManifest {
    /// Compatibility ABI version.
    pub abi_version: u32,
    /// All user context lanes in their canonical order.
    pub user_context_lanes: Vec<UserContextLane>,
    /// All system-section lanes in their canonical order.
    pub system_section_lanes: Vec<SystemSectionLane>,
    /// Native-reserved single-owner slots.
    pub reserved_native_slots: Vec<String>,
    /// Multi-extension lanes.
    pub multi_extension_slots: Vec<String>,
    /// Trusted provenance namespaces.
    pub provenance_namespaces: Vec<String>,
    /// Proposal kinds accepted by the core contract.
    pub allowed_proposal_kinds: Vec<ContextProposalKind>,
}

impl ContextCompatibilityManifest {
    /// Derives the manifest directly from the finite lane/provenance
    /// contract constants used by assembly validation.
    #[must_use]
    pub fn native() -> Self {
        Self {
            abi_version: CONTEXT_COMPATIBILITY_ABI_VERSION,
            user_context_lanes: UserContextLane::ALL.to_vec(),
            system_section_lanes: SystemSectionLane::ALL.to_vec(),
            reserved_native_slots: NativeContextContributor::ALL
                .into_iter()
                .map(|owner| owner.manifest_name().to_owned())
                .collect(),
            multi_extension_slots: vec![
                UserContextLane::ExtensionEnvironment
                    .manifest_name()
                    .to_owned(),
                SystemSectionLane::CertifiedExtension
                    .manifest_name()
                    .to_owned(),
            ],
            provenance_namespaces: crate::message::types::UserSource::PROVENANCE_NAMESPACES
                .into_iter()
                .map(str::to_owned)
                .collect(),
            allowed_proposal_kinds: ContextProposalKind::ALL.to_vec(),
        }
    }
}

/// Assembly failures are explicit and transactional: no proposal is
/// partially accepted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ContextAssemblyError {
    /// A logical extension identity failed canonicalization.
    InvalidContributorIdentity(String),
    /// An extension attempted to use a native-reserved logical key.
    ReservedNativeIdentity(String),
    /// A second single-owner contributor was registered.
    DuplicateSingleOwner(String),
    /// A contributor returned too many proposals.
    ProposalLimitExceeded,
    /// A proposal body was empty or too large.
    InvalidProposal(String),
    /// A contributor failed while producing proposals.
    ContributorFailed(String),
}

impl core::fmt::Display for ContextAssemblyError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::InvalidContributorIdentity(detail) => {
                write!(f, "invalid contributor identity: {detail}")
            }
            Self::ReservedNativeIdentity(identity) => {
                write!(f, "extension identity {identity:?} is native-reserved")
            }
            Self::DuplicateSingleOwner(slot) => {
                write!(f, "semantic slot {slot:?} already has an owner")
            }
            Self::ProposalLimitExceeded => {
                f.write_str("context contributor proposal limit exceeded")
            }
            Self::InvalidProposal(detail) => write!(f, "invalid context proposal: {detail}"),
            Self::ContributorFailed(detail) => write!(f, "context contributor failed: {detail}"),
        }
    }
}

impl std::error::Error for ContextAssemblyError {}

const MAX_PROPOSALS_PER_CONTRIBUTOR: usize = 128;
const MAX_CONTEXT_TEXT_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
struct RegisteredExtension {
    identity: CertifiedExtensionIdentity,
    generation: ContributorGeneration,
    contributor: Arc<dyn ContextContributor>,
}

/// The rustX-owned Context Assembly registry and validator.
#[derive(Clone, Default)]
pub struct ContextAssembly {
    extensions: Vec<RegisteredExtension>,
}

impl core::fmt::Debug for ContextAssembly {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("ContextAssembly")
            .field(
                "extensions",
                &self
                    .extensions
                    .iter()
                    .map(|extension| extension.identity.as_str())
                    .collect::<Vec<_>>(),
            )
            .finish()
    }
}

impl ContextAssembly {
    /// Creates an empty assembly with only native core owners available.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            extensions: Vec::new(),
        }
    }

    /// Registers one certified extension. The logical key is canonicalized
    /// and is the sole ordering identity; attestation is recorded separately.
    ///
    /// # Errors
    ///
    /// Returns an error when the logical identity is invalid, native-reserved,
    /// or already registered.
    pub fn register_extension(
        &mut self,
        logical_key: impl Into<String>,
        attestation: Option<String>,
        contributor: Arc<dyn ContextContributor>,
    ) -> Result<CertifiedExtensionIdentity, ContextAssemblyError> {
        let identity = CertifiedExtensionIdentity::new(logical_key)
            .map_err(ContextAssemblyError::InvalidContributorIdentity)?;
        if is_reserved_extension_key(identity.as_str()) {
            return Err(ContextAssemblyError::ReservedNativeIdentity(
                identity.as_str().to_owned(),
            ));
        }
        if self
            .extensions
            .iter()
            .any(|registered| registered.identity == identity)
        {
            return Err(ContextAssemblyError::DuplicateSingleOwner(format!(
                "extension:{}",
                identity.as_str()
            )));
        }
        let generation = ContributorGeneration {
            identity: ContextContributorIdentity::CertifiedExtension(identity.clone()),
            attestation,
        };
        self.extensions.push(RegisteredExtension {
            identity: identity.clone(),
            generation,
            contributor,
        });
        Ok(identity)
    }

    /// Returns the registered extension identities in canonical logical order.
    #[must_use]
    pub fn extension_identities(&self) -> Vec<CertifiedExtensionIdentity> {
        let mut identities = self
            .extensions
            .iter()
            .map(|extension| extension.identity.clone())
            .collect::<Vec<_>>();
        identities.sort();
        identities
    }

    /// The mechanically derived native compatibility manifest.
    #[must_use]
    pub fn compatibility_manifest() -> ContextCompatibilityManifest {
        ContextCompatibilityManifest::native()
    }

    /// Assembles native and certified-extension proposals against one finite
    /// immutable input snapshot. The returned User messages have trusted
    /// source/kind but no `MessageId`; the Agent Loop allocates and commits ids
    /// at its one admission boundary.
    ///
    /// # Errors
    ///
    /// Returns an error when native values or contributor proposals violate
    /// the bounded assembly contract.
    #[allow(clippy::too_many_lines)]
    pub fn assemble(
        &self,
        input: &ContributorInputSnapshot,
        native: &NativeContextInput,
    ) -> Result<AcceptedContext, ContextAssemblyError> {
        let mut entries = Vec::new();
        let mut generations = vec![
            native
                .workspace_instructions
                .as_ref()
                .map(|_| native_generation(NativeContextContributor::WorkspaceInstructions)),
            native
                .skill_guidance
                .as_ref()
                .map(|_| native_generation(NativeContextContributor::SkillGuidance)),
            native
                .agent_status
                .as_ref()
                .map(|_| native_generation(NativeContextContributor::AgentStatus)),
            native
                .core_runtime_identity
                .as_ref()
                .map(|_| native_generation(NativeContextContributor::CoreSystemIdentity)),
            native
                .agent_profile
                .as_ref()
                .map(|_| native_generation(NativeContextContributor::AgentProfile)),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();

        if let Some(text) = &native.workspace_instructions {
            entries.push(ContributionEntry::native_user(
                UserContextLane::WorkspaceInstructions,
                NativeContextContributor::WorkspaceInstructions,
                ContextKind::WorkspaceInstructions,
                text.clone(),
                0,
            )?);
        }
        if let Some(text) = &native.skill_guidance {
            entries.push(ContributionEntry::native_user(
                UserContextLane::SkillGuidance,
                NativeContextContributor::SkillGuidance,
                ContextKind::SkillGuidance,
                text.clone(),
                0,
            )?);
        }
        if let Some(text) = &native.agent_status {
            entries.push(ContributionEntry::native_user(
                UserContextLane::AgentStatus,
                NativeContextContributor::AgentStatus,
                ContextKind::AgentStatus,
                text.clone(),
                0,
            )?);
        }

        let mut native_sections = Vec::new();
        if let Some(text) = &native.core_runtime_identity {
            validate_text(text, "core runtime identity")?;
            native_sections.push(AcceptedSystemSection {
                lane: SystemSectionLane::CoreRuntimeIdentity,
                contributor: ContextContributorIdentity::Native(
                    NativeContextContributor::CoreSystemIdentity,
                ),
                content: text.clone(),
            });
        }
        if let Some(text) = &native.agent_profile {
            validate_text(text, "agent profile")?;
            native_sections.push(AcceptedSystemSection {
                lane: SystemSectionLane::AgentProfile,
                contributor: ContextContributorIdentity::Native(
                    NativeContextContributor::AgentProfile,
                ),
                content: text.clone(),
            });
        }

        let mut extensions = self.extensions.clone();
        extensions.sort_by(|left, right| left.identity.cmp(&right.identity));
        for registered in extensions {
            let proposals = registered
                .contributor
                .contribute(input)
                .map_err(|error| ContextAssemblyError::ContributorFailed(error.to_string()))?;
            if proposals.len() > MAX_PROPOSALS_PER_CONTRIBUTOR {
                return Err(ContextAssemblyError::ProposalLimitExceeded);
            }
            for (sequence, proposal) in proposals.into_iter().enumerate() {
                match proposal {
                    ContextProposal::UserMessage(message) => {
                        let text = text_content(&message.content)?;
                        entries.push(ContributionEntry {
                            lane: UserContextLane::ExtensionEnvironment,
                            identity: registered.generation.identity.clone(),
                            source: UserSource::Extension {
                                contributor: registered.identity.clone(),
                            },
                            kind: ContextKind::ExtensionEnvironment,
                            content: text,
                            sequence,
                        });
                    }
                    ContextProposal::SystemPromptSection(section) => {
                        validate_text(&section.content, "extension system section")?;
                        native_sections.push(AcceptedSystemSection {
                            lane: SystemSectionLane::CertifiedExtension,
                            contributor: registered.generation.identity.clone(),
                            content: section.content,
                        });
                    }
                }
            }
            generations.push(registered.generation);
        }

        entries.sort_by(|left, right| {
            left.lane
                .cmp(&right.lane)
                .then_with(|| left.identity.cmp(&right.identity))
                .then_with(|| left.sequence.cmp(&right.sequence))
        });
        native_sections.sort_by(|left, right| {
            left.lane
                .cmp(&right.lane)
                .then_with(|| left.contributor.cmp(&right.contributor))
        });
        generations.sort();

        Ok(AcceptedContext {
            user_messages: entries
                .into_iter()
                .map(|entry| AcceptedUserContext {
                    source: entry.source,
                    kind: entry.kind,
                    content: entry.content,
                })
                .collect(),
            system_sections: native_sections,
            generation: ContextGeneration {
                id: 0,
                contributors: generations,
            },
        })
    }
}

#[derive(Debug)]
struct ContributionEntry {
    lane: UserContextLane,
    identity: ContextContributorIdentity,
    source: UserSource,
    kind: ContextKind,
    content: Vec<UserContentBlock>,
    sequence: usize,
}

impl ContributionEntry {
    fn native_user(
        lane: UserContextLane,
        contributor: NativeContextContributor,
        kind: ContextKind,
        text: String,
        sequence: usize,
    ) -> Result<Self, ContextAssemblyError> {
        validate_text(&text, "native context")?;
        Ok(Self {
            lane,
            identity: ContextContributorIdentity::Native(contributor),
            source: UserSource::Runtime,
            kind,
            content: vec![UserContentBlock::Text(crate::message::content::TextBlock {
                text,
            })],
            sequence,
        })
    }
}

fn native_generation(contributor: NativeContextContributor) -> ContributorGeneration {
    ContributorGeneration {
        identity: ContextContributorIdentity::Native(contributor),
        attestation: None,
    }
}

fn is_reserved_extension_key(key: &str) -> bool {
    NativeContextContributor::ALL
        .into_iter()
        .any(|owner| owner.logical_key() == key)
}

fn validate_text(text: &str, label: &str) -> Result<(), ContextAssemblyError> {
    if text.trim().is_empty() {
        return Err(ContextAssemblyError::InvalidProposal(format!(
            "{label} must not be empty"
        )));
    }
    if text.len() > MAX_CONTEXT_TEXT_BYTES {
        return Err(ContextAssemblyError::InvalidProposal(format!(
            "{label} exceeds the bounded context size"
        )));
    }
    Ok(())
}

fn text_content(
    content: &[UserContentBlock],
) -> Result<Vec<UserContentBlock>, ContextAssemblyError> {
    if content.is_empty() {
        return Err(ContextAssemblyError::InvalidProposal(
            "a User context proposal must contain content".to_owned(),
        ));
    }
    let bytes = content
        .iter()
        .map(|block| match block {
            UserContentBlock::Text(text) => text.text.len(),
            UserContentBlock::Image(_) | UserContentBlock::File(_) => 0,
        })
        .sum::<usize>();
    if bytes == 0 || bytes > MAX_CONTEXT_TEXT_BYTES {
        return Err(ContextAssemblyError::InvalidProposal(
            "a User context proposal must contain bounded text content".to_owned(),
        ));
    }
    Ok(content.to_vec())
}

/// Renders the exact provider-neutral Effective System Prompt from active
/// canonical System messages plus already-admitted request-time sections.
#[must_use]
pub fn render_effective_system_prompt(
    messages: &[MessageBlock],
    sections: &[AcceptedSystemSection],
) -> String {
    let mut parts = Vec::new();
    for message in messages {
        if let MessageBlock::System(system) = message {
            parts.extend(system.content.iter().map(|text| text.text.clone()));
        }
    }
    parts.extend(sections.iter().map(|section| section.content.clone()));
    parts.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::content::TextBlock;
    use crate::runtime::identity::MessageId;

    fn input() -> ContributorInputSnapshot {
        ContributorInputSnapshot {
            attempt_id: AttemptId::new("attempt-1"),
            conversation_id: ConversationId::new("conversation-1"),
            turn: 1,
            surface_revision: SurfaceRevision::INITIAL,
            surface_ids: vec![MessageId::new("inbound")],
            claimed_inbound: Vec::new(),
            workspace_root: PathBuf::from("/workspace"),
            capability_revision: CapabilityRevision::new(3),
        }
    }

    #[test]
    fn extension_order_uses_logical_identity_not_registration_order() {
        let first = Arc::new(|_: &ContributorInputSnapshot| {
            Ok(vec![ContextProposal::UserMessage(UserMessageProposal {
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "z".to_owned(),
                })],
            })])
        });
        let second = Arc::new(|_: &ContributorInputSnapshot| {
            Ok(vec![ContextProposal::UserMessage(UserMessageProposal {
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "a".to_owned(),
                })],
            })])
        });
        let mut left = ContextAssembly::new();
        left.register_extension("zeta", Some("v1".to_owned()), first)
            .expect("zeta");
        left.register_extension("alpha", Some("v1".to_owned()), second)
            .expect("alpha");
        let mut right = ContextAssembly::new();
        right
            .register_extension(
                "alpha",
                Some("v2".to_owned()),
                Arc::new(|_: &ContributorInputSnapshot| {
                    Ok(vec![ContextProposal::UserMessage(UserMessageProposal {
                        content: vec![UserContentBlock::Text(TextBlock {
                            text: "a".to_owned(),
                        })],
                    })])
                }),
            )
            .expect("alpha");
        right
            .register_extension(
                "zeta",
                Some("v9".to_owned()),
                Arc::new(|_: &ContributorInputSnapshot| {
                    Ok(vec![ContextProposal::UserMessage(UserMessageProposal {
                        content: vec![UserContentBlock::Text(TextBlock {
                            text: "z".to_owned(),
                        })],
                    })])
                }),
            )
            .expect("zeta");
        let a = left
            .assemble(&input(), &NativeContextInput::default())
            .expect("left");
        let b = right
            .assemble(&input(), &NativeContextInput::default())
            .expect("right");
        assert_eq!(a.user_messages, b.user_messages);
        assert_eq!(
            a.generation.contributors[0].identity,
            b.generation.contributors[0].identity
        );
        assert_ne!(
            a.generation.contributors[0].attestation,
            b.generation.contributors[0].attestation
        );
    }

    #[test]
    fn extension_cannot_claim_native_identity() {
        let mut assembly = ContextAssembly::new();
        for key in ["Agent-Status", "skill-guidance", "core-runtime-identity"] {
            let error = assembly
                .register_extension(
                    key,
                    None,
                    Arc::new(|_: &ContributorInputSnapshot| Ok(Vec::new())),
                )
                .expect_err("reserved identity must reject");
            assert!(matches!(
                error,
                ContextAssemblyError::ReservedNativeIdentity(_)
            ));
        }
    }

    #[test]
    fn extension_provenance_and_identity_are_core_assigned() {
        let mut assembly = ContextAssembly::new();
        assembly
            .register_extension(
                "Example.Extension",
                Some("package-generation-1".to_owned()),
                Arc::new(|_: &ContributorInputSnapshot| {
                    Ok(vec![ContextProposal::UserMessage(UserMessageProposal {
                        content: vec![UserContentBlock::Text(TextBlock {
                            text: "extension context".to_owned(),
                        })],
                    })])
                }),
            )
            .expect("register extension");
        let accepted = assembly
            .assemble(&input(), &NativeContextInput::default())
            .expect("assemble extension");
        assert_eq!(accepted.user_messages.len(), 1);
        assert_eq!(
            accepted.user_messages[0].source,
            UserSource::Extension {
                contributor: CertifiedExtensionIdentity::new("example.extension")
                    .expect("identity")
            }
        );
        assert_eq!(
            accepted.user_messages[0].kind,
            ContextKind::ExtensionEnvironment
        );
        let identity = &accepted.generation.contributors[0].identity;
        let encoded = serde_json::to_string(identity).expect("serialize identity");
        let decoded: ContextContributorIdentity =
            serde_json::from_str(&encoded).expect("deserialize identity");
        assert_eq!(&decoded, identity);
    }

    #[test]
    fn native_provenance_is_assigned_by_core() {
        let assembly = ContextAssembly::new();
        let accepted = assembly
            .assemble(
                &input(),
                &NativeContextInput {
                    agent_status: Some("same bytes".to_owned()),
                    ..NativeContextInput::default()
                },
            )
            .expect("native proposal");
        assert_eq!(accepted.user_messages[0].source, UserSource::Runtime);
        assert_eq!(accepted.user_messages[0].kind, ContextKind::AgentStatus);
    }

    #[test]
    fn system_sections_use_native_slots_and_stable_extension_order() {
        let mut assembly = ContextAssembly::new();
        assembly
            .register_extension(
                "zeta.extension",
                Some("package-v2".to_owned()),
                Arc::new(|_: &ContributorInputSnapshot| {
                    Ok(vec![ContextProposal::SystemPromptSection(
                        SystemPromptSectionProposal {
                            content: "zeta section".to_owned(),
                        },
                    )])
                }),
            )
            .expect("zeta extension");
        assembly
            .register_extension(
                "alpha.extension",
                Some("package-v1".to_owned()),
                Arc::new(|_: &ContributorInputSnapshot| {
                    Ok(vec![ContextProposal::SystemPromptSection(
                        SystemPromptSectionProposal {
                            content: "alpha section".to_owned(),
                        },
                    )])
                }),
            )
            .expect("alpha extension");

        let accepted = assembly
            .assemble(
                &input(),
                &NativeContextInput {
                    core_runtime_identity: Some("core identity".to_owned()),
                    agent_profile: Some("agent profile".to_owned()),
                    ..NativeContextInput::default()
                },
            )
            .expect("system sections assemble");
        assert_eq!(
            accepted
                .system_sections
                .iter()
                .map(|section| section.content.as_str())
                .collect::<Vec<_>>(),
            vec![
                "core identity",
                "agent profile",
                "alpha section",
                "zeta section"
            ]
        );
        assert_eq!(
            accepted.system_sections[0].contributor,
            ContextContributorIdentity::Native(NativeContextContributor::CoreSystemIdentity)
        );
        assert_eq!(
            render_effective_system_prompt(&[], &accepted.system_sections),
            "core identity\n\nagent profile\n\nalpha section\n\nzeta section"
        );
    }

    #[test]
    fn manifest_is_derived_from_contract_constants() {
        let manifest = ContextAssembly::compatibility_manifest();
        assert_eq!(manifest.user_context_lanes, UserContextLane::ALL);
        assert_eq!(manifest.system_section_lanes, SystemSectionLane::ALL);
        assert_eq!(manifest.abi_version, CONTEXT_COMPATIBILITY_ABI_VERSION);
        assert_eq!(
            manifest.reserved_native_slots,
            NativeContextContributor::ALL
                .into_iter()
                .map(|owner| owner.manifest_name().to_owned())
                .collect::<Vec<_>>()
        );
        assert_eq!(manifest.allowed_proposal_kinds, ContextProposalKind::ALL);
        assert_eq!(
            manifest.provenance_namespaces,
            crate::message::types::UserSource::PROVENANCE_NAMESPACES
                .into_iter()
                .map(str::to_owned)
                .collect::<Vec<_>>()
        );
    }
}
