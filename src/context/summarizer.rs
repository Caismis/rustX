//! Provider-neutral summary generation over detached typed projected history.
//!
//! The selected historical span may contain Original messages and previous
//! Derived summaries. Fixed structured Markdown preserves Goal, Constraints,
//! Progress, Decisions, Next Steps and Critical Context. Trusted cumulative file
//! metadata comes from canonical Tool facts and prior immutable checkpoint
//! metadata; generated prose never becomes execution authority.
//!
//! Summary generation exposes no tools, dynamic contributors or continuation.
//! It never enters `AgentExecution` and never commits a canonical message.

use std::fmt::Write as _;
use std::sync::{Arc, OnceLock};

use futures_util::future::BoxFuture;
use serde::{Deserialize, Serialize};

use crate::context::error::{ContextError, ContextErrorKind};
use crate::message::content::TextBlock;
use crate::message::types::{
    AssistantContentBlock, MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
};
use crate::model::deadline::ModelTimeoutPolicy;
use crate::model::invocation::ResolvedModelInvocation;
use crate::model::types::ModelRequest;
use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::monotonic::MonotonicClock;
use crate::tools::types::{ToolExecutionStatus, ToolResultContent};

/// One summarizer request.
///
/// The request carries the complete typed messages of the selected historical
/// projection span in its compiled order, including prior Derived summaries.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SummaryRequest {
    /// Detached selected historical projection messages, including Derived context.
    pub retired: Vec<crate::model::ModelInputMessage>,
}

/// The exact provider-neutral model input assembled for one summary request.
///
/// Both compaction planning and [`ModelBackedSummarizer`] consume this value:
/// the fixed instruction, the deterministic rendered transcript, and the
/// identity-free runtime User wrapper therefore cannot drift between estimation
/// and the production invocation.
#[derive(Debug, Clone, PartialEq)]
pub struct SummaryModelInput {
    /// Exact typed semantic messages sent to the summary model.
    pub messages: Vec<crate::model::ModelInputMessage>,
}

/// The maximum characters of one tool result rendered into a summary
/// request.
///
/// Tool output dominates the token cost of an agentic conversation and is
/// the least summary-relevant part of it: the head of a result carries the
/// outcome, the tail is bulk. Without this bound the rendered span is
/// *larger* than the conversation it replaces, which is exactly how a
/// compaction triggered by a context overflow overflows again.
pub const SUMMARY_TOOL_RESULT_MAX_CHARS: usize = 2_000;

/// The maximum characters of one reasoning block rendered into a summary
/// request. Replayed reasoning is bulky and historical; the summary needs
/// its gist, never its full text.
pub const SUMMARY_REASONING_MAX_CHARS: usize = 1_000;

/// The maximum characters of one rendered tool-call argument payload.
pub const SUMMARY_TOOL_ARGUMENTS_MAX_CHARS: usize = 1_000;

impl SummaryRequest {
    /// Whether this request covers at least one canonical message.
    #[must_use]
    pub fn advances_coverage(&self) -> bool {
        !self.retired.is_empty()
    }

    /// Renders the retired span as the deterministic plain-text transcript
    /// the summary model reads.
    ///
    /// This is deliberately **not** the canonical JSON encoding. Canonical
    /// JSON is the durable interchange format, and pushing it through a
    /// model request costs far more tokens than the conversation it
    /// describes: every structural quote, field name, and escaped newline
    /// becomes input. The rendering below is a pure, deterministic function
    /// of the retired span that keeps what a summary needs — who said what,
    /// which tools ran with which arguments, and how they ended — while
    /// bounding the bulk contributors (tool results, replayed reasoning,
    /// tool-call arguments).
    #[must_use]
    pub fn render_transcript(&self) -> String {
        let mut transcript = String::new();
        for message in &self.retired {
            append_summary_message(message, &mut transcript);
        }
        transcript
    }

    /// Assembles the exact provider-neutral model input used by the production
    /// summary invocation.
    #[must_use]
    pub fn model_input(&self) -> SummaryModelInput {
        summary_model_input(&self.render_transcript())
    }
}

