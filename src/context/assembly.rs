//! The single rustX-owned Context Assembly contract (Issue #55).
//!
//! Contributors only produce transient, bounded proposals against one finite
//! immutable invocation snapshot.  This module assigns semantic lanes,
//! stable contributor identities, trusted provenance, deterministic order,
//! and the exact system-section family.  It never owns canonical history,
//! Surface mutation, admission, cancellation, or provider dispatch.

use std::path::PathBuf;
use std::sync::Arc;

use futures_util::future::BoxFuture;
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
    /// The semantic lane of the **native runtime observation owner**
    /// ([`NativeContextContributor::ToolResultObservation`]).
    ///
    /// This lane describes *who owns the fact*, not *when the fact became
    /// eligible*. Deferred post-tool proposals produced by a certified
    /// extension keep that extension's own semantics and land in
    /// [`UserContextLane::ExtensionEnvironment`]; only the native runtime
    /// observation owner's facts belong here. The lane is native-reserved:
    /// no extension can claim it.
    ///
    /// The lane sits immediately after claimed inbound because a native
    /// runtime observation describes what the environment just did for the
    /// tool batch that precedes this step, while the request-time
    /// workspace/extension/Skill and Agent Status lanes describe the
    /// *current* step.
    PostToolObservation,
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
    pub const ALL: [Self; 6] = [
        Self::ClaimedInbound,
        Self::PostToolObservation,
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
            Self::PostToolObservation => "post_tool_observation",
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

/// One deferred transient proposal staged by the Agent Loop before this
/// step's assembly (Issue #56).
///
/// # Timing is not provenance
///
/// A deferred proposal is one whose *eligibility* was established earlier —
/// by the immutable observation of a structurally settled tool batch — rather
/// than during this step's contributor invocation. That is a **lifecycle
/// timing** fact owned by the Agent Loop.
///
/// It says nothing about **semantic ownership**. The `producer` below is the
/// trusted contributor identity the Agent Loop assigned from the observer's
/// *registration*, never from anything the observer returned. Context
/// Assembly then derives the lane, the trusted [`UserSource`], and the
/// [`ContextKind`] from that identity alone, using exactly the same table it
/// applies to the same owner's request-time proposals. A certified extension
/// therefore keeps its extension provenance and its own lane when it produces
/// deferred context; nothing is rewritten into native runtime context because
/// of when it was produced.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeferredContextProposal {
    /// The trusted semantic owner of the proposal, assigned by the Agent Loop
    /// from the producing observer's registration.
    pub producer: ContextContributorIdentity,
    /// The transient proposal exactly as the producer returned it.
    pub proposal: ContextProposal,
}

/// Whether a contribution's eligibility was established before this step or
/// during it.
///
/// The phase is only an ordering tiebreak *inside* one `(lane, contributor)`
/// bucket: a deferred fact describes the tool batch that precedes the step, so
/// it precedes the same owner's request-time fact. It never selects a lane, a
/// provenance, or a semantic family.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum ContributionPhase {
    /// Staged before this step by a settled tool-batch observation.
    Deferred,
    /// Produced by this step's own assembly invocation.
    RequestTime,
}

/// The canonical user-context semantics of one contributor identity.
///
/// This is the single table that maps *semantic ownership* to a lane, a
/// trusted provenance, and a semantic family. Request-time and deferred
/// proposals of the same owner resolve through it identically, which is what
/// makes post-tool timing incapable of rewriting provenance.
///
/// `None` means the owner publishes no model-visible User context at all (it
/// is an effective-system-prompt owner), so such a proposal is invalid.
fn user_semantics(
    identity: &ContextContributorIdentity,
) -> Option<(UserContextLane, UserSource, ContextKind)> {
    match identity {
        ContextContributorIdentity::Native(owner) => match owner {
            NativeContextContributor::WorkspaceInstructions => Some((
                UserContextLane::WorkspaceInstructions,
                UserSource::Runtime,
                ContextKind::WorkspaceInstructions,
            )),
            NativeContextContributor::SkillGuidance => Some((
                UserContextLane::SkillGuidance,
                UserSource::Runtime,
                ContextKind::SkillGuidance,
            )),
            NativeContextContributor::AgentStatus => Some((
                UserContextLane::AgentStatus,
                UserSource::Runtime,
                ContextKind::AgentStatus,
            )),
            NativeContextContributor::ToolResultObservation => Some((
                UserContextLane::PostToolObservation,
                UserSource::Runtime,
                ContextKind::PostToolObservation,
            )),
            NativeContextContributor::CoreSystemIdentity
            | NativeContextContributor::AgentProfile => None,
        },
        ContextContributorIdentity::CertifiedExtension(extension) => Some((
            UserContextLane::ExtensionEnvironment,
            UserSource::Extension {
                contributor: extension.clone(),
            },
            ContextKind::ExtensionEnvironment,
        )),
    }
}

