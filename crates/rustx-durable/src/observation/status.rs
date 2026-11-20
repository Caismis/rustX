//! The structured Agent Status descriptor of one composition, prepared once at
//! its native source (request commit or status observation).
use super::transcript::RuntimeClientTranscriptCursor;
use rustx_types::context::contribution::ContributionOpportunities;
use rustx_types::context::status::AgentStatus;
use rustx_types::durable::canonical::TranscriptCursor;

use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use rustx_types::context::status::{AgentStatusSectionData, render_agent_status};
use rustx_types::runtime::identity::{AttemptId, MessageId, ToolExecutionId};
use rustx_types::tools::background::BackgroundLifecycle;
use rustx_types::tools::todo::TodoStatus;
use rustx_types::tools::types::ToolProgress;
use serde::{Deserialize, Serialize};

/// The structured Agent Status view of one composition.
///
/// Derived from the exact composed status the model path consumed: the
/// structured sections and the canonical rendered representation originate
/// from the same composition, so a client never parses the rendered text
/// to recover structure and never triggers a second composition.
///
/// The view also carries the runtime facts that place the composition in
/// conversation order: the eligible
/// [`opportunities`](Self::opportunities), each with the durable identity it
/// was established against. Placement is a runtime fact because only the
/// runtime knows it, and it is frozen where it is determined rather than
/// reconstructed downstream; how a client draws a status at that place is
/// presentation and stays entirely outside this type.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct AgentStatusView {
    /// The attempt that composed the status.
    pub attempt_id: AttemptId,
    /// The turn number of the request preparation.
    pub turn: u32,
    /// The canonical Agent Status User message described by this view. It is
    /// the stable identity of the composition: observing the same
    /// `status_message_id` twice describes one composition, never two.
    pub status_message_id: Option<MessageId>,
    pub source_locator: Option<crate::observation::content::CanonicalContentLocator>,
    pub request_id: Option<rustx_types::runtime::identity::RequestId>,
    pub rendered_ref: Option<crate::observation::content::ContentRef>,
    /// The delivery opportunities that made this generation eligible, each
    /// carrying its own placement fact.
    ///
    /// The Agent Status Context message is request-scoped model history: it
    /// carries no transcript cursor of its own and never becomes a transcript
    /// item, so placement has to be published or it cannot be known. Each
    /// opportunity publishes the identity it was established against —
    /// `FreshInbound` the exact inbound message, `PostToolBatch` the durable
    /// position of its settled tool batch — and both were frozen by the
    /// semantic owner at that establishment, not sampled when this
    /// observation was folded.
    pub opportunities: AgentStatusOpportunityView,
    /// The ordered structured sections.
    pub sections: Vec<RuntimeClientStatusSection>,
    /// The canonical rendered representation, derived from the same
    /// composition as the sections.
    ///
    /// It exists for diagnostics and for proving that a client and the model
    /// saw one composition. It is **not** a presentation source: a client
    /// renders [`sections`](Self::sections) and never parses this text back
    /// into structure.
    pub rendered: String,
}
/// One structured Agent Status section of the external view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum RuntimeClientStatusSection {
    /// The typed Time payload.
    Temporal {
        /// The runtime clock value sampled at composition time.
        current_time: DateTime<Utc>,
        /// The Time status timezone, when configured.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[schemars(with = "Option<String>")]
        timezone: Option<Tz>,
    },
    /// The runtime-owned background-execution section.
    BackgroundExecutions {
        /// The active background executions in allocation order.
        executions: Vec<RuntimeClientStatusBackgroundExecution>,
        /// Active executions omitted by the module-local bound.
        omitted_count: usize,
    },
    /// The bounded conversation-owned Todo reminder.
    Todo {
        /// The first committed in-progress task, when any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        current: Option<RuntimeClientTodoStatusTask>,
        /// Remaining committed active tasks in creation order.
        #[serde(default)]
        tasks: Vec<RuntimeClientTodoStatusTask>,
        /// Number of committed active tasks.
        active_count: usize,
        /// Number of active tasks blocked by active dependencies.
        blocked_count: usize,
        /// Number of committed completed tasks.
        completed_count: usize,
        /// Number of committed deleted tasks.
        deleted_count: usize,
        /// Number of active tasks omitted from the bounded view.
        omitted_count: usize,
    },
}
/// Exactly the bounded Background presentation accepted by the Status owner.
/// Job invocation bodies and terminal results remain in the native Job read domain.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientStatusBackgroundExecution {
    pub job_id: ToolExecutionId,
    pub tool_name: String,
    pub state: BackgroundLifecycle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<ToolProgress>,
}
/// One bounded Todo task in the Agent Status client view.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct RuntimeClientTodoStatusTask {
    /// The conversation-owned task id.
    pub id: u64,
    /// The bounded task subject.
    pub subject: String,
    /// The bounded in-progress label, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active_form: Option<String>,
    /// The committed lifecycle status.
    pub status: TodoStatus,
    /// Whether an active dependency still blocks this task.
    pub blocked: bool,
}
/// The external view of the Agent Status opportunity set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct AgentStatusOpportunityView {
    /// The `FreshInbound` opportunity that produced this status, when one is
    /// present. Future delivery opportunities can be added alongside it
    /// without making this member structurally mandatory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fresh_inbound: Option<FreshInboundOpportunityView>,
    /// The complete settled tool batch that made this existing primary step
    /// eligible, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub post_tool_batch: Option<PostToolBatchOpportunityView>,
}
/// The external view of one `FreshInbound` status opportunity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct FreshInboundOpportunityView {
    /// The inbound message that made status generation eligible.
    pub target_message_id: Option<MessageId>,
    pub target_locator: Option<crate::observation::content::CanonicalContentLocator>,
    pub transcript_anchor: Option<RuntimeClientTranscriptCursor>,
    pub target_metadata_ref: Option<crate::observation::content::ContentRef>,
}
/// The external view of one `PostToolBatch` status opportunity.
///
/// The opportunity itself remains a marker with no durable or scheduling
/// metadata. What it carries here is the one ordering fact that places a
/// composition made from it: the durable transcript position of the canonical
/// `ToolResult` batch that established it, frozen by the Agent Loop at that
/// batch's commit.
///
/// The freeze point matters and is the whole reason this is a published fact
/// rather than something a client or the projection reconstructs. A status is
/// composed at the primary-step preparation that consumes this opportunity,
/// but it is not observed until the durable model-turn-start commit lands,
/// and inbound acceptance is an independent durable boundary that may commit
/// in between. Anything that read "the newest durable position" at fold time
/// would place the status after an unrelated inbound turn.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct PostToolBatchOpportunityView {
    /// The durable position of the settled `ToolResult` batch this
    /// opportunity belongs to.
    ///
    /// `None` only when that batch committed no visible transcript item, in
    /// which case the composition carries no transcript-position placement
    /// and a client draws no annotation for it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transcript_anchor: Option<RuntimeClientTranscriptCursor>,
}