/// Render each historical message once; no detached prefix clone is needed.
pub(crate) fn append_summary_message(
    message: &crate::model::ModelInputMessage,
    transcript: &mut String,
) {
    use crate::model::projected::{ModelMessageView, ProjectedMessage};
    #[cfg(test)]
    SUMMARY_RENDERED_MESSAGES.with(|count| count.set(count.get() + 1));
    let mut parts = Vec::new();
    match message.view() {
        ModelMessageView::User(content) => {
            let label = match message {
                crate::model::ModelInputMessage::Canonical(MessageBlock::User(user)) => {
                    user_label(user)
                }
                crate::model::ModelInputMessage::Derived(derived)
                    if derived
                        .transformations
                        .contains(&crate::model::projected::SemanticTransformation::Summary) =>
                {
                    "Earlier summary".to_owned()
                }
                crate::model::ModelInputMessage::Derived(derived) => match &derived.message {
                    ProjectedMessage::User { source, kind, .. } => source_label(source, kind),
                    _ => unreachable!(),
                },
                _ => unreachable!(),
            };
            render_user(content, &label, &mut parts);
        }
        ModelMessageView::Assistant(content) => render_assistant(content, &mut parts),
        ModelMessageView::Tool(call, result) => render_tool(call, result, &mut parts),
        ModelMessageView::RequestOnly(context) => parts.push(context.render()),
    }
    for part in parts {
        if !transcript.is_empty() {
            transcript.push_str("\n\n");
        }
        transcript.push_str(&part);
    }
}