/// [`user_semantics`] as a fallible lookup: an owner that publishes no
/// model-visible User context cannot propose one, whatever its timing.
fn user_lane_of(
    identity: &ContextContributorIdentity,
) -> Result<(UserContextLane, UserSource, ContextKind), ContextAssemblyError> {
    user_semantics(identity).ok_or_else(|| {
        ContextAssemblyError::InvalidProposal(format!(
            "contributor {identity:?} owns no model-visible User context lane"
        ))
    })
}

/// [`system_semantics`] as a fallible lookup.
fn system_lane_of(
    identity: &ContextContributorIdentity,
) -> Result<SystemSectionLane, ContextAssemblyError> {
    system_semantics(identity).ok_or_else(|| {
        ContextAssemblyError::InvalidProposal(format!(
            "contributor {identity:?} owns no effective-system-prompt section lane"
        ))
    })
}

/// The canonical system-section lane of one contributor identity.
///
/// `None` means the owner publishes no effective-system-prompt section.
fn system_semantics(identity: &ContextContributorIdentity) -> Option<SystemSectionLane> {
    match identity {
        ContextContributorIdentity::Native(owner) => match owner {
            NativeContextContributor::CoreSystemIdentity => {
                Some(SystemSectionLane::CoreRuntimeIdentity)
            }
            NativeContextContributor::AgentProfile => Some(SystemSectionLane::AgentProfile),
            NativeContextContributor::SkillGuidance => {
                Some(SystemSectionLane::NativeCapabilityGuidance)
            }
            NativeContextContributor::WorkspaceInstructions
            | NativeContextContributor::AgentStatus
            | NativeContextContributor::ToolResultObservation => None,
        },
        ContextContributorIdentity::CertifiedExtension(_) => {
            Some(SystemSectionLane::CertifiedExtension)
        }
    }
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
    fn contribute<'a>(
        &'a self,
        input: &'a ContributorInputSnapshot,
    ) -> BoxFuture<'a, Result<Vec<ContextProposal>, ContextAssemblyError>>;
}

impl<F> ContextContributor for F
where
    F: Fn(&ContributorInputSnapshot) -> Result<Vec<ContextProposal>, ContextAssemblyError>
        + Send
        + Sync,
{
    fn contribute<'a>(
        &'a self,
        input: &'a ContributorInputSnapshot,
    ) -> BoxFuture<'a, Result<Vec<ContextProposal>, ContextAssemblyError>> {
        Box::pin(async move { self(input) })
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

/// The bounded number of transient proposals one contributor may return for
/// one primary step.
pub const MAX_PROPOSALS_PER_CONTRIBUTOR: usize = 128;

/// The bounded number of deferred proposals one primary step may carry, over
/// all producers together.
///
/// The Agent Loop enforces this same bound at its observer transaction
/// boundary, so an unbounded observation pass is rejected before anything is
/// staged; assembly re-checks it because assembly never trusts its input.
pub const MAX_DEFERRED_CONTEXT_PROPOSALS: usize = 128;

const MAX_CONTEXT_TEXT_BYTES: usize = 1024 * 1024;

