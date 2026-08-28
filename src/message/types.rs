//! The canonical conversation model.
//!
//! The canonical conversation contains exactly three top-level message roles:
//! [`MessageBlock::User`], [`MessageBlock::Assistant`], and
//! [`MessageBlock::Tool`]. These three semantics are frozen; provider
//! roles such as `OpenAI`'s `developer` are adapter concerns, not canonical
//! roles, and are never mapped to a fifth role.
//!
//! Role and provenance are separate: a `UserMessageBlock` means inbound
//! information supplied to the current agent, regardless of whether a human,
//! another agent, the fleet, an external system, or the runtime produced it.
//! Streaming deltas are `ModelEvent` facts and never become message blocks;
//! only completed generations are committed as `AssistantMessageBlock` values.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::message::content::{FileReference, ImageReference, TextBlock};
use crate::runtime::continuation::ProviderContinuationState;
use crate::runtime::identity::{
    AgentId, CertifiedExtensionIdentity, MessageId, ToolCallId, ToolId,
};
use crate::tools::types::{ToolCall, ToolExecutionResult};

/// The canonical conversation message.
///
/// The `role` discriminator is stable: `user`, `assistant`, `tool`.
/// No additional top-level role exists.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "snake_case")]
pub enum MessageBlock {
    /// Inbound information supplied to the current agent.
    User(UserMessageBlock),
    /// One completed model generation produced by the current agent.
    Assistant(AssistantMessageBlock),
    /// The result of one tool call produced by the current agent.
    Tool(ToolMessageBlock),
}

impl MessageBlock {
    /// The durable identity of this canonical message.
    ///
    /// Canonical messages are immutable Ledger facts keyed by this identity,
    /// so equal identities imply equal content: comparing identities is a
    /// sound and cheap way to decide whether two projections share a prefix.
    #[must_use]
    pub const fn id(&self) -> &MessageId {
        match self {
            Self::User(user) => &user.id,
            Self::Assistant(assistant) => &assistant.id,
            Self::Tool(tool) => &tool.id,
        }
    }

    /// Returns the structured generation descriptor when this is a canonical
    /// Agent Status message.
    #[must_use]
    pub fn agent_status_metadata(&self) -> Option<&AgentStatusGenerationMetadata> {
        match self {
            Self::User(user) => match &user.kind {
                InboundKind::Context(kind) => kind.agent_status_metadata(),
                InboundKind::Message | InboundKind::CompactionSummary(_) => None,
            },
            Self::Assistant(_) | Self::Tool(_) => None,
        }
    }

    /// Whether this canonical message belongs to the Agent Status context
    /// family.
    #[must_use]
    pub fn is_agent_status(&self) -> bool {
        matches!(
            self,
            Self::User(UserMessageBlock {
                kind: InboundKind::Context(ContextKind::AgentStatus(_)),
                ..
            })
        )
    }
}

/// A stable index identifying one content block within the ordered content
/// list of the canonical message being assembled.
///
/// Streaming facts (text deltas, reasoning deltas, provider continuation
/// state, tool-call content) reference the block they belong to by this
/// index, so interleaved and multiple blocks remain unambiguous without
/// exposing any provider-specific block id type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ContentBlockIndex(u32);

impl ContentBlockIndex {
    /// Creates an index from a raw value.
    #[must_use]
    pub const fn new(index: u32) -> Self {
        Self(index)
    }

    /// Returns the raw index value.
    #[must_use]
    pub const fn get(self) -> u32 {
        self.0
    }
}