/// The same complete instruction/wrapper used by the real summary invocation.
pub(crate) fn summary_model_input(transcript: &str) -> SummaryModelInput {
    let instruction = format!(
        "{SUMMARY_INSTRUCTION}\n\n<retired-conversation>\n{transcript}\n</retired-conversation>"
    );
    #[cfg(test)]
    SUMMARY_COPIED_BYTES
        .with(|total| total.set(total.get().saturating_add(instruction.len() as u64)));
    SummaryModelInput {
        messages: vec![crate::model::ModelInputMessage::Derived(
            crate::model::projected::DerivedModelMessage {
                message: crate::model::projected::ProjectedMessage::User {
                    content: vec![UserContentBlock::Text(TextBlock { text: instruction })],
                    source: UserSource::Runtime,
                    kind: crate::model::projected::ProjectedUserKind::Message,
                    timestamp: None,
                },
                sources: Vec::new(),
                checkpoint_sources: Vec::new(),
                transformations: vec![crate::model::projected::SemanticTransformation::Summary],
            },
        )],
    }
}
#[cfg(test)]
thread_local! { static SUMMARY_RENDERED_MESSAGES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(crate) fn take_summary_rendered_messages() -> usize {
    SUMMARY_RENDERED_MESSAGES.with(|count| count.replace(0))
}

#[cfg(test)]
thread_local! { static SUMMARY_COPIED_BYTES: std::cell::Cell<u64> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(crate) fn take_summary_copied_bytes() -> u64 {
    SUMMARY_COPIED_BYTES.with(|total| total.replace(0))
}

/// Truncates on a character boundary, reporting how much was dropped.
///
/// Truncation is always announced: a summary model must never be able to
/// mistake a cut-off tool result for a complete one.
fn truncate_for_summary(text: &str, max_chars: usize) -> String {
    let mut indices = text.char_indices();
    let Some((cut, _)) = indices.nth(max_chars) else {
        return text.to_owned();
    };
    let dropped = text[cut..].chars().count();
    format!(
        "{}\n[... {dropped} more characters truncated]",
        &text[..cut]
    )
}

/// The transcript label of one inbound message's provenance and kind.
fn user_label(user: &UserMessageBlock) -> String {
    source_label(
        &user.source,
        &crate::model::projected::ProjectedUserKind::from(&user.kind),
    )
}

fn source_label(source: &UserSource, kind: &crate::model::projected::ProjectedUserKind) -> String {
    match (kind, source) {
        (crate::model::projected::ProjectedUserKind::CompactionContext, _) => {
            "Runtime compaction context".to_owned()
        }
        (crate::model::projected::ProjectedUserKind::Context(kind), _) => {
            format!("Runtime context ({kind:?})")
        }
        (_, UserSource::Runtime) => "Runtime".to_owned(),
        (_, UserSource::Agent { agent_id }) => format!("Agent {agent_id}"),
        (_, UserSource::Fleet) => "Fleet".to_owned(),
        (_, UserSource::ExternalSystem) => "External system".to_owned(),
        (_, UserSource::Extension { contributor }) => format!("Extension {contributor}"),
        (_, UserSource::Human) => "User".to_owned(),
    }
}

fn render_user(content: &[UserContentBlock], label: &str, parts: &mut Vec<String>) {
    let mut rendered = String::new();
    for block in content {
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        match block {
            UserContentBlock::Text(text) => rendered.push_str(&text.text),
            UserContentBlock::UploadedFile(file) => {
                let _ = write!(rendered, "[uploaded file {}]", file.name);
            }
            UserContentBlock::Image(image) => {
                let _ = write!(rendered, "[image {}]", image.artifact_id);
            }
            UserContentBlock::File(file) => {
                let _ = write!(
                    rendered,
                    "[file {}]",
                    file.name.as_deref().unwrap_or("attachment")
                );
            }
        }
    }
    if rendered.is_empty() {
        return;
    }
    parts.push(format!("[{label}]: {rendered}"));
}

fn render_assistant(content: &[AssistantContentBlock], parts: &mut Vec<String>) {
    for block in content {
        match block {
            AssistantContentBlock::Text(text) if !text.text.is_empty() => {
                parts.push(format!("[Assistant]: {}", text.text));
            }
            AssistantContentBlock::Reasoning(reasoning) => {
                if let Some(text) = reasoning.text.as_deref()
                    && !text.is_empty()
                {
                    parts.push(format!(
                        "[Assistant thinking]: {}",
                        truncate_for_summary(text, SUMMARY_REASONING_MAX_CHARS)
                    ));
                }
            }
            AssistantContentBlock::ToolCall(call) => {
                let arguments = serde_json::to_string(&call.arguments)
                    .unwrap_or_else(|_| "[unserializable]".to_owned());
                parts.push(format!(
                    "[Tool call {}]: {}({})",
                    call.id,
                    call.name,
                    truncate_for_summary(&arguments, SUMMARY_TOOL_ARGUMENTS_MAX_CHARS)
                ));
            }
            AssistantContentBlock::Refusal(refusal) => {
                parts.push(format!("[Assistant refusal]: {}", refusal.text));
            }
            AssistantContentBlock::Image(image) => {
                parts.push(format!("[Assistant image]: {}", image.artifact_id));
            }
            AssistantContentBlock::Text(_) => {}
        }
    }
}

fn render_tool(
    call: &crate::runtime::identity::ToolCallId,
    result: &crate::tools::types::ToolExecutionResult,
    parts: &mut Vec<String>,
) {
    let mut rendered = String::new();
    for content in &result.content {
        if !rendered.is_empty() {
            rendered.push('\n');
        }
        match content {
            ToolResultContent::Text(text) => rendered.push_str(&text.text),
            ToolResultContent::Json { value } => rendered.push_str(
                &serde_json::to_string(value).unwrap_or_else(|_| "[unserializable]".to_owned()),
            ),
            ToolResultContent::File(file) => {
                let _ = write!(
                    rendered,
                    "[file {}]",
                    file.name.as_deref().unwrap_or("artifact")
                );
            }
            ToolResultContent::Image(image) => {
                let _ = write!(rendered, "[image {}]", image.artifact_id);
            }
        }
    }
    let status = match &result.status {
        ToolExecutionStatus::Success => "success".to_owned(),
        ToolExecutionStatus::Failed { error } => format!("failed: {error}"),
        ToolExecutionStatus::Denied { reason } => format!("denied: {reason}"),
        // Cancelled, TimedOut, and OutcomeUnknown keep their full typed
        // model-facing feedback so the semantic distinction between a
        // confirmed terminal settlement and an unknown external outcome
        // survives summarization.
        status @ (ToolExecutionStatus::Cancelled { .. }
        | ToolExecutionStatus::TimedOut
        | ToolExecutionStatus::OutcomeUnknown { .. }) => status
            .feedback_text()
            .expect("non-success status has model-facing text"),
    };
    parts.push(format!(
        "[Tool result {} ({status})]: {}",
        call,
        truncate_for_summary(&rendered, SUMMARY_TOOL_RESULT_MAX_CHARS)
    ));
}

/// The summary service.
pub trait ContextSummarizer: Send + Sync {
    /// Composition installs one native owner before Summary admission.
    /// # Panics
    /// A production service cannot replace its already-frozen native owner.
    fn bind_backend(&self, backend: Arc<dyn SummaryBackend>, resource_revision: u64);
    /// Scripted Context services own their test authority directly.
    fn has_backend(&self) -> bool;
    /// Admits one business operation before history/media preparation. Every
    /// replan shares its budget, frozen invocation and close-fenced children.
    /// # Errors
    /// Refuses Busy, closed admission, an expired budget or missing native ownership.
    fn operation(
        &self,
        budget: super::compaction_budget::CompactionBudget,
        cancellation: CancellationSignal,
    ) -> Result<Box<dyn SummaryOperation>, ContextError>;
}

/// One Compaction business tail; normal return waits for its native children.
pub trait SummaryOperation: Send + Sync {
    /// The original accounting object, shared by every candidate and re-entry.
    fn budget(&self) -> super::compaction_budget::CompactionBudget;
    /// Reserves the next Call before expensive candidate preparation.
    /// # Errors
    /// Refuses exhausted capacity, closed parent authority or an expired budget.
    fn prepare(&self) -> Result<Box<dyn PreparedSummary>, ContextError>;
    /// Deadline and parent close arbitrate at the checkpoint grant itself.
    /// # Errors
    /// Rejects expired/revoked authority or children that have not physically exited.
    fn authorize_commit(
        &self,
    ) -> Result<crate::runtime::cancellation::CompactionCommitGrant, ContextError>;
    /// Closes child admission, cancels inference and awaits actual native exit.
    fn close(&self) -> BoxFuture<'_, ()>;
}

/// The sole prepared Call consumed by one planned Summary input.
pub trait PreparedSummary: Send {
    fn summarize(
        self: Box<Self>,
        request: SummaryRequest,
        cut: Option<super::resources::ResourceReadCut>,
    ) -> BoxFuture<'static, Result<SummaryGeneration, ContextError>>;
}

/// Accepted Summary text plus confirmed native generation dependency.
#[derive(Debug)]
pub struct SummaryGeneration {
    pub text: String,
    /// Scripted Context services have no physical generation; production does.
    pub generation: Option<super::checkpoint::SummaryGenerationRef>,
}

/// Runtime supplies this narrow Context interface, keeping concrete execution
/// ownership out of Context and Store/model facts.
pub trait SummaryBackend: Send + Sync {
    /// Freezes native execution policy and reserves before preparation.
    /// # Errors
    /// Refuses unavailable capacity, lifecycle/durability closure or invalid ownership.
    fn operation(
        &self,
        invocation: ResolvedModelInvocation,
        timeout: ModelTimeoutPolicy,
        resource_revision: u64,
        budget: super::compaction_budget::CompactionBudget,
        cancellation: CancellationSignal,
    ) -> Result<Box<dyn SummaryOperation>, ContextError>;
}

/// The production summarizer backed by a frozen resolved summary-model
/// invocation.
///
/// For automatic compaction the invocation is the attempt's frozen summary
/// resolution. Manual compaction freezes the same session resolution at its
/// idle admission boundary. In `session` mode this is the primary invocation;
/// in `explicit` mode it is the separately resolved summary invocation,
/// optionally narrowed by the context plane's summary output safety cap.
/// There is deliberately no independent "summary provider config" system.
///
/// The summary request is a canonical one-off `ModelRequest`: no tools, no
/// Agent Status, no Skill catalog, no continuation, and a deterministic
/// instruction plus the serialized request input. A refusal (with or without
/// a `RefusalDelta`), a tool request, an invalid stream, a model failure, or
/// an empty/whitespace-only output is a compaction failure, and any provider
/// continuation emitted by the one-off request is discarded: summary
/// activity never contaminates the attempt's continuation or usage state.
pub struct ModelBackedSummarizer {
    invocation: ResolvedModelInvocation,
    model_timeout_policy: ModelTimeoutPolicy,
    #[cfg(test)]
    monotonic_clock: Arc<dyn MonotonicClock>,
    backend: OnceLock<(Arc<dyn SummaryBackend>, u64)>,
}

/// The fixed structured summary instruction (Issue #140).
///
/// The production summarizer requires one exact Markdown contract — Goal,
/// Constraints & Preferences, Progress (Done / In Progress / Blocked), Key
/// Decisions, Next Steps, Critical Context — rather than free-form text.
/// Structured means this fixed contract only: rustX does not parse the
/// headings back into fields, and the file-operation facts of the transition
/// come from canonical tool-call extraction, never from the generated prose.
const SUMMARY_INSTRUCTION: &str = r#"Summarize the following retired conversation history so a later primary model turn can continue the work without re-reading the retired span. When the retired history contains an earlier summary, update and merge it into one coherent new summary; do not repeat it verbatim as a separate section.

Output the summary in EXACTLY this Markdown structure, with these exact headings and nothing before the first heading or after the last section:

## Goal

[What is the user trying to accomplish?]

## Constraints & Preferences

- [Constraints, preferences, requirements]
- [Or "(none)"]

## Progress

### Done

- [x] [Completed work]

### In Progress

- [ ] [Current work]

### Blocked

- [Current blockers]

## Key Decisions

- **[Decision]**: [Brief rationale]

## Next Steps

1. [Ordered next action]

## Critical Context

- [Important data, examples, references]
- [Or "(none)"]

Preserve relevant exact details: file paths, type and function names, important error strings, critical values, user constraints, and the rationale behind key decisions. Treat Agent Status and other runtime observations as historical evidence, not live authority: for example, observations that a task was running at one time and completed later may be summarized as having run earlier and completed, never as a claim about what is currently running or completed. If an Agent Status section is absent from a later observation, do not infer lifecycle completion or disappearance unless that section's explicit contract says absence has that meaning. Output only the summary Markdown; do not wrap it in code fences."#;

impl ModelBackedSummarizer {
    /// Creates a summarizer over one resolved summary invocation.
    #[must_use]
    pub fn new(
        invocation: ResolvedModelInvocation,
        model_timeout_policy: ModelTimeoutPolicy,
        monotonic_clock: Arc<dyn MonotonicClock>,
    ) -> Self {
        #[cfg(not(test))]
        drop(monotonic_clock);
        Self {
            invocation,
            model_timeout_policy,
            #[cfg(test)]
            monotonic_clock,
            backend: OnceLock::new(),
        }
    }

    /// Standalone stream fixtures use the production native executor over a
    /// real isolated Store; they carry no historical checkpoint authority.
    #[cfg(test)]
    pub(crate) fn summarize(
        &self,
        request: SummaryRequest,
        cancellation: CancellationSignal,
    ) -> BoxFuture<'_, Result<String, ContextError>> {
        Box::pin(async move {
            if !self.has_backend() {
                let store = Arc::new(
                    crate::durable::SqliteConversationStore::in_memory(
                        crate::runtime::identity::ConversationId::generate(),
                    )
                    .expect("summary fixture Store"),
                );
                self.bind_backend(
                    Arc::new(
                        crate::runtime::auxiliary::executor::AuxiliaryExecutor::for_test(
                            store,
                            self.monotonic_clock.clone(),
                        )?,
                    ),
                    0,
                );
            }
            let budget = super::compaction_budget::CompactionBudget::new(
                crate::runtime::identity::CompactionOperationId::new(
                    uuid::Uuid::now_v7().to_string(),
                ),
                self.monotonic_clock.clone(),
            );
            let operation = self.operation(budget, cancellation)?;
            let result = operation.prepare()?.summarize(request, None).await;
            operation.close().await;
            result.map(|generation| generation.text)
        })
    }

    /// The resolved invocation this summarizer sends with.
    #[must_use]
    pub const fn invocation(&self) -> &ResolvedModelInvocation {
        &self.invocation
    }

    /// The deterministic summary instruction text.
    #[must_use]
    pub const fn instruction() -> &'static str {
        SUMMARY_INSTRUCTION
    }

    /// Renders the request input deterministically for the model.
    ///
    /// The rendering is a pure function of the request; see
    /// [`SummaryRequest::render_transcript`] for why it is a bounded plain
    /// transcript rather than the canonical JSON encoding.
    #[must_use]
    pub fn serialize_input(request: &SummaryRequest) -> String {
        request.render_transcript()
    }
}