/// Validates one transient proposal against the bounded content contract.
///
/// This is the exact content validation [`ContextAssembly::assemble`] applies.
/// It is public so the Agent Loop can run it at its observer transaction
/// boundary and reject an oversized or empty deferred proposal *before* it is
/// staged, instead of discovering it one step later.
///
/// # Errors
///
/// Returns [`ContextAssemblyError::InvalidProposal`] when the proposal body is
/// empty or exceeds the bounded context size.
pub fn validate_context_proposal(proposal: &ContextProposal) -> Result<(), ContextAssemblyError> {
    match proposal {
        ContextProposal::UserMessage(message) => text_content(&message.content).map(|_| ()),
        ContextProposal::SystemPromptSection(section) => {
            validate_text(&section.content, "system section")
        }
    }
}

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

    /// Assembles deferred, native, and certified-extension proposals against
    /// one finite immutable input snapshot. The returned User messages have
    /// trusted source/kind but no `MessageId`; the Agent Loop allocates and
    /// commits ids at its one admission boundary.
    ///
    /// `deferred` carries the proposals whose eligibility the Agent Loop
    /// established earlier, at the observer transaction boundary of a settled
    /// tool batch. They are validated, laned, and given provenance here by the
    /// *same* rules as request-time proposals of the same semantic owner:
    /// assembly reads their producer identity and nothing else about their
    /// timing.
    ///
    /// # Errors
    ///
    /// Returns an error when native values, deferred proposals, or contributor
    /// proposals violate the bounded assembly contract.
    #[allow(clippy::too_many_lines)]
    pub async fn assemble(
        &self,
        input: &ContributorInputSnapshot,
        native: &NativeContextInput,
        deferred: &[DeferredContextProposal],
    ) -> Result<AcceptedContext, ContextAssemblyError> {
        let mut entries = Vec::new();
        let mut native_sections = Vec::new();
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

        // Deferred context. The Agent Loop already staged these in canonical
        // `(ToolCall batch position, producer identity, proposal FIFO)` order,
        // so the enumerated sequence preserves exactly that order inside each
        // producer's own lane and physical tool completion timing can never
        // reach canonical history. Provenance comes from `producer`, so a
        // certified extension's deferred fact stays an extension fact.
        if deferred.len() > MAX_DEFERRED_CONTEXT_PROPOSALS {
            return Err(ContextAssemblyError::ProposalLimitExceeded);
        }
        for (sequence, staged) in deferred.iter().enumerate() {
            match &staged.proposal {
                ContextProposal::UserMessage(message) => {
                    let (lane, source, kind) = user_lane_of(&staged.producer)?;
                    entries.push(ContributionEntry {
                        lane,
                        identity: staged.producer.clone(),
                        source,
                        kind,
                        content: text_content(&message.content)?,
                        phase: ContributionPhase::Deferred,
                        sequence,
                    });
                }
                ContextProposal::SystemPromptSection(section) => {
                    let lane = system_lane_of(&staged.producer)?;
                    validate_text(&section.content, "deferred system section")?;
                    native_sections.push(AcceptedSystemSection {
                        lane,
                        contributor: staged.producer.clone(),
                        content: section.content.clone(),
                    });
                }
            }
            generations.push(ContributorGeneration {
                identity: staged.producer.clone(),
                attestation: None,
            });
        }

        if let Some(text) = &native.workspace_instructions {
            entries.push(ContributionEntry::native_user(
                NativeContextContributor::WorkspaceInstructions,
                text.clone(),
            )?);
        }
        if let Some(text) = &native.skill_guidance {
            entries.push(ContributionEntry::native_user(
                NativeContextContributor::SkillGuidance,
                text.clone(),
            )?);
        }
        if let Some(text) = &native.agent_status {
            entries.push(ContributionEntry::native_user(
                NativeContextContributor::AgentStatus,
                text.clone(),
            )?);
        }

        if let Some(text) = &native.core_runtime_identity {
            validate_text(text, "core runtime identity")?;
            native_sections.push(native_section(
                NativeContextContributor::CoreSystemIdentity,
                text.clone(),
            ));
        }
        if let Some(text) = &native.agent_profile {
            validate_text(text, "agent profile")?;
            native_sections.push(native_section(
                NativeContextContributor::AgentProfile,
                text.clone(),
            ));
        }

        let mut extensions = self.extensions.clone();
        extensions.sort_by(|left, right| left.identity.cmp(&right.identity));
        for registered in extensions {
            let proposals = registered
                .contributor
                .contribute(input)
                .await
                .map_err(|error| ContextAssemblyError::ContributorFailed(error.to_string()))?;
            if proposals.len() > MAX_PROPOSALS_PER_CONTRIBUTOR {
                return Err(ContextAssemblyError::ProposalLimitExceeded);
            }
            for (sequence, proposal) in proposals.into_iter().enumerate() {
                match proposal {
                    ContextProposal::UserMessage(message) => {
                        let (lane, source, kind) = user_lane_of(&registered.generation.identity)?;
                        let text = text_content(&message.content)?;
                        entries.push(ContributionEntry {
                            lane,
                            identity: registered.generation.identity.clone(),
                            source,
                            kind,
                            content: text,
                            phase: ContributionPhase::RequestTime,
                            sequence,
                        });
                    }
                    ContextProposal::SystemPromptSection(section) => {
                        let lane = system_lane_of(&registered.generation.identity)?;
                        validate_text(&section.content, "extension system section")?;
                        native_sections.push(AcceptedSystemSection {
                            lane,
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
                .then_with(|| left.phase.cmp(&right.phase))
                .then_with(|| left.sequence.cmp(&right.sequence))
        });
        // `sort_by` is stable, so deferred sections keep their staged order
        // ahead of the same owner's request-time sections in one lane.
        native_sections.sort_by(|left, right| {
            left.lane
                .cmp(&right.lane)
                .then_with(|| left.contributor.cmp(&right.contributor))
        });
        // One logical owner appears exactly once in the accepted generation,
        // even when it contributed both deferred and request-time proposals.
        // The registered attestation wins over the deferred entry's absent
        // one, so the generation still explains the exact package.
        generations.sort_by(|left, right| {
            left.identity
                .cmp(&right.identity)
                .then_with(|| right.attestation.cmp(&left.attestation))
        });
        generations.dedup_by(|later, first| later.identity == first.identity);

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
    phase: ContributionPhase,
    sequence: usize,
}

impl ContributionEntry {
    /// One request-time native User context fact. Its lane, provenance, and
    /// semantic family come from the same owner table the deferred path uses.
    fn native_user(
        contributor: NativeContextContributor,
        text: String,
    ) -> Result<Self, ContextAssemblyError> {
        let identity = ContextContributorIdentity::Native(contributor);
        let (lane, source, kind) =
            user_semantics(&identity).expect("this native owner publishes User context");
        validate_text(&text, "native context")?;
        Ok(Self {
            lane,
            identity,
            source,
            kind,
            content: vec![UserContentBlock::Text(crate::message::content::TextBlock {
                text,
            })],
            phase: ContributionPhase::RequestTime,
            sequence: 0,
        })
    }
}

/// One request-time native effective-system-prompt section, laned through the
/// same owner table the deferred path uses.
fn native_section(contributor: NativeContextContributor, content: String) -> AcceptedSystemSection {
    let contributor = ContextContributorIdentity::Native(contributor);
    let lane =
        system_semantics(&contributor).expect("this native owner publishes a system section");
    AcceptedSystemSection {
        lane,
        contributor,
        content,
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

    #[tokio::test]
    async fn extension_order_uses_logical_identity_not_registration_order() {
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
            .assemble(&input(), &NativeContextInput::default(), &[])
            .await
            .expect("left");
        let b = right
            .assemble(&input(), &NativeContextInput::default(), &[])
            .await
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
        for key in [
            "Agent-Status",
            "skill-guidance",
            "core-runtime-identity",
            "Tool-Result-Observation",
        ] {
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

    #[tokio::test]
    async fn extension_provenance_and_identity_are_core_assigned() {
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
            .assemble(&input(), &NativeContextInput::default(), &[])
            .await
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

    #[tokio::test]
    async fn native_provenance_is_assigned_by_core() {
        let assembly = ContextAssembly::new();
        let accepted = assembly
            .assemble(
                &input(),
                &NativeContextInput {
                    agent_status: Some("same bytes".to_owned()),
                    ..NativeContextInput::default()
                },
                &[],
            )
            .await
            .expect("native proposal");
        assert_eq!(accepted.user_messages[0].source, UserSource::Runtime);
        assert_eq!(accepted.user_messages[0].kind, ContextKind::AgentStatus);
    }

    #[tokio::test]
    async fn system_sections_use_native_slots_and_stable_extension_order() {
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
                &[],
            )
            .await
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

    /// Helpers for the deferred-context suite.
    fn user(text: &str) -> ContextProposal {
        ContextProposal::UserMessage(UserMessageProposal {
            content: vec![UserContentBlock::Text(TextBlock {
                text: text.to_owned(),
            })],
        })
    }

    fn native_deferred(text: &str) -> DeferredContextProposal {
        DeferredContextProposal {
            producer: ContextContributorIdentity::Native(
                NativeContextContributor::ToolResultObservation,
            ),
            proposal: user(text),
        }
    }

    fn extension_deferred(key: &str, text: &str) -> DeferredContextProposal {
        DeferredContextProposal {
            producer: ContextContributorIdentity::CertifiedExtension(
                CertifiedExtensionIdentity::new(key).expect("identity"),
            ),
            proposal: user(text),
        }
    }

    fn texts(accepted: &AcceptedContext) -> Vec<String> {
        accepted
            .user_messages
            .iter()
            .map(|message| match &message.content[0] {
                UserContentBlock::Text(text) => text.text.clone(),
                _ => unreachable!("text proposals"),
            })
            .collect()
    }

    /// A deferred proposal owned by the **native** runtime observation owner
    /// receives native runtime provenance, the native-reserved lane, and the
    /// native semantic family — because of its producer identity, not because
    /// it arrived after a tool batch.
    #[tokio::test]
    async fn native_deferred_proposals_receive_native_provenance() {
        let assembly = ContextAssembly::new();
        let accepted = assembly
            .assemble(
                &input(),
                &NativeContextInput::default(),
                &[native_deferred("A1"), native_deferred("A2")],
            )
            .await
            .expect("assemble native deferred context");
        assert_eq!(
            accepted
                .user_messages
                .iter()
                .map(|message| (message.source.clone(), message.kind))
                .collect::<Vec<_>>(),
            vec![
                (UserSource::Runtime, ContextKind::PostToolObservation),
                (UserSource::Runtime, ContextKind::PostToolObservation),
            ]
        );
        assert_eq!(texts(&accepted), vec!["A1".to_owned(), "A2".to_owned()]);
        assert_eq!(
            accepted.generation.contributors,
            vec![ContributorGeneration {
                identity: ContextContributorIdentity::Native(
                    NativeContextContributor::ToolResultObservation,
                ),
                attestation: None,
            }],
            "the accepted generation explains the deferred-context owner exactly once"
        );
    }

    /// A deferred proposal owned by a **certified extension** keeps that
    /// extension's identity, its extension provenance, and its own semantic
    /// lane. Post-tool timing does not convert it into native runtime context.
    #[tokio::test]
    async fn extension_deferred_proposals_preserve_extension_provenance() {
        let assembly = ContextAssembly::new();
        let accepted = assembly
            .assemble(
                &input(),
                &NativeContextInput::default(),
                &[extension_deferred("example.extension", "deferred")],
            )
            .await
            .expect("assemble extension deferred context");
        assert_eq!(accepted.user_messages.len(), 1);
        assert_eq!(
            accepted.user_messages[0].source,
            UserSource::Extension {
                contributor: CertifiedExtensionIdentity::new("example.extension")
                    .expect("identity"),
            },
            "extension provenance survives deferred timing"
        );
        assert_eq!(
            accepted.user_messages[0].kind,
            ContextKind::ExtensionEnvironment,
            "the semantic family follows the owner, not the timing"
        );
        assert_eq!(
            accepted.generation.contributors[0].identity,
            ContextContributorIdentity::CertifiedExtension(
                CertifiedExtensionIdentity::new("example.extension").expect("identity"),
            ),
            "contributor identity is not rewritten to the native owner"
        );
        assert!(
            !accepted
                .user_messages
                .iter()
                .any(|message| message.kind == ContextKind::PostToolObservation
                    || message.source == UserSource::Runtime),
            "no part of the extension fact was converted into native runtime context"
        );
    }

    /// The same extension's deferred and request-time proposals are the same
    /// semantic fact family with the same provenance; only their order inside
    /// the owner's lane records that one describes the preceding tool batch.
    #[tokio::test]
    async fn deferred_and_request_time_context_of_one_owner_agree_on_semantics() {
        let mut assembly = ContextAssembly::new();
        assembly
            .register_extension(
                "example.extension",
                Some("package-1".to_owned()),
                Arc::new(|_: &ContributorInputSnapshot| Ok(vec![user("request-time")])),
            )
            .expect("register extension");
        let accepted = assembly
            .assemble(
                &input(),
                &NativeContextInput::default(),
                &[extension_deferred("example.extension", "deferred")],
            )
            .await
            .expect("assemble mixed-phase extension context");
        assert_eq!(
            texts(&accepted),
            vec!["deferred".to_owned(), "request-time".to_owned()],
            "the deferred fact describes the preceding tool batch and precedes it"
        );
        assert_eq!(
            accepted.user_messages[0].source, accepted.user_messages[1].source,
            "one owner has one provenance regardless of timing"
        );
        assert_eq!(
            accepted.user_messages[0].kind,
            accepted.user_messages[1].kind
        );
        assert_eq!(
            accepted.generation.contributors,
            vec![ContributorGeneration {
                identity: ContextContributorIdentity::CertifiedExtension(
                    CertifiedExtensionIdentity::new("example.extension").expect("identity"),
                ),
                attestation: Some("package-1".to_owned()),
            }],
            "one owner appears once, with its registered attestation"
        );
    }

    /// Deferred producers with different identities are ordered by lane and
    /// logical identity, exactly like request-time contributors. The staging
    /// order of the buffer decides nothing about relative placement between
    /// owners, so no registration order is observable.
    #[tokio::test]
    async fn deferred_producers_keep_deterministic_identity_order() {
        let assembly = ContextAssembly::new();
        let staged = [
            extension_deferred("zeta.extension", "z1"),
            native_deferred("n1"),
            extension_deferred("alpha.extension", "a1"),
            extension_deferred("zeta.extension", "z2"),
            native_deferred("n2"),
            extension_deferred("alpha.extension", "a2"),
        ];
        let mut reversed = staged.clone();
        reversed.reverse();

        let forward = assembly
            .assemble(&input(), &NativeContextInput::default(), &staged)
            .await
            .expect("forward staging");
        assert_eq!(
            forward
                .user_messages
                .iter()
                .map(|message| (message.source.clone(), message.kind))
                .collect::<Vec<_>>(),
            vec![
                (UserSource::Runtime, ContextKind::PostToolObservation),
                (UserSource::Runtime, ContextKind::PostToolObservation),
                (
                    UserSource::Extension {
                        contributor: CertifiedExtensionIdentity::new("alpha.extension")
                            .expect("identity"),
                    },
                    ContextKind::ExtensionEnvironment
                ),
                (
                    UserSource::Extension {
                        contributor: CertifiedExtensionIdentity::new("alpha.extension")
                            .expect("identity"),
                    },
                    ContextKind::ExtensionEnvironment
                ),
                (
                    UserSource::Extension {
                        contributor: CertifiedExtensionIdentity::new("zeta.extension")
                            .expect("identity"),
                    },
                    ContextKind::ExtensionEnvironment
                ),
                (
                    UserSource::Extension {
                        contributor: CertifiedExtensionIdentity::new("zeta.extension")
                            .expect("identity"),
                    },
                    ContextKind::ExtensionEnvironment
                ),
            ],
            "owners are grouped by lane then logical identity"
        );
        assert_eq!(
            texts(&forward),
            vec![
                "n1".to_owned(),
                "n2".to_owned(),
                "a1".to_owned(),
                "a2".to_owned(),
                "z1".to_owned(),
                "z2".to_owned()
            ],
            "each owner keeps its own FIFO order"
        );

        let backward = assembly
            .assemble(&input(), &NativeContextInput::default(), &reversed)
            .await
            .expect("reversed staging");
        assert_eq!(
            backward
                .user_messages
                .iter()
                .map(|message| (message.source.clone(), message.kind))
                .collect::<Vec<_>>(),
            forward
                .user_messages
                .iter()
                .map(|message| (message.source.clone(), message.kind))
                .collect::<Vec<_>>(),
            "the owner order never depends on staging/registration order"
        );
        assert_eq!(
            texts(&backward),
            vec![
                "n2".to_owned(),
                "n1".to_owned(),
                "a2".to_owned(),
                "a1".to_owned(),
                "z2".to_owned(),
                "z1".to_owned()
            ],
            "only each owner's own FIFO order follows the buffer"
        );
    }

    /// Deferred proposals sit in their owner's lane inside the one total lane
    /// order, together with every request-time proposal of that lane.
    #[tokio::test]
    async fn deferred_context_uses_the_one_total_lane_order() {
        let mut assembly = ContextAssembly::new();
        assembly
            .register_extension(
                "example.extension",
                None,
                Arc::new(|_: &ContributorInputSnapshot| Ok(vec![user("extension context")])),
            )
            .expect("register extension");
        let accepted = assembly
            .assemble(
                &input(),
                &NativeContextInput {
                    workspace_instructions: Some("workspace".to_owned()),
                    agent_status: Some("status".to_owned()),
                    ..NativeContextInput::default()
                },
                &[
                    native_deferred("A1"),
                    native_deferred("A2"),
                    extension_deferred("example.extension", "deferred extension"),
                ],
            )
            .await
            .expect("assemble deferred context");
        assert_eq!(
            accepted
                .user_messages
                .iter()
                .map(|message| (message.kind, message.source.clone()))
                .collect::<Vec<_>>(),
            vec![
                (ContextKind::PostToolObservation, UserSource::Runtime),
                (ContextKind::PostToolObservation, UserSource::Runtime),
                (ContextKind::WorkspaceInstructions, UserSource::Runtime),
                (
                    ContextKind::ExtensionEnvironment,
                    UserSource::Extension {
                        contributor: CertifiedExtensionIdentity::new("example.extension")
                            .expect("identity"),
                    }
                ),
                (
                    ContextKind::ExtensionEnvironment,
                    UserSource::Extension {
                        contributor: CertifiedExtensionIdentity::new("example.extension")
                            .expect("identity"),
                    }
                ),
                (ContextKind::AgentStatus, UserSource::Runtime),
            ],
            "the native observation lane is early; an extension's deferred fact stays in its own lane"
        );
        assert_eq!(
            texts(&accepted),
            vec![
                "A1".to_owned(),
                "A2".to_owned(),
                "workspace".to_owned(),
                "deferred extension".to_owned(),
                "extension context".to_owned(),
                "status".to_owned(),
            ]
        );
    }

    /// A deferred system-prompt section is laned by its owner too, and an
    /// owner with no lane for the proposed kind is rejected outright.
    #[tokio::test]
    async fn deferred_proposal_kinds_are_laned_by_owner() {
        let assembly = ContextAssembly::new();
        let accepted = assembly
            .assemble(
                &input(),
                &NativeContextInput::default(),
                &[DeferredContextProposal {
                    producer: ContextContributorIdentity::CertifiedExtension(
                        CertifiedExtensionIdentity::new("example.extension").expect("identity"),
                    ),
                    proposal: ContextProposal::SystemPromptSection(SystemPromptSectionProposal {
                        content: "deferred section".to_owned(),
                    }),
                }],
            )
            .await
            .expect("assemble deferred section");
        assert_eq!(
            accepted.system_sections,
            vec![AcceptedSystemSection {
                lane: SystemSectionLane::CertifiedExtension,
                contributor: ContextContributorIdentity::CertifiedExtension(
                    CertifiedExtensionIdentity::new("example.extension").expect("identity"),
                ),
                content: "deferred section".to_owned(),
            }]
        );

        let error = assembly
            .assemble(
                &input(),
                &NativeContextInput::default(),
                &[DeferredContextProposal {
                    producer: ContextContributorIdentity::Native(
                        NativeContextContributor::CoreSystemIdentity,
                    ),
                    proposal: user("core identity is not a User fact"),
                }],
            )
            .await
            .expect_err("an owner with no User lane cannot publish a User fact");
        assert!(matches!(error, ContextAssemblyError::InvalidProposal(_)));
    }

    /// A deferred batch is bounded exactly like a contributor batch.
    #[tokio::test]
    async fn deferred_context_is_bounded() {
        let assembly = ContextAssembly::new();
        let error = assembly
            .assemble(
                &input(),
                &NativeContextInput::default(),
                &(0..=MAX_DEFERRED_CONTEXT_PROPOSALS)
                    .map(|index| native_deferred(&format!("proposal {index}")))
                    .collect::<Vec<_>>(),
            )
            .await
            .expect_err("an unbounded deferred batch is rejected");
        assert_eq!(error, ContextAssemblyError::ProposalLimitExceeded);
    }

    /// Deferred content is validated by the same bounded content contract.
    #[tokio::test]
    async fn deferred_content_is_validated() {
        let assembly = ContextAssembly::new();
        let error = assembly
            .assemble(
                &input(),
                &NativeContextInput::default(),
                &[native_deferred("")],
            )
            .await
            .expect_err("an empty deferred proposal is rejected");
        assert!(matches!(error, ContextAssemblyError::InvalidProposal(_)));
        assert!(validate_context_proposal(&user("")).is_err());
        assert!(validate_context_proposal(&user("bounded")).is_ok());
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