impl core::fmt::Display for ContentBlockIndex {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Inbound information supplied to the current agent.
///
/// A `UserMessageBlock` does not necessarily mean a human spoke: it is the
/// canonical home for anything inbound, including messages from other agents
/// (with [`UserSource::Agent`] provenance) and runtime compaction summaries
/// (with [`InboundKind::CompactionSummary`] kind). It
/// must never become `AssistantMessageBlock` or `ToolMessageBlock`, which are
/// reserved for output and actions of the current agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserMessageBlock {
    /// Durable message identity.
    pub id: MessageId,
    /// The inbound content.
    pub content: Vec<UserContentBlock>,
    /// Provenance: who supplied the inbound information.
    pub source: UserSource,
    /// Typed kind of inbound information.
    #[serde(default)]
    pub kind: InboundKind,
    /// The persisted UTC instant associated with the inbound message, when
    /// the producer supplied one.
    ///
    /// An ordinary asynchronously delivered inbound message
    /// ([`InboundKind::Message`]) carries the persisted instant of its
    /// delivery; the producer supplies the original timestamp explicitly and
    /// no wall-clock time is fabricated. Derived M4 compaction summaries
    /// ([`InboundKind::CompactionSummary`]) never carry one. Older or
    /// derived messages without a timestamp remain representable: the field
    /// defaults to `None` on deserialization and is omitted from the
    /// canonical encoding while absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<DateTime<Utc>>,
}

/// Provenance of inbound information.
///
/// Provenance is metadata; it never changes the message role.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UserSource {
    /// A human user.
    Human,
    /// Another agent.
    Agent {
        /// Identity of the sending agent.
        agent_id: AgentId,
    },
    /// The fleet/control plane.
    Fleet,
    /// An external system.
    ExternalSystem,
    /// The runtime itself.
    Runtime,
    /// A certified extension. The identity is assigned by rustX during
    /// context admission; contributors never supply this provenance.
    Extension {
        /// The rustX-derived logical extension identity.
        contributor: CertifiedExtensionIdentity,
    },
}

impl UserSource {
    /// The provenance namespaces the canonical message contract exposes.
    /// Context Assembly can derive only `runtime` and `certified_extension`;
    /// the remaining namespaces belong to other core-owned inbound paths.
    pub const PROVENANCE_NAMESPACES: [&'static str; 6] = [
        "human",
        "agent",
        "fleet",
        "external_system",
        "runtime",
        "certified_extension",
    ];
}

/// The cumulative native file-operation facts of one compaction summary
/// (Issue #140).
///
/// This is the typed canonical authority for *which files the retired history
/// read and which files it modified*. It is derived deterministically from
/// the canonical tool calls of the selected retired span — native
/// `read(path)` contributes a read, native `edit(path)` and `write(path)`
/// contribute a modification — merged with the metadata of every earlier
/// compaction summary inside that same span. It records conversation facts,
/// never current filesystem state: a path stays listed even when the file has
/// since been deleted, and the rendered `<read-files>`/`<modified-files>`
/// sections of the summary text are a model-visible projection of this value,
/// never its source.
///
/// The fields are private so every value, including one decoded from durable
/// JSON, satisfies the canonical invariants: both lists are unique and in
/// ascending byte order, and `read_files ∩ modified_files = ∅` (modification
/// wins over read).
///
/// ```compile_fail
/// use rustx::message::types::CompactionSummaryMetadata;
///
/// fn mutate(metadata: &mut CompactionSummaryMetadata) {
///     metadata.read_files = Vec::new();
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CompactionSummaryMetadata {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    read_files: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    modified_files: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct CompactionSummaryMetadataRepr {
    #[serde(default)]
    read_files: Vec<String>,
    #[serde(default)]
    modified_files: Vec<String>,
}

impl<'de> Deserialize<'de> for CompactionSummaryMetadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let repr = CompactionSummaryMetadataRepr::deserialize(deserializer)?;
        Self::new(repr.read_files, repr.modified_files).map_err(serde::de::Error::custom)
    }
}

/// An invalid canonical compaction summary metadata value.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CompactionSummaryMetadataError {
    /// One path appeared more than once within a single list.
    DuplicatePath(String),
    /// A list was not in the canonical ascending byte order.
    NonCanonicalOrder {
        /// The path that appeared first.
        previous: String,
        /// The path that appeared after it.
        next: String,
    },
    /// One path appeared in both lists. Modification wins over read, so an
    /// overlapping value is never canonical.
    ReadModifiedOverlap(String),
}

