//! The Runtime Client host: the projection + control + attachment adapter
//! over the conversation runtime coordinator (Issue #61).
//!
//! [`RuntimeClientHost`] is the native projection/control owner reused by App Server. It
//! observes and controls the
//! [`ConversationRuntime`](crate::runtime::conversation_runtime::ConversationRuntime)
//! of the same conversation; it does **not** own the conversation runtime:
//!
//! ```text
//! ConversationRuntime semantic facts/observations
//!         |
//!         v
//! RuntimeClientProjection (snapshot / cursor / replay / subscribers)
//!         |
//!         v
//! RuntimeClientHost (attachment / protocol control adapter)
//!         |
//!         v
//! +--> App Server native host/attachment operations -> stdio / WebSocket -> clients
//! |
//! +--> RuntimeClientEndpoint -> child-owned read-only inspection socket
//! ```
//!
//! `RuntimeClientEndpoint` is the typed internal request endpoint and child
//! inspection protocol endpoint. The production Runtime Client JSONL consumer
//! is the child-owned read-only inspection socket. TUI uses App Server.
//! The host does not forward Session intent.
//!
//! The host owns:
//!
//! - one control attachment plus any number of explicitly read-only
//!   observation attachments;
//! - the Runtime Client projection (snapshot read model, cursor allocation,
//!   bounded replay, subscribers) and its linearization boundary;
//! - the root publication-admission check for child interactions, limited to
//!   the lifetime of the bound runtime projection, independently of clients;
//! - protocol adaptation: request dispatch, `model_set`/`shutdown`/
//!   `cancel_current_attempt` forwarding and inbound publish forwarding;
//! - transport-independent client subscriptions.
//!
//! The host does **not** own:
//!
//! - canonical conversation state (the coordinator owns `ConversationState`
//!   between attempts);
//! - session model authority (the coordinator freezes attempt snapshots at
//!   admission);
//! - attempt admission (the coordinator is the one admission owner);
//! - mailbox semantic sequencing (the coordinator owns the
//!   mailbox/admission relationship);
//! - `ConversationToolRuntime` / `CapabilityCoordinator` semantic ownership;
//! - child interaction waiters, pending state, audit, cancellation,
//!   settlement, or execution authority;
//! - SessionCatalog/SessionGraph ownership (`SessionController` owns it and
//!   App Server addresses it directly, never through this host);
//! - cancellation terminal settlement (`AgentExecution` remains the attempt
//!   execution/terminal authority);
//! - background/subagent lifecycle.
//!
//! # Observation handoff
//!
//! Native owners deliver complete same-commit receipts and immutable actor
//! sources through the installed CPU publication sink. The runtime-owned
//! observation fan-out retains
//! [`PendingObservations`](crate::runtime::observation::PendingObservations)
//! for indexed journal readiness and finite disposable activity. Its bootstrap
//! handshake runs over an inert runtime and captures every subsystem seam at
//! one native cut. The initial seed publishes nothing and allocates no cursor.
//! Ancillary Session observers read finite aggregate watches and retain no
//! independent body queue or history authority.
//!
//! The worker prepares indexed source evidence outside the Projection lock.
//! Public checkpoint, domain, page, value and content reads capture the already
//! published seed and perform bounded source reads after releasing that lock.
//! They never drain queued observations or publish a cursor. Attachment and
//! registration bookkeeping also acquires only the raw Projection lock.
//! A checkpoint at C and replay after C share this single cursor authority.
//!
//! # The lock-order graph
//!
//! ```text
//! App Server route / registration -> attachment -> Projection (bookkeeping)
//! Source preparation -> Projection (native fold and cursor publication)
//! Native subsystem -> observation fan-out -> pending queue
//! Staged publication -> fan-out enqueue, unlock -> source preparation -> Projection
//! ```
//!
//! Source preparation never acquires an App Server route, registration, or
//! attachment lock. Registration operations never acquire source preparation
//! or perform Store I/O. The publication callback runs after durable staging
//! and after releasing fan-out dispatch locks; it commits the finite native
//! display transition before returning to the existing User Release owner.
//! Physical readiness and Flush remain outside all these boundaries.
//!
//! # Lifetime
//!
//! The host retains the runtime for control and seed reads inside the resident
//! composition. External attachment/endpoint/subscription handles are weak;
//! the runtime also holds only weak references through its installed observer
//! seams. Releasing the last resident host handle closes the
//! primary observation queue (the projection worker's terminal condition);
//! releasing the last runtime handle closes the queue and the admission
//! wake gate. A detached or absent Runtime Client never stops the
//! conversation: admission, execution, settlement, and canonical state all
//! belong to the coordinator and run identically with zero attachments.
//!
//! # Detach is not cancellation
//!
//! Detaching an attachment changes only attachment state. It never
//! cancels, settles, or mutates semantic runtime work: the current
//! attempt, conversation-owned background executions, mailbox contents,
//! canonical conversation state, and capability state are untouched.

mod durable_bootstrap;

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
#[cfg_attr(not(test), allow(unused_imports))]
use std::sync::{Arc, Mutex, MutexGuard, Weak};

use super::projection::{RuntimeClientProjection, SubscriberPoll, background_view};
use super::snapshot::{RuntimeClientTranscriptCursor, RuntimeClientTranscriptPage};
use super::types::{
    AttachmentId, RUNTIME_CLIENT_PROTOCOL_VERSION, RuntimeClientCursor, RuntimeClientError,
    RuntimeClientProtocolEvent, RuntimeClientResult,
};
use crate::durable::{ConversationStore, TRANSCRIPT_PAGE_LIMIT_MAX};
use crate::model::session::SessionModelConfig;
use crate::model::{ModelRequest, RequestIdentity};
use crate::runtime::conversation_runtime::{
    CancelAttemptError, ConversationRuntime, InboundAdmissionError, ManualCompactionError,
    ModelUpdateError, RuntimeBootstrapError,
};
use crate::runtime::identity::{ConversationId, MessageId, ToolExecutionId};
use crate::runtime::interaction::{InteractionRef, InteractionResponse, RoutedInteractionError};
use crate::runtime::observation::{ConversationObservation, PendingObservations};
use crate::runtime::request_history::{RequestHistory, RequestHistoryError};
use crate::runtime::subagent::InteractionPublicationAuthority;

/// The one Runtime Client host construction failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostConstructionError {
    /// The conversation runtime identity is already bound to a Runtime
    /// Client host.
    ///
    /// The Runtime Client protocol binds one runtime identity to at most one
    /// [`RuntimeClientHost`] for that identity's lifetime, so cloning a
    /// runtime never yields a second bindable identity and dropping the
    /// bound host never makes it bindable again. Reconnect replaces the
    /// attachment, not the host.
    RuntimeClientAlreadyBound {
        /// The conversation whose runtime identity is already bound.
        conversation_id: ConversationId,
    },
    /// An observation bridge is already installed over the conversation
    /// runtime (a previous headless observation consumer), so the host
    /// cannot establish its own projection handshake.
    ///
    /// Unreachable through the production composition path (the binding
    /// claim gates it); reported typed so a failed construction releases
    /// the binding claim instead of leaving a claimed-but-broken runtime.
    ObservationBridgeAlreadyInstalled {
        /// The conversation whose runtime already has a bridge.
        conversation_id: ConversationId,
    },
    /// The conversation runtime was already activated.
    ///
    /// Binding a Runtime Client host is a **pre-activation** composition
    /// decision (Issue #61). A host binds while the runtime is inert, so
    /// its finite seed captures the runtime's real state at the activation
    /// cut; there is no supported hot installation of a first host over a
    /// runtime that has already begun semantic execution.
    RuntimeAlreadyActivated {
        /// The conversation whose runtime is already activated.
        conversation_id: ConversationId,
    },
    /// The native durable authority could not provide a coherent bootstrap
    /// seed for the client projection.
    Durable(String),
}

impl core::fmt::Display for HostConstructionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::RuntimeClientAlreadyBound { conversation_id } => write!(
                f,
                "the runtime identity of conversation {conversation_id} is already bound to a Runtime Client host"
            ),
            Self::ObservationBridgeAlreadyInstalled { conversation_id } => write!(
                f,
                "the conversation runtime of {conversation_id} already has an observation bridge installed"
            ),
            Self::RuntimeAlreadyActivated { conversation_id } => write!(
                f,
                "the conversation runtime of {conversation_id} is already activated; a Runtime Client host binds before activation"
            ),
            Self::Durable(message) => write!(f, "durable conversation bootstrap failed: {message}"),
        }
    }
}

impl std::error::Error for HostConstructionError {}

/// The host-owned attachment state.
pub(crate) struct AttachmentState {
    /// The attachment identity.
    attachment_id: AttachmentId,
    /// The registered subscriber of the attachment, when it subscribed.
    subscriber_id: Option<u64>,
}

/// The one synchronized host state (the projection linearization owner).
pub(crate) struct ClientState {
    /// The Runtime Client projection: snapshot read model, cursor,
    /// bounded replay, subscribers.
    projection: RuntimeClientProjection,
    /// The one control-capable Runtime Client attachment. Read-only
    /// inspection attachments are kept separately so observation subscribers
    /// do not compete with execution/control ownership.
    control_attachment: Option<AttachmentState>,
    /// Read-only Runtime Client inspection attachments. They share the one
    /// projection and replay ring, but never acquire control authority.
    read_only_attachments: BTreeMap<AttachmentId, AttachmentState>,
    /// The next attachment identity sequence.
    next_attachment_seq: u64,
}

#[cfg(test)]
impl ClientInner {
    /// Test-only: waits until the projection has nothing left to publish
    /// without new input — every observation folded, none awaiting its
    /// source release, and the read domains installed. Public reads remain
    /// pure; this barrier waits for the existing worker's publication before
    /// the test chooses its baseline. Join preparation and publication in
    /// their existing lock order before checking raw state: an empty queue
    /// alone does not prove its drained observation has been installed.
    pub(crate) async fn projection_settled(&self) {
        let started = std::time::Instant::now();
        *self.publication_gate.settlement_wait.lock().unwrap() = None;
        let mut turns = self.worker_turns.subscribe();
        loop {
            let worker_turn = *turns.borrow_and_update();
            {
                let _preparation = self
                    .publication_gate
                    .settlement_lock(&self.observation_preparation);
                let _publication = self
                    .publication_gate
                    .settlement_lock(&self.publication_gate.order);
                let guard = self
                    .state
                    .lock()
                    .expect("runtime client host lock poisoned");
                let pending_settled = self.pending.settled();
                let read_cut = guard.projection.read_domain_cut();
                let source_status = match guard.projection.validate_source_prefix() {
                    Ok(()) => "ready",
                    Err(RuntimeClientError::ReadCutExpired) => "pending",
                    Err(RuntimeClientError::ObservationUnavailable { .. }) => "unavailable",
                    Err(_) => "other error",
                };
                if pending_settled && read_cut.is_none() && source_status == "ready" {
                    return;
                }
                *self.publication_gate.settlement_wait.lock().unwrap() = Some(SettlementWait {
                    checked: std::time::Instant::now(),
                    elapsed: started.elapsed(),
                    worker_turn,
                    pending_settled,
                    read_cut,
                    source_status,
                    queue: self.pending.source_progress_debug(),
                });
            }
            turns
                .changed()
                .await
                .expect("the host owns the worker turn signal");
        }
    }

    /// Test-only: parks the projection worker so the test owns the fold
    /// schedule.
    ///
    /// The worker is spawned when the host is constructed and is woken by
    /// every enqueue, so without this a test that wants to look at an
    /// intermediate cut is racing it: the queue can be drained before the
    /// test reads it. Parking is ordered against every concurrent drain, so
    /// after this returns only synchronous native commits advance the projection.
    pub(crate) fn park_projection_worker(&self) {
        let _preparation = self
            .observation_preparation
            .lock()
            .expect("test source preparation gate");
        let _publication = self
            .publication_gate
            .order
            .lock()
            .expect("test publication gate");
        self.pending.park();
    }

    pub(crate) fn queued_observation_records(&self) -> usize {
        self.pending.queued()
    }
    #[cfg(test)]
    pub(crate) fn queued_lifecycle_records(&self) -> usize {
        self.pending.queued_lifecycle_records()
    }

    pub(crate) fn unpark_projection_worker(&self) {
        self.pending.unpark();
    }
}

/// No Store operation occurs under the native Projection synchronization
/// boundary. Preparation serializes drain order; every prepared observation
/// still folds and publishes only under the one Projection lock.
fn source_value<T>(value: T) -> Arc<super::domain::SourceValue<T>> {
    Arc::new(super::domain::SourceValue::new(Arc::new(value)))
}
fn prepare_display_sources(
    observation: &mut ConversationObservation,
    model_capabilities: &mut Option<crate::model::catalog::ModelCapabilities>,
    capability_source: &mut Option<super::capability_source::CapabilityReadSource>,
) {
    match observation {
        ConversationObservation::Published { observation, .. }
        | ConversationObservation::CollectionProof { observation, .. }
        | ConversationObservation::ToolOccurrenceProof { observation, .. } => {
            prepare_display_sources(observation, model_capabilities, capability_source);
        }
        ConversationObservation::ObservationBatch(values)
        | ConversationObservation::JournalBatch {
            observations: values,
            ..
        } => {
            for value in values {
                prepare_display_sources(value, model_capabilities, capability_source);
            }
        }
        ConversationObservation::Capability { source } => {
            *source = source.with_model(model_capabilities.clone());
            source.prepare();
            *capability_source = Some(source.clone());
        }
        ConversationObservation::Resources {
            model,
            capability,
            display,
            ..
        } => {
            model.prepare();
            *model_capabilities = model.capabilities().cloned();
            capability.prepare();
            *capability_source = Some(capability.clone());
            display.prepare();
        }
        ConversationObservation::SessionModelChanged { model, capability } => {
            model.prepare();
            *model_capabilities = model.capabilities().cloned();
            *capability = capability_source
                .as_ref()
                .map(|source| source.with_model(model_capabilities.clone()));
            if let Some(source) = capability.as_ref() {
                source.prepare();
            }
            capability_source.clone_from(capability);
        }
        ConversationObservation::AttemptAdmitted { model, .. } => model.prepare(),
        ConversationObservation::Workflow(cuts) => {
            for source in cuts {
                source.prepare();
            }
        }
        _ => {}
    }
}
/// These callbacks carry complete immutable owner receipts and no J debt.
/// Registry callbacks hold their native owner lock: this selection must never
/// invoke Registry reads or source preparation that reacquires that owner.
fn native_actor_transition(observation: &ConversationObservation) -> bool {
    match observation {
        ConversationObservation::Published { observation, .. } => {
            native_actor_transition(observation)
        }
        ConversationObservation::AttemptAdmitted { .. }
        | ConversationObservation::CanonicalCommitted { .. }
        | ConversationObservation::SessionModelChanged { .. }
        | ConversationObservation::Capability { .. }
        | ConversationObservation::Resources { .. }
        | ConversationObservation::GoalChanged(_)
        | ConversationObservation::Workflow(_)
        | ConversationObservation::PendingHead { .. }
        | ConversationObservation::InteractionPending { .. }
        | ConversationObservation::InteractionSettled { .. } => true,
        #[cfg(test)]
        ConversationObservation::InteractionRemoved { .. } => true,
        ConversationObservation::CollectionProof {
            owner, observation, ..
        } => {
            matches!(
                (*owner, observation.as_ref()),
                ("agents", ConversationObservation::SubagentLifecycle { .. })
                    | ("jobs", ConversationObservation::Background(_))
            )
        }
        ConversationObservation::ObservationBatch(items) => {
            !items.is_empty() && items.iter().all(native_actor_transition)
        }
        // Durable transitions are discharged by their indexed source proof,
        // and disposable progress/activity remains in the existing finite lanes.
        _ => false,
    }
}

fn weighted_sources(
    sources: Vec<(String, Arc<super::domain::DomainSource>)>,
) -> Vec<(String, Arc<super::domain::DomainSource>, usize)> {
    sources
        .into_iter()
        .map(|(key, source)| {
            let weight = source.encoded_weight();
            (key, source, weight)
        })
        .collect()
}
fn observation_domain_sources(
    observation: &ConversationObservation,
) -> Vec<(String, Arc<super::domain::DomainSource>)> {
    use super::domain::DomainSource;
    let pair = |key: String, source| (key, Arc::new(source));
    match observation {
        ConversationObservation::Published { observation, .. }
        | ConversationObservation::CollectionProof { observation, .. }
        | ConversationObservation::ToolOccurrenceProof { observation, .. } => {
            observation_domain_sources(observation)
        }
        ConversationObservation::ObservationBatch(observations)
        | ConversationObservation::JournalBatch { observations, .. } => observations
            .iter()
            .flat_map(observation_domain_sources)
            .collect(),
        ConversationObservation::GoalChanged(view) => vec![pair(
            "goal".into(),
            DomainSource::Goal(source_value(view.clone())),
        )],
        ConversationObservation::AttemptAdmitted { model, .. } => vec![pair(
            "attempt.model".into(),
            DomainSource::AttemptModel(source_value(model.clone())),
        )],
        ConversationObservation::SessionModelChanged { model, capability } => {
            let mut sources = vec![pair(
                "model".into(),
                DomainSource::Model(source_value(model.clone())),
            )];
            if let Some(capability) = capability {
                sources.push(pair(
                    "capabilities".into(),
                    DomainSource::Capability(source_value(capability.clone())),
                ));
            }
            sources
        }
        ConversationObservation::Capability { source } => vec![pair(
            "capabilities".into(),
            DomainSource::Capability(source_value(source.clone())),
        )],
        ConversationObservation::Resources {
            model,
            capability,
            display,
            ..
        } => vec![
            pair(
                "resources".into(),
                DomainSource::Resources(source_value(display.clone())),
            ),
            pair(
                "model".into(),
                DomainSource::Model(source_value(model.clone())),
            ),
            pair(
                "capabilities".into(),
                DomainSource::Capability(source_value(capability.clone())),
            ),
            pair(
                "model_catalog".into(),
                DomainSource::Catalog(source_value(model.catalog_source())),
            ),
        ],
        ConversationObservation::Background(job) => {
            let view = background_view(job);
            vec![pair(
                format!("jobs.{}", view.job_id),
                DomainSource::Job(source_value(view)),
            )]
        }
        ConversationObservation::SubagentLifecycle {
            agent: Some(agent),
            snapshot,
        } => {
            let view = super::projection::agent_view(agent, snapshot);
            vec![pair(
                format!("agents.{}", view.agent_id),
                DomainSource::Agent(source_value(view)),
            )]
        }
        _ => Vec::new(),
    }
}

fn activity_domain_sources(
    state: &Mutex<ClientState>,
    observation: &ConversationObservation,
    prepared: &[(String, Arc<super::domain::DomainSource>)],
) -> Vec<(String, Arc<super::domain::DomainSource>)> {
    match observation {
        ConversationObservation::Published { observation, .. }
        | ConversationObservation::CollectionProof { observation, .. }
        | ConversationObservation::ToolOccurrenceProof { observation, .. } => {
            activity_domain_sources(state, observation, prepared)
        }
        ConversationObservation::ObservationBatch(observations)
        | ConversationObservation::JournalBatch { observations, .. } => observations
            .iter()
            .flat_map(|observation| activity_domain_sources(state, observation, prepared))
            .collect(),
        ConversationObservation::SubagentActivity(snapshot) => {
            let key = format!("agents.{}", snapshot.child_agent_id);
            let existing = prepared
                .iter()
                .rev()
                .find(|(candidate, _)| candidate == &key)
                .map(|(_, source)| source.clone())
                .or_else(|| {
                    state
                        .lock()
                        .expect("native projection lock")
                        .projection
                        .domain_source(&key, 0)
                });
            let Some(existing) = existing else {
                return Vec::new();
            };
            let super::domain::DomainSource::Agent(agent) = existing.as_ref() else {
                return Vec::new();
            };
            let agent = agent.value();
            if agent.activation_id != snapshot.subagent_id {
                return Vec::new();
            }
            let view = super::projection::activation_view(
                snapshot,
                agent.state,
                agent.current_activation.clone(),
                &agent.title,
            );
            vec![(
                key,
                Arc::new(super::domain::DomainSource::Agent(source_value(view))),
            )]
        }
        _ => Vec::new(),
    }
}

enum TranscriptFieldSource {
    Live(u64),
    Inherited(u64),
}
fn transcript_native_field(domain: &str) -> Option<(u64, &str)> {
    let tail = domain.strip_prefix("transcript.entries.")?;
    let (cursor, field) = tail.split_once('.')?;
    let cursor = cursor.parse().ok()?;
    matches!(field, "completed_response.models" | "turn_process.failure").then_some((cursor, field))
}

#[cfg(test)]
fn observation_terminal_positions(
    store: &dyn ConversationStore,
    observation: &ConversationObservation,
    through: Option<u64>,
) -> Vec<(
    crate::runtime::identity::AttemptId,
    crate::durable::TranscriptCursor,
)> {
    match observation {
        ConversationObservation::Published {
            journal_sequence,
            observation,
        } => observation_terminal_positions(store, observation, Some(*journal_sequence)),
        ConversationObservation::JournalBatch {
            observations,
            through,
        } => observations
            .iter()
            .flat_map(|observation| observation_terminal_positions(store, observation, *through))
            .take(super::checkpoint::CHECKPOINT_PAGE_ITEMS)
            .collect(),
        ConversationObservation::JournalFact(witness)
            if witness.event_kind.starts_with("attempt_")
                && witness.event_kind != "attempt_started" =>
        {
            witness
                .attempt_id
                .as_ref()
                .and_then(|attempt| {
                    store
                        .observation_attempt_summary(attempt, witness.sequence)
                        .ok()
                        .map(|summary| (attempt, summary))
                })
                .and_then(|(attempt, summary)| {
                    summary
                        .terminal_event_id
                        .and_then(|identity| {
                            store.event_transcript_cursor(&identity).ok().flatten()
                        })
                        .map(|cursor| (attempt.clone(), cursor))
                })
                .into_iter()
                .collect()
        }
        #[cfg(test)]
        ConversationObservation::Event { attempt_id, event }
            if super::projection::is_attempt_terminal(event) =>
        {
            through
                .and_then(|through| store.observation_attempt_summary(attempt_id, through).ok())
                .and_then(|summary| summary.terminal_event_id)
                .and_then(|identity| store.event_transcript_cursor(&identity).ok().flatten())
                .map(|cursor| (attempt_id.clone(), cursor))
                .into_iter()
                .collect()
        }
        _ => Vec::new(),
    }
}

fn observation_headers(
    store: &dyn ConversationStore,
    observation: &ConversationObservation,
) -> Vec<(
    MessageId,
    crate::durable::observation::MessageHeader,
    Option<crate::conversation::SurfaceRevision>,
)> {
    let _ = store; // The direct Committed source seam exists only in cfg(test).
    observation
        .facts()
        .flat_map(|fact| match fact {
            ConversationObservation::CanonicalCommitted { receipt, .. } => receipt
                .observation
                .head
                .iter()
                .filter_map(|header| {
                    header.identity.as_ref().map(|identity| {
                        (
                            identity.clone(),
                            header.clone(),
                            Some(receipt.observation.read_cut.surface_revision),
                        )
                    })
                })
                .collect::<Vec<_>>(),
            #[cfg(test)]
            ConversationObservation::Committed { block, .. } => store
                .message_header(block.id())
                .ok()
                .flatten()
                .map(|header| {
                    let revision = store.message_append_revision(block.id()).ok().flatten();
                    (block.id().clone(), header, revision)
                })
                .into_iter()
                .collect::<Vec<_>>(),
            _ => Vec::new(),
        })
        .take(super::checkpoint::CHECKPOINT_PAGE_ITEMS)
        .collect()
}

fn validate_pending_path(path: &[String]) -> Result<(), RuntimeClientError> {
    if path.len() > 32 || path.iter().any(|part| part.len() > 256) {
        Err(RuntimeClientError::InvalidRequest {
            message: "invalid pending source path".into(),
        })
    } else {
        Ok(())
    }
}

fn collection_identity<T: serde::de::DeserializeOwned>(id: &str) -> Result<T, RuntimeClientError> {
    serde_json::from_value(serde_json::Value::String(id.into())).map_err(|_| {
        RuntimeClientError::InvalidRequest {
            message: "invalid native collection identity".into(),
        }
    })
}

fn indexed_collection_read(
    source: &crate::runtime::read_layout::SeekableObservationSource,
    cut: &super::checkpoint::CheckpointReadCut,
    domain: &str,
    item_offset: u64,
    path: &[String],
    range: (u64, u32),
) -> Result<super::checkpoint::CheckpointValueChunk, RuntimeClientError> {
    use base64::Engine;
    let chunk = source
        .read(path, range.0, range.1)
        .map_err(RuntimeClientError::observation_read_error)?;
    Ok(super::checkpoint::CheckpointValueChunk {
        representation:
            crate::runtime_client::checkpoint::CheckpointValueRepresentation::CanonicalJsonV1,
        cut: cut.clone(),
        domain: domain.into(),
        item_offset,
        path: path.to_vec(),
        offset: range.0,
        bytes_base64: base64::engine::general_purpose::STANDARD.encode(chunk.bytes),
        readable_end: chunk.readable_end,
        next_offset: chunk.next_offset,
    })
}

fn direct_collection_read(
    text: &str,
    cut: &super::checkpoint::CheckpointReadCut,
    domain: &str,
    item_offset: u64,
    path: &[String],
    range: (u64, u32),
) -> Result<super::checkpoint::CheckpointValueChunk, RuntimeClientError> {
    use base64::Engine;
    let chunk = crate::runtime::read_layout::text_chunk(text, range.0, range.1)
        .map_err(RuntimeClientError::observation_read_error)?;
    Ok(super::checkpoint::CheckpointValueChunk {
        representation: super::checkpoint::CheckpointValueRepresentation::Utf8Text,
        cut: cut.clone(),
        domain: domain.into(),
        item_offset,
        path: path.to_vec(),
        offset: range.0,
        bytes_base64: base64::engine::general_purpose::STANDARD.encode(chunk.bytes),
        readable_end: chunk.readable_end,
        next_offset: chunk.next_offset,
    })
}

fn collection_page_item<T: serde::Serialize + schemars::JsonSchema>(
    value: &T,
    identity: String,
    offset: u64,
) -> super::checkpoint::CheckpointPageItem {
    let value = super::checkpoint::structural_value(value);
    let deferred = value.is_none();
    super::checkpoint::CheckpointPageItem {
        offset,
        identity,
        value,
        deferred,
        content: Vec::new(),
        key: None,
        body_complete: None,
    }
}

#[derive(Default)]
struct PublicationGate {
    order: Mutex<()>,
    progress: (Mutex<PublicationProgress>, std::sync::Condvar),
    #[cfg(test)]
    encoding_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    preparation_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    settlement_contention_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    settlement_wait: Mutex<Option<SettlementWait>>,
    /// Runs after a checkpoint requested its Source repair read.
    #[cfg(test)]
    repair_request_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    /// Runs when a checkpoint's repair must await an in-flight publication.
    #[cfg(test)]
    repair_settle_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    diagnostics: Mutex<ProjectionDiagnostics>,
}

/// Completed display transitions, and whether one is between its fold and
/// its completion. Readers wait on these, never on the publication order.
#[derive(Default)]
struct PublicationProgress {
    generation: u64,
    publishing: bool,
}

/// One display transition in flight; dropping it completes the transition,
/// also when its fold or encoding unwinds.
struct Publishing<'a>(&'a PublicationGate);

impl Drop for Publishing<'_> {
    fn drop(&mut self) {
        self.0.completed();
    }
}

#[cfg(test)]
#[derive(Default)]
struct ProjectionDiagnostics {
    worker: Option<ProjectionStage>,
    preparation: Option<ProjectionStage>,
    publication: Option<ProjectionStage>,
    read: Option<(ProjectionStage, super::projection::ReadDomainCut)>,
}

#[cfg(test)]
struct ProjectionStage {
    name: &'static str,
    entered: std::time::Instant,
    thread: std::thread::ThreadId,
}

#[cfg(test)]
impl ProjectionStage {
    fn new(name: &'static str) -> Self {
        Self {
            name,
            entered: std::time::Instant::now(),
            thread: std::thread::current().id(),
        }
    }

    fn describe(&self) -> String {
        format!(
            "{}@{:?} age={:?}",
            self.name,
            self.thread,
            self.entered.elapsed()
        )
    }
}

/// Last failed predicate, retained before cancellation can release native gates.
#[cfg(test)]
struct SettlementWait {
    checked: std::time::Instant,
    elapsed: std::time::Duration,
    worker_turn: u64,
    pending_settled: bool,
    read_cut: Option<super::projection::ReadDomainCut>,
    source_status: &'static str,
    queue: String,
}

impl PublicationGate {
    #[cfg(test)]
    fn worker_stage(&self, name: &'static str) {
        self.diagnostics.lock().unwrap().worker = Some(ProjectionStage::new(name));
    }

    #[cfg(test)]
    fn preparation_stage(&self, name: &'static str) {
        self.diagnostics.lock().unwrap().preparation = Some(ProjectionStage::new(name));
    }

    #[cfg(test)]
    fn publication_stage(&self, name: &'static str) {
        self.diagnostics.lock().unwrap().publication = Some(ProjectionStage::new(name));
    }

    #[cfg(test)]
    fn read_stage(&self, name: &'static str, cut: &super::projection::ReadDomainCut) {
        self.diagnostics.lock().unwrap().read = Some((ProjectionStage::new(name), cut.clone()));
    }

    #[cfg(test)]
    fn diagnostic(&self) -> String {
        let Ok(state) = self.diagnostics.try_lock() else {
            return "stage diagnostic busy or poisoned".into();
        };
        let describe =
            |value: &Option<ProjectionStage>| value.as_ref().map(ProjectionStage::describe);
        format!(
            "worker={:?} preparation={:?} publication={:?} physical_read={:?}",
            describe(&state.worker),
            describe(&state.preparation),
            describe(&state.publication),
            state
                .read
                .as_ref()
                .map(|(stage, cut)| (stage.describe(), cut))
        )
    }

    #[cfg(test)]
    fn settlement_lock<'a>(&self, boundary: &'a Mutex<()>) -> MutexGuard<'a, ()> {
        match boundary.try_lock() {
            Ok(guard) => guard,
            Err(std::sync::TryLockError::WouldBlock) => {
                let hook = self.settlement_contention_hook.lock().unwrap().take();
                if let Some(hook) = hook {
                    hook();
                }
                boundary
                    .lock()
                    .expect("test settlement publication boundary")
            }
            Err(std::sync::TryLockError::Poisoned(_)) => {
                panic!("test settlement publication boundary poisoned");
            }
        }
    }

    fn generation(&self) -> u64 {
        self.progress
            .0
            .lock()
            .expect("native publication progress")
            .generation
    }
    /// Marks a transition in flight; the caller holds the publication order.
    fn publishing(&self) -> Publishing<'_> {
        self.progress
            .0
            .lock()
            .expect("native publication progress")
            .publishing = true;
        Publishing(self)
    }
    fn completed(&self) {
        let mut progress = self.progress.0.lock().expect("native publication progress");
        progress.generation = progress.generation.saturating_add(1);
        progress.publishing = false;
        self.progress.1.notify_all();
    }
    /// Waits until no transition is in flight, or the deadline passes
    /// (false). A transition that folded before this call is installed when
    /// it returns true.
    fn wait_installed(&self, deadline: std::time::Instant) -> bool {
        let mut progress = self.progress.0.lock().expect("native publication progress");
        #[cfg(test)]
        if progress.publishing
            && let Some(hook) = self.repair_settle_hook.lock().unwrap().take()
        {
            hook();
        }
        while progress.publishing {
            let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
                return false;
            };
            progress = self
                .progress
                .1
                .wait_timeout(progress, remaining)
                .expect("native publication progress wait")
                .0;
        }
        true
    }
    fn wait_after(&self, generation: u64, deadline: std::time::Instant) {
        let mut current = self.progress.0.lock().expect("native publication progress");
        while current.generation == generation {
            let Some(remaining) = deadline.checked_duration_since(std::time::Instant::now()) else {
                break;
            };
            let (next, elapsed) = self
                .progress
                .1
                .wait_timeout(current, remaining)
                .expect("native publication progress wait");
            current = next;
            if elapsed.timed_out() {
                break;
            }
        }
    }
}

fn model_name_page_item(
    offset: u64,
    identity: String,
    model: &super::snapshot::RuntimeClientModelName,
) -> super::checkpoint::CheckpointPageItem {
    super::checkpoint::CheckpointPageItem {
        body_complete: Some(false),
        key: None,
        offset,
        identity,
        deferred: model.value.is_none(),
        value: super::checkpoint::structural_value(model),
        content: Vec::new(),
    }
}

fn read_domains_installable(
    projection: &RuntimeClientProjection,
    pending: &PendingObservations,
) -> bool {
    !pending.is_closed()
        && pending.can_publish_prepared_reads()
        && !pending.has_unpublished()
        && !projection.snapshot_ref().shutting_down
}

fn close_shutting_down_reader(
    projection: &RuntimeClientProjection,
    store: &std::sync::Weak<dyn ConversationStore>,
) {
    if projection.snapshot_ref().shutting_down
        && let Some(store) = store.upgrade()
    {
        store.close_presentation_reader();
    }
}

/// One complete native display transition. Encoding cannot hold the raw state
/// lock, and a visible cursor is installed only after its frozen bytes exist.
fn commit_display<T>(
    state: &Mutex<ClientState>,
    publication: &PublicationGate,
    fold: impl FnOnce(&mut RuntimeClientProjection) -> T,
) -> T {
    let _publication = publication
        .order
        .lock()
        .expect("native display publication gate");
    // A source failure may resolve a pending reader without creating a cursor.
    // Completing wakes readers after every CPU fold, including that failure
    // path, and only once the fold's publication is installed.
    let _publishing = publication.publishing();
    #[cfg(test)]
    publication.publication_stage("fold");
    let (draft, installed) = {
        let mut state = state.lock().expect("native display capture");
        state.projection.begin_publication();
        let installed = fold(&mut state.projection);
        (state.projection.finish_publication(), installed)
    };
    if let Some(draft) = draft {
        #[cfg(test)]
        publication.publication_stage("encode");
        #[cfg(test)]
        if let Some(hook) = publication.encoding_hook.lock().unwrap().take() {
            hook();
        }
        let encoded = draft.encode();
        #[cfg(test)]
        publication.publication_stage("install");
        let mut state = state.lock().expect("native display install");
        match encoded {
            Ok(encoded) => state.projection.install_publication(encoded),
            Err(error) => state.projection.publication_failed(&error),
        }
        drop(state);
    }
    #[cfg(test)]
    publication.publication_stage("idle");
    installed
}

fn prepared_lock<'a>(
    state: &'a Mutex<ClientState>,
    preparation: &Mutex<()>,
    publication: &PublicationGate,
    pending: &PendingObservations,
    store: &dyn ConversationStore,
    workflows: Option<&crate::runtime::workflow::read_model::WorkflowReadModel>,
) -> MutexGuard<'a, ClientState> {
    let _preparation = preparation.lock().expect("projection preparation mutex");
    #[cfg(test)]
    publication.preparation_stage("drain");
    let observations = pending.drain();
    #[cfg(test)]
    if !observations.is_empty() {
        let hook = publication.preparation_hook.lock().unwrap().take();
        if let Some(hook) = hook {
            hook();
        }
    }
    apply_prepared_observations(state, publication, observations, store, false, true);
    #[cfg(test)]
    publication.preparation_stage("workflow");
    if let Some(workflows) = workflows
        && !pending.has_unpublished()
    {
        let revision = state
            .lock()
            .expect("native Workflow descriptor")
            .projection
            .snapshot_ref()
            .workflows
            .revision;
        let cuts = workflows
            .published_source_cuts_after(revision)
            .into_iter()
            .filter(|cut| cut.revision > revision)
            .map(super::workflow_source::WorkflowReadSource::new)
            .collect::<Vec<_>>();
        if !cuts.is_empty() {
            apply_prepared_observations(
                state,
                publication,
                vec![ConversationObservation::Workflow(cuts)],
                store,
                false,
                true,
            );
        }
    }
    #[cfg(test)]
    publication.preparation_stage("idle");
    state.lock().expect("runtime client host lock poisoned")
}
fn journal_preparation(
    store: &dyn ConversationStore,
    observation: &ConversationObservation,
) -> Vec<(
    u64,
    Option<super::event::RuntimeClientAttemptFailure>,
    Option<super::snapshot::JournalFieldRef>,
)> {
    observation
        .facts()
        .filter_map(|fact| {
            let ConversationObservation::JournalFact(witness) = fact else {
                return None;
            };
            let failure = if witness.event_kind == "attempt_failed" {
                witness
                    .attempt_id
                    .as_ref()
                    .and_then(|attempt| {
                        store
                            .observation_attempt_summary(attempt, witness.sequence)
                            .ok()
                    })
                    .and_then(|summary| summary.failure)
            } else {
                None
            };
            let field = match witness.event_kind.as_str() {
                "tool_execution_completed" => Some("result"),
                "tool_execution_failed" => Some("error"),
                _ => None,
            };
            let source = field.and_then(|field| {
                let path = vec!["event".into(), field.into()];
                store
                    .event_value_read(witness.sequence, witness.sequence, &path, 0, 1)
                    .ok()
                    .map(|chunk| super::snapshot::JournalFieldRef {
                        sequence: witness.sequence,
                        path,
                        encoded_bytes: chunk.readable_end,
                    })
            });
            Some((witness.sequence, failure, source))
        })
        .take(32)
        .collect()
}

fn apply_prepared_observations(
    state: &Mutex<ClientState>,
    publication_gate: &PublicationGate,
    observations: Vec<ConversationObservation>,
    store: &dyn ConversationStore,
    publication: bool,
    complete: bool,
) {
    for mut observation in observations {
        #[cfg(test)]
        publication_gate.preparation_stage("metadata");
        let mut model_capabilities = state
            .lock()
            .expect("source header state")
            .projection
            .current_model_capabilities();
        let mut capability_source = state
            .lock()
            .expect("source header state")
            .projection
            .current_capability_source();
        prepare_display_sources(
            &mut observation,
            &mut model_capabilities,
            &mut capability_source,
        );
        let publication_content = super::checkpoint::publication_content_evidence(&observation);
        let references = super::checkpoint::observation_metadata(store, &observation);
        let headers = observation_headers(store, &observation);
        #[cfg(test)]
        let terminals = observation_terminal_positions(store, &observation, None);
        let journal = journal_preparation(store, &observation);
        let mut sources = observation_domain_sources(&observation);
        sources.extend(activity_domain_sources(state, &observation, &sources));
        let sources = weighted_sources(sources);
        super::checkpoint::bound_publication_payload(&mut observation);
        #[cfg(test)]
        publication_gate.preparation_stage("publish");
        commit_display(state, publication_gate, |projection| {
            projection.prepare_publication_content(publication_content);
            projection.install_content_metadata(references);
            projection.prepare_message_headers(headers);
            #[cfg(test)]
            projection.prepare_terminal_positions(terminals);
            for (sequence, failure, source) in journal {
                projection.prepare_journal_fact(sequence, failure, source);
            }
            projection.publication_commit_scope(publication, complete);
            projection.apply(observation);
            projection.publication_commit_scope(false, complete);
            projection.install_domain_sources(sources);
        });
    }
}

struct NativePublicationProjection {
    inner: std::sync::Weak<ClientInner>,
}
impl NativePublicationProjection {
    fn commit_observation(
        &self,
        observation: ConversationObservation,
        content: Vec<super::checkpoint::PreparedPublicationContent>,
        receipt: Option<&crate::publication::PublicationFrameReceipt>,
    ) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        let complete = true;
        #[cfg(test)]
        let mut hook = matches!(&observation, ConversationObservation::Publication { .. })
            .then(|| inner.publication_commit_hook.lock().unwrap().take())
            .flatten();
        #[cfg(test)]
        if let Some(hook) = &mut hook {
            hook(&inner, std::slice::from_ref(&observation), false);
        }
        #[cfg(test)]
        let evidence = hook.as_ref().map(|_| observation.clone());
        commit_display(&inner.state, &inner.publication_gate, |projection| {
            projection.prepare_publication_content(content);
            if let Some(receipt) = receipt {
                projection.prepare_publication_frontier(receipt);
            }
            projection.publication_commit_scope(true, complete);
            projection.apply(observation);
            projection.publication_commit_scope(false, complete);
        });
        #[cfg(test)]
        if let Some(hook) = &mut hook {
            hook(
                &inner,
                std::slice::from_ref(evidence.as_ref().unwrap()),
                true,
            );
        }
    }
}
impl crate::runtime::observation::PublicationProjectionCommit for NativePublicationProjection {
    fn native_actor(
        &self,
        observation: &ConversationObservation,
    ) -> Option<crate::runtime::observation::NativeReceiptInstallation> {
        if !native_actor_transition(observation) {
            return None;
        }
        let Some(inner) = self.inner.upgrade() else {
            return Some(crate::runtime::observation::NativeReceiptInstallation::Unavailable);
        };
        let (mut model_capabilities, mut capability_source) = {
            let state = inner.state.lock().expect("native actor source descriptors");
            (
                state.projection.current_model_capabilities(),
                state.projection.current_capability_source(),
            )
        };
        let mut observation = observation.clone();
        // Immutable source headers are prepared without a Projection lock,
        // storage reads, or waiting for the asynchronous display worker.
        prepare_display_sources(
            &mut observation,
            &mut model_capabilities,
            &mut capability_source,
        );
        let audit_batches = observation
            .facts()
            .filter_map(|fact| match fact {
                ConversationObservation::InteractionPending {
                    audit: Some((event, _)),
                    ..
                }
                | ConversationObservation::InteractionSettled {
                    audit: Some((event, _)),
                    ..
                } => Some(event.observation.clone()),
                _ => None,
            })
            .collect::<Vec<_>>();
        let sources = weighted_sources(observation_domain_sources(&observation));
        Some(commit_display(
            &inner.state,
            &inner.publication_gate,
            |projection| {
                projection.publication_commit_scope(true, true);
                projection.apply(observation);
                for batch in audit_batches {
                    if let Some(batch) = batch {
                        projection.apply_native_batch(&batch);
                    } else {
                        projection.mark_observation_unavailable(
                            "native Interaction audit source unavailable".into(),
                        );
                    }
                }
                projection.install_domain_sources(sources);
                projection.publication_commit_scope(false, true);
                projection.native_receipt_installation()
            },
        ))
    }
    fn native_owner(
        &self,
        observation: &ConversationObservation,
        batch: Option<&crate::durable::observation::NativeObservationBatch>,
    ) -> crate::runtime::observation::NativeReceiptInstallation {
        let Some(inner) = self.inner.upgrade() else {
            return crate::runtime::observation::NativeReceiptInstallation::Unavailable;
        };
        let sources = weighted_sources(observation_domain_sources(observation));
        let complete = batch.is_some();
        commit_display(&inner.state, &inner.publication_gate, |projection| {
            projection.publication_commit_scope(true, complete);
            projection.apply(observation.clone());
            if let Some(batch) = batch {
                projection.apply_native_batch(batch);
            } else {
                projection.mark_observation_unavailable(
                    "native Registry commit source unavailable".into(),
                );
            }
            projection.install_domain_sources(sources);
            projection.publication_commit_scope(false, complete);
            projection.native_receipt_installation()
        })
    }
    fn native_goal(
        &self,
        receipt: &crate::durable::inbox::GoalObservationReceipt,
    ) -> crate::runtime::observation::NativeReceiptInstallation {
        let Some(inner) = self.inner.upgrade() else {
            return crate::runtime::observation::NativeReceiptInstallation::Unavailable;
        };
        let sources = weighted_sources(vec![(
            "goal".into(),
            Arc::new(super::domain::DomainSource::Goal(source_value(
                receipt.view.clone(),
            ))),
        )]);
        commit_display(&inner.state, &inner.publication_gate, |projection| {
            projection.publication_commit_scope(true, true);
            projection.apply(ConversationObservation::GoalChanged(receipt.view.clone()));
            if let Some(pending) = &receipt.pending {
                projection.apply_pending_receipt(Some(pending));
            }
            if let Some(native) = &receipt.native {
                projection.apply_native_batch(native);
            } else {
                projection.mark_observation_unavailable(
                    "native Goal observation source unavailable".into(),
                );
            }
            projection.install_domain_sources(sources);
            projection.publication_commit_scope(false, receipt.native.is_some());
            projection.native_receipt_installation()
        })
    }
    fn native_audit(
        &self,
        attempt: &crate::runtime::identity::AttemptId,
        receipt: &crate::durable::inbox::PublicationAuditReceipt,
    ) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        commit_display(&inner.state, &inner.publication_gate, |projection| {
            projection.publication_commit_scope(true, true);
            projection.apply(ConversationObservation::AuditSettled {
                attempt_id: attempt.clone(),
                receipt: receipt.clone(),
            });
            projection.publication_commit_scope(false, true);
        });
        #[cfg(test)]
        if let Some(hook) = inner.audit_commit_hook.lock().unwrap().take() {
            hook(&inner);
        }
    }
    fn native_pending(
        &self,
        receipt: Option<&crate::durable::inbox::PendingObservationReceipt>,
        _cut: &crate::durable::reading::ConversationReadCut,
    ) {
        let Some(inner) = self.inner.upgrade() else {
            return;
        };
        commit_display(&inner.state, &inner.publication_gate, |projection| {
            projection.publication_commit_scope(true, true);
            projection.apply_pending_receipt(receipt);
            projection.publication_commit_scope(false, true);
        });
    }
    fn native_manual_compaction(
        &self,
        request: Option<&crate::runtime::identity::ManualCompactionRequestId>,
        batch: Option<&crate::durable::observation::NativeObservationBatch>,
    ) -> crate::runtime::observation::NativeReceiptInstallation {
        let Some(inner) = self.inner.upgrade() else {
            return crate::runtime::observation::NativeReceiptInstallation::Unavailable;
        };
        commit_display(&inner.state, &inner.publication_gate, |projection| {
            projection.apply_manual_compaction(request, batch);
            projection.native_receipt_installation()
        })
    }

    fn native_batch(
        &self,
        batch: &crate::durable::observation::NativeObservationBatch,
    ) -> crate::runtime::observation::NativeReceiptInstallation {
        let Some(inner) = self.inner.upgrade() else {
            return crate::runtime::observation::NativeReceiptInstallation::Unavailable;
        };
        let complete = true;
        commit_display(&inner.state, &inner.publication_gate, |projection| {
            projection.publication_commit_scope(true, complete);
            projection.apply_native_batch(batch);
            projection.publication_commit_scope(false, complete);
            projection.native_receipt_installation()
        })
    }
    fn opened(
        &self,
        attempt: &crate::runtime::identity::AttemptId,
        start: &crate::publication::PublicationStreamStart,
    ) {
        self.commit_observation(
            ConversationObservation::PublicationOpened {
                attempt_id: attempt.clone(),
                start: start.clone(),
            },
            Vec::new(),
            None,
        );
    }
    fn frame(
        &self,
        attempt: &crate::runtime::identity::AttemptId,
        frame: &crate::publication::PublicationFrame,
        receipt: &crate::publication::PublicationFrameReceipt,
    ) {
        if receipt.unavailable {
            if let Some(inner) = self.inner.upgrade() {
                inner
                    .state
                    .lock()
                    .expect("native publication source fence")
                    .projection
                    .mark_observation_unavailable(
                        "native publication display source unavailable".into(),
                    );
            }
            return;
        }
        assert_eq!(
            frame.stream_id, receipt.stream_id,
            "frozen native staging stream"
        );
        assert_eq!(
            frame.sequence, receipt.sequence,
            "frozen native staging sequence"
        );
        let content = receipt
            .content
            .iter()
            .filter_map(|reference| {
                let super::content::ContentSource::LivePublicationPrefix {
                    component,
                    block_index,
                    ..
                } = &reference.source
                else {
                    return None;
                };
                Some(super::checkpoint::PreparedPublicationContent {
                    suffix: receipt.suffix_bytes.is_some()
                        && *component == super::content::PublicationContentComponent::Payload,
                    stream_id: receipt.stream_id.clone(),
                    block_index: *block_index,
                    revision: reference.content_revision,
                    component: *component,
                    representation: reference.representation,
                    base_length: receipt
                        .suffix_bytes
                        .filter(|_| {
                            *component == super::content::PublicationContentComponent::Payload
                        })
                        .map(|bytes| {
                            reference
                                .readable_end
                                .checked_sub(bytes)
                                .expect("frozen staged suffix frontier")
                        }),
                    readable_end: reference.readable_end,
                })
            })
            .collect();
        let frame = super::checkpoint::bounded_publication_frame(frame);
        self.commit_observation(
            ConversationObservation::Publication {
                attempt_id: attempt.clone(),
                frame,
            },
            content,
            Some(receipt),
        );
    }
}

type ProjectionReadResult = (
    super::projection::ReadDomainCut,
    Result<super::projection::ReadDomainValues, crate::durable::ConversationStoreError>,
);
type ProjectionReadSlot = tokio::sync::Mutex<Option<tokio::task::JoinHandle<ProjectionReadResult>>>;

#[derive(Default)]
enum ProjectionWorker {
    #[default]
    Vacant,
    Running(tokio::task::JoinHandle<()>),
    // Retained if a shutdown waiter is cancelled while joining the Source read.
    Joined(Result<(), tokio::task::JoinError>),
}

fn completed_projection_read(
    result: Result<ProjectionReadResult, tokio::task::JoinError>,
    cut: Option<super::projection::ReadDomainCut>,
) -> Option<ProjectionReadResult> {
    match result {
        Ok(value) => Some(value),
        Err(error) => cut.map(|cut| {
            let message = if error.is_panic() {
                "native projection Source task panicked"
            } else {
                "native projection Source task was cancelled"
            };
            (
                cut,
                Err(crate::durable::ConversationStoreError::Storage(
                    message.into(),
                )),
            )
        }),
    }
}

async fn next_projection_read(
    reads: &ProjectionReadSlot,
) -> Result<ProjectionReadResult, tokio::task::JoinError> {
    let mut slot = reads.lock().await;
    let Some(task) = slot.as_mut() else {
        drop(slot);
        return std::future::pending().await;
    };
    let result = task.await;
    slot.take();
    result
}

async fn drain_projection_read(reads: &ProjectionReadSlot) -> Result<(), tokio::task::JoinError> {
    let mut slot = reads.lock().await;
    let result = match slot.as_mut() {
        Some(task) => task.await.map(|_| ()),
        None => Ok(()),
    };
    slot.take();
    result
}

/// The shared Runtime Client host state.
pub(crate) struct ClientInner {
    pub(super) conversation_id: ConversationId,
    agent_id: crate::runtime::identity::AgentId,
    /// The live conversation runtime this host observes and controls. A
    /// durable inspection host deliberately has no runtime owner here.
    runtime: Option<ConversationRuntime>,
    /// The one durable authority used for transcript/bootstrap reads by both
    /// live and read-only hosts.
    store: Arc<dyn ConversationStore>,
    read_store: Arc<dyn ConversationStore>,
    /// Physical child inspection sockets/SQL workers across all root connections.
    pub(super) inspection_capacity: Arc<tokio::sync::Semaphore>,
    /// Whether this host is a read-only attachment to durable conversation
    /// state rather than a control adapter over a live runtime.
    read_only: bool,
    /// The one projection synchronization boundary.
    state: Arc<Mutex<ClientState>>,
    /// Serializes preparation and native folds; metadata I/O happens before
    /// acquiring the Projection lock. Lock order: preparation -> Projection.
    observation_preparation: Arc<Mutex<()>>,
    publication_gate: Arc<PublicationGate>,
    /// The observation queue shared with the conversation runtime (the
    /// projection sink installed at construction).
    pending: Arc<PendingObservations>,
    /// Whether the projection worker task was spawned.
    worker_started: AtomicBool,
    worker: tokio::sync::Mutex<ProjectionWorker>,
    projection_read: Arc<ProjectionReadSlot>,
    #[cfg(test)]
    read_domain_hook: Arc<Mutex<Option<ReadDomainHook>>>,
    #[cfg(test)]
    trace_cut_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    publication_commit_hook: Mutex<Option<PublicationCommitHook>>,
    #[cfg(test)]
    audit_commit_hook: Mutex<Option<AuditCommitHook>>,
    #[cfg(test)]
    snapshot_read_hook: Mutex<Option<Box<dyn FnMut() + Send>>>,
    #[cfg(test)]
    cancellation_receipt_hook: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    #[cfg(test)]
    manual_compaction_response_gate: Mutex<Option<ManualCompactionResponseGate>>,
    /// Test-only: advanced after every projection worker turn.
    #[cfg(test)]
    worker_turns: Arc<tokio::sync::watch::Sender<u64>>,
}

#[cfg(test)]
type ManualCompactionResponseGate = (
    tokio::sync::oneshot::Sender<crate::runtime::conversation_runtime::ManualCompactionOutcome>,
    tokio::sync::oneshot::Receiver<()>,
);

#[cfg(test)]
type ReadDomainHook =
    Box<dyn FnOnce() -> Result<(), crate::durable::ConversationStoreError> + Send>;

#[cfg(test)]
type AuditCommitHook = Box<dyn FnOnce(&ClientInner) + Send>;

#[cfg(test)]
type PublicationCommitHook = Box<dyn FnMut(&ClientInner, &[ConversationObservation], bool) + Send>;

#[cfg(test)]
fn publication_frame(
    observations: &[ConversationObservation],
) -> Option<&crate::publication::PublicationFrame> {
    observations
        .iter()
        .find_map(|observation| match observation {
            ConversationObservation::Publication { frame, .. } => Some(frame),
            ConversationObservation::ObservationBatch(observations)
            | ConversationObservation::JournalBatch { observations, .. } => {
                publication_frame(observations)
            }
            ConversationObservation::Published { observation, .. }
            | ConversationObservation::CollectionProof { observation, .. }
            | ConversationObservation::ToolOccurrenceProof { observation, .. } => {
                publication_frame(std::slice::from_ref(observation.as_ref()))
            }
            _ => None,
        })
}

/// Root-side publication authority installed into the parent subagent
/// registry. It is a weak adapter over runtime projection binding, independent
/// of external attachments, and is not an interaction owner.
struct RootInteractionPublicationAuthority {
    inner: Weak<ClientInner>,
}

impl InteractionPublicationAuthority for RootInteractionPublicationAuthority {
    fn admit(&self, _interaction: &InteractionRef) -> bool {
        self.inner
            .upgrade()
            .is_some_and(|inner| inner.admits_interaction_publication())
    }
}

/// Releasing the last host handle closes the shared observation queue,
/// which is the projection worker's terminal condition.
impl Drop for ClientInner {
    fn drop(&mut self) {
        self.read_store.close_presentation_reader();
        self.pending.close();
    }
}

impl ClientInner {
    #[cfg(test)]
    pub(crate) fn projection_source_debug(&self) -> String {
        let projection = self.state.try_lock().map_or_else(
            |_| "projection state busy or poisoned".into(),
            |state| state.projection.source_prefix_debug(),
        );
        let last_wait = self.publication_gate.settlement_wait.try_lock().ok();
        let wait = last_wait.as_ref().and_then(|guard| guard.as_ref()).map(|wait| {
            format!(
                "checked_ago={:?} elapsed={:?} worker_turn={} pending_settled={} read_cut={:?} source={} queue={}",
                wait.checked.elapsed(), wait.elapsed, wait.worker_turn, wait.pending_settled,
                wait.read_cut, wait.source_status, wait.queue,
            )
        });
        format!(
            "{projection} queue={} last_settlement_wait={wait:?} stages={}",
            self.pending.source_progress_debug(),
            self.publication_gate.diagnostic()
        )
    }

    pub(crate) fn observation_activation_id(&self) -> Option<crate::runtime::identity::SubagentId> {
        self.state
            .lock()
            .expect("native observation scope")
            .projection
            .activation_id()
            .cloned()
    }

    /// Prepares queued evidence for explicit worker/control operations.
    /// Public observation reads use the already published cut without draining.
    pub(crate) fn lock_state(&self) -> MutexGuard<'_, ClientState> {
        prepared_lock(
            &self.state,
            &self.observation_preparation,
            &self.publication_gate,
            &self.pending,
            self.store.as_ref(),
            self.runtime
                .as_ref()
                .map(|runtime| runtime.tool_runtime().workflows()),
        )
    }

    /// Capture published read authority without preparing or publishing work.
    fn lock_published_state(&self) -> MutexGuard<'_, ClientState> {
        self.state.lock().expect("published native read state")
    }

    /// Pending readback is advisory repair at its captured fence. A raced or
    /// unpublished mutation uses the already represented semantic pending view;
    /// candidate acquisition never retries to catch the head.
    #[cfg(test)]
    fn snapshot_candidate(
        &self,
    ) -> Result<super::projection::SnapshotCandidate, RuntimeClientError> {
        let fence = self.lock_state().projection.read_domain_fence();
        let items = if let Some(runtime) = &self.runtime {
            runtime
                .tool_runtime()
                .mailbox()
                .with_pending_snapshot(|items| items)
                .map_err(|error| RuntimeClientError::RuntimeFailure {
                    message: format!("durable pending snapshot failed: {error}"),
                })?
        } else {
            None
        };
        let candidate = {
            let mut state = self.lock_state();
            if !self.pending.has_unpublished()
                && state.projection.read_domain_fence() == fence
                && let Some(items) = items
            {
                state.projection.repair_pending(items);
            }
            state.projection.snapshot_candidate()?
        };
        #[cfg(test)]
        if let Some(hook) = self.snapshot_read_hook.lock().unwrap().as_mut() {
            hook();
        }
        let candidate = candidate.materialize(self.store.as_ref())?;
        // Completion belongs to C, regardless of whether C may still repair live
        // state. Reuse the background installation owner after folding new facts.
        drop(self.lock_state());
        let trace_total = self
            .store
            .observation_trace_total(candidate.trace_through)
            .map_err(RuntimeClientError::observation_store_error)?;
        commit_display(&self.state, &self.publication_gate, |projection| {
            if !self.read_only
                && !self.pending.is_closed()
                && !self.pending.has_unpublished()
                && !projection.snapshot_ref().shutting_down
            {
                projection.install_read_domains(
                    &candidate.read_cut,
                    Ok(super::projection::ReadDomainValues {
                        projection_head: candidate.snapshot.context.projection_head.clone(),
                        transcript: candidate.snapshot.transcript.clone(),
                        occupancy: candidate.snapshot.context.occupancy.clone(),
                        trace: candidate.snapshot.trace.clone(),
                        trace_total,
                    }),
                    true,
                );
            }
        });
        Ok(candidate)
    }

    /// A bound live projection can expose interactions to a future client.
    /// Admission follows runtime binding, never external attachment presence.
    pub(crate) fn admits_interaction_publication(&self) -> bool {
        self.runtime.is_some() && !self.read_only
    }

    /// Claims the one worker when a Tokio runtime is available.
    fn claim_worker(&self) -> Option<tokio::sync::MutexGuard<'_, ProjectionWorker>> {
        let worker_slot = self.worker.try_lock().ok()?;
        if self.pending.is_closed()
            || self.runtime.is_none()
            || tokio::runtime::Handle::try_current().is_err()
            || self.worker_started.swap(true, Ordering::SeqCst)
        {
            return None;
        }
        Some(worker_slot)
    }

    fn workflow_read_model(
        &self,
    ) -> Option<crate::runtime::workflow::read_model::WorkflowReadModel> {
        self.runtime
            .as_ref()
            .map(|runtime| runtime.tool_runtime().workflows().clone())
    }

    /// Advances source-backed Journal and read-domain observations for idle subscribers.
    /// Native owner callbacks publish their finite receipts synchronously; this worker
    /// reads remaining indexed sources outside the Projection lock. Its cancellable
    /// blocking read is joined before shutdown releases physical source ownership.
    #[allow(clippy::too_many_lines)] // Test-only stage probes preserve the production owner sequence.
    pub(crate) fn ensure_worker(self: &Arc<Self>) {
        let Some(mut worker_slot) = self.claim_worker() else {
            return;
        };
        let state = self.state.clone();
        let preparation = self.observation_preparation.clone();
        let publication_gate = self.publication_gate.clone();
        let store = Arc::downgrade(&self.read_store);
        let pending = Arc::clone(&self.pending);
        let workflow_state = self.workflow_read_model();
        let mut workflows = workflow_state
            .as_ref()
            .map(crate::runtime::workflow::read_model::WorkflowReadModel::subscribe);
        #[cfg(test)]
        let read_domain_hook = self.read_domain_hook.clone();
        #[cfg(test)]
        let worker_turns = Arc::clone(&self.worker_turns);
        let reads = self.projection_read.clone();
        let worker = tokio::spawn(async move {
            let mut reading = None;
            loop {
                #[cfg(test)]
                publication_gate.worker_stage("wait");
                tokio::select! {
                    () = pending.wait() => {},
                    () = async {
                        match &mut workflows {
                            Some(receiver) => { let _ = receiver.changed().await; },
                            None => std::future::pending::<()>().await,
                        }
                    } => {},
                    result = next_projection_read(&reads) => {
                        #[cfg(test)]
                        publication_gate.worker_stage("read_completion");
                        if let Some((cut, domains)) = completed_projection_read(result, reading.take()) {
                            let Some(source)=store.upgrade() else { break };
                            drop(prepared_lock(&state,&preparation,&publication_gate,&pending,source.as_ref(),workflow_state.as_ref()));
                            commit_display(&state, &publication_gate, |projection| {
                                if read_domains_installable(projection, &pending) {
                                    projection.install_read_domains(&cut, domains, true);
                                }
                            });
                        }
                    },
                }
                if pending.is_closed() {
                    break;
                }
                #[cfg(test)]
                publication_gate.worker_stage("prepare");
                let cut = {
                    let Some(source) = store.upgrade() else { break };
                    let guard = prepared_lock(
                        &state,
                        &preparation,
                        &publication_gate,
                        &pending,
                        source.as_ref(),
                        workflow_state.as_ref(),
                    );
                    close_shutting_down_reader(&guard.projection, &store);
                    if reading.is_some()
                        || !pending.can_publish_prepared_reads()
                        || pending.has_unpublished()
                        || guard.projection.snapshot_ref().shutting_down
                    {
                        None
                    } else {
                        guard.projection.read_domain_candidate()
                    }
                };
                if reading.is_none()
                    && let Some(cut) = cut
                {
                    #[cfg(test)]
                    publication_gate.worker_stage("spawn_read");
                    let Some(store) = store.upgrade() else {
                        break;
                    };
                    // One bounded read in flight. It owns no host/runtime or
                    // projection lock; semantic delivery continues while it waits.
                    #[cfg(test)]
                    let hook = read_domain_hook.lock().unwrap().take();
                    let mut read_slot = reads.lock().await;
                    reading = Some(cut.clone());
                    #[cfg(test)]
                    let read_diagnostic = publication_gate.clone();
                    *read_slot = Some(tokio::task::spawn_blocking(move || {
                        #[cfg(test)]
                        read_diagnostic.read_stage("hook", &cut);
                        #[cfg(test)]
                        if let Some(hook) = hook
                            && let Err(error) = hook()
                        {
                            read_diagnostic.read_stage("hook_failed", &cut);
                            return (cut, Err(error));
                        }
                        #[cfg(test)]
                        read_diagnostic.read_stage("source", &cut);
                        let domains = cut.read(store.as_ref());
                        #[cfg(test)]
                        read_diagnostic.read_stage("completed", &cut);
                        (cut, domains)
                    }));
                }
                #[cfg(test)]
                worker_turns.send_modify(|turn| *turn += 1);
            }
            if let Some(store) = store.upgrade() {
                store.close_presentation_reader();
            }
            let _ = drain_projection_read(&reads).await;
            // Cancellation is followed by terminal proof, not handle detachment.
            #[cfg(test)]
            pending.signal_worker_exit();
        });
        *worker_slot = ProjectionWorker::Running(worker);
    }

    pub(crate) fn cancel_presentation_reads(&self) {
        self.read_store.close_presentation_reader();
    }

    /// Cancel Store waits/queries, then join semantic delivery and its finite read.
    pub(crate) async fn drain_projection(&self) -> Result<(), tokio::task::JoinError> {
        self.read_store.close_presentation_reader();
        self.pending.close();
        let mut worker_slot = self.worker.lock().await;
        // A cancelled waiter releases this guard, not the native JoinHandle.
        // A retry must still join the original worker and its finite Source read.
        if let ProjectionWorker::Running(worker) = &mut *worker_slot {
            *worker_slot = ProjectionWorker::Joined(worker.await);
        }
        // The read slot outlives worker panic/abort. Neither await removes its
        // original handle until completion, including a cancelled drain waiter.
        let read_result = drain_projection_read(&self.projection_read).await;
        let worker_result = match std::mem::take(&mut *worker_slot) {
            ProjectionWorker::Joined(result) => result,
            ProjectionWorker::Vacant => Ok(()),
            ProjectionWorker::Running(_) => unreachable!("worker was joined in place"),
        };
        worker_result.and(read_result)
    }

    /// Admits one attachment: the internal primitive behind the
    /// `initialize` protocol method.
    ///
    /// The Runtime Client host allows one control attachment. A second
    /// simultaneous control attach fails deterministically and never evicts
    /// the first; explicitly read-only inspection attachments use a separate
    /// subscriber set and may coexist with it. Initialization returns only
    /// the admitted identity and finite capabilities; a checkpoint is read
    /// separately against the current observation-source state.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::UnsupportedProtocolVersion`] for an
    /// unsupported version, [`RuntimeClientError::AttachmentInUse`] when the
    /// control attachment is active, and
    /// [`RuntimeClientError::ProjectionExhausted`] once the observation
    /// stream is over.
    pub(crate) fn attach(
        self: &Arc<Self>,
        protocol_version: u16,
    ) -> Result<(super::attachment::RuntimeAttachment, RuntimeClientResult), RuntimeClientError>
    {
        self.attach_with_mode(protocol_version, false)
    }

    /// Admits one explicitly read-only attachment. Read-only attachments may
    /// coexist with the one control attachment and with one another; they
    /// consume only projection subscription state and never change provider
    /// admission or runtime lifecycle state.
    pub(crate) fn attach_read_only(
        self: &Arc<Self>,
        protocol_version: u16,
    ) -> Result<(super::attachment::RuntimeAttachment, RuntimeClientResult), RuntimeClientError>
    {
        self.attach_with_mode(protocol_version, true)
    }

    fn attach_with_mode(
        self: &Arc<Self>,
        protocol_version: u16,
        read_only_attachment: bool,
    ) -> Result<(super::attachment::RuntimeAttachment, RuntimeClientResult), RuntimeClientError>
    {
        if protocol_version != RUNTIME_CLIENT_PROTOCOL_VERSION {
            return Err(RuntimeClientError::UnsupportedProtocolVersion {
                supported: RUNTIME_CLIENT_PROTOCOL_VERSION,
                requested: protocol_version,
            });
        }
        let attachment = self.admit_attachment_identity(read_only_attachment)?;
        let result = RuntimeClientResult::Initialized {
            attachment_id: attachment.attachment_id().clone(),
            conversation_id: self.conversation_id.clone(),
            agent_id: self.agent_id.clone(),
            activation_id: self
                .state
                .lock()
                .expect("native attachment scope")
                .projection
                .activation_id()
                .cloned(),
            protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION,
            read_only: self.read_only || read_only_attachment,
            capabilities: vec!["checkpoint".into(), "events".into(), "content_read".into()],
        };
        Ok((attachment, result))
    }

    /// Identity admission does no Store reads and materializes no display state.
    /// The Projection lock is the sole admission linearization point. Checkpoints
    /// and subscriptions are independent reads after this finite receipt.
    pub(crate) fn admit_attachment_identity(
        self: &Arc<Self>,
        read_only_attachment: bool,
    ) -> Result<super::attachment::RuntimeAttachment, RuntimeClientError> {
        self.ensure_worker();
        let read_only_attachment = self.read_only || read_only_attachment;
        let mut state = self.state.lock().expect("native projection lock");
        if !read_only_attachment && let Some(existing) = &state.control_attachment {
            return Err(RuntimeClientError::AttachmentInUse {
                existing_attachment_id: existing.attachment_id.clone(),
            });
        }
        let sequence = state
            .next_attachment_seq
            .checked_add(1)
            .ok_or(RuntimeClientError::ProjectionExhausted)?;
        let attachment_id = AttachmentId::new(format!("attachment-{sequence}"));
        let admission = AttachmentState {
            attachment_id: attachment_id.clone(),
            subscriber_id: None,
        };
        state.next_attachment_seq = sequence;
        if read_only_attachment {
            state
                .read_only_attachments
                .insert(attachment_id.clone(), admission);
        } else {
            state.control_attachment = Some(admission);
        }
        drop(state);
        Ok(super::attachment::RuntimeAttachment::new(
            attachment_id,
            self,
            read_only_attachment,
        ))
    }

    #[cfg(test)]
    pub(crate) fn inspection_subscriber_for_test(
        &self,
        attachment_id: &AttachmentId,
    ) -> Option<u64> {
        self.state
            .lock()
            .expect("native projection lock")
            .read_only_attachments
            .get(attachment_id)
            .and_then(|attachment| attachment.subscriber_id)
    }

    /// Releases one attachment. Idempotent: a second detach (or an
    /// attachment drop after an explicit detach) is a no-op. Detach is
    /// never cancellation and never shutdown.
    ///
    /// # Panics
    ///
    /// Panics only if the host lock is poisoned, which would mean a
    /// previous operation panicked while holding the lock.
    pub(crate) fn detach(&self, attachment_id: &AttachmentId) {
        let mut state = self.state.lock().expect("native projection lock");
        let control = state
            .control_attachment
            .as_ref()
            .is_some_and(|attachment| attachment.attachment_id == *attachment_id);
        let attachment = if control {
            state
                .control_attachment
                .take()
                .expect("the control attachment identity was just checked")
        } else {
            let Some(attachment) = state.read_only_attachments.remove(attachment_id) else {
                return;
            };
            attachment
        };
        if let Some(subscriber_id) = attachment.subscriber_id {
            state.projection.remove_subscriber(subscriber_id);
        }
    }

    /// Submits one inbound user message through the conversation runtime's
    /// single publish path.
    ///
    /// The runtime owns authoritative metadata: the message identity, the
    /// inbound sequence, the persisted timestamp, and the provenance are
    /// all runtime-assigned. Success means accepted/published, never
    /// assistant-finished: the runtime wake gate admits the next attempt
    /// when the runtime is idle, and while an attempt is running the
    /// message waits in the authoritative mailbox for the next safe-boundary
    /// drain. The host never admits an attempt itself.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::InvalidRequest`] for empty content,
    /// [`RuntimeClientError::RuntimeShutdown`] after shutdown, and
    /// [`RuntimeClientError::InvalidState`] for a mailbox admission
    /// failure.
    pub(crate) fn submit_inbound(
        &self,
        content: Vec<crate::message::types::UserContentBlock>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        if content
            .iter()
            .any(|b| !matches!(b, crate::message::types::UserContentBlock::Text(_)))
        {
            return Err(RuntimeClientError::InvalidRequest {
                message: "user uploads require server-issued Session receipts".into(),
            });
        }
        self.submit_session_inbound(content)
    }

    pub(crate) fn edit_pending(
        &self,
        expected: &crate::durable::inbox::PendingInboundRef,
        text: &str,
    ) -> Result<crate::durable::inbox::PendingMutationOutcome, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let runtime = self.runtime.as_ref().expect("writable runtime");
        Ok(runtime
            .tool_runtime()
            .mailbox()
            .edit_pending(expected, text)
            .unwrap_or(crate::durable::inbox::PendingMutationOutcome::DurabilityUncertain))
    }

    pub(crate) fn remove_pending(
        &self,
        expected: &crate::durable::inbox::PendingInboundRef,
    ) -> Result<crate::durable::inbox::PendingMutationOutcome, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let runtime = self.runtime.as_ref().expect("writable runtime");
        Ok(runtime
            .tool_runtime()
            .mailbox()
            .remove_pending(expected)
            .unwrap_or(crate::durable::inbox::PendingMutationOutcome::DurabilityUncertain))
    }

    pub(crate) fn submit_session_inbound(
        &self,
        content: Vec<crate::message::types::UserContentBlock>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let runtime = self
            .runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime");
        let admission = runtime
            .submit_inbound(content)
            .map_err(|error| match error {
                InboundAdmissionError::Shutdown => RuntimeClientError::RuntimeShutdown,
                InboundAdmissionError::Inactive => RuntimeClientError::InvalidState {
                    message: "the conversation runtime is not activated".to_owned(),
                },
                InboundAdmissionError::EmptyContent => RuntimeClientError::InvalidRequest {
                    message: "inbound content must not be empty".to_owned(),
                },
                InboundAdmissionError::DurabilityFailed { message } => {
                    RuntimeClientError::InvalidState { message }
                }
                InboundAdmissionError::Mailbox(error) => RuntimeClientError::InvalidState {
                    message: error.to_string(),
                },
                error @ InboundAdmissionError::ModelUnavailable { .. } => {
                    RuntimeClientError::InvalidModelConfiguration {
                        message: error.to_string(),
                    }
                }
                // The activation guidance seal belongs to the child plane;
                // the human submit path never enters that class.
                error @ InboundAdmissionError::GuidanceSealed => RuntimeClientError::InvalidState {
                    message: error.to_string(),
                },
            })?;
        Ok(RuntimeClientResult::InboundAccepted {
            message_id: admission.message_id,
            inbound_sequence: admission.inbound_sequence,
        })
    }

    /// Requests cancellation through the native current-attempt owner.
    ///
    /// Selection and cancellation share the coordinator's admission/settlement
    /// lock. Observation availability and publication progress do not select
    /// the target or gate this control. The returned native identity denotes
    /// acceptance, while terminal settlement remains owned by the Agent Loop.
    ///
    /// # Errors
    /// Returns the native no-current-attempt or Goal-pause failure.
    pub(crate) fn cancel_current_attempt(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let result = self
            .runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime")
            .cancel_active_attempt();
        #[cfg(test)]
        if let Some(hook) = self.cancellation_receipt_hook.lock().unwrap().take() {
            hook();
        }
        match result {
            Ok(attempt_id) => Ok(RuntimeClientResult::AttemptCancellationAccepted { attempt_id }),
            Err(CancelAttemptError::NoCurrentAttempt) => Err(RuntimeClientError::NoCurrentAttempt),
            // The attempt was cancelled, but the interrupt's durable Goal
            // pause did not commit (Issue #351). The client is told the
            // truth rather than shown a pause that does not exist.
            Err(CancelAttemptError::GoalPauseFailed { diagnostic }) => {
                Err(RuntimeClientError::InvalidState {
                    message: format!(
                        "the attempt was cancelled, but the Goal could not be durably paused: {diagnostic}"
                    ),
                })
            }
        }
    }

    /// Runs one manual idle context compaction to its terminal result and
    /// returns the exact native commit receipt for this invocation.
    pub(crate) async fn compact_context(
        &self,
        request_id: Option<crate::runtime::identity::ManualCompactionRequestId>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let outcome = self
            .runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime")
            .compact_context(request_id)
            .await
            .map_err(|error| match error {
                ManualCompactionError::Inactive | ManualCompactionError::Busy => {
                    RuntimeClientError::InvalidState {
                        message: error.to_string(),
                    }
                }
                ManualCompactionError::Shutdown => RuntimeClientError::RuntimeShutdown,
                ManualCompactionError::ModelUnavailable { .. } => {
                    RuntimeClientError::InvalidModelConfiguration {
                        message: error.to_string(),
                    }
                }
                ManualCompactionError::DurabilityFailed { message }
                | ManualCompactionError::Durable { message } => {
                    RuntimeClientError::InvalidState { message }
                }
                ManualCompactionError::Context(context)
                    if matches!(
                        context.kind,
                        crate::context::ContextErrorKind::NoProgress
                            | crate::context::ContextErrorKind::CannotFit
                    ) =>
                {
                    RuntimeClientError::InvalidState {
                        message: context.message,
                    }
                }
                ManualCompactionError::Context(context) => RuntimeClientError::RuntimeFailure {
                    message: context.message,
                },
            })?;
        #[cfg(test)]
        {
            let gate = self.manual_compaction_response_gate.lock().unwrap().take();
            if let Some((committed, release)) = gate {
                let _ = committed.send(outcome.clone());
                let _ = release.await;
            }
        }
        Ok(RuntimeClientResult::ContextCompacted {
            compaction: Some(super::snapshot::RuntimeClientCompactionView {
                generation: outcome.generation,
                checkpoint_id: outcome.checkpoint_id,
                activation_id: outcome.activation_id,
                tokens_before: outcome.tokens_before,
                estimated_tokens_after: outcome.estimated_tokens_after,
            }),
        })
    }

    /// Accepts one typed native interaction response through the
    /// conversation-owned coordinator.
    ///
    /// # Errors
    ///
    /// Returns a typed Runtime Client error when the interaction is stale,
    /// already settled, or the response fails bounded validation.
    pub(crate) async fn respond_interaction(
        &self,
        interaction: &InteractionRef,
        response: InteractionResponse,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        self.runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime")
            .respond_interaction(interaction, response)
            .await
            .map(|()| RuntimeClientResult::InteractionResponseAccepted {
                interaction: interaction.clone(),
            })
            .map_err(|error| match error {
                RoutedInteractionError::NotPending { interaction } => {
                    RuntimeClientError::InteractionNotPending { interaction }
                }
                RoutedInteractionError::InvalidResponse { message } => {
                    RuntimeClientError::InteractionInvalidResponse { message }
                }
                RoutedInteractionError::AuditFailed { interaction } => {
                    RuntimeClientError::InteractionAuditFailed { interaction }
                }
            })
    }

    pub(crate) async fn cancel_interaction(
        &self,
        interaction: &InteractionRef,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        self.runtime
            .as_ref()
            .expect("writable runtime")
            .control_interaction(
                interaction,
                crate::runtime::interaction::InteractionControl::Cancel,
            )
            .await
            .map(|()| RuntimeClientResult::InteractionResponseAccepted {
                interaction: interaction.clone(),
            })
            .map_err(|error| match error {
                RoutedInteractionError::NotPending { interaction } => {
                    RuntimeClientError::InteractionNotPending { interaction }
                }
                RoutedInteractionError::InvalidResponse { message } => {
                    RuntimeClientError::InteractionInvalidResponse { message }
                }
                RoutedInteractionError::AuditFailed { interaction } => {
                    RuntimeClientError::InteractionAuditFailed { interaction }
                }
            })
    }

    pub(crate) fn content_read(
        &self,
        reference: &super::content::ContentRef,
        offset: u64,
        max_bytes: u32,
    ) -> Result<super::content::ContentChunk, RuntimeClientError> {
        if reference.conversation_id != self.conversation_id {
            return Err(RuntimeClientError::ContentSourceUnavailable);
        }
        let live = matches!(
            reference.source,
            super::content::ContentSource::LivePublicationPrefix { .. }
        );
        let source_cut = {
            let state = self.state.lock().expect("published native content cut");
            state.projection.validate_live_content_ref(reference)?;
            if live {
                None
            } else {
                state.projection.validate_published_source()?;
                Some(
                    state
                        .projection
                        .checkpoint_read_cut()
                        .source_cut
                        .ok_or(RuntimeClientError::ReadCutExpired)?,
                )
            }
        };
        if let Some(source_cut) = source_cut {
            self.store
                .validate_content_source_cut(reference, &source_cut)
                .map_err(|_| RuntimeClientError::ContentSourceUnavailable)?;
        }
        self.store
            .content_read(reference, offset, max_bytes)
            .map_err(RuntimeClientError::content_read_error)
    }
    pub(crate) fn domain_page(
        &self,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let cut = self.lock_published_state().projection.checkpoint_read_cut();
        self.checkpoint_page(&cut, domain, offset, limit)
            .map(|page| RuntimeClientResult::DomainPage { page })
    }

    fn collection_page(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
        let native = *cut
            .collection_cuts
            .get(domain)
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let runtime = self
            .runtime
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let mut items = Vec::new();
        let next_offset;
        match domain {
            "jobs" => {
                let source = runtime
                    .background_registry()
                    .observation_sources_page(native, offset, limit)
                    .map_err(RuntimeClientError::observation_collection_error)?;
                next_offset = source.next_offset;
                for (index, source) in source.items.iter().enumerate() {
                    source.with_value(|value| {
                        items.push(collection_page_item(
                            &super::projection::background_source_view(value),
                            value.job_id.to_string(),
                            offset + index as u64,
                        ));
                    });
                }
            }
            "agents" => {
                let source = self
                    .agent_registry()?
                    .observation_sources_page(native, offset, limit)
                    .map_err(RuntimeClientError::observation_collection_error)?;
                next_offset = source.next_offset;
                for (index, source) in source.items.iter().enumerate() {
                    source.with_value(|value| {
                        items.push(collection_page_item(
                            &super::projection::agent_source_view(value),
                            value.agent_id.to_string(),
                            offset + index as u64,
                        ));
                    });
                }
            }
            _ => return Err(RuntimeClientError::ReadCutExpired),
        }
        Ok(super::checkpoint::CheckpointPage {
            read_cut: cut.clone(),
            cursor: RuntimeClientCursor::new(cut.revision),
            domain: domain.into(),
            total: native.total,
            items,
            next_offset,
            collections: Vec::new(),
        })
    }

    fn collection_identity_page(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
        if limit == 0 || limit > 32 || offset > 1 {
            return Err(RuntimeClientError::InvalidRequest {
                message: "invalid native identity page bounds".into(),
            });
        }
        let (owner, id) = domain
            .split_once('.')
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let native = *cut
            .collection_cuts
            .get(owner)
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let runtime = self
            .runtime
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let items = match owner {
            "jobs" => {
                let id = collection_identity(id)?;
                runtime
                    .background_registry()
                    .with_observation_value_by_id(native, &id, |value| {
                        (offset == 0)
                            .then(|| {
                                collection_page_item(
                                    &super::projection::background_source_view(value),
                                    value.job_id.to_string(),
                                    0,
                                )
                            })
                            .into_iter()
                            .collect()
                    })
            }
            "agents" => {
                let id = collection_identity(id)?;
                self.agent_registry()?
                    .with_observation_value_by_id(native, &id, |value| {
                        (offset == 0)
                            .then(|| {
                                collection_page_item(
                                    &super::projection::agent_source_view(value),
                                    value.agent_id.to_string(),
                                    0,
                                )
                            })
                            .into_iter()
                            .collect()
                    })
            }
            _ => return Err(RuntimeClientError::ReadCutExpired),
        }
        .map_err(RuntimeClientError::observation_collection_error)?;
        Ok(super::checkpoint::CheckpointPage {
            read_cut: cut.clone(),
            cursor: RuntimeClientCursor::new(cut.revision),
            domain: domain.into(),
            total: 1,
            items,
            next_offset: None,
            collections: Vec::new(),
        })
    }
    fn collection_value_read(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        item_offset: u64,
        path: &[String],
        range: (u64, u32),
    ) -> Result<super::checkpoint::CheckpointValueChunk, RuntimeClientError> {
        let (owner, id) = domain
            .split_once('.')
            .map_or((domain, None), |(owner, id)| (owner, Some(id)));
        let native = *cut
            .collection_cuts
            .get(owner)
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let runtime = self
            .runtime
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        match (owner, id) {
            ("jobs", None) => runtime.background_registry().with_observation_source(
                native,
                item_offset,
                |source| {
                    if let Some(text) = source.direct_text(path) {
                        return direct_collection_read(text, cut, domain, item_offset, path, range);
                    }
                    let indexed = source.indexed().map_err(|message| {
                        RuntimeClientError::ObservationUnavailable {
                            message: message.into(),
                        }
                    })?;
                    indexed_collection_read(indexed, cut, domain, item_offset, path, range)
                },
            ),
            ("agents", None) => {
                self.agent_registry()?
                    .with_observation_source(native, item_offset, |source| {
                        if let Some(text) = source.direct_text(path) {
                            return direct_collection_read(
                                text,
                                cut,
                                domain,
                                item_offset,
                                path,
                                range,
                            );
                        }
                        let indexed = source.indexed().map_err(|message| {
                            RuntimeClientError::ObservationUnavailable {
                                message: message.into(),
                            }
                        })?;
                        indexed_collection_read(indexed, cut, domain, item_offset, path, range)
                    })
            }
            ("jobs", Some(id)) if item_offset == 0 => {
                let id = collection_identity(id)?;
                runtime
                    .background_registry()
                    .with_observation_source_by_id(native, &id, |source| {
                        if let Some(text) = source.direct_text(path) {
                            return direct_collection_read(
                                text,
                                cut,
                                domain,
                                item_offset,
                                path,
                                range,
                            );
                        }
                        let indexed = source.indexed().map_err(|message| {
                            RuntimeClientError::ObservationUnavailable {
                                message: message.into(),
                            }
                        })?;
                        indexed_collection_read(indexed, cut, domain, item_offset, path, range)
                    })
            }
            ("agents", Some(id)) if item_offset == 0 => {
                let id = collection_identity(id)?;
                self.agent_registry()?
                    .with_observation_source_by_id(native, &id, |source| {
                        if let Some(text) = source.direct_text(path) {
                            return direct_collection_read(
                                text,
                                cut,
                                domain,
                                item_offset,
                                path,
                                range,
                            );
                        }
                        let indexed = source.indexed().map_err(|message| {
                            RuntimeClientError::ObservationUnavailable {
                                message: message.into(),
                            }
                        })?;
                        indexed_collection_read(indexed, cut, domain, item_offset, path, range)
                    })
            }
            _ => return Err(RuntimeClientError::ReadCutExpired),
        }
        .map_err(RuntimeClientError::observation_collection_error)?
    }

    pub(super) fn pending_read_header(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        source_offset: u64,
    ) -> Result<super::snapshot::RuntimeClientInteractionHeader, RuntimeClientError> {
        {
            let state = self.state.lock().expect("runtime client state lock");
            state.projection.validate_checkpoint_cut(cut)?;
            state
                .projection
                .validate_domain_version(cut, "pending_interactions")?;
            if let Some(header) = state
                .projection
                .snapshot_ref()
                .pending_interactions
                .iter()
                .find(|header| header.source_offset == source_offset)
            {
                return Ok(header.clone());
            }
        }
        if !source_offset.is_multiple_of(2) {
            let native_cut = *cut
                .collection_cuts
                .get("pending_children")
                .ok_or(RuntimeClientError::ReadCutExpired)?;
            return self
                .runtime
                .as_ref()
                .and_then(ConversationRuntime::subagent_registry)
                .ok_or(RuntimeClientError::ReadCutExpired)?
                .pending_interaction_source_at(native_cut, source_offset / 2)
                .map_err(RuntimeClientError::observation_collection_error);
        }
        let native_cut = *cut
            .collection_cuts
            .get("pending_primary")
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let source = self
            .runtime
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?
            .pending_interaction_source_at(native_cut, source_offset / 2)
            .map_err(RuntimeClientError::observation_collection_error)?;
        let mut header = source.header().clone();
        header.source_offset = source_offset;
        Ok(header)
    }
    fn live_page(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
        let live = cut
            .live_publication
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let source = if domain == "attempt.foreground" {
            self.store
                .live_tool_header_page(&live.source, live.revision, offset, limit)
        } else {
            self.store
                .live_block_header_page(&live.source, live.revision, offset, limit)
        }
        .map_err(|_| RuntimeClientError::ContentSourceUnavailable)?;
        let items = source
            .headers
            .into_iter()
            .map(|header| {
                let content = header
                    .content
                    .iter()
                    .map(|reference| super::checkpoint::ContentBinding {
                        path: vec![if reference.source.key().0 == "live_block_metadata" {
                            "metadata".into()
                        } else {
                            "payload".into()
                        }],
                        source_path: None,
                        content_ref: reference.clone(),
                        deferred: true,
                    })
                    .collect();
                Ok(super::checkpoint::CheckpointPageItem {
                    offset: header.position,
                    identity: format!("{}:{}", header.kind, header.block_index.get()),
                    value: Some(
                        super::checkpoint::structural_value(&header)
                            .ok_or(RuntimeClientError::ReadCutExpired)?,
                    ),
                    body_complete: None,
                    key: None,
                    content,
                    deferred: true,
                })
            })
            .collect::<Result<Vec<_>, RuntimeClientError>>()?;
        Ok(super::checkpoint::CheckpointPage {
            read_cut: cut.clone(),
            cursor: RuntimeClientCursor::new(cut.revision),
            domain: domain.into(),
            total: source.total,
            items,
            next_offset: source.next_offset,
            collections: Vec::new(),
        })
    }

    fn canonical_foreground_page(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        offset: u64,
        limit: usize,
    ) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
        let foreground = cut
            .canonical_foreground
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let seed = self
            .state
            .lock()
            .expect("native published foreground")
            .projection
            .checkpoint_seed();
        let mut page = super::tool_observation::page(
            self.store.as_ref(),
            cut,
            "attempt.foreground",
            &foreground.locator,
            offset,
            limit,
        )?;
        if page.total != foreground.total {
            return Err(RuntimeClientError::ReadCutExpired);
        }
        seed.retain_foreground_progress(&mut page)?;
        Ok(page)
    }

    fn pending_page(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        offset: u64,
        limit: usize,
    ) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
        if limit == 0 || limit > 32 {
            return Err(RuntimeClientError::InvalidRequest {
                message: "invalid pending page bounds".into(),
            });
        }
        let runtime = self
            .runtime
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let primary_cut = *cut
            .collection_cuts
            .get("pending_primary")
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let proof = runtime
            .pending_interaction_page(
                primary_cut,
                offset.checked_sub(1).map(|offset| offset / 2),
                limit,
            )
            .map_err(RuntimeClientError::observation_collection_error)?;
        let mut total = proof.cut.total;
        let mut next = proof
            .next_sequence
            .and_then(|sequence| sequence.checked_mul(2));
        let mut headers: Vec<_> = proof
            .head
            .into_iter()
            .map(|(sequence, source)| {
                let mut header = source.header().clone();
                header.source_offset = sequence.checked_mul(2).expect("bounded source offset");
                header
            })
            .collect();
        if let Some(registry) = runtime.subagent_registry() {
            let child_cut = *cut
                .collection_cuts
                .get("pending_children")
                .ok_or(RuntimeClientError::ReadCutExpired)?;
            let child = registry
                .pending_interaction_page(
                    child_cut,
                    offset.checked_sub(2).map(|offset| offset / 2),
                    limit,
                )
                .map_err(RuntimeClientError::observation_collection_error)?;
            total = total
                .checked_add(child.cut.total)
                .ok_or(RuntimeClientError::ReadCutExpired)?;
            let child_next = child
                .next_sequence
                .and_then(|sequence| sequence.checked_mul(2))
                .and_then(|offset| offset.checked_add(1));
            next = next.into_iter().chain(child_next).min();
            headers.extend(child.head);
        }
        headers.sort_by_key(|header| header.source_offset);
        if let Some(header) = headers.get(limit) {
            next = next.into_iter().chain(Some(header.source_offset)).min();
        }
        headers.truncate(limit);
        let items = headers
            .into_iter()
            .map(|header| {
                Ok(super::checkpoint::CheckpointPageItem {
                    offset: header.source_offset,
                    identity: header.interaction.to_string(),
                    value: Some(
                        super::checkpoint::structural_value(&header)
                            .ok_or(RuntimeClientError::ReadCutExpired)?,
                    ),
                    deferred: true,
                    body_complete: Some(false),
                    key: None,
                    content: Vec::new(),
                })
            })
            .collect::<Result<Vec<_>, RuntimeClientError>>()?;
        Ok(super::checkpoint::CheckpointPage {
            read_cut: cut.clone(),
            cursor: RuntimeClientCursor::new(cut.revision),
            domain: "pending_interactions".into(),
            total,
            items,
            next_offset: next,
            collections: Vec::new(),
        })
    }
    fn primary_pending_source(
        &self,
        interaction: &InteractionRef,
    ) -> Result<Arc<crate::runtime::interaction::PendingInteractionSource>, RuntimeClientError>
    {
        if interaction.conversation_id != self.conversation_id {
            return Err(RuntimeClientError::ReadCutExpired);
        }
        self.runtime
            .as_ref()
            .and_then(|runtime| runtime.pending_interaction_source(interaction))
            .ok_or(RuntimeClientError::ReadCutExpired)
    }
    pub(crate) fn interaction_fields(
        &self,
        interaction: &InteractionRef,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        validate_pending_path(path)?;
        let source = self.primary_pending_source(interaction)?;
        let page = source
            .page(path, offset, limit)
            .map_err(RuntimeClientError::observation_read_error)?;
        let fields = page
            .fields
            .into_iter()
            .map(|field| super::types::InteractionFieldItem {
                offset: field.ordinal,
                key: field.key,
                key_complete: field.key_complete,
                key_bytes: field.key_bytes,
                encoded_bytes: field.encoded_bytes,
                children: field.children,
                deferred: field.value.is_none(),
                value: field.value,
            })
            .collect();
        Ok(RuntimeClientResult::InteractionFields {
            interaction: interaction.clone(),
            path: path.to_vec(),
            total: page.total,
            fields,
            next_offset: page.next_offset,
        })
    }
    pub(crate) fn interaction_value_read(
        &self,
        interaction: &InteractionRef,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        use base64::Engine as _;
        validate_pending_path(path)?;
        let source = self.primary_pending_source(interaction)?;
        let chunk = source
            .read(path, offset, max_bytes)
            .map_err(RuntimeClientError::observation_read_error)?;
        Ok(RuntimeClientResult::InteractionValueChunk {
            interaction: interaction.clone(),
            path: path.to_vec(),
            offset,
            bytes_base64: base64::engine::general_purpose::STANDARD.encode(chunk.bytes),
            readable_end: chunk.readable_end,
            next_offset: chunk.next_offset,
        })
    }
    fn pending_value_read(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        item_offset: u64,
        path: &[String],
        range: (u64, u32),
    ) -> Result<super::checkpoint::CheckpointValueChunk, RuntimeClientError> {
        let selection = super::interaction_observation::selection(domain);
        let (ordinal, mut selected) = selection.clone().unwrap_or((item_offset, Vec::new()));
        if selection.is_some() {
            selected.splice(0..0, ["kind".to_owned(), "arguments".to_owned()]);
        }
        let header = self.pending_read_header(cut, ordinal)?;
        let source = self.primary_pending_source(&header.interaction)?;
        if domain == "pending_interactions" && path.is_empty() {
            let source_end = source
                .read(&[], 0, 1)
                .map_err(RuntimeClientError::observation_read_error)?
                .readable_end;
            let plan =
                super::pending_observation::routed_range(&header, source_end, range.0, range.1)?;
            let chunk = source
                .read(&[], plan.native_offset, plan.native_max_bytes)
                .map_err(RuntimeClientError::observation_read_error)?;
            let (bytes, end, next) =
                super::pending_observation::compose_routed(&plan, &chunk.bytes)?;
            return Ok(super::pending_observation::checkpoint_chunk(
                cut,
                domain,
                item_offset,
                path,
                range.0,
                (bytes, end, next),
            ));
        }
        if !selected.is_empty() {
            selected.push(format!("@{item_offset}"));
        }
        selected.extend_from_slice(path);
        let chunk = source
            .read(&selected, range.0, range.1)
            .map_err(RuntimeClientError::observation_read_error)?;
        Ok(super::pending_observation::checkpoint_chunk(
            cut,
            domain,
            item_offset,
            path,
            range.0,
            (chunk.bytes, chunk.readable_end, chunk.next_offset),
        ))
    }

    fn state_source_available(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
    ) -> Result<bool, RuntimeClientError> {
        let available = super::state_observation::available(self.store.as_ref(), cut, domain)?;
        if !available && domain.split('.').next() == Some("goal") {
            let state = self.state.lock().expect("native projection lock");
            if state
                .projection
                .snapshot_ref()
                .goal
                .as_ref()
                .is_some_and(|goal| goal.current.is_some())
            {
                return Err(RuntimeClientError::ReadCutExpired);
            }
        }
        Ok(available)
    }

    pub(crate) fn checkpoint_value_read(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        item_offset: u64,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<super::checkpoint::CheckpointValueChunk, RuntimeClientError> {
        if domain.len() > 256
            || path.len() > 32
            || path.iter().any(|part| part.len() > 256)
            || offset > 9_007_199_254_740_991
            || item_offset > 9_007_199_254_740_991
            || max_bytes == 0
            || max_bytes > 65_536
            || offset
                .checked_add(u64::from(max_bytes))
                .is_none_or(|end| end > 9_007_199_254_740_991)
        {
            return Err(RuntimeClientError::InvalidRequest {
                message: "invalid native domain value read bounds".into(),
            });
        }
        if super::journal_observation::selection(domain).is_some() {
            self.state
                .lock()
                .expect("native source cut")
                .projection
                .validate_checkpoint_cut(cut)?;
            return super::journal_observation::value_read(
                self.store.as_ref(),
                cut,
                domain,
                item_offset,
                path,
                offset,
                max_bytes,
            );
        }
        if let Some(cursor) = super::response::identity_selection(domain) {
            self.state
                .lock()
                .expect("native projection lock")
                .projection
                .validate_checkpoint_cut(cut)?;
            if !path.is_empty() {
                return Err(RuntimeClientError::InvalidRequest {
                    message: "identity source read requires empty field path".into(),
                });
            }
            return super::response::identity_read(
                self.store.as_ref(),
                cut,
                domain,
                cursor,
                item_offset,
                (offset, max_bytes),
            );
        }
        self.selected_value_read(cut, domain, item_offset, path, (offset, max_bytes))
    }

    fn immutable_source_value_read(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        item_offset: u64,
        path: &[String],
        (offset, max_bytes): (u64, u32),
    ) -> Result<Option<super::checkpoint::CheckpointValueChunk>, RuntimeClientError> {
        if super::projection_history::selection(domain).is_some() {
            self.lock_published_state()
                .projection
                .validate_checkpoint_cut(cut)?;
            return super::projection_history::value_read(
                self.store.as_ref(),
                cut,
                domain,
                item_offset,
                path,
                (offset, max_bytes),
            )
            .map(Some);
        }
        if super::delivery::selection(domain).is_some() {
            self.lock_published_state()
                .projection
                .validate_checkpoint_cut(cut)?;
            return super::delivery::value_read(
                self.store.as_ref(),
                cut,
                domain,
                item_offset,
                path,
                offset,
                max_bytes,
            )
            .map(Some);
        }
        if super::attempt_identity::selection(domain).is_some() {
            self.lock_published_state()
                .projection
                .validate_checkpoint_cut(cut)?;
            return super::attempt_identity::value_read(
                self.store.as_ref(),
                cut,
                domain,
                item_offset,
                path,
                offset,
                max_bytes,
            )
            .map(Some);
        }
        if super::occupancy::selection(domain).is_some() {
            self.lock_published_state()
                .projection
                .validate_checkpoint_cut(cut)?;
            return super::occupancy::value_read(
                self.store.as_ref(),
                cut,
                domain,
                item_offset,
                path,
                offset,
                max_bytes,
            )
            .map(Some);
        }
        Ok(None)
    }

    fn selected_value_read(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        item_offset: u64,
        path: &[String],
        range: (u64, u32),
    ) -> Result<super::checkpoint::CheckpointValueChunk, RuntimeClientError> {
        let (offset, max_bytes) = range;
        if let Some(chunk) =
            self.immutable_source_value_read(cut, domain, item_offset, path, range)?
        {
            return Ok(chunk);
        }
        if domain
            .split('.')
            .next()
            .is_some_and(|owner| owner == "jobs" || owner == "agents")
        {
            self.lock_published_state()
                .projection
                .validate_checkpoint_cut(cut)?;
            return self.collection_value_read(cut, domain, item_offset, path, (offset, max_bytes));
        }
        if domain == "pending_interactions"
            || super::interaction_observation::selection(domain).is_some()
        {
            return self.pending_value_read(cut, domain, item_offset, path, (offset, max_bytes));
        }
        if super::state_observation::selection(domain).is_some()
            && self.state_source_available(cut, domain)?
        {
            self.lock_published_state()
                .projection
                .validate_checkpoint_cut(cut)?;
            return super::state_observation::read(
                self.store.as_ref(),
                cut,
                domain,
                item_offset,
                path,
                (offset, max_bytes),
            );
        }
        if let Some((sequence, selected)) = super::trace_observation::selection(domain) {
            self.lock_published_state()
                .projection
                .validate_checkpoint_cut(cut)?;
            return if selected.is_empty() {
                super::trace_observation::read(
                    self.store.as_ref(),
                    cut,
                    domain,
                    sequence,
                    (item_offset, path),
                    (offset, max_bytes),
                )
            } else {
                super::trace_observation::field_read(
                    self.store.as_ref(),
                    cut,
                    domain,
                    sequence,
                    &selected,
                    (item_offset, path),
                    (offset, max_bytes),
                )
            };
        }
        if let Some(field) = transcript_native_field(domain) {
            return self.transcript_field_read(
                cut,
                domain,
                field,
                item_offset,
                path,
                (offset, max_bytes),
            );
        }
        if domain == "inbound.pending" {
            return self.inbound_value_read(cut, domain, item_offset, path, (offset, max_bytes));
        }
        let source = {
            let state = self.lock_published_state();
            state.projection.validate_checkpoint_cut(cut)?;
            state.projection.validate_domain_version(cut, domain)?;
            state
                .projection
                .domain_source(domain, item_offset)
                .ok_or(RuntimeClientError::ReadCutExpired)?
        };
        let mut selected = path.to_vec();
        if let Some((mut prefix, singleton)) = source_domain_path(domain) {
            if !singleton {
                prefix.push(format!("@{item_offset}"));
            }
            selected.splice(0..0, prefix);
        } else if domain == "todos.tasks" {
            selected.splice(0..0, ["tasks".into(), format!("@{item_offset}")]);
        }
        let mut chunk = source.read(cut, domain, item_offset, &selected, offset, max_bytes)?;
        // Source layout paths are internal selectors. The public range identity
        // retains the caller's path within its exact domain and item.
        chunk.path = path.to_vec();
        Ok(chunk)
    }

    fn inbound_value_read(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        item_offset: u64,
        path: &[String],
        range: (u64, u32),
    ) -> Result<super::checkpoint::CheckpointValueChunk, RuntimeClientError> {
        use base64::Engine as _;
        let (offset, max_bytes) = range;
        if !path.is_empty() {
            return Err(RuntimeClientError::InvalidRequest {
                message: "pending read path must name the whole authoritative UserMessage".into(),
            });
        }
        self.lock_published_state()
            .projection
            .validate_checkpoint_cut(cut)?;
        let source_cut = cut
            .source_cut
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let chunk = self
            .store
            .pending_value_read(&source_cut.read, item_offset, offset, max_bytes)
            .map_err(RuntimeClientError::observation_store_error)?;
        Ok(super::checkpoint::CheckpointValueChunk {
            representation:
                crate::runtime_client::checkpoint::CheckpointValueRepresentation::CanonicalJsonV1,
            cut: cut.clone(),
            domain: domain.into(),
            item_offset,
            path: Vec::new(),
            offset,
            bytes_base64: base64::engine::general_purpose::STANDARD.encode(chunk.bytes),
            next_offset: chunk.next_offset,
            readable_end: chunk.readable_end,
        })
    }

    fn transcript_field_read(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        field: (u64, &str),
        item_offset: u64,
        path: &[String],
        range: (u64, u32),
    ) -> Result<super::checkpoint::CheckpointValueChunk, RuntimeClientError> {
        use base64::Engine as _;
        let (offset, max_bytes) = range;
        if !path.is_empty() {
            return Err(RuntimeClientError::InvalidRequest {
                message: "native transcript field read expects an empty path".into(),
            });
        }
        self.lock_published_state()
            .projection
            .validate_checkpoint_cut(cut)?;
        let source = self.transcript_field_source(cut, field.0)?;
        let native = cut
            .source_cut
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let chunk = match (source, field.1) {
            (TranscriptFieldSource::Live(sequence), "completed_response.models") => self
                .store
                .observation_model_read_at(native, sequence, item_offset, offset, max_bytes),
            (TranscriptFieldSource::Inherited(ordinal), "completed_response.models") => self
                .store
                .observation_inherited_model_read(ordinal, item_offset, offset, max_bytes),
            (TranscriptFieldSource::Live(sequence), "turn_process.failure") if item_offset == 0 => {
                self.store
                    .observation_failure_read_at(native, sequence, offset, max_bytes)
            }
            _ => return Err(RuntimeClientError::ReadCutExpired),
        }
        .map_err(RuntimeClientError::observation_store_error)?;
        Ok(super::checkpoint::CheckpointValueChunk {
            representation:
                crate::runtime_client::checkpoint::CheckpointValueRepresentation::CanonicalJsonV1,
            cut: cut.clone(),
            domain: domain.into(),
            item_offset,
            path: Vec::new(),
            offset,
            bytes_base64: base64::engine::general_purpose::STANDARD.encode(chunk.bytes),
            next_offset: chunk.next_offset,
            readable_end: chunk.readable_end,
        })
    }

    fn domain_source_page(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<Option<super::checkpoint::CheckpointPage>, RuntimeClientError> {
        if let Some((path, singleton)) = source_domain_path(domain) {
            let source = {
                let state = self.lock_published_state();
                state.projection.validate_domain_version(cut, domain)?;
                state
                    .projection
                    .domain_source(domain, 0)
                    .ok_or(RuntimeClientError::ReadCutExpired)?
            };
            return source
                .page(cut, domain, &path, offset, limit, singleton)
                .map(Some);
        }
        Ok(None)
    }

    pub(crate) fn checkpoint_page(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
        if cut.conversation_id != self.conversation_id {
            return Err(RuntimeClientError::ReadCutExpired);
        }
        self.lock_published_state()
            .projection
            .validate_checkpoint_cut(cut)?;
        if super::delivery::selection(domain).is_some() {
            return super::delivery::page(self.store.as_ref(), cut, domain, offset, limit);
        }
        if super::occupancy::selection(domain).is_some() {
            return super::occupancy::page(self.store.as_ref(), cut, domain, offset, limit);
        }
        if domain == super::auxiliary::DOMAIN {
            return super::auxiliary::page(self.store.as_ref(), cut, offset, limit);
        }
        if let Some(page) = self.domain_source_page(cut, domain, offset, limit)? {
            return Ok(page);
        }

        if (domain == "attempt.in_flight.blocks" || domain == "attempt.foreground")
            && cut.live_publication.is_some()
        {
            return self.live_page(cut, domain, offset, limit);
        }
        if domain == "attempt.foreground" && cut.canonical_foreground.is_some() {
            return self.canonical_foreground_page(cut, offset, limit);
        }
        if super::journal_observation::selection(domain).is_some() {
            return super::journal_observation::page(
                self.store.as_ref(),
                cut,
                domain,
                offset,
                limit,
            );
        }
        if let Some(cursor) = super::response::identity_selection(domain) {
            return super::response::identity_page(
                self.store.as_ref(),
                cut,
                domain,
                cursor,
                offset,
                limit,
            );
        }
        if domain == "trace.records" {
            return super::trace::checkpoint_page(self.store.as_ref(), cut, offset, limit);
        }
        if super::audit_observation::selection(domain).is_some() {
            return super::audit_observation::page(self.store.as_ref(), cut, domain, offset, limit);
        }
        if domain == "pending_interactions" {
            return self.pending_page(cut, offset, limit);
        }
        if domain.starts_with("jobs.") || domain.starts_with("agents.") {
            return self.collection_identity_page(cut, domain, offset, limit);
        }
        if domain == "jobs" || domain == "agents" {
            return self.collection_page(cut, domain, offset, limit);
        }
        if let Some((ordinal, selected)) = super::interaction_observation::selection(domain) {
            return self.interaction_checkpoint_page(cut, domain, ordinal, selected, offset, limit);
        }
        if super::state_observation::selection(domain).is_some()
            && self.state_source_available(cut, domain)?
        {
            return super::state_observation::page(self.store.as_ref(), cut, domain, offset, limit);
        }
        if let Some(page) = self.trace_checkpoint_page(cut, domain, offset, limit) {
            return page;
        }
        if let Some(field) = transcript_native_field(domain) {
            return self.transcript_field_page(cut, domain, field.0, field.1, offset, limit);
        }
        if let Some(page) = self.durable_checkpoint_page(cut, domain, offset, limit) {
            return page;
        }
        self.published_checkpoint_page(cut, domain, offset, limit)
    }

    fn trace_checkpoint_page(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Option<Result<super::checkpoint::CheckpointPage, RuntimeClientError>> {
        if super::trace_observation::context_selection(domain).is_some() {
            return Some(super::trace_observation::context_page(
                self.store.as_ref(),
                cut,
                domain,
                offset,
                limit,
            ));
        }
        if super::trace_observation::messages_selection(domain).is_some() {
            return Some(super::trace_observation::messages_page(
                self.store.as_ref(),
                cut,
                domain,
                offset,
                limit,
            ));
        }
        let (sequence, selected) = super::trace_observation::selection(domain)?;
        Some(if selected.is_empty() {
            super::trace_observation::page(self.store.as_ref(), cut, domain, sequence, offset)
        } else {
            super::trace_observation::field_page(
                self.store.as_ref(),
                cut,
                domain,
                sequence,
                &selected,
                offset,
                limit,
            )
        })
    }

    fn published_checkpoint_page(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
        let seed = self.lock_published_state().projection.checkpoint_seed();
        let mut page = seed.checkpoint_page(cut, domain, offset, limit)?;
        for item in &mut page.items {
            self.materialize_content_bindings(&mut item.content)?;
        }
        Ok(page)
    }

    fn durable_checkpoint_page(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Option<Result<super::checkpoint::CheckpointPage, RuntimeClientError>> {
        let source_cut = cut.source_cut.as_ref()?;
        if domain == "inbound.pending" {
            return Some(self.pending_checkpoint_page(cut, source_cut, offset, limit));
        }
        if domain == "messages"
            || domain == "transcript.entries"
            || domain.rsplit_once('.').is_some_and(|(prefix, family)| {
                prefix.starts_with("messages.") && matches!(family, "blocks" | "tools")
            })
        {
            return Some(super::history_observation::page(
                self.store.as_ref(),
                cut,
                domain,
                offset,
                limit,
            ));
        }
        None
    }
    fn interaction_checkpoint_page(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        ordinal: u64,
        mut selected: Vec<String>,
        offset: u64,
        limit: usize,
    ) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
        selected.splice(0..0, ["kind".to_owned(), "arguments".to_owned()]);
        let header = self.pending_read_header(cut, ordinal)?;
        let source = self.primary_pending_source(&header.interaction)?;
        let page = source
            .page(&selected, offset, limit)
            .map_err(RuntimeClientError::observation_read_error)?;
        let items = page
            .fields
            .into_iter()
            .map(|field| super::checkpoint::CheckpointPageItem {
                offset: field.ordinal,
                identity: field.ordinal.to_string(),
                value: field.value.clone(),
                deferred: field.value.is_none(),
                key: None,
                body_complete: None,
                content: Vec::new(),
            })
            .collect();
        Ok(super::checkpoint::CheckpointPage {
            read_cut: cut.clone(),
            cursor: RuntimeClientCursor::new(cut.revision),
            domain: domain.into(),
            total: page.total,
            items,
            next_offset: page.next_offset,
            collections: Vec::new(),
        })
    }

    fn pending_checkpoint_page(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        source_cut: &crate::durable::observation::ObservationReadCut,
        offset: u64,
        limit: usize,
    ) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
        #[derive(serde::Serialize, schemars::JsonSchema)]
        struct PendingSummary {
            sequence: crate::runtime::inbound::InboundSequence,
            revision: u64,
            message: Option<crate::message::types::UserMessageBlock>,
        }
        let source = self
            .store
            .pending_header_page(&source_cut.read, (offset > 0).then_some(offset), limit)
            .map_err(RuntimeClientError::observation_store_error)?;
        let items = source
            .headers
            .into_iter()
            .map(|header| {
                let message = match header.message.display {
                    Some(crate::message::types::MessageBlock::User(message)) => Some(message),
                    _ => None,
                };
                let value = super::checkpoint::structural_value(&PendingSummary {
                    sequence: crate::runtime::inbound::InboundSequence::new(header.sequence),
                    revision: header.revision,
                    message,
                });
                super::checkpoint::CheckpointPageItem {
                    body_complete: None,
                    key: None,
                    offset: header.sequence,
                    identity: header.message.identity.as_ref().map_or_else(
                        || format!("pending:{}", header.sequence),
                        ToString::to_string,
                    ),
                    deferred: header.message.deferred,
                    value,
                    content: Vec::new(),
                }
            })
            .collect();
        Ok(super::checkpoint::CheckpointPage {
            collections: Vec::new(),
            cursor: RuntimeClientCursor::new(cut.revision),
            read_cut: cut.clone(),
            domain: "inbound.pending".into(),
            total: source.total,
            items,
            next_offset: source.next_sequence,
        })
    }

    fn transcript_field_source(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        cursor: u64,
    ) -> Result<TranscriptFieldSource, RuntimeClientError> {
        let source_cut = cut
            .source_cut
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        if cursor > source_cut.read.transcript {
            return Err(RuntimeClientError::ReadCutExpired);
        }
        self.store
            .response_identity_source(source_cut, cursor)
            .map_err(RuntimeClientError::observation_store_error)?
            .map(|source| match source {
                crate::durable::observation::ResponseIdentitySource::Attempt { sequence } => {
                    TranscriptFieldSource::Live(sequence)
                }
                crate::durable::observation::ResponseIdentitySource::Inherited { ordinal } => {
                    TranscriptFieldSource::Inherited(ordinal)
                }
            })
            .ok_or(RuntimeClientError::ContentSourceUnavailable)
    }

    #[allow(clippy::too_many_arguments)]
    fn transcript_field_page(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        cursor: u64,
        field: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
        if !(1..=super::checkpoint::CHECKPOINT_PAGE_ITEMS).contains(&limit) {
            return Err(RuntimeClientError::InvalidRequest {
                message: "native field page limit must be in 1..=32".into(),
            });
        }
        let source = self.transcript_field_source(cut, cursor)?;
        let native = cut
            .source_cut
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let (total, items, next_offset) = match (source, field) {
            (TranscriptFieldSource::Live(sequence), "completed_response.models") => {
                let page = self
                    .store
                    .observation_models_at(native, sequence, offset, limit)
                    .map_err(RuntimeClientError::observation_store_error)?;
                let items = page
                    .names
                    .into_iter()
                    .map(|name| {
                        model_name_page_item(
                            name.ordinal,
                            name.request_id.as_ref().map_or_else(
                                || format!("trace:{}", name.source_sequence),
                                ToString::to_string,
                            ),
                            &super::snapshot::RuntimeClientModelName {
                                request_id: name.request_id,
                                request_id_source: name.request_id_source,
                                source_sequence: Some(name.source_sequence),
                                encoded_bytes: name.encoded_bytes,
                                value: name.value,
                            },
                        )
                    })
                    .collect();
                (page.total, items, page.next_offset)
            }
            (TranscriptFieldSource::Inherited(ordinal), "completed_response.models") => {
                let page = self
                    .store
                    .observation_inherited_models(ordinal, offset, limit)
                    .map_err(RuntimeClientError::observation_store_error)?;
                let items = page
                    .names
                    .into_iter()
                    .map(|name| {
                        model_name_page_item(
                            name.ordinal,
                            name.ordinal.to_string(),
                            &super::snapshot::RuntimeClientModelName {
                                request_id: None,
                                request_id_source: None,
                                source_sequence: None,
                                encoded_bytes: name.encoded_bytes,
                                value: name.value,
                            },
                        )
                    })
                    .collect();
                (page.total, items, page.next_offset)
            }
            (TranscriptFieldSource::Live(sequence), "turn_process.failure") => {
                let summary = self
                    .store
                    .observation_execution_at(native, sequence)
                    .map_err(RuntimeClientError::observation_store_error)?;
                let items = if offset == 0 {
                    summary
                        .failure
                        .map(|failure| super::checkpoint::CheckpointPageItem {
                            body_complete: None,
                            key: None,
                            offset: 0,
                            identity: format!("attempt:{sequence}"),
                            deferred: summary.failure_deferred,
                            value: super::checkpoint::structural_value(&failure),
                            content: Vec::new(),
                        })
                        .into_iter()
                        .collect()
                } else {
                    Vec::new()
                };
                (u64::from(summary.failure_readable_end > 0), items, None)
            }
            _ => return Err(RuntimeClientError::ReadCutExpired),
        };
        Ok(super::checkpoint::CheckpointPage {
            collections: Vec::new(),
            cursor: RuntimeClientCursor::new(cut.revision),
            read_cut: cut.clone(),
            domain: domain.into(),
            total,
            items,
            next_offset,
        })
    }

    fn materialize_content_bindings(
        &self,
        bindings: &mut [super::checkpoint::ContentBinding],
    ) -> Result<(), RuntimeClientError> {
        for binding in bindings {
            if matches!(
                binding.content_ref.source,
                super::content::ContentSource::LivePublicationPrefix { .. }
            ) {
                continue;
            }
            let raw = if matches!(
                binding.content_ref.source,
                super::content::ContentSource::CanonicalMessageBlock { .. }
            ) && binding
                .path
                .last()
                .is_some_and(|field| field == "arguments")
            {
                self.store
                    .content_descriptor(
                        &binding.content_ref.source,
                        super::content::ContentRepresentation::RawJsonUtf8,
                    )
                    .map_err(|_| RuntimeClientError::ContentSourceUnavailable)?
            } else {
                None
            };
            binding.content_ref = match raw {
                Some(reference) => reference,
                None => self
                    .store
                    .content_descriptor(
                        &binding.content_ref.source,
                        binding.content_ref.representation,
                    )
                    .map_err(|_| RuntimeClientError::ContentSourceUnavailable)?
                    .ok_or(RuntimeClientError::ContentSourceUnavailable)?,
            };
        }
        Ok(())
    }

    /// Read a bounded checkpoint at one captured native cut. Trace enrichment
    /// happens outside the Projection lock and never substitutes a moving cursor.
    pub(crate) fn checkpoint_with_trace(
        &self,
        records: &[super::trace::TraceCursor],
    ) -> Result<super::checkpoint::RuntimeClientCheckpoint, RuntimeClientError> {
        if records.len() > super::trace::TRACE_RECORD_LIMIT {
            return Err(RuntimeClientError::InvalidRequest {
                message: "Trace refresh limit is 8".into(),
            });
        }
        let deadline = std::time::Instant::now()
            + std::time::Duration::from_secs(super::checkpoint::CHECKPOINT_MATERIALIZATION_SECONDS);
        let mut last = RuntimeClientError::ReadCutExpired;
        for attempt in 0..super::checkpoint::CHECKPOINT_ATTEMPTS {
            let progress = self.publication_gate.generation();
            if std::time::Instant::now() >= deadline {
                break;
            }
            let result = self
                .repair_failed_source_for_checkpoint(deadline)
                .and_then(|()| self.materialize_checkpoint(records, deadline));
            match result {
                Ok(checkpoint) => return Ok(checkpoint),
                Err(RuntimeClientError::ReadCutExpired) => {
                    last = RuntimeClientError::ReadCutExpired;
                    if attempt + 1 < super::checkpoint::CHECKPOINT_ATTEMPTS {
                        self.publication_gate.wait_after(progress, deadline);
                    }
                }
                Err(error) => return Err(error),
            }
        }
        Err(last)
    }

    /// Request one read on the existing worker and await its exact fence.
    /// The worker retains/cancels/joins physical Source work during shutdown.
    fn repair_failed_source_for_checkpoint(
        &self,
        deadline: std::time::Instant,
    ) -> Result<(), RuntimeClientError> {
        if self.read_only || !self.worker_started.load(Ordering::Acquire) {
            return Ok(());
        }
        let cut = {
            let mut state = self.lock_published_state();
            if self.pending.is_closed()
                || !self.pending.can_publish_prepared_reads()
                || state.projection.snapshot_ref().shutting_down
            {
                return Ok(());
            }
            state.projection.request_failed_read_repair()
        };
        let Some((cut, newly_requested)) = cut else {
            return Ok(());
        };
        if newly_requested {
            self.pending
                .push(ConversationObservation::JournalSourceReady);
        }
        #[cfg(test)]
        if let Some(hook) = self
            .publication_gate
            .repair_request_hook
            .lock()
            .unwrap()
            .take()
        {
            hook();
        }
        loop {
            let progress = self.publication_gate.generation();
            {
                let state = self.lock_published_state();
                if state.projection.read_domain_fence() != cut {
                    return Err(RuntimeClientError::ReadCutExpired);
                }
                if state.projection.read_domain_cut().is_none() {
                    drop(state);
                    // The read is folded once the domains are clean, but its
                    // publication is encoded outside the state lock and
                    // installed afterwards, before its transition completes.
                    // The checkpoint this repair serves must include that
                    // install, within the checkpoint's own deadline.
                    if self.publication_gate.wait_installed(deadline) {
                        return Ok(());
                    }
                    return Err(RuntimeClientError::ObservationUnavailable {
                        message: "checkpoint Source repair deadline exhausted before its publication installed".into(),
                    });
                }
                if self.pending.is_closed()
                    || !self.pending.can_publish_prepared_reads()
                    || state.projection.snapshot_ref().shutting_down
                {
                    return Err(RuntimeClientError::ObservationUnavailable {
                        message: "native Source repair awaits its observation owner".into(),
                    });
                }
            }
            if std::time::Instant::now() >= deadline {
                return Err(RuntimeClientError::ObservationUnavailable {
                    message: "checkpoint Source repair deadline exhausted".into(),
                });
            }
            self.publication_gate.wait_after(progress, deadline);
        }
    }

    fn materialize_checkpoint(
        &self,
        records: &[super::trace::TraceCursor],
        deadline: std::time::Instant,
    ) -> Result<super::checkpoint::RuntimeClientCheckpoint, RuntimeClientError> {
        let seed = {
            let state = self.state.lock().expect("published checkpoint state");
            #[cfg(test)]
            state.projection.probe_snapshot_enter();
            state.projection.validate_published_source()?;
            state.projection.checkpoint_seed()
        };
        let mut checkpoint = seed.checkpoint_candidate()?;
        self.materialize_content_bindings(&mut checkpoint.content)?;
        self.materialize_trace(&mut checkpoint.snapshot, &checkpoint.read_cut, records)?;
        self.materialize_auxiliary_audit_frontier(&mut checkpoint, deadline)?;
        if !records.is_empty() {
            checkpoint
                .collections
                .retain(|frontier| frontier.domain != "trace_updates");
            checkpoint
                .collections
                .push(super::checkpoint::CollectionFrontier {
                    domain: "trace_updates".into(),
                    revision: checkpoint.read_cut.through,
                    total: checkpoint.snapshot.trace_updates.len() as u64,
                    loaded: checkpoint.snapshot.trace_updates.len() as u64,
                    next_offset: None,
                });
        }
        if std::time::Instant::now() >= deadline {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: "checkpoint materialization deadline exhausted".into(),
            });
        }
        super::checkpoint::fit_checkpoint(&mut checkpoint)?;
        self.state
            .lock()
            .expect("published checkpoint fence")
            .projection
            .validate_checkpoint_cut(&checkpoint.read_cut)?;
        Ok(checkpoint)
    }

    fn materialize_auxiliary_audit_frontier(
        &self,
        checkpoint: &mut super::checkpoint::RuntimeClientCheckpoint,
        deadline: std::time::Instant,
    ) -> Result<(), RuntimeClientError> {
        if std::time::Instant::now() >= deadline {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: "checkpoint auxiliary audit deadline exhausted".into(),
            });
        }
        let total = self
            .store
            .auxiliary_audit_total(crate::model::auxiliary::AuxiliaryAuditCut {
                journal: checkpoint.read_cut.through,
            })
            .map_err(RuntimeClientError::observation_store_error)?;
        if total > 9_007_199_254_740_991 {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: "auxiliary audit total exceeds the protocol integer domain".into(),
            });
        }
        if std::time::Instant::now() >= deadline {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: "checkpoint auxiliary audit deadline exhausted".into(),
            });
        }
        checkpoint
            .collections
            .retain(|frontier| frontier.domain != super::auxiliary::DOMAIN);
        checkpoint
            .collections
            .push(super::checkpoint::CollectionFrontier {
                domain: super::auxiliary::DOMAIN.into(),
                revision: checkpoint
                    .read_cut
                    .domain_revisions
                    .get("auxiliary")
                    .copied()
                    .unwrap_or(0),
                total,
                loaded: 0,
                next_offset: (total > 0).then_some(0),
            });
        Ok(())
    }

    pub(crate) fn checkpoint(
        &self,
    ) -> Result<super::checkpoint::RuntimeClientCheckpoint, RuntimeClientError> {
        self.checkpoint_with_trace(&[])
    }

    fn materialize_trace(
        &self,
        snapshot: &mut super::snapshot::RuntimeClientSnapshot,
        cut: &super::checkpoint::CheckpointReadCut,
        records: &[super::trace::TraceCursor],
    ) -> Result<(), RuntimeClientError> {
        #[cfg(test)]
        if let Some(hook) = self.trace_cut_hook.lock().unwrap().take() {
            hook();
        }
        // The default head is published by the same bounded read-domain owner
        // as event metadata. Only an explicitly requested refresh is read here.
        snapshot.trace_updates = super::trace::TraceProjection::at_cut(
            self.store.as_ref(),
            cut.source_cut
                .as_ref()
                .ok_or(RuntimeClientError::ReadCutExpired)?,
        )
        .and_then(|projection| {
            projection.refresh(records, self.runtime.as_ref().map(|_| &*snapshot))
        })
        .map_err(RuntimeClientError::observation_store_error)?;
        Ok(())
    }

    pub(crate) fn goal_control(
        &self,
        control: crate::goal::GoalControl,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        if matches!(control, crate::goal::GoalControl::Show) {
            return self.domain_page("goal", 0, 32);
        }
        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| RuntimeClientError::InvalidState {
                message: "No live Goal capability in historical inspection".to_owned(),
            })?;
        runtime
            .control_goal(control)
            .map(|view| RuntimeClientResult::GoalMutation {
                reference: view.current.map(|goal| goal.reference),
            })
            .map_err(|message| RuntimeClientError::InvalidRequest { message })
    }

    /// Read Trace through its independent presentation owner. No cursor mutation.
    #[allow(clippy::needless_pass_by_value)] // Attachment dispatch owns request parameters.
    pub(crate) fn trace_page(
        &self,
        before: Option<super::trace::TraceCursor>,
        limit: usize,
        records: Vec<super::trace::TraceCursor>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        if limit == 0 || limit > super::trace::TRACE_PAGE_LIMIT {
            return Err(RuntimeClientError::InvalidRequest {
                message: "Trace limit must be 1..=32".into(),
            });
        }
        if records.len() > super::trace::TRACE_RECORD_LIMIT {
            return Err(RuntimeClientError::InvalidRequest {
                message: "Trace refresh limit is 8".into(),
            });
        }
        let seed = self.lock_published_state().projection.checkpoint_seed();
        let checkpoint = seed.checkpoint_candidate()?;
        let current = checkpoint.snapshot;
        let projection = super::trace::TraceProjection::at_cut(
            self.store.as_ref(),
            checkpoint
                .read_cut
                .source_cut
                .as_ref()
                .ok_or(RuntimeClientError::ReadCutExpired)?,
        )
        .map_err(RuntimeClientError::observation_store_error)?;
        let mut page = projection
            .page(before.as_ref(), limit)
            .map_err(RuntimeClientError::observation_store_error)?;
        page.read_cut = Some(checkpoint.read_cut.clone());
        if self.runtime.is_some() {
            super::trace::repair_records(&mut page.records, &current);
        }
        page.updates = projection
            .refresh(&records, self.runtime.as_ref().map(|_| &current))
            .map_err(RuntimeClientError::observation_store_error)?;
        super::trace::ensure_page_bound(&page)
            .map_err(RuntimeClientError::observation_store_error)?;
        Ok(RuntimeClientResult::TracePage { page })
    }

    /// Locate an exact Tool occurrence through the complete native history.
    pub(crate) async fn trace_locate_tool(
        self: &Arc<Self>,
        locator: super::trace::TraceToolLocator,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let owner = Arc::clone(self);
        // The complete-history scan cannot occupy an async transport worker.
        tokio::task::spawn_blocking(move || {
            let (current, cut) = {
                let state = owner
                    .state
                    .lock()
                    .expect("runtime client host lock poisoned");
                (
                    state.projection.checkpoint_seed(),
                    state.projection.checkpoint_read_cut(),
                )
            };
            let current = current.checkpoint_candidate()?.snapshot;
            #[cfg(test)]
            crate::local_runtime::live_inspection::trace_read_gate(owner.store.conversation_id());
            let source_cut = cut
                .source_cut
                .as_ref()
                .ok_or(RuntimeClientError::ReadCutExpired)?;
            let projection =
                super::trace::TraceProjection::at_cut(owner.store.as_ref(), source_cut)
                    .map_err(RuntimeClientError::observation_store_error)?;
            let mut location = projection
                .locate_tool_at(&locator, source_cut)
                .map_err(RuntimeClientError::observation_store_error)?;
            if let Some(location) = &mut location {
                location.page.read_cut = Some(cut.clone());
            }
            if owner.runtime.is_some()
                && let Some(location) = &mut location
            {
                super::trace::repair_records(&mut location.page.records, &current);
            }
            Ok(RuntimeClientResult::TraceToolLocation { location })
        })
        .await
        .map_err(|_| RuntimeClientError::RuntimeFailure {
            message: "Trace tool locator failed".into(),
        })?
    }

    /// Reads the heavy detail of one exact Trace record identity.
    ///
    /// Detail is a pure historical read at the same kind of cut a page uses:
    /// it drains no observation, advances no cursor, consumes no pending
    /// work and settles nothing. Its lifecycle-bearing summary counterpart
    /// stays the authority for current state, so no live repair applies here
    /// — detail carries immutable historical facts only.
    #[allow(clippy::needless_pass_by_value)] // Attachment dispatch owns request parameters.
    pub(crate) fn projection_page(
        &self,
        after: crate::context::checkpoint::ProjectionActivationId,
        limit: usize,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let through = {
            let state = self.lock_state();
            state.projection.validate_published_source()?;
            state.projection.checkpoint_read_cut().through
        };
        super::projection_history::page(self.store.as_ref(), through, after, limit)
            .map(|page| RuntimeClientResult::ProjectionPage { page })
            .map_err(|e| RuntimeClientError::RuntimeFailure {
                message: e.to_string(),
            })
    }
    #[allow(clippy::needless_pass_by_value)] // Attachment dispatch owns request parameters.
    pub(crate) fn projection_detail(
        &self,
        id: crate::runtime::identity::CheckpointId,
        offset: u32,
        limit: usize,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let cut = {
            let state = self.lock_state();
            state.projection.validate_published_source()?;
            state.projection.checkpoint_read_cut()
        };
        super::projection_history::detail(self.store.as_ref(), &cut, &id, offset, limit)
            .map(|detail| RuntimeClientResult::ProjectionDetail {
                detail: Box::new(detail),
            })
            .map_err(|e| RuntimeClientError::RuntimeFailure {
                message: e.to_string(),
            })
    }
    #[allow(clippy::needless_pass_by_value)] // Attachment dispatch owns request parameters.
    pub(crate) fn trace_detail(
        &self,
        record_id: String,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        if record_id.len() > 256 {
            return Err(RuntimeClientError::InvalidRequest {
                message: "Trace record identity is too long".into(),
            });
        }
        let sequence = record_id
            .strip_prefix("trace:")
            .and_then(|value| value.parse::<u64>().ok())
            .ok_or_else(|| RuntimeClientError::InvalidRequest {
                message: "invalid native Trace record identity".into(),
            })?;
        let domain = format!("trace.records.{sequence}.detail");
        self.domain_page(&domain, 0, 32)
    }

    /// Reads one bounded durable transcript page. The transcript cursor is
    /// intentionally distinct from the Runtime Client observation cursor.
    pub(crate) fn transcript_page(
        &self,
        before: Option<RuntimeClientTranscriptCursor>,
        limit: usize,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        validate_transcript_page_limit(limit)?;
        let page = read_transcript_page(self.store.as_ref(), before, limit, || {
            self.lock_published_state()
                .projection
                .checkpoint_read_cut()
                .source_cut
                .ok_or(RuntimeClientError::ReadCutExpired)
        })?;
        Ok(RuntimeClientResult::TranscriptPage { page })
    }

    pub(crate) fn conversation_turns(
        &self,
        offset: Option<usize>,
        limit: usize,
    ) -> Result<crate::runtime_client::turn::ObservedTurnPage, RuntimeClientError> {
        self.store
            .observation_turns(offset.map(|value| value as u64), limit)
            .map_err(RuntimeClientError::observation_store_error)
    }

    pub(crate) fn conversation_window(
        &self,
        at: &crate::durable::reading::ConversationWindowAt,
        limit: usize,
    ) -> Result<crate::runtime_client::snapshot::ConversationWindow, RuntimeClientError> {
        read_conversation_window(self.store.as_ref(), at, limit)
    }

    /// Reads one bounded page of historical user-message boundaries, selected
    /// against the current committed Surface head.
    ///
    /// The revision travels with the page because it is part of what a `/fork`
    /// or `/tree` selection *means*: a later append must not silently change
    /// which conversation cut the reader chose.
    pub(crate) fn user_message_boundaries(
        &self,
        offset: usize,
        limit: usize,
    ) -> Result<
        (
            crate::conversation::SurfaceRevision,
            Vec<crate::local_runtime::session::observation::SessionUserMessageBoundaryObservation>,
            Option<usize>,
        ),
        RuntimeClientError,
    > {
        if limit == 0 || limit > 32 {
            return Err(RuntimeClientError::InvalidRequest {
                message: "boundary pages require 1..32 rows".into(),
            });
        }
        let cut = {
            let state = self.state.lock().expect("native projection lock");
            state.projection.validate_published_source()?;
            state.projection.checkpoint_read_cut()
        };
        let source = cut
            .source_cut
            .as_ref()
            .ok_or(RuntimeClientError::ReadCutExpired)?;
        let page = self
            .store
            .surface_boundary_header_page(source.surface_revision, offset as u64, limit)
            .map_err(RuntimeClientError::observation_store_error)?;
        let boundaries=page.boundaries.into_iter().map(|boundary|{
            let header=boundary.header;
            let locator=header.locator.ok_or(RuntimeClientError::ReadCutExpired)?;
            let message=match header.display{Some(crate::message::types::MessageBlock::User(user))=>Some(user),None=>None,_=>return Err(RuntimeClientError::ReadCutExpired)};
            Ok(crate::local_runtime::session::observation::SessionUserMessageBoundaryObservation{surface_revision:boundary.surface_revision,locator,message,block_count:header.block_count,content:header.content,collections:header.collections})
        }).collect::<Result<Vec<_>,RuntimeClientError>>()?;
        self.state
            .lock()
            .expect("native projection lock")
            .projection
            .validate_checkpoint_cut(&cut)?;
        Ok((
            page.through,
            boundaries,
            page.next_offset
                .map(usize::try_from)
                .transpose()
                .map_err(|_| RuntimeClientError::ReadCutExpired)?,
        ))
    }

    /// Returns a durable request-history read handle owned by the
    /// conversation runtime.
    ///
    /// The durable `ConversationStore` owns these snapshots. The returned
    /// value is a read-only handle; each historical read is resolved through
    /// the runtime authority and does not create another conversation or
    /// transcript authority.
    #[must_use]
    pub(crate) fn request_history(&self) -> RequestHistory {
        RequestHistory::new(self.store.clone())
    }

    /// Reconstructs one retained provider-neutral request from durable facts.
    ///
    /// # Errors
    ///
    /// Returns a lookup or historical reconstruction error for an unknown or
    /// invalid request.
    pub(crate) fn reconstruct_request(
        &self,
        identity: &RequestIdentity,
    ) -> Result<ModelRequest, RequestHistoryError> {
        self.request_history().reconstruct(identity)
    }

    /// A request-owned wait reads the Child Projection's native ring. There
    /// is no Parent mirror and no event queue beyond this finite result batch.
    pub(crate) async fn observe_events(
        self: &Arc<Self>,
        attachment_id: &AttachmentId,
        after_cursor: RuntimeClientCursor,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let (subscription, _) = self.subscribe_events(attachment_id, after_cursor)?;
        let mut cursor = after_cursor;
        let mut events = Vec::new();
        let mut bytes = 0usize;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let delivery = if events.is_empty() {
                match tokio::time::timeout_at(deadline, subscription.next()).await {
                    Ok(delivery) => delivery,
                    Err(_) => break,
                }
            } else {
                subscription.try_next()
            };
            match delivery {
                EventDelivery::Event(event) => {
                    let weight = super::checkpoint::bounded_bytes(
                        &event,
                        super::checkpoint::CHECKPOINT_TARGET_BYTES,
                    )
                    .ok_or_else(|| RuntimeClientError::InvalidState {
                        message: "projection published an unbounded transition".into(),
                    })?
                    .len();
                    if bytes + weight > super::checkpoint::CHECKPOINT_TARGET_BYTES {
                        break;
                    }
                    if event.cursor.get()
                        != cursor
                            .get()
                            .checked_add(1)
                            .ok_or(RuntimeClientError::ProjectionExhausted)?
                    {
                        return Err(RuntimeClientError::ResyncRequired {
                            after_cursor,
                            earliest_serviceable: event.cursor,
                        });
                    }
                    bytes += weight;
                    cursor = event.cursor;
                    events.push(event);
                    if events.len() == super::checkpoint::CHECKPOINT_PAGE_ITEMS {
                        break;
                    }
                }
                EventDelivery::Pending => break,
                EventDelivery::ResyncRequired {
                    after_cursor,
                    earliest_serviceable,
                } => {
                    return Err(RuntimeClientError::ResyncRequired {
                        after_cursor,
                        earliest_serviceable,
                    });
                }
                EventDelivery::Unavailable { message } => {
                    return Err(RuntimeClientError::ObservationUnavailable { message });
                }
                EventDelivery::Exhausted => return Err(RuntimeClientError::ProjectionExhausted),
                EventDelivery::Closed => {
                    return Err(RuntimeClientError::ObservationUnavailable {
                        message: "native observation source ended".into(),
                    });
                }
            }
        }
        Ok(RuntimeClientResult::EventBatch {
            after_cursor,
            cursor,
            events,
        })
    }

    /// Subscribes one attachment to the observation stream after a
    /// serviceable cursor.
    ///
    /// The returned subscription receives every subsequently published
    /// event (and the retained replay gap) or fails explicitly with
    /// [`RuntimeClientError::ResyncRequired`].
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::NotAttached`] for an unknown
    /// attachment identity and [`RuntimeClientError::ResyncRequired`] for
    /// an unserviceable cursor.
    pub(crate) fn subscribe_events(
        self: &Arc<Self>,
        attachment_id: &AttachmentId,
        after_cursor: RuntimeClientCursor,
    ) -> Result<(EventSubscription, RuntimeClientResult), RuntimeClientError> {
        self.ensure_worker();
        let mut state = self.state.lock().expect("native projection lock");
        let previous_subscriber = if state
            .control_attachment
            .as_ref()
            .is_some_and(|attachment| attachment.attachment_id == *attachment_id)
        {
            state
                .control_attachment
                .as_ref()
                .expect("the control attachment identity was just checked")
                .subscriber_id
        } else {
            state
                .read_only_attachments
                .get(attachment_id)
                .map_or(Err(RuntimeClientError::NotAttached), |attachment| {
                    Ok(attachment.subscriber_id)
                })?
        };
        let (subscriber_id, notify) = state.projection.subscribe(after_cursor)?;
        if let Some(previous) = previous_subscriber {
            state.projection.remove_subscriber(previous);
        }
        if state
            .control_attachment
            .as_ref()
            .is_some_and(|attachment| attachment.attachment_id == *attachment_id)
        {
            state
                .control_attachment
                .as_mut()
                .expect("the control attachment identity was just checked")
                .subscriber_id = Some(subscriber_id);
        } else {
            state
                .read_only_attachments
                .get_mut(attachment_id)
                .expect("the read-only attachment identity was just checked")
                .subscriber_id = Some(subscriber_id);
        }
        drop(state);
        Ok((
            EventSubscription {
                inner: Arc::new(SubscriptionInner {
                    host: Arc::downgrade(self),
                    subscriber_id,
                    notify,
                }),
            },
            RuntimeClientResult::Subscribed { after_cursor },
        ))
    }

    /// Subscription replacement and removal of the previous registration
    /// share this host lock. A transport can observe the old handle closing
    /// before the attachment has stored its new handle, so retirement must
    /// consult this owner rather than the attachment's delivery-handle cache.
    pub(crate) fn subscription_superseded(
        &self,
        attachment_id: &AttachmentId,
        observed: &EventSubscription,
    ) -> bool {
        if !std::ptr::eq(self, observed.inner.host.as_ptr()) {
            return false;
        }
        let state = self.state.lock().expect("native projection lock");
        state
            .control_attachment
            .as_ref()
            .filter(|attachment| attachment.attachment_id == *attachment_id)
            .or_else(|| state.read_only_attachments.get(attachment_id))
            .and_then(|attachment| attachment.subscriber_id)
            .is_some_and(|current| current != observed.inner.subscriber_id)
    }

    /// Read the native capability projection.
    pub(crate) fn capability(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.domain_page("capabilities", 0, 32)
    }

    /// Reads the safe public model catalog through the conversation
    /// runtime's authoritative session model state.
    ///
    /// It never carries a credential value, an adapter, or a provider HTTP
    /// client.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] when the
    /// observation stream is over.
    pub(crate) fn model_catalog(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.domain_page("model_catalog.models", 0, 32)
    }

    /// Resolve only a committed successful Tool delivery in this exact viewed
    /// lineage. A browser supplies canonical coordinates, never a file path.
    pub(crate) fn session_file_reference(
        &self,
        locator: &super::content::CanonicalContentLocator,
        index: usize,
    ) -> Result<crate::tools::session_files::SessionFileReference, RuntimeClientError> {
        let index = u64::try_from(index).map_err(|_| RuntimeClientError::InvalidRange)?;
        self.store
            .observation_file_delivery(locator, index)
            .map(|selected| selected.file)
            .map_err(RuntimeClientError::observation_store_error)
    }

    pub(crate) fn agent_file_workspace(
        &self,
        id: &crate::runtime::identity::AgentId,
    ) -> Result<std::path::PathBuf, RuntimeClientError> {
        self.agent_registry()?
            .agent_workspace(id)
            .ok_or_else(|| RuntimeClientError::InvalidState {
                message: "child workspace unavailable".into(),
            })
    }
    pub(crate) fn agent_file_reference(
        &self,
        id: &crate::runtime::identity::AgentId,
        locator: &super::content::CanonicalContentLocator,
        index: usize,
    ) -> Result<crate::tools::session_files::SessionFileReference, RuntimeClientError> {
        let agent = self.child_view(id)?;
        let store = self
            .agent_registry()?
            .transcript_store(&agent.activation_id)
            .map_err(|_| RuntimeClientError::InvalidState {
                message: "child history unavailable".into(),
            })?;
        let index = u64::try_from(index).map_err(|_| RuntimeClientError::InvalidRange)?;
        let selected = store
            .observation_file_delivery(locator, index)
            .map_err(RuntimeClientError::observation_store_error)?;
        if selected.cut.read.conversation_id != agent.child_conversation_id
            || selected.file.scope.conversation_id != agent.child_conversation_id
        {
            return Err(RuntimeClientError::InvalidState {
                message: "delivery does not belong to this child".into(),
            });
        }
        let current = self.child_view(id)?;
        let durable = |state| {
            matches!(
                state,
                crate::runtime::subagent::AgentState::Inactive
                    | crate::runtime::subagent::AgentState::Unavailable
            )
        };
        if current.activation_id != agent.activation_id
            || current.child_conversation_id != agent.child_conversation_id
            || durable(current.state) != durable(agent.state)
        {
            return Err(RuntimeClientError::ReadCutExpired);
        }
        Ok(selected.file)
    }
    pub(crate) fn agent_artifact_read(
        &self,
        id: &crate::runtime::identity::AgentId,
        artifact_id: &crate::runtime::identity::ArtifactId,
    ) -> Result<String, RuntimeClientError> {
        use base64::Engine;
        let agent = self.child_view(id)?;
        let registry = self.agent_registry()?;
        let _store = registry
            .transcript_store(&agent.activation_id)
            .map_err(|_| RuntimeClientError::InvalidState {
                message: "child history unavailable".into(),
            })?;
        let root =
            registry
                .agent_artifact_root(id)
                .ok_or_else(|| RuntimeClientError::UnknownAgent {
                    agent_id: id.clone(),
                })?;
        let bytes = crate::tools::artifacts::ArtifactStore::read_bounded_from(&root, artifact_id)
            .map_err(|_| RuntimeClientError::InvalidState {
            message: "child artifact unavailable or exceeds 256 KiB".into(),
        })?;
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    }
    /// Read a bounded conversation-owned artifact without exposing its path.
    pub(crate) fn artifact_read(
        &self,
        id: &crate::runtime::identity::ArtifactId,
    ) -> Result<String, RuntimeClientError> {
        use base64::Engine;
        let runtime = self
            .runtime
            .as_ref()
            .ok_or_else(|| RuntimeClientError::InvalidState {
                message: "artifact runtime unavailable".into(),
            })?;
        let bytes = runtime
            .tool_runtime()
            .artifacts()
            .read_bounded(id)
            .map_err(|_| RuntimeClientError::InvalidState {
                message: "artifact unavailable or exceeds 256 KiB".into(),
            })?;
        Ok(base64::engine::general_purpose::STANDARD.encode(bytes))
    }

    /// Reads the authoritative session model state through the folded
    /// projection, so the value always agrees with the snapshot read model.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] when the
    /// observation stream is over.
    pub(crate) fn model_get(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.domain_page("model", 0, 32)
    }

    /// Replaces a standalone conversation's in-memory model configuration.
    /// Durable Session runtimes reject this control; their native Session owner
    /// must commit settings, retained binding, and runtime authority together.
    ///
    /// # Linearization
    ///
    /// The runtime resolves from an immutable capture off the coordinator
    /// lock, then fences and commits under the lock that also owns admission. An
    /// update therefore either linearizes before an admission (and that
    /// attempt observes it) or after it (and only later attempts observe
    /// it). There is no third possibility and no timing assumption.
    ///
    /// # Transactionality
    ///
    /// A rejected update changes nothing: the session keeps its previous
    /// configuration, no cursor is allocated, and no event is published.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::InvalidModelConfiguration`] when the
    /// configuration cannot be resolved against the catalog or cannot run
    /// under the session context policy, [`RuntimeClientError::InvalidState`]
    /// for a durable Session or while the runtime is not yet activated, and
    /// [`RuntimeClientError::ProjectionExhausted`] when the observation
    /// stream is over.
    pub(crate) fn model_set(
        &self,
        config: SessionModelConfig,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let state = self.lock_state();
        state.projection.snapshot_ref_checked()?;
        drop(state);
        self.runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime")
            .model_set(config)
            .map_err(|error| match error {
                ModelUpdateError::SessionOwned => RuntimeClientError::InvalidState {
                    message: "Session model selection requires the native Session owner".to_owned(),
                },
                ModelUpdateError::Conflict => RuntimeClientError::ConfigurationAdoption {
                    rejection:
                        crate::local_runtime::configuration::application::AdoptionError::Conflict,
                },
                ModelUpdateError::Inactive => RuntimeClientError::InvalidState {
                    message: "the conversation runtime is not activated".to_owned(),
                },
                ModelUpdateError::InvalidConfiguration(message) => {
                    RuntimeClientError::InvalidModelConfiguration { message }
                }
                ModelUpdateError::DurabilityFailed { message } => {
                    RuntimeClientError::InvalidState { message }
                }
            })?;
        Ok(RuntimeClientResult::ModelSet)
    }

    #[cfg(test)]
    pub(crate) fn job_status(
        &self,
        id: &ToolExecutionId,
    ) -> Result<super::snapshot::RuntimeClientJob, RuntimeClientError> {
        let snapshot = self
            .runtime
            .as_ref()
            .and_then(|r| r.background_status(id))
            .ok_or_else(|| RuntimeClientError::UnknownBackgroundExecution {
                execution_id: id.clone(),
            })?;
        Ok(background_view(&snapshot))
    }

    pub(crate) fn job_status_page(
        &self,
        id: &ToolExecutionId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        if !self
            .runtime
            .as_ref()
            .is_some_and(|runtime| runtime.background_registry().contains_execution(id))
        {
            return Err(RuntimeClientError::UnknownBackgroundExecution {
                execution_id: id.clone(),
            });
        }
        self.domain_page(&format!("jobs.{id}"), 0, 32)
    }

    pub(crate) fn job_list(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.domain_page("jobs", 0, 32)
    }

    pub(crate) fn job_cancel_intent(
        &self,
        id: &ToolExecutionId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let runtime = self
            .runtime
            .as_ref()
            .ok_or(RuntimeClientError::RuntimeShutdown)?;
        let job_id = runtime
            .background_cancel_intent(id)
            .map_err(|_| RuntimeClientError::RuntimeShutdown)?
            .ok_or_else(|| RuntimeClientError::UnknownBackgroundExecution {
                execution_id: id.clone(),
            })?;
        Ok(RuntimeClientResult::JobCancellationAccepted { job_id })
    }

    pub(crate) async fn job_wait(
        &self,
        id: &ToolExecutionId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let runtime = self.runtime.as_ref().ok_or_else(|| {
            RuntimeClientError::UnknownBackgroundExecution {
                execution_id: id.clone(),
            }
        })?;
        let snapshot = runtime
            .background_registry()
            .wait_until_terminal(id)
            .await
            .map_err(|error| match error {
                crate::tools::background::BackgroundWaitError::UnknownJob => {
                    RuntimeClientError::UnknownBackgroundExecution {
                        execution_id: id.clone(),
                    }
                }
                crate::tools::background::BackgroundWaitError::PublicationAbandoned => {
                    RuntimeClientError::JobPublicationAbandoned { job_id: id.clone() }
                }
            })?;
        Ok(RuntimeClientResult::JobOutcome {
            job_id: snapshot.execution_id.clone(),
            state: snapshot.state,
        })
    }

    pub(super) fn agent_registry(
        &self,
    ) -> Result<&crate::runtime::subagent::SubagentRegistry, RuntimeClientError> {
        self.runtime
            .as_ref()
            .and_then(|r| r.subagent_registry())
            .ok_or_else(|| RuntimeClientError::InvalidState {
                message: "Agent registry unavailable".into(),
            })
    }

    pub(super) fn agent_view(
        &self,
        id: &crate::runtime::identity::AgentId,
    ) -> Result<super::snapshot::RuntimeClientAgent, RuntimeClientError> {
        let registry = self.agent_registry()?;
        let (agent, activation) = registry.agent_snapshot_with_activation(id).ok_or_else(|| {
            RuntimeClientError::UnknownAgent {
                agent_id: id.clone(),
            }
        })?;
        Ok(super::projection::agent_view(&agent, &activation))
    }

    /// Resolve either native child owner for inspection only. Mutations still
    /// require the Registry's durable Agent record and reject Workflow executions.
    pub(super) fn child_view(
        &self,
        id: &crate::runtime::identity::AgentId,
    ) -> Result<super::snapshot::RuntimeClientAgent, RuntimeClientError> {
        use crate::runtime::subagent::{AgentState, SubagentPublication, SubagentState};
        let registry = self.agent_registry()?;
        if registry.agent_snapshot(id).is_some() {
            return self.agent_view(id);
        }
        let child = registry.workflow_inspection_snapshot(id).ok_or_else(|| {
            RuntimeClientError::UnknownAgent {
                agent_id: id.clone(),
            }
        })?;
        let state = if child.is_settled() {
            AgentState::Inactive
        } else if child.state.is_terminal()
            || child.settlement.publication == SubagentPublication::Abandoned
        {
            AgentState::Unavailable
        } else if child.state == SubagentState::Running {
            AgentState::Active
        } else {
            AgentState::Stopping
        };
        Ok(super::projection::activation_view(
            &child,
            state,
            child.state.is_active().then(|| child.subagent_id.clone()),
            &child.agent,
        ))
    }

    pub(crate) async fn agent_statistics(
        self: &Arc<Self>,
        id: &crate::runtime::identity::AgentId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let owner = Arc::clone(self);
        let id = id.clone();
        tokio::task::spawn_blocking(move || owner.read_agent_statistics(&id))
            .await
            .map_err(|_| RuntimeClientError::RuntimeFailure {
                message: "Agent statistics reader failed".to_owned(),
            })?
    }

    fn read_agent_statistics(
        &self,
        id: &crate::runtime::identity::AgentId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let agent = self.child_view(id)?;
        let store = self
            .agent_registry()?
            .transcript_store(&agent.activation_id)
            .map_err(|error| match error {
                crate::runtime::subagent::SubagentTranscriptError::Unknown(subagent_id) => {
                    RuntimeClientError::UnknownSubagent { subagent_id }
                }
                crate::runtime::subagent::SubagentTranscriptError::Unavailable(message) => {
                    RuntimeClientError::RuntimeFailure { message }
                }
            })?;
        let failed = RuntimeClientError::observation_store_error;
        let through = store.presentation_frontier().map_err(failed)?;
        let metrics = super::agent_statistics::read(&store, through, || {
            self.agent_registry()
                .ok()?
                .execution_started_at(&agent.activation_id)
        })
        .map_err(failed)?;
        Ok(RuntimeClientResult::AgentStatistics { metrics })
    }

    pub(crate) fn agent_status(
        &self,
        id: &crate::runtime::identity::AgentId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        if self.agent_registry()?.agent_snapshot(id).is_some() {
            return self.domain_page(&format!("agents.{id}"), 0, 32);
        }
        let child = self.child_view(id)?;
        let cut = self.lock_published_state().projection.checkpoint_read_cut();
        Ok(RuntimeClientResult::DomainPage {
            page: super::checkpoint::CheckpointPage {
                cursor: RuntimeClientCursor::new(cut.revision),
                read_cut: cut,
                domain: format!("workflow_children.{}", child.activation_id),
                total: 1,
                items: vec![collection_page_item(&child, id.to_string(), 0)],
                next_offset: None,
                collections: Vec::new(),
            },
        })
    }

    pub(crate) fn agent_list(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.domain_page("agents", 0, 32)
    }

    pub(crate) fn check_agent_message_target(
        &self,
        id: &crate::runtime::identity::AgentId,
    ) -> Result<(), RuntimeClientError> {
        self.agent_registry()?
            .check_message_target(id)
            .map_err(|error| agent_control_error(id, error))
    }

    pub(crate) async fn agent_send_message(
        &self,
        id: &crate::runtime::identity::AgentId,
        message: String,
        attachments: Vec<crate::message::content::UploadedFileRef>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let accepted = self
            .agent_registry()?
            .send_message_with_attachments(
                id,
                &message,
                &attachments,
                crate::runtime::subagent::AgentActivationOrigin::ClientControl,
                crate::runtime::cancellation::CancellationSignal::new(),
            )
            .await
            .map_err(|error| agent_control_error(id, error))?;
        Ok(RuntimeClientResult::AgentMessage { accepted })
    }

    pub(crate) fn agent_interrupt_intent(
        &self,
        id: &crate::runtime::identity::AgentId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        let activation_id = self
            .agent_registry()?
            .interrupt_agent_intent(id)
            .map_err(|error| agent_control_error(id, error))?;
        Ok(RuntimeClientResult::AgentInterruptionAccepted {
            agent_id: id.clone(),
            activation_id,
        })
    }

    pub(crate) async fn agent_wait(
        &self,
        id: &crate::runtime::identity::AgentId,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let registry = self.agent_registry()?;
        let result = registry
            .wait_agent(id)
            .await
            .map_err(|error| agent_control_error(id, error))?;
        let outcome = result.outcome.as_ref().map(|s| s.state);
        Ok(RuntimeClientResult::AgentWait {
            agent_id: result.agent_id,
            activation_id: result.activation_id,
            outcome,
        })
    }

    fn agent_reading_store(
        &self,
        id: &crate::runtime::identity::AgentId,
    ) -> Result<crate::durable::SqliteConversationStore, RuntimeClientError> {
        let registry = self.agent_registry()?;
        let agent = self.child_view(id)?;
        registry
            .transcript_store(&agent.activation_id)
            .map_err(|error| match error {
                crate::runtime::subagent::SubagentTranscriptError::Unknown(subagent_id) => {
                    RuntimeClientError::UnknownSubagent { subagent_id }
                }
                crate::runtime::subagent::SubagentTranscriptError::Unavailable(message) => {
                    RuntimeClientError::RuntimeFailure {
                        message: format!("subagent history unavailable: {message}"),
                    }
                }
            })
    }

    pub(crate) fn agent_turns(
        &self,
        id: &crate::runtime::identity::AgentId,
        offset: Option<usize>,
        limit: usize,
    ) -> Result<crate::runtime_client::turn::ObservedTurnPage, RuntimeClientError> {
        validate_transcript_page_limit(limit)?;
        self.agent_reading_store(id)?
            .observation_turns(offset.map(|value| value as u64), limit)
            .map_err(RuntimeClientError::observation_store_error)
    }

    pub(crate) fn agent_transcript_window(
        &self,
        id: &crate::runtime::identity::AgentId,
        at: &crate::durable::reading::ConversationWindowAt,
        limit: usize,
    ) -> Result<crate::runtime_client::snapshot::ConversationWindow, RuntimeClientError> {
        validate_transcript_page_limit(limit)?;
        read_conversation_window(&self.agent_reading_store(id)?, at, limit)
    }

    /// Drains the conversation runtime and returns only after it is
    /// quiescent. Runtime Client remains a control adapter: the semantic
    /// lifecycle and all settlement ownership remain in `ConversationRuntime`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::InvalidState`] while the runtime is
    /// not yet activated: an inert conversation has no runtime lifecycle
    /// to end, so the request is refused and nothing is published.
    pub(crate) async fn shutdown(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.ensure_writable_runtime()?;
        self.runtime
            .as_ref()
            .expect("a writable Runtime Client host has a runtime")
            .shutdown()
            .await
            .map_err(|error| match error {
                crate::runtime::conversation_runtime::ShutdownError::Inactive => {
                    RuntimeClientError::InvalidState {
                        message: "the conversation runtime is not activated".to_owned(),
                    }
                }
                crate::runtime::conversation_runtime::ShutdownError::RuntimeOwnedSettlement {
                    detail,
                } => RuntimeClientError::RuntimeFailure {
                    message: format!("runtime-owned shutdown settlement failed: {detail}"),
                },
            })?;
        Ok(RuntimeClientResult::ShutdownCompleted)
    }

    /// Fences every semantic mutation from a durable inspection host. The
    /// read-only host owns no coordinator, mailbox, registry, or lifecycle
    /// handle, so an inspection client cannot accidentally become an
    /// execution owner through a control request.
    fn ensure_writable_runtime(&self) -> Result<(), RuntimeClientError> {
        if self.read_only {
            return Err(RuntimeClientError::InvalidState {
                message: "conversation inspection is read-only".to_owned(),
            });
        }
        if self.runtime.is_none() {
            return Err(RuntimeClientError::InvalidState {
                message: "the conversation runtime is unavailable".to_owned(),
            });
        }
        Ok(())
    }
}

/// Builds the ordinary Runtime Client projection from one conversation's
/// durable authorities. Durable journal facts update the read model only;
/// they do not become live Runtime Client events or consume the live cursor
/// domain.
fn durable_projection(
    store: &dyn ConversationStore,
    replay_limit: usize,
) -> Result<RuntimeClientProjection, HostConstructionError> {
    let conversation_id = store.conversation_id().clone();
    let source_cut = store
        .observation_read_cut()
        .map_err(|error| HostConstructionError::Durable(error.to_string()))?;
    let through = source_cut.read.journal;
    let auxiliary_total = store
        .auxiliary_audit_total(crate::model::auxiliary::AuxiliaryAuditCut { journal: through })
        .map_err(|error| HostConstructionError::Durable(error.to_string()))?;
    if auxiliary_total > 9_007_199_254_740_991 {
        return Err(HostConstructionError::Durable(
            "auxiliary audit total exceeds the protocol integer domain".into(),
        ));
    }
    let capabilities = super::snapshot::CapabilityView {
        configured_tools: Vec::new(),
        revision: crate::runtime::identity::CapabilityRevision::new(0),
        tools: Vec::new(),
        available_tools: Vec::new(),
        skills: Vec::new(),
        sources: Vec::new(),
    };
    let mut projection = RuntimeClientProjection::new(
        conversation_id,
        Vec::new(),
        capabilities,
        None,
        replay_limit,
    );
    projection.set_auxiliary_audit_total(auxiliary_total);
    projection.set_settings_evidence(super::settings::SettingsEvidence::HistoricalPartial);
    // No runtime stands behind a durable projection, so there is no
    // authoritative Agent composition to project: `effective_plugins`
    // stays absent (Issue #256). It is deliberately *not* reconstructed from
    // the configuration document on disk today, from built-in defaults, or
    // from the Agent Status observations this journal replay does install —
    // historical evidence stays honest about what it cannot know.
    projection
        .initialize_descriptor_heads(store, source_cut.clone())
        .map_err(|error| HostConstructionError::Durable(format!("{error:?}")))?;
    let reading = durable_bootstrap::read(store, &source_cut)
        .map_err(|error| HostConstructionError::Durable(error.to_string()))?;
    projection.bootstrap_durable_reading(reading.attempt, reading.context, reading.tool_content);

    projection.set_inspection_frontier(through);
    // This local constructor has no worker. Resolve its bounded read domains
    // at the same native cut before the caller freezes the published seed.
    projection.initialize_read_domains(store);
    let references = projection
        .snapshot_ref()
        .messages
        .iter()
        .take(super::checkpoint::CHECKPOINT_PAGE_ITEMS)
        .flat_map(|message| super::checkpoint::message_metadata(store, message))
        .collect();
    projection.install_content_metadata(references);
    Ok(projection)
}

fn bootstrap_live_projection(
    seed: &crate::runtime::conversation_runtime::RuntimeBootstrapSnapshot,
    store: &dyn ConversationStore,
    replay_limit: usize,
) -> RuntimeClientProjection {
    seed.model.prepare();
    let resource_source =
        super::resource_source::ResourceReadSource::new(Arc::clone(&seed.resources));
    resource_source.prepare();
    let capability_source = super::capability_source::CapabilityReadSource::new(
        Arc::clone(&seed.capabilities),
        seed.capability_availability.clone(),
        seed.model.capabilities().cloned(),
    );
    capability_source.prepare();
    let mut projection = RuntimeClientProjection::new(
        seed.conversation_id.clone(),
        {
            let mut capture = super::checkpoint::Capture::new(0);
            capture.message_page("messages", &seed.messages, &seed.conversation_id)
        },
        capability_source.header(),
        seed.model.inline(),
        replay_limit,
    );
    let auxiliary_journal = seed
        .observation_cut
        .as_ref()
        .map_or(seed.journal_through, |cut| cut.read.journal);
    if let Ok(total) = store.auxiliary_audit_total(crate::model::auxiliary::AuxiliaryAuditCut {
        journal: auxiliary_journal,
    }) && total <= 9_007_199_254_740_991
    {
        projection.set_auxiliary_audit_total(total);
    }
    install_bootstrap_sources(&mut projection, seed, &resource_source, &capability_source);
    projection.bootstrap(seed);
    install_bootstrap_frontiers(&mut projection, seed, &resource_source, &capability_source);
    if seed.source_failure.is_none()
        && let Some(cut) = seed.observation_cut.clone()
    {
        if let Err(error) = projection.initialize_descriptor_heads(store, cut) {
            projection.mark_observation_unavailable(format!("{error:?}"));
        } else {
            projection.initialize_read_domains(store);
        }
    }
    if let Ok(checkpoint) = projection.checkpoint_candidate() {
        projection.install_domain_sources(weighted_sources(vec![(
            "context".into(),
            Arc::new(super::domain::DomainSource::Context(source_value(
                checkpoint.snapshot.context,
            ))),
        )]));
    }
    let references = seed
        .messages
        .iter()
        .take(super::checkpoint::CHECKPOINT_PAGE_ITEMS)
        .flat_map(|message| super::checkpoint::message_metadata(store, message))
        .collect();
    projection.install_content_metadata(references);
    projection
}

fn install_bootstrap_sources(
    projection: &mut RuntimeClientProjection,
    seed: &crate::runtime::conversation_runtime::RuntimeBootstrapSnapshot,
    resource_source: &super::resource_source::ResourceReadSource,
    capability_source: &super::capability_source::CapabilityReadSource,
) {
    projection.install_domain_sources(weighted_sources(vec![
        (
            "resources".into(),
            Arc::new(super::domain::DomainSource::Resources(source_value(
                resource_source.clone(),
            ))),
        ),
        (
            "model_catalog".into(),
            Arc::new(super::domain::DomainSource::Catalog(source_value(
                seed.model.catalog_source(),
            ))),
        ),
        (
            "model".into(),
            Arc::new(super::domain::DomainSource::Model(source_value(
                seed.model.clone(),
            ))),
        ),
        (
            "capabilities".into(),
            Arc::new(super::domain::DomainSource::Capability(source_value(
                capability_source.clone(),
            ))),
        ),
    ]));
    if let Some(goal) = &seed.goal {
        projection.install_domain_sources(weighted_sources(vec![(
            "goal".into(),
            Arc::new(super::domain::DomainSource::Goal(source_value(
                goal.clone(),
            ))),
        )]));
    }
    if let Some(todos) = &seed.todos {
        projection.install_domain_sources(weighted_sources(vec![(
            "todos".into(),
            Arc::new(super::domain::DomainSource::Todo(source_value(
                todos.clone(),
            ))),
        )]));
    }
}

fn install_bootstrap_frontiers(
    projection: &mut RuntimeClientProjection,
    seed: &crate::runtime::conversation_runtime::RuntimeBootstrapSnapshot,
    resource_source: &super::resource_source::ResourceReadSource,
    capability_source: &super::capability_source::CapabilityReadSource,
) {
    projection.install_source_frontiers(capability_source.frontiers());
    projection.install_source_frontiers(vec![super::checkpoint::CollectionFrontier {
        domain: "model_catalog.models".into(),
        revision: 0,
        total: seed.model.catalog_source().model_count() as u64,
        loaded: 0,
        next_offset: (seed.model.catalog_source().model_count() > 0).then_some(0),
    }]);
    projection.install_source_frontiers(resource_source.frontiers());
    projection.install_source_frontiers(vec![super::checkpoint::CollectionFrontier {
        domain: "model".into(),
        revision: 0,
        total: 1,
        loaded: u64::from(seed.model.inline().is_some()),
        next_offset: seed.model.inline().is_none().then_some(0),
    }]);
    projection.install_resource_header(resource_source.header());
}

fn install_bootstrap_bridge(
    runtime: &ConversationRuntime,
    pending: &Arc<PendingObservations>,
) -> Result<crate::runtime::conversation_runtime::RuntimeBootstrapSnapshot, HostConstructionError> {
    runtime
        .install_client_observation_bridge(Arc::clone(pending))
        .map_err(|error| {
            runtime.release_client_binding();
            match error {
                RuntimeBootstrapError::BridgeAlreadyInstalled { conversation_id } => {
                    HostConstructionError::ObservationBridgeAlreadyInstalled { conversation_id }
                }
                RuntimeBootstrapError::RuntimeAlreadyActivated { conversation_id } => {
                    HostConstructionError::RuntimeAlreadyActivated { conversation_id }
                }
                RuntimeBootstrapError::Durable(message) => HostConstructionError::Durable(message),
            }
        })
}

/// The Runtime Client host of one conversation.
///
/// Construct one host per conversation runtime instance; the host installs
/// the projection sink on the conversation runtime exactly once and claims
/// the one-time Runtime Client binding. The host is cheaply cloneable and
/// all clones share one state.
#[derive(Clone)]
pub struct RuntimeClientHost {
    pub(crate) inner: Arc<ClientInner>,
}

impl core::fmt::Debug for RuntimeClientHost {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("RuntimeClientHost")
            .field("conversation_id", &self.inner.conversation_id)
            .finish()
    }
}

impl RuntimeClientHost {
    /// Creates the host over one conversation runtime.
    ///
    /// # One conversation authority
    ///
    /// The conversation identity of the host *is*
    /// [`ConversationRuntime::conversation_id`]. The configuration carries
    /// no conversation id of its own, so the host's identity, the
    /// projection's conversation, and the `initialized` result all name the
    /// conversation of the runtime it observes.
    ///
    /// # One host per runtime identity
    ///
    /// Construction claims the one-time Runtime Client binding of the
    /// conversation tool runtime and of the capability coordinator. A
    /// second construction over the same runtime is rejected with
    /// [`HostConstructionError::RuntimeClientAlreadyBound`] rather than
    /// silently installing a second projection sink.
    ///
    /// The binding lasts for the runtime identity's lifetime and is not
    /// released when the bound host is dropped: reconnect belongs to
    /// attachments (detach, then a fresh
    /// [`RuntimeClientEndpoint`](super::endpoint::RuntimeClientEndpoint)
    /// `initialize`), not to host reconstruction.
    ///
    /// # Lifecycle
    ///
    /// A host binds **before** its conversation runtime is activated. The
    /// composition constructs the runtime, optionally binds this host, and
    /// then calls [`ConversationRuntime::activate`]; binding after
    /// activation is refused with
    /// [`HostConstructionError::RuntimeAlreadyActivated`]. App Server residency
    /// keeps this host bound with zero clients; lower-level runtime-only
    /// compositions may intentionally omit a client projection.
    ///
    /// # Bootstrap linearization
    ///
    /// After the binding claim the host performs exactly one fallible
    /// step: the runtime's observation bridge handshake
    /// ([`ConversationRuntime::install_observation_bridge`]), which
    /// installs the observation queue and every subsystem seam and
    /// captures the bootstrap snapshot at one global cut, under the one
    /// coordinator lock and over an inert runtime. The projection then
    /// mirrors that snapshot as pure seed state — publishing nothing and
    /// allocating no cursor — so the initial state plus the live
    /// observation stream is exactly one complete projection, with no lost
    /// transition, no duplicate, and no synthetic event for state that
    /// already existed. If the handshake fails, the binding claim is
    /// released and the failure is reported typed; a failed construction
    /// never leaves a claimed-but-invalid binding.
    ///
    /// # Errors
    ///
    /// Returns [`HostConstructionError::RuntimeClientAlreadyBound`] when
    /// the runtime identity is already bound to a Runtime Client host,
    /// [`HostConstructionError::RuntimeAlreadyActivated`] when the runtime
    /// has already been activated, and
    /// [`HostConstructionError::ObservationBridgeAlreadyInstalled`] when a
    /// headless observation bridge already exists over the runtime, or
    /// [`HostConstructionError::Durable`] when native durable bootstrap fails.
    pub fn new(config: RuntimeClientHostConfig) -> Result<Self, HostConstructionError> {
        Self::construct(config, None)
    }

    pub(crate) fn new_for_activation(
        config: RuntimeClientHostConfig,
        activation: crate::runtime::identity::SubagentId,
    ) -> Result<Self, HostConstructionError> {
        Self::construct(config, Some(activation))
    }

    /// Creates a read-only Runtime Client host over a known conversation's
    /// durable store.
    ///
    /// This is the generic read-only conversation-attachment primitive over
    /// durable authorities. It does not construct a [`ConversationRuntime`], install
    /// an observation bridge, claim a mailbox, or acquire an execution
    /// lifecycle. The store supplies the current Surface/transcript and the
    /// Event Journal is folded into the same ordinary Runtime Client
    /// projection used by a live host. No child-specific transcript payload
    /// or parent-side history is introduced.
    ///
    /// The attachment captures a single database read cut, then folds its
    /// finite Journal prefix. Concurrent writes cannot enter that seed or
    /// its decorations. No live event cursor is fabricated for inspection.
    ///
    /// # Errors
    ///
    /// Returns [`HostConstructionError::Durable`] when the durable
    /// conversation cannot provide a coherent current Surface, transcript,
    /// request-model, or Event Journal bootstrap.
    pub fn new_durable(
        store: Arc<dyn ConversationStore>,
        replay_limit: Option<usize>,
    ) -> Result<Self, HostConstructionError> {
        let replay_limit =
            replay_limit.unwrap_or(super::projection::RUNTIME_CLIENT_REPLAY_LIMIT_DEFAULT);
        let conversation_id = store.conversation_id().clone();
        let mut projection = durable_projection(store.as_ref(), replay_limit)?;
        projection.initialize_publication_view();

        let pending = Arc::new(PendingObservations::new());
        let inner = Arc::new(ClientInner {
            conversation_id,
            agent_id: crate::runtime::identity::AgentId::new(format!(
                "inspection:{}",
                store.conversation_id()
            )),
            runtime: None,
            inspection_capacity: Arc::new(tokio::sync::Semaphore::new(8)),
            read_store: store.presentation_reader(),
            store,
            read_only: true,
            state: Arc::new(Mutex::new(ClientState {
                projection,
                control_attachment: None,
                read_only_attachments: BTreeMap::new(),
                next_attachment_seq: 0,
            })),
            observation_preparation: Arc::new(Mutex::new(())),
            publication_gate: Arc::new(PublicationGate::default()),
            pending,
            worker_started: AtomicBool::new(false),
            worker: tokio::sync::Mutex::new(ProjectionWorker::default()),
            projection_read: Arc::new(tokio::sync::Mutex::new(None)),
            #[cfg(test)]
            trace_cut_hook: Mutex::new(None),
            #[cfg(test)]
            publication_commit_hook: Mutex::new(None),
            #[cfg(test)]
            audit_commit_hook: Mutex::new(None),
            #[cfg(test)]
            snapshot_read_hook: Mutex::new(None),
            #[cfg(test)]
            cancellation_receipt_hook: Mutex::new(None),
            #[cfg(test)]
            manual_compaction_response_gate: Mutex::new(None),
            #[cfg(test)]
            read_domain_hook: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            worker_turns: Arc::new(tokio::sync::watch::channel(0).0),
        });
        // Durable-only reads have no producer or followable stream. The
        // physical read closure owns all work; no detached worker is started.
        Ok(Self { inner })
    }

    fn construct(
        config: RuntimeClientHostConfig,
        activation: Option<crate::runtime::identity::SubagentId>,
    ) -> Result<Self, HostConstructionError> {
        // ---- Ownership commit: the one-time binding claim. ----
        //
        // The claim is the linearization point that gates every later
        // step: a second construction fails here before touching the
        // runtime, and this construction is the only one that can proceed.
        if !config.runtime.claim_client_binding() {
            return Err(HostConstructionError::RuntimeClientAlreadyBound {
                conversation_id: config.runtime.conversation_id().clone(),
            });
        }

        // ---- The one fallible step after the claim: the bridge handshake.
        //
        // The runtime installs the observation queue and every subsystem
        // observation seam and captures the bootstrap snapshot at one
        // global cut. On failure the claim is released: a rejected
        // construction must leave no trace.
        let replay_limit = config
            .replay_limit
            .unwrap_or(super::projection::RUNTIME_CLIENT_REPLAY_LIMIT_DEFAULT);
        let store = config.runtime.tool_runtime().durable_store();
        let pending = Arc::new(PendingObservations::new());
        let seed = install_bootstrap_bridge(&config.runtime, &pending)?;

        // ---- Infallible wiring: from here construction always succeeds. ----
        //
        // The projection mirrors the runtime's authoritative seed exactly
        // — current Surface working set, session model, capability snapshot, and
        // pending inbound — entirely as snapshot state. No seeded fact is
        // routed through `RuntimeClientProjection::apply`, so bootstrap
        // allocates no cursor and publishes no event: the first cursor
        // belongs to a real post-activation transition. (The background
        // seed is provably empty by the ownership-transfer invariant: a
        // `ConversationRuntime` is constructed only over a pristine
        // tool-runtime background plane, and the transfer then refuses
        // dispatch commits while its mailbox is bound inactive.)
        let mut projection = bootstrap_live_projection(&seed, store.as_ref(), replay_limit);
        // The effective native Agent Extension composition of the runtime
        // this host is bound to (Issue #256). It is read from the runtime's
        // own materialized extension owners, so a root host projects the
        // composition frozen at `LocalConversationCore::compose` and a
        // subagent-child host projects the one its `ResolvedSubagentSpec`
        // carried — with no configuration document, resource generation, or
        // Agent Status observation anywhere on the path.
        projection.set_effective_plugins(super::settings::EffectivePlugins::project(
            &config.runtime.native_extensions(),
        ));
        if config.runtime.model_is_frozen() {
            projection.set_settings_evidence(super::settings::SettingsEvidence::FrozenChild);
        }
        projection.bind_activation(activation);
        projection.initialize_publication_view();
        let inner = Arc::new(ClientInner {
            conversation_id: seed.conversation_id,
            agent_id: config.runtime.agent_id().clone(),
            runtime: Some(config.runtime),
            inspection_capacity: Arc::new(tokio::sync::Semaphore::new(8)),
            read_store: store.presentation_reader(),
            store,
            read_only: false,
            state: Arc::new(Mutex::new(ClientState {
                projection,
                control_attachment: None,
                read_only_attachments: BTreeMap::new(),
                next_attachment_seq: 0,
            })),
            observation_preparation: Arc::new(Mutex::new(())),
            publication_gate: Arc::new(PublicationGate::default()),
            pending,
            worker_started: AtomicBool::new(false),
            worker: tokio::sync::Mutex::new(ProjectionWorker::default()),
            projection_read: Arc::new(tokio::sync::Mutex::new(None)),
            #[cfg(test)]
            trace_cut_hook: Mutex::new(None),
            #[cfg(test)]
            publication_commit_hook: Mutex::new(None),
            #[cfg(test)]
            audit_commit_hook: Mutex::new(None),
            #[cfg(test)]
            snapshot_read_hook: Mutex::new(None),
            #[cfg(test)]
            cancellation_receipt_hook: Mutex::new(None),
            #[cfg(test)]
            manual_compaction_response_gate: Mutex::new(None),
            #[cfg(test)]
            read_domain_hook: Arc::new(Mutex::new(None)),
            #[cfg(test)]
            worker_turns: Arc::new(tokio::sync::watch::channel(0).0),
        });
        if let Some(runtime) = inner.runtime.as_ref() {
            runtime.install_publication_projection(Arc::new(NativePublicationProjection {
                inner: Arc::downgrade(&inner),
            }));
            runtime.set_interaction_provider_available(true);
            runtime.install_interaction_publication_authority(Arc::new(
                RootInteractionPublicationAuthority {
                    inner: Arc::downgrade(&inner),
                },
            ));
        }
        // Only the projection worker: activating the conversation runtime
        // is the composition's explicit next step, never a side effect of
        // binding a client.
        inner.ensure_worker();
        Ok(Self { inner })
    }

    /// Creates the host with the test-only projection linearization hooks
    /// installed. Only available under `#[cfg(test)]`; never used by
    /// production code.
    #[cfg(test)]
    pub(crate) fn with_probe(
        config: RuntimeClientHostConfig,
        probe: super::test_sync::ProjectionProbe,
    ) -> Result<Self, HostConstructionError> {
        let host = Self::new(config)?;
        host.inner
            .state
            .lock()
            .expect("host lock")
            .projection
            .install_probe(probe);
        Ok(host)
    }

    /// The conversation identity of this host.
    #[must_use]
    pub fn conversation_id(&self) -> &ConversationId {
        &self.inner.conversation_id
    }

    /// Creates the transport-neutral semantic endpoint of this runtime.
    ///
    /// This is an in-process typed request surface; no process serves it as a
    /// client protocol (App Server's stdio and WebSocket transports wrap
    /// `AppServerConnection`). This endpoint accepts every
    /// [`RuntimeClientRequest`](super::types::RuntimeClientRequest),
    /// including `initialize`, and returns the correlated
    /// [`RuntimeClientResponse`](super::types::RuntimeClientResponse). A
    /// transport therefore never performs protocol negotiation, attachment
    /// admission, identity allocation, or replacement/rejection semantics
    /// itself.
    #[must_use]
    pub fn endpoint(&self) -> super::endpoint::RuntimeClientEndpoint {
        if self.inner.read_only {
            super::endpoint::RuntimeClientEndpoint::new_read_only(self)
        } else {
            super::endpoint::RuntimeClientEndpoint::new(self)
        }
    }

    /// Admits one attachment: the internal primitive behind the
    /// `initialize` protocol method.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::UnsupportedProtocolVersion`] for an
    /// unsupported version, [`RuntimeClientError::AttachmentInUse`] when the
    /// control attachment is active, and
    /// [`RuntimeClientError::ProjectionExhausted`] once the observation
    /// stream is over.
    pub fn attach(
        &self,
        protocol_version: u16,
    ) -> Result<(super::attachment::RuntimeAttachment, RuntimeClientResult), RuntimeClientError>
    {
        self.inner.attach(protocol_version)
    }

    /// Admits one explicitly read-only attachment to the live projection.
    ///
    /// The attachment can read the same projection as the Runtime Client
    /// control owner, including disposable live state, but every semantic
    /// mutation is rejected before it reaches the conversation runtime.
    #[cfg(test)]
    pub(crate) fn attach_read_only(
        &self,
        protocol_version: u16,
    ) -> Result<(super::attachment::RuntimeAttachment, RuntimeClientResult), RuntimeClientError>
    {
        self.inner.attach_read_only(protocol_version)
    }

    /// Releases one attachment. Idempotent. Detach is never cancellation
    /// and never shutdown.
    pub fn detach(&self, attachment_id: &AttachmentId) {
        self.inner.detach(attachment_id);
    }

    /// Submits one inbound user message through the conversation runtime's
    /// single publish path.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::InvalidRequest`] for empty content,
    /// [`RuntimeClientError::RuntimeShutdown`] after shutdown, and
    /// [`RuntimeClientError::InvalidState`] for a mailbox admission
    /// failure.
    pub fn submit_inbound(
        &self,
        content: Vec<crate::message::types::UserContentBlock>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.submit_inbound(content)
    }

    /// Requests cancellation of the current attempt.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::NoCurrentAttempt`] when no attempt
    /// is currently cancellable.
    pub fn cancel_current_attempt(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.cancel_current_attempt()
    }

    /// Manually compacts the current context while the runtime is idle.
    ///
    /// # Errors
    ///
    /// Returns a typed lifecycle, busy, context, or durability error. A
    /// successful response means the typed checkpoint already committed.
    pub async fn compact_context(
        &self,
        request_id: Option<crate::runtime::identity::ManualCompactionRequestId>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.compact_context(request_id).await
    }

    /// Responds to one live native interaction through Runtime Client
    /// semantics. The response is finite and cannot replace tool arguments.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::InteractionNotPending`] for a stale or
    /// settled identity, or [`RuntimeClientError::InteractionInvalidResponse`]
    /// for a response that fails bounded validation.
    pub async fn respond_interaction(
        &self,
        interaction: &InteractionRef,
        response: InteractionResponse,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.respond_interaction(interaction, response).await
    }

    /// Reads the authoritative snapshot and its cursor, linearized
    /// together.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] once the cursor
    /// space is exhausted.
    pub(crate) fn published_read_cut(
        &self,
    ) -> Result<super::checkpoint::CheckpointReadCut, RuntimeClientError> {
        let state = self.inner.state.lock().expect("runtime client state lock");
        state.projection.validate_published_source()?;
        let cut = state.projection.checkpoint_read_cut();
        if cut.source_cut.is_none() {
            return Err(RuntimeClientError::ReadCutExpired);
        }
        Ok(cut)
    }

    /// # Errors
    ///
    /// Returns a native source/cut error when the captured checkpoint cannot be served.
    /// A failed derived Source may request one worker retry within the shared
    /// checkpoint deadline. This synchronous call must leave the existing
    /// worker runnable (use a blocking lane when called from an async executor).
    pub fn checkpoint(
        &self,
    ) -> Result<super::checkpoint::RuntimeClientCheckpoint, RuntimeClientError> {
        self.inner.checkpoint()
    }
    /// Materializes bounded Trace descriptors at the same native checkpoint cut.
    /// # Errors
    /// Invalid selections, expired sources, and exhausted cuts are explicit.
    pub fn checkpoint_with_trace(
        &self,
        records: &[super::trace::TraceCursor],
    ) -> Result<super::checkpoint::RuntimeClientCheckpoint, RuntimeClientError> {
        self.inner.checkpoint_with_trace(records)
    }

    /// Returns a durable request-history read handle owned by the
    /// conversation runtime.
    #[must_use]
    pub fn request_history(&self) -> RequestHistory {
        self.inner.request_history()
    }

    /// Reads one bounded durable transcript page. `before` is exclusive and
    /// means that the returned page contains only older transcript entries.
    /// This cursor is independent from the live Runtime Client event cursor.
    ///
    /// # Errors
    ///
    /// Returns a typed Runtime Client error for an invalid limit, a durable
    /// read failure, or an invalid resolved transcript item.
    pub fn transcript_page(
        &self,
        before: Option<RuntimeClientTranscriptCursor>,
        limit: usize,
    ) -> Result<RuntimeClientTranscriptPage, RuntimeClientError> {
        match self.inner.transcript_page(before, limit)? {
            RuntimeClientResult::TranscriptPage { page } => Ok(page),
            other => Err(RuntimeClientError::RuntimeFailure {
                message: format!("unexpected transcript page result: {other:?}"),
            }),
        }
    }

    /// Reconstructs one retained provider-neutral request from durable facts.
    ///
    /// # Errors
    ///
    /// Returns a lookup or historical reconstruction error for an unknown or
    /// invalid request.
    pub fn reconstruct_request(
        &self,
        identity: &RequestIdentity,
    ) -> Result<ModelRequest, RequestHistoryError> {
        self.inner.reconstruct_request(identity)
    }

    /// Subscribes one attachment to the observation stream after a
    /// serviceable cursor.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::NotAttached`] for an unknown
    /// attachment identity and [`RuntimeClientError::ResyncRequired`] for
    /// an unserviceable cursor.
    pub fn subscribe_events(
        &self,
        attachment_id: &AttachmentId,
        after_cursor: RuntimeClientCursor,
    ) -> Result<(EventSubscription, RuntimeClientResult), RuntimeClientError> {
        self.inner.subscribe_events(attachment_id, after_cursor)
    }

    /// Reads the active capability projection.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] once the
    /// observation stream is over.
    pub fn capability(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.capability()
    }

    /// Reads the safe public model catalog.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] when the
    /// observation stream is over.
    pub fn model_catalog(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.model_catalog()
    }

    /// Reads the authoritative session model state through the folded
    /// projection.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] when the
    /// observation stream is over.
    pub fn model_get(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.model_get()
    }

    /// Replaces a standalone conversation's in-memory model configuration.
    /// Durable Session runtimes reject this control; their native Session owner
    /// must commit settings, retained binding, and runtime authority together.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::InvalidModelConfiguration`] when the
    /// configuration cannot be resolved against the catalog or cannot run
    /// under the session context policy, [`RuntimeClientError::InvalidState`]
    /// for a durable Session or inactive runtime, and
    /// [`RuntimeClientError::ProjectionExhausted`] when the observation
    /// stream is over.
    pub fn model_set(
        &self,
        config: SessionModelConfig,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.model_set(config)
    }

    /// Drains the local runtime and completes only at runtime quiescence.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::InvalidState`] while the runtime is
    /// not yet activated.
    pub async fn shutdown(&self) -> Result<RuntimeClientResult, RuntimeClientError> {
        self.inner.shutdown().await
    }

    /// Convenience for tests: host-level conversation-state accessors that
    /// forward to the conversation runtime.
    #[cfg(test)]
    pub(crate) fn host_ledger(&self) -> Option<Vec<crate::message::types::MessageBlock>> {
        self.inner
            .runtime
            .as_ref()
            .and_then(ConversationRuntime::coordinator_ledger)
    }

    /// The runtime's active Surface identities, or `None` while an attempt
    /// owns the conversation state.
    #[cfg(test)]
    pub(crate) fn host_active_ids(&self) -> Option<Vec<crate::runtime::identity::MessageId>> {
        self.inner
            .runtime
            .as_ref()
            .and_then(ConversationRuntime::coordinator_active_ids)
    }

    #[cfg(test)]
    #[allow(dead_code)] // used by the race regression tests
    pub(crate) fn has_current_attempt(&self) -> bool {
        self.inner
            .runtime
            .as_ref()
            .is_some_and(ConversationRuntime::has_current_attempt)
    }

    /// Injects one derived Source failure at the current native dependency fence.
    #[cfg(test)]
    pub(crate) fn fail_projection_read_for_test(&self, message: &str) {
        commit_display(
            &self.inner.state,
            &self.inner.publication_gate,
            |projection| {
                projection.request_read_domains_for_test();
                let cut = projection.read_domain_fence();
                assert!(projection.install_read_domains(
                    &cut,
                    Err(crate::durable::ConversationStoreError::InvalidReference(
                        message.into()
                    ),),
                    true
                ));
            },
        );
    }

    #[cfg(test)]
    pub(crate) fn gate_projection_read_for_test(
        &self,
        entered: tokio::sync::oneshot::Sender<()>,
        cancelled: tokio::sync::oneshot::Sender<()>,
        release: std::sync::mpsc::Receiver<()>,
    ) {
        let reader = self.inner.read_store.clone();
        *self.inner.read_domain_hook.lock().unwrap() = Some(Box::new(move || {
            reader.park_presentation_read_until_cancelled(entered);
            let _ = cancelled.send(());
            let _ = release.recv();
            Ok(())
        }));
        self.inner
            .lock_state()
            .projection
            .request_read_domains_for_test();
        self.inner
            .pending
            .push(ConversationObservation::JournalSourceReady);
    }

    #[cfg(test)]
    pub(crate) fn weak_inner(&self) -> Weak<ClientInner> {
        Arc::downgrade(&self.inner)
    }

    /// A non-owning handle to the shared conversation runtime state, for
    /// lifetime tests.
    #[cfg(test)]
    pub(crate) fn weak_runtime_inner(
        &self,
    ) -> Weak<crate::runtime::conversation_runtime::RuntimeInner> {
        self.inner
            .runtime
            .as_ref()
            .expect("live host has a conversation runtime")
            .weak_inner()
    }

    /// Waits until the projection has nothing left to publish without new
    /// input (see [`ClientInner::projection_settled`]).
    #[cfg(test)]
    pub(crate) async fn projection_settled(&self) {
        self.inner.projection_settled().await;
    }

    /// Installs the deterministic worker-exit signal of the projection
    /// worker, for lifetime tests.
    #[cfg(test)]
    pub(crate) fn install_worker_exit_probe(&self, sender: std::sync::mpsc::Sender<()>) {
        self.inner.pending.install_worker_exit_probe(sender);
    }
}

/// The construction-time configuration of one Runtime Client host.
pub struct RuntimeClientHostConfig {
    /// The conversation runtime this host observes and controls.
    ///
    /// This is the one conversation authority of the runtime; the host
    /// derives its identity, its snapshot seed, and every control outcome
    /// from it.
    pub runtime: ConversationRuntime,
    /// The bounded projection replay retention; the default is used when
    /// omitted. This cache is not the durable Event Journal.
    pub replay_limit: Option<usize>,
}

/// One delivery of the Runtime Client observation stream.
///
/// Delivery is explicit in all four terminal shapes: a client can never
/// confuse "nothing yet" with "the stream ended" or, critically, with
/// "events were skipped".
// The event variant is the overwhelmingly common one and is produced once
// per delivered event; boxing it would add an allocation to every delivery
// to shrink a short-lived stack value.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
pub enum EventDelivery {
    /// The next event, at its published cursor. Cursors delivered to one
    /// subscription are strictly contiguous within the retained stream.
    Event(RuntimeClientProtocolEvent),
    /// Nothing has been published after the subscription's cursor yet.
    /// Only [`EventSubscription::try_next`] returns this.
    Pending,
    /// The subscription was released (detach, re-subscription, or a
    /// dropped handle). The stream is over.
    Closed,
    /// The subscriber fell behind the bounded retention: the events it
    /// still needed were evicted from the replay ring. This is reported
    /// explicitly instead of skipping the gap, and it is stable — the
    /// client must re-subscribe (or take a fresh snapshot) to continue.
    ResyncRequired {
        /// The cursor the subscription consumed through.
        after_cursor: RuntimeClientCursor,
        /// The oldest cursor the runtime can still serve.
        earliest_serviceable: RuntimeClientCursor,
    },
    /// The native source is unavailable; no cursor is consumed or skipped.
    Unavailable { message: String },
    /// The cursor space is exhausted; nothing further will be published.
    Exhausted,
}

/// The shared registration of one event subscription.
///
/// Dropping the last handle removes the registration from the projection,
/// which is the same release an explicit detach performs.
struct SubscriptionInner {
    /// The host whose projection owns the registration.
    host: Weak<ClientInner>,
    /// The opaque registration identity.
    subscriber_id: u64,
    /// The edge-triggered wakeup handle of this subscriber.
    notify: Arc<tokio::sync::Notify>,
}

impl Drop for SubscriptionInner {
    fn drop(&mut self) {
        if let Some(host) = self.host.upgrade() {
            let mut state = host.state.lock().expect("native projection lock");
            state.projection.remove_subscriber(self.subscriber_id);
        }
    }
}

/// The live delivery handle of one event subscription.
///
/// The handle owns **no** event buffer. It is a registration identity plus
/// a wakeup handle over the projection's one bounded replay ring: reads
/// pull the next retained event by cursor under the host lock. A stalled
/// consumer therefore costs one cursor, never a growing queue, and can
/// never make the runtime drop events silently — falling behind retention
/// surfaces as [`EventDelivery::ResyncRequired`].
///
/// Cloning shares one registration; the registration is released when the
/// last clone drops.
#[derive(Clone)]
pub struct EventSubscription {
    inner: Arc<SubscriptionInner>,
}

impl core::fmt::Debug for EventSubscription {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EventSubscription")
            .field("subscriber_id", &self.inner.subscriber_id)
            .finish()
    }
}

impl EventSubscription {
    /// Whether two handles share one projection registration.
    ///
    /// A re-subscription installs a *new* registration and closes the previous
    /// one. A consumer parked on the previous handle therefore observes
    /// [`EventDelivery::Closed`], which means "this registration was replaced",
    /// not "this attachment ended". Only identity can tell those apart.
    #[must_use]
    pub fn same_registration(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }

    /// Polls the projection once for the next retained event.
    fn poll(&self) -> EventDelivery {
        let Some(host) = self.inner.host.upgrade() else {
            return EventDelivery::Closed;
        };
        let mut state = host.state.lock().expect("native projection lock");
        match state.projection.poll_subscriber(self.inner.subscriber_id) {
            SubscriberPoll::Event(event) => EventDelivery::Event(event),
            SubscriberPoll::Pending => EventDelivery::Pending,
            SubscriberPoll::Closed => EventDelivery::Closed,
            SubscriberPoll::Unavailable { message } => EventDelivery::Unavailable { message },
            SubscriberPoll::Lagged {
                after_cursor,
                earliest_serviceable,
            } => EventDelivery::ResyncRequired {
                after_cursor,
                earliest_serviceable,
            },
            SubscriberPoll::Exhausted => EventDelivery::Exhausted,
        }
    }

    /// Waits for the next delivery of the observation stream.
    ///
    /// Never returns [`EventDelivery::Pending`]: it parks on the
    /// subscriber's wakeup handle until an event, a closure, a lag, or
    /// exhaustion is observable. Parking holds no lock.
    pub async fn next(&self) -> EventDelivery {
        loop {
            match self.poll() {
                EventDelivery::Pending => {}
                delivery => return delivery,
            }
            // `Notify::notify_one` stores one permit even with no waiter,
            // so a publication between the poll above and this await is
            // never missed.
            self.inner.notify.notified().await;
        }
    }

    /// Polls for the next delivery without waiting.
    #[must_use]
    pub fn try_next(&self) -> EventDelivery {
        self.poll()
    }
}

/// Shared parameter authority, checked before resolving child ownership or storage.
fn agent_control_error(
    id: &crate::runtime::identity::AgentId,
    error: crate::runtime::subagent::AgentControlError,
) -> RuntimeClientError {
    match error {
        crate::runtime::subagent::AgentControlError::NotDelivered => {
            RuntimeClientError::AgentNotDelivered {
                agent_id: id.clone(),
            }
        }
        crate::runtime::subagent::AgentControlError::DeliveryUnknown => {
            RuntimeClientError::AgentDeliveryUnknown {
                agent_id: id.clone(),
            }
        }
        crate::runtime::subagent::AgentControlError::Stopping => {
            RuntimeClientError::AgentStopping {
                agent_id: id.clone(),
            }
        }
        crate::runtime::subagent::AgentControlError::Unknown(agent_id) => {
            RuntimeClientError::UnknownAgent { agent_id }
        }
        crate::runtime::subagent::AgentControlError::Settlement => {
            RuntimeClientError::AgentSettlement {
                agent_id: id.clone(),
            }
        }
        error => RuntimeClientError::InvalidState {
            message: error.to_string(),
        },
    }
}

fn validate_transcript_page_limit(limit: usize) -> Result<(), RuntimeClientError> {
    if limit == 0 || limit > TRANSCRIPT_PAGE_LIMIT_MAX {
        return Err(RuntimeClientError::InvalidRequest {
            message: format!(
                "transcript page limit must be between 1 and {TRANSCRIPT_PAGE_LIMIT_MAX}"
            ),
        });
    }
    Ok(())
}

/// Shared durable transcript projection for root and exact owned children.
fn read_transcript_page(
    store: &dyn crate::durable::ConversationStore,
    before: Option<RuntimeClientTranscriptCursor>,
    limit: usize,
    frontier: impl FnOnce()
        -> Result<crate::durable::observation::ObservationReadCut, RuntimeClientError>,
) -> Result<RuntimeClientTranscriptPage, RuntimeClientError> {
    let cut = frontier()?;
    let headers = store
        .transcript_header_page(
            &cut,
            before.map(Into::into),
            limit.min(super::checkpoint::CHECKPOINT_PAGE_ITEMS),
        )
        .map_err(|error| RuntimeClientError::RuntimeFailure {
            message: error.to_string(),
        })?;
    let mut page = super::snapshot::transcript_header_view(headers);
    let through = cut.read.journal;
    super::response::decorate_through(store, &mut page, through).map_err(|error| {
        RuntimeClientError::RuntimeFailure {
            message: error.to_string(),
        }
    })?;
    Ok(page)
}

/// Both parent and child navigation read from their own canonical store.
fn read_conversation_window(
    store: &dyn crate::durable::ConversationStore,
    at: &crate::durable::reading::ConversationWindowAt,
    limit: usize,
) -> Result<crate::runtime_client::snapshot::ConversationWindow, RuntimeClientError> {
    let failed = RuntimeClientError::observation_store_error;
    let read = store
        .read_window_headers(at, limit.min(super::checkpoint::CHECKPOINT_PAGE_ITEMS))
        .map_err(failed)?;
    let mut page = super::snapshot::transcript_header_view(read.page);
    super::response::decorate_window(store, &mut page, read.cut.read.journal).map_err(failed)?;
    if !read
        .cut
        .read
        .reconstructible_from(&store.conversation_read_cut().map_err(failed)?)
    {
        return Err(RuntimeClientError::InvalidState {
            message: "conversation history was edited during the read; read it again".into(),
        });
    }
    Ok(crate::runtime_client::snapshot::ConversationWindow {
        cut: read.cut.read,
        page,
        newer_cursor: read.newer_cursor.map(Into::into),
        target: read.target,
        target_cursor: read.target_cursor.map(Into::into),
    })
}

/// Native metadata keys have fixed names; numeric segments select bounded
/// source ordinals, never an ambiguous opaque JSON object key.
fn source_domain_path(domain: &str) -> Option<(Vec<String>, bool)> {
    if matches!(
        domain,
        "model" | "capabilities" | "resources" | "attempt.model" | "workflows"
    ) {
        return Some((Vec::new(), true));
    }
    if domain == "resources.inspection" {
        return Some((vec!["inspection".into()], true));
    }
    let relative = domain
        .strip_prefix("model.")
        .or_else(|| domain.strip_prefix("attempt.model."))
        .or_else(|| domain.strip_prefix("capabilities."))
        .or_else(|| domain.strip_prefix("resources."))
        .or_else(|| domain.strip_prefix("model_catalog."))
        .or_else(|| domain.strip_prefix("workflows."))?;
    Some((
        relative
            .split('.')
            .map(|part| {
                if part.parse::<u64>().is_ok() {
                    format!("@{part}")
                } else {
                    part.to_owned()
                }
            })
            .collect(),
        false,
    ))
}

#[cfg(test)]
mod tests {
    mod pure_read_tests {
        include!("host/pure_read_tests.rs");
    }
    mod initial_cut_tests {
        include!("host/initial_cut_tests.rs");
    }
    mod compaction_head_tests {
        include!("host/compaction_head_tests.rs");
    }
    mod source_recovery_tests {
        include!("host/source_recovery_tests.rs");
    }
    mod native_cancellation_tests {
        include!("host/native_cancellation_tests.rs");
    }
    mod settlement_tests {
        include!("host/settlement_tests.rs");
    }
    mod member_tests {
        include!("host/member_tests.rs");
    }
    mod owner_tests {
        include!("host/owner_tests.rs");
    }
    mod delivery_tests {
        include!("host/delivery_tests.rs");
    }
    #[test]
    fn permanent_agent_settlement_is_a_typed_non_retryable_error() {
        let id = crate::runtime::identity::AgentId::new("agent-unavailable");
        assert!(
            matches!(super::agent_control_error(&id, crate::runtime::subagent::AgentControlError::NotDelivered), crate::runtime_client::types::RuntimeClientError::AgentNotDelivered { agent_id } if agent_id == id)
        );
        assert!(
            matches!(super::agent_control_error(&id, crate::runtime::subagent::AgentControlError::DeliveryUnknown), crate::runtime_client::types::RuntimeClientError::AgentDeliveryUnknown { agent_id } if agent_id == id)
        );
        assert!(matches!(
            super::agent_control_error(&id, crate::runtime::subagent::AgentControlError::Settlement),
            crate::runtime_client::types::RuntimeClientError::AgentSettlement { agent_id } if agent_id == id
        ));
        assert!(matches!(
            super::agent_control_error(&id, crate::runtime::subagent::AgentControlError::Stopping),
            crate::runtime_client::types::RuntimeClientError::AgentStopping { agent_id } if agent_id == id
        ));
    }

    use crate::runtime::observation::ConversationObservation;
    use std::collections::VecDeque;
    use std::sync::{Arc, Mutex};

    use tokio::sync::watch;

    use super::{
        EventDelivery, EventSubscription, ProjectionWorker, RuntimeClientHost,
        RuntimeClientHostConfig,
    };
    use crate::context::{
        AgentStatusClock, AgentStatusConfig, AgentStatusEngine, DefaultTokenEstimator,
        TokenEstimator,
    };
    use crate::durable::{ConversationStore, SqliteConversationStore};
    use crate::events::types::{EVENT_SCHEMA_VERSION, RuntimeEvent, RuntimeEventEnvelope};
    use crate::message::content::TextBlock;
    use crate::message::types::{
        AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, MessageBlock,
        ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::model::adapter::{ModelAdapter, ModelStream};
    use crate::model::error::{ModelError, ModelErrorKind};
    use crate::model::event::ModelEvent;
    use crate::model::finish::ModelFinishReason;
    use crate::model::types::{ModelProtocol, ModelRequest};
    use crate::runtime::cancellation::CancellationSignal;
    use crate::runtime::conversation_runtime::{
        ConversationContextConfig, ConversationRuntime, ConversationRuntimeError, CoordinatorProbe,
        InboundAdmissionError, ModelUpdateError, RuntimeConversationConfig,
    };
    use crate::runtime::identity::{
        AgentId, AttemptId, ConversationId, EventId, MessageId, ToolCallId, ToolExecutionId, ToolId,
    };
    use crate::runtime::request_history::RequestHistory;
    use crate::runtime::types::RuntimeClock;
    use crate::runtime_client::event::RuntimeClientEvent;
    use crate::runtime_client::host::HostConstructionError;
    use crate::runtime_client::snapshot::RuntimeClientAttemptPhase;
    use crate::runtime_client::types::{
        RuntimeClientCursor, RuntimeClientError, RuntimeClientProtocolEvent, RuntimeClientRequest,
        RuntimeClientResult,
    };
    use crate::scripted_suites::support::model::scripted_session_model;
    use crate::tools::background::{
        BackgroundDispatchError, BackgroundDispatchOutcome, BackgroundExecutionSnapshot,
        BackgroundLifecycle, ConversationBackgroundRegistry,
    };
    use crate::tools::deadline::ToolProgressCapability;
    use crate::tools::executor::{ToolExecutionContext, ToolExecutor, ToolRegistry};
    use crate::tools::types::{
        ToolCall, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolExecutionResult,
        ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolOrigin, ToolReplayPolicy,
        ToolResultContent,
    };

    fn test_resources(
        capability: &crate::capabilities::CapabilityCoordinator,
    ) -> Arc<crate::runtime::RuntimeResourceSnapshot> {
        Arc::new(crate::runtime::RuntimeResourceSnapshot::new(
            crate::runtime::RuntimeResourceRevision::new(1),
            Vec::new(),
            None,
            crate::context::ContextAssembly::new(),
            capability.current_snapshot(),
        ))
    }

    fn test_resource_loader(
        capability: &crate::capabilities::CapabilityCoordinator,
    ) -> Arc<dyn crate::runtime::RuntimeResourceLoader> {
        Arc::new(crate::runtime::FilesystemRuntimeResourceLoader::new(
            capability.current_snapshot().workspace_root(),
        ))
    }

    fn request_snapshots(history: &RequestHistory) -> Vec<crate::model::RequestSnapshot> {
        let mut snapshots = Vec::new();
        let mut cursor = None;
        loop {
            let page = history.page(cursor, 32).expect("request snapshot page");
            if page.snapshots.is_empty() {
                break;
            }
            cursor = page.next_sequence;
            snapshots.extend(page.snapshots);
        }
        snapshots
    }

    /// One scripted step of the gated adapter.
    enum GatedStep {
        /// Yield one canonical model event.
        Emit(ModelEvent),
        /// Wait until the shared watch releases, then continue without
        /// yielding; if the attempt cancellation fires first, fail with a
        /// cancelled model error like a real adapter.
        ParkUntilReleased(watch::Receiver<bool>),
    }

    /// A scripted cancellation-aware model adapter: one script per
    /// invocation, with deterministic park points.
    struct GatedAdapter {
        scripts: Mutex<VecDeque<VecDeque<GatedStep>>>,
        requests: Arc<Mutex<Vec<ModelRequest>>>,
        request_count: Arc<watch::Sender<usize>>,
    }

    impl GatedAdapter {
        fn new(scripts: Vec<Vec<GatedStep>>) -> Self {
            let (request_count, _receiver) = watch::channel(0);
            Self {
                scripts: Mutex::new(scripts.into_iter().map(VecDeque::from).collect()),
                requests: Arc::new(Mutex::new(Vec::new())),
                request_count: Arc::new(request_count),
            }
        }

        fn requests(&self) -> Vec<ModelRequest> {
            self.requests.lock().expect("requests lock").clone()
        }

        fn request_count(&self) -> watch::Receiver<usize> {
            self.request_count.subscribe()
        }
    }

    impl ModelAdapter for GatedAdapter {
        fn protocol(&self) -> ModelProtocol {
            ModelProtocol::OpenAiChatCompletions
        }

        fn stream(&self, request: ModelRequest, cancellation: CancellationSignal) -> ModelStream {
            let request_count = {
                let mut requests = self.requests.lock().expect("requests lock");
                requests.push(request);
                requests.len()
            };
            self.request_count.send_replace(request_count);
            let script = self
                .scripts
                .lock()
                .expect("scripts lock")
                .pop_front()
                .unwrap_or_default();
            Box::pin(futures_util::stream::unfold(
                (script, cancellation),
                |(mut script, cancellation)| async move {
                    loop {
                        match script.pop_front() {
                            None => return None,
                            Some(GatedStep::Emit(event)) => {
                                return Some((
                                    crate::model::adapter::ModelStreamItem::Event(event),
                                    (script, cancellation),
                                ));
                            }
                            Some(GatedStep::ParkUntilReleased(mut release)) => {
                                tokio::select! {
                                    biased;
                                    () = cancellation.cancelled() => {
                                        return Some((crate::model::adapter::ModelStreamItem::Event(ModelEvent::Failed {
                                            error: ModelError {
                                                kind: ModelErrorKind::Cancelled,
                                                message: "cancelled while parked".to_owned(),
                                                retry_disposition: crate::model::error::ModelRetryDisposition::Never,
                                                retry_after_ms: None,
                                                provider_code: None,
                                                context_overflow: None,
                                                malformed_tool_proposal: None,
                                                timeout_phase: None,
                                                generation: None,
                                            },
                                        }), (VecDeque::new(), cancellation)));
                                    }
                                    result = release.wait_for(|released| *released) => {
                                        result.expect("release channel stays open");
                                    }
                                }
                            }
                        }
                    }
                },
            ))
        }
    }

    fn model_release() -> (watch::Sender<bool>, watch::Receiver<bool>) {
        watch::channel(false)
    }

    /// A parking background executor: its returned future reports entry,
    /// waits on a durable release state, then settles with a fixed result.
    struct ParkingBackgroundTool {
        #[allow(dead_code)] // the definition documents the tool identity
        definition: ToolDefinition,
        started: watch::Sender<bool>,
        release: watch::Sender<bool>,
        execution_gate: Option<watch::Receiver<bool>>,
    }

    impl ParkingBackgroundTool {
        fn new() -> (Self, watch::Receiver<bool>, watch::Sender<bool>) {
            Self::build(None)
        }

        /// Builds a fixture whose returned future waits at an explicit gate
        /// before it observes `release`. This is used by the lost-wakeup
        /// regression to force release-before-wait ordering.
        fn new_with_execution_gate() -> (
            Self,
            watch::Receiver<bool>,
            watch::Sender<bool>,
            watch::Sender<bool>,
        ) {
            let (execution_gate, execution_gate_rx) = watch::channel(false);
            let (tool, started, release) = Self::build(Some(execution_gate_rx));
            (tool, started, release, execution_gate)
        }

        fn build(
            execution_gate: Option<watch::Receiver<bool>>,
        ) -> (Self, watch::Receiver<bool>, watch::Sender<bool>) {
            let (started, started_rx) = watch::channel(false);
            let (release, _release_rx) = watch::channel(false);
            (
                Self {
                    definition: ToolDefinition {
                        id: ToolId::new("tool-bg"),
                        name: "bg".to_owned(),
                        description: String::new(),
                        input_schema: serde_json::json!({"type": "object"}),
                        execution_policy: ToolExecutionPolicy::ModelSelectable,
                        concurrency_policy: ToolConcurrencyPolicy::Sequential,
                        approval_policy: crate::tools::types::ToolApprovalPolicy::Never,
                        replay_policy: ToolReplayPolicy::Never,
                        origin: ToolOrigin::Builtin,
                    },
                    started,
                    release: release.clone(),
                    execution_gate,
                },
                started_rx,
                release,
            )
        }
    }

    impl ToolExecutor for ParkingBackgroundTool {
        fn start<'a>(
            &'a self,
            _invocation: ToolInvocation,
            context: ToolExecutionContext<'a>,
        ) -> crate::tools::executor::ToolExecutionHandle<'a> {
            let started = self.started.clone();
            let mut execution_gate = self.execution_gate.clone();
            let mut release = self.release.subscribe();
            crate::tools::executor::ToolExecutionHandle::settled_by_operation(
                Box::pin(async move {
                    // This signal is published by the returned future, so
                    // observing it means the deterministic execution gate has
                    // actually been entered rather than merely returned by
                    // `start`.
                    started.send_replace(true);
                    if let Some(execution_gate) = execution_gate.as_mut() {
                        execution_gate
                            .wait_for(|entered| *entered)
                            .await
                            .expect("execution gate stays open");
                    }
                    release
                        .wait_for(|released| *released)
                        .await
                        .expect("release channel stays open");
                    ToolExecutionResult {
                        deliveries: Vec::new(),
                        status: ToolExecutionStatus::Success,
                        content: Vec::new(),
                        duration_ms: 0,
                        exit_code: None,
                        artifacts: Vec::new(),
                        truncation: None,
                        workflow: None,
                        managed_output: None,
                    }
                }),
                context.cancellation.clone(),
            )
        }

        fn progress_capability(&self) -> ToolProgressCapability {
            ToolProgressCapability::None
        }
    }

    /// Bounds only the terminal wait in background-runtime tests. The
    /// release and start orderings are established by watch state; this is
    /// fail-fast containment for a broken fixture or registry invariant, not
    /// a synchronization primitive.
    const BACKGROUND_LIVENESS_GUARD: std::time::Duration = std::time::Duration::from_mins(2);

    async fn await_background_started(
        started: &mut watch::Receiver<bool>,
        description: &'static str,
    ) {
        tokio::time::timeout(
            BACKGROUND_LIVENESS_GUARD,
            started.wait_for(|is_started| *is_started),
        )
        .await
        .unwrap_or_else(|_| panic!("{description}: start wait exceeded liveness guard"))
        .expect("start channel stays open");
    }

    async fn await_background_terminal(
        registry: &ConversationBackgroundRegistry,
        execution_id: &ToolExecutionId,
        description: &'static str,
    ) -> BackgroundExecutionSnapshot {
        tokio::time::timeout(
            BACKGROUND_LIVENESS_GUARD,
            registry.wait_until_terminal(execution_id),
        )
        .await
        .unwrap_or_else(|_| panic!("{description}: terminal wait exceeded liveness guard"))
        .unwrap_or_else(|error| panic!("{description}: Job wait failed: {error:?}"))
    }

    /// A fixed deterministic status clock.
    #[derive(Debug, Clone, Copy)]
    struct FixedStatusClock;

    impl AgentStatusClock for FixedStatusClock {
        fn now(&self) -> chrono::DateTime<chrono::Utc> {
            chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                .expect("fixed clock")
                .with_timezone(&chrono::Utc)
        }
    }

    /// A fixed deterministic runtime clock.
    #[derive(Debug, Clone, Copy)]
    struct FixedRuntimeClock;

    impl RuntimeClock for FixedRuntimeClock {
        fn now(&self) -> chrono::DateTime<chrono::Utc> {
            chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                .expect("fixed clock")
                .with_timezone(&chrono::Utc)
        }
    }

    /// A fixture over one conversation: the conversation runtime
    /// coordinator, its Runtime Client host adapter, and the scripted
    /// adapter driving attempts.
    struct HostFixture {
        directory: tempfile::TempDir,
        host: RuntimeClientHost,
        runtime: ConversationRuntime,
        coordinator: crate::capabilities::CapabilityCoordinator,
        store: Option<Arc<SqliteConversationStore>>,
    }

    #[tokio::test]
    async fn child_reading_invalid_limits_precede_ownership_resolution() {
        let (_, fixture) = host_fixture(vec![], ToolRegistry::new(), status_engine()).await;
        let id = crate::runtime::identity::AgentId::new("unknown-child");
        for limit in [0, crate::durable::TRANSCRIPT_PAGE_LIMIT_MAX + 1] {
            assert!(matches!(
                fixture.host.inner.agent_transcript_window(
                    &id,
                    &crate::durable::reading::ConversationWindowAt::Latest,
                    limit
                ),
                Err(RuntimeClientError::InvalidRequest { .. })
            ));
            assert!(matches!(
                fixture.host.inner.agent_turns(&id, None, limit),
                Err(RuntimeClientError::InvalidRequest { .. })
            ));
            assert!(matches!(
                fixture.host.inner.transcript_page(None, limit),
                Err(RuntimeClientError::InvalidRequest { .. })
            ));
        }
        // This fixture intentionally has no Agent registry. Valid bounds enter
        // ownership resolution; invalid bounds above never reach it.
        for limit in [1, crate::durable::TRANSCRIPT_PAGE_LIMIT_MAX] {
            assert!(matches!(
                fixture.host.inner.agent_transcript_window(&id, &crate::durable::reading::ConversationWindowAt::Latest, limit),
                Err(RuntimeClientError::InvalidState { message }) if message == "Agent registry unavailable"
            ));
        }
    }

    async fn await_fixture_activation(host: &RuntimeClientHost) {
        let inner = host.weak_inner().upgrade().expect("resident fixture");
        tokio::task::spawn_blocking(move || {
            let deadline = std::time::Instant::now() + BACKGROUND_LIVENESS_GUARD;
            loop {
                let generation = inner.publication_gate.generation();
                let (eligibility, cursor) = {
                    let state = inner.lock_published_state();
                    (state.projection.snapshot_ref().configuration_adoption_eligibility.clone(), state.projection.cursor())
                };
                if eligibility == crate::local_runtime::configuration::application::AdoptionEligibility::Eligible {
                    assert!(cursor.get() > 0, "activation eligibility is actually published");
                    break;
                }
                assert!(std::time::Instant::now() < deadline,
                    "native activation eligibility did not publish: cursor={cursor:?}, eligibility={eligibility:?}");
                inner.publication_gate.wait_after(generation, deadline);
            }
        }).await.expect("fixture activation publication wait");
    }

    /// Builds the conversation runtime + host over one conversation with
    /// the given adapter scripts and tool registry.
    async fn host_fixture(
        scripts: Vec<Vec<GatedStep>>,
        tools: ToolRegistry,
        status_engine: AgentStatusEngine,
    ) -> (Arc<GatedAdapter>, HostFixture) {
        host_fixture_with_native_tools(scripts, tools, status_engine, false).await
    }

    /// Builds the conversation runtime + host, optionally activating the
    /// real native tool plane. Skill projection tests opt into this variant
    /// so the fixture exercises the normal native composition and its
    /// default-enabled Read capability.
    async fn host_fixture_with_native_tools(
        scripts: Vec<Vec<GatedStep>>,
        tools: ToolRegistry,
        status_engine: AgentStatusEngine,
        include_native_tools: bool,
    ) -> (Arc<GatedAdapter>, HostFixture) {
        let (adapter, fixture) = host_fixture_before_activation(
            scripts,
            tools,
            status_engine,
            include_native_tools,
            None,
        )
        .await;
        fixture.runtime.activate();
        await_fixture_activation(&fixture.host).await;
        (adapter, fixture)
    }

    #[allow(
        clippy::too_many_lines,
        reason = "one complete deterministic fixture boundary"
    )]
    async fn host_fixture_before_activation(
        scripts: Vec<Vec<GatedStep>>,
        mut tools: ToolRegistry,
        status_engine: AgentStatusEngine,
        include_native_tools: bool,
        monotonic_clock: Option<Arc<dyn crate::runtime::monotonic::MonotonicClock>>,
    ) -> (Arc<GatedAdapter>, HostFixture) {
        let adapter = Arc::new(GatedAdapter::new(scripts));
        let dir = tempfile::tempdir().expect("temp dir");
        let conversation_id = ConversationId::new("conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let store = Arc::new(
            SqliteConversationStore::open(
                conversation_id.clone(),
                &dir.path().join("conversation.sqlite"),
            )
            .expect("store"),
        );
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
            conversation_id.clone(),
            crate::tools::runtime::ConversationRuntimeConfig {
                durable_binding: Some(crate::durable::ConversationStoreBinding::new(store.clone())),
                ..crate::tools::runtime::ConversationRuntimeConfig::new(
                    &workspace,
                    dir.path().join("artifacts"),
                )
            }
            // These Runtime Client fixtures compose Agent Status and nothing
            // else: the composition must match the status engine below and
            // the extension Tool plane the coordinator is given, because
            // `ConversationRuntime` construction now proves all three follow
            // from one frozen decision (Issue #259). The Todo extension has
            // its own suites; composing it here would only add an unrelated
            // Tool and an unrelated conversation-owned resource to every
            // host contract.
            .with_extensions(
                crate::extensions::NativeAgentExtensions::with_agent_status(
                    crate::context::AgentStatusConfig::default(),
                ),
            ),
        )
        .expect("tool runtime");
        if include_native_tools {
            crate::tools::register_native_tools(
                &mut tools,
                crate::tools::NativeToolResources {
                    subagent_catalog: crate::runtime::subagent::AgentCatalog::empty(),
                    background: tool_runtime.background().clone(),
                    subagents: None,
                },
                crate::tools::NativeToolPolicies::default(),
            )
            .expect("register native tools");
        }
        let coordinator = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                source_demand: crate::capabilities::source::ToolSourceDemand::default(),
                conversation_id: conversation_id.clone(),
                workspace: tool_runtime.workspace().clone(),
                agent_activation: {
                    let mut activation = crate::capabilities::AgentActivation::default();
                    activation.profile.skills =
                        Some(crate::runtime::agent_profile::AgentSkillSelection::All);
                    activation.profile.extensions =
                        crate::scripted_suites::common::plugin_document(tool_runtime.extensions());
                    activation.profile.tools.builtin = tools
                        .definitions()
                        .into_iter()
                        .filter(|tool| tool.origin.source().is_none())
                        .map(|tool| tool.name.clone())
                        .collect();
                    activation
                },
                base_tool_registry: Arc::new(tools),
                extension_tools: tool_runtime.extension_tool_plane(),
                skill_discovery: crate::skills::SkillDiscoveryConfig::workspace_root(
                    workspace.join(".agents/skills"),
                ),
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let candidate = coordinator
            .prepare_candidate()
            .await
            .expect("prepare candidate");
        coordinator.commit(candidate).expect("commit candidate");
        let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
        let config = RuntimeConversationConfig {
            explicit_model: true,
            agent_id: AgentId::new("agent-a"),
            model: scripted_session_model(adapter.clone()),
            approval_mode: crate::runtime::ApprovalMode::Policy,
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator,
                status_engine: Some(status_engine),
            },
            tool_runtime,
            resources: test_resources(&coordinator),
            resource_loader: test_resource_loader(&coordinator),
            capability: coordinator.clone(),
            clock: Some(Arc::new(FixedRuntimeClock)),
            initial_messages: Vec::new(),
            subagents: None,
            workflow_output: None,
        };
        let runtime = match monotonic_clock {
            Some(clock) => ConversationRuntime::with_test_monotonic_clock(config, clock),
            None => ConversationRuntime::new(config),
        }
        .expect("conversation runtime");
        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: runtime.clone(),
            replay_limit: None,
        })
        .expect("runtime client host");
        (
            adapter,
            HostFixture {
                directory: dir,
                host,
                runtime,
                coordinator,
                store: Some(store),
            },
        )
    }

    /// Builds a fixture whose conversation runtime carries the given
    /// coordinator synchronization hooks.
    async fn host_fixture_with_runtime_probe(
        scripts: Vec<Vec<GatedStep>>,
        probe: CoordinatorProbe,
    ) -> (Arc<GatedAdapter>, HostFixture) {
        let adapter = Arc::new(GatedAdapter::new(scripts));
        let dir = tempfile::tempdir().expect("temp dir");
        let conversation_id = ConversationId::new("conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
            conversation_id.clone(),
            crate::tools::runtime::ConversationRuntimeConfig::new(
                &workspace,
                dir.path().join("artifacts"),
            )
            // These Runtime Client fixtures compose Agent Status and nothing
            // else: the composition must match the status engine below and
            // the extension Tool plane the coordinator is given, because
            // `ConversationRuntime` construction now proves all three follow
            // from one frozen decision (Issue #259). The Todo extension has
            // its own suites; composing it here would only add an unrelated
            // Tool and an unrelated conversation-owned resource to every
            // host contract.
            .with_extensions(
                crate::extensions::NativeAgentExtensions::with_agent_status(
                    crate::context::AgentStatusConfig::default(),
                ),
            ),
        )
        .expect("tool runtime");
        let coordinator = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                source_demand: crate::capabilities::source::ToolSourceDemand::default(),
                conversation_id: conversation_id.clone(),
                workspace: tool_runtime.workspace().clone(),
                base_tool_registry: Arc::new(ToolRegistry::new()),
                extension_tools: tool_runtime.extension_tool_plane(),
                agent_activation: crate::capabilities::AgentActivation::default(),
                skill_discovery: crate::skills::SkillDiscoveryConfig::workspace_root(
                    workspace.join(".agents/skills"),
                ),
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");
        let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
        let runtime = ConversationRuntime::with_probe(
            RuntimeConversationConfig {
                explicit_model: true,
                agent_id: AgentId::new("agent-a"),
                model: scripted_session_model(adapter.clone()),
                approval_mode: crate::runtime::ApprovalMode::Policy,
                model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
                tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(
                ),
                context: ConversationContextConfig {
                    policy: crate::context::SessionContextPolicy {
                        reserve_tokens: 0,
                        keep_recent_tokens: 0,
                        summary_output_cap: None,
                    },
                    estimator,
                    status_engine: Some(status_engine()),
                },
                tool_runtime,
                resources: test_resources(&coordinator),
                resource_loader: test_resource_loader(&coordinator),
                capability: coordinator.clone(),
                clock: Some(Arc::new(FixedRuntimeClock)),
                initial_messages: Vec::new(),
                subagents: None,
                workflow_output: None,
            },
            probe,
        )
        .expect("conversation runtime with probe");
        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: runtime.clone(),
            replay_limit: None,
        })
        .expect("runtime client host");
        // The explicit lifecycle boundary: the host bound over the inert
        // runtime, so semantic execution may begin now.
        runtime.activate();
        (
            adapter,
            HostFixture {
                directory: dir,
                host,
                runtime,
                coordinator,
                store: None,
            },
        )
    }

    /// The default status engine over the fixed clock.
    fn status_engine() -> AgentStatusEngine {
        AgentStatusEngine::new(AgentStatusConfig::default(), Arc::new(FixedStatusClock))
    }

    fn inbound_text(id: &str, text: &str) -> UserMessageBlock {
        UserMessageBlock {
            id: crate::runtime::identity::MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: text.to_owned(),
            })],
            source: UserSource::Human,
            kind: crate::message::types::InboundKind::Message,
            timestamp: Some(
                chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                    .expect("parse")
                    .with_timezone(&chrono::Utc),
            ),
        }
    }

    fn commit_auxiliary_call(
        store: &SqliteConversationStore,
    ) -> crate::runtime::identity::AuxiliaryCallId {
        use crate::model::auxiliary::{
            AuxiliaryAttribution, AuxiliaryBusinessSource, AuxiliaryCallStart,
            AuxiliaryFingerprint, AuxiliaryInputEvidence,
        };
        let call_id = crate::runtime::identity::AuxiliaryCallId::generate();
        let fingerprint = AuxiliaryFingerprint::new("a".repeat(64)).unwrap();
        store
            .commit_auxiliary_call_start(
                &AuxiliaryCallStart {
                    call_id: call_id.clone(),
                    purpose: AuxiliaryAttribution::new("summary").unwrap(),
                    trigger: AuxiliaryAttribution::new("manual").unwrap(),
                    business_source: AuxiliaryBusinessSource::Independent,
                    input: AuxiliaryInputEvidence::Temporary,
                    semantic_input_fingerprint: fingerprint.clone(),
                    invocation_fingerprint: fingerprint,
                    selected_message_ids: Vec::new(),
                },
                chrono::Utc::now(),
            )
            .unwrap();
        call_id
    }

    fn submit_content(text: &str) -> Vec<UserContentBlock> {
        vec![UserContentBlock::Text(TextBlock {
            text: text.to_owned(),
        })]
    }

    fn durable_event(
        conversation_id: &ConversationId,
        event_id: &str,
        attempt_id: &AttemptId,
        event: RuntimeEvent,
    ) -> RuntimeEventEnvelope {
        RuntimeEventEnvelope {
            tool_occurrence: None,
            observation: None,
            schema_version: EVENT_SCHEMA_VERSION,
            event_id: EventId::new(event_id),
            sequence: 0,
            conversation_id: conversation_id.clone(),
            attempt_id: Some(attempt_id.clone()),
            turn_id: None,
            timestamp: chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                .expect("fixed event timestamp")
                .with_timezone(&chrono::Utc),
            event,
        }
    }

    fn durable_tool_result() -> ToolExecutionResult {
        ToolExecutionResult {
            deliveries: Vec::new(),
            status: ToolExecutionStatus::Success,
            content: vec![ToolResultContent::Text(TextBlock {
                text: "child tool output".to_owned(),
            })],
            duration_ms: 7,
            exit_code: Some(0),
            artifacts: Vec::new(),
            truncation: None,
            workflow: None,
            managed_output: None,
        }
    }

    fn one_turn_stop() -> Vec<GatedStep> {
        vec![
            GatedStep::Emit(ModelEvent::Started),
            GatedStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(0),
                text: "done".to_owned(),
            }),
            GatedStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ]
    }

    struct DurableHistoryFixture {
        conversation_id: ConversationId,
        store: Arc<SqliteConversationStore>,
        user: MessageBlock,
        assistant: MessageBlock,
        tool: MessageBlock,
        call: ToolCall,
        tool_result: ToolExecutionResult,
    }

    fn durable_history_fixture() -> DurableHistoryFixture {
        let conversation_id = ConversationId::new("conv_8ae2de1c-f83b-7ff0-882f-98321c50d37d");
        let store = Arc::new(
            SqliteConversationStore::in_memory(conversation_id.clone()).expect("child store"),
        );
        let user = MessageBlock::User(inbound_text("child-user", "inspect this child"));
        let call = ToolCall {
            id: ToolCallId::new("child-call-1"),
            tool_id: ToolId::new("tool-read"),
            name: "read".to_owned(),
            arguments: serde_json::json!({"path": "README.md"}),
        };
        let assistant = MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("child-assistant"),
            content: vec![
                AssistantContentBlock::Text(TextBlock {
                    text: "I will inspect the file.".to_owned(),
                }),
                AssistantContentBlock::ToolCall(call.clone()),
            ],
        });
        let tool_result = durable_tool_result();
        let tool = MessageBlock::Tool(ToolMessageBlock {
            occurrence: crate::message::types::ToolCallOccurrenceRef::new(
                crate::runtime::identity::MessageId::new("child-assistant"),
                crate::message::types::ContentBlockIndex::new(1),
            ),
            id: MessageId::new("child-tool-result"),
            tool_call_id: call.id.clone(),
            tool_id: call.tool_id.clone(),
            result: tool_result.clone(),
        });
        append_durable_history(
            &store,
            &conversation_id,
            &assistant,
            &tool,
            &call,
            &tool_result,
        );
        DurableHistoryFixture {
            conversation_id,
            store,
            user,
            assistant,
            tool,
            call,
            tool_result,
        }
    }

    fn append_durable_history(
        store: &SqliteConversationStore,
        conversation_id: &ConversationId,
        assistant: &MessageBlock,
        tool: &MessageBlock,
        call: &ToolCall,
        tool_result: &ToolExecutionResult,
    ) {
        let user = MessageBlock::User(inbound_text("child-user", "inspect this child"));
        store
            .initialize(std::slice::from_ref(&user))
            .expect("child seed");
        let attempt_id = AttemptId::new("child-attempt-1");
        store
            .append_event(durable_event(
                conversation_id,
                "child-attempt-started",
                &attempt_id,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt_id.clone(),
                },
            ))
            .expect("attempt start");
        store
            .append_canonical_with_event(
                assistant,
                durable_event(
                    conversation_id,
                    "child-assistant-committed",
                    &attempt_id,
                    RuntimeEvent::AssistantMessageCommitted {
                        message_id: assistant.id().clone(),
                    },
                ),
            )
            .expect("assistant commit");
        store
            .append_event(durable_event(
                conversation_id,
                "child-tool-started",
                &attempt_id,
                RuntimeEvent::ToolExecutionStarted {
                    tool_call_id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                },
            ))
            .expect("tool start");
        store
            .append_event(durable_event(
                conversation_id,
                "child-tool-completed",
                &attempt_id,
                RuntimeEvent::ToolExecutionCompleted {
                    tool_call_id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                    result: tool_result.clone(),
                },
            ))
            .expect("tool completion");
        store
            .append_canonical_with_event(
                tool,
                durable_event(
                    conversation_id,
                    "child-tool-committed",
                    &attempt_id,
                    RuntimeEvent::ToolMessageCommitted {
                        message_id: tool.id().clone(),
                        tool_call_id: call.id.clone(),
                    },
                ),
            )
            .expect("tool result commit");
        store
            .append_event(durable_event(
                conversation_id,
                "child-attempt-completed",
                &attempt_id,
                RuntimeEvent::AttemptCompleted {
                    attempt_id: attempt_id.clone(),
                    finish_reason: ModelFinishReason::Stop,
                },
            ))
            .expect("attempt completion");
    }

    /// The outer liveness guard of the event-stream helpers.
    ///
    /// Waiting for an event is exact: the subscription wakes on
    /// publication. This bounds only the total wall time of one
    /// `receive_until` call, so a genuine regression fails with a message
    /// instead of hanging. It is deliberately far larger than any
    /// scheduling delay a loaded runner can produce, and it is a whole-call
    /// budget rather than a per-event bound — a single scheduling stall can
    /// never fail a correct run.
    const STREAM_LIVENESS_GUARD: std::time::Duration = std::time::Duration::from_mins(2);

    /// Receives events until the predicate matches.
    async fn receive_until(
        subscription: &EventSubscription,
        mut predicate: impl FnMut(&RuntimeClientProtocolEvent) -> bool,
    ) -> Vec<RuntimeClientProtocolEvent> {
        tokio::time::timeout(STREAM_LIVENESS_GUARD, async {
            let mut seen = Vec::new();
            loop {
                let delivery = subscription.next().await;
                let EventDelivery::Event(event) = delivery else {
                    panic!("subscription must stay open and contiguous, got {delivery:?}");
                };
                let RuntimeClientEvent::ProjectionTransition { metadata, .. } = &event.event else {
                    panic!("one complete native transition");
                };
                assert_eq!(metadata.read_cut.revision, event.cursor.get());
                let matched = event.event.facts().iter().any(|fact| {
                    predicate(&RuntimeClientProtocolEvent {
                        cursor: event.cursor,
                        event: fact.clone(),
                    })
                });
                seen.push(event);
                if matched {
                    return seen;
                }
            }
        })
        .await
        .expect("the observation stream must not stall")
    }

    async fn receive_until_metadata(
        subscription: &EventSubscription,
        mut predicate: impl FnMut(&super::super::checkpoint::ProjectionMetadata) -> bool,
    ) -> RuntimeClientProtocolEvent {
        tokio::time::timeout(STREAM_LIVENESS_GUARD, async {
            loop {
                let EventDelivery::Event(event) = subscription.next().await else {
                    panic!("native subscription must remain contiguous");
                };
                let RuntimeClientEvent::ProjectionTransition { metadata, .. } = &event.event else {
                    panic!("one complete native transition");
                };
                assert_eq!(metadata.read_cut.revision, event.cursor.get());
                if predicate(metadata) { return event; }
                if let Some(outcome) = event.event.facts().iter().find_map(|fact| match fact {
                    RuntimeClientEvent::AttemptSettled { outcome, .. } => Some(outcome),
                    _ => None,
                }) {
                    panic!("native producer settled before the expected published block frontier: {outcome:?}");
                }
            }
        }).await.expect("the native metadata stream must advance")
    }

    fn append_auxiliary_test_primary_facts(store: &SqliteConversationStore, ordinal: u64) {
        let attempt =
            crate::runtime::identity::AttemptId::new(format!("cold-audit-primary-{ordinal}"));
        for event in [
            RuntimeEvent::AttemptStarted {
                attempt_id: attempt.clone(),
            },
            RuntimeEvent::AttemptCompleted {
                attempt_id: attempt.clone(),
                finish_reason: crate::model::ModelFinishReason::Stop,
            },
        ] {
            store
                .append_event(crate::events::types::RuntimeEventEnvelope {
                    schema_version: crate::events::types::EVENT_SCHEMA_VERSION,
                    event_id: crate::runtime::identity::EventId::new(format!(
                        "cold-audit-primary-{ordinal}-{}",
                        store.auxiliary_audit_cut().unwrap().journal
                    )),
                    sequence: 0,
                    conversation_id: store.conversation_id().clone(),
                    attempt_id: Some(attempt.clone()),
                    turn_id: None,
                    timestamp: chrono::Utc::now(),
                    tool_occurrence: None,
                    observation: None,
                    event,
                })
                .unwrap();
        }
    }

    /// A cold durable checkpoint discovers audit rows from the Store at its
    /// exact captured Journal cut, without waiting for a live auxiliary event.
    #[test]
    fn cold_checkpoint_discovers_existing_auxiliary_audit_with_dense_paging() {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store.initialize(&[]).unwrap();
        for ordinal in 0..33 {
            commit_auxiliary_call(&store);
            append_auxiliary_test_primary_facts(&store, ordinal);
            store
                .append_canonical(&MessageBlock::User(inbound_text(
                    &format!("cold-primary-{ordinal}"),
                    &format!("unrelated primary fact {ordinal}"),
                )))
                .unwrap();
        }
        let expected_journal = store.observation_read_cut().unwrap().read.journal;
        let host = RuntimeClientHost::new_durable(Arc::new(store), Some(16)).unwrap();
        let checkpoint = host.checkpoint().unwrap();
        assert_eq!(checkpoint.read_cut.through, expected_journal);
        assert!(checkpoint.read_cut.through > 33);
        let frontier = checkpoint
            .collections
            .iter()
            .find(|frontier| frontier.domain == crate::runtime_client::auxiliary::DOMAIN)
            .expect("cold checkpoint advertises existing auxiliary audit");
        assert_eq!(frontier.total, 33);
        assert_eq!(frontier.loaded, 0);
        assert_eq!(frontier.next_offset, Some(0));

        let first = host
            .inner
            .checkpoint_page(
                &checkpoint.read_cut,
                crate::runtime_client::auxiliary::DOMAIN,
                0,
                32,
            )
            .unwrap();
        assert_eq!(first.total, 33);
        assert_eq!(first.items.len(), 32);
        assert_eq!(first.next_offset, Some(32));
        assert_eq!(first.items[0].offset, 0);
        assert_eq!(first.items[31].offset, 31);

        let final_page = host
            .inner
            .checkpoint_page(
                &first.read_cut,
                crate::runtime_client::auxiliary::DOMAIN,
                first.next_offset.unwrap(),
                32,
            )
            .unwrap();
        assert_eq!(final_page.total, 33);
        assert_eq!(final_page.items.len(), 1);
        assert_eq!(final_page.items[0].offset, 32);
        assert_eq!(final_page.next_offset, None);
    }

    #[test]
    fn cold_empty_auxiliary_audit_has_no_pages_after_long_unrelated_history() {
        let store = SqliteConversationStore::in_memory(ConversationId::generate()).unwrap();
        store.initialize(&[]).unwrap();
        for ordinal in 0..48 {
            store
                .append_canonical(&MessageBlock::User(inbound_text(
                    &format!("cold-empty-primary-{ordinal}"),
                    &format!("unrelated history {ordinal}"),
                )))
                .unwrap();
        }
        let host = RuntimeClientHost::new_durable(Arc::new(store), Some(16)).unwrap();
        let checkpoint = host.checkpoint().unwrap();
        let frontier = checkpoint
            .collections
            .iter()
            .find(|frontier| frontier.domain == crate::runtime_client::auxiliary::DOMAIN)
            .expect("empty audit still has an explicit finite frontier");
        assert_eq!(frontier.total, 0);
        assert_eq!(frontier.loaded, 0);
        assert_eq!(frontier.next_offset, None);
        let page = host
            .inner
            .checkpoint_page(
                &checkpoint.read_cut,
                crate::runtime_client::auxiliary::DOMAIN,
                0,
                32,
            )
            .unwrap();
        assert_eq!(page.total, 0);
        assert!(page.items.is_empty());
        assert_eq!(page.next_offset, None);
    }

    #[test]
    fn auxiliary_checkpoint_page_retains_its_exact_target_and_read_cut() {
        let store =
            Arc::new(SqliteConversationStore::in_memory(ConversationId::generate()).unwrap());
        store.initialize(&[]).unwrap();
        for ordinal in 0..33 {
            commit_auxiliary_call(&store);
            store
                .append_canonical(&MessageBlock::User(inbound_text(
                    &format!("exact-cut-primary-{ordinal}"),
                    &format!("interleaved primary fact {ordinal}"),
                )))
                .unwrap();
        }
        let host = RuntimeClientHost::new_durable(store.clone(), Some(16)).unwrap();
        let checkpoint = host.checkpoint().unwrap();
        let first = host
            .inner
            .checkpoint_page(
                &checkpoint.read_cut,
                crate::runtime_client::auxiliary::DOMAIN,
                0,
                32,
            )
            .unwrap();
        assert_eq!(first.total, 33);
        assert_eq!(first.items.len(), 32);
        assert_eq!(first.next_offset, Some(32));
        assert_eq!(first.read_cut, checkpoint.read_cut);

        commit_auxiliary_call(&store);
        store
            .append_canonical(&MessageBlock::User(inbound_text(
                "exact-cut-later-admission",
                "admitted after the checkpoint",
            )))
            .unwrap();
        let retained = host
            .inner
            .checkpoint_page(
                &checkpoint.read_cut,
                crate::runtime_client::auxiliary::DOMAIN,
                0,
                32,
            )
            .unwrap();
        assert_eq!(retained.items, first.items);
        assert_eq!(retained.total, 33);

        let mut wrong_incarnation = checkpoint.read_cut.clone();
        wrong_incarnation.activation_id = Some(crate::runtime::identity::SubagentId::new(
            "foreign-auxiliary-reader",
        ));
        assert!(matches!(
            host.inner.checkpoint_page(
                &wrong_incarnation,
                crate::runtime_client::auxiliary::DOMAIN,
                0,
                32,
            ),
            Err(RuntimeClientError::ReadCutExpired)
        ));

        let mut wrong_revision = checkpoint.read_cut.clone();
        *wrong_revision
            .domain_revisions
            .entry("auxiliary".into())
            .or_default() += 1;
        assert!(matches!(
            host.inner.checkpoint_page(
                &wrong_revision,
                crate::runtime_client::auxiliary::DOMAIN,
                0,
                32,
            ),
            Err(RuntimeClientError::ReadCutExpired)
        ));
    }

    /// A durable attachment reads the child's canonical Ledger and Event
    /// Journal through the ordinary Runtime Client snapshot. It has no live
    /// runtime to mutate, and detaching it leaves both authorities byte-for-
    /// byte semantically unchanged.
    #[test]
    fn durable_attachment_replays_child_history_without_writing_or_copying() {
        let fixture = durable_history_fixture();

        let canonical_before = fixture
            .store
            .load_canonical()
            .expect("canonical before inspection");
        let events_before = fixture
            .store
            .read_events(None, 64)
            .expect("journal before inspection")
            .events;
        let host = RuntimeClientHost::new_durable(fixture.store.clone(), Some(16))
            .expect("inspection host");
        assert!(host.inner.runtime.is_none(), "inspection owns no runtime");

        let (attachment, initialized) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach child conversation");
        let RuntimeClientResult::Initialized {
            conversation_id: attached_id,
            ..
        } = initialized
        else {
            panic!("durable attach must return its bounded identity receipt");
        };
        let (snapshot, cursor) = host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("explicit authoritative checkpoint read");
        assert_eq!(attached_id, fixture.conversation_id);
        assert_eq!(cursor, RuntimeClientCursor::new(0));
        assert_eq!(
            snapshot.messages,
            vec![
                fixture.user.clone(),
                fixture.assistant.clone(),
                fixture.tool.clone()
            ]
        );
        assert!(snapshot.transcript.entries.iter().any(|entry| {
            matches!(
                &entry.item,
                crate::runtime_client::snapshot::RuntimeClientTranscriptItem::Message { message }
                    if message == &fixture.assistant
            )
        }));
        assert!(snapshot.transcript.entries.iter().any(|entry| {
            matches!(
                &entry.item,
                crate::runtime_client::snapshot::RuntimeClientTranscriptItem::Message { message }
                    if message == &fixture.tool
            )
        }));
        let attempt = snapshot.attempt.expect("durable attempt history");
        assert!(matches!(
            attempt.phase,
            RuntimeClientAttemptPhase::Settled {
                outcome: crate::runtime_client::event::RuntimeClientOutcome::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    ..
                }
            }
        ));
        assert!(matches!(
            attempt.foreground.as_slice(),
            [crate::runtime_client::snapshot::ForegroundToolExecution {
                call_id,
                state: crate::runtime_client::snapshot::ForegroundToolState::Settled { result, .. },
                ..
            }] if call_id.as_ref() == Some(&fixture.call.id) && result == &fixture.tool_result
        ));

        let rejected = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("must not mutate child"),
        });
        assert!(matches!(
            rejected.error,
            Some(RuntimeClientError::InvalidState { message })
                if message == "conversation inspection is read-only"
        ));
        attachment.detach();
        assert_eq!(
            fixture
                .store
                .load_canonical()
                .expect("canonical after inspection"),
            canonical_before
        );
        assert_eq!(
            fixture
                .store
                .read_events(None, 64)
                .expect("journal after inspection")
                .events,
            events_before
        );
    }

    /// A new durable owner captures a later original terminal. Reads on the
    /// existing owner retain its constructor cut and subscription unchanged.
    #[test]
    fn durable_attachment_resyncs_cancelled_child_after_runtime_disappears() {
        let conversation_id = ConversationId::new("conv_ef919f28-b766-7227-8980-3d65cbf34ff6");
        let store = Arc::new(
            SqliteConversationStore::in_memory(conversation_id.clone()).expect("child store"),
        );
        let partial_user = MessageBlock::User(inbound_text(
            "cancelled-child-user",
            "the cancelled child started this durable turn",
        ));
        store
            .initialize(std::slice::from_ref(&partial_user))
            .expect("partial child seed");
        let host = RuntimeClientHost::new_durable(store.clone(), None).expect("inspection host");
        let (attachment, initialized) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("initial attach");
        let RuntimeClientResult::Initialized { .. } = initialized else {
            panic!("durable attach must initialize");
        };
        let initial = host.checkpoint().unwrap();
        let subscription = attachment.subscribe_events(initial.cursor).unwrap();
        let (snapshot, _) = host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("explicit durable checkpoint");
        assert_eq!(snapshot.messages, vec![partial_user]);
        assert!(snapshot.attempt.is_none());

        let attempt_id = AttemptId::new("cancelled-attempt");
        store
            .append_event(durable_event(
                &conversation_id,
                "cancelled-attempt-started",
                &attempt_id,
                RuntimeEvent::AttemptStarted {
                    attempt_id: attempt_id.clone(),
                },
            ))
            .expect("attempt start");
        store
            .append_event(durable_event(
                &conversation_id,
                "cancelled-attempt-cancelled",
                &attempt_id,
                RuntimeEvent::AttemptCancelled {
                    attempt_id: attempt_id.clone(),
                    reason: crate::runtime::types::CancellationReason::UserRequested,
                },
            ))
            .expect("attempt cancellation");

        assert_eq!(host.checkpoint().unwrap(), initial);
        assert!(matches!(subscription.try_next(), EventDelivery::Pending));
        let fresh = RuntimeClientHost::new_durable(store.clone(), None).unwrap();
        let (repaired, cursor) = fresh
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("new durable owner");
        assert_eq!(cursor, RuntimeClientCursor::new(0));
        assert!(matches!(
            repaired.attempt.expect("cancelled attempt").phase,
            RuntimeClientAttemptPhase::Settled {
                outcome: crate::runtime_client::event::RuntimeClientOutcome::Cancelled {
                    reason: crate::runtime::types::CancellationReason::UserRequested,
                }
            }
        ));
        attachment.detach();
    }

    /// Issue #256 regression 9: historical-only inspection reports no
    /// effective native Agent Extension composition, and never fabricates
    /// one.
    ///
    /// There is no live Agent behind a durable projection, so there is no
    /// authoritative composition to project. The history seeded here
    /// deliberately *does* contain Agent Status evidence — a canonical Agent
    /// Status context message, with its durable generation identity, exactly
    /// as a runtime that once ran the extension would have left behind.
    /// That evidence is not allowed to become extension configuration: it
    /// describes a step that happened, not the composition of an Agent that
    /// is running now. Neither is today's configuration document, which
    /// describes a prospective next launch.
    #[test]
    fn ext256_historical_inspection_reports_no_effective_extension_composition() {
        let conversation_id = ConversationId::new("conv_576d0fd2-f9b2-75ce-8187-0c2ca5bbfe62");
        let store = Arc::new(
            SqliteConversationStore::in_memory(conversation_id.clone()).expect("durable store"),
        );
        let user = MessageBlock::User(inbound_text(
            "historical-user",
            "this turn is only durable history now",
        ));
        // Canonical Agent Status evidence in the very history being
        // inspected: a projection that inferred the extension set from what
        // it can see would report the extension as composed here.
        let mut status = inbound_text("historical-status", "<system-reminder>\n</system-reminder>");
        status.source = UserSource::Runtime;
        status.kind = crate::message::types::InboundKind::Context(
            crate::message::types::ContextKind::AgentStatus(
                crate::message::types::AgentStatusGenerationMetadata::new(
                    chrono::DateTime::parse_from_rfc3339("2026-08-07T12:00:00Z")
                        .expect("fixed status timestamp")
                        .with_timezone(&chrono::Utc),
                    [crate::context::AgentStatusModuleId::Time],
                )
                .expect("canonical Agent Status membership"),
            ),
        );
        store
            .initialize(&[user.clone(), MessageBlock::User(status)])
            .expect("durable seed");
        let attempt_id = AttemptId::new("historical-attempt");
        store
            .append_event(durable_event(
                &conversation_id,
                "historical-attempt-started",
                &attempt_id,
                crate::events::types::RuntimeEvent::AttemptStarted {
                    attempt_id: attempt_id.clone(),
                },
            ))
            .expect("attempt start");

        let host = RuntimeClientHost::new_durable(store, None).expect("inspection host");
        let (attachment, initialized) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("durable attach");
        let RuntimeClientResult::Initialized { .. } = initialized else {
            panic!("durable attach must initialize");
        };
        let (snapshot, _) = host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("explicit durable checkpoint");
        assert_eq!(
            snapshot.settings_evidence,
            crate::runtime_client::settings::SettingsEvidence::HistoricalPartial
        );
        assert_eq!(
            snapshot.effective_plugins, None,
            "unavailable evidence is reported as unavailable, not reconstructed"
        );
        assert_eq!(
            snapshot.messages.len(),
            2,
            "the Agent Status evidence really is in the inspected history"
        );

        // A durable resync is the other read path into this projection, and
        // it must stay just as honest.
        let (resynced, _) = host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("durable resync");
        assert_eq!(resynced.effective_plugins, None);
        attachment.detach();
    }

    struct StagedFrameProbe {
        staged: std::sync::mpsc::Receiver<()>,
        proceed: std::sync::mpsc::Sender<()>,
        encoding: std::sync::mpsc::Receiver<()>,
        finish_encoding: std::sync::mpsc::Sender<()>,
        committed: std::sync::mpsc::Receiver<super::super::content::ContentRef>,
    }

    fn install_publication_encoding_probe(
        inner: &super::ClientInner,
        owner: Arc<super::ClientInner>,
        future_reference: super::super::content::ContentRef,
        before: RuntimeClientCursor,
        encoding_tx: std::sync::mpsc::Sender<()>,
        encode_release_rx: std::sync::mpsc::Receiver<()>,
    ) {
        *inner.publication_gate.encoding_hook.lock().unwrap() = Some(Box::new(move || {
            // Encoding is parked with the CPU publication gate held;
            // the raw state boundary must remain usable at C.
            let state = owner
                .state
                .try_lock()
                .expect("encoding releases raw Projection");
            assert_eq!(state.projection.cursor(), before);
            assert!(
                state
                    .projection
                    .validate_live_content_ref(&future_reference)
                    .is_err()
            );
            let seed = state.projection.checkpoint_seed();
            drop(state);
            assert_eq!(seed.checkpoint_candidate().unwrap().cursor, before);
            assert_eq!(
                owner.checkpoint().unwrap().cursor,
                before,
                "the actual checkpoint reader materializes frozen C while D encodes"
            );
            let admitted = owner.admit_attachment_identity(true).unwrap();
            let (subscription, _) = owner
                .subscribe_events(admitted.attachment_id(), before)
                .unwrap();
            drop(subscription);
            owner.detach(admitted.attachment_id());
            encoding_tx.send(()).unwrap();
            encode_release_rx.recv().unwrap();
        }));
    }

    fn install_staged_frame_probe(inner: &Arc<super::ClientInner>) -> StagedFrameProbe {
        let (stage_tx, stage_rx) = std::sync::mpsc::channel();
        let (resume_tx, resume_rx) = std::sync::mpsc::channel();
        let (committed_tx, committed_rx) = std::sync::mpsc::channel();
        let (encoding_tx, encoding_rx) = std::sync::mpsc::channel();
        let (encode_release_tx, encode_release_rx) = std::sync::mpsc::channel();
        let mut encode_release_rx = Some(encode_release_rx);
        let mut candidate = None;
        let encoding_owner = Arc::clone(inner);
        *inner.publication_commit_hook.lock().unwrap() =
            Some(Box::new(move |inner, observations, committed| {
                let frame = super::publication_frame(observations).expect("actual staged frame");
                let state = inner.state.lock().unwrap();
                if committed {
                    let (reference, before) = candidate.take().unwrap();
                    state
                        .projection
                        .validate_live_content_ref(&reference)
                        .unwrap();
                    assert!(state.projection.cursor() > before);
                    committed_tx.send(reference).unwrap();
                } else {
                    let source = state
                        .projection
                        .publication_source(ContentBlockIndex::new(0));
                    let before = state.projection.cursor();
                    drop(state);
                    let reference = inner
                        .store
                        .content_descriptor_at(
                            &source,
                            super::super::content::ContentRepresentation::Utf8Text,
                            frame.sequence + 1,
                        )
                        .unwrap()
                        .expect("P already durable");
                    assert!(reference.readable_end > 0);
                    assert!(
                        inner
                            .state
                            .lock()
                            .unwrap()
                            .projection
                            .validate_live_content_ref(&reference)
                            .is_err()
                    );
                    let owner = Arc::clone(&encoding_owner);
                    let future_reference = reference.clone();
                    let encoding_tx = encoding_tx.clone();
                    let encode_release_rx = encode_release_rx.take().unwrap();
                    install_publication_encoding_probe(
                        inner,
                        owner,
                        future_reference,
                        before,
                        encoding_tx,
                        encode_release_rx,
                    );
                    candidate = Some((reference, before));
                    stage_tx.send(()).unwrap();
                    resume_rx.recv().unwrap();
                }
            }));
        StagedFrameProbe {
            staged: stage_rx,
            proceed: resume_tx,
            encoding: encoding_rx,
            finish_encoding: encode_release_tx,
            committed: committed_rx,
        }
    }

    /// T19: the real Agent stages P, then its native observer commits C before
    /// returning to User Release. The worker is fenced so a later reader cannot
    /// accidentally repair the missing synchronous transition for this proof.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn t19_staged_frame_is_projected_before_observer_returns_to_release() {
        let (start_tx, start_rx) = model_release();
        let (finish_tx, finish_rx) = model_release();
        let (adapter, fixture) = host_fixture(
            vec![vec![
                GatedStep::Emit(ModelEvent::Started),
                GatedStep::ParkUntilReleased(start_rx),
                GatedStep::Emit(ModelEvent::TextDelta {
                    block_index: ContentBlockIndex::new(0),
                    text: "π".repeat(40_000),
                }),
                GatedStep::ParkUntilReleased(finish_rx),
                GatedStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ]],
            ToolRegistry::new(),
            status_engine(),
        )
        .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let inner = fixture.host.weak_inner().upgrade().unwrap();
        let mut requests = adapter.request_count();
        attachment
            .handle_request(RuntimeClientRequest::SubmitInbound {
                id: crate::runtime_client::RequestId::new(1),
                content: submit_content("bounded publication"),
            })
            .result
            .unwrap();
        requests.wait_for(|count| *count == 1).await.unwrap();
        inner.park_projection_worker();
        let probe = install_staged_frame_probe(&inner);
        start_tx.send(true).unwrap();
        probe.staged.recv().unwrap();
        // The model task is parked after native durable P and before display C.
        // It cannot return through its observer or release the frame yet.
        assert!(probe.committed.try_recv().is_err());
        probe.proceed.send(()).unwrap();
        probe.encoding.recv().unwrap();
        assert!(
            probe.committed.try_recv().is_err(),
            "U cannot pass an unencoded D"
        );
        probe.finish_encoding.send(()).unwrap();
        let reference = probe.committed.recv().unwrap();
        assert!(reference.readable_end <= 80_000);
        // Armed before the trigger: settlement wakes only registered waiters.
        let settled = fixture.runtime.settlement_signal().notified();
        tokio::pin!(settled);
        settled.as_mut().enable();
        finish_tx.send(true).unwrap();
        settled.await;
        assert_eq!(adapter.requests().len(), 1);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn t19_native_audit_commits_with_worker_parked_before_execution_terminal() {
        let (_finish, finish_rx) = model_release();
        let (_, fixture) = host_fixture(
            vec![vec![
                GatedStep::Emit(ModelEvent::Started),
                GatedStep::Emit(ModelEvent::TextDelta {
                    block_index: ContentBlockIndex::new(0),
                    text: "π".repeat(40_000),
                }),
                GatedStep::ParkUntilReleased(finish_rx),
            ]],
            ToolRegistry::new(),
            status_engine(),
        )
        .await;
        let inner = fixture.host.weak_inner().upgrade().unwrap();
        inner.park_projection_worker();
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let initial = inner.checkpoint().unwrap();
        let subscription = attachment.subscribe_events(initial.cursor).unwrap();
        // Armed before the trigger: settlement wakes only registered waiters.
        let settled = fixture.runtime.settlement_signal().notified();
        tokio::pin!(settled);
        settled.as_mut().enable();
        fixture
            .host
            .submit_inbound(submit_content("audit ownership"))
            .unwrap();
        receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::BlockContentAdvanced { .. }))
        })
        .await;
        let attempt = inner
            .state
            .lock()
            .unwrap()
            .projection
            .snapshot_ref()
            .attempt
            .as_ref()
            .unwrap()
            .attempt_id
            .clone()
            .expect("exact native fixture attempt identity");
        let (observed_tx, observed_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        *inner.audit_commit_hook.lock().unwrap() = Some(Box::new(move |owner| {
            let state = owner
                .state
                .try_lock()
                .expect("native audit leaves the raw boundary available");
            let snapshot = state.projection.snapshot_ref();
            assert_eq!(snapshot.publication_settlements.len(), 1);
            assert_eq!(
                snapshot.publication_settlements[0].settlement.kind,
                crate::publication::PublicationAuditKind::Incomplete
            );
            assert!(matches!(
                snapshot.attempt.as_ref().unwrap().phase,
                RuntimeClientAttemptPhase::Running
            ));
            assert!(snapshot.attempt.as_ref().unwrap().in_flight.is_none());
            assert!(
                snapshot
                    .messages
                    .iter()
                    .all(|message| !matches!(message, MessageBlock::Assistant(_)))
            );
            assert_eq!(
                state
                    .projection
                    .checkpoint_seed()
                    .checkpoint_candidate()
                    .unwrap()
                    .snapshot
                    .publication_settlements
                    .len(),
                1,
            );
            observed_tx.send(()).unwrap();
            drop(state);
            release_rx.recv().unwrap();
        }));
        let preparation = inner.observation_preparation.lock().unwrap();
        assert_eq!(
            fixture.runtime.cancel_current_attempt(&attempt).unwrap(),
            attempt
        );
        observed_rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .expect("Audit CPU receipt does not wait for source preparation or the worker");
        release_tx.send(()).unwrap();
        drop(preparation);
        settled.await;
        inner.unpark_projection_worker();
    }

    fn assert_adopted_input(request: &ModelRequest, message_id: &MessageId) {
        assert!(request.messages.iter().any(|message| {
            matches!(message, crate::model::ModelInputMessage::Canonical(MessageBlock::User(user)) if &user.id == message_id)
        }));
        assert!(request.messages.iter().any(|message| {
            matches!(
                message,
                crate::model::ModelInputMessage::Canonical(MessageBlock::User(user))
                    if matches!(
                        &user.kind,
                        crate::message::types::InboundKind::Context(
                            crate::message::types::ContextKind::AgentStatus(_)
                        )
                    )
            )
        }));
    }

    fn assert_read_domain_replay(
        inner: &super::ClientInner,
        after_cursor: RuntimeClientCursor,
        cursor: RuntimeClientCursor,
    ) {
        let mut state = inner.lock_state();
        let (subscriber, _) = state.projection.subscribe(after_cursor).unwrap();
        for expected in after_cursor.get() + 1..=cursor.get() {
            let super::super::projection::SubscriberPoll::Event(event) =
                state.projection.poll_subscriber(subscriber)
            else {
                panic!("derived suffix");
            };
            assert_eq!(event.cursor.get(), expected);
            assert!(
                event
                    .event
                    .facts()
                    .iter()
                    .any(|fact| matches!(fact, RuntimeClientEvent::ReadDomainsUpdated { .. }))
            );
        }
        drop(state);
    }

    fn wire_value<T: serde::Serialize + schemars::JsonSchema>(value: &T) -> serde_json::Value {
        let mut bytes = Vec::new();
        super::super::structural_json::to_writer(&mut bytes, value).unwrap();
        serde_json::from_slice(&bytes).unwrap()
    }

    fn many_live_block_steps(
        suffix_rx: watch::Receiver<bool>,
        finish_rx: watch::Receiver<bool>,
    ) -> Vec<GatedStep> {
        use sha2::Digest as _;
        use std::fmt::Write as _;
        let mut steps = vec![GatedStep::Emit(ModelEvent::Started)];
        for block in 0..32 {
            let mut text = String::new();
            for part in 0..100 {
                write!(
                    text,
                    "{:x}",
                    sha2::Sha256::digest(format!("{block}/{part}").as_bytes())
                )
                .unwrap();
            }
            assert!(
                super::super::checkpoint::bounded_bytes(
                    &super::super::snapshot::InFlightBlock::Text {
                        block_index: ContentBlockIndex::new(block),
                        text: text.clone()
                    },
                    super::super::checkpoint::CHECKPOINT_ITEM_BYTES,
                )
                .is_some(),
                "each individual native prefix fits; aggregate pressure causes deferral"
            );
            steps.push(GatedStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(block),
                text,
            }));
        }
        let generated_bytes: usize = steps
            .iter()
            .filter_map(|step| match step {
                GatedStep::Emit(ModelEvent::TextDelta { text, .. }) => Some(text.len()),
                _ => None,
            })
            .sum();
        let policy = crate::model::invocation::runtime_fallback_generation_safety_policy(512);
        assert!(generated_bytes > super::super::checkpoint::CHECKPOINT_TARGET_BYTES / 8);
        assert!(
            u64::try_from(generated_bytes + "exact final suffix π".len()).unwrap()
                < policy.max_generated_bytes,
            "aggregate capture pressure must remain valid native generated output"
        );
        steps.extend([
            GatedStep::ParkUntilReleased(suffix_rx),
            GatedStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(31),
                text: "exact final suffix π".into(),
            }),
            GatedStep::ParkUntilReleased(finish_rx),
            GatedStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ]);
        steps
    }

    fn assert_live_block_count(
        checkpoint: &super::super::checkpoint::RuntimeClientCheckpoint,
        expected: usize,
    ) {
        assert_eq!(
            checkpoint
                .snapshot
                .attempt
                .as_ref()
                .unwrap()
                .in_flight
                .as_ref()
                .unwrap()
                .blocks
                .len(),
            expected
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn t06_t14_many_live_blocks_keep_every_descriptor_after_inline_budget_exhaustion() {
        let (suffix_tx, suffix_rx) = model_release();
        let (finish_tx, finish_rx) = model_release();
        let steps = many_live_block_steps(suffix_rx, finish_rx);
        let (_, fixture) = host_fixture(vec![steps], ToolRegistry::new(), status_engine()).await;
        let inner = fixture.host.weak_inner().upgrade().unwrap();
        inner.park_projection_worker();
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let initial = inner.checkpoint().unwrap();
        let subscription = attachment.subscribe_events(initial.cursor).unwrap();
        fixture
            .host
            .submit_inbound(submit_content("all native block descriptors"))
            .unwrap();
        receive_until_metadata(&subscription, |metadata| {
            metadata
                .content
                .iter()
                .any(|binding| binding.path == ["attempt", "in_flight", "blocks", "31", "text"])
        })
        .await;
        let before = inner.checkpoint().unwrap();
        assert_live_block_count(&before, 32);
        let prefixes: Vec<_> = before
            .content
            .iter()
            .filter(|binding| {
                binding
                    .path
                    .starts_with(&["attempt".into(), "in_flight".into(), "blocks".into()])
                    && binding.path.last().is_some_and(|field| field == "text")
            })
            .collect();
        assert_eq!(
            prefixes.len(),
            32,
            "each native loaded position keeps its exact frontier"
        );
        assert!(
            prefixes.iter().any(|binding| binding.deferred),
            "optional inline bytes exhausted the aggregate budget"
        );
        let last = prefixes
            .iter()
            .find(|binding| binding.path[3] == "31")
            .unwrap()
            .content_ref
            .clone();
        suffix_tx.send(true).unwrap();
        let update = receive_until_metadata(&subscription, |metadata| {
            metadata.content.iter().any(|binding| {
                binding.path == ["attempt", "in_flight", "blocks", "31", "text"]
                    && binding.content_ref.readable_end > last.readable_end
            })
        })
        .await;
        let RuntimeClientEvent::ProjectionTransition { metadata, .. } = &update.event else {
            unreachable!()
        };
        let event_ref = &metadata
            .content
            .iter()
            .find(|binding| binding.path == ["attempt", "in_flight", "blocks", "31", "text"])
            .unwrap()
            .content_ref;
        let after = inner.checkpoint().unwrap();
        assert_live_block_count(&after, 32);
        assert!(
            after
                .content
                .iter()
                .any(|binding| &binding.content_ref == event_ref)
        );
        assert!(event_ref.readable_end > last.readable_end);
        assert!(
            super::super::checkpoint::bounded_bytes(
                &after,
                super::super::checkpoint::CHECKPOINT_TARGET_BYTES
            )
            .is_some()
        );
        // Armed before the trigger: settlement wakes only registered waiters.
        let settled = fixture.runtime.settlement_signal().notified();
        tokio::pin!(settled);
        settled.as_mut().enable();
        finish_tx.send(true).unwrap();
        settled.await;
        inner.unpark_projection_worker();
    }

    /// T16: identity and registration commit only bookkeeping under Projection.
    /// Holding the source-preparation gate must not prevent any registration
    /// linearization or its cleanup. The channel proves completion while the
    /// gate remains held; no schedule-dependent delay is used.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn t16_registration_and_identity_do_not_acquire_source_preparation() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let inner = fixture.host.weak_inner().upgrade().expect("resident host");
        inner.park_projection_worker();
        let cursor = inner.state.lock().unwrap().projection.cursor();
        let preparation = inner.observation_preparation.lock().unwrap();
        let (complete_tx, complete_rx) = std::sync::mpsc::channel();
        let worker_inner = inner.clone();
        let task = tokio::task::spawn_blocking(move || {
            let attachment = worker_inner.admit_attachment_identity(false).unwrap();
            let (first, _) = worker_inner
                .subscribe_events(attachment.attachment_id(), cursor)
                .unwrap();
            assert!(matches!(first.try_next(), EventDelivery::Pending));
            let (second, _) = worker_inner
                .subscribe_events(attachment.attachment_id(), cursor)
                .unwrap();
            assert!(worker_inner.subscription_superseded(attachment.attachment_id(), &first));
            assert!(matches!(first.try_next(), EventDelivery::Closed));
            drop(first);
            worker_inner.detach(attachment.attachment_id());
            assert!(matches!(second.try_next(), EventDelivery::Closed));
            drop(second);
            drop(attachment);
            complete_tx.send(()).unwrap();
        });
        complete_rx
            .recv()
            .expect("bookkeeping completes with preparation fenced");
        drop(preparation);
        task.await.unwrap();
    }

    /// First attachment succeeds, the second concurrent attachment is
    /// rejected deterministically without evicting the first, detach
    /// permits a later attachment with a distinct identity, and request
    /// ids are attachment-scoped.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn attachment_lifecycle_and_request_id_scope() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let (first, initialized) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("first attach");
        let RuntimeClientResult::Initialized { attachment_id, .. } = &initialized else {
            panic!("initialized result");
        };
        let first_id = attachment_id.clone();
        // Activation's first publication is the runtime's adoption eligibility.
        assert_eq!(
            fixture
                .host
                .checkpoint()
                .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
                .unwrap()
                .1,
            RuntimeClientCursor::new(1)
        );

        let second = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION);
        assert!(matches!(
            second,
            Err(RuntimeClientError::AttachmentInUse {
                existing_attachment_id,
            }) if existing_attachment_id == first_id
        ));

        // The rejection never evicts the first attachment: its requests
        // still work.
        let response = first.handle_request(RuntimeClientRequest::CheckpointGet {
            id: crate::runtime_client::RequestId::new(1),
        });
        assert_eq!(response.id.get(), 1);
        assert!(response.error.is_none());
        assert!(matches!(
            response.result,
            Some(RuntimeClientResult::Checkpoint { .. })
        ));

        // Incompatible protocol version fails explicitly.
        let bad = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION + 1);
        assert!(matches!(
            bad,
            Err(RuntimeClientError::UnsupportedProtocolVersion {
                requested,
                ..
            }) if requested == crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION + 1
        ));

        // Explicit detach releases the attachment; a fresh attachment has
        // a distinct identity and a fresh request-id scope.
        first.detach();
        let (second_attachment, initialized) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach after detach");
        let RuntimeClientResult::Initialized {
            attachment_id: second_id,
            ..
        } = initialized
        else {
            panic!("initialized result");
        };
        assert_ne!(first_id, second_id);
        let response = second_attachment.handle_request(RuntimeClientRequest::CheckpointGet {
            id: crate::runtime_client::RequestId::new(1),
        });
        assert_eq!(response.id.get(), 1, "request ids are attachment-scoped");
        assert!(response.error.is_none());
    }

    /// Read-only inspection attachments share the live projection without
    /// competing with the one control owner or changing provider admission.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn read_only_attachments_coexist_with_the_control_attachment() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let (control, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("control attachment");
        let (inspection_one, _) = fixture
            .host
            .attach_read_only(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("first read-only attachment");
        let (inspection_two, _) = fixture
            .host
            .attach_read_only(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("second read-only attachment");
        assert_ne!(
            inspection_one.attachment_id(),
            inspection_two.attachment_id()
        );

        let rejection = inspection_one.handle_request(RuntimeClientRequest::CancelCurrentAttempt {
            id: crate::runtime_client::RequestId::new(1),
        });
        assert!(matches!(
            rejection.error,
            Some(RuntimeClientError::InvalidState { message })
                if message == "conversation inspection is read-only"
        ));
        assert!(
            inspection_two
                .handle_request(RuntimeClientRequest::CheckpointGet {
                    id: crate::runtime_client::RequestId::new(2),
                })
                .error
                .is_none()
        );
        assert!(
            control
                .handle_request(RuntimeClientRequest::CheckpointGet {
                    id: crate::runtime_client::RequestId::new(3),
                })
                .error
                .is_none(),
            "read-only attachments do not displace control ownership"
        );
    }

    /// Dropping the attachment releases it (RAII detach semantics).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn dropping_the_attachment_detaches_it() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        drop(attachment);
        let (second, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach after drop");
        assert!(
            second
                .handle_request(RuntimeClientRequest::CheckpointGet {
                    id: crate::runtime_client::RequestId::new(1),
                })
                .error
                .is_none()
        );
    }

    /// A detached handle rejects requests deterministically.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn detached_handle_rejects_requests() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        attachment.detach();
        let response = attachment.handle_request(RuntimeClientRequest::CheckpointGet {
            id: crate::runtime_client::RequestId::new(7),
        });
        assert_eq!(response.id.get(), 7);
        assert!(matches!(
            response.error,
            Some(RuntimeClientError::NotAttached)
        ));
    }

    fn print_agent_request_delta(
        turns: usize,
        current: &crate::agent::execution::attribution::AgentRequestProfile,
        previous: &crate::agent::execution::attribution::AgentRequestProfile,
    ) {
        for (name, current, previous) in [
            ("prepare", &current.prepare, &previous.prepare),
            ("start", &current.start, &previous.start),
            ("commit", &current.commit, &previous.commit),
            (
                "commit_evidence",
                &current.commit_evidence,
                &previous.commit_evidence,
            ),
            (
                "context_projection",
                &current.context_projection,
                &previous.context_projection,
            ),
            ("source", &current.source, &previous.source),
            (
                "source_capture",
                &current.source_capture,
                &previous.source_capture,
            ),
            (
                "source_authenticate",
                &current.source_authenticate,
                &previous.source_authenticate,
            ),
            ("source_leaf", &current.source_leaf, &previous.source_leaf),
            (
                "source_scalar",
                &current.source_scalar,
                &previous.source_scalar,
            ),
            (
                "source_paths",
                &current.source_paths,
                &previous.source_paths,
            ),
            (
                "source_length",
                &current.source_length,
                &previous.source_length,
            ),
            (
                "context_observe",
                &current.context_observe,
                &previous.context_observe,
            ),
            (
                "context_measure",
                &current.context_measure,
                &previous.context_measure,
            ),
            (
                "input_assembly",
                &current.input_assembly,
                &previous.input_assembly,
            ),
        ] {
            eprintln!(
                "native agent phase turns={turns} phase={name} calls={} inclusive_us={}",
                current.calls - previous.calls,
                current.total_us - previous.total_us,
            );
        }
    }

    fn print_agent_work_delta(
        turns: usize,
        current: &crate::agent::execution::attribution::AgentRequestProfile,
        previous: &crate::agent::execution::attribution::AgentRequestProfile,
    ) {
        for (scope, current, previous) in [
            ("request", &current.source_work, &previous.source_work),
            (
                "authenticate",
                &current.source_auth_work,
                &previous.source_auth_work,
            ),
            (
                "new_leaf",
                &current.source_leaf_work,
                &previous.source_leaf_work,
            ),
        ] {
            eprintln!(
                "native source work turns={turns} scope={scope} inclusive={:?}",
                current.since(previous)
            );
        }
        eprintln!(
            "native context work turns={turns} delta={:?}",
            current.context_work.since(&previous.context_work)
        );
        eprintln!(
            "native input work turns={turns} delta={:?}",
            current.native_work.since(&previous.native_work)
        );
    }

    fn assert_agent_profile_complete(
        agent: &crate::agent::execution::attribution::AgentRequestProfile,
        turns: usize,
    ) {
        let turns = u64::try_from(turns).unwrap();
        for phase in [
            agent.prepare,
            agent.start,
            agent.commit,
            agent.commit_evidence,
            agent.source,
            agent.source_capture,
            agent.source_scalar,
            agent.source_paths,
            agent.source_length,
        ] {
            assert_eq!(phase.calls, turns);
        }
        assert_eq!(agent.source_work.samples, turns);
        assert!(agent.source_work.index_pages > 0 && agent.source_work.index_bytes > 0);
        assert!(agent.source_work.metadata_blobs > 0 && agent.source_work.metadata_bytes > 0);
        assert!(
            agent.source_work.native_header_queries > 0
                && agent.source_work.native_node_queries > 0
        );
        assert!(agent.source_work.new_leaves > 0 && agent.source_work.new_nodes > 0);
        if turns > 1 {
            assert!(agent.source_auth_work.samples > 0);
        }
    }

    async fn await_profile_settlement(subscription: &EventSubscription) {
        let events = receive_until(subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        })
        .await;
        assert_eq!(
            events
                .iter()
                .flat_map(|event| event.event.facts())
                .filter(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
                .count(),
            1
        );
    }

    /// Measures inclusive elapsed phases of frozen publication and native execution.
    /// Source receipts, runtime admission and every settlement remain native.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[ignore = "measurement only; run explicitly with --ignored --nocapture"]
    async fn t19_native_publication_encoding_profile() {
        let turns = std::env::var("RUSTX_PUBLICATION_PROFILE_TURNS").map_or(20, |value| {
            value.parse::<usize>().expect("profile turn count")
        });
        assert!((1..=1000).contains(&turns));
        let (adapter, fixture) = host_fixture_with_native_tools(
            (0..turns).map(|_| one_turn_stop()).collect(),
            ToolRegistry::new(),
            status_engine(),
            true,
        )
        .await;
        let inner = fixture.host.weak_inner().upgrade().unwrap();
        let profile = Arc::new(Mutex::new(
            super::super::projection::PublicationEncodingProfile::default(),
        ));
        inner
            .state
            .lock()
            .unwrap()
            .projection
            .profile_encoding(Arc::clone(&profile));
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let subscription = attachment
            .subscribe_events(fixture.host.checkpoint().unwrap().cursor)
            .unwrap();
        let started = std::time::Instant::now();
        let mut previous = super::super::projection::PublicationEncodingProfile::default();
        let agent_profile = crate::agent::execution::attribution::start();
        let mut previous_agent = agent_profile.snapshot();
        for turn in 0..turns {
            fixture
                .host
                .submit_inbound(submit_content(&format!("encoding measurement {turn}")))
                .unwrap();
            await_profile_settlement(&subscription).await;
            if (turn + 1) % 25 == 0 {
                let current = profile.lock().unwrap().clone();
                eprintln!(
                    "native encoding turns={} elapsed_ms={} records={} bytes={} seed_us={} capture_us={} fit_us={} probe_us={} encode_us={} content_metadata_max={}",
                    turn + 1,
                    started.elapsed().as_millis(),
                    current.records - previous.records,
                    current.bytes - previous.bytes,
                    current.seed_us - previous.seed_us,
                    current.capture_us - previous.capture_us,
                    current.fit_us - previous.fit_us,
                    current.probe_us - previous.probe_us,
                    current.encode_us - previous.encode_us,
                    current.content_metadata_max
                );
                previous = current;
                let current_agent = agent_profile.snapshot();
                print_agent_request_delta(turn + 1, &current_agent, &previous_agent);
                print_agent_work_delta(turn + 1, &current_agent, &previous_agent);
                previous_agent = current_agent;
            }
        }
        assert_eq!(adapter.requests().len(), turns);
        let elapsed = started.elapsed();
        fixture.runtime.shutdown().await.unwrap();
        let profile = profile.lock().unwrap();
        assert!(profile.records >= turns as u64);
        assert!(profile.bytes > 0);
        let agent = agent_profile.snapshot();
        assert_agent_profile_complete(&agent, turns);
        eprintln!(
            "native publication encoding{turns} elapsed_ms={} {profile:?} agent_inclusive={agent:?}",
            elapsed.as_millis()
        );
    }

    /// Captures real native authority on both sides of a contiguous stream.
    /// The TypeScript regression consumes this wire capture; its expected state
    /// is produced only by an independent native snapshot read.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)] // One end-to-end capture keeps the independent authorities visible.
    async fn incremental_projection_independent_snapshot_capture() {
        use crate::app_server::protocol::{
            AttachmentTarget, JsonRpcVersion, MethodResult, Notification, NotificationMethod,
            RequestId as WireId, Response, Success,
        };
        let (tool, _started, release) = ParkingBackgroundTool::new();
        release.send_replace(true);
        let definition = ToolDefinition {
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            ..tool.definition.clone()
        };
        let mut tools = ToolRegistry::new();
        tools.register(definition.clone(), Arc::new(tool)).unwrap();
        let call_id = ToolCallId::new("capture-call");
        let script = vec![
            GatedStep::Emit(ModelEvent::Started),
            GatedStep::Emit(ModelEvent::ReasoningDelta {
                block_index: ContentBlockIndex::new(0),
                text: "reasoning".into(),
            }),
            GatedStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(1),
                text: "first ".into(),
            }),
            GatedStep::Emit(ModelEvent::TextDelta {
                block_index: ContentBlockIndex::new(1),
                text: "second".into(),
            }),
            GatedStep::Emit(ModelEvent::RefusalDelta {
                block_index: ContentBlockIndex::new(2),
                text: "refusal".into(),
            }),
            GatedStep::Emit(ModelEvent::ToolCallStarted {
                block_index: ContentBlockIndex::new(3),
                call: crate::tools::types::ToolCallStart {
                    id: call_id.clone(),
                    tool_id: definition.id.clone(),
                    name: definition.name.clone(),
                },
            }),
            GatedStep::Emit(ModelEvent::ToolCallArgumentsDelta {
                block_index: ContentBlockIndex::new(3),
                call_id: call_id.clone(),
                arguments_delta: "{".into(),
            }),
            GatedStep::Emit(ModelEvent::ToolCallArgumentsDelta {
                block_index: ContentBlockIndex::new(3),
                call_id: call_id.clone(),
                arguments_delta: "}".into(),
            }),
            GatedStep::Emit(ModelEvent::ToolCallCompleted {
                block_index: ContentBlockIndex::new(3),
                call: ToolCall {
                    id: call_id,
                    tool_id: definition.id,
                    name: definition.name,
                    arguments: serde_json::json!({}),
                },
            }),
            GatedStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::ToolCalls,
                usage: None,
            }),
        ];
        let (_, fixture) =
            host_fixture(vec![script, one_turn_stop()], tools, status_engine()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let initial = fixture.host.inner.checkpoint().unwrap();
        let c = initial.cursor;
        let subscription = attachment.subscribe_events(c).unwrap();
        fixture
            .host
            .submit_inbound(submit_content("projection equivalence"))
            .unwrap();
        let mut events = receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        })
        .await;
        // Attempt settlement is a semantic event, not completion of the
        // independently owned presentation read. Capture the equivalence cut
        // only after the terminal response's decorated domain was published.
        let terminal_read = |event: &RuntimeClientProtocolEvent| {
            event.event.facts().iter().any(|fact| {
                matches!(fact, RuntimeClientEvent::ReadDomainsUpdated { transcript, .. }
                    if transcript.entries.iter().any(|entry| entry.completed_response.is_some()))
            })
        };
        if !events.iter().any(terminal_read) {
            events.extend(receive_until(&subscription, terminal_read).await);
        }
        let expected = fixture.host.inner.checkpoint().unwrap();
        let n = expected.cursor;
        while events.last().is_none_or(|event| event.cursor < n) {
            let EventDelivery::Event(event) = subscription.next().await else {
                panic!("contiguous event");
            };
            events.push(event);
        }
        let mut cursor = c.get();
        for event in &events {
            cursor += 1;
            assert_eq!(event.cursor.get(), cursor);
        }
        assert_eq!(cursor, n.get());
        let (expired_cursor, expired_reference) = events
            .iter()
            .find_map(|event| {
                event.event.facts().iter().find_map(|fact| match fact {
                    RuntimeClientEvent::BlockContentAdvanced { content_ref, .. }
                        if content_ref.readable_end > 0
                            && matches!(
                                &content_ref.source,
                                crate::runtime_client::content::ContentSource::LivePublicationPrefix { .. }
                            ) => Some((event.cursor, content_ref.clone())),
                    _ => None,
                })
            })
            .expect("actual released nonempty LiveRef in the original native stream");
        // Canonical acceptance has expired this untouched original Live source.
        // Neither Replay retention nor a later read can redirect it to C/Audit.
        assert!(matches!(
            fixture
                .store
                .as_ref()
                .unwrap()
                .content_read(&expired_reference, 0, 1),
            Err(crate::durable::ConversationStoreError::InvalidReference(_))
        ));
        assert!(matches!(
            fixture.host.inner.content_read(&expired_reference, 0, 1),
            Err(RuntimeClientError::ContentSourceUnavailable)
        ));
        // Subscribe only after expiry. These are retained native records, not
        // the already-consumed live delivery vector or a synthetic replay.
        let replay = attachment.subscribe_events(c).unwrap();
        let mut replayed = Vec::with_capacity(events.len());
        for original in &events {
            let EventDelivery::Event(event) = replay.try_next() else {
                panic!("the exact original cut remains serviceable after Live expiry");
            };
            assert_eq!(event.cursor, original.cursor);
            replayed.push(event);
        }
        assert_eq!(replayed, events);
        assert_eq!(replayed.last().unwrap().cursor, n);
        assert!(expected.snapshot.transcript.statistics.is_some());
        assert!(
            expected
                .snapshot
                .transcript
                .entries
                .iter()
                .any(|entry| entry.completed_response.is_some())
        );
        let checkpoint = |value| {
            wire_value(&Response::Success(Box::new(Success {
                jsonrpc: JsonRpcVersion::V2,
                id: WireId::String("capture".into()),
                result: MethodResult::Checkpoint {
                    checkpoint: Box::new(value),
                },
            })))
        };
        let target = AttachmentTarget {
            session_id: crate::local_runtime::session::SessionId::new(
                "ses_00000000-0000-7000-8000-000000000420",
            ),
            conversation_id: initial.snapshot.conversation_id.clone(),
            runtime_incarnation: serde_json::from_str("1").unwrap(),
            attachment_id: attachment.attachment_id().clone(),
        };
        let wire: Vec<_> = replayed
            .into_iter()
            .map(|event| {
                wire_value(&Notification {
                    jsonrpc: JsonRpcVersion::V2,
                    notification: NotificationMethod::Event {
                        target: target.clone(),
                        registration_id: "capture-registration".into(),
                        cursor: event.cursor,
                        event: Box::new(event.event),
                    },
                })
            })
            .collect();
        if let Ok(path) = std::env::var("RUSTX_PROJECTION_CAPTURE") {
            std::fs::write(path, serde_json::to_string_pretty(&serde_json::json!({
                "initial": checkpoint(initial), "events": wire, "expected": checkpoint(expected),
                "expired_live_reference": {
                    "cursor": expired_cursor.get().to_string(),
                    "reference": wire_value(&expired_reference),
                },
            })).unwrap()).unwrap();
        }
    }

    /// Submitting while idle admits and runs the attempt through the
    /// conversation runtime's single admission path; the admission response
    /// is accepted, not finished; the attempt settles and the canonical
    /// history is committed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn submit_when_idle_admits_and_runs_the_attempt() {
        let (adapter, fixture) =
            host_fixture(vec![one_turn_stop()], ToolRegistry::new(), status_engine()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");

        let response = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("hello"),
        });
        let RuntimeClientResult::InboundAccepted {
            message_id,
            inbound_sequence,
        } = response.result.expect("accepted")
        else {
            panic!("accepted result");
        };
        assert_eq!(inbound_sequence.get(), 1);
        assert_eq!(
            message_id.as_str(),
            "conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b-inbound-1"
        );

        // The attempt settles asynchronously; the subscription observes the
        // terminal settlement exactly once.
        let mut settled = 0;
        let events = receive_until(&subscription, |event| {
            if event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
            {
                settled += 1;
                return true;
            }
            false
        })
        .await;
        let settled_events: Vec<_> = events
            .iter()
            .filter(|event| {
                event
                    .event
                    .facts()
                    .iter()
                    .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
            })
            .collect();
        assert_eq!(settled_events.len(), 1);
        assert_eq!(settled, 1);

        // The first model request observed the admitted Runtime Agent Status
        // fact through canonical history.
        let requests = adapter.requests();
        assert_eq!(requests.len(), 1);
        assert_adopted_input(&requests[0], &message_id);

        // The snapshot carries the committed canonical history and the
        // settled attempt.
        let (snapshot, _) = checkpoint_state(&fixture.host);
        assert_eq!(
            snapshot.messages.len(),
            3,
            "user message + admitted Agent Status + Assistant message"
        );
        assert!(matches!(
            snapshot.attempt.expect("attempt view").phase,
            RuntimeClientAttemptPhase::Settled { .. }
        ));
        // The terminal settlement event is emitted by the loop, and the
        // authoritative canonical history is committed by the runtime's
        // settlement path immediately afterwards. Observing the commit is a
        // wait on that exact condition, never a delay.
        await_canonical_history(&fixture.host, &snapshot.messages).await;

        // Request facts remain in the durable ConversationStore after the
        // AgentExecutionResult transfer. Mutate the live session
        // configuration after settlement and reconstruct from the durable
        // snapshot plus its historical Surface; neither current
        // configuration nor a live contributor is consulted.
        let requests = adapter.requests();
        let history = fixture.host.request_history();
        let snapshots = request_snapshots(&history);
        assert_eq!(snapshots.len(), 1);
        let retained = snapshots[0].clone();
        let mut live_config = fixture.runtime.model_config();
        live_config.request_params.insert(
            "live_mutation".to_owned(),
            serde_json::json!("changed-after-settlement"),
        );
        fixture
            .host
            .model_set(live_config)
            .expect("live model mutation remains valid");
        let reconstructed = fixture
            .host
            .reconstruct_request(&retained.identity)
            .expect("retained request reconstructs after settlement");
        assert_eq!(reconstructed, requests[0]);
        assert_eq!(
            history.get(&retained.identity).unwrap(),
            Some(retained),
            "request history lookup is identity-based and immutable"
        );
    }

    /// A composed runtime keeps every actual primary request, including an
    /// overflow retry, reconstructable in the durable `ConversationStore`
    /// after the `AgentExecutionResult` has been transferred and dropped. The
    /// retry keeps the pending fresh inbound visible while both request facts
    /// remain reconstructable from their own Surface revisions.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)] // one durable overflow/retry scenario
    async fn settled_host_retains_distinct_overflow_request_snapshots() {
        let (adapter, fixture) = host_fixture(
            vec![
                one_turn_stop(),
                vec![GatedStep::Emit(ModelEvent::Failed {
                    error: ModelError {
                        kind: ModelErrorKind::ContextWindowExceeded,
                        message: "context window exceeded".to_owned(),
                        retry_disposition: crate::model::error::ModelRetryDisposition::Never,
                        retry_after_ms: None,
                        provider_code: None,
                        context_overflow: None,
                        malformed_tool_proposal: None,
                        timeout_phase: None,
                        generation: None,
                    },
                })],
                vec![
                    GatedStep::Emit(ModelEvent::Started),
                    GatedStep::Emit(ModelEvent::TextDelta {
                        block_index: ContentBlockIndex::new(0),
                        text: "historical summary".to_owned(),
                    }),
                    GatedStep::Emit(ModelEvent::Completed {
                        finish_reason: ModelFinishReason::Stop,
                        usage: None,
                    }),
                ],
                one_turn_stop(),
            ],
            ToolRegistry::new(),
            status_engine(),
        )
        .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");

        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("first"),
        });
        receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        })
        .await;
        await_request_history_len(&fixture.host, 1).await;

        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(2),
            content: submit_content("second"),
        });
        receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        })
        .await;
        await_request_history_len(&fixture.host, 3).await;

        let history = fixture.host.request_history();
        let snapshots = request_snapshots(&history);
        assert_eq!(snapshots.len(), 3);
        assert_eq!(snapshots[0].identity.retry_number, 0);
        assert_eq!(snapshots[1].identity.retry_number, 0);
        assert_eq!(snapshots[2].identity.retry_number, 1);
        assert_eq!(
            snapshots[1].identity.attempt_id,
            snapshots[2].identity.attempt_id
        );
        assert_eq!(
            snapshots[1].context_generation, snapshots[2].context_generation,
            "overflow retry keeps the one admitted context generation"
        );
        assert_ne!(
            snapshots[1].history_cut.activation, snapshots[2].history_cut.activation,
            "compaction gives the retry a distinct typed historical activation"
        );

        let provider_requests = adapter.requests();
        assert_eq!(
            provider_requests.len(),
            4,
            "three primary requests plus summary"
        );
        for (snapshot, request) in snapshots.iter().zip([
            &provider_requests[0],
            &provider_requests[1],
            &provider_requests[3],
        ]) {
            assert_eq!(
                fixture
                    .host
                    .reconstruct_request(&snapshot.identity)
                    .expect("settled historical request reconstructs"),
                *request
            );
        }
        assert!(
            provider_requests[3].messages.iter().any(|message| {
                matches!(
                    message,
                    crate::model::ModelInputMessage::Canonical(MessageBlock::User(user)) if user.id.as_str() == "conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b-inbound-2"
                )
            }),
            "the retry still observes the pending fresh inbound"
        );
    }

    /// Waits until the runtime owns the conversation state again and its
    /// Message Ledger equals the expected records.
    async fn await_canonical_history(host: &RuntimeClientHost, expected: &[MessageBlock]) {
        tokio::time::timeout(std::time::Duration::from_mins(2), async {
            loop {
                if host.host_ledger().as_deref() == Some(expected) {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the projection mirrors the authoritative canonical history");
    }

    /// Waits for the durable request-fact read to expose the expected count.
    async fn await_request_history_len(host: &RuntimeClientHost, expected: usize) {
        tokio::time::timeout(std::time::Duration::from_mins(2), async {
            loop {
                if request_snapshots(&host.request_history()).len() == expected {
                    return;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("request history transfer must settle");
    }

    /// Waits until the fake provider has actually received the expected
    /// number of provider-neutral requests. Request snapshots are durable
    /// before adapter invocation, so request-history visibility alone is not
    /// a sufficient synchronization point for provider-side assertions.
    async fn await_adapter_request_count(adapter: &GatedAdapter, expected: usize) {
        let mut count = adapter.request_count();
        tokio::time::timeout(
            std::time::Duration::from_mins(2),
            count.wait_for(|actual| *actual >= expected),
        )
        .await
        .expect("provider invocation must settle")
        .expect("provider request-count signal must stay open");
    }

    /// Submitting while an attempt is running queues the message in the
    /// authoritative mailbox; the running attempt drains it at its next
    /// safe boundary. An enqueue during an active attempt never creates a
    /// second `AgentExecution`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn submit_while_busy_queues_for_the_next_drain() {
        let (release_tx, release_rx) = model_release();
        let (adapter, fixture) = host_fixture(
            vec![
                // Turn 1 parks in its stream; after release it completes
                // with Stop and the safe boundary drains the queued
                // message into a second turn.
                vec![
                    GatedStep::Emit(ModelEvent::Started),
                    GatedStep::Emit(ModelEvent::TextDelta {
                        block_index: ContentBlockIndex::new(0),
                        text: "working".to_owned(),
                    }),
                    GatedStep::ParkUntilReleased(release_rx),
                    GatedStep::Emit(ModelEvent::Completed {
                        finish_reason: ModelFinishReason::Stop,
                        usage: None,
                    }),
                ],
                one_turn_stop(),
            ],
            ToolRegistry::new(),
            status_engine(),
        )
        .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");

        let first = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("first"),
        });
        assert!(matches!(
            first.result,
            Some(RuntimeClientResult::InboundAccepted { .. })
        ));

        // Wait until the first request is in flight (the adapter has been
        // asked) so the second submit provably arrives while busy.
        receive_until(&subscription, |event| {
            event.event.facts().iter().any(|fact| matches!(fact, RuntimeClientEvent::BlockContentAdvanced { kind, .. } if kind == "text"))
        })
        .await;
        assert_eq!(adapter.requests().len(), 1);
        assert!(fixture.host.has_current_attempt());

        let second = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(2),
            content: submit_content("second"),
        });
        let RuntimeClientResult::InboundAccepted {
            message_id: second_id,
            inbound_sequence,
        } = second.result.expect("accepted")
        else {
            panic!("accepted result");
        };
        assert_eq!(inbound_sequence.get(), 2);

        // While the attempt is parked, the second message remains pending
        // in the authoritative mailbox diagnostics, and no second
        // AgentExecution exists.
        let (snapshot, _) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        assert_eq!(snapshot.inbound.pending.len(), 1);
        assert_eq!(snapshot.inbound.pending[0].message.id, second_id);
        assert_eq!(adapter.requests().len(), 1, "no second AgentExecution yet");

        // Release the parked turn: the safe boundary drains the queued
        // message and a second turn observes it within the SAME attempt.
        release_tx.send(true).expect("release");
        receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        })
        .await;
        let requests = adapter.requests();
        assert_eq!(requests.len(), 2, "the drained batch opens a second turn");
        assert!(
            requests[1]
                .messages
                .iter()
                .any(|message| matches!(message, crate::model::ModelInputMessage::Canonical(MessageBlock::User(user)) if user.id == second_id))
        );
        let (snapshot, _) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        assert!(
            snapshot.inbound.pending.is_empty(),
            "the queued message was drained exactly once"
        );
        assert!(
            snapshot.messages.iter().any(|message| {
                matches!(message, MessageBlock::User(user) if user.id == second_id)
            }),
            "the drained message committed to canonical history"
        );
    }

    /// Cancelling the current attempt: the acceptance response is not
    /// terminal settlement; the actual runtime settlement is observed
    /// exactly once after the model releases.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancel_current_attempt_acceptance_is_not_settlement() {
        let (release_tx, release_rx) = model_release();
        let (_, fixture) = host_fixture(
            vec![vec![
                GatedStep::Emit(ModelEvent::Started),
                GatedStep::ParkUntilReleased(release_rx),
                GatedStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ]],
            ToolRegistry::new(),
            status_engine(),
        )
        .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");

        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("go"),
        });
        receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptStarted { .. }))
        })
        .await;

        // The cancel response is acceptance, never terminal settlement.
        let response = attachment.handle_request(RuntimeClientRequest::CancelCurrentAttempt {
            id: crate::runtime_client::RequestId::new(2),
        });
        let Some(RuntimeClientResult::AttemptCancellationAccepted { attempt_id }) = response.result
        else {
            panic!("cancellation of a running attempt is accepted, got {response:?}");
        };
        let (snapshot, _) = checkpoint_state(&fixture.host);
        let view = snapshot.attempt.expect("attempt view");
        assert_eq!(
            view.attempt_id,
            Some(attempt_id),
            "acceptance names the attempt the snapshot describes"
        );
        assert!(
            matches!(
                view.phase,
                RuntimeClientAttemptPhase::Running | RuntimeClientAttemptPhase::Settled { .. }
            ),
            "an accepted cancellation leaves the attempt running or already settled"
        );

        // A second cancel is idempotent at the signal level.
        let second_cancel = attachment.handle_request(RuntimeClientRequest::CancelCurrentAttempt {
            id: crate::runtime_client::RequestId::new(3),
        });
        assert!(
            second_cancel.error.is_none()
                || matches!(
                    second_cancel.error,
                    Some(RuntimeClientError::NoCurrentAttempt)
                ),
            "a second cancel is accepted or reports no cancellable attempt, got {second_cancel:?}"
        );

        // Release the parked model so it can finish.
        let _ = release_tx.send(true);
        let events = receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        })
        .await;
        let settled: Vec<_> = events
            .iter()
            .filter(|event| {
                event.event.facts().iter().any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled {
                        outcome: crate::runtime_client::event::RuntimeClientOutcome::Cancelled { .. },
                        ..
                    }))
            })
            .collect();
        assert_eq!(
            settled.len(),
            1,
            "terminal cancellation observed exactly once"
        );
        let (snapshot, _) = checkpoint_state(&fixture.host);
        assert!(matches!(
            snapshot.attempt.expect("attempt view").phase,
            RuntimeClientAttemptPhase::Settled {
                outcome: crate::runtime_client::event::RuntimeClientOutcome::Cancelled { .. }
            }
        ));
    }

    /// With no attempt running, cancel returns a deterministic typed
    /// error.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cancel_with_no_attempt_fails_typed() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let response = attachment.handle_request(RuntimeClientRequest::CancelCurrentAttempt {
            id: crate::runtime_client::RequestId::new(1),
        });
        assert!(matches!(
            response.error,
            Some(RuntimeClientError::NoCurrentAttempt)
        ));
    }

    /// Detaching an attachment never cancels the active attempt: the
    /// attempt continues to settlement after detach, and a reattached
    /// client observes the terminal settlement.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn detach_is_not_cancellation() {
        let (release_tx, release_rx) = model_release();
        let (adapter, fixture) = host_fixture(
            vec![vec![
                GatedStep::Emit(ModelEvent::Started),
                GatedStep::ParkUntilReleased(release_rx),
                GatedStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ]],
            ToolRegistry::new(),
            status_engine(),
        )
        .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("go"),
        });
        // Native request-start publication precedes provider invocation. Use
        // that exact signal instead of polling snapshots through moving cuts.
        await_adapter_request_count(&adapter, 1).await;
        // Detach while the attempt is parked.
        attachment.detach();
        // A reattached client sees the attempt still running.
        let (second, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("reattach");
        let (snapshot, cursor) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        assert!(
            matches!(
                snapshot.attempt.expect("attempt view").phase,
                RuntimeClientAttemptPhase::Running
            ),
            "detach never cancels the attempt"
        );
        // The attempt completes normally after release.
        release_tx.send(true).expect("release");
        let subscription = second
            .subscribe_events(cursor)
            .expect("resume from the retained cursor");
        receive_until(&subscription, |event| {
            event.event.facts().iter().any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled {
                    outcome: crate::runtime_client::event::RuntimeClientOutcome::Completed { .. },
                    ..
                }))
        })
        .await;
    }

    fn publish_pending_mutation(
        inner: &Arc<super::ClientInner>,
        outcome: crate::durable::inbox::PendingMutationOutcome,
    ) {
        let crate::durable::inbox::PendingMutationOutcome::Applied {
            pending_observation,
            read_cut,
        } = outcome
        else {
            panic!("native pending mutation applied");
        };
        let sink = super::NativePublicationProjection {
            inner: Arc::downgrade(inner),
        };
        crate::runtime::observation::PublicationProjectionCommit::native_pending(
            &sink,
            pending_observation.as_deref(),
            &read_cut,
        );
        if let Some(cursor) = pending_observation.as_ref().and_then(|receipt| {
            receipt
                .selected
                .as_ref()
                .and_then(|header| header.message.transcript_cursor)
                .or(receipt.removed_transcript)
        }) {
            inner
                .store
                .mark_observation_transcript_ready(cursor.get())
                .unwrap();
        }
    }

    fn pending_terminal_diagnostic(
        store: &dyn ConversationStore,
        attempt: &AttemptId,
    ) -> Vec<String> {
        let mut after = None;
        let mut diagnostics = Vec::new();
        for _ in 0..4 {
            let page = store
                .read_events(after, 32)
                .expect("original fixture journal remains readable");
            if page.events.is_empty() {
                break;
            }
            for envelope in page.events {
                after = Some(envelope.sequence);
                if envelope.attempt_id.as_ref() != Some(attempt) {
                    continue;
                }
                match envelope.event {
                    RuntimeEvent::ModelRequestFailed { error, .. } => diagnostics.push(format!(
                        "J{} model_failed {:?} phase={:?}",
                        envelope.sequence, error.kind, error.timeout_phase
                    )),
                    RuntimeEvent::AttemptCompleted { .. } => {
                        diagnostics.push(format!("J{} completed", envelope.sequence));
                    }
                    RuntimeEvent::AttemptCancelled { reason, .. } => {
                        diagnostics.push(format!("J{} cancelled {reason:?}", envelope.sequence));
                    }
                    RuntimeEvent::AttemptFailed { .. } => {
                        diagnostics.push(format!("J{} failed", envelope.sequence));
                    }
                    RuntimeEvent::AttemptTimedOut { .. } => {
                        diagnostics.push(format!("J{} timed_out", envelope.sequence));
                    }
                    RuntimeEvent::AttemptLimitExceeded { .. } => {
                        diagnostics.push(format!("J{} limit_exceeded", envelope.sequence));
                    }
                    _ => {}
                }
                if diagnostics.len() == 8 {
                    return diagnostics;
                }
            }
        }
        diagnostics
    }

    async fn await_pending_fixture_publication(host: &RuntimeClientHost) {
        tokio::time::timeout(BACKGROUND_LIVENESS_GUARD, host.projection_settled())
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "pending owner publication exceeded liveness guard: {}",
                    host.weak_inner()
                        .upgrade()
                        .unwrap()
                        .projection_source_debug()
                )
            });
    }

    async fn assert_native_pending_removal_wakes_worker(
        fixture: &HostFixture,
        attempt: &AttemptId,
    ) {
        let mailbox = fixture.runtime.tool_runtime().mailbox();
        let sequence = mailbox
            .enqueue(inbound_text(
                "native-removal",
                "remove through mailbox owner",
            ))
            .unwrap();
        await_pending_fixture_publication(&fixture.host).await;
        let before = fixture.host.checkpoint().unwrap();
        assert_eq!(before.snapshot.inbound.pending.len(), 1);
        let removed = mailbox
            .remove_pending(&crate::durable::inbox::PendingInboundRef {
                sequence,
                revision: 0,
            })
            .unwrap();
        let crate::durable::inbox::PendingMutationOutcome::Applied {
            pending_observation: Some(receipt),
            read_cut,
        } = removed
        else {
            panic!("actual native removal receipt");
        };
        assert!(receipt.selected.is_none());
        assert!(receipt.removed_transcript.is_some());
        await_pending_fixture_publication(&fixture.host).await;
        let after = fixture.host.checkpoint().unwrap();
        assert!(after.cursor > before.cursor);
        assert!(after.snapshot.inbound.pending.is_empty());
        assert_eq!(after.read_cut.source_cut.as_ref().unwrap().read, read_cut);
        assert_eq!(
            after.snapshot.attempt.as_ref().unwrap().attempt_id.as_ref(),
            Some(attempt)
        );
        assert!(fixture.runtime.has_current_attempt());
    }

    async fn gated_pending_fixture(
        release_rx: tokio::sync::watch::Receiver<bool>,
    ) -> (Arc<GatedAdapter>, HostFixture) {
        let clock = Arc::new(crate::runtime::monotonic::ManualMonotonicClock::new());
        let (adapter, fixture) = host_fixture_before_activation(
            vec![vec![
                GatedStep::Emit(ModelEvent::Started),
                GatedStep::ParkUntilReleased(release_rx),
                GatedStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ]],
            ToolRegistry::new(),
            status_engine(),
            false,
            Some(clock),
        )
        .await;
        fixture.runtime.activate();
        await_fixture_activation(&fixture.host).await;
        fixture
            .runtime
            .tool_runtime()
            .mailbox()
            .enqueue(inbound_text("admitted", "running"))
            .unwrap();
        // Request-start publication precedes final prompt admission. Wait for
        // the actual provider invocation so this message stays pending behind
        // the parked Attempt instead of racing into its initial prompt.
        await_adapter_request_count(&adapter, 1).await;
        (adapter, fixture)
    }

    async fn finish_pending_fixture(
        fixture: &HostFixture,
        adapter: &GatedAdapter,
        release_tx: &watch::Sender<bool>,
        held_attempt: &AttemptId,
    ) {
        let store = fixture.runtime.tool_runtime().durable_store();
        release_tx.send(true).unwrap_or_else(|_| {
            panic!(
                "native gate receiver retired: {:?}",
                pending_terminal_diagnostic(store.as_ref(), held_attempt)
            )
        });
        let settled = fixture.runtime.child_attempt_settlement(0).await;
        assert_eq!(&settled.attempt_id, held_attempt);
        assert!(matches!(
            settled.outcome,
            crate::runtime::conversation_runtime::NativeAttemptOutcome::Completed
        ));
        assert_eq!(adapter.requests().len(), 1);
        fixture.runtime.shutdown().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn pending_snapshot_and_reattach_repair_a_committed_unpublished_mutation() {
        let (release_tx, release_rx) = model_release();
        let (adapter, fixture) = gated_pending_fixture(release_rx).await;
        let mailbox = fixture.runtime.tool_runtime().mailbox();
        let sequence = mailbox
            .enqueue(inbound_text("pending-repair", "old"))
            .unwrap();
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        await_pending_fixture_publication(&fixture.host).await;
        let inner = fixture.host.weak_inner().upgrade().unwrap();
        inner.park_projection_worker();
        let (before, cursor) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .unwrap();
        assert_eq!(before.inbound.pending.len(), 1);
        let held_attempt = before
            .attempt
            .as_ref()
            .expect("actual gated native attempt")
            .attempt_id
            .clone()
            .expect("exact native fixture attempt identity");
        assert!(fixture.runtime.has_current_attempt());
        assert!(matches!(
            before.attempt.as_ref().unwrap().phase,
            RuntimeClientAttemptPhase::Running
        ));
        let store = fixture.runtime.tool_runtime().durable_store();
        let expected = crate::durable::inbox::PendingInboundRef {
            sequence,
            revision: 0,
        };
        // The durable mutation precedes its exact mailbox-owner receipt.
        // Published state remains frozen until that native owner installs it.
        let edited = store.edit_pending(&expected, "committed edit").unwrap();
        let (published_cursor, published_inbound) = {
            let published = inner.lock_published_state();
            (
                published.projection.cursor(),
                published.projection.snapshot_ref().inbound.clone(),
            )
        };
        assert_eq!(published_cursor, cursor);
        assert_eq!(published_inbound, before.inbound);
        publish_pending_mutation(&inner, edited);
        inner.unpark_projection_worker();
        await_pending_fixture_publication(&fixture.host).await;
        let (repaired, _) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .unwrap();
        assert_eq!(repaired.inbound.pending[0].revision, 1);
        assert_eq!(
            repaired.inbound.pending[0].message.content,
            inbound_text("x", "committed edit").content
        );
        let removed = store
            .remove_pending(&crate::durable::inbox::PendingInboundRef {
                revision: 1,
                ..expected
            })
            .unwrap();
        publish_pending_mutation(&inner, removed);
        // The existing source worker installs the completed read-domain
        // result before the reconnect baseline; public reads remain pure.
        await_pending_fixture_publication(&fixture.host).await;
        assert_native_pending_removal_wakes_worker(&fixture, &held_attempt).await;
        attachment.detach();
        let (_, initialized) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let RuntimeClientResult::Initialized { .. } = initialized else {
            panic!("initialized")
        };
        let (snapshot, _) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("explicit reconnect checkpoint");
        assert!(snapshot.inbound.pending.is_empty());
        assert!(!format!("{:?}", snapshot.transcript).contains("pending-repair"));
        assert!(
            fixture.runtime.has_current_attempt()
                && snapshot
                    .attempt
                    .as_ref()
                    .is_some_and(|attempt| attempt.attempt_id.as_ref() == Some(&held_attempt)
                        && matches!(attempt.phase, RuntimeClientAttemptPhase::Running)),
            "native gated execution retired before release: {:?}",
            pending_terminal_diagnostic(store.as_ref(), &held_attempt)
        );
        finish_pending_fixture(&fixture, &adapter, &release_tx, &held_attempt).await;
    }

    /// Detaching never cancels conversation-owned background work and
    /// never drains mailbox contents: the mailbox is drained only by the
    /// conversation runtime's admission/safe-boundary authority, never by
    /// the client boundary (Test 2).
    #[allow(clippy::too_many_lines)] // one complete detach lifecycle
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn detach_never_mutates_background_or_mailbox_state() {
        let (release_tx, release_rx) = model_release();
        let (adapter, fixture) = host_fixture(
            vec![
                vec![
                    GatedStep::Emit(ModelEvent::Started),
                    GatedStep::ParkUntilReleased(release_rx),
                    GatedStep::Emit(ModelEvent::Completed {
                        finish_reason: ModelFinishReason::Stop,
                        usage: None,
                    }),
                ],
                one_turn_stop(),
            ],
            ToolRegistry::new(),
            status_engine(),
        )
        .await;
        // Dispatch one detached background execution directly through the
        // authoritative registry.
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = fixture
            .runtime
            .tool_runtime()
            .background()
            .prepare_dispatch(
                &ToolInvocation {
                    id: crate::tools::types::ToolInvocationId::Agent {
                        call_id: ToolCallId::new("call-bg"),
                    },
                    tool_id: ToolId::new("tool-bg"),
                    tool_name: "bg".to_owned(),
                    mode: ToolInvocationMode::Background,
                    arguments: serde_json::json!({}),
                },
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = fixture
            .runtime
            .tool_runtime()
            .background()
            .commit_dispatch(prepared, &CancellationSignal::new())
            .expect("dispatch commits")
        else {
            panic!("accepted dispatch");
        };
        await_background_started(&mut started, "background runner started").await;
        // One mailbox item admitted by the runtime's idle wakeup: the first
        // attempt starts and parks in its model stream.
        fixture
            .runtime
            .tool_runtime()
            .mailbox()
            .enqueue(inbound_text("msg-first", "admitted"))
            .expect("enqueue");
        await_adapter_request_count(&adapter, 1).await;
        // Exact pending removal cannot cancel the parked attempt or the
        // independently committed background execution.
        let mailbox = fixture.runtime.tool_runtime().mailbox();
        let sequence = mailbox
            .enqueue(inbound_text("msg-remove", "remove only this occurrence"))
            .unwrap();
        let before_background = fixture
            .runtime
            .tool_runtime()
            .background()
            .snapshot(&execution_id);
        assert!(matches!(
        mailbox
                        .remove_pending(&crate::durable::inbox::PendingInboundRef {
                            sequence,
                            revision: 0,
                        })
                        .unwrap(),
        crate::durable::inbox::PendingMutationOutcome::Applied { read_cut, .. } if read_cut.pending_total <= read_cut.pending_through
        ));
        assert_eq!(
            fixture
                .runtime
                .tool_runtime()
                .background()
                .snapshot(&execution_id),
            before_background
        );
        assert!(fixture.runtime.has_current_attempt());
        // A second mailbox item stays pending while the attempt runs.
        fixture
            .runtime
            .tool_runtime()
            .mailbox()
            .enqueue(inbound_text("msg-pending", "kept"))
            .expect("enqueue");

        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let (before, _) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        assert_eq!(before.jobs.len(), 1);
        assert!(matches!(before.jobs[0].state, BackgroundLifecycle::Running));
        assert_eq!(before.inbound.pending.len(), 1);
        assert_eq!(before.inbound.pending[0].message.id.as_str(), "msg-pending");
        attachment.detach();

        // After detach the background execution still runs and the mailbox
        // item still pends; the running attempt is untouched.
        let (after, _) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        assert_eq!(after.jobs.len(), 1);
        assert!(
            matches!(after.jobs[0].state, BackgroundLifecycle::Running),
            "detach never cancels background work"
        );
        assert_eq!(
            after.inbound.pending.len(),
            1,
            "detach never drains mailbox contents"
        );
        assert!(
            after.attempt.as_ref().is_some_and(|attempt| {
                matches!(attempt.phase, RuntimeClientAttemptPhase::Running)
            }),
            "detach never cancels the active attempt"
        );

        // Releasing the model lets the attempt settle: the safe boundary
        // drains the pending mailbox item into a second turn within the
        // same attempt.
        // Armed before the trigger: settlement wakes only registered waiters.
        let drained = fixture.runtime.settlement_signal().notified();
        tokio::pin!(drained);
        drained.as_mut().enable();
        release_tx.send(true).expect("release");
        await_request_history_len(&fixture.host, 2).await;
        // Request facts commit before their owner's semantic publication. The
        // native attempt completion, not durable visibility, proves the drain
        // observations have been published and may be folded by this snapshot.
        drained.await;
        let (settled, _) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        assert!(
            settled.inbound.pending.is_empty(),
            "the runtime drained the pending item at the safe boundary"
        );

        // The background execution settles normally after release.
        // Armed before the trigger: settlement wakes only registered waiters.
        let settled = fixture.runtime.settlement_signal().notified();
        tokio::pin!(settled);
        settled.as_mut().enable();
        release.send_replace(true);
        await_background_terminal(
            fixture.runtime.tool_runtime().background(),
            &execution_id,
            "detach background execution",
        )
        .await;
        settled.await;
        let (final_snapshot, _) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        assert!(matches!(
            final_snapshot.jobs[0].state,
            BackgroundLifecycle::Succeeded
        ));
    }

    /// Detach never changes canonical conversation state, and a detached
    /// conversation keeps admitting asynchronous inbound through the one
    /// coordinator path (Test 2).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn detach_does_not_affect_conversation_or_future_async_admission() {
        let (adapter, fixture) =
            host_fixture(vec![one_turn_stop()], ToolRegistry::new(), status_engine()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");
        // Armed before the trigger: settlement wakes only registered waiters.
        let first_settled = fixture.runtime.settlement_signal().notified();
        tokio::pin!(first_settled);
        first_settled.as_mut().enable();
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("first"),
        });
        receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        })
        .await;
        first_settled.await;
        let (before, _) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        let ledger_before = fixture
            .runtime
            .coordinator_ledger()
            .expect("settled ledger");

        // Detach: nothing semantic changes.
        attachment.detach();
        let (after_detach, _) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        assert_eq!(
            after_detach.messages, before.messages,
            "detach changes nothing"
        );
        assert_eq!(
            fixture
                .runtime
                .coordinator_ledger()
                .expect("settled ledger"),
            ledger_before,
            "detach never mutates canonical conversation state"
        );

        // A purely asynchronous enqueue with no attachment admits exactly
        // one further attempt through the runtime wake gate.
        // Armed before the trigger: settlement wakes only registered waiters.
        let async_settled = fixture.runtime.settlement_signal().notified();
        tokio::pin!(async_settled);
        async_settled.as_mut().enable();
        fixture
            .runtime
            .tool_runtime()
            .mailbox()
            .enqueue(inbound_text(
                "conv_9ec557e9-8373-72d1-92d3-cf39a531c6cf",
                "async after detach",
            ))
            .expect("async enqueue");
        await_request_history_len(&fixture.host, 2).await;
        async_settled.await;
        assert_eq!(
            adapter.requests().len(),
            2,
            "the detached conversation admitted the next attempt"
        );
    }

    /// There is exactly one authoritative mutable conversation-state owner
    /// at a time, and ownership transfers at the attempt boundaries.
    ///
    /// Since Issue #54 the ownership is *structural*: admission moves the
    /// one `ConversationState` out of the coordinator, so while an attempt
    /// runs the coordinator holds nothing at all and physically cannot
    /// mutate a competing copy.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn canonical_history_has_one_owner_at_a_time() {
        // Turn 1 calls a tool that parks. The loop therefore commits the
        // Assistant message (the model stream ended) and then blocks in tool
        // execution: exactly the "running, history already grown" window.
        let (tool, mut tool_started, release) = ParkingBackgroundTool::new();
        let definition = ToolDefinition {
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            ..tool.definition.clone()
        };
        let mut tools = ToolRegistry::new();
        tools
            .register(definition.clone(), Arc::new(tool))
            .expect("register the parking tool");
        let call_id = ToolCallId::new("call-park");
        let script = vec![
            GatedStep::Emit(ModelEvent::Started),
            GatedStep::Emit(ModelEvent::ToolCallStarted {
                block_index: ContentBlockIndex::new(0),
                call: crate::tools::types::ToolCallStart {
                    id: call_id.clone(),
                    tool_id: definition.id.clone(),
                    name: definition.name.clone(),
                },
            }),
            GatedStep::Emit(ModelEvent::ToolCallArgumentsDelta {
                block_index: ContentBlockIndex::new(0),
                call_id: call_id.clone(),
                arguments_delta: "{}".to_owned(),
            }),
            GatedStep::Emit(ModelEvent::ToolCallCompleted {
                block_index: ContentBlockIndex::new(0),
                call: crate::tools::types::ToolCall {
                    id: call_id,
                    tool_id: definition.id.clone(),
                    name: definition.name.clone(),
                    arguments: serde_json::json!({}),
                },
            }),
            GatedStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::ToolCalls,
                usage: None,
            }),
        ];
        let (adapter, fixture) = host_fixture(
            vec![script, one_turn_stop(), one_turn_stop()],
            tools,
            status_engine(),
        )
        .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");

        // Idle: the runtime owns the conversation state, and it is the
        // projection's only source.
        assert_eq!(
            fixture.host.host_ledger(),
            Some(Vec::new()),
            "an idle runtime owns an empty conversation state"
        );
        assert!(
            fixture
                .host
                .checkpoint()
                .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
                .expect("snapshot")
                .0
                .messages
                .is_empty()
        );

        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("first"),
        });

        // Running: the loop committed the Assistant message (observable on the
        // stream) and is now parked inside tool execution, so the attempt
        // provably has not settled.
        receive_until(&subscription, |event| {
            event.event.facts().iter().any(|fact| {
                matches!(fact, RuntimeClientEvent::MessageCommitted { message, .. }
                    if matches!(message, MessageBlock::Assistant(_)))
            })
        })
        .await;
        await_background_started(&mut tool_started, "the parking tool started").await;
        assert!(
            fixture.host.host_ledger().is_none(),
            "the attempt owns the conversation state; the runtime holds nothing"
        );
        assert!(
            fixture.host.host_active_ids().is_none(),
            "there is no runtime-side surface to compete with the attempt's"
        );
        let (mirror, _) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        assert_eq!(
            mirror.messages.len(),
            3,
            "the projection mirrors the attempt's committed history"
        );
        assert!(fixture.host.has_current_attempt());

        // An inbound message arriving now stays mailbox-owned: the runtime
        // does not append it to a competing history.
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(2),
            content: submit_content("second"),
        });
        assert!(
            fixture.host.host_ledger().is_none(),
            "a busy-path submission never gives the runtime a competing conversation state"
        );

        // Releasing the tool lets the attempt finish its tool turn and then
        // drain the mailbox at its safe boundary. The drained message joins
        // the *execution's* history — the loop commits it — and the attempt
        // continues rather than settling.
        release.send_replace(true);
        let settlement = receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        })
        .await;
        let committed_during_attempt: Vec<&MessageBlock> = settlement
            .iter()
            .flat_map(|event| event.event.facts())
            .filter_map(|fact| match fact {
                RuntimeClientEvent::MessageCommitted { message, .. } => Some(message),
                _ => None,
            })
            .collect();
        assert!(
            committed_during_attempt
                .iter()
                .any(|message| matches!(message, MessageBlock::Tool(_))),
            "the loop committed the tool message"
        );
        assert!(
            committed_during_attempt
                .iter()
                .any(|message| matches!(message, MessageBlock::User(_))),
            "the safe-boundary drain committed the queued inbound message into the attempt"
        );

        // A third submission while idle: its admission is the deterministic
        // proof that settlement transferred the execution's final
        // conversation state back to the runtime — admission only happens
        // once `finish_attempt` restored the state and cleared the attempt
        // slot.
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(3),
            content: submit_content("third"),
        });
        receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        })
        .await;

        // The next attempt's first request began from exactly the previous
        // attempt's committed history.
        let requests = adapter.requests();
        assert_eq!(
            requests.len(),
            3,
            "two turns in the first attempt, one in the second"
        );
        let previous_committed = requests[1].messages.len();
        assert!(
            requests[2].messages.len() > previous_committed,
            "the next attempt started from the previous committed history \
             ({} vs {previous_committed} messages)",
            requests[2].messages.len()
        );

        // The externally visible history is one coherent sequence across
        // the tool turn, the safe-boundary drain, and both attempts.
        let (final_snapshot, _) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        let roles: Vec<&str> = final_snapshot
            .messages
            .iter()
            .map(|message| match message {
                MessageBlock::User(_) => "user",
                MessageBlock::Assistant(_) => "assistant",
                MessageBlock::Tool(_) => "tool",
            })
            .collect();
        assert_eq!(
            roles,
            vec![
                "user",
                "user",
                "assistant",
                "tool",
                "user",
                "assistant",
                "user",
                "assistant",
            ],
            "one authoritative history, with only eligible canonical Runtime \
             context facts, extended across the tool turn, the safe-boundary \
             drain, and both attempts"
        );
    }

    /// Blocks off the runtime until the worker-exit signal arrives.
    async fn await_worker_exit(receiver: std::sync::mpsc::Receiver<()>) {
        tokio::task::spawn_blocking(move || {
            receiver
                .recv_timeout(std::time::Duration::from_secs(30))
                .expect("the observation worker must terminate after the host is released");
        })
        .await
        .expect("worker exit task");
    }

    /// Writes one discoverable Skill package, making the next capability
    /// candidate a real (non-no-op) commit.
    fn write_probe_skill(workspace: &std::path::Path, name: &str) {
        let skill = workspace.join(".agents").join("skills").join(name);
        std::fs::create_dir_all(&skill).expect("skill dir");
        std::fs::write(
            skill.join("SKILL.md"),
            format!("---\nname: {name}\ndescription: \"a probe skill\"\n---\nbody\n"),
        )
        .expect("SKILL.md");
    }

    /// Uses the public bounded checkpoint path and retains its exact cursor.
    fn checkpoint_state(
        host: &RuntimeClientHost,
    ) -> (
        super::super::snapshot::RuntimeClientSnapshot,
        RuntimeClientCursor,
    ) {
        let checkpoint = host.checkpoint().expect("bounded authoritative checkpoint");
        (checkpoint.snapshot, checkpoint.cursor)
    }

    /// Reads the projection without draining the pending queue, so a test
    /// can look at the state a consumer would see at an exact cut.
    fn host_projection_snapshot(
        inner: &Arc<super::ClientInner>,
    ) -> (
        crate::runtime_client::snapshot::RuntimeClientSnapshot,
        RuntimeClientCursor,
    ) {
        inner
            .state
            .lock()
            .expect("runtime client host lock poisoned")
            .projection
            .snapshot()
            .expect("projection is live")
    }

    /// A surviving authoritative subsystem handle neither retains nor
    /// resurrects the runtime: its observer no-ops, and its own transitions
    /// still succeed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_surviving_subsystem_handle_never_retains_the_host() {
        let (_adapter, fixture) =
            host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let HostFixture {
            directory: dir,
            host,
            runtime,
            coordinator,
            store: _,
        } = fixture;

        // Clone subsystem handles out of the runtime, exactly as an embedder
        // legitimately may.
        let mailbox = runtime.tool_runtime().mailbox();
        let registry = runtime.tool_runtime().background().clone();
        let weak = host.weak_inner();
        let weak_runtime = host.weak_runtime_inner();
        let (exit_tx, exit_rx) = std::sync::mpsc::channel();
        host.install_worker_exit_probe(exit_tx);

        drop(host);
        drop(runtime);
        await_worker_exit(exit_rx).await;
        assert!(weak.upgrade().is_none(), "the host is gone");
        assert!(
            weak_runtime.upgrade().is_none(),
            "the conversation runtime is gone"
        );

        // Authoritative mailbox transition: the observer's upgrade fails and
        // the seam no-ops, but the mailbox is unaffected.
        let sequence = mailbox
            .enqueue(inbound_text("msg-after", "still authoritative"))
            .expect("the mailbox remains authoritative without a runtime");
        assert_eq!(sequence.get(), 1);
        let batch = mailbox
            .select_pending_batch()
            .expect("select")
            .expect("the select still works");
        assert_eq!(batch.items().len(), 1);

        // Authoritative background transition: same.
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = registry
            .prepare_dispatch(
                &ToolInvocation {
                    id: crate::tools::types::ToolInvocationId::Agent {
                        call_id: ToolCallId::new("call-after"),
                    },
                    tool_id: ToolId::new("tool-bg"),
                    tool_name: "bg".to_owned(),
                    mode: ToolInvocationMode::Background,
                    arguments: serde_json::json!({}),
                },
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = registry
            .commit_dispatch(prepared, &CancellationSignal::new())
            .expect("dispatch commits")
        else {
            panic!("accepted dispatch");
        };
        await_background_started(&mut started, "background runner started").await;
        release.send_replace(true);
        await_background_terminal(
            &registry,
            &execution_id,
            "surviving registry background execution",
        )
        .await;

        // A claimed coordinator remains runtime-owned even after the runtime
        // and host are dropped; a surviving clone cannot bypass the resource
        // publication owner.
        write_probe_skill(&dir.path().join("workspace"), "after-skill");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        assert_eq!(
            coordinator.commit(candidate),
            Err(crate::capabilities::CapabilityCommitError::RuntimePublicationRequired),
            "a claimed coordinator cannot publish outside its runtime owner"
        );

        // None of those transitions resurrected the runtime or the host.
        assert_eq!(weak.strong_count(), 0);
        assert_eq!(weak_runtime.strong_count(), 0);
        assert!(
            weak.upgrade().is_none() && weak_runtime.upgrade().is_none(),
            "an observation seam can never resurrect a destroyed runtime"
        );
    }

    /// Attachment detach is not host destruction: the host survives, the
    /// attachment slot is released, and a fresh endpoint initializes.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn detach_releases_the_attachment_but_never_the_host() {
        let (_adapter, fixture) =
            host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let weak = fixture.host.weak_inner();

        let endpoint = fixture.host.endpoint();
        let response = endpoint.handle_request(RuntimeClientRequest::Initialize {
            id: crate::runtime_client::RequestId::new(1),
            protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
        });
        assert!(response.error.is_none());

        // Dropping the endpoint detaches only that attachment.
        drop(endpoint);
        assert!(
            weak.upgrade().is_some(),
            "detach is not host destruction while a semantic owner remains"
        );

        // The host is still usable and the slot is free.
        fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("the host still serves reads");
        let reconnected = fixture.host.endpoint();
        let response = reconnected.handle_request(RuntimeClientRequest::Initialize {
            id: crate::runtime_client::RequestId::new(1),
            protocol_version: crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION,
        });
        assert!(
            matches!(
                response.result,
                Some(RuntimeClientResult::Initialized { .. })
            ),
            "reconnect remains possible while the host is owned"
        );

        // Only releasing the host itself ends its lifetime.
        drop(reconnected);
        drop(fixture);
        assert!(weak.upgrade().is_none(), "the host ends with its owner");
    }

    /// A pure checkpoint capture and the native cancellation callback have
    /// one lock order: capture completes, then the finite callback installs.
    /// Neither read-side preparation nor Registry re-entry is required.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_background_transition_and_checkpoint_capture_have_one_lock_order() {
        let probe = Arc::new(crate::runtime_client::test_sync::ProjectionProbe::default());
        let (_, fixture) = host_fixture_probe(probe.clone(), Vec::new()).await;
        await_fixture_activation(&fixture.host).await;
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = fixture
            .runtime
            .tool_runtime()
            .background()
            .prepare_dispatch(
                &ToolInvocation {
                    id: crate::tools::types::ToolInvocationId::Agent {
                        call_id: ToolCallId::new("call-bg"),
                    },
                    tool_id: ToolId::new("tool-bg"),
                    tool_name: "bg".to_owned(),
                    mode: ToolInvocationMode::Background,
                    arguments: serde_json::json!({}),
                },
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = fixture
            .runtime
            .tool_runtime()
            .background()
            .commit_dispatch(prepared, &CancellationSignal::new())
            .expect("dispatch commits")
        else {
            panic!("accepted dispatch");
        };
        await_background_started(&mut started, "background runner started").await;

        // T1 takes the host lock and parks inside it.
        probe.arm_snapshot();
        let parked_host = fixture.host.clone();
        let snapshot_task = tokio::task::spawn_blocking(move || {
            parked_host
                .checkpoint()
                .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        });
        probe.wait_snapshot_entered();

        // T2 enters actual cancellation concurrently with the parked
        // capture. Its callback may wait for the finite raw capture.
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let registry = fixture.runtime.tool_runtime().background().clone();
        let cancel_id = execution_id.clone();
        let transition = tokio::task::spawn_blocking(move || {
            entered_tx.send(()).expect("the test still listens");
            registry.cancel(&cancel_id).expect("known execution")
        });

        entered_rx
            .recv_timeout(BACKGROUND_LIVENESS_GUARD)
            .expect("actual cancellation entered while capture was parked");
        probe.release_snapshot();
        snapshot_task
            .await
            .expect("snapshot task")
            .expect("snapshot");
        let cancelled = transition.await.expect("transition task");
        assert!(matches!(cancelled.state, BackgroundLifecycle::Cancelling));

        // The synchronous native callback was installed before cancellation
        // returned. The executor now releases and commits its terminal fact.
        release.send_replace(true);
        await_background_terminal(
            fixture.runtime.tool_runtime().background(),
            &execution_id,
            "snapshot-fold background execution",
        )
        .await;
        // The terminal notice has its own root attempt; settle that finite
        // transition before asking for the final authoritative cut.
        fixture.runtime.child_attempt_settlement(0).await;
        let (snapshot, _) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        assert_eq!(snapshot.jobs.len(), 1);
        assert_eq!(snapshot.jobs[0].job_id, execution_id);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn trace_snapshot_cut_excludes_commits_after_cursor_capture() {
        let (_, fixture) =
            host_fixture_with_native_tools(Vec::new(), ToolRegistry::new(), status_engine(), true)
                .await;
        let inner = fixture.host.weak_inner().upgrade().unwrap();
        inner.park_projection_worker();
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let (baseline, cursor) = checkpoint_state(&fixture.host);
        let subscription = attachment.subscribe_events(cursor).unwrap();
        let (entered_tx, entered_rx) = std::sync::mpsc::channel();
        let (release_tx, release_rx) = std::sync::mpsc::channel();
        *inner.trace_cut_hook.lock().unwrap() = Some(Box::new(move || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        }));
        let host = fixture.host.clone();
        let task = tokio::task::spawn_blocking(move || {
            host.checkpoint()
                .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
                .unwrap()
        });
        entered_rx.recv().unwrap(); // Cursor and Journal prefix captured; host lock released.
        let committed = inner
            .store
            .append_event(crate::events::RuntimeEventEnvelope {
                tool_occurrence: None,
                observation: None,
                schema_version: 1,
                event_id: crate::runtime::identity::EventId::new("after-trace-cut"),
                sequence: 0,
                conversation_id: inner.conversation_id.clone(),
                attempt_id: Some(crate::runtime::identity::AttemptId::new("after-cut")),
                turn_id: None,
                timestamp: chrono::Utc::now(),
                event: crate::events::RuntimeEvent::AttemptStarted {
                    attempt_id: crate::runtime::identity::AttemptId::new("after-cut"),
                },
            })
            .unwrap();
        assert!(inner.pending.has_unpublished());
        assert_eq!(host_projection_snapshot(&inner).1, cursor);
        release_tx.send(()).unwrap();
        let (before, returned) = task.await.unwrap();
        assert_eq!(returned, cursor);
        assert_eq!(before.trace, baseline.trace);
        assert!(matches!(subscription.try_next(), EventDelivery::Pending));
        // Historical reads have their own frontier, but never fold the queue or move C.
        let _ = inner.trace_page(None, 32, Vec::new()).unwrap();
        assert_eq!(host_projection_snapshot(&inner).1, cursor);
        let sink = super::NativePublicationProjection {
            inner: Arc::downgrade(&inner),
        };
        assert!(
            crate::runtime::observation::PublicationProjectionCommit::native_batch(
                &sink,
                committed.observation.as_deref().unwrap(),
            )
            .is_installed()
        );
        inner
            .store
            .mark_observation_direct(committed.sequence, committed.sequence)
            .unwrap();
        let (continuous, after_cursor) = checkpoint_state(&fixture.host);
        assert!(after_cursor > cursor);
        let id = format!("trace:{}", committed.sequence);
        // Default checkpoints preserve their published bounded read cache.
        // The read-only native Trace page discovers the newly represented J.
        let RuntimeClientResult::TracePage { page } =
            inner.trace_page(None, 32, Vec::new()).unwrap()
        else {
            panic!("native Trace page");
        };
        assert_eq!(
            page.records.iter().filter(|entry| entry.id == id).count(),
            1
        );
        assert_eq!(
            host_projection_snapshot(&inner).1,
            after_cursor,
            "discovering historical records does not publish a new display cursor"
        );
        assert!(matches!(subscription.try_next(), EventDelivery::Event(_)));
        drop(attachment);
        let (_, reconnect) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let RuntimeClientResult::Initialized { .. } = reconnect else {
            panic!("snapshot")
        };
        let (reconnect, cursor) = checkpoint_state(&fixture.host);
        assert_eq!(reconnect.trace, continuous.trace);
        assert!(cursor >= after_cursor);
        assert_read_domain_replay(&inner, after_cursor, cursor);
        inner.unpark_projection_worker();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)] // one two-cut native publication and terminal settlement contract
    async fn background_durable_commit_cannot_publish_a_half_semantic_snapshot() {
        struct ReleaseOnDrop(Arc<crate::tools::background::test_sync::CommitBoundaryHook>);
        impl Drop for ReleaseOnDrop {
            fn drop(&mut self) {
                self.0.proceed();
            }
        }
        let (_, fixture) =
            host_fixture_with_native_tools(Vec::new(), ToolRegistry::new(), status_engine(), true)
                .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .unwrap();
        let registry = fixture.runtime.tool_runtime().background().clone();
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = registry
            .prepare_dispatch(
                &claim_background_invocation("cut-background"),
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .unwrap();
        // Preparation independently publishes Busy. Include that semantic fact
        // in the baseline before parking the next Job publication; otherwise it
        // can advance the cursor while this test expects an unchanged cut.
        receive_until(&subscription, |event| {
            event.event.facts().iter().any(|fact| {
                matches!(fact, RuntimeClientEvent::ConfigurationAdoptionEligibilityChanged {
                    eligibility:
                        crate::local_runtime::configuration::application::AdoptionEligibility::Busy
                })
            })
        })
        .await;
        let hook = Arc::new(crate::tools::background::test_sync::CommitBoundaryHook::default());
        let _release_commit = ReleaseOnDrop(hook.clone());
        registry.install_publication_hook(hook.clone());
        let (baseline, cursor) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .unwrap();
        let commit_registry = registry.clone();
        let commit = tokio::task::spawn_blocking(move || {
            commit_registry
                .commit_dispatch(prepared, &CancellationSignal::new())
                .unwrap()
        });
        // Condvar waits belong to the blocking pool, including during failure
        // teardown. The guards release parked producers if an assertion panics.
        tokio::task::spawn_blocking({
            let hook = hook.clone();
            move || hook.wait_entered()
        })
        .await
        .unwrap();
        // SQLite COMMIT complete; native registry installation deliberately paused.
        let (during, during_cursor) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .unwrap();
        assert_eq!(during_cursor, cursor);
        assert_eq!(during.jobs, baseline.jobs);
        assert_eq!(during.trace, baseline.trace);
        hook.proceed();
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = commit.await.unwrap() else {
            panic!("accepted")
        };
        await_background_started(&mut started, "published background").await;
        receive_until(&subscription, |event| event.event.facts().iter().any(|fact| {
            matches!(fact, RuntimeClientEvent::ReadDomainsUpdated { trace, .. }
                if trace.records.iter().any(|record| record.native_id.as_deref() == Some(execution_id.as_str())))
        })).await;
        fixture.host.projection_settled().await;
        let after_checkpoint = fixture.host.checkpoint().unwrap();
        let after_cursor = after_checkpoint.cursor;
        let after_cut = after_checkpoint.read_cut;
        let after = after_checkpoint.snapshot;
        assert!(after_cursor > cursor);
        assert!(
            after
                .jobs
                .iter()
                .any(|record| record.job_id == execution_id)
        );
        assert!(
            after
                .trace
                .records
                .iter()
                .any(|record| record.native_id.as_deref() == Some(execution_id.as_str()))
        );
        let position = after
            .trace
            .records
            .iter()
            .find(|record| record.native_id.as_deref() == Some(execution_id.as_str()))
            .unwrap()
            .position
            .clone();
        // Isolate native terminal publication from the separate root-attempt
        // admission. Adopting the terminal notice removes Pending Inbox rows
        // and invalidates the historical cut this test deliberately holds.
        let admission_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        fixture
            .runtime
            .install_admission_gate(admission_gate.clone());
        let _release_admission = admission_gate.arm_scoped();
        let terminal_hook =
            Arc::new(crate::tools::background::test_sync::CommitBoundaryHook::default());
        let _release_terminal = ReleaseOnDrop(terminal_hook.clone());
        registry.install_publication_hook(terminal_hook.clone());
        release.send(true).unwrap();
        tokio::task::spawn_blocking({
            let hook = terminal_hook.clone();
            move || hook.wait_entered()
        })
        .await
        .unwrap();
        // Terminal COMMIT complete; native lifecycle still running.
        let inner = fixture.host.weak_inner().upgrade().unwrap();
        let during_checkpoint = inner
            .checkpoint_with_trace(std::slice::from_ref(&position))
            .unwrap_or_else(|error| {
                panic!(
                    "terminal publication checkpoint: {error:?}; prior={after_cut:?}; durable={:?}; published_cursor={:?}",
                    fixture.store.as_ref().unwrap().observation_read_cut(),
                    host_projection_snapshot(&inner).1,
                )
            });
        let terminal_cursor = during_checkpoint.cursor;
        assert_eq!(during_checkpoint.read_cut, after_cut);
        let during_terminal = during_checkpoint.snapshot;
        assert_eq!(terminal_cursor, after_cursor);
        assert_eq!(during_terminal.trace_updates.len(), 1);
        assert_eq!(
            during_terminal.trace_updates[0].state,
            crate::runtime_client::trace::TraceState::Running
        );
        assert_eq!(during_terminal.trace_updates[0].timing.duration_ms, None);
        let RuntimeClientResult::TracePage { page: historical } =
            inner.trace_page(None, 32, Vec::new()).unwrap()
        else {
            panic!("Trace page")
        };
        assert_eq!(
            historical
                .records
                .iter()
                .find(|entry| entry.native_id.as_deref() == Some(execution_id.as_str()))
                .unwrap()
                .state,
            crate::runtime_client::trace::TraceState::Running
        );
        assert_eq!(host_projection_snapshot(&inner).1, terminal_cursor);
        terminal_hook.proceed();
        await_background_terminal(&registry, &execution_id, "settled background").await;
        fixture.host.projection_settled().await;
        let settled_checkpoint = inner.checkpoint_with_trace(&[position]).unwrap();
        let settled_cursor = settled_checkpoint.cursor;
        let settled = settled_checkpoint.snapshot;
        assert!(settled_cursor > terminal_cursor);
        assert_eq!(
            settled.trace_updates[0].id,
            during_terminal.trace_updates[0].id
        );
        assert_eq!(
            settled.trace_updates[0].state,
            crate::runtime_client::trace::TraceState::Completed
        );
    }

    fn assert_source_failure_preserves_retained_cut(
        host: &RuntimeClientHost,
        attachment: &crate::runtime_client::RuntimeAttachment,
        subscriber: &EventSubscription,
        retained: &crate::runtime_client::checkpoint::RuntimeClientCheckpoint,
    ) -> crate::runtime_client::projection::ReadDomainCut {
        let inner = &host.inner;
        let (cut, cursor) = {
            let state = inner.lock_state();
            assert!(matches!(
                state.projection.snapshot_ref_checked(),
                Err(RuntimeClientError::ObservationUnavailable { .. })
            ));
            assert!(
                state.projection.read_domain_cut().is_none(),
                "no automatic error spin"
            );
            (
                state.projection.read_domain_fence(),
                state.projection.cursor(),
            )
        };
        assert_eq!(cursor, retained.cursor);
        // Current checkpoint reads reject without clearing the failure;
        // the retained historical cut remains independently readable.
        assert!(matches!(
            host.checkpoint(),
            Err(RuntimeClientError::ObservationUnavailable { .. })
        ));
        assert!(matches!(
            attachment.subscribe_events(cursor),
            Err(RuntimeClientError::ObservationUnavailable { .. })
        ));
        assert!(matches!(
            subscriber.try_next(),
            EventDelivery::Unavailable { .. }
        ));
        let old_page = inner
            .checkpoint_page(&retained.read_cut, "capabilities.available_tools", 0, 1)
            .unwrap();
        assert_eq!(old_page.read_cut, retained.read_cut);
        assert_eq!(inner.lock_state().projection.cursor(), cursor);
        let failed = {
            let state = inner.state.lock().unwrap();
            state.projection.snapshot_ref_checked().is_err()
        };
        assert!(failed);
        cut
    }

    fn assert_source_repair_resumes_normal_events(
        inner: &super::ClientInner,
        recovered: &EventSubscription,
        cursor: RuntimeClientCursor,
    ) {
        super::commit_display(&inner.state, &inner.publication_gate, |projection| {
            projection.apply(ConversationObservation::Event {
                attempt_id: AttemptId::new("failed-cut"),
                event: RuntimeEvent::AttemptStarted {
                    attempt_id: AttemptId::new("failed-cut"),
                },
            });
        });
        let EventDelivery::Event(started) = recovered.try_next() else {
            panic!("normal start delivery resumes after genuine source repair");
        };
        assert_eq!(started.cursor.get(), cursor.get() + 1);
        let cursor = started.cursor;
        super::commit_display(&inner.state, &inner.publication_gate, |projection| {
            projection.apply(ConversationObservation::Event {
                attempt_id: AttemptId::new("failed-cut"),
                event: RuntimeEvent::TurnStarted,
            });
        });
        let EventDelivery::Event(event) = recovered.try_next() else {
            panic!("normal delivery resumes without another snapshot");
        };
        assert_eq!(event.cursor.get(), cursor.get() + 1);
        assert!(
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptTurnUpdated { turn: 1, .. }))
        );
    }

    #[tokio::test]
    async fn panicked_projection_source_installs_same_cut_unavailability_without_spinning() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let inner = fixture.host.weak_inner().upgrade().unwrap();
        inner.projection_settled().await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let (cursor, cut) = {
            let state = inner.lock_state();
            (
                state.projection.cursor(),
                state.projection.read_domain_fence(),
            )
        };
        let subscriber = attachment.subscribe_events(cursor).unwrap();
        *inner.read_domain_hook.lock().unwrap() = Some(Box::new(|| {
            panic!("private Source panic payload must not enter display diagnostics");
        }));
        inner
            .lock_state()
            .projection
            .request_read_domains_for_test();
        inner
            .pending
            .push(ConversationObservation::JournalSourceReady);
        let EventDelivery::Unavailable { message } = subscriber.next().await else {
            panic!("a Source panic must resolve subscribers as unavailable");
        };
        assert!(message.contains("native projection Source task panicked"));
        assert!(!message.contains("private Source panic payload"));
        let mut turns = inner.worker_turns.subscribe();
        turns.borrow_and_update();
        inner
            .pending
            .push(ConversationObservation::JournalSourceReady);
        turns.changed().await.unwrap();
        {
            let state = inner.lock_state();
            assert_eq!(state.projection.cursor(), cursor);
            assert_eq!(state.projection.read_domain_fence(), cut);
            assert!(state.projection.read_domain_cut().is_none());
            assert!(matches!(
                state.projection.validate_published_source(),
                Err(RuntimeClientError::ObservationUnavailable { .. })
            ));
        }
        assert!(inner.projection_read.lock().await.is_none());
        fixture.runtime.shutdown().await.unwrap();
        inner.drain_projection().await.unwrap();
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn source_failure_is_unavailable_until_same_cut_native_read_repair() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let inner = fixture.host.weak_inner().upgrade().unwrap();
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let cursor = inner.lock_state().projection.cursor();
        let subscriber = attachment.subscribe_events(cursor).unwrap();
        let retained = fixture.host.checkpoint().unwrap();
        let mut source_fault = source_recovery_tests::OccupancySourceFault::new(&fixture);
        *inner.read_domain_hook.lock().unwrap() = Some(Box::new(|| {
            Err(crate::durable::ConversationStoreError::InvalidReference(
                "one read fault".into(),
            ))
        }));
        let journal = inner.store.presentation_frontier().unwrap();
        inner
            .lock_state()
            .projection
            .request_read_domains_for_test();
        inner
            .pending
            .push(ConversationObservation::JournalSourceReady);
        let EventDelivery::Unavailable { message } = subscriber.next().await else {
            panic!("Source failure reports unavailability without an ordinary event");
        };
        assert!(message.contains("one read fault"));
        let cut = assert_source_failure_preserves_retained_cut(
            &fixture.host,
            &attachment,
            &subscriber,
            &retained,
        );
        assert_eq!(inner.store.presentation_frontier().unwrap(), journal);
        // The source worker's existing owner reads outside the raw lock and
        // installs one successful result at the exact failed fence.
        source_fault.repair();
        let read = cut.read(inner.store.as_ref());
        super::commit_display(&inner.state, &inner.publication_gate, |projection| {
            assert!(projection.install_read_domains(&cut, read, true));
        });
        let repaired = fixture.host.checkpoint().unwrap();
        let recovered_cursor = repaired.cursor;
        assert_eq!(recovered_cursor.get(), cursor.get() + 1);
        assert_eq!(repaired.read_cut.source_cut, retained.read_cut.source_cut);
        assert_eq!(inner.store.presentation_frontier().unwrap(), journal);
        let (same_fence, valid, no_spin) = {
            let state = inner.state.lock().unwrap();
            (
                state.projection.read_domain_fence() == cut,
                state.projection.snapshot_ref_checked().is_ok(),
                state.projection.read_domain_cut().is_none(),
            )
        };
        assert!(same_fence && valid && no_spin);
        assert_eq!(fixture.host.checkpoint().unwrap(), repaired);
        let EventDelivery::Event(repair) = subscriber.try_next() else {
            panic!("genuine Source repair publishes one complete transition");
        };
        assert_eq!(repair.cursor, recovered_cursor);
        assert!(
            repair
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::ReadDomainsUpdated { .. }))
        );
        let recovered = attachment.subscribe_events(recovered_cursor).unwrap();
        assert!(matches!(subscriber.try_next(), EventDelivery::Closed));
        assert!(matches!(recovered.try_next(), EventDelivery::Pending));
        assert_source_repair_resumes_normal_events(&inner, &recovered, recovered_cursor);
    }

    #[tokio::test]
    async fn historical_candidate_cannot_repair_newer_dirty_or_failed_cut() {
        for fail_newer in [false, true] {
            let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
            let inner = fixture.host.weak_inner().upgrade().unwrap();
            let before = inner.snapshot_candidate().unwrap();
            let state = inner.state.clone();
            let publication_gate = inner.publication_gate.clone();
            *inner.snapshot_read_hook.lock().unwrap() = Some(Box::new(move || {
                super::commit_display(&state, &publication_gate, |projection| {
                    projection.apply(ConversationObservation::Event {
                        attempt_id: AttemptId::new("newer-cut"),
                        event: RuntimeEvent::AttemptStarted {
                            attempt_id: AttemptId::new("newer-cut"),
                        },
                    });
                });
                if fail_newer {
                    super::commit_display(&state, &publication_gate, |projection| {
                        let cut = projection.read_domain_cut().unwrap();
                        assert!(projection.install_read_domains(
                            &cut,
                            Err(crate::durable::ConversationStoreError::InvalidReference(
                                "newer failure".into()
                            )),
                            true
                        ));
                    });
                }
            }));
            let candidate = inner.snapshot_candidate().unwrap();
            assert_eq!(candidate.cursor, before.cursor);
            assert_eq!(candidate.snapshot, before.snapshot);
            {
                let state = inner.lock_state();
                assert_eq!(
                    state.projection.cursor().get(),
                    before.cursor.get() + 1,
                    "no false derived publication"
                );
                assert_ne!(state.projection.read_domain_fence(), candidate.read_cut);
                assert_eq!(state.projection.read_domain_cut().is_some(), !fail_newer);
                assert_eq!(state.projection.snapshot_ref_checked().is_err(), fail_newer);
                assert!(state.projection.snapshot_ref().attempt.is_some());
            }
            *inner.snapshot_read_hook.lock().unwrap() = None;
            let current = inner.snapshot_candidate().unwrap();
            let state = inner.lock_state();
            assert!(state.projection.read_domain_cut().is_none());
            assert_eq!(
                state.projection.snapshot_ref_checked().unwrap().transcript,
                current.snapshot.transcript
            );
            assert!(current.snapshot.attempt.is_some());
        }
    }

    #[tokio::test]
    async fn snapshot_candidate_survives_moving_head_and_replays_exact_suffix() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let inner = fixture.host.weak_inner().upgrade().unwrap();
        super::commit_display(&inner.state, &inner.publication_gate, |projection| {
            projection.apply(ConversationObservation::Event {
                attempt_id: AttemptId::new("candidate"),
                event: RuntimeEvent::AttemptStarted {
                    attempt_id: AttemptId::new("candidate"),
                },
            });
        });
        let cursor = inner.lock_state().projection.cursor();
        let state = inner.state.clone();
        let publication_gate = inner.publication_gate.clone();
        *inner.snapshot_read_hook.lock().unwrap() = Some(Box::new(move || {
            for _ in 0..8 {
                super::commit_display(&state, &publication_gate, |projection| {
                    projection.apply(ConversationObservation::Event {
                        attempt_id: AttemptId::new("candidate"),
                        event: RuntimeEvent::TurnStarted,
                    });
                });
            }
        }));
        let candidate = inner.snapshot_candidate().unwrap();
        assert_eq!(candidate.cursor, cursor);
        assert_eq!(candidate.snapshot.attempt.as_ref().unwrap().turn, 0);
        let mut state = inner.lock_state();
        let (subscriber, _) = state.projection.subscribe(candidate.cursor).unwrap();
        for turn in 1..=8 {
            let super::super::projection::SubscriberPoll::Event(event) =
                state.projection.poll_subscriber(subscriber)
            else {
                panic!("exact replay suffix");
            };
            assert_eq!(event.cursor.get(), cursor.get() + u64::from(turn));
            assert!(
                event.event.facts().iter().any(|fact| matches!(fact, RuntimeClientEvent::AttemptTurnUpdated { turn: actual, .. } if *actual == turn))
            );
        }
        assert_eq!(
            state
                .projection
                .snapshot_ref()
                .attempt
                .as_ref()
                .unwrap()
                .turn,
            8
        );
    }

    fn fold_captured_snapshot_fact(
        snapshot: &mut super::super::snapshot::RuntimeClientSnapshot,
        event: &RuntimeClientEvent,
    ) {
        match event {
            RuntimeClientEvent::MessageDescriptorCommitted {
                head,
                evicted_message_id,
                ..
            } => {
                snapshot.message_descriptors.clone_from(head);
                if let Some(id) = evicted_message_id {
                    snapshot.messages.retain(|message| message.id() != id);
                }
            }
            RuntimeClientEvent::MessageCommitted {
                message,
                evicted_message_id,
                ..
            } => {
                if let Some(id) = evicted_message_id {
                    snapshot.messages.retain(|message| message.id() != id);
                }
                if let Some(current) = snapshot
                    .messages
                    .iter_mut()
                    .find(|current| current.id() == message.id())
                {
                    current.clone_from(message);
                } else {
                    snapshot.messages.push(message.clone());
                }
            }
            RuntimeClientEvent::ReadDomainsUpdated {
                projection_head,
                transcript,
                trace,
                occupancy,
                todos,
            } => {
                snapshot.context.projection_head.clone_from(projection_head);
                snapshot.transcript.clone_from(transcript);
                snapshot.trace.clone_from(trace);
                snapshot.context.occupancy.clone_from(occupancy);
                snapshot.todos.clone_from(todos);
            }
            RuntimeClientEvent::CheckpointAdvanced { checkpoint, .. } => {
                snapshot.clone_from(&checkpoint.snapshot);
            }
            _ => panic!("unexpected captured-cut fact"),
        }
    }

    #[tokio::test]
    async fn captured_snapshot_excludes_later_durable_messages_and_replay_converges() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let inner = fixture.host.weak_inner().upgrade().unwrap();
        let baseline = inner.snapshot_candidate().unwrap();
        let old_cut = inner.lock_state().projection.read_domain_fence();
        let store = inner.store.clone();
        let state = inner.state.clone();
        let publication = Arc::clone(&inner.publication_gate);
        *inner.snapshot_read_hook.lock().unwrap() = Some(Box::new(move || {
            for id in ["candidate-later-one", "candidate-later-two"] {
                let message = MessageBlock::User(inbound_text(id, id));
                let receipt = store.append_canonical(&message).unwrap();
                let position = receipt.ledger_position;
                super::commit_display(&state, &publication, |projection| {
                    projection.apply(ConversationObservation::CanonicalCommitted {
                        attempt_id: None,
                        receipt,
                    });
                });
                store
                    .mark_canonical_transcript_ready(position, position)
                    .unwrap();
            }
        }));
        let candidate = inner.snapshot_candidate().unwrap();
        assert_eq!(candidate.cursor, baseline.cursor);
        assert_eq!(
            candidate.snapshot, baseline.snapshot,
            "later durable bodies cannot leak backward"
        );
        inner.snapshot_read_hook.lock().unwrap().take();
        let mut replayed = candidate.snapshot.clone();
        let mut state = inner.lock_state();
        assert!(
            !state.projection.install_read_domains(
                &old_cut,
                Ok(super::super::projection::ReadDomainValues {
                    projection_head: candidate.snapshot.context.projection_head.clone(),
                    transcript: candidate.snapshot.transcript.clone(),
                    occupancy: candidate.snapshot.context.occupancy.clone(),
                    trace: candidate.snapshot.trace.clone(),
                    trace_total: 0,
                }),
                true
            ),
            "returnable candidate is stale for live installation"
        );
        drop(state);
        let current = inner.snapshot_candidate().unwrap();
        let mut state = inner.lock_published_state();
        let (subscriber, _) = state.projection.subscribe(candidate.cursor).unwrap();
        let mut through = candidate.cursor;
        while through < state.projection.cursor() {
            let super::super::projection::SubscriberPoll::Event(event) =
                state.projection.poll_subscriber(subscriber)
            else {
                panic!("contiguous replay");
            };
            assert_eq!(event.cursor.get(), through.get() + 1);
            through = event.cursor;
            for fact in event.event.facts() {
                fold_captured_snapshot_fact(&mut replayed, fact);
            }
        }
        assert!(matches!(
            state.projection.poll_subscriber(subscriber),
            super::super::projection::SubscriberPoll::Pending
        ));
        drop(state);
        assert_eq!(
            replayed, current.snapshot,
            "replay equals independent authoritative completion"
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn captured_pending_body_survives_later_edit_removal_and_commit() {
        let (release, gate) = model_release();
        let (adapter, fixture) = host_fixture(
            vec![vec![
                GatedStep::Emit(ModelEvent::Started),
                GatedStep::ParkUntilReleased(gate),
                GatedStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ]],
            ToolRegistry::new(),
            status_engine(),
        )
        .await;
        let mailbox = fixture.runtime.tool_runtime().mailbox();
        mailbox
            .enqueue(inbound_text("candidate-active", "running"))
            .unwrap();
        await_adapter_request_count(&adapter, 1).await;
        let sequence = mailbox
            .enqueue(inbound_text("candidate-pending", "captured body"))
            .unwrap();
        let inner = fixture.host.weak_inner().upgrade().unwrap();
        let store = inner.store.clone();
        let candidate = inner.snapshot_candidate().unwrap();
        let frozen = candidate.snapshot.clone();
        {
            let expected = crate::durable::inbox::PendingInboundRef {
                sequence,
                revision: 0,
            };
            store.edit_pending(&expected, "later edit").unwrap();
            store
                .remove_pending(&crate::durable::inbox::PendingInboundRef {
                    revision: 1,
                    ..expected
                })
                .unwrap();
            store
                .append_canonical(&MessageBlock::User(inbound_text("future-commit", "future")))
                .unwrap();
        }
        assert!(matches!(
            candidate.read_domains(store.as_ref()),
            Err(crate::durable::ConversationStoreError::ObservationCutExpired)
        ));
        assert_eq!(candidate.snapshot, frozen);
        assert_eq!(candidate.snapshot.inbound.pending[0].revision, 0);
        assert_eq!(
            candidate.snapshot.inbound.pending[0].message.content,
            inbound_text("x", "captured body").content
        );
        let transcript = format!("{:?}", candidate.snapshot.transcript);
        assert!(transcript.contains("captured body"));
        assert!(!transcript.contains("later edit"));
        assert!(!transcript.contains("future-commit"));
        release.send(true).unwrap();
        fixture.runtime.shutdown().await.unwrap();
    }

    #[tokio::test]
    async fn captured_snapshot_survives_replay_eviction_but_subscribe_requires_resync() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let inner = fixture.host.weak_inner().upgrade().unwrap();
        let state = inner.state.clone();
        *inner.snapshot_read_hook.lock().unwrap() = Some(Box::new(move || {
            let mut state = state.lock().unwrap();
            for _ in 0..=super::super::projection::RUNTIME_CLIENT_REPLAY_LIMIT_DEFAULT {
                state.projection.apply(ConversationObservation::Shutdown);
            }
        }));
        let candidate = inner.snapshot_candidate().unwrap();
        assert!(!candidate.snapshot.shutting_down);
        assert!(matches!(
            inner.lock_state().projection.subscribe(candidate.cursor),
            Err(RuntimeClientError::ResyncRequired { .. })
        ));
    }

    #[tokio::test]
    async fn blocked_read_materialization_does_not_own_shutdown_or_projection_drain() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let inner = fixture.host.weak_inner().upgrade().unwrap();
        let (entered, entered_rx) = tokio::sync::oneshot::channel();
        let reader = inner.read_store.clone();
        *inner.read_domain_hook.lock().unwrap() = Some(Box::new(move || {
            reader.park_presentation_read_until_cancelled(entered);
            Ok(())
        }));
        inner.pending.push(ConversationObservation::Event {
            attempt_id: AttemptId::new("read-cut"),
            event: RuntimeEvent::TurnStarted,
        });
        entered_rx.await.unwrap(); // The derived read is blocked, with one Tokio worker.
        inner.pending.push(ConversationObservation::Shutdown);
        assert!(inner.lock_state().projection.snapshot_ref().shutting_down);
        assert!(matches!(
            inner.shutdown().await.unwrap(),
            RuntimeClientResult::ShutdownCompleted
        ));
        inner.drain_projection().await.unwrap(); // Joins the cancelled read; no test release.
        assert!(inner.pending.is_closed());
    }

    #[tokio::test]
    async fn projection_drain_retry_retains_the_original_worker_after_waiter_cancellation() {
        use std::future::Future as _;
        struct ReleaseOnDrop(std::sync::mpsc::Sender<()>);
        impl Drop for ReleaseOnDrop {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let inner = fixture.host.weak_inner().upgrade().unwrap();
        let (entered, entered_rx) = tokio::sync::oneshot::channel();
        let (cancelled, cancelled_rx) = tokio::sync::oneshot::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let release_on_unwind = ReleaseOnDrop(release.clone());
        let reader = inner.read_store.clone();
        *inner.read_domain_hook.lock().unwrap() = Some(Box::new(move || {
            reader.park_presentation_read_until_cancelled(entered);
            cancelled.send(()).unwrap();
            // This is the actual blocking Source task, after cancellation;
            // it cannot release its original ownership until the gate opens.
            release_rx.recv().unwrap();
            Ok(())
        }));
        inner.pending.push(ConversationObservation::Event {
            attempt_id: AttemptId::new("cancelled-join-waiter"),
            event: RuntimeEvent::TurnStarted,
        });
        entered_rx.await.unwrap();
        fixture.runtime.shutdown().await.unwrap();
        let mut first = Box::pin(inner.drain_projection());
        tokio::select! {
            biased;
            result = &mut first => panic!("the original gated Source must still be owned: {result:?}"),
            result = cancelled_rx => result.unwrap(),
        }
        drop(first);
        assert!(matches!(
            *inner.worker.try_lock().unwrap(),
            ProjectionWorker::Running(_)
        ));
        let mut retry = Box::pin(inner.drain_projection());
        std::future::poll_fn(|context| {
            assert!(
                retry.as_mut().poll(context).is_pending(),
                "a retry cannot claim a detached original worker has joined"
            );
            std::task::Poll::Ready(())
        })
        .await;
        release.send(()).unwrap();
        retry.await.unwrap();
        assert!(matches!(
            *inner.worker.try_lock().unwrap(),
            ProjectionWorker::Vacant
        ));
        assert!(inner.pending.is_closed());
        drop(release_on_unwind);
    }

    async fn failed_projection_worker_keeps_its_physical_read(abort: bool) {
        use std::future::Future as _;
        struct ReleaseOnDrop(std::sync::mpsc::Sender<()>);
        impl Drop for ReleaseOnDrop {
            fn drop(&mut self) {
                let _ = self.0.send(());
            }
        }
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let inner = fixture.host.weak_inner().upgrade().unwrap();
        let capacity = Arc::new(tokio::sync::Semaphore::new(1));
        let permit = capacity.clone().acquire_owned().await.unwrap();
        let (entered, entered_rx) = tokio::sync::oneshot::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let release_on_unwind = ReleaseOnDrop(release.clone());
        *inner.read_domain_hook.lock().unwrap() = Some(Box::new(move || {
            let _permit = permit;
            entered.send(()).unwrap();
            release_rx.recv().unwrap();
            Ok(())
        }));
        inner.pending.push(ConversationObservation::Event {
            attempt_id: AttemptId::new("failed-worker-read"),
            event: RuntimeEvent::TurnStarted,
        });
        entered_rx.await.unwrap();
        if abort {
            let worker = inner.worker.lock().await;
            let ProjectionWorker::Running(task) = &*worker else {
                panic!("worker must be running")
            };
            task.abort();
        } else {
            // This is the real production panic boundary while a Source task
            // remains outstanding, rather than a replacement worker fixture.
            let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let _guard = inner.observation_preparation.lock().unwrap();
                panic!("poison preparation before the worker's next turn");
            }));
            inner
                .pending
                .push(ConversationObservation::JournalSourceReady);
        }
        loop {
            let finished = match &*inner.worker.lock().await {
                ProjectionWorker::Running(task) => task.is_finished(),
                _ => panic!("only drain may consume the worker proof"),
            };
            if finished {
                break;
            }
            tokio::task::yield_now().await;
        }
        inner.observation_preparation.clear_poison();
        fixture.runtime.shutdown().await.unwrap();
        let mut first = Box::pin(inner.drain_projection());
        std::future::poll_fn(|cx| {
            assert!(first.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        })
        .await;
        assert_eq!(capacity.available_permits(), 0);
        drop(first);
        assert!(matches!(
            *inner.worker.try_lock().unwrap(),
            ProjectionWorker::Joined(Err(_))
        ));
        assert!(inner.projection_read.try_lock().unwrap().is_some());
        let mut retry = Box::pin(inner.drain_projection());
        std::future::poll_fn(|cx| {
            assert!(
                retry.as_mut().poll(cx).is_pending(),
                "failed worker cannot prove its detached read ended"
            );
            std::task::Poll::Ready(())
        })
        .await;
        assert_eq!(capacity.available_permits(), 0);
        release.send(()).unwrap();
        let error = retry.await.unwrap_err();
        assert_eq!(error.is_cancelled(), abort);
        assert_eq!(error.is_panic(), !abort);
        assert_eq!(capacity.available_permits(), 1);
        assert!(inner.projection_read.try_lock().unwrap().is_none());
        inner.drain_projection().await.unwrap(); // Both physical tasks are now joined.
        drop(release_on_unwind);
    }

    #[tokio::test]
    async fn projection_drain_joins_residual_read_after_worker_panic_and_waiter_cancellation() {
        failed_projection_worker_keeps_its_physical_read(false).await;
    }

    #[tokio::test]
    async fn projection_drain_joins_residual_read_after_worker_abort_and_waiter_cancellation() {
        failed_projection_worker_keeps_its_physical_read(true).await;
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn a_parked_projection_fold_cannot_retain_the_host_or_storage_authority() {
        let probe = Arc::new(crate::runtime_client::test_sync::ProjectionProbe::default());
        let (_, fixture) = host_fixture_probe(probe.clone(), Vec::new()).await;
        let weak = fixture.host.weak_inner();
        let pending = weak.upgrade().unwrap().pending.clone();
        let (exited, exit) = std::sync::mpsc::channel();
        fixture.host.install_worker_exit_probe(exited);
        probe.arm_publish();
        pending.push(ConversationObservation::GoalChanged(
            crate::runtime_client::goal::RuntimeClientGoalView { current: None },
        ));
        probe.wait_publish_entered(); // Worker holds the projection mutex inside a fold.
        drop(fixture.host);
        let host_released = weak.upgrade().is_none();
        probe.release_publish();
        await_worker_exit(exit).await;
        assert!(
            host_released,
            "a read-model fold must not own the host's resource authority"
        );
    }

    /// The exact snapshot/cursor race, interleaving A (snapshot wins): the
    /// snapshot linearizes first and the concurrent transition is observed
    /// by a resume after the snapshot's cursor.
    ///
    /// The runtime admission gate makes the interleaving exact: the parked
    /// snapshot captures the immutable published seed before the submit, and the
    /// admission commit is released only after the snapshot returned.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn snapshot_cursor_race_snapshot_wins() {
        let probe = Arc::new(crate::runtime_client::test_sync::ProjectionProbe::default());
        let admission_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let submit_arrival = Arc::new(tokio::sync::Notify::new());
        let (_, fixture) = host_fixture_probe_with_runtime_gate(
            probe.clone(),
            vec![one_turn_stop()],
            CoordinatorProbe {
                admission_gate: Some(admission_gate.clone()),
                settlement_gate: None,
                before_restore_gate: None,
                activation_gate: None,
                manual_compaction_settlement_gate: None,
                submit_gate: None,
                submit_arrival: Some(submit_arrival.clone()),
                shutdown_arrival: None,
                mcp_failure_drain_gate: None,
                start_boundary_pause: None,
                model_arbitration_pause: None,
                drain_linearization: None,
                tool_start_pause: None,
                drain_supervision: None,
                attempt_exit_gate: None,
                parent_guidance_seal_gate: None,
                background_failure_gate: None,
                subagent_failure_published_gate: None,
                ..CoordinatorProbe::default()
            },
        )
        .await;
        await_fixture_activation(&fixture.host).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        admission_gate.arm();
        probe.arm_snapshot();
        let snapshot_probe = probe.clone();
        let host = fixture.host.clone();
        let snapshot_task = tokio::task::spawn_blocking(move || {
            host.checkpoint()
                .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        });
        snapshot_probe.wait_snapshot_entered();

        // The concurrent transition: a submit whose admission is gated
        // until after the snapshot returns.
        let submitting = fixture.host.clone();
        let submit_task = tokio::task::spawn_blocking(move || {
            submitting
                .submit_inbound(submit_content("racing"))
                .expect("accepted")
        });
        // The native submit entered while capture owns raw. Its finite
        // pending callback can finish only after that capture releases raw.
        submit_arrival.notified().await;
        snapshot_probe.release_snapshot();
        let (snapshot, cursor) = snapshot_task
            .await
            .expect("snapshot task")
            .expect("snapshot");
        assert!(snapshot.inbound.pending.is_empty());
        submit_task.await.expect("submit task");
        admission_gate.wait_entered();

        // Release the admission: the transition commits and is observed
        // after C. Resume and receive the admission events, never a gap.
        admission_gate.release();
        let subscription = attachment
            .subscribe_events(cursor)
            .expect("resume after the snapshot cursor");
        let events = receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::InboundEnqueued { .. }))
        })
        .await;
        assert!(
            events.iter().all(|event| event.cursor > cursor),
            "every resumed event is strictly after the snapshot cursor"
        );
        assert!(events.iter().any(|event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::InboundEnqueued { .. }))
        }));
        // Drain the attempt so the fixture settles cleanly.
        receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        })
        .await;
    }

    /// The exact snapshot/cursor race, interleaving B (publish wins): the
    /// concurrent transition linearizes before the snapshot, so the
    /// snapshot at its cursor already reflects it.
    ///
    /// The runtime admission gate and the projection publish gate make the
    /// interleaving exact: the admission commits while the publish of its
    /// observations is parked, and the snapshot acquires the projection
    /// lock only after the fold completed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn snapshot_cursor_race_publish_wins() {
        let (pause, control, _) =
            crate::agent::execution::test_sync::StartBoundaryPause::install(true, false);
        let mut control = control.unwrap();
        let probe = Arc::new(crate::runtime_client::test_sync::ProjectionProbe::default());
        let admission_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let (_, fixture) = host_fixture_probe_with_runtime_gate(
            probe.clone(),
            vec![one_turn_stop()],
            CoordinatorProbe {
                admission_gate: Some(admission_gate.clone()),
                start_boundary_pause: Some(pause),
                ..CoordinatorProbe::default()
            },
        )
        .await;
        await_fixture_activation(&fixture.host).await;
        // Baseline: an idle host at some cursor C.
        let (before, cursor) = checkpoint_state(&fixture.host);
        assert!(before.inbound.pending.is_empty());

        // Submit; the admission parks at the runtime gate before committing.
        admission_gate.arm();
        let submitting = fixture.host.clone();
        let submit_task = tokio::task::spawn_blocking(move || {
            submitting
                .submit_inbound(submit_content("racing"))
                .expect("accepted")
        });
        let _accepted = submit_task.await.expect("submit task");
        admission_gate.wait_entered();

        // Submit's exact pending receipt has already published. The gate
        // below therefore parks the later admission transition.
        let (folded, _) = checkpoint_state(&fixture.host);
        assert_eq!(folded.inbound.pending.len(), 1);

        // Release the admission while the projection publish gate is armed:
        // the fold of the commit observations parks at the gate.
        probe.arm_publish();
        admission_gate.release();
        probe.wait_publish_entered();
        // Admission publication completes before the Agent can reach its
        // request-start gate. Release the earlier owner before awaiting that
        // next durable boundary, then capture the completed admission cut.
        probe.release_publish();
        control.await_park(1).await;
        let snapshot_host = fixture.host.clone();
        let snapshot_task = tokio::task::spawn_blocking(move || {
            snapshot_host
                .checkpoint()
                .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
        });
        // The pure snapshot reads the admission cut already installed above.
        let (after_snapshot, after_cursor) = snapshot_task
            .await
            .expect("snapshot task")
            .expect("snapshot");
        assert!(after_cursor > cursor, "the transition advanced the cursor");
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(cursor)
            .expect("resume from the pre-transition cursor");
        control.release();
        let mut saw_inbound = false;
        loop {
            let delivery = tokio::time::timeout(STREAM_LIVENESS_GUARD, subscription.next())
                .await
                .expect("stream must not stall");
            let EventDelivery::Event(event) = delivery else {
                panic!("subscription stays open and contiguous, got {delivery:?}");
            };
            if event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::InboundEnqueued { .. }))
            {
                saw_inbound = true;
            }
            if event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
            {
                break;
            }
        }
        assert!(saw_inbound, "the transition event is on the stream");
        assert!(
            after_snapshot.messages.iter().any(|message| matches!(
                message,
                MessageBlock::User(user) if user.id.as_str() == "conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b-inbound-1"
            )),
            "the snapshot reflects the transition state"
        );
    }

    #[tokio::test]
    async fn resubscription_is_superseded_before_local_handle_publication() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let (_, cursor) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .unwrap();
        let old = attachment.subscribe_events(cursor).unwrap();
        // Split the native transaction from local handle publication, exactly
        // the interval in which a waiting notification consumer can wake. No
        // timer or scheduler race is needed to hold this boundary open.
        let (new, _) = fixture
            .host
            .subscribe_events(attachment.attachment_id(), cursor)
            .unwrap();
        assert!(matches!(old.try_next(), EventDelivery::Closed));
        assert!(attachment.subscription().unwrap().same_registration(&old));
        assert!(
            attachment.superseded(&old),
            "resync must not retire the live attachment"
        );
        assert!(!attachment.superseded(&new));
        attachment.store_subscription(new.clone());
        assert!(attachment.subscription().unwrap().same_registration(&new));
        assert!(attachment.superseded(&old));
        attachment.detach();
        assert!(
            !attachment.superseded(&new),
            "real detach is not replacement"
        );
        assert!(matches!(new.try_next(), EventDelivery::Closed));
    }

    /// The bounded replay/resync contract: a serviceable resume has no
    /// gap, an expired cursor returns `resync_required`, and a fresh
    /// snapshot repairs all state. The cursor survives detach (it belongs
    /// to the observation stream, not the attachment).
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn replay_resync_and_cursor_survival() {
        let (_, fixture) =
            host_fixture(vec![one_turn_stop()], ToolRegistry::new(), status_engine()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("go"),
        });
        let events = receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        })
        .await;
        let terminal_cursor = events.last().expect("terminal event").cursor;

        // Detach: the cursor is stream-owned and survives.
        attachment.detach();
        let (second, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("reattach");
        let second_subscription = second
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("resume from the retained stream");
        let replayed = receive_until(&second_subscription, |event| {
            event.cursor == terminal_cursor
        })
        .await;
        assert_eq!(
            replayed.last().expect("terminal replayed").cursor,
            terminal_cursor,
            "the full retained stream is replayable after reconnect"
        );
        assert!(
            replayed
                .windows(2)
                .all(|pair| pair[0].cursor < pair[1].cursor)
        );

        // A cursor ahead of the stream is unserviceable.
        let error = second
            .subscribe_events(RuntimeClientCursor::new(terminal_cursor.get() + 100))
            .expect_err("ahead of the stream");
        assert!(matches!(error, RuntimeClientError::ResyncRequired { .. }));
    }

    fn dispatch_background_fixture(
        fixture: &HostFixture,
        executor: &Arc<dyn ToolExecutor>,
        call_id: &str,
    ) -> crate::runtime::identity::ToolExecutionId {
        let prepared = fixture
            .runtime
            .tool_runtime()
            .background()
            .prepare_dispatch(
                &ToolInvocation {
                    id: crate::tools::types::ToolInvocationId::Agent {
                        call_id: ToolCallId::new(call_id),
                    },
                    tool_id: ToolId::new("tool-bg"),
                    tool_name: "bg".to_owned(),
                    mode: ToolInvocationMode::Background,
                    arguments: serde_json::json!({}),
                },
                executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = fixture
            .runtime
            .tool_runtime()
            .background()
            .commit_dispatch(prepared, &CancellationSignal::new())
            .expect("dispatch commits")
        else {
            panic!("accepted");
        };
        execution_id
    }

    /// The background lifecycle projection: Starting/Running/Cancelling/
    /// terminal transitions project from the authoritative registry, the
    /// protocol cancel is acceptance-not-settlement, and terminal records
    /// stay visible after detach.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn background_lifecycle_projection_and_protocol_cancel() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let execution_id = dispatch_background_fixture(&fixture, &executor, "call-bg");
        await_background_started(&mut started, "runner started").await;

        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let (snapshot, cursor) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        assert!(matches!(
            snapshot.jobs[0].state,
            BackgroundLifecycle::Running
        ));

        // Intent commits synchronously; the independent wait still owns physical settlement.
        let receipt = attachment
            .job_cancel(&execution_id)
            .expect("accepted intent");
        assert!(
            matches!(receipt, RuntimeClientResult::JobCancellationAccepted { job_id } if job_id == execution_id)
        );
        let wait = attachment.job_wait(&execution_id);
        tokio::pin!(wait);
        assert!(futures_util::poll!(&mut wait).is_pending());
        assert_eq!(
            fixture
                .runtime
                .background_status(&execution_id)
                .expect("job")
                .state,
            BackgroundLifecycle::Cancelling
        );
        let unknown =
            fixture
                .host
                .inner
                .job_status(&crate::runtime::identity::ToolExecutionId::new(
                    "exec_9f85dfef-c2b2-7a62-837d-620fed38822f",
                ));
        assert!(matches!(
            unknown,
            Err(RuntimeClientError::UnknownBackgroundExecution { .. })
        ));

        // Settlement: the executor raced past the cancellation request and
        // proved success. Cancellation intent owns the request fact and its
        // reason, never the physical outcome (Issue #202), so the execution
        // settles as `Succeeded`, not `Cancelled`.
        let subscription = attachment
            .subscribe_events(cursor)
            .expect("observe terminal notice attempt");
        release.send_replace(true);
        let RuntimeClientResult::JobOutcome { job_id, state } =
            wait.await.expect("physical settlement after intent")
        else {
            panic!("Job result")
        };
        assert_eq!(job_id, execution_id);
        assert_eq!(state, BackgroundLifecycle::Succeeded);

        let terminal = await_background_terminal(
            fixture.runtime.tool_runtime().background(),
            &execution_id,
            "background execution settlement after a racing cancel",
        )
        .await;
        assert_eq!(terminal.state, BackgroundLifecycle::Succeeded);
        // The background notice also starts a root attempt. Its settlement is
        // the exact authoritative boundary for the final snapshot.
        receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        })
        .await;
        let (snapshot, _) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        assert_eq!(snapshot.jobs[0].state, BackgroundLifecycle::Succeeded);
        assert!(snapshot.jobs[0].result.is_some());
    }

    #[tokio::test]
    async fn job_intent_and_waits_preserve_abandoned_publication_for_the_captured_job() {
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let registry = fixture.runtime.background_registry();
        let invocation = ToolInvocation {
            id: crate::tools::types::ToolInvocationId::Agent {
                call_id: ToolCallId::new("job-failure-call"),
            },
            tool_id: ToolId::new("tool-bg"),
            tool_name: "bg".into(),
            mode: ToolInvocationMode::Background,
            arguments: serde_json::json!({}),
        };
        let prepared = registry
            .prepare_dispatch(
                &invocation,
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .unwrap();
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = registry
            .commit_dispatch(prepared, &CancellationSignal::new())
            .unwrap()
        else {
            panic!("accepted")
        };
        started.wait_for(|started| *started).await.unwrap();
        let wait = fixture.host.inner.job_wait(&execution_id);
        tokio::pin!(wait);
        assert!(futures_util::poll!(&mut wait).is_pending());
        let receipt = fixture
            .host
            .inner
            .job_cancel_intent(&execution_id)
            .expect("accepted intent");
        assert!(
            matches!(receipt, RuntimeClientResult::JobCancellationAccepted { job_id } if job_id == execution_id)
        );
        let after_intent = fixture.host.inner.job_wait(&execution_id);
        tokio::pin!(after_intent);
        assert!(futures_util::poll!(&mut after_intent).is_pending());
        fixture.store.as_ref().unwrap().arm_fail_accept_times(2);
        release.send_replace(true);
        let results = tokio::time::timeout(BACKGROUND_LIVENESS_GUARD, async {
            [wait.await, after_intent.await]
        })
        .await
        .expect("abandoned Job observers must terminate");
        for result in results {
            assert!(
                matches!(result, Err(RuntimeClientError::JobPublicationAbandoned { job_id }) if job_id == execution_id)
            );
        }
        assert_eq!(
            registry.snapshot(&execution_id).unwrap().state,
            BackgroundLifecycle::PublishingTerminal
        );
        assert!(
            matches!(fixture.host.inner.job_wait(&execution_id).await, Err(RuntimeClientError::JobPublicationAbandoned { job_id }) if job_id == execution_id)
        );
    }

    #[tokio::test]
    async fn job_list_preserves_domain_bound_order_and_omission_metadata() {
        use crate::tools::background::MAX_JOB_LIST_LIMIT;
        let (_, fixture) = host_fixture(Vec::new(), ToolRegistry::new(), status_engine()).await;
        let (attachment, _) = fixture
            .host
            .attach(super::super::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .unwrap();
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .unwrap();
        let registry = fixture.runtime.background_registry();
        let mut ids = Vec::new();
        for index in 0..MAX_JOB_LIST_LIMIT + 3 {
            let (tool, _, release) = ParkingBackgroundTool::new();
            let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
            let invocation = ToolInvocation {
                id: crate::tools::types::ToolInvocationId::Agent {
                    call_id: ToolCallId::new(format!("job-list-{index}")),
                },
                tool_id: ToolId::new("tool-bg"),
                tool_name: "bg".into(),
                mode: ToolInvocationMode::Background,
                arguments: serde_json::json!({}),
            };
            let prepared = registry
                .prepare_dispatch(
                    &invocation,
                    &executor,
                    crate::tools::environment::ToolEnvironment::new(),
                )
                .unwrap();
            let BackgroundDispatchOutcome::Accepted { execution_id, .. } = registry
                .commit_dispatch(prepared, &CancellationSignal::new())
                .unwrap()
            else {
                panic!("accepted")
            };
            release.send_replace(true);
            registry.wait_until_terminal(&execution_id).await.unwrap();
            ids.push(execution_id);
        }
        let listing = registry.listing(false, MAX_JOB_LIST_LIMIT);
        let (returned, matched, limit, truncated) = (
            listing.returned,
            listing.matched,
            listing.limit,
            listing.truncated,
        );
        let jobs: Vec<_> = listing
            .snapshots
            .iter()
            .map(super::background_view)
            .collect();
        assert_eq!(
            (returned, matched, limit, truncated),
            (MAX_JOB_LIST_LIMIT, ids.len(), MAX_JOB_LIST_LIMIT, true)
        );
        assert_eq!(
            jobs.iter().map(|job| &job.job_id).collect::<Vec<_>>(),
            ids.iter()
                .rev()
                .take(MAX_JOB_LIST_LIMIT)
                .collect::<Vec<_>>()
        );
        receive_until(&subscription, |event| event.event.facts().iter().any(|fact| matches!(fact, RuntimeClientEvent::JobUpdated { job, .. } if Some(&job.job_id) == ids.last() && job.state == crate::tools::background::BackgroundLifecycle::Succeeded))).await;
        let RuntimeClientResult::DomainPage { page } = attachment.job_list().unwrap() else {
            panic!("bounded jobs page");
        };
        assert_eq!(page.total, ids.len() as u64);
        assert_eq!(page.items.len(), 32);
        assert_eq!(page.next_offset, Some(32));
        let mut observed: Vec<_> = page
            .items
            .iter()
            .map(|item| item.identity.clone())
            .collect();
        let mut after = page.next_offset;
        while let Some(offset) = after {
            let next = fixture
                .host
                .inner
                .checkpoint_page(&page.read_cut, "jobs", offset, 32)
                .unwrap();
            assert!(next.items.len() <= 32);
            observed.extend(next.items.iter().map(|item| item.identity.clone()));
            assert!(next.next_offset.is_none_or(|next| next > offset));
            after = next.next_offset;
        }
        assert_eq!(
            observed,
            ids.iter()
                .rev()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
        );
    }

    /// Detached background work stays visible after the originating
    /// attempt terminates.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn background_survives_attempt_termination() {
        let (_, fixture) =
            host_fixture(vec![one_turn_stop()], ToolRegistry::new(), status_engine()).await;
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = fixture
            .runtime
            .tool_runtime()
            .background()
            .prepare_dispatch(
                &ToolInvocation {
                    id: crate::tools::types::ToolInvocationId::Agent {
                        call_id: ToolCallId::new("call-bg"),
                    },
                    tool_id: ToolId::new("tool-bg"),
                    tool_name: "bg".to_owned(),
                    mode: ToolInvocationMode::Background,
                    arguments: serde_json::json!({}),
                },
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = fixture
            .runtime
            .tool_runtime()
            .background()
            .commit_dispatch(prepared, &CancellationSignal::new())
            .expect("dispatch commits")
        else {
            panic!("accepted");
        };
        await_background_started(&mut started, "runner started").await;

        // Run one attempt to completion.
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("go"),
        });
        receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        })
        .await;

        // The detached execution remains visible after the attempt
        // terminated and settles on its own schedule.
        let (snapshot, _) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        assert!(matches!(
            snapshot.attempt.expect("attempt view").phase,
            RuntimeClientAttemptPhase::Settled { .. }
        ));
        assert_eq!(snapshot.jobs.len(), 1);
        assert!(matches!(
            snapshot.jobs[0].state,
            BackgroundLifecycle::Running
        ));
        release.send_replace(true);
        let terminal = await_background_terminal(
            fixture.runtime.tool_runtime().background(),
            &execution_id,
            "detached background execution",
        )
        .await;
        assert_eq!(terminal.state, BackgroundLifecycle::Succeeded);
    }

    /// Agent Status is admitted from the exact same composition the model
    /// path consumes: the client event's rendered text equals the canonical
    /// Runtime context fact sent in the model request.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn agent_status_projection_shares_one_composition() {
        let (adapter, fixture) =
            host_fixture(vec![one_turn_stop()], ToolRegistry::new(), status_engine()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("go"),
        });
        let events = receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AgentStatusComposed { .. }))
        })
        .await;
        let facts: Vec<_> = events
            .iter()
            .flat_map(|event| event.event.facts())
            .collect();
        let status_index = facts
            .iter()
            .position(|fact| matches!(fact, RuntimeClientEvent::AgentStatusComposed { .. }))
            .expect("status event index");
        let committed_index = facts
            .iter()
            .position(|fact| {
                matches!(fact, RuntimeClientEvent::MessageCommitted { message, .. }
                if matches!(
                    message,
                    MessageBlock::User(user)
                        if matches!(
                            &user.kind,
                            crate::message::types::InboundKind::Context(
                                crate::message::types::ContextKind::AgentStatus(_)
                            )
                        )
                ))
            })
            .expect("canonical status commit event");
        assert!(
            committed_index < status_index,
            "canonical Agent Status publication precedes structured observation"
        );
        let status_event = facts
            .iter()
            .find_map(|fact| match fact {
                RuntimeClientEvent::AgentStatusComposed { status, .. } => Some(status),
                _ => None,
            })
            .expect("status event");
        // The model request is recorded slightly after the status
        // observation; wait for the attempt to settle so the request is
        // provably recorded.
        receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        })
        .await;
        let requests = adapter.requests();
        assert_eq!(requests.len(), 1);
        let model_rendered = requests[0]
            .messages
            .iter()
            .find_map(|message| match message.as_canonical() {
                Some(MessageBlock::User(user))
                    if matches!(
                        &user.kind,
                        crate::message::types::InboundKind::Context(
                            crate::message::types::ContextKind::AgentStatus(_)
                        )
                    ) =>
                {
                    user.content.first().and_then(|content| match content {
                        crate::message::types::UserContentBlock::Text(text) => {
                            Some(text.text.clone())
                        }
                        _ => None,
                    })
                }
                _ => None,
            })
            .expect("model path carries canonical Agent Status");
        assert_eq!(
            status_event.rendered, model_rendered,
            "the client view derives from the same composition as the model path"
        );
        let (snapshot, _) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        let committed_status = snapshot
            .messages
            .iter()
            .find_map(|message| match message {
                MessageBlock::User(user)
                    if matches!(
                        &user.kind,
                        crate::message::types::InboundKind::Context(
                            crate::message::types::ContextKind::AgentStatus(_)
                        )
                    ) =>
                {
                    Some(user)
                }
                _ => None,
            })
            .expect("canonical status message");
        assert_eq!(
            status_event.status_message_id,
            Some(committed_status.id.clone())
        );
        assert!(
            status_event.opportunities.fresh_inbound.is_some(),
            "the current FreshInbound-only producer still populates its opportunity"
        );
        assert!(snapshot.messages.iter().any(|message| {
            matches!(
                message,
                MessageBlock::User(user)
                    if user.id
                        == status_event
                            .opportunities
                            .fresh_inbound
                            .as_ref()
                            .expect("FreshInbound is populated by the current producer")
                            .target_message_id.as_ref().expect("native inline FreshInbound identity").clone()
            )
        }));
        assert!(matches!(
            status_event.sections.first(),
            Some(crate::runtime_client::snapshot::RuntimeClientStatusSection::Temporal { .. })
        ));
        assert_eq!(
            snapshot
                .statuses
                .last()
                .expect("status view")
                .rendered
                .clone(),
            model_rendered
        );
    }

    /// Shutdown is distinct from detach: it drains the current attempt to
    /// quiescence, and detach remains available afterwards.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    #[allow(clippy::too_many_lines)] // one complete shutdown lifecycle
    async fn shutdown_is_not_detach_and_reaches_quiescence() {
        let (release_tx, release_rx) = model_release();
        let (_, fixture) = host_fixture(
            vec![vec![
                GatedStep::Emit(ModelEvent::Started),
                GatedStep::ParkUntilReleased(release_rx),
                GatedStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ]],
            ToolRegistry::new(),
            status_engine(),
        )
        .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("go"),
        });
        receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptStarted { .. }))
        })
        .await;

        release_tx
            .send(true)
            .expect("release current model request");
        let response = attachment
            .handle_request_async(RuntimeClientRequest::Shutdown {
                id: crate::runtime_client::RequestId::new(2),
            })
            .await;
        assert!(matches!(
            response.result,
            Some(RuntimeClientResult::ShutdownCompleted)
        ));

        let first_shutdown_events = receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::RuntimeShutdown))
        })
        .await;
        let (after_shutdown, _) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot after native shutdown publication");
        assert!(after_shutdown.shutting_down);
        assert_eq!(
            first_shutdown_events
                .iter()
                .filter(|event| event
                    .event
                    .facts()
                    .iter()
                    .any(|fact| matches!(fact, RuntimeClientEvent::RuntimeShutdown)))
                .count(),
            1
        );
        let repeated = attachment
            .handle_request_async(RuntimeClientRequest::Shutdown {
                id: crate::runtime_client::RequestId::new(5),
            })
            .await;
        assert!(matches!(
            repeated.result,
            Some(RuntimeClientResult::ShutdownCompleted)
        ));
        let mut duplicate_shutdown = false;
        loop {
            match subscription.try_next() {
                EventDelivery::Event(event) => {
                    duplicate_shutdown |= event
                        .event
                        .facts()
                        .iter()
                        .any(|fact| matches!(fact, RuntimeClientEvent::RuntimeShutdown));
                }
                EventDelivery::Pending => break,
                delivery => panic!("subscription remains open after repeat: {delivery:?}"),
            }
        }
        assert!(
            !duplicate_shutdown,
            "repeated shutdown publishes no duplicate fact"
        );
        let snapshot_response = attachment.handle_request(RuntimeClientRequest::CheckpointGet {
            id: crate::runtime_client::RequestId::new(4),
        });
        let Some(RuntimeClientResult::Checkpoint { checkpoint }) = snapshot_response.result else {
            panic!("checkpoint returns the shutdown state: {snapshot_response:?}");
        };
        assert!(checkpoint.snapshot.shutting_down);

        // Further admission fails explicitly.
        let submit = attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(3),
            content: submit_content("too late"),
        });
        assert!(matches!(
            submit.error,
            Some(RuntimeClientError::RuntimeShutdown)
        ));

        // The current attempt settled before shutdown completed; detach still
        // works independently of runtime lifetime.
        attachment.detach();
        let (reattached, initialized) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach after shutdown still works");
        let RuntimeClientResult::Initialized { .. } = initialized else {
            panic!("fresh initialize returns a bounded identity receipt");
        };
        let (snapshot, _) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .unwrap();
        assert!(snapshot.shutting_down);
        let (snapshot, _) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        assert!(matches!(
            snapshot.attempt.expect("attempt view").phase,
            RuntimeClientAttemptPhase::Settled { .. }
        ));
        assert!(
            reattached
                .handle_request(RuntimeClientRequest::CheckpointGet {
                    id: crate::runtime_client::RequestId::new(1),
                })
                .error
                .is_none()
        );
    }

    /// One human inbound through the Runtime Client and one Runtime/Agent
    /// inbound through the native publisher reach the same coordinator
    /// admission path: one finite batch, mailbox order preserved, exactly
    /// one attempt (Test 3).
    ///
    /// The admission gate makes the interleaving exact: the async enqueue
    /// starts an admission that parks before the coordinator lock, so the
    /// human submit provably lands in the mailbox before the finite drain.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn human_and_runtime_inbound_share_one_admission_path() {
        let admission_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let (adapter, fixture) = host_fixture_probe_with_runtime_gate(
            Arc::new(crate::runtime_client::test_sync::ProjectionProbe::default()),
            vec![one_turn_stop()],
            CoordinatorProbe {
                admission_gate: Some(admission_gate.clone()),
                settlement_gate: None,
                before_restore_gate: None,
                activation_gate: None,
                manual_compaction_settlement_gate: None,
                submit_gate: None,
                submit_arrival: None,
                shutdown_arrival: None,
                mcp_failure_drain_gate: None,
                start_boundary_pause: None,
                model_arbitration_pause: None,
                drain_linearization: None,
                tool_start_pause: None,
                drain_supervision: None,
                attempt_exit_gate: None,
                parent_guidance_seal_gate: None,
                background_failure_gate: None,
                subagent_failure_published_gate: None,
                ..CoordinatorProbe::default()
            },
        )
        .await;
        admission_gate.arm();
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");

        // The native Runtime producer publishes first (a background-style
        // terminal notification), waking the admission worker; the worker
        // parks at the runtime gate before the coordinator lock, so the
        // human submit below provably lands before the finite drain.
        fixture
            .runtime
            .tool_runtime()
            .mailbox()
            .enqueue(inbound_text(
                "conv_15466ddf-043b-7e5a-9915-c32c837b38bc",
                "runtime",
            ))
            .expect("runtime enqueue");
        admission_gate.wait_entered();
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("human"),
        });

        // Release: one admission drains both messages in mailbox order.
        admission_gate.release();
        receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        })
        .await;

        // Exactly one attempt observed both messages in mailbox order.
        let requests = adapter.requests();
        assert_eq!(requests.len(), 1, "one admission, one attempt");
        let inbound_ids: Vec<&str> = requests[0]
            .messages
            .iter()
            .filter_map(|message| match message.as_canonical() {
                Some(MessageBlock::User(user))
                    if user.kind == crate::message::types::InboundKind::Message =>
                {
                    Some(user.id.as_str())
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            inbound_ids,
            vec![
                "conv_15466ddf-043b-7e5a-9915-c32c837b38bc",
                "conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b-inbound-2"
            ],
            "both producers sequence through one durable sequence domain in order"
        );
        let (snapshot, _) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        assert!(
            snapshot.inbound.pending.is_empty(),
            "the batch was consumed exactly once"
        );
    }

    /// An enqueue racing attempt settlement loses nothing and creates at
    /// most one next attempt (Test 6).
    ///
    /// The settlement gate parks `finish_attempt` after the conversation
    /// state is restored and the current-attempt slot is cleared, before
    /// the next-admission handoff; the enqueue during that park provably
    /// races the settlement boundary.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn enqueue_racing_settlement_admits_exactly_one_next_attempt() {
        let settlement_gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let (adapter, fixture) = host_fixture_with_runtime_probe(
            vec![one_turn_stop(), one_turn_stop()],
            CoordinatorProbe {
                admission_gate: None,
                settlement_gate: Some(settlement_gate.clone()),
                before_restore_gate: None,
                activation_gate: None,
                manual_compaction_settlement_gate: None,
                submit_gate: None,
                submit_arrival: None,
                shutdown_arrival: None,
                mcp_failure_drain_gate: None,
                start_boundary_pause: None,
                model_arbitration_pause: None,
                drain_linearization: None,
                tool_start_pause: None,
                drain_supervision: None,
                attempt_exit_gate: None,
                parent_guidance_seal_gate: None,
                background_failure_gate: None,
                subagent_failure_published_gate: None,
                ..CoordinatorProbe::default()
            },
        )
        .await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");

        // Run the first attempt; its settlement handoff parks at the gate
        // after the conversation restore and the slot clear.
        settlement_gate.arm();
        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("first"),
        });
        settlement_gate.wait_entered();

        // An ordinary async enqueue lands while the settlement handoff is
        // parked (the gate holds the coordinator lock after the
        // conversation restore and the slot clear, so the test never reads
        // coordinator state while it is parked).
        fixture
            .runtime
            .tool_runtime()
            .mailbox()
            .enqueue(inbound_text(
                "conv_9ec557e9-8373-72d1-92d3-cf39a531c6cf",
                "racing settlement",
            ))
            .expect("async enqueue");

        // Release the handoff: exactly one next attempt consumes the
        // inbound; the settlement path never consumes it again. Waiting on
        // the request-history transfer makes both settlements provable
        // before the assertions below.
        settlement_gate.release();
        await_request_history_len(&fixture.host, 2).await;
        await_adapter_request_count(&adapter, 2).await;
        let requests = adapter.requests();
        assert_eq!(
            requests.len(),
            2,
            "exactly one next attempt (never two, never zero)"
        );
        assert!(
            requests[1].messages.iter().any(|message| matches!(
                message.as_canonical(),
                Some(MessageBlock::User(user)) if user.id.as_str() == "conv_9ec557e9-8373-72d1-92d3-cf39a531c6cf"
            )),
            "the racing inbound was consumed exactly once by the next attempt"
        );
        let (snapshot, _) = fixture
            .host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        assert!(
            snapshot.inbound.pending.is_empty(),
            "no second consumption of the same batch"
        );
    }

    /// The safe-boundary tool-batch invariant (Test 8): with sibling tool
    /// calls A and B, an async inbound arriving while A executes is never
    /// interleaved between the tool results — the full sibling structural
    /// settlement lands before the inbound enters model-visible context.
    #[allow(clippy::too_many_lines)] // one complete tool-batch lifecycle
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn safe_boundary_keeps_sibling_tool_results_together() {
        use crate::runtime::identity::ToolCallId;
        let (tool_a, mut a_started, a_release) = ParkingBackgroundTool::new();
        let (tool_b, mut b_started, b_release) = ParkingBackgroundTool::new();
        let definition_a = ToolDefinition {
            id: ToolId::new("tool-a"),
            name: "a".to_owned(),
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            concurrency_policy: ToolConcurrencyPolicy::Parallel,
            ..tool_a.definition.clone()
        };
        let definition_b = ToolDefinition {
            id: ToolId::new("tool-b"),
            name: "b".to_owned(),
            execution_policy: ToolExecutionPolicy::ForegroundOnly,
            concurrency_policy: ToolConcurrencyPolicy::Parallel,
            ..tool_b.definition.clone()
        };
        let mut tools = ToolRegistry::new();
        tools
            .register(definition_a.clone(), Arc::new(tool_a))
            .expect("register a");
        tools
            .register(definition_b.clone(), Arc::new(tool_b))
            .expect("register b");
        let call_a = ToolCallId::new("call-a");
        let call_b = ToolCallId::new("call-b");
        let script = vec![
            GatedStep::Emit(ModelEvent::Started),
            GatedStep::Emit(ModelEvent::ToolCallStarted {
                block_index: ContentBlockIndex::new(0),
                call: crate::tools::types::ToolCallStart {
                    id: call_a.clone(),
                    tool_id: definition_a.id.clone(),
                    name: definition_a.name.clone(),
                },
            }),
            GatedStep::Emit(ModelEvent::ToolCallArgumentsDelta {
                block_index: ContentBlockIndex::new(0),
                call_id: call_a.clone(),
                arguments_delta: "{}".to_owned(),
            }),
            GatedStep::Emit(ModelEvent::ToolCallCompleted {
                block_index: ContentBlockIndex::new(0),
                call: crate::tools::types::ToolCall {
                    id: call_a.clone(),
                    tool_id: definition_a.id.clone(),
                    name: definition_a.name.clone(),
                    arguments: serde_json::json!({}),
                },
            }),
            GatedStep::Emit(ModelEvent::ToolCallStarted {
                block_index: ContentBlockIndex::new(1),
                call: crate::tools::types::ToolCallStart {
                    id: call_b.clone(),
                    tool_id: definition_b.id.clone(),
                    name: definition_b.name.clone(),
                },
            }),
            GatedStep::Emit(ModelEvent::ToolCallArgumentsDelta {
                block_index: ContentBlockIndex::new(1),
                call_id: call_b.clone(),
                arguments_delta: "{}".to_owned(),
            }),
            GatedStep::Emit(ModelEvent::ToolCallCompleted {
                block_index: ContentBlockIndex::new(1),
                call: crate::tools::types::ToolCall {
                    id: call_b.clone(),
                    tool_id: definition_b.id.clone(),
                    name: definition_b.name.clone(),
                    arguments: serde_json::json!({}),
                },
            }),
            GatedStep::Emit(ModelEvent::Completed {
                finish_reason: ModelFinishReason::ToolCalls,
                usage: None,
            }),
        ];
        let (adapter, fixture) =
            host_fixture(vec![script, one_turn_stop()], tools, status_engine()).await;
        let (attachment, _) = fixture
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");

        attachment.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("go"),
        });
        // Both sibling tool calls start (the loop executes the batch); A
        // parks.
        await_background_started(&mut a_started, "tool A started").await;
        await_background_started(&mut b_started, "tool B started").await;

        // An async inbound arrives while the sibling batch is in flight.
        fixture
            .runtime
            .tool_runtime()
            .mailbox()
            .enqueue(inbound_text(
                "conv_9ec557e9-8373-72d1-92d3-cf39a531c6cf",
                "during batch",
            ))
            .expect("async enqueue");

        // Release both tools; the batch settles structurally, the safe
        // boundary drains the inbound into the next turn.
        a_release.send_replace(true);
        b_release.send_replace(true);
        receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        })
        .await;
        let requests = adapter.requests();
        assert_eq!(requests.len(), 2, "tool turn + drained-inbound turn");
        // The second request replays the whole conversation; the model-
        // visible tail must be ToolResult A, ToolResult B, then the drained
        // inbound — the sibling structural settlement always lands before
        // the inbound enters model-visible context.
        let roles: Vec<&str> = requests[1]
            .messages
            .iter()
            .filter_map(|message| match message.as_canonical() {
                Some(MessageBlock::Tool(_)) => Some("tool"),
                Some(MessageBlock::User(user))
                    if user.kind == crate::message::types::InboundKind::Message =>
                {
                    Some("user")
                }
                _ => None,
            })
            .collect();
        assert_eq!(
            &roles[roles.len() - 3..],
            &["tool", "tool", "user"],
            "ToolResult A, ToolResult B, then the inbound — never interleaved"
        );
        assert!(
            requests[1].messages.iter().any(|message| matches!(
                message.as_canonical(),
                Some(MessageBlock::User(user)) if user.id.as_str() == "conv_9ec557e9-8373-72d1-92d3-cf39a531c6cf"
            )),
            "the drained inbound is the async one"
        );
    }

    /// Model configuration freezes at the admission boundary (Test 10):
    /// an update that linearizes before admission is observed by the
    /// admitted attempt; one that linearizes after admission affects only
    /// future attempts.
    #[allow(clippy::too_many_lines)] // two full freeze interleavings
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn model_update_freezes_at_admission() {
        // Interleaving B: the update linearizes after the admission.
        let (release_b_tx, release_b_rx) = model_release();
        let (adapter_b, fixture_b) = host_fixture(
            vec![vec![
                GatedStep::Emit(ModelEvent::Started),
                GatedStep::ParkUntilReleased(release_b_rx),
                GatedStep::Emit(ModelEvent::Completed {
                    finish_reason: ModelFinishReason::Stop,
                    usage: None,
                }),
            ]],
            ToolRegistry::new(),
            status_engine(),
        )
        .await;
        let (attachment_b, _) = fixture_b
            .host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription_b = attachment_b
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");
        attachment_b.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(1),
            content: submit_content("first"),
        });
        // The attempt is provably admitted (its model stream is parked).
        receive_until(&subscription_b, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptStarted { .. }))
        })
        .await;

        // The update lands mid-attempt: the running attempt keeps its
        // frozen snapshot.
        let mut updated = fixture_b.runtime.model_config();
        updated.request_params.insert(
            "frozen_probe".to_owned(),
            serde_json::json!("updated-after-admission"),
        );
        fixture_b
            .host
            .model_set(updated)
            .expect("active selection commits");

        release_b_tx.send(true).expect("release");
        receive_until(&subscription_b, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        })
        .await;
        let requests = adapter_b.requests();
        assert!(
            requests[0].request_params().get("frozen_probe").is_none(),
            "the admitted attempt never observes the post-admission update"
        );

        // A later attempt observes the selection already committed while active.
        attachment_b.handle_request(RuntimeClientRequest::SubmitInbound {
            id: crate::runtime_client::RequestId::new(2),
            content: submit_content("second"),
        });
        receive_until(&subscription_b, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        })
        .await;
        let requests = adapter_b.requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(
            requests[1].request_params().get("frozen_probe"),
            Some(&serde_json::json!("updated-after-admission")),
            "a future attempt observes the update"
        );
    }

    /// Builds a host with the projection linearization probe installed.
    async fn host_fixture_probe(
        probe: Arc<crate::runtime_client::test_sync::ProjectionProbe>,
        scripts: Vec<Vec<GatedStep>>,
    ) -> (Arc<GatedAdapter>, HostFixture) {
        host_fixture_probe_with_native_tools(probe, scripts, false).await
    }

    /// Probe fixture variant with the real native tool plane activated.
    #[allow(
        clippy::too_many_lines,
        reason = "one complete deterministic fixture boundary"
    )]
    async fn host_fixture_probe_with_native_tools(
        probe: Arc<crate::runtime_client::test_sync::ProjectionProbe>,
        scripts: Vec<Vec<GatedStep>>,
        include_native_tools: bool,
    ) -> (Arc<GatedAdapter>, HostFixture) {
        let adapter = Arc::new(GatedAdapter::new(scripts));
        let dir = tempfile::tempdir().expect("temp dir");
        let conversation_id = ConversationId::new("conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
            conversation_id.clone(),
            crate::tools::runtime::ConversationRuntimeConfig::new(
                &workspace,
                dir.path().join("artifacts"),
            )
            // These Runtime Client fixtures compose Agent Status and nothing
            // else: the composition must match the status engine below and
            // the extension Tool plane the coordinator is given, because
            // `ConversationRuntime` construction now proves all three follow
            // from one frozen decision (Issue #259). The Todo extension has
            // its own suites; composing it here would only add an unrelated
            // Tool and an unrelated conversation-owned resource to every
            // host contract.
            .with_extensions(
                crate::extensions::NativeAgentExtensions::with_agent_status(
                    crate::context::AgentStatusConfig::default(),
                ),
            ),
        )
        .expect("tool runtime");
        let mut tools = ToolRegistry::new();
        if include_native_tools {
            crate::tools::register_native_tools(
                &mut tools,
                crate::tools::NativeToolResources {
                    subagent_catalog: crate::runtime::subagent::AgentCatalog::empty(),
                    background: tool_runtime.background().clone(),
                    subagents: None,
                },
                crate::tools::NativeToolPolicies::default(),
            )
            .expect("register native tools");
        }
        let coordinator = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                source_demand: crate::capabilities::source::ToolSourceDemand::default(),
                conversation_id: conversation_id.clone(),
                workspace: tool_runtime.workspace().clone(),
                agent_activation: {
                    let mut activation = crate::capabilities::AgentActivation::default();
                    activation.profile.skills =
                        Some(crate::runtime::agent_profile::AgentSkillSelection::All);
                    activation.profile.extensions =
                        crate::scripted_suites::common::plugin_document(tool_runtime.extensions());
                    activation.profile.tools.builtin = tools
                        .definitions()
                        .into_iter()
                        .filter(|tool| tool.origin.source().is_none())
                        .map(|tool| tool.name.clone())
                        .collect();
                    activation
                },
                base_tool_registry: Arc::new(tools),
                extension_tools: tool_runtime.extension_tool_plane(),
                skill_discovery: crate::skills::SkillDiscoveryConfig::workspace_root(
                    workspace.join(".agents/skills"),
                ),
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");
        let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
        let runtime = ConversationRuntime::new(RuntimeConversationConfig {
            explicit_model: true,
            agent_id: AgentId::new("agent-a"),
            model: crate::scripted_suites::support::model::scripted_session_model(adapter.clone()),
            approval_mode: crate::runtime::ApprovalMode::Policy,
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator,
                status_engine: Some(status_engine()),
            },
            tool_runtime,
            resources: test_resources(&coordinator),
            resource_loader: test_resource_loader(&coordinator),
            capability: coordinator.clone(),
            clock: Some(Arc::new(FixedRuntimeClock)),
            initial_messages: Vec::new(),
            subagents: None,
            workflow_output: None,
        })
        .expect("conversation runtime");
        let host = RuntimeClientHost::with_probe(
            RuntimeClientHostConfig {
                runtime: runtime.clone(),
                replay_limit: None,
            },
            (*probe).clone(),
        )
        .expect("host");
        runtime.activate();
        (
            adapter,
            HostFixture {
                directory: dir,
                host,
                runtime,
                coordinator,
                store: None,
            },
        )
    }

    /// Builds a host whose runtime carries both the projection probe and
    /// the coordinator synchronization hooks.
    async fn host_fixture_probe_with_runtime_gate(
        probe: Arc<crate::runtime_client::test_sync::ProjectionProbe>,
        scripts: Vec<Vec<GatedStep>>,
        runtime_probe: CoordinatorProbe,
    ) -> (Arc<GatedAdapter>, HostFixture) {
        let adapter = Arc::new(GatedAdapter::new(scripts));
        let dir = tempfile::tempdir().expect("temp dir");
        let conversation_id = ConversationId::new("conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
            conversation_id.clone(),
            crate::tools::runtime::ConversationRuntimeConfig::new(
                &workspace,
                dir.path().join("artifacts"),
            )
            // These Runtime Client fixtures compose Agent Status and nothing
            // else: the composition must match the status engine below and
            // the extension Tool plane the coordinator is given, because
            // `ConversationRuntime` construction now proves all three follow
            // from one frozen decision (Issue #259). The Todo extension has
            // its own suites; composing it here would only add an unrelated
            // Tool and an unrelated conversation-owned resource to every
            // host contract.
            .with_extensions(
                crate::extensions::NativeAgentExtensions::with_agent_status(
                    crate::context::AgentStatusConfig::default(),
                ),
            ),
        )
        .expect("tool runtime");
        let coordinator = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                source_demand: crate::capabilities::source::ToolSourceDemand::default(),
                conversation_id: conversation_id.clone(),
                workspace: tool_runtime.workspace().clone(),
                base_tool_registry: Arc::new(ToolRegistry::new()),
                extension_tools: tool_runtime.extension_tool_plane(),
                agent_activation: crate::capabilities::AgentActivation::default(),
                skill_discovery: crate::skills::SkillDiscoveryConfig::workspace_root(
                    workspace.join(".agents/skills"),
                ),
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");
        let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
        let runtime = ConversationRuntime::with_probe(
            RuntimeConversationConfig {
                explicit_model: true,
                agent_id: AgentId::new("agent-a"),
                model: crate::scripted_suites::support::model::scripted_session_model(
                    adapter.clone(),
                ),
                approval_mode: crate::runtime::ApprovalMode::Policy,
                model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
                tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(
                ),
                context: ConversationContextConfig {
                    policy: crate::context::SessionContextPolicy {
                        reserve_tokens: 0,
                        keep_recent_tokens: 0,
                        summary_output_cap: None,
                    },
                    estimator,
                    status_engine: Some(status_engine()),
                },
                tool_runtime,
                resources: test_resources(&coordinator),
                resource_loader: test_resource_loader(&coordinator),
                capability: coordinator.clone(),
                clock: Some(Arc::new(FixedRuntimeClock)),
                initial_messages: Vec::new(),
                subagents: None,
                workflow_output: None,
            },
            runtime_probe,
        )
        .expect("conversation runtime with probe");
        let host = RuntimeClientHost::with_probe(
            RuntimeClientHostConfig {
                runtime: runtime.clone(),
                replay_limit: None,
            },
            (*probe).clone(),
        )
        .expect("host");
        runtime.activate();
        (
            adapter,
            HostFixture {
                directory: dir,
                host,
                runtime,
                coordinator,
                store: None,
            },
        )
    }

    /// A fixture over one conversation runtime **without** a Runtime
    /// Client host, so a test controls host construction itself (the
    /// Issue #61 bootstrap regressions).
    struct RuntimeOnlyFixture {
        _dir: tempfile::TempDir,
        runtime: ConversationRuntime,
    }

    /// Builds the conversation runtime alone (no host), with the given
    /// scripts, tool registry, and optional coordinator probe.
    async fn runtime_only_fixture(
        scripts: Vec<Vec<GatedStep>>,
        tools: ToolRegistry,
        probe: Option<CoordinatorProbe>,
    ) -> (Arc<GatedAdapter>, RuntimeOnlyFixture) {
        let conversation_id = ConversationId::new("conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b");
        let adapter = Arc::new(GatedAdapter::new(scripts));
        let dir = tempfile::tempdir().expect("temp dir");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
            conversation_id.clone(),
            crate::tools::runtime::ConversationRuntimeConfig::new(
                &workspace,
                dir.path().join("artifacts"),
            )
            // These Runtime Client fixtures compose Agent Status and nothing
            // else: the composition must match the status engine below and
            // the extension Tool plane the coordinator is given, because
            // `ConversationRuntime` construction now proves all three follow
            // from one frozen decision (Issue #259). The Todo extension has
            // its own suites; composing it here would only add an unrelated
            // Tool and an unrelated conversation-owned resource to every
            // host contract.
            .with_extensions(
                crate::extensions::NativeAgentExtensions::with_agent_status(
                    crate::context::AgentStatusConfig::default(),
                ),
            ),
        )
        .expect("tool runtime");
        let coordinator = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                source_demand: crate::capabilities::source::ToolSourceDemand::default(),
                conversation_id: conversation_id.clone(),
                workspace: tool_runtime.workspace().clone(),
                agent_activation: {
                    let mut activation = crate::capabilities::AgentActivation::default();
                    activation.profile.skills =
                        Some(crate::runtime::agent_profile::AgentSkillSelection::All);
                    activation.profile.extensions =
                        crate::scripted_suites::common::plugin_document(tool_runtime.extensions());
                    activation.profile.tools.builtin = tools
                        .definitions()
                        .into_iter()
                        .filter(|tool| tool.origin.source().is_none())
                        .map(|tool| tool.name.clone())
                        .collect();
                    activation
                },
                base_tool_registry: Arc::new(tools),
                extension_tools: tool_runtime.extension_tool_plane(),
                skill_discovery: crate::skills::SkillDiscoveryConfig::workspace_root(
                    workspace.join(".agents/skills"),
                ),
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");
        let estimator: Arc<dyn TokenEstimator> = Arc::new(DefaultTokenEstimator);
        let config = RuntimeConversationConfig {
            explicit_model: true,
            agent_id: AgentId::new("agent-a"),
            model: scripted_session_model(adapter.clone()),
            approval_mode: crate::runtime::ApprovalMode::Policy,
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator,
                status_engine: Some(status_engine()),
            },
            tool_runtime,
            resources: test_resources(&coordinator),
            resource_loader: test_resource_loader(&coordinator),
            capability: coordinator.clone(),
            clock: Some(Arc::new(FixedRuntimeClock)),
            initial_messages: Vec::new(),
            subagents: None,
            workflow_output: None,
        };
        let runtime = match probe {
            Some(probe) => ConversationRuntime::with_probe(config, probe).expect("runtime"),
            None => ConversationRuntime::new(config).expect("runtime"),
        };
        (adapter, RuntimeOnlyFixture { _dir: dir, runtime })
    }

    /// A marker-bearing alternate session model configuration.
    fn marked_model_config(
        runtime: &ConversationRuntime,
        marker: &str,
    ) -> crate::model::session::SessionModelConfig {
        let mut config = runtime.model_config();
        config
            .request_params
            .insert(marker.to_owned(), serde_json::json!("changed"));
        config
    }

    /// Test B1 — the interactive composition: construct the runtime, bind
    /// the Runtime Client host over the inert runtime, activate, and run a
    /// real turn end to end.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn interactive_pre_activation_bind_runs_a_real_turn() {
        let (adapter, fixture) =
            runtime_only_fixture(vec![one_turn_stop()], ToolRegistry::new(), None).await;
        assert!(
            !fixture.runtime.is_activated(),
            "a freshly constructed runtime is inert"
        );

        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: fixture.runtime.clone(),
            replay_limit: None,
        })
        .expect("a host binds before activation");
        fixture.runtime.activate();
        assert!(fixture.runtime.is_activated());

        let (attachment, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let (_, cursor) = host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        let subscription = attachment.subscribe_events(cursor).expect("subscribe");
        attachment
            .handle_request(RuntimeClientRequest::SubmitInbound {
                id: crate::runtime_client::RequestId::new(1),
                content: submit_content("drive a turn"),
            })
            .result
            .expect("accepted");
        let events = receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        })
        .await;
        assert_eq!(
            events
                .iter()
                .filter(|event| event
                    .event
                    .facts()
                    .iter()
                    .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. })))
                .count(),
            1,
            "exactly one terminal settlement"
        );
        assert_eq!(adapter.requests().len(), 1, "the real provider path ran");
    }

    /// Test B2 — the headless composition: construct the runtime, bind no
    /// host at all, activate, and run a real turn.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn headless_activation_runs_without_any_client_host() {
        let (adapter, fixture) =
            runtime_only_fixture(vec![one_turn_stop()], ToolRegistry::new(), None).await;
        fixture.runtime.activate();

        // Armed before the trigger: settlement wakes only registered waiters.
        let settled = fixture.runtime.settlement_signal().notified();
        tokio::pin!(settled);
        settled.as_mut().enable();
        fixture
            .runtime
            .submit_inbound(submit_content("headless"))
            .expect("accepted");
        settled.await;
        assert_eq!(adapter.requests().len(), 1, "the real provider path ran");
        assert!(
            !fixture.runtime.tool_runtime().is_runtime_client_bound(),
            "no Runtime Client host ever existed"
        );
    }

    /// Test B3 — a Runtime Client host bind after activation is refused
    /// with the explicit lifecycle error: no panic, no partial bridge, and
    /// the one-time client binding claim is not consumed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn late_host_bind_after_activation_is_rejected_typed() {
        let (adapter, fixture) = runtime_only_fixture(
            vec![one_turn_stop(), one_turn_stop()],
            ToolRegistry::new(),
            None,
        )
        .await;
        fixture.runtime.activate();
        // Semantic execution really started before the bind attempt.
        // Armed before the trigger: settlement wakes only registered waiters.
        let started = fixture.runtime.settlement_signal().notified();
        tokio::pin!(started);
        started.as_mut().enable();
        fixture
            .runtime
            .submit_inbound(submit_content("start executing"))
            .expect("accepted");
        started.await;
        assert_eq!(adapter.requests().len(), 1);

        let rejected = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: fixture.runtime.clone(),
            replay_limit: None,
        });
        match rejected {
            Err(HostConstructionError::RuntimeAlreadyActivated { conversation_id }) => {
                assert_eq!(
                    conversation_id.as_str(),
                    "conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b"
                );
            }
            _ => panic!("a post-activation host bind must fail typed"),
        }
        // Transactional: the rejected construction consumed no binding
        // claim and installed no bridge.
        assert!(
            !fixture.runtime.tool_runtime().is_runtime_client_bound(),
            "the tool runtime binding claim was not consumed"
        );
        assert!(
            !fixture.runtime.capability().is_runtime_client_bound(),
            "the capability binding claim was not consumed"
        );
        // The runtime keeps executing normally afterwards.
        // Armed before the trigger: settlement wakes only registered waiters.
        let kept_going = fixture.runtime.settlement_signal().notified();
        tokio::pin!(kept_going);
        kept_going.as_mut().enable();
        fixture
            .runtime
            .submit_inbound(submit_content("keep going"))
            .expect("accepted");
        kept_going.await;
        assert_eq!(
            adapter.requests().len(),
            2,
            "the runtime keeps executing after the rejected bind"
        );
    }

    /// Test B4 — attachments stay dynamic after activation: attach, detach
    /// while an attempt is running, and reattach, without ever affecting
    /// semantic execution.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn attachments_remain_dynamic_after_activation() {
        let (release_tx, release_rx) = model_release();
        let (adapter, fixture) = runtime_only_fixture(
            vec![
                vec![
                    GatedStep::Emit(ModelEvent::Started),
                    GatedStep::Emit(ModelEvent::TextDelta {
                        block_index: ContentBlockIndex::new(0),
                        text: "working".to_owned(),
                    }),
                    GatedStep::ParkUntilReleased(release_rx),
                    GatedStep::Emit(ModelEvent::Completed {
                        finish_reason: ModelFinishReason::Stop,
                        usage: None,
                    }),
                ],
                one_turn_stop(),
            ],
            ToolRegistry::new(),
            None,
        )
        .await;
        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: fixture.runtime.clone(),
            replay_limit: None,
        })
        .expect("host binds before activation");
        fixture.runtime.activate();

        let (first, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let (_, cursor) = host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        let subscription = first.subscribe_events(cursor).expect("subscribe");
        first
            .handle_request(RuntimeClientRequest::SubmitInbound {
                id: crate::runtime_client::RequestId::new(1),
                content: submit_content("first"),
            })
            .result
            .expect("accepted");
        // Wait for a real in-flight streaming fact through the client
        // projection, then detach mid-attempt.
        receive_until(&subscription, |event| {
            event.event.facts().iter().any(|fact| matches!(fact, RuntimeClientEvent::BlockContentAdvanced { kind, .. } if kind == "text"))
        })
        .await;
        first.detach();

        // The detached attempt still settles canonically.
        // Armed before the trigger: settlement wakes only registered waiters.
        let settled = fixture.runtime.settlement_signal().notified();
        tokio::pin!(settled);
        settled.as_mut().enable();
        release_tx.send(true).expect("release the parked attempt");
        settled.await;
        assert_eq!(adapter.requests().len(), 1, "the attempt ran to settlement");

        // Reattach: a fresh attachment over the same host and the same
        // semantic owners, and the projection observed the settlement it
        // was detached for.
        let (second, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("reattach");
        let (snapshot, cursor) = host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        assert!(
            matches!(
                snapshot.attempt.as_ref().map(|attempt| &attempt.phase),
                Some(RuntimeClientAttemptPhase::Settled { .. })
            ),
            "the detach never altered semantic execution"
        );
        let subscription = second.subscribe_events(cursor).expect("subscribe");
        second
            .handle_request(RuntimeClientRequest::SubmitInbound {
                id: crate::runtime_client::RequestId::new(2),
                content: submit_content("second"),
            })
            .result
            .expect("accepted");
        receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        })
        .await;
        assert_eq!(adapter.requests().len(), 2, "the reattached turn ran");
    }

    /// Regression for the PR #70 test-fixture lost wakeup: release the
    /// background execution while its returned future is intentionally held
    /// before the old `Notify::notified()` wait point. Durable release state
    /// must still settle the one committed record exactly once, without a
    /// second registry record or an invented admission.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn background_release_before_future_wait_cannot_be_lost() {
        let fixture = ownership_fixture(Vec::new(), ToolRegistry::new()).await;
        let registry = fixture.tool_runtime.background().clone();
        let (tool, mut started, release, execution_gate) =
            ParkingBackgroundTool::new_with_execution_gate();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = registry
            .prepare_dispatch(
                &claim_background_invocation("call-lost-wakeup"),
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let outcome = registry
            .commit_dispatch(prepared, &CancellationSignal::new())
            .expect("dispatch commits");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
            panic!("accepted dispatch");
        };

        await_background_started(&mut started, "lost-wakeup regression runner").await;
        // This is the critical ordering: release is durable before the
        // returned future is allowed to reach its wait point.
        release.send_replace(true);
        execution_gate.send_replace(true);

        let terminal =
            await_background_terminal(&registry, &execution_id, "lost-wakeup regression").await;
        assert_eq!(terminal.state, BackgroundLifecycle::Succeeded);
        assert_eq!(
            registry.all_snapshots().len(),
            1,
            "no duplicate background record"
        );
        assert!(
            terminal.result.is_some(),
            "the single execution has one result"
        );
    }

    /// The standalone pre-runtime pieces of one ownership-transfer test:
    /// the tool runtime and the capability coordinator exist over one
    /// conversation, but no `ConversationRuntime` owns them yet.
    struct OwnershipFixture {
        _dir: tempfile::TempDir,
        adapter: Arc<GatedAdapter>,
        tool_runtime: crate::tools::runtime::ConversationToolRuntime,
        coordinator: crate::capabilities::CapabilityCoordinator,
    }

    /// Builds the standalone pieces exactly like the runtime fixtures, but
    /// stops before the `ConversationRuntime` construction.
    async fn ownership_fixture(
        scripts: Vec<Vec<GatedStep>>,
        tools: ToolRegistry,
    ) -> OwnershipFixture {
        let adapter = Arc::new(GatedAdapter::new(scripts));
        let dir = tempfile::tempdir().expect("temp dir");
        let conversation_id = ConversationId::new("conv_29ce21a0-54d0-7f99-88a0-dda036ee7dbc");
        let workspace = dir.path().join("workspace");
        std::fs::create_dir_all(&workspace).expect("workspace");
        let tool_runtime = crate::tools::runtime::ConversationToolRuntime::from_config(
            conversation_id.clone(),
            crate::tools::runtime::ConversationRuntimeConfig::new(
                &workspace,
                dir.path().join("artifacts"),
            )
            // These Runtime Client fixtures compose Agent Status and nothing
            // else: the composition must match the status engine below and
            // the extension Tool plane the coordinator is given, because
            // `ConversationRuntime` construction now proves all three follow
            // from one frozen decision (Issue #259). The Todo extension has
            // its own suites; composing it here would only add an unrelated
            // Tool and an unrelated conversation-owned resource to every
            // host contract.
            .with_extensions(
                crate::extensions::NativeAgentExtensions::with_agent_status(
                    crate::context::AgentStatusConfig::default(),
                ),
            ),
        )
        .expect("tool runtime");
        let coordinator = crate::capabilities::CapabilityCoordinator::new(
            crate::capabilities::CapabilityCoordinatorConfig {
                source_demand: crate::capabilities::source::ToolSourceDemand::default(),
                conversation_id: conversation_id.clone(),
                workspace: tool_runtime.workspace().clone(),
                agent_activation: {
                    let mut activation = crate::capabilities::AgentActivation::default();
                    activation.profile.skills =
                        Some(crate::runtime::agent_profile::AgentSkillSelection::All);
                    activation.profile.extensions =
                        crate::scripted_suites::common::plugin_document(tool_runtime.extensions());
                    activation.profile.tools.builtin = tools
                        .definitions()
                        .into_iter()
                        .filter(|tool| tool.origin.source().is_none())
                        .map(|tool| tool.name.clone())
                        .collect();
                    activation
                },
                base_tool_registry: Arc::new(tools),
                extension_tools: tool_runtime.extension_tool_plane(),
                skill_discovery: crate::skills::SkillDiscoveryConfig::workspace_root(
                    workspace.join(".agents/skills"),
                ),
                mcp_servers: std::collections::BTreeMap::new(),
                base_environment: tool_runtime.environment().clone(),
                environment_store_root: dir.path().join("skill-env"),
            },
        )
        .expect("coordinator");
        let candidate = coordinator.prepare_candidate().await.expect("prepare");
        coordinator.commit(candidate).expect("commit");
        OwnershipFixture {
            _dir: dir,
            adapter,
            tool_runtime,
            coordinator,
        }
    }

    /// The `RuntimeConversationConfig` over one ownership fixture.
    fn claim_config(fixture: &OwnershipFixture) -> RuntimeConversationConfig {
        RuntimeConversationConfig {
            explicit_model: true,
            agent_id: AgentId::new("agent-claim"),
            model: scripted_session_model(fixture.adapter.clone()),
            approval_mode: crate::runtime::ApprovalMode::Policy,
            model_timeout_policy: crate::model::ModelTimeoutPolicy::default(),
            tool_deadline_policy: crate::tools::deadline::ToolExecutionDeadlinePolicy::default(),
            context: ConversationContextConfig {
                policy: crate::context::SessionContextPolicy {
                    reserve_tokens: 0,
                    keep_recent_tokens: 0,
                    summary_output_cap: None,
                },
                estimator: Arc::new(DefaultTokenEstimator),
                status_engine: Some(status_engine()),
            },
            tool_runtime: fixture.tool_runtime.clone(),
            resources: test_resources(&fixture.coordinator),
            resource_loader: test_resource_loader(&fixture.coordinator),
            capability: fixture.coordinator.clone(),
            clock: Some(Arc::new(FixedRuntimeClock)),
            initial_messages: Vec::new(),
            subagents: None,
            workflow_output: None,
        }
    }

    /// A background invocation for the ownership-transfer tests.
    fn claim_background_invocation(call_id: &str) -> ToolInvocation {
        ToolInvocation {
            id: crate::tools::types::ToolInvocationId::Agent {
                call_id: ToolCallId::new(call_id),
            },
            tool_id: ToolId::new("tool-bg"),
            tool_name: "bg".to_owned(),
            mode: ToolInvocationMode::Background,
            arguments: serde_json::json!({}),
        }
    }

    /// The ownership transfer rejects a tool runtime whose background plane
    /// already holds a **committed** standalone execution: typed
    /// `ToolRuntimeNotQuiescent`, no coordinator claim consumed, no
    /// capability claim consumed, the mailbox stays standalone/unbound, and
    /// the detached execution continues under standalone semantics.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn conversation_runtime_claim_rejects_committed_standalone_background_work() {
        let fixture = ownership_fixture(Vec::new(), ToolRegistry::new()).await;
        let registry = fixture.tool_runtime.background().clone();
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = registry
            .prepare_dispatch(
                &claim_background_invocation("call-committed"),
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let outcome = registry
            .commit_dispatch(prepared, &CancellationSignal::new())
            .expect("a standalone commit succeeds");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
            panic!("accepted");
        };
        await_background_started(&mut started, "the standalone runner starts").await;
        assert_eq!(
            registry.all_snapshots().len(),
            1,
            "the standalone execution is committed and running"
        );

        // The ownership transfer is refused typed...
        let refused = ConversationRuntime::new(claim_config(&fixture))
            .expect_err("a tool runtime with committed background work is not claimable");
        assert_eq!(
            refused,
            ConversationRuntimeError::ToolRuntimeNotQuiescent {
                conversation_id: ConversationId::new("conv_29ce21a0-54d0-7f99-88a0-dda036ee7dbc"),
            }
        );

        // ...and consumed nothing: no coordinator claim, no capability
        // claim, and the mailbox remains standalone.
        assert!(
            !fixture.tool_runtime.is_conversation_runtime_bound(),
            "the failed claim consumed no tool-runtime ownership"
        );
        assert!(
            !fixture.coordinator.is_conversation_runtime_bound(),
            "the failed claim consumed no capability ownership"
        );
        fixture
            .tool_runtime
            .mailbox()
            .enqueue(inbound_text("standalone-1", "still standalone"))
            .expect("the mailbox remains standalone/unbound");

        // The detached execution keeps its standalone semantics and
        // settles normally.
        release.send_replace(true);
        let terminal =
            await_background_terminal(&registry, &execution_id, "standalone background execution")
                .await;
        assert_eq!(
            terminal.state,
            BackgroundLifecycle::Succeeded,
            "the standalone execution settles normally"
        );
    }

    /// The ownership transfer rejects a tool runtime with a **prepared but
    /// not committed** dispatch: typed `ToolRuntimeNotQuiescent`, no claim
    /// consumed, the mailbox stays standalone, and the prepared handle
    /// keeps its standalone semantics — dropping it rolls the dispatch back
    /// with no fabricated record. Once the background plane is pristine
    /// again, a fresh construction of the same identity succeeds, proving
    /// the failed claim did not consume the one-time ownership.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn conversation_runtime_claim_rejects_a_prepared_standalone_dispatch() {
        let fixture = ownership_fixture(Vec::new(), ToolRegistry::new()).await;
        let registry = fixture.tool_runtime.background().clone();
        let (tool, started, _release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = registry
            .prepare_dispatch(
                &claim_background_invocation("call-prepared"),
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        assert!(
            registry.all_snapshots().is_empty(),
            "preparation publishes no record"
        );

        // The ownership transfer is refused typed and consumes nothing.
        let refused = ConversationRuntime::new(claim_config(&fixture))
            .expect_err("a staged dispatch makes the background plane non-quiescent");
        assert_eq!(
            refused,
            ConversationRuntimeError::ToolRuntimeNotQuiescent {
                conversation_id: ConversationId::new("conv_29ce21a0-54d0-7f99-88a0-dda036ee7dbc"),
            }
        );
        assert!(!fixture.tool_runtime.is_conversation_runtime_bound());
        assert!(!fixture.coordinator.is_conversation_runtime_bound());
        fixture
            .tool_runtime
            .mailbox()
            .enqueue(inbound_text("standalone-2", "still standalone"))
            .expect("the mailbox remains standalone/unbound");

        // The prepared handle stays valid under standalone semantics:
        // dropping it rolls the dispatch back and fabricates no record.
        drop(prepared);
        assert!(
            registry.all_snapshots().is_empty(),
            "the rolled-back dispatch published no record"
        );
        assert!(!*started.borrow(), "the rolled-back runner never begins");

        // The one-time claim was not consumed by the failed construction:
        // a fresh claim of the same identity succeeds once the background
        // plane is pristine again.
        let runtime = ConversationRuntime::new(claim_config(&fixture))
            .expect("a fresh claim succeeds after the prepared dispatch rolled back");
        assert!(
            fixture.tool_runtime.is_conversation_runtime_bound(),
            "the retried construction owns the identity"
        );
        assert!(!runtime.is_activated(), "the runtime is still inactive");
        runtime.activate();
    }

    /// Interleaving A of the ownership-transfer race: a standalone
    /// background commit parked exactly at its ownership-commit boundary
    /// (holding the registry synchronization lock) beats the racing
    /// `ConversationRuntime::new`. The claim provably linearizes after the
    /// committed record, fails typed `ToolRuntimeNotQuiescent`, and
    /// consumes nothing — the mailbox stays standalone and the detached
    /// execution stays valid.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn background_commit_racing_the_runtime_claim_wins_and_construction_fails_typed() {
        let fixture = ownership_fixture(Vec::new(), ToolRegistry::new()).await;
        let registry = fixture.tool_runtime.background().clone();
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = registry
            .prepare_dispatch(
                &claim_background_invocation("call-race-a"),
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare");
        let hook = Arc::new(crate::tools::background::test_sync::CommitBoundaryHook::default());
        registry.install_commit_boundary_hook(hook.clone());

        // The commit enters its critical section and parks there, holding
        // the registry lock at the ownership-commit boundary.
        let commit_registry = registry.clone();
        let commit_task = tokio::task::spawn_blocking(move || {
            commit_registry.commit_dispatch(prepared, &CancellationSignal::new())
        });
        {
            let hook = hook.clone();
            tokio::task::spawn_blocking(move || hook.wait_entered())
                .await
                .expect("the commit entered the ownership boundary");
        }

        // Race the runtime claim: the rendezvous marker proves the claim
        // thread is in flight while the commit is parked, and because the
        // commit holds the registry lock, the claim's quiescence
        // observation necessarily linearizes *after* the commit's record
        // publication.
        let (marker_tx, marker_rx) = std::sync::mpsc::sync_channel(0);
        let claim_config = claim_config(&fixture);
        let claim_task = tokio::task::spawn_blocking(move || {
            marker_tx.send(()).expect("the claim is in flight");
            ConversationRuntime::new(claim_config)
        });
        marker_rx
            .recv()
            .expect("the claim thread started while the commit was parked");

        // Release the boundary: the commit wins, publishes the record, and
        // only then may the claim acquire the registry lock.
        {
            let hook = hook.clone();
            tokio::task::spawn_blocking(move || hook.proceed())
                .await
                .expect("the commit boundary was released");
        }
        let outcome = commit_task
            .await
            .expect("commit outcome")
            .expect("the standalone commit succeeds");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
            panic!("accepted");
        };
        await_background_started(&mut started, "the standalone runner starts").await;

        let refused = claim_task
            .await
            .expect("claim outcome")
            .expect_err("the claim linearizes after the committed background record");
        assert_eq!(
            refused,
            ConversationRuntimeError::ToolRuntimeNotQuiescent {
                conversation_id: ConversationId::new("conv_29ce21a0-54d0-7f99-88a0-dda036ee7dbc"),
            }
        );
        assert!(!fixture.tool_runtime.is_conversation_runtime_bound());
        assert!(!fixture.coordinator.is_conversation_runtime_bound());
        fixture
            .tool_runtime
            .mailbox()
            .enqueue(inbound_text("standalone-3", "still standalone"))
            .expect("the mailbox remains standalone/unbound");

        release.send_replace(true);
        let terminal = await_background_terminal(
            &registry,
            &execution_id,
            "racing standalone background execution",
        )
        .await;
        assert_eq!(
            terminal.state,
            BackgroundLifecycle::Succeeded,
            "the racing standalone execution settles normally"
        );
    }

    /// Interleaving B of the ownership-transfer race: the runtime ownership
    /// transfer linearizes first, and a background commit that arrives
    /// afterwards fails typed `ConversationInactive` — no record published,
    /// the prepared runner rolls back, and the runtime remains inert until
    /// `activate()`. Ordinary capability commits are independently rejected
    /// because live publication belongs to native configuration coordination; after activation,
    /// a fresh background dispatch commits normally.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn runtime_claim_racing_the_background_commit_wins_and_commit_fails_inactive() {
        let fixture = ownership_fixture(Vec::new(), ToolRegistry::new()).await;
        let registry = fixture.tool_runtime.background().clone();

        // The ownership transfer completes first: the runtime exists,
        // inactive, with its mailbox bound inactive.
        let runtime = ConversationRuntime::new(claim_config(&fixture))
            .expect("the ownership transfer wins the race");
        assert!(
            fixture.tool_runtime.is_conversation_runtime_bound(),
            "the runtime owns the tool runtime identity"
        );
        assert!(
            fixture.coordinator.is_conversation_runtime_bound(),
            "the runtime owns the capability identity"
        );
        assert!(!runtime.is_activated(), "the runtime is still inactive");

        // A background commit that linearizes after the transfer is refused
        // typed: no record, no runner start, the prepared dispatch rolls
        // back completely.
        let (tool, mut started, release) = ParkingBackgroundTool::new();
        let executor: Arc<dyn ToolExecutor> = Arc::new(tool);
        let prepared = registry
            .prepare_dispatch(
                &claim_background_invocation("call-race-b"),
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("preparation is still allowed");
        let refused = registry
            .commit_dispatch(prepared, &CancellationSignal::new())
            .expect_err("a commit after the transfer observes the inactive runtime");
        assert_eq!(
            refused,
            BackgroundDispatchError::ConversationInactive {
                conversation_id: ConversationId::new("conv_29ce21a0-54d0-7f99-88a0-dda036ee7dbc"),
            }
        );
        assert!(
            registry.all_snapshots().is_empty(),
            "the refused commit published no record"
        );
        assert!(!*started.borrow(), "the rolled-back runner never begins");

        // Activation is the single semantic-open boundary: a fresh dispatch
        // commits normally afterwards.
        runtime.activate();
        let prepared = registry
            .prepare_dispatch(
                &claim_background_invocation("call-race-b-2"),
                &executor,
                crate::tools::environment::ToolEnvironment::new(),
            )
            .expect("prepare after activation");
        let outcome = registry
            .commit_dispatch(prepared, &CancellationSignal::new())
            .expect("a post-activation commit succeeds");
        let BackgroundDispatchOutcome::Accepted { execution_id, .. } = outcome else {
            panic!("accepted");
        };
        await_background_started(&mut started, "the post-activation runner starts").await;
        release.send_replace(true);
        let terminal = await_background_terminal(
            &registry,
            &execution_id,
            "post-activation background execution",
        )
        .await;
        assert_eq!(
            terminal.state,
            BackgroundLifecycle::Succeeded,
            "the post-activation execution settles normally"
        );
    }

    /// Transactional construction: when the capability claim fails after
    /// the tool-runtime ownership transfer, the transfer is rolled back to
    /// its exact previous standalone state — the coordinator claim is
    /// cleared and the mailbox is unbound again.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn failed_capability_claim_rolls_back_the_tool_runtime_ownership_transfer() {
        let fixture = ownership_fixture(Vec::new(), ToolRegistry::new()).await;
        // Consume the capability identity's one-time claim *before*
        // construction, so the capability claim inside `new` fails after
        // the tool-runtime transfer already succeeded.
        assert!(
            fixture
                .coordinator
                .claim_conversation_runtime(&crate::runtime::types::ConversationLifecycle::new())
                .is_some()
        );

        let refused = ConversationRuntime::new(claim_config(&fixture))
            .expect_err("a claimed capability identity rejects construction");
        assert_eq!(
            refused,
            ConversationRuntimeError::RuntimeAlreadyBound {
                conversation_id: ConversationId::new("conv_29ce21a0-54d0-7f99-88a0-dda036ee7dbc"),
            }
        );
        assert!(
            !fixture.tool_runtime.is_conversation_runtime_bound(),
            "the failed construction released the tool-runtime claim"
        );
        fixture
            .tool_runtime
            .mailbox()
            .enqueue(inbound_text("standalone-4", "still standalone"))
            .expect("the rolled-back mailbox accepts standalone inbound");
    }

    /// The host-binding vs activation race: `RuntimeClientHost::new` and
    /// `ConversationRuntime::activate` race against the same lifecycle
    /// transition, serialized by the one coordinator lock the transition
    /// commits under. This interleaving proves "host wins": the host
    /// binds while `activate` is parked before the lifecycle transition,
    /// completes with the bootstrap seed at cursor 0, and the transition
    /// then commits — the first cursor belongs to a real post-activation
    /// transition. The activation-wins interleaving is proven by
    /// `late_host_bind_after_activation_is_rejected_typed`.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn host_bind_racing_activation_has_one_clean_linearization() {
        let gate = Arc::new(crate::runtime::conversation_runtime::Gate::default());
        let (_adapter, fixture) = runtime_only_fixture(
            vec![one_turn_stop()],
            ToolRegistry::new(),
            Some(CoordinatorProbe {
                admission_gate: None,
                settlement_gate: None,
                before_restore_gate: None,
                activation_gate: Some(gate.clone()),
                manual_compaction_settlement_gate: None,
                submit_gate: None,
                submit_arrival: None,
                shutdown_arrival: None,
                mcp_failure_drain_gate: None,
                start_boundary_pause: None,
                model_arbitration_pause: None,
                drain_linearization: None,
                tool_start_pause: None,
                drain_supervision: None,
                attempt_exit_gate: None,
                parent_guidance_seal_gate: None,
                background_failure_gate: None,
                subagent_failure_published_gate: None,
                ..CoordinatorProbe::default()
            }),
        )
        .await;
        gate.arm();

        // Park `activate` before the lifecycle transition: the
        // conversation is provably still Inactive, so the host bind wins
        // the race and completes with the full bootstrap seed.
        let runtime = fixture.runtime.clone();
        let activate_task = tokio::task::spawn_blocking(move || runtime.activate());
        {
            let gate = gate.clone();
            tokio::task::spawn_blocking(move || gate.wait_entered())
                .await
                .expect("activate entered the gate");
        }
        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: fixture.runtime.clone(),
            replay_limit: None,
        })
        .expect("the host binds before the lifecycle transition");
        assert!(
            fixture.runtime.tool_runtime().is_runtime_client_bound(),
            "the successful bind consumed the one-time claim"
        );
        let (snapshot, cursor) = host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        assert_eq!(cursor, RuntimeClientCursor::new(0));
        assert!(
            snapshot.jobs.is_empty(),
            "the inert runtime contributes no background seed"
        );

        // Release the transition: activation commits and the one-time
        // post-activation kick runs.
        {
            let gate = gate.clone();
            tokio::task::spawn_blocking(move || gate.release())
                .await
                .expect("the activation gate was released");
        }
        activate_task.await.expect("activate completes");
        assert!(fixture.runtime.is_activated());

        // The first cursor belongs to a real post-activation transition.
        let (attachment, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe from the bootstrap cursor");
        host.submit_inbound(submit_content("first transition"))
            .expect("accepted");
        let events = receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::AttemptSettled { .. }))
        })
        .await;
        assert_eq!(
            events[0].cursor,
            RuntimeClientCursor::new(1),
            "the first cursor is a real post-activation transition"
        );
    }

    /// Concurrent `activate` calls are idempotent: exactly one call
    /// commits the lifecycle transition (`Inactive -> Running` CAS) and
    /// performs the one-time post-transition work — worker spawn and the
    /// admission kick — and every other call observes `Running` and returns
    /// without changing anything. A single inbound item therefore admits
    /// exactly one attempt, never two.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn concurrent_activation_is_idempotent_and_creates_one_worker() {
        let (adapter, fixture) =
            runtime_only_fixture(vec![one_turn_stop()], ToolRegistry::new(), None).await;
        let runtime = fixture.runtime.clone();
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let a = {
            let runtime = runtime.clone();
            let barrier = barrier.clone();
            tokio::task::spawn_blocking(move || {
                barrier.wait();
                runtime.activate();
            })
        };
        let b = {
            let runtime = runtime.clone();
            let barrier = barrier.clone();
            tokio::task::spawn_blocking(move || {
                barrier.wait();
                runtime.activate();
            })
        };
        barrier.wait();
        a.await.expect("activate a");
        b.await.expect("activate b");
        assert!(fixture.runtime.is_activated());

        // One inbound item admits exactly one attempt: a duplicated
        // activation kick can never admit a second attempt from one item,
        // and a duplicated worker is structurally impossible (one CAS
        // winner, one `worker_started` guard).
        // Armed before the trigger: settlement wakes only registered waiters.
        let settled = fixture.runtime.settlement_signal().notified();
        tokio::pin!(settled);
        settled.as_mut().enable();
        fixture
            .runtime
            .submit_inbound(submit_content("one item"))
            .expect("accepted");
        settled.await;
        assert_eq!(
            adapter.requests().len(),
            1,
            "exactly one attempt from one activation epoch"
        );
    }

    /// Test A — a model mutation while the runtime is inactive is rejected
    /// typed and consumes nothing: the model is unchanged, the host
    /// snapshot stays at cursor 0, and no `SessionModelChanged` event
    /// exists. After activation the same update succeeds and is delivered
    /// exactly once with the first real cursor.
    #[allow(clippy::too_many_lines)]
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn model_set_while_inactive_is_rejected_and_consumes_nothing() {
        let (_adapter, fixture) =
            runtime_only_fixture(vec![one_turn_stop()], ToolRegistry::new(), None).await;
        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: fixture.runtime.clone(),
            replay_limit: None,
        })
        .expect("host construction");

        // The live mutation is refused typed while inactive...
        let refused = fixture
            .runtime
            .model_set(marked_model_config(&fixture.runtime, "early"))
            .expect_err("a model update while inactive is refused");
        assert_eq!(refused, ModelUpdateError::Inactive);

        // ...consumes nothing: the model is unchanged, the snapshot is the
        // bootstrap state at cursor 0, and no event exists.
        let (snapshot, cursor) = host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        assert_eq!(cursor, RuntimeClientCursor::new(0));
        assert!(
            snapshot
                .model
                .as_ref()
                .unwrap()
                .configured
                .request_params
                .get("early")
                .is_none(),
            "the rejected update left the model unchanged"
        );
        let (attachment, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");
        assert!(
            matches!(subscription.try_next(), EventDelivery::Pending),
            "the rejected update published no event"
        );

        // After activation the same update succeeds and receives the first
        // real cursor, exactly once.
        fixture.runtime.activate();
        fixture
            .runtime
            .model_set(marked_model_config(&fixture.runtime, "early"))
            .expect("the update succeeds after activation");
        let events = receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::SessionModelChanged { .. }))
        })
        .await;
        assert_eq!(
            events[0].cursor,
            RuntimeClientCursor::new(1),
            "the first cursor belongs to the real post-activation transition"
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| event
                    .event
                    .facts()
                    .iter()
                    .any(|fact| matches!(fact, RuntimeClientEvent::SessionModelChanged { .. })))
                .count(),
            1,
            "the post-activation update is delivered exactly once"
        );
    }

    /// Test B — a shutdown while the runtime is inactive is rejected typed
    /// and is non-semantic: the runtime is not marked shutting down, the
    /// snapshot stays at cursor 0, and no `RuntimeShutdown` event exists.
    /// After activation shutdown retains the existing runtime semantics.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn shutdown_while_inactive_is_rejected_and_consumes_nothing() {
        let (_adapter, fixture) =
            runtime_only_fixture(vec![one_turn_stop()], ToolRegistry::new(), None).await;
        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: fixture.runtime.clone(),
            replay_limit: None,
        })
        .expect("host construction");

        assert_eq!(
            fixture.runtime.shutdown().await,
            Err(crate::runtime::conversation_runtime::ShutdownError::Inactive),
            "a shutdown while inactive is refused typed"
        );
        let (snapshot, cursor) = host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        assert_eq!(cursor, RuntimeClientCursor::new(0));
        assert!(
            !snapshot.shutting_down,
            "the refused shutdown never marked the runtime shutting down"
        );
        let (attachment, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe");
        assert!(
            matches!(subscription.try_next(), EventDelivery::Pending),
            "the refused shutdown published no event"
        );

        // After activation shutdown completes its one semantic drain,
        // publishes the drain observation exactly once, and gates inbound
        // afterwards.
        fixture.runtime.activate();
        fixture
            .runtime
            .shutdown()
            .await
            .expect("accepted after activation");
        let events = receive_until(&subscription, |event| {
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::RuntimeShutdown))
        })
        .await;
        assert_eq!(
            events
                .iter()
                .filter(|event| event
                    .event
                    .facts()
                    .iter()
                    .any(|fact| matches!(fact, RuntimeClientEvent::RuntimeShutdown)))
                .count(),
            1,
            "the post-activation shutdown publishes exactly one event"
        );
        assert_eq!(
            fixture.runtime.submit_inbound(submit_content("late")),
            Err(InboundAdmissionError::Shutdown)
        );
    }

    /// A model update that linearizes after activation — that is, after
    /// the bootstrap cut — arrives through the live observation stream
    /// exactly once and is never lost.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn post_activation_model_transition_is_delivered_exactly_once() {
        let (_adapter, fixture) =
            runtime_only_fixture(vec![one_turn_stop()], ToolRegistry::new(), None).await;
        let host = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: fixture.runtime.clone(),
            replay_limit: None,
        })
        .expect("host construction");
        fixture.runtime.activate();

        let (attachment, _) = host
            .attach(crate::runtime_client::RUNTIME_CLIENT_PROTOCOL_VERSION)
            .expect("attach");
        let subscription = attachment
            .subscribe_events(RuntimeClientCursor::new(0))
            .expect("subscribe from the bootstrap cursor");

        fixture
            .runtime
            .model_set(marked_model_config(&fixture.runtime, "after-cut"))
            .expect("model transition after the cut");

        let (snapshot, _cursor) = host
            .checkpoint()
            .map(|checkpoint| (checkpoint.snapshot, checkpoint.cursor))
            .expect("snapshot");
        assert_eq!(
            snapshot
                .model
                .as_ref()
                .unwrap()
                .configured
                .request_params
                .get("after-cut"),
            Some(&serde_json::json!("changed")),
            "the post-cut transition must be visible in the projection"
        );
        let mut session_model_events = 0;
        receive_until(&subscription, |event| {
            if event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::SessionModelChanged { .. }))
            {
                session_model_events += 1;
            }
            event
                .event
                .facts()
                .iter()
                .any(|fact| matches!(fact, RuntimeClientEvent::SessionModelChanged { .. }))
        })
        .await;
        assert_eq!(
            session_model_events, 1,
            "the post-cut transition is delivered exactly once"
        );
    }

    /// Failed host construction never leaves a claimed-but-invalid
    /// binding: when the observation bridge handshake fails (a previous
    /// headless bridge exists), the one-time client binding claim is
    /// released again and the failure is typed.
    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn failed_host_construction_releases_the_client_binding() {
        let (_adapter, fixture) = runtime_only_fixture(Vec::new(), ToolRegistry::new(), None).await;
        // A headless observation bridge already exists over the runtime.
        let queue = Arc::new(crate::runtime::observation::PendingObservations::new());
        fixture
            .runtime
            .install_observation_bridge(queue)
            .expect("headless bridge");

        let rejected = RuntimeClientHost::new(RuntimeClientHostConfig {
            runtime: fixture.runtime.clone(),
            replay_limit: None,
        });
        match rejected {
            Err(HostConstructionError::ObservationBridgeAlreadyInstalled { conversation_id }) => {
                assert_eq!(
                    conversation_id.as_str(),
                    "conv_7d0e433c-438a-7d1c-8bf7-e3e68527161b"
                );
            }
            _ => panic!("the bridge conflict must fail typed"),
        }
        // The failed construction released the binding claim: no
        // claimed-but-invalid binding remains.
        assert!(
            !fixture.runtime.tool_runtime().is_runtime_client_bound(),
            "the tool runtime binding was released"
        );
        assert!(
            !fixture.runtime.capability().is_runtime_client_bound(),
            "the capability binding was released"
        );
    }
}