impl ContextSummarizer for ModelBackedSummarizer {
    fn bind_backend(&self, backend: Arc<dyn SummaryBackend>, resource_revision: u64) {
        assert!(
            self.backend.set((backend, resource_revision)).is_ok(),
            "one native Summary backend per frozen model service"
        );
    }
    fn has_backend(&self) -> bool {
        self.backend.get().is_some()
    }
    fn operation(
        &self,
        budget: super::compaction_budget::CompactionBudget,
        cancellation: CancellationSignal,
    ) -> Result<Box<dyn SummaryOperation>, ContextError> {
        let (backend, revision) = self.backend.get().ok_or_else(|| {
            ContextError::new(
                ContextErrorKind::AuxiliaryNotStarted,
                "Summary has no native execution owner",
            )
        })?;
        backend.operation(
            self.invocation.clone(),
            self.model_timeout_policy,
            *revision,
            budget,
            cancellation,
        )
    }
}

impl SummaryRequest {
    /// Context owns the exact one-shot input; Runtime only executes the frozen value.
    #[must_use]
    pub fn model_request(&self, invocation: &ResolvedModelInvocation) -> ModelRequest {
        ModelRequest {
            continuation_source: None,
            images: std::collections::BTreeMap::new(),
            invocation: invocation.invocation_config(),
            messages: self.model_input().messages.into(),
            tools: Vec::new(),
            effective_system_prompt: String::new(),
            continuation: None,
        }
    }
}