impl core::fmt::Display for CompactionSummaryMetadataError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::DuplicatePath(path) => {
                write!(f, "compaction summary metadata lists the path {path} twice")
            }
            Self::NonCanonicalOrder { previous, next } => write!(
                f,
                "compaction summary metadata paths are not in ascending order: \
                 {previous} precedes {next}"
            ),
            Self::ReadModifiedOverlap(path) => write!(
                f,
                "compaction summary metadata lists {path} as both read and modified"
            ),
        }
    }
}

impl std::error::Error for CompactionSummaryMetadataError {}

impl CompactionSummaryMetadata {
    /// Creates canonical metadata from already-canonical lists.
    ///
    /// The constructor validates, it never normalizes: both lists must be
    /// duplicate-free, in ascending byte order, and disjoint. Builders that
    /// hold unordered observations use
    /// [`CompactionSummaryMetadata::accumulate`].
    ///
    /// # Errors
    ///
    /// Returns the [`CompactionSummaryMetadataError`] of the first violated
    /// invariant.
    pub fn new(
        read_files: Vec<String>,
        modified_files: Vec<String>,
    ) -> Result<Self, CompactionSummaryMetadataError> {
        validate_ordered_unique(&read_files)?;
        validate_ordered_unique(&modified_files)?;
        for path in &modified_files {
            if read_files.binary_search(path).is_ok() {
                return Err(CompactionSummaryMetadataError::ReadModifiedOverlap(
                    path.clone(),
                ));
            }
        }
        Ok(Self {
            read_files,
            modified_files,
        })
    }

    /// The empty metadata of a compaction whose retired span performed no
    /// native file operation.
    #[must_use]
    pub const fn empty() -> Self {
        Self {
            read_files: Vec::new(),
            modified_files: Vec::new(),
        }
    }

    /// Accumulates the cumulative metadata of one new compaction.
    ///
    /// `inherited` carries the metadata of the earlier compaction summaries
    /// that are inside the selected retired span — and only those; a summary
    /// outside the selected span contributes nothing. The merge is set
    /// semantics over the lineage:
    ///
    /// ```text
    /// read     = inherited_read ∪ new_read
    /// modified = inherited_modified ∪ new_modified
    /// read    -= modified
    /// ```
    ///
    /// The result is canonical by construction: unique, ascending, disjoint.
    #[must_use]
    pub fn accumulate<'a>(
        inherited: impl IntoIterator<Item = &'a Self>,
        new_read: impl IntoIterator<Item = String>,
        new_modified: impl IntoIterator<Item = String>,
    ) -> Self {
        let mut read = std::collections::BTreeSet::new();
        let mut modified = std::collections::BTreeSet::new();
        for summary in inherited {
            read.extend(summary.read_files.iter().cloned());
            modified.extend(summary.modified_files.iter().cloned());
        }
        read.extend(new_read);
        modified.extend(new_modified);
        for path in &modified {
            read.remove(path);
        }
        Self {
            read_files: read.into_iter().collect(),
            modified_files: modified.into_iter().collect(),
        }
    }

    /// The files the retired lineage read without later modifying, in
    /// canonical ascending order.
    #[must_use]
    pub fn read_files(&self) -> &[String] {
        &self.read_files
    }

    /// The files the retired lineage modified, in canonical ascending order.
    #[must_use]
    pub fn modified_files(&self) -> &[String] {
        &self.modified_files
    }
}

/// Validates one metadata list: duplicate-free and in ascending byte order.
fn validate_ordered_unique(paths: &[String]) -> Result<(), CompactionSummaryMetadataError> {
    for pair in paths.windows(2) {
        let [previous, next] = pair else {
            unreachable!("windows(2) yields exactly two elements");
        };
        if previous == next {
            return Err(CompactionSummaryMetadataError::DuplicatePath(
                previous.clone(),
            ));
        }
        if previous > next {
            return Err(CompactionSummaryMetadataError::NonCanonicalOrder {
                previous: previous.clone(),
                next: next.clone(),
            });
        }
    }
    Ok(())
}

