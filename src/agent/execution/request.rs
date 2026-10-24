//! Preparation of one actual model request: Context Assembly admission,
//! pre-step policy, carryover and media freezing, staging, fit validation,
//! and the frozen Request Snapshot and provider-neutral request. Nothing in
//! this module commits; the prepared turn becomes durable only through the
//! start arbitration in the sibling `start` module.

#[cfg(test)]
use super::attribution;
use super::start::RequestObservationIdentity;
use super::terminal::Terminal;
use super::{AgentExecution, DurableFailureKind};
use crate::agent::lifecycle::{PreStepBatch, PreStepDecision};
use crate::context::error::{ContextError, ContextErrorKind};
use crate::context::projection::ContextProjection;
use crate::context::tokens::ObservedAnchor;
use crate::context::{
    AcceptedContext, ContributionOpportunities, ContributorInputSnapshot, FreshInboundOpportunity,
    render_effective_system_prompt,
};
use crate::conversation::PreparedCanonicalCommit;
use crate::events::types::AttemptFailure;
use crate::message::types::MessageBlock;
use crate::model::snapshot::{RequestIdentity, RequestSnapshot};
use crate::model::types::ModelRequest;
use crate::model::{
    CarryoverDetailLevel, ModelInputMessage, RenderedUnresolvedOutputCarryover,
    RequestOnlyInsertionAnchor,
};
use crate::runtime::identity::{MessageId, ToolId, TurnId};
use crate::runtime::types::RuntimeError;
use std::sync::Arc;

/// The fully prepared, not-yet-started model turn (Issue #12, M9b).
///
/// Every fallible input of one actual model request is resolved: the
/// request-scoped context is validated against the conversation state, the
/// Effective System Prompt, the projection, the frozen Request Snapshot,
/// and the exact provider-neutral request are computed from the staged
/// view, and any compaction the request requires already committed as its
/// own independent canonical transition. Nothing in this value is durable
/// or provider-visible yet: it becomes observable only through the one
/// cancellation-vs-start arbitration in [`AgentExecution::start_model_turn`],
/// and is discarded without a trace when cancellation wins there.
pub(super) struct PreparedModelTurn {
    #[cfg(feature = "issue-387-profile")]
    pub(super) adapter_input_profile: crate::model::adapter::profile::InputLayout,
    /// The validated, not-yet-committed request-scoped context commits, in
    /// canonical append order.
    pub(super) context: Vec<PreparedCanonicalCommit>,
    /// The frozen snapshot of this actual request.
    pub(super) snapshot: RequestSnapshot,
    /// The exact provider-neutral request of the staged projection.
    pub(super) request: ModelRequest,
    /// The staged projection's structural anchor, used to reuse a later
    /// provider-observed measurement for every request context this one is a
    /// prefix of.
    pub(super) anchor: ObservedAnchor,
    /// The identity of the non-conversation request state this request
    /// carries: its resolved invocation configuration and its provider
    /// continuation. A measurement is only reusable while this is unchanged.
    pub(super) request_identity: Arc<RequestObservationIdentity>,
    /// The runtime's deterministic input estimate for exactly this request.
    ///
    /// When the provider rejects the request as oversized, this is the
    /// estimate that was proven wrong, and the pair (estimate, provider
    /// count) is the correction the recovery compaction plans with.
    pub(super) estimated_input: u64,
}

/// Working set of the existing execution owner; never an execution state machine.
#[derive(Clone)]
pub(super) struct FrozenLogicalInput {
    #[cfg(feature = "issue-387-profile")]
    pub(super) adapter_input_profile: crate::model::adapter::profile::InputLayout,
    pub(super) snapshot: RequestSnapshot,
    pub(super) request: ModelRequest,
    pub(super) anchor: ObservedAnchor,
    pub(super) request_identity: Arc<RequestObservationIdentity>,
    pub(super) estimated_input: u64,
}

/// The intermediate staged view of one model turn: scratch conversation,
/// Effective System Prompt, projection, snapshot metadata, and request — all
/// transient, nothing committed.
pub(super) struct StagedModelTurn {
    #[cfg(feature = "issue-387-profile")]
    adapter_input_profile: crate::model::adapter::profile::InputLayout,
    /// The staged projection (over the scratch conversation that includes
    /// the not-yet-committed request-scoped context).
    projection: ContextProjection,
    /// The staged Effective System Prompt.
    effective_system_prompt: String,
    /// Snapshot metadata; exact input is frozen only after final preflight.
    snapshot: RequestSnapshot,
    /// The exact provider-neutral request of the staged projection.
    request: ModelRequest,
    /// Native selection root carried through the actual request mutations.
    input_manifest: crate::model::SelectedInputManifest,
    /// Coverage reconciliation is committed to hot state only after preflight.
    retire_continuation: bool,
    /// The deterministic estimate of the exact mixed request input.
    estimated_input: u64,
}