/// Domain acceptance is independent of successful native execution and settlement.
/// # Errors
/// A refused or empty result cannot replace retired historical input.
pub fn accept_summary(text: String, refused: bool) -> Result<String, ContextError> {
    if refused {
        return Err(ContextError::new(
            ContextErrorKind::SummaryFailed,
            "summary generation refused",
        ));
    }
    if text.trim().is_empty() {
        return Err(ContextError::new(
            ContextErrorKind::SummaryFailed,
            "summary generation produced no content",
        ));
    }
    Ok(text)
}

/// Maps confirmed physical failure into the Summary domain without retrying it.
#[must_use]
pub fn summary_execution_failed(error: crate::model::ModelError) -> ContextError {
    ContextError::new(
        match error.kind {
            crate::model::ModelErrorKind::Cancelled => ContextErrorKind::Cancelled,
            crate::model::ModelErrorKind::ContextWindowExceeded => {
                ContextErrorKind::SummaryInputTooLarge
            }
            crate::model::ModelErrorKind::Timeout => ContextErrorKind::AuxiliaryDeadlineExceeded,
            _ => ContextErrorKind::SummaryFailed,
        },
        error.message,
    )
}
#[cfg(test)]
mod tests {
    use super::{ModelBackedSummarizer, SUMMARY_TOOL_RESULT_MAX_CHARS, SummaryRequest};
    use crate::message::content::TextBlock;
    use crate::message::types::{
        InboundKind, MessageBlock, ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::runtime::identity::{MessageId, ToolCallId, ToolId};
    use crate::runtime::types::CancellationReason;
    use crate::tools::types::{
        ToolCancellationPhase, ToolExecutionResult, ToolExecutionStatus, ToolResultContent,
    };

    fn request() -> SummaryRequest {
        SummaryRequest {
            retired: (vec![MessageBlock::User(UserMessageBlock {
                id: MessageId::new("retired-1"),
                content: vec![UserContentBlock::Text(TextBlock {
                    text: "same payload".to_owned(),
                })],
                source: UserSource::Human,
                kind: InboundKind::Message,
                timestamp: None,
            })])
            .into_iter()
            .map(Into::into)
            .collect(),
        }
    }

    /// The shared assembly boundary includes the instruction, serialized
    /// request envelope, and canonical runtime User wrapper exactly once.
    #[test]
    fn summary_model_input_is_deterministic_and_complete() {
        let request = request();
        let first = request.model_input();
        assert_eq!(first, request.model_input());
        assert_eq!(first.messages.len(), 1);
        let crate::model::input::ModelInputMessage::Derived(wrapper) = &first.messages[0] else {
            panic!("summary service input must not fabricate a canonical identity");
        };
        assert!(wrapper.sources.is_empty());
        let crate::model::projected::ProjectedMessage::User { content, .. } = &wrapper.message
        else {
            panic!("summary service input must use the User role");
        };
        let UserContentBlock::Text(text) = &content[0] else {
            panic!("summary input must use a text wrapper");
        };
        assert_eq!(
            text.text,
            format!(
                "{}\n\n<retired-conversation>\n{}\n</retired-conversation>",
                ModelBackedSummarizer::instruction(),
                request.render_transcript()
            )
        );
    }

    /// The rendered transcript is plain text, not canonical JSON, and it
    /// bounds the bulk contributors. A summary request must never be larger
    /// than the history it replaces.
    #[test]
    fn rendered_transcript_bounds_tool_output() {
        let request = SummaryRequest {
            retired: (vec![MessageBlock::Tool(ToolMessageBlock {
                occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                    crate::runtime::identity::MessageId::new("assistant"),
                    crate::message::types::ContentBlockIndex::new(0),
                ),
                id: MessageId::new("tool-1"),
                tool_call_id: ToolCallId::new("call-1"),
                tool_id: ToolId::new("tool-bash"),
                result: ToolExecutionResult {
                    deliveries: Vec::new(),
                    status: ToolExecutionStatus::Success,
                    content: vec![ToolResultContent::Text(TextBlock {
                        text: "x".repeat(50_000),
                    })],
                    duration_ms: 1,
                    exit_code: Some(0),
                    artifacts: Vec::new(),
                    truncation: None,
                    workflow: None,
                    managed_output: None,
                },
            })])
            .into_iter()
            .map(Into::into)
            .collect(),
        };
        let rendered = request.render_transcript();
        assert!(
            rendered.chars().count() < SUMMARY_TOOL_RESULT_MAX_CHARS + 200,
            "a tool result must be truncated into the summary request"
        );
        assert!(rendered.contains("more characters truncated"));
        assert!(
            rendered.len()
                < serde_json::to_vec(&request.retired)
                    .expect("canonical messages serialize")
                    .len(),
            "the rendered transcript must be smaller than the canonical encoding"
        );
    }