/// Typed kind of inbound information.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InboundKind {
    /// An ordinary inbound message.
    #[default]
    Message,
    /// A runtime-provided compaction summary. It remains a `User` message so
    /// no fifth canonical message role is needed for runtime-derived context.
    /// The typed metadata is the cumulative canonical record of the native
    /// file operations of the retired lineage.
    CompactionSummary(CompactionSummaryMetadata),
    /// A model-visible context fact admitted through the rustX Context
    /// Assembly path.
    Context(ContextKind),
}

impl InboundKind {
    /// Whether this inbound fact is a runtime compaction summary.
    #[must_use]
    pub const fn is_compaction_summary(&self) -> bool {
        matches!(self, Self::CompactionSummary(_))
    }

    /// The cumulative file-operation metadata of a compaction summary.
    #[must_use]
    pub const fn compaction_summary_metadata(&self) -> Option<&CompactionSummaryMetadata> {
        match self {
            Self::CompactionSummary(metadata) => Some(metadata),
            _ => None,
        }
    }
}

/// The stable identity of one code-owned Agent Status module.
///
/// This identity belongs to the canonical message layer because an active
/// Agent Status message must carry enough durable information for a later
/// Surface scan to identify the modules it contains. It is intentionally a
/// closed enum rather than extension metadata or a generic key/value field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatusModuleId {
    /// The Time module.
    Time,
    /// The Background module.
    Background,
    /// The conversation-owned Todo module.
    Todo,
}

impl AgentStatusModuleId {
    /// The stable diagnostic and durable-storage name of this module.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Time => "time",
            Self::Background => "background",
            Self::Todo => "todo",
        }
    }
}

/// One semantic Agent Status emission carried by a prepared model-turn start.
///
/// `key` identifies the reminder meaning (for example, the active Todo
/// reminder), while `fingerprint` identifies the bounded relevant state that
/// was actually presented. They are deliberately separate so a changed Todo
/// state does not become a different reminder kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentStatusEmission {
    /// Stable semantic reminder identity owned by the status module.
    pub module_id: AgentStatusModuleId,
    /// Stable key for the reminder meaning.
    pub key: String,
    /// Fingerprint of the exact bounded relevant content.
    pub fingerprint: String,
}

/// An invalid canonical Agent Status module membership.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AgentStatusMetadataError {
    /// A generation must contain at least one admitted module.
    EmptyModules,
    /// A module appeared more than once in the membership list.
    DuplicateModule(AgentStatusModuleId),
    /// Modules must use the closed semantic order (`Time`, `Background`, then
    /// `Todo`).
    NonCanonicalOrder {
        /// The module that appeared first.
        previous: AgentStatusModuleId,
        /// The module that appeared after it.
        next: AgentStatusModuleId,
    },
}

impl core::fmt::Display for AgentStatusMetadataError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::EmptyModules => f.write_str("an Agent Status generation must contain a module"),
            Self::DuplicateModule(module) => {
                write!(f, "Agent Status module {module:?} is duplicated")
            }
            Self::NonCanonicalOrder { previous, next } => write!(
                f,
                "Agent Status modules are not in semantic order: {previous:?} precedes {next:?}"
            ),
        }
    }
}

impl std::error::Error for AgentStatusMetadataError {}

/// The structured durable identity of one canonical Agent Status generation.
///
/// The descriptor is attached to [`ContextKind::AgentStatus`] itself. Its
/// timestamp is the single Agent Status clock sample used to produce the
/// generation, and its typed module list is the source of truth for active
/// Surface visibility. Renderer text is never consulted for either fact.
///
/// The fields are private so every value, including one decoded from durable
/// JSON, has non-empty, duplicate-free membership in deterministic semantic
/// order.
///
/// ```compile_fail
/// use rustx::message::types::AgentStatusGenerationMetadata;
///
/// fn mutate(metadata: &mut AgentStatusGenerationMetadata) {
///     metadata.modules = Vec::new();
/// }
/// ```
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AgentStatusGenerationMetadata {
    generated_at: DateTime<Utc>,
    modules: Vec<AgentStatusModuleId>,
}