/// Builds the structured external status view from the exact composed
/// status observation; the rendered representation derives from the same
/// composition through the one canonical renderer.
pub fn status_view(observation: &AgentStatusObservation) -> AgentStatusView {
    prepared_status_view(observation, None)
}
/// Prepare finite display metadata before entering the projection synchronization boundary.
pub fn prepared_status_view(
    observation: &AgentStatusObservation,
    source: Option<&crate::StatusObservationSource>,
) -> AgentStatusView {
    prepared_status(
        &observation.attempt_id,
        observation.turn,
        &observation.status_message_id,
        &observation.opportunities,
        observation.post_tool_batch_anchor,
        &observation.status,
        source,
    )
}
pub fn prepared_status(
    attempt_id: &rustx_types::runtime::identity::AttemptId,
    turn: u32,
    status_message_id: &rustx_types::runtime::identity::MessageId,
    opportunities: &rustx_types::context::contribution::ContributionOpportunities,
    post_tool_batch_anchor: Option<crate::TranscriptCursor>,
    status: &rustx_types::context::status::AgentStatus,
    source: Option<&crate::StatusObservationSource>,
) -> AgentStatusView {
    let sections = status
        .sections
        .iter()
        .map(|section| match &section.data {
            AgentStatusSectionData::Temporal {
                current_time,
                timezone,
            } => RuntimeClientStatusSection::Temporal {
                current_time: *current_time,
                timezone: *timezone,
            },
            AgentStatusSectionData::BackgroundExecution {
                executions,
                omitted_count,
            } => RuntimeClientStatusSection::BackgroundExecutions {
                executions: executions
                    .iter()
                    .map(
                        |execution| super::status::RuntimeClientStatusBackgroundExecution {
                            job_id: execution.execution_id.clone(),
                            tool_name: execution.tool_name.to_string(),
                            state: execution.state,
                            progress: execution.progress.clone(),
                        },
                    )
                    .collect(),
                omitted_count: *omitted_count,
            },
            AgentStatusSectionData::Todo { presentation } => RuntimeClientStatusSection::Todo {
                current: presentation.current.as_ref().map(todo_status_task_view),
                tasks: presentation
                    .tasks
                    .iter()
                    .map(todo_status_task_view)
                    .collect(),
                active_count: presentation.active_count,
                blocked_count: presentation.blocked_count,
                completed_count: presentation.completed_count,
                deleted_count: presentation.deleted_count,
                omitted_count: presentation.omitted_count,
            },
        })
        .collect();
    let fresh_inbound =
        opportunities
            .fresh_inbound
            .as_ref()
            .map(|fresh| FreshInboundOpportunityView {
                target_message_id: (fresh.target_message_id.as_str().len() <= 256)
                    .then(|| fresh.target_message_id.clone()),
                target_locator: source.and_then(|source| source.target_locator.clone()),
                transcript_anchor: source
                    .and_then(|source| source.target_transcript)
                    .map(Into::into),
                target_metadata_ref: source.and_then(|source| source.target_metadata_ref.clone()),
            });
    // The placement fact of a `PostToolBatch` composition is carried by the
    // observation, frozen by the Agent Loop at the tool batch's own commit.
    // It is copied, never derived.
    let post_tool_batch = opportunities
        .post_tool_batch
        .map(|_| PostToolBatchOpportunityView {
            transcript_anchor: post_tool_batch_anchor.map(RuntimeClientTranscriptCursor::from),
        });
    AgentStatusView {
        attempt_id: attempt_id.clone(),
        turn,
        status_message_id: (status_message_id.as_str().len() <= 256)
            .then(|| status_message_id.clone()),
        source_locator: source.map(|source| source.locator.clone()),
        request_id: source.map(|source| source.request_id.clone()),
        rendered_ref: source.and_then(|source| source.rendered_ref.clone()),
        opportunities: AgentStatusOpportunityView {
            fresh_inbound,
            post_tool_batch,
        },
        rendered: render_agent_status(status),
        sections,
    }
}
fn todo_status_task_view(
    task: &rustx_types::tools::todo::TodoStatusTask,
) -> RuntimeClientTodoStatusTask {
    RuntimeClientTodoStatusTask {
        id: task.id,
        subject: task.subject.clone(),
        active_form: task.active_form.clone(),
        status: task.status,
        blocked: task.blocked,
    }
}