    #[test]
    fn issue136_rendered_transcript_explains_cancellation_phase() {
        for (reason, reason_label) in [
            (CancellationReason::UserRequested, "user_requested"),
            (CancellationReason::RuntimeShutdown, "runtime_shutdown"),
        ] {
            for (phase, required) in [
                (
                    ToolCancellationPhase::BeforeStart,
                    "rustX did not start execution",
                ),
                (
                    ToolCancellationPhase::DuringExecution,
                    "Partial side effects may have occurred",
                ),
            ] {
                let request = SummaryRequest {
                    retired: (vec![MessageBlock::Tool(ToolMessageBlock {
                        occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                            crate::runtime::identity::MessageId::new("assistant"),
                            crate::message::types::ContentBlockIndex::new(0),
                        ),
                        id: MessageId::new("tool-cancelled"),
                        tool_call_id: ToolCallId::new("call-cancelled"),
                        tool_id: ToolId::new("tool-cancelled"),
                        result: ToolExecutionResult {
                            deliveries: Vec::new(),
                            status: ToolExecutionStatus::Cancelled { reason, phase },
                            content: Vec::new(),
                            duration_ms: 0,
                            exit_code: None,
                            artifacts: Vec::new(),
                            truncation: None,
                            workflow: None,
                            managed_output: None,
                        },
                    })])
                    .into_iter()
                    .map(Into::into)
                    .collect(),
                };
                let rendered = request.render_transcript();
                assert!(rendered.contains(reason_label));
                assert!(rendered.contains(required));
            }
        }
    }

    /// Issue #202: a proven terminal timeout and an admitted unknown outcome
    /// keep their distinct typed feedback in the summary request — the
    /// unknown outcome never collapses into generic failure wording.
    #[test]
    fn issue202_rendered_transcript_distinguishes_timeout_from_unknown_outcome() {
        for (status, required) in [
            (
                ToolExecutionStatus::TimedOut,
                "established terminal settlement",
            ),
            (
                ToolExecutionStatus::OutcomeUnknown {
                    detail: "the runtime restarted before a durable outcome was committed"
                        .to_owned(),
                },
                "could not establish its final external outcome",
            ),
        ] {
            let request = SummaryRequest {
                retired: (vec![MessageBlock::Tool(ToolMessageBlock {
                    occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                        crate::runtime::identity::MessageId::new("assistant"),
                        crate::message::types::ContentBlockIndex::new(0),
                    ),
                    id: MessageId::new("tool-unknown"),
                    tool_call_id: ToolCallId::new("call-unknown"),
                    tool_id: ToolId::new("tool-unknown"),
                    result: ToolExecutionResult {
                        deliveries: Vec::new(),
                        status,
                        content: Vec::new(),
                        duration_ms: 0,
                        exit_code: None,
                        artifacts: Vec::new(),
                        truncation: None,
                        workflow: None,
                        managed_output: None,
                    },
                })])
                .into_iter()
                .map(Into::into)
                .collect(),
            };
            let rendered = request.render_transcript();
            assert!(
                rendered.contains(required),
                "the summary preserves the typed status distinction: {rendered}"
            );
            assert!(
                !rendered.contains("failed:"),
                "an unknown outcome is not reported as a failure: {rendered}"
            );
        }
    }

    /// Truncation never splits a multi-byte character.
    #[test]
    fn truncation_respects_character_boundaries() {
        let text = "汉".repeat(10);
        let truncated = super::truncate_for_summary(&text, 4);
        assert!(truncated.starts_with("汉汉汉汉\n[..."));
        assert_eq!(super::truncate_for_summary(&text, 10), text);
        assert_eq!(super::truncate_for_summary(&text, 99), text);
    }

    /// The production instruction requires the fixed Pi-style structured
    /// Markdown contract (Issue #140) and no longer advertises free-form
    /// output.
    #[test]
    fn summary_instruction_requires_the_structured_contract() {
        let instruction = ModelBackedSummarizer::instruction();
        for section in [
            "## Goal",
            "## Constraints & Preferences",
            "## Progress",
            "### Done",
            "### In Progress",
            "### Blocked",
            "## Key Decisions",
            "## Next Steps",
            "## Critical Context",
        ] {
            assert!(
                instruction.contains(section),
                "the structured summary contract requires {section}"
            );
        }
        assert!(
            instruction.contains("EXACTLY this Markdown structure"),
            "the structure is a fixed contract, not guidance"
        );
        assert!(
            !instruction.contains("free-form"),
            "free-form output is no longer advertised"
        );
        for exact_detail in [
            "file paths",
            "type and function names",
            "error strings",
            "critical values",
            "user constraints",
        ] {
            assert!(
                instruction.contains(exact_detail),
                "the contract requires preserving {exact_detail}"
            );
        }
    }
}