#[derive(Debug, Deserialize)]
struct AgentStatusGenerationMetadataRepr {
    generated_at: DateTime<Utc>,
    modules: Vec<AgentStatusModuleId>,
}

impl<'de> Deserialize<'de> for AgentStatusGenerationMetadata {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let repr = AgentStatusGenerationMetadataRepr::deserialize(deserializer)?;
        Self::new(repr.generated_at, repr.modules).map_err(serde::de::Error::custom)
    }
}

impl AgentStatusGenerationMetadata {
    /// Creates canonical metadata for one admitted Agent Status generation.
    ///
    /// # Errors
    ///
    /// Returns an error when membership is empty, contains a duplicate module,
    /// or is not in the closed semantic order.
    pub fn new(
        generated_at: DateTime<Utc>,
        modules: impl IntoIterator<Item = AgentStatusModuleId>,
    ) -> Result<Self, AgentStatusMetadataError> {
        let mut validated = Vec::new();
        for module in modules {
            if validated.contains(&module) {
                return Err(AgentStatusMetadataError::DuplicateModule(module));
            }
            if let Some(previous) = validated.last().copied()
                && module < previous
            {
                return Err(AgentStatusMetadataError::NonCanonicalOrder {
                    previous,
                    next: module,
                });
            }
            validated.push(module);
        }
        if validated.is_empty() {
            return Err(AgentStatusMetadataError::EmptyModules);
        }
        Ok(Self {
            generated_at,
            modules: validated,
        })
    }

    /// The UTC instant at which this generation was produced.
    #[must_use]
    pub fn generated_at(&self) -> DateTime<Utc> {
        self.generated_at
    }

    /// The admitted modules in deterministic semantic order.
    #[must_use]
    pub fn modules(&self) -> &[AgentStatusModuleId] {
        &self.modules
    }

    /// Whether this generation contains `module`.
    #[must_use]
    pub fn contains(&self, module: AgentStatusModuleId) -> bool {
        self.modules.contains(&module)
    }
}

/// The semantic family of one admitted model-visible context fact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContextKind {
    /// The rustX runtime's own observation of a structurally settled tool
    /// batch (Issue #56).
    ///
    /// This family names the native runtime owner, not the timing that made
    /// the fact eligible: a certified extension observing the same batch
    /// produces `ExtensionEnvironment`. The fact is admitted through the
    /// ordinary Context Assembly / pre-step policy / admission path; the
    /// observer never commits it.
    RuntimeToolObservation,
    /// Generic certified-extension/environment context.
    ExtensionEnvironment,
    /// Native runtime/Agent Status and its durable generation identity.
    AgentStatus(AgentStatusGenerationMetadata),
}

impl ContextKind {
    /// Returns the durable Agent Status generation metadata when this is an
    /// Agent Status context fact.
    #[must_use]
    pub fn agent_status_metadata(&self) -> Option<&AgentStatusGenerationMetadata> {
        match self {
            Self::AgentStatus(metadata) => Some(metadata),
            _ => None,
        }
    }
}

/// A content block inside a `UserMessageBlock`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum UserContentBlock {
    /// Plain text.
    Text(TextBlock),
    /// An image reference.
    Image(ImageReference),
    /// A file reference.
    File(FileReference),
}

/// One completed model generation produced by the current agent.
///
/// One generation becomes one immutable `AssistantMessageBlock` containing
/// multiple content blocks. Streaming deltas are never committed here; they
/// belong to `ModelEvent` until the generation completes. `send_message`
/// results and other inbound material from other agents never appear in this
/// role.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AssistantMessageBlock {
    /// Durable message identity.
    pub id: MessageId,
    /// The completed generation content.
    pub content: Vec<AssistantContentBlock>,
}