/// Process-local frozen semantics of the one carryover source admitted by a
/// logical primary step. The source and full bounded projection are captured
/// once; actual retries only reuse or monotonically degrade this value.
#[derive(Clone)]
pub(super) struct FrozenCarryover {
    pub(super) full: RenderedUnresolvedOutputCarryover,
    pub(super) level: CarryoverDetailLevel,
    pub(super) anchor: Option<RequestOnlyInsertionAnchor>,
}

impl FrozenCarryover {
    pub(super) fn admitted(&self) -> Option<RenderedUnresolvedOutputCarryover> {
        match self.level {
            CarryoverDetailLevel::Omitted => None,
            level => Some(self.full.degraded(level)),
        }
    }

    pub(super) fn degrade(&mut self) -> bool {
        let next = match self.level {
            CarryoverDetailLevel::Full => CarryoverDetailLevel::Reduced,
            CarryoverDetailLevel::Reduced => CarryoverDetailLevel::MetadataOnly,
            CarryoverDetailLevel::MetadataOnly => CarryoverDetailLevel::Omitted,
            CarryoverDetailLevel::Omitted => return false,
        };
        self.level = next;
        true
    }
}

impl AgentExecution<'_> {
    /// Prepares the next model turn without committing anything (Issue #12,
    /// M9b).
    ///
    /// This is the fallible preparation half of every primary model request:
    /// the current Surface flows through Context Assembly and the context
    /// engine into a staged projection, and the staged view is compiled into
    /// the frozen Request Snapshot and the exact provider-neutral request.
    /// The pending fresh inbound turn (when one exists) is sampled into one
    /// native context fact for the admitted primary step, together with the
    /// deferred post-tool proposals staged by the previous structurally
    /// settled tool batch. That accepted generation is reused throughout
    /// transient replay, proactive compaction, and overflow retry.
    ///
    /// The exact structure is:
    ///
    /// ```text
    /// Context Assembly (native + extension + deferred post-tool)
    ///     ↓ final immutable AcceptedContext
    /// PreStepPolicy → Enter | Reject(reason)
    ///     ↓
    /// stage request-scoped context (validate only — nothing commits)
    ///     ↓
    /// staged Effective System Prompt → staged projection
    ///     ↓ (reservation-aware compaction when the staged input overflows)
    /// PreparedModelTurn: validated context commits + RequestSnapshot + request
    /// ```
    ///
    /// A rejection, a policy failure, or a preparation failure leaves no
    /// canonical context, no Surface advancement, no frozen snapshot, and no
    /// provider request. Nothing request-scoped commits here: the one
    /// cancellation-vs-start arbitration in [`AgentExecution::start_model_turn`]
    /// decides whether this prepared turn ever starts. A recovery request
    /// never re-enters this block: it reuses the already-admitted context
    /// generation, so the policy is evaluated exactly once per primary step.
    ///
    /// The generic cancellation check at the top is a cheap fast path that
    /// avoids wasted preparation work; it is **not** the cancellation
    /// arbitration (a cancellation that lands after it is decided by the
    /// start gate, exactly like one landing during assembly).
    #[allow(clippy::too_many_lines)]
    pub(super) async fn prepare_model_turn(
        &mut self,
        request_ordinal: u32,
    ) -> Result<PreparedModelTurn, Terminal> {
        #[cfg(test)]
        let _profile = attribution::Timer::new(attribution::Phase::Prepare);
        if let Some(message) = self.durable_failure.clone() {
            return Err(Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::DurableStore { message },
                },
            });
        }
        if self.cancellation.is_cancelled() {
            return Err(Terminal::Cancelled {
                reason: self.cancellation.reason(),
            });
        }
        if self.logical_step.accepted_context.is_some() {
            return Err(Self::context_failure_terminal(&ContextError::new(
                ContextErrorKind::Internal,
                "a primary model turn is prepared exactly once per turn",
            )));
        }
        self.freeze_pending_carryover()?;
        // Compact already-committed history before staging this step's
        // dynamic context. This keeps an unobserved fresh inbound batch and
        // its newly staged Runtime fact from blocking a
        // complete-message compaction candidate that belongs to older
        // history. A second fit check below still rejects a request whose
        // newly staged context cannot fit on its own. This compaction is
        // independent conversation-history maintenance with its own
        // canonical commit; it reserves no staged tokens.
        self.freeze_model_media(&[]).await?;
        let baseline_prompt = self.effective_system_prompt()?;
        let baseline_projection = self.current_projection(&baseline_prompt)?;
        if self
            .context_runtime
            .engine
            .should_compact(
                &baseline_projection,
                self.compaction_budgets().primary_output_budget,
            )
            .map_err(|error| Self::context_failure_terminal(&error))?
        {
            let must_cover = self.continuation_owner.clone();
            let fresh = self.pending_fresh_inbound.clone();
            self.perform_compaction(
                must_cover.as_ref(),
                fresh.as_ref(),
                &baseline_prompt,
                None,
                &[],
                None,
            )
            .await?;
        }
        // The pending fresh inbound turn's canonical consistency is an
        // Agent Loop contract, checked before and independently of the
        // launch's native Agent Extension composition (Issue #256): an
        // inconsistent execution state must fail the same way whether or
        // not this runtime composes Agent Status.
        if let Err(error) = self.validate_pending_fresh_inbound() {
            return Err(Self::context_failure_terminal(&error));
        }
        // Native inbound sequence order is retained by FreshInboundTurn.
        // The most recent Human Message replaces the current request's unused
        // Goal-create authorization. No Human in this batch leaves it intact:
        // ordinary steps and Runtime input do not end the Human request.
        if let Some(origin) = self.pending_fresh_inbound.as_ref().and_then(|fresh| {
            fresh.message_ids().iter().rev().find_map(|id| {
                match self.conversation.ledger().get(id) {
                    Some(MessageBlock::User(user))
                        if user.source == crate::message::UserSource::Human
                            && user.kind == crate::message::InboundKind::Message =>
                    {
                        Some(crate::goal::GoalOrigin::HumanAttempt {
                            message_id: id.clone(),
                            attempt_id: self.request.attempt_id.clone(),
                        })
                    }
                    _ => None,
                }
            })
        }) {
            if let crate::goal::GoalOrigin::HumanAttempt {
                message_id,
                attempt_id,
            } = &origin
            {
                *self
                    .schedule_authorization
                    .get_mut()
                    .expect("Schedule authorization") =
                    Some(crate::scheduler::control::ScheduleHumanGrant {
                        message_id: message_id.clone(),
                        original_attempt_id: attempt_id.clone(),
                    });
            }
            *self
                .goal_creation_authorization
                .get_mut()
                .expect("Goal authorization mutex poisoned") = Some(origin);
        }
        let input = match self.contributor_input_snapshot() {
            Ok(input) => input,
            Err(terminal) => return Err(terminal),
        };
        // The deferred proposals of the previous structurally settled tool
        // batch enter the *same* final transient batch as every other
        // proposal, and are laned and given provenance by their producer
        // identity, not by their timing. There is exactly one
        // model-visible dynamic-context admission path: an observer has no
        // privileged committer role and cannot bypass the policy below.
        let deferred = core::mem::take(&mut self.deferred_context);
        let native = self.context_runtime.native_system.clone();
        self.pending_post_tool_batch = None;
        let accepted = self
            .context_runtime
            .assembly
            .assemble(&input, &native, &deferred)
            .await
            .map_err(|error| {
                Self::context_failure_terminal(&ContextError::new(
                    ContextErrorKind::Internal,
                    error.to_string(),
                ))
            })?;
        // The typed pre-step policy boundary (Issue #56). It observes the
        // complete final proposal batch and returns Enter/Reject only. It
        // owns no cancellation, allocates no identity, and commits nothing;
        // a rejection settles the attempt strictly before the start
        // arbitration in `start_model_turn`.
        let policy = self.lifecycle.pre_step_policy();
        let decision = {
            let batch = PreStepBatch {
                attempt_id: &self.request.attempt_id,
                conversation_id: &self.request.conversation_id,
                turn: self.turn,
                surface_revision: self.conversation.revision(),
                context: &accepted,
            };
            policy.evaluate(&batch).await
        };
        match decision {
            Ok(PreStepDecision::Enter) => {}
            Ok(PreStepDecision::Reject { reason }) => {
                return Err(Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::PreStepRejected { reason },
                    },
                });
            }
            Err(error) => {
                return Err(Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::PreStepPolicyFailed {
                            message: error.message,
                        },
                    },
                });
            }
        }
        // Stage the request-scoped context: generation, canonical
        // identities, and in-memory validation only. Nothing commits until
        // the start arbitration wins.
        let staged_context = self.stage_context(accepted)?;
        self.logical_step.context_ids = staged_context.iter().map(|m| m.id().clone()).collect();
        self.freeze_model_media(&staged_context).await?;
        let mut staged = self.stage_model_turn(request_ordinal, &staged_context)?;
        let budgets = self.compaction_budgets();
        let should_compact = match self
            .context_runtime
            .engine
            .should_compact(&staged.projection, budgets.primary_output_budget)
        {
            Ok(value) => value,
            Err(error) => return Err(Self::context_failure_terminal(&error)),
        };
        if should_compact {
            // The staged context is not yet canonical, so this compaction
            // plans over the committed Surface and evaluates every candidate
            // against the exact hypothetical post-compaction request: the
            // rewritten Surface plus the staged request-scoped context plus
            // the Effective System Prompt plus tools. The rewrite must leave
            // room for the context that the start transaction will append on
            // top of it. The compaction itself is an independent canonical
            // commit of conversation-history maintenance; if cancellation
            // wins the start arbitration afterwards, the compaction remains
            // valid and the staged context is discarded without a trace.
            let must_cover = self.continuation_owner.clone();
            let fresh = self.pending_fresh_inbound.clone();
            // `perform_compaction` owns the post-surface-rewrite
            // invalidation of the opaque provider continuation; no caller
            // clears it a second time.
            self.perform_compaction(
                must_cover.as_ref(),
                fresh.as_ref(),
                &staged.effective_system_prompt,
                None,
                &staged_context,
                None,
            )
            .await?;
            // Restage over the rewritten Surface: the staged context blocks
            // are unchanged, the Surface revision and projection are not.
            staged = self.stage_model_turn(request_ordinal, &staged_context)?;
        }
        // Carryover is auxiliary continuity context. If the canonical
        // projection is runnable, reduce only this request-only value until
        // the exact mixed request is runnable; it never creates a compaction
        // trigger and never turns a canonical fit into `CannotFit`.
        let soft_limit = self
            .context_runtime
            .engine
            .soft_input_limit(budgets.primary_output_budget)
            .map_err(|error| Self::context_failure_terminal(&error))?;
        while staged.estimated_input >= soft_limit {
            let canonical_fits = staged.projection.estimated_input.input_tokens < soft_limit;
            let Some(carryover) = self.logical_step.frozen_carryover.as_mut() else {
                if canonical_fits {
                    return Err(Self::context_failure_terminal(&ContextError::new(
                        ContextErrorKind::CannotFit,
                        "the staged model turn exceeds the soft input limit",
                    )));
                }
                return Err(Self::context_failure_terminal(&ContextError::new(
                    ContextErrorKind::CannotFit,
                    "the staged model turn still exceeds the soft input limit after compaction",
                )));
            };
            if !carryover.degrade() {
                return Err(Self::context_failure_terminal(&ContextError::new(
                    ContextErrorKind::CannotFit,
                    if canonical_fits {
                        "request-only carryover exhausted its fit degradation ladder"
                    } else {
                        "the staged model turn still exceeds the soft input limit after compaction"
                    },
                )));
            }
            staged = self.stage_model_turn(request_ordinal, &staged_context)?;
        }
        self.finalize_model_turn(&staged_context, staged)
    }

    /// Freezes the pending source identity's bounded Publication Audit once
    /// for this logical step. The audit is the sole body authority; the
    /// process-local copy is only the immutable value later frozen into each
    /// request snapshot of this step.
    fn freeze_pending_carryover(&mut self) -> Result<(), Terminal> {
        let source = self
            .store
            .load_pending_unresolved_output_stream_id()
            .map_err(|error| {
                self.durable_failure_kind = Some(DurableFailureKind::Publication);
                self.durable_failure = Some(format!(
                    "pending unresolved-output carryover could not be loaded: {error}"
                ));
                Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::DurableStore {
                            message: format!(
                                "pending unresolved-output carryover could not be loaded: {error}"
                            ),
                        },
                    },
                }
            })?;
        let Some(source) = source else {
            return Ok(());
        };
        let audit = self
            .store
            .load_publication_audit(&source)
            .map_err(|error| {
                self.durable_failure_kind = Some(DurableFailureKind::Publication);
                self.durable_failure = Some(format!(
                    "pending unresolved-output carryover audit could not be loaded: {error}"
                ));
                Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::DurableStore {
                            message: format!(
                                "pending unresolved-output carryover audit could not be loaded: {error}"
                            ),
                        },
                    },
                }
            })?
            .ok_or_else(|| {
                self.durable_failure_kind = Some(DurableFailureKind::Publication);
                self.durable_failure = Some(format!(
                    "pending unresolved-output carryover source {source} has no Publication Audit"
                ));
                Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::DurableStore {
                            message: format!(
                                "pending unresolved-output carryover source {source} has no Publication Audit"
                            ),
                        },
                    },
                }
            })?;
        let Some(full) = crate::publication::render_unresolved_output_carryover(&audit) else {
            self.durable_failure_kind = Some(DurableFailureKind::Publication);
            let message = format!(
                "pending unresolved-output carryover source {source} has no meaningful renderable audit content"
            );
            self.durable_failure = Some(message.clone());
            return Err(Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::DurableStore { message },
                },
            });
        };
        if full.source_stream_id != source {
            let message = format!(
                "pending unresolved-output carryover source {source} disagrees with its audit identity {}",
                full.source_stream_id
            );
            self.durable_failure_kind = Some(DurableFailureKind::Publication);
            self.durable_failure = Some(message.clone());
            return Err(Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::DurableStore { message },
                },
            });
        }
        self.logical_step.frozen_carryover =
            Some(FrozenCarryover {
                full,
                level: CarryoverDetailLevel::Full,
                // The fixed native anchor is known before dynamic context is
                // staged. Fresh inbound is the only eligible message-relative
                // anchor; a continuation always places carryover after the
                // existing canonical projection and before newly admitted
                // request context.
                anchor: Some(self.pending_fresh_inbound.as_ref().map_or(
                    RequestOnlyInsertionAnchor::AfterCanonical,
                    |fresh| {
                        RequestOnlyInsertionAnchor::BeforeMessage(fresh.message_ids()[0].clone())
                    },
                )),
            });
        Ok(())
    }

    /// Validates the pending fresh inbound turn against canonical history.
    ///
    /// This is an ordinary Agent Loop contract, not an extension concern:
    /// the referenced messages must exist in the active Surface, be inbound
    /// User messages, carry persisted timestamps, and appear in canonical
    /// order. It is deliberately checked outside
    /// contribution preparation so an inconsistent execution state fails
    /// identically whether or not this launch composed the Agent Status
    /// extension (Issue #256).
    ///
    /// # Errors
    ///
    /// Returns a `MalformedHistory` context error naming the violation.
    fn validate_pending_fresh_inbound(&self) -> Result<(), ContextError> {
        let Some(fresh) = self.pending_fresh_inbound.as_ref() else {
            return Ok(());
        };
        let active = self
            .claimed_model_input(fresh.message_ids())
            .map_err(|error| {
                ContextError::new(ContextErrorKind::MalformedHistory, error.to_string())
            })?;
        fresh.validate_against(&active).map_err(|error| {
            ContextError::new(
                ContextErrorKind::MalformedHistory,
                format!("pending fresh inbound turn is inconsistent: {error}"),
            )
        })
    }

    async fn freeze_model_media(
        &mut self,
        staged_context: &[MessageBlock],
    ) -> Result<(), Terminal> {
        let history = self.store.projected_history().map_err(|error| {
            Self::context_failure_terminal(&ContextError::new(
                ContextErrorKind::MalformedHistory,
                error.to_string(),
            ))
        })?;
        let anchor = self
            .logical_step
            .frozen_carryover
            .as_ref()
            .and_then(|c| c.anchor.as_ref());
        let carryover = self
            .logical_step
            .frozen_carryover
            .as_ref()
            .filter(|_| anchor.is_some())
            .and_then(FrozenCarryover::admitted);
        let (complete, _) = crate::model::input::assemble_selected_model_input(
            &self.request.conversation_id,
            &history.messages,
            &history.input_manifest,
            staged_context,
            carryover.as_ref(),
            anchor,
        )
        .map_err(|e| {
            Self::context_failure_terminal(&ContextError::new(
                ContextErrorKind::MalformedHistory,
                e,
            ))
        })?;
        self.context_runtime
            .engine
            .prepare_media(&history.messages, &complete, &self.cancellation.signal())
            .await
            .map_err(|e| {
                if e.kind == ContextErrorKind::Cancelled {
                    self.cancelled_terminal()
                } else {
                    Self::context_failure_terminal(&e)
                }
            })
    }

    /// Builds the staged view of one actual model request: a scratch
    /// conversation (the current durable head plus the not-yet-committed
    /// request-scoped context), its Effective System Prompt, the staged
    /// projection, the frozen Request Snapshot, and the exact
    /// provider-neutral request.
    ///
    /// Everything produced here is transient preparation: the scratch
    /// conversation shares no state with the canonical conversation, and
    /// nothing commits until the start arbitration wins.
    #[allow(clippy::too_many_lines)] // keeps the request assembly boundary explicit
    pub(super) fn stage_model_turn(
        &mut self,
        retry_number: u32,
        staged_context: &[MessageBlock],
    ) -> Result<StagedModelTurn, Terminal> {
        let history = self.store.projected_history().map_err(|error| {
            Self::context_failure_terminal(&ContextError::new(
                ContextErrorKind::MalformedHistory,
                error.to_string(),
            ))
        })?;
        let active = &history.messages;
        let carryover_anchor = self
            .logical_step
            .frozen_carryover
            .as_ref()
            .and_then(|carryover| carryover.anchor.as_ref());
        let carryover = self
            .logical_step
            .frozen_carryover
            .as_ref()
            .filter(|_| carryover_anchor.is_some())
            .and_then(FrozenCarryover::admitted);
        let sections = self
            .logical_step
            .accepted_context
            .as_ref()
            .map_or(&[][..], |accepted| accepted.system_sections.as_slice());
        let effective_system_prompt = render_effective_system_prompt(sections);
        let mut revision = self.conversation.revision();
        for _ in staged_context {
            revision = revision.next();
        }
        let mut projected = history.input_manifest.clone();
        for message in staged_context {
            projected = projected.append(
                &self.request.conversation_id,
                &ModelInputMessage::Canonical(message.clone()),
            );
        }
        let projected = projected.messages();
        let projection = self.context_runtime.engine.build_projection_input(
            revision,
            projected,
            &self.tool_registry().model_definitions(),
            self.reusable_observation(),
            &effective_system_prompt,
        );
        let (mut request_messages, mut input_manifest) =
            crate::model::input::assemble_selected_model_input(
                &self.request.conversation_id,
                active,
                &history.input_manifest,
                staged_context,
                carryover.as_ref(),
                carryover_anchor,
            )
            .map_err(|error| {
                Self::context_failure_terminal(&ContextError::new(
                    ContextErrorKind::Internal,
                    format!("request-only context cannot be assembled: {error}"),
                ))
            })?;
        let (images, changed) = self
            .context_runtime
            .engine
            .materialize_selected_media(&mut request_messages, &input_manifest)
            .map_err(|error| Self::context_failure_terminal(&error))?;
        for index in changed {
            input_manifest = input_manifest.replace(
                &self.request.conversation_id,
                index,
                &request_messages[index],
            );
        }
        let mut request = self.model_request_from_projection(&projection, request_messages);
        request.images = images;
        let compatibility_failure = |message: String| Terminal::Failed {
            failure: AttemptFailure::Runtime {
                error: RuntimeError::Unsupported {
                    message: format!("native model input compatibility: {message}"),
                },
            },
        };
        let retire_continuation = request
            .reconcile_continuation()
            .map_err(compatibility_failure)?;
        let accepted = self.logical_step.accepted_context.as_ref().ok_or_else(|| {
            Self::context_failure_terminal(&ContextError::new(
                ContextErrorKind::Internal,
                "model request built before context staging",
            ))
        })?;
        let primary = self.request.model.primary();
        let mut snapshot = RequestSnapshot::new(
            RequestIdentity {
                attempt_id: self.request.attempt_id.clone(),
                turn: TurnId::new(self.turn.to_string()),
                retry_number,
            },
            crate::context::resources::LedgerFrontier((projection.surface_revision).get()),
            // `model_request_from_projection` may append request-scoped
            // protocol feedback (for example, a rejected workflow_output).
            // The snapshot must freeze the exact prompt sent to the provider,
            // not the pre-feedback projection prompt, so durable
            // reconstruction remains byte-for-byte identical on the next
            // turn.
            request.effective_system_prompt.clone(),
            accepted.system_sections.clone(),
            self.context_runtime.resource_revision,
            request.invocation.clone(),
            primary.context_window(),
            primary.profile().cloned(),
            primary.reasoning_enabled(),
            request.tools.clone(),
            self.capability.snapshot().revision(),
            accepted.generation.clone(),
            request.continuation.clone(),
            staged_context
                .iter()
                .map(crate::conversation::message_id_of)
                .collect(),
        );
        snapshot.history_cut = history.cut.clone();
        if let Some(frozen_carryover) = self.logical_step.frozen_carryover.as_ref() {
            snapshot.unresolved_output_carryover_source =
                Some(frozen_carryover.full.source_stream_id.clone());
            snapshot.unresolved_output_carryover = carryover;
            snapshot
                .unresolved_output_carryover_anchor
                .clone_from(&frozen_carryover.anchor);
        }
        // Recovery stages no new Context: the first start already committed
        // this logical step's accepted records. Retain their frozen receipts
        // together with that input instead of treating an empty append delta
        // as an empty contribution set.
        snapshot.contributions = self.logical_step.current_input.as_ref().map_or_else(
            || accepted.request_contributions(staged_context),
            |input| input.snapshot.contributions.clone(),
        );
        let inserted_at = request
            .messages
            .has_request_only()
            .then(|| {
                request
                    .messages
                    .iter()
                    .position(|message| matches!(message, ModelInputMessage::RequestOnly(_)))
            })
            .flatten();
        snapshot.tool_associations = crate::model::occurrence::freeze_associations(
            &history,
            &snapshot.request_id,
            &request,
            inserted_at,
        )
        .map_err(|error| {
            Self::context_failure_terminal(&ContextError::new(
                ContextErrorKind::MalformedHistory,
                error,
            ))
        })?;
        let estimated_input = self
            .context_runtime
            .engine
            .estimate_frozen_request(&request);
        Ok(StagedModelTurn {
            #[cfg(feature = "issue-387-profile")]
            adapter_input_profile: crate::model::adapter::profile::InputLayout {
                selected_history_count: history.messages.len(),
                request_only_at: inserted_at,
            },
            input_manifest,
            retire_continuation,
            projection,
            effective_system_prompt: request.effective_system_prompt.clone(),
            snapshot,
            request,
            estimated_input,
        })
    }

    /// Freezes one staged model turn into its final prepared value: every
    /// staged context block is validated against the canonical conversation
    /// state, so the post-commit installation is infallible (Issue #63,
    /// Finding 2). Nothing commits here.
    pub(super) fn finalize_model_turn(
        &mut self,
        staged_context: &[MessageBlock],
        mut staged: StagedModelTurn,
    ) -> Result<PreparedModelTurn, Terminal> {
        let soft_limit = self
            .context_runtime
            .engine
            .soft_input_limit(self.compaction_budgets().primary_output_budget)
            .map_err(|error| Self::context_failure_terminal(&error))?;
        if staged.estimated_input >= soft_limit {
            return Err(Self::context_failure_terminal(&ContextError::new(
                ContextErrorKind::CannotFit,
                "final frozen request exceeds the soft input limit",
            )));
        }
        // The single mandatory adapter-owned semantic admission boundary follows
        // all compaction/carryover/media transformations, before freeze and start.
        // Ordinary retries bypass finalization and reuse this validated input.
        #[cfg(feature = "issue-387-profile")]
        let adapter_preflight_profile = crate::model::adapter::profile::NativeScope::new(
            staged.snapshot.request_id.as_str(),
            staged.adapter_input_profile,
        );
        crate::model::adapter::validation::validate_prepared_request(&staged.request).map_err(
            |error| Terminal::Failed {
                failure: AttemptFailure::Runtime {
                    error: RuntimeError::Unsupported {
                        message: format!("native model input compatibility: {}", error.message),
                    },
                },
            },
        )?;
        #[cfg(feature = "issue-387-profile")]
        drop(adapter_preflight_profile);
        let associations = std::mem::take(&mut staged.snapshot.tool_associations);
        staged
            .snapshot
            .freeze_selected_input(
                &self.request.conversation_id,
                &staged.request,
                &staged.input_manifest,
            )
            .map_err(|error| {
                Self::context_failure_terminal(&ContextError::new(
                    ContextErrorKind::CannotFit,
                    error,
                ))
            })?;
        staged.snapshot.tool_associations = associations;
        if staged.retire_continuation {
            self.retire_continuation();
            self.measurements.withdraw();
        }
        let mut context = Vec::with_capacity(staged_context.len());
        for block in staged_context {
            context.push(self.conversation.prepare_commit(block).map_err(|error| {
                Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::ContractViolation {
                            message: format!("staged context cannot be validated: {error}"),
                        },
                    },
                }
            })?);
        }
        let anchor = ObservedAnchor::of_selected(
            staged.projection.surface_revision,
            &staged.request.messages,
            &staged.request.effective_system_prompt,
            &staged.request.tools,
        );
        let request_identity = Arc::new(RequestObservationIdentity {
            invocation: staged.request.invocation.clone(),
            continuation: staged.request.continuation.clone(),
        });
        let request = staged.request;
        Ok(PreparedModelTurn {
            #[cfg(feature = "issue-387-profile")]
            adapter_input_profile: staged.adapter_input_profile,
            context,
            snapshot: staged.snapshot,
            request,
            anchor,
            request_identity,
            estimated_input: staged.estimated_input,
        })
    }

    /// The current finite projection of the Conversation Surface, or the
    /// terminal the attempt must settle with when the context plane failed.
    fn current_projection(
        &self,
        effective_system_prompt: &str,
    ) -> Result<ContextProjection, Terminal> {
        #[cfg(test)]
        let _profile = attribution::Timer::new(attribution::Phase::ContextProjection);
        let history = self.store.projected_history().map_err(|error| {
            Self::context_failure_terminal(&ContextError::new(
                ContextErrorKind::MalformedHistory,
                error.to_string(),
            ))
        })?;
        Ok(self.context_runtime.engine.build_projection_input(
            self.conversation.revision(),
            history.messages,
            &self.tool_registry().model_definitions(),
            self.reusable_observation(),
            effective_system_prompt,
        ))
    }

    /// Renders the exact request-time Effective System Prompt solely from
    /// the attempt-pinned resource generation and accepted request sections.
    pub(super) fn effective_system_prompt(&self) -> Result<String, Terminal> {
        if let Some(accepted) = &self.logical_step.accepted_context {
            return Ok(render_effective_system_prompt(&accepted.system_sections));
        }
        let sections = self
            .context_runtime
            .assembly
            .system_sections(&self.context_runtime.native_system)
            .map_err(|error| {
                Self::context_failure_terminal(&ContextError::new(
                    ContextErrorKind::Internal,
                    error.to_string(),
                ))
            })?;
        Ok(render_effective_system_prompt(&sections))
    }

    fn claimed_model_input(
        &self,
        ids: &[MessageId],
    ) -> Result<Vec<MessageBlock>, crate::durable::ConversationStoreError> {
        #[cfg(test)]
        let _reads = crate::model::selected_input::PreparationReads::new();
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        let history = self.store.projected_history()?;
        let mut selected = Vec::with_capacity(ids.len());
        for message in history
            .messages
            .iter_rev()
            .filter_map(ModelInputMessage::as_canonical)
        {
            if ids.iter().any(|id| id == message.id()) {
                selected.push(message.clone());
                if selected.len() == ids.len() {
                    break;
                }
            }
        }
        selected.reverse();
        Ok(selected)
    }

    fn contributor_input_snapshot(&self) -> Result<ContributorInputSnapshot, Terminal> {
        let claimed_inbound = self
            .claimed_model_input(
                self.pending_fresh_inbound
                    .as_ref()
                    .map_or(&[], |fresh| fresh.message_ids()),
            )
            .map_err(|error| {
                Self::context_failure_terminal(&ContextError::new(
                    ContextErrorKind::MalformedHistory,
                    error.to_string(),
                ))
            })?;
        Ok(ContributorInputSnapshot {
            surface: self.conversation.freeze_active_surface(),
            opportunities: ContributionOpportunities {
                fresh_inbound: self.pending_fresh_inbound.as_ref().map(|fresh| {
                    FreshInboundOpportunity {
                        target_message_id: fresh.last_message_id().clone(),
                    }
                }),
                post_tool_batch: self
                    .pending_post_tool_batch
                    .as_ref()
                    .map(|pending| pending.opportunity),
            },
            post_tool_batch_anchor: self
                .pending_post_tool_batch
                .as_ref()
                .and_then(|pending| pending.transcript_anchor),
            attempt_id: self.request.attempt_id.clone(),
            conversation_id: self.request.conversation_id.clone(),
            turn: self.turn,
            surface_revision: self.conversation.revision(),
            claimed_inbound,
            workspace_root: self.capability.snapshot().workspace_root().to_path_buf(),
            capability_revision: self.capability.snapshot().revision(),
        })
    }

    /// The only dynamic-context staging path (Issue #12, M9b). Core assigns
    /// the generation, provenance, semantic kind, and canonical `MessageId`;
    /// contributors have no access to any of those authorities.
    ///
    /// Staging validates the new messages against the canonical owner and
    /// the already-staged identities, then records the accepted generation.
    /// It commits nothing and does not rebuild historical canonical bodies. The staged blocks become canonical
    /// exactly when the model-turn start arbitration commits them inside
    /// the durable start transaction; if cancellation wins there, they are
    /// discarded without a trace and history never claims the model
    /// observed them.
    fn stage_context(
        &mut self,
        mut accepted: AcceptedContext,
    ) -> Result<Vec<MessageBlock>, Terminal> {
        self.context_generation_serial = self
            .context_generation_serial
            .checked_add(1)
            .expect("context generation cannot overflow");
        accepted.generation.id = self.context_generation_serial;
        let namespace = format!("{}-turn-{}", self.request.attempt_id, self.turn);
        let mut serial = self
            .conversation
            .revision()
            .get()
            .max(self.conversation.ledger().len() as u64);
        let mut staged = Vec::with_capacity(accepted.user_messages.len());
        for context in &mut accepted.user_messages {
            let id = loop {
                let candidate = MessageId::new(format!("rustx-context-{namespace}-{serial}"));
                serial = serial
                    .checked_add(1)
                    .expect("context identity space exhausted");
                if !self.conversation.ledger().contains(&candidate)
                    && !staged
                        .iter()
                        .any(|message: &MessageBlock| message.id() == &candidate)
                {
                    break candidate;
                }
            };
            context.message_id = Some(id.clone());
            let block = MessageBlock::User(crate::message::types::UserMessageBlock {
                id,
                content: context.content.clone(),
                source: context.source.clone(),
                kind: crate::message::types::InboundKind::Context(context.kind.clone()),
                timestamp: None,
            });
            self.conversation
                .prepare_commit(&block)
                .map_err(|error| Terminal::Failed {
                    failure: AttemptFailure::Runtime {
                        error: RuntimeError::ContractViolation {
                            message: format!("staged context cannot be committed: {error}"),
                        },
                    },
                })?;
            staged.push(block);
        }
        self.logical_step.accepted_context = Some(accepted);
        Ok(staged)
    }

    /// Builds the provider-neutral request from one finite context projection.
    ///
    /// Every projected item is already a complete canonical message, so
    /// there is nothing to materialize: the projection's messages *are* the
    /// request messages. The exact Effective System Prompt is carried as a
    /// provider-neutral request value and adapters only translate it.
    fn model_request_from_projection(
        &mut self,
        projection: &ContextProjection,
        messages: crate::model::SelectedModelInput,
    ) -> ModelRequest {
        let primary = self.request.model.primary();
        // Projection and invocation share exactly the frozen registry.
        // Model capability validation may reject an unsupported request;
        // it must not silently rewrite the admitted Tool authority.
        let mut tools = self.tool_registry().model_definitions();
        if let Some(workflow_output) = self.workflow_output.as_ref() {
            tools.push(crate::tools::types::ModelToolDefinition {
                id: ToolId::new("runtime-workflow-output"),
                name: crate::runtime::workflow::WORKFLOW_OUTPUT_TOOL_NAME.to_owned(),
                description: "Commit the structured terminal value of this Workflow AgentRun. It must be the sole tool-shaped call in the Assistant turn.".to_owned(),
                input_schema: workflow_output.output_schema(),
            });
        }
        let mut effective_system_prompt = projection.effective_system_prompt.clone();
        if let Some(feedback) = &self.logical_step.frozen_workflow_feedback {
            if !effective_system_prompt.is_empty() {
                effective_system_prompt.push('\n');
            }
            effective_system_prompt.push_str("[runtime Workflow output feedback]\n");
            effective_system_prompt.push_str(feedback);
        }
        // The corrective hint of the pending corrective generation. It is
        // *borrowed*, never taken: building a request is not the lifetime
        // boundary of this state. Every actual request that realizes the
        // corrective generation — the first one, a transient retry of it, a
        // post-compaction retry of it — must carry the same hint, and the
        // state is cleared only when that generation resolves or the logical
        // step ends. The instruction is chosen by the anomaly class and is
        // bounded, provider-independent, and request-only; it never contains
        // the discarded generation.
        if let Some(hint) = self.logical_step.semantic_recovery.pending_hint() {
            if !effective_system_prompt.is_empty() {
                effective_system_prompt.push('\n');
            }
            effective_system_prompt.push_str("[runtime generation feedback]\n");
            effective_system_prompt.push_str(hint.anomaly.instruction());
            effective_system_prompt.push_str("\nReason: ");
            effective_system_prompt.push_str(&hint.reason);
        }
        ModelRequest {
            response_schema: None,
            response_byte_limit: None,
            images: std::collections::BTreeMap::new(),
            invocation: primary.invocation_config(),
            messages,
            tools,
            effective_system_prompt,
            continuation: self.pending_continuation.clone(),
            continuation_source: self.continuation_source.clone(),
        }
    }
}

#[cfg(test)]
mod tests;