/// arbitration and after that message's `observe_committed` callback.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentStatusObservation {
    /// The attempt that composed the status.
    pub attempt_id: AttemptId,
    /// The turn number of the model request being prepared.
    pub turn: u32,
    /// The canonical Agent Status User message described by this observation.
    pub status_message_id: rustx_identity::MessageId,
    /// The opportunity set that made this generation eligible.
    pub opportunities: ContributionOpportunities,
    /// The durable transcript position frozen when the `PostToolBatch`
    /// opportunity in [`opportunities`](Self::opportunities) was established:
    /// the canonical `ToolResult` batch's own position, read once at that
    /// commit and never re-read afterwards.
    ///
    /// This is the placement fact of a `PostToolBatch` composition, and it is
    /// deliberately frozen by the semantic owner rather than derived later.
    /// Inbound acceptance is an independent durable boundary that may commit
    /// between this freeze and `observe_status`; a consumer that instead read
    /// "the newest durable position" when it folded this observation would
    /// place the status after an unrelated inbound turn it has nothing to do
    /// with.
    ///
    /// `None` when this composition had no `PostToolBatch` opportunity, or
    /// when the batch that established one committed no visible transcript
    /// item. A `FreshInbound` composition is placed by the exact inbound
    /// message identity its opportunity carries, which is a stronger fact
    /// than a position, so it needs nothing here.
    pub post_tool_batch_anchor: Option<TranscriptCursor>,
    /// The one composed structured status.
    pub status: AgentStatus,
}