/// A content block inside an `AssistantMessageBlock`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AssistantContentBlock {
    /// Generated text.
    Text(TextBlock),
    /// Model reasoning, with optional provider continuation state.
    Reasoning(ReasoningBlock),
    /// A tool call emitted by the generation.
    ToolCall(ToolCall),
    /// A refusal to comply with the request.
    Refusal(RefusalBlock),
    /// An image reference produced by the generation.
    Image(ImageReference),
}

/// Model reasoning content.
///
/// Reasoning text is preserved for diagnostics, but reasoning/continuation
/// state is never flattened into plain text: provider-specific opaque state
/// survives on the [`ProviderContinuationState`] boundary for later
/// continuation.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ReasoningBlock {
    /// The reasoning text, when the provider exposed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Provider continuation state required to continue the generation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_state: Option<ProviderContinuationState>,
}

/// A refusal generated by the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusalBlock {
    /// The refusal explanation text.
    pub text: String,
}

/// The result of one tool call produced by the current agent.
///
/// This block is the canonical conversation record of an execution outcome
/// and composes [`ToolExecutionResult`] as its single source of truth. For
/// `send_message`-style platform tools, the result is only the delivery
/// acceptance/rejection acknowledgment; a later reply from the recipient
/// arrives as a `UserMessageBlock` with agent provenance and is never nested
/// here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ToolMessageBlock {
    /// Durable message identity.
    pub id: MessageId,
    /// Identity of the tool call this block answers.
    pub tool_call_id: ToolCallId,
    /// Identity of the executed tool.
    pub tool_id: ToolId,
    /// The normalized execution result.
    pub result: ToolExecutionResult,
}

#[cfg(test)]
mod tests {
    use super::{
        AgentStatusGenerationMetadata, AgentStatusMetadataError, AgentStatusModuleId,
        AssistantContentBlock, AssistantMessageBlock, CompactionSummaryMetadata,
        CompactionSummaryMetadataError, InboundKind, MessageBlock, ToolMessageBlock,
        UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::message::content::TextBlock;
    use crate::runtime::identity::{AgentId, MessageId, ToolCallId, ToolId};
    use crate::tools::types::{ToolExecutionResult, ToolExecutionStatus};
    use chrono::{DateTime, Utc};

    /// All three canonical conversational roles serialize with stable discriminators.
    #[test]
    fn three_roles_have_stable_discriminators() {
        let user = MessageBlock::User(UserMessageBlock {
            id: MessageId::new("msg-user-1"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "hi".to_owned(),
            })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        });
        let assistant = MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("msg-assistant-1"),
            content: vec![AssistantContentBlock::Text(TextBlock {
                text: "ok".to_owned(),
            })],
        });
        let tool = MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new("msg-tool-1"),
            tool_call_id: ToolCallId::new("call_01"),
            tool_id: ToolId::new("tool-bash"),
            result: ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: Vec::new(),
                duration_ms: 1,
                exit_code: Some(0),
                artifacts: Vec::new(),
                truncation: None,
                managed_output: None,
            },
        });
        for (block, role) in [(user, "user"), (assistant, "assistant"), (tool, "tool")] {
            let value = serde_json::to_value(&block).expect("serialize block");
            assert_eq!(value["role"], role, "unexpected discriminator");
        }

        let legacy_agent = serde_json::json!({
            "role": "agent",
            "id": "msg-agent-legacy",
            "content": [{"type": "text", "text": "must reject"}],
        });
        assert!(
            serde_json::from_value::<MessageBlock>(legacy_agent).is_err(),
            "the canonical role discriminator must be assistant"
        );
    }

    /// An inbound message from another agent stays a `UserMessageBlock`.
    #[test]
    fn agent_to_agent_inbound_remains_user_role() {
        let block = MessageBlock::User(UserMessageBlock {
            id: MessageId::new("msg-user-2"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "Task done.".to_owned(),
            })],
            source: UserSource::Agent {
                agent_id: AgentId::new("agent-b"),
            },
            kind: InboundKind::Message,
            timestamp: None,
        });
        let value = serde_json::to_value(&block).expect("serialize block");
        assert_eq!(value["role"], "user");
        assert_eq!(value["source"]["agent"]["agent_id"], "agent-b");
        assert!(matches!(
            block,
            MessageBlock::User(UserMessageBlock {
                source: UserSource::Agent { .. },
                ..
            })
        ));
        assert!(!matches!(block, MessageBlock::Assistant(_)));
        assert!(!matches!(block, MessageBlock::Tool(_)));
    }

    /// A runtime compaction summary remains a `UserMessageBlock`; no fifth
    /// canonical role is required. Its typed cumulative file-operation
    /// metadata rides on the kind and round-trips exactly.
    #[test]
    fn compaction_summary_is_user_role() {
        let metadata = CompactionSummaryMetadata::new(
            vec!["/src/a.rs".to_owned()],
            vec!["/src/b.rs".to_owned()],
        )
        .expect("valid metadata");
        let block = MessageBlock::User(UserMessageBlock {
            id: MessageId::new("msg-summary-1"),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "Earlier in the conversation the agent listed files.".to_owned(),
            })],
            source: UserSource::Runtime,
            kind: InboundKind::CompactionSummary(metadata),
            timestamp: None,
        });
        let json = serde_json::to_string(&block).expect("serialize block");
        let decoded: MessageBlock = serde_json::from_str(&json).expect("deserialize block");
        assert_eq!(decoded, block);
        assert!(matches!(
            decoded,
            MessageBlock::User(UserMessageBlock {
                kind: InboundKind::CompactionSummary(_),
                source: UserSource::Runtime,
                ..
            })
        ));
    }

    /// Valid metadata keeps its exact lists; invalid values are rejected
    /// without normalization.
    #[test]
    fn compaction_metadata_validates_the_canonical_invariants() {
        let valid = CompactionSummaryMetadata::new(
            vec!["/a".to_owned(), "/b".to_owned()],
            vec!["/c".to_owned()],
        )
        .expect("valid metadata");
        assert_eq!(valid.read_files(), &["/a".to_owned(), "/b".to_owned()]);
        assert_eq!(valid.modified_files(), &["/c".to_owned()]);
        assert!(CompactionSummaryMetadata::empty().read_files().is_empty());

        assert_eq!(
            CompactionSummaryMetadata::new(vec!["/a".to_owned(), "/a".to_owned()], Vec::new()),
            Err(CompactionSummaryMetadataError::DuplicatePath(
                "/a".to_owned()
            ))
        );
        assert_eq!(
            CompactionSummaryMetadata::new(vec!["/b".to_owned(), "/a".to_owned()], Vec::new()),
            Err(CompactionSummaryMetadataError::NonCanonicalOrder {
                previous: "/b".to_owned(),
                next: "/a".to_owned(),
            })
        );
        assert_eq!(
            CompactionSummaryMetadata::new(vec!["/a".to_owned()], vec!["/a".to_owned()]),
            Err(CompactionSummaryMetadataError::ReadModifiedOverlap(
                "/a".to_owned()
            ))
        );
    }

    /// Accumulation is lineage set semantics: union, deterministic ascending
    /// order, and modification wins over read.
    #[test]
    fn compaction_metadata_accumulates_over_the_lineage() {
        let inherited = CompactionSummaryMetadata::new(
            vec!["/a".to_owned(), "/b".to_owned()],
            vec!["/c".to_owned()],
        )
        .expect("valid metadata");
        let merged = CompactionSummaryMetadata::accumulate(
            [&inherited],
            ["/d".to_owned(), "/a".to_owned()],
            ["/a".to_owned()],
        );
        assert_eq!(merged.read_files(), &["/b".to_owned(), "/d".to_owned()]);
        assert_eq!(merged.modified_files(), &["/a".to_owned(), "/c".to_owned()]);
    }

    /// Durable JSON of invalid metadata fails closed instead of decoding
    /// into a non-canonical value.
    #[test]
    fn compaction_metadata_serde_rejects_invalid_values() {
        for value in [
            serde_json::json!({"read_files": ["/a", "/a"], "modified_files": []}),
            serde_json::json!({"read_files": ["/b", "/a"], "modified_files": []}),
            serde_json::json!({"read_files": ["/a"], "modified_files": ["/a"]}),
        ] {
            assert!(
                serde_json::from_value::<CompactionSummaryMetadata>(value).is_err(),
                "invalid metadata must fail closed"
            );
        }
        let metadata =
            CompactionSummaryMetadata::new(vec![], vec!["/a".to_owned()]).expect("valid metadata");
        let value = serde_json::to_value(&metadata).expect("serialize");
        assert_eq!(value, serde_json::json!({"modified_files": ["/a"]}));
        assert_eq!(
            serde_json::from_value::<CompactionSummaryMetadata>(value).expect("decode"),
            metadata,
            "a durable round trip preserves the validated value exactly"
        );
    }

    fn status_timestamp() -> DateTime<Utc> {
        DateTime::from_timestamp(1_754_000_000, 0).expect("timestamp")
    }

    #[test]
    fn agent_status_metadata_accepts_each_valid_closed_membership() {
        for modules in [
            vec![AgentStatusModuleId::Time],
            vec![AgentStatusModuleId::Background],
            vec![AgentStatusModuleId::Todo],
            vec![AgentStatusModuleId::Time, AgentStatusModuleId::Background],
            vec![
                AgentStatusModuleId::Time,
                AgentStatusModuleId::Background,
                AgentStatusModuleId::Todo,
            ],
        ] {
            let metadata = AgentStatusGenerationMetadata::new(status_timestamp(), modules.clone())
                .expect("valid Agent Status module membership");
            assert_eq!(metadata.modules(), modules.as_slice());
            assert_eq!(metadata.generated_at(), status_timestamp());
        }
    }

    #[test]
    fn agent_status_metadata_rejects_invalid_membership_without_normalizing() {
        assert_eq!(
            AgentStatusGenerationMetadata::new(status_timestamp(), Vec::new()),
            Err(AgentStatusMetadataError::EmptyModules)
        );
        assert_eq!(
            AgentStatusGenerationMetadata::new(
                status_timestamp(),
                [AgentStatusModuleId::Time, AgentStatusModuleId::Time]
            ),
            Err(AgentStatusMetadataError::DuplicateModule(
                AgentStatusModuleId::Time
            ))
        );
        assert_eq!(
            AgentStatusGenerationMetadata::new(
                status_timestamp(),
                [AgentStatusModuleId::Background, AgentStatusModuleId::Time]
            ),
            Err(AgentStatusMetadataError::NonCanonicalOrder {
                previous: AgentStatusModuleId::Background,
                next: AgentStatusModuleId::Time,
            })
        );
    }

    #[test]
    fn agent_status_metadata_serde_rejects_invalid_membership() {
        let invalid_memberships = [
            serde_json::json!([]),
            serde_json::json!(["time", "time"]),
            serde_json::json!(["background", "time"]),
            serde_json::json!(["unknown"]),
        ];
        for modules in invalid_memberships {
            let value = serde_json::json!({
                "generated_at": status_timestamp(),
                "modules": modules,
            });
            assert!(
                serde_json::from_value::<AgentStatusGenerationMetadata>(value).is_err(),
                "invalid membership must fail closed"
            );
        }
    }

    #[test]
    fn agent_status_metadata_keeps_the_existing_wire_shape() {
        let metadata = AgentStatusGenerationMetadata::new(
            status_timestamp(),
            [AgentStatusModuleId::Time, AgentStatusModuleId::Background],
        )
        .expect("valid metadata");
        assert_eq!(
            serde_json::to_value(metadata).expect("serialize metadata"),
            serde_json::json!({
                "generated_at": status_timestamp(),
                "modules": ["time", "background"],
            })
        );
    }
}
