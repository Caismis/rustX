//! Construction, recovery bootstrap, activation, and observation-bridge
//! installation of the one conversation runtime coordinator.

use super::*;

/// The failure of a bounded local observation subscription.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RuntimeObservationSubscriptionError {
    /// No Runtime Client host (or other primary consumer) installed the
    /// observation bridge yet.
    BridgeNotInstalled {
        /// The conversation whose observation stream was requested.
        conversation_id: ConversationId,
    },
    /// The runtime crossed its activation cut before the subscription was
    /// admitted, so adding it would create a gap in the live stream.
    RuntimeAlreadyActivated {
        /// The conversation whose observation stream was requested.
        conversation_id: ConversationId,
    },
}

impl core::fmt::Display for RuntimeObservationSubscriptionError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BridgeNotInstalled { conversation_id } => write!(
                f,
                "the conversation runtime of {conversation_id} has no observation bridge"
            ),
            Self::RuntimeAlreadyActivated { conversation_id } => write!(
                f,
                "the conversation runtime of {conversation_id} is already activated; local observation subscriptions bind before activation"
            ),
        }
    }
}

impl std::error::Error for RuntimeObservationSubscriptionError {}

/// The one runtime observation-bridge failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RuntimeBootstrapError {
    /// An observation bridge was already installed over this runtime.
    ///
    /// The one-time Runtime Client binding claim makes this unreachable
    /// for a production adapter; it exists so a rejected host construction
    /// over a runtime already observed by a headless consumer fails typed
    /// and releases its claim instead of silently sharing a queue.
    BridgeAlreadyInstalled {
        /// The conversation whose runtime is already bridged.
        conversation_id: ConversationId,
    },
    /// The runtime was already activated.
    ///
    /// Binding a Runtime Client host is a pre-activation composition
    /// decision (Issue #61): the bootstrap seed is only coherent while the
    /// runtime is inert, so a late bind is refused rather than
    /// approximated.
    RuntimeAlreadyActivated {
        /// The conversation whose runtime is already activated.
        conversation_id: ConversationId,
    },
    /// The native durable authority could not provide a coherent bootstrap
    /// head or active working set.
    Durable(String),
}

impl core::fmt::Display for RuntimeBootstrapError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::BridgeAlreadyInstalled { conversation_id } => write!(
                f,
                "the conversation runtime of {conversation_id} already has an observation bridge installed"
            ),
            Self::RuntimeAlreadyActivated { conversation_id } => write!(
                f,
                "the conversation runtime of {conversation_id} is already activated; a Runtime Client host binds before activation"
            ),
            Self::Durable(message) => {
                write!(f, "durable conversation bootstrap failed: {message}")
            }
        }
    }
}

impl std::error::Error for RuntimeBootstrapError {}

/// The semantic facts a Runtime Client adapter seeds from at construction:
/// the runtime-owned bootstrap snapshot captured by
/// [`ConversationRuntime::install_observation_bridge`] at the global cut
/// `R`.
///
/// Every field is a runtime-owned semantic source type or runtime-owned
/// immutable view; the Runtime Client layer owns the translation into its
/// snapshot read model.
///
/// There is deliberately no live attempt / Agent Status / compaction owner
/// field: this allocation binds its Runtime Client host while inactive and
/// has admitted none of them. The client separately reads committed historical
/// compaction metadata at `observation_cut`; later transitions arrive through
/// the live observation stream.
#[derive(Debug, Clone)]
pub(crate) struct RuntimeBootstrapSnapshot {
    pub observation_cut: Option<crate::durable::observation::ObservationReadCut>,
    pub source_failure: Option<String>,
    pub journal_through: u64,
    pub goal: Option<crate::observation::goal::RuntimeClientGoalView>,
    /// The conversation identity.
    pub conversation_id: ConversationId,
    /// Whether runtime drain has begun and new admission is closed.
    pub shutting_down: bool,
    /// The current model-visible Surface at the cut. Historical Ledger rows
    /// are deliberately not hydrated into the client projection; callers
    /// needing them use the durable store's paged read APIs.
    pub messages: Vec<MessageBlock>,
    /// The bounded newest page of the derived transcript. Bodies remain
    /// owned by their canonical durable domains; this seed is only a read
    /// result for the Runtime Client bootstrap.
    pub transcript: TranscriptPage,
    /// The authoritative session model view.
    pub model: crate::model::session::SessionModelReadSource,
    /// The authoritative effective/desired `ApprovalMode` state at the cut.
    pub approval_mode: ApprovalMode,
    /// The currently pending inbound items, in mailbox sequence order.
    pub inbound_pending: Vec<InboundItem>,
    /// The authoritative background execution records at the cut.
    ///
    /// Provably empty: a `ConversationRuntime` is constructed only over a
    /// pristine tool-runtime background plane — the ownership transfer
    /// requires no prepared dispatch and no committed record and then
    /// refuses `commit_dispatch` while the mailbox is bound inactive — so
    /// no record can exist when the bridge is installed. The seed is
    /// captured anyway, in the same registry section that installs the
    /// observer, so the handshake is one coherent cut for whatever state
    /// exists.
    pub background: Vec<BackgroundExecutionSnapshot>,
    pub background_cut: crate::runtime::identity::ObservationCollectionCut,
    pub agents_cut: crate::runtime::identity::ObservationCollectionCut,
    /// Durable Agent owner state and its exact latest committed activation,
    /// captured together under the registry lock. Recovered activation history
    /// never determines the latest Agent by iteration order.
    pub agents: Vec<(
        crate::runtime::subagent::AgentSnapshot,
        crate::runtime::subagent::SubagentSnapshot,
    )>,
    /// The active authoritative capability snapshot.
    pub capabilities: Arc<crate::capabilities::CapabilitySnapshot>,
    /// The authoritative capability-source availability at the cut
    /// (Issue #81).
    pub capability_availability: crate::capabilities::CapabilityAvailability,
    /// The immutable runtime resource generation current at the cut.
    pub resources: Arc<crate::runtime::resources::RuntimeResourceSnapshot>,
    /// Live process-owned native interactions at the bootstrap cut.
    pub pending_primary_cut: crate::runtime::identity::ObservationCollectionCut,
    pub pending_children_cut: crate::runtime::identity::ObservationCollectionCut,
    pub pending_interactions: Vec<crate::observation::interaction::RuntimeClientInteractionHeader>,
    /// The conversation's committed task list at the cut, when this runtime
    /// composes the Todo extension (Issue #259).
    ///
    /// The tool runtime rebuilt it from the whole canonical history at
    /// construction, so seeding it here is what lets a client that holds
    /// only the newest transcript page still show the current list. Every
    /// later change arrives as an ordinary committed `todo` result on the
    /// live observation stream.
    ///
    /// `None` means this runtime composes no Todo extension. It is
    /// deliberately distinct from `Some(empty)`: a client must present no
    /// current Todo surface at all, however many historical `todo` results
    /// the conversation's transcript still carries.
    pub todos: Option<crate::tools::todo::TodoSnapshot>,
}

impl RuntimeInner {
    /// Installs the observation bridge and captures the bootstrap seed at
    /// one global cut `R`.
    ///
    /// The whole handshake runs under the one coordinator lock, which is
    /// also what rejects an already-activated runtime atomically against
    /// [`ConversationRuntime::activate`]. See the module documentation
    /// ("The bootstrap cut") for the coherence proof; in short, the
    /// coordinator facts are frozen by the held lock, and every subsystem
    /// semantic commit is lifecycle-gated: the background registry refuses
    /// `commit_dispatch` while its mailbox is bound inactive, the mailbox
    /// refuses inbound while inactive, and the capability coordinator
    /// refuses a runtime-owned `commit` before activation — with the
    /// capability snapshot itself captured at `R`.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeBootstrapError::RuntimeAlreadyActivated`] when the
    /// runtime was already activated and
    /// [`RuntimeBootstrapError::BridgeAlreadyInstalled`] when an
    /// observation bridge already exists, or [`RuntimeBootstrapError::Durable`]
    /// when the native durable bootstrap cannot be read coherently.
    #[allow(clippy::too_many_lines)] // One inactive-runtime bootstrap synchronization boundary.
    pub(super) fn install_observation_bridge(
        self: &Arc<Self>,
        queue: Arc<PendingObservations>,
        client_projection: bool,
    ) -> Result<RuntimeBootstrapSnapshot, RuntimeBootstrapError> {
        // The one coordinator lock is held across every phase below: it is
        // the freeze that makes the combined seed one real global state.
        let state = self.lock_state();
        // Binding a Runtime Client host is a pre-activation composition
        // decision. Rejecting here, under the lock `activate` also takes
        // for its lifecycle transition, is what makes the host-binding
        // decision race atomically against activation: a bootstrap that
        // acquires the lock first sees `Inactive` and completes before
        // activation, one that acquires it after sees `Running` and is
        // refused.
        if self.lifecycle.is_activated() {
            return Err(RuntimeBootstrapError::RuntimeAlreadyActivated {
                conversation_id: self.conversation_id.clone(),
            });
        }
        if self.pending.get().is_some() {
            return Err(RuntimeBootstrapError::BridgeAlreadyInstalled {
                conversation_id: self.conversation_id.clone(),
            });
        }
        // Display indexes are derived read layouts. Their failure fences the
        // observation seed without rejecting the native runtime's activation.
        let mut source_failure = None;
        let observation_cut = match self.store.observation_read_cut() {
            Ok(cut) => Some(cut),
            Err(error) if client_projection => {
                source_failure = Some(error.to_string());
                None
            }
            Err(error) => return Err(RuntimeBootstrapError::Durable(error.to_string())),
        };
        let (messages, transcript) = if client_projection {
            let headers = observation_cut.as_ref().and_then(|cut| {
                match self.store.message_header_page(
                    cut,
                    None,
                    crate::durable::observation::HEADER_PAGE_MAX,
                ) {
                    Ok(page) => Some(page),
                    Err(error) => {
                        source_failure.get_or_insert(error.to_string());
                        None
                    }
                }
            });
            (
                headers.map_or_else(Vec::new, |page| {
                    page.headers
                        .into_iter()
                        .filter_map(|header| header.display)
                        .collect()
                }),
                TranscriptPage {
                    inherited_through: observation_cut.as_ref().map_or_else(
                        || crate::durable::TranscriptCursor::new(0),
                        |cut| cut.inherited_through,
                    ),
                    entries: Vec::new(),
                    next_cursor: None,
                },
            )
        } else {
            let head = self
                .store
                .load_head()
                .map_err(|error| RuntimeBootstrapError::Durable(error.to_string()))?;
            let messages = self
                .store
                .load_messages(&head.active_message_ids)
                .map_err(|error| RuntimeBootstrapError::Durable(error.to_string()))?;
            let transcript = self
                .store
                .load_transcript_page(None, TRANSCRIPT_BOOTSTRAP_PAGE_LIMIT)
                .map_err(|error| RuntimeBootstrapError::Durable(error.to_string()))?;
            (messages, transcript)
        };
        // ---- T0: the coordinator-owned facts ----
        //
        // An inactive runtime never moved its conversation state into an
        // attempt. Bootstrap only hydrates the current Surface working set;
        // the append-only Ledger remains a paged durable read authority.
        // The observation bridge is installed only before activation, so its
        // shutdown projection seed is necessarily false.
        let shutting_down = false;
        let model = state
            .model
            .read_source()
            .at_binding_revision(state.binding_revision);
        // The resource generation is part of the same freeze as every other
        // seeded fact: a client attaching before the first configuration application still sees
        // exactly which project instruction files the runtime loaded.
        let resources = Arc::clone(&state.resources);
        let approval_mode = state.effective_approval_mode;
        let observer: Arc<RuntimeObserver> = Arc::new(RuntimeObserver::new(self));
        // Interaction pending state is an ephemeral runtime observation, but
        // it still participates in the same bootstrap cut as every other
        // live projection fact. Installing the observer here means all later
        // pending/settled transitions enter the one observation queue.
        let pending_primary = self.interaction.observation_head();
        let mut pending_interactions: Vec<_> = pending_primary
            .head
            .iter()
            .map(|(sequence, source)| {
                let mut header = source.header().clone();
                header.source_offset = sequence.checked_mul(2).expect("pending source offset");
                header
            })
            .collect();
        let child_pending = self
            .subagents
            .as_ref()
            .map(crate::runtime::subagent::SubagentRegistry::pending_interaction_head);
        let child_pending_interactions = child_pending
            .as_ref()
            .map_or_else(Vec::new, |proof| proof.head.clone());
        // The list is not a live subsystem with an observer of its own: it
        // is a derivation of canonical tool results, and it moves only when
        // one of those commits. Reading the committed list inside the same
        // freeze keeps the seed and the live stream on one cut.
        let todos = if client_projection {
            self.tool_runtime.todos().map(|_| {
                observation_cut
                    .as_ref()
                    .and_then(|cut| match self.store.todo_observation_head(cut) {
                        Ok(head) => head.map(|head| head.snapshot),
                        Err(error) => {
                            source_failure.get_or_insert(error.to_string());
                            None
                        }
                    })
                    .unwrap_or_default()
            })
        } else {
            self.tool_runtime.todo_snapshot()
        };
        let goal = match self
            .tool_runtime
            .goal()
            .map(crate::goal::GoalDomain::observation_view)
            .transpose()
        {
            Ok(goal) => goal,
            Err(error) if client_projection => {
                source_failure.get_or_insert(error.to_string());
                None
            }
            Err(error) => return Err(RuntimeBootstrapError::Durable(error.to_string())),
        };
        let native_journal_seed = match self.store.presentation_frontier() {
            Ok(frontier) => frontier,
            Err(error) if client_projection => {
                source_failure.get_or_insert(error.to_string());
                0
            }
            Err(error) => return Err(RuntimeBootstrapError::Durable(error.to_string())),
        };
        queue.bind_journal(&self.store, native_journal_seed);
        let journal_through = if client_projection {
            let through = match self.store.observe_journal(queue.clone()) {
                Ok(through) => through,
                Err(error) => {
                    source_failure.get_or_insert(error.to_string());
                    native_journal_seed
                }
            };
            queue.bind_journal(&self.store, through);
            let workflow_observer = observer.clone();
            self.tool_runtime
                .workflows()
                .install_publication_observer(Arc::new(move |snapshot, sequence| {
                    workflow_observer.push(ConversationObservation::Published {
                        journal_sequence: sequence,
                        observation: Box::new(ConversationObservation::Workflow(
                            snapshot
                                .into_iter()
                                .map(crate::observation::workflow_source::WorkflowReadSource::new)
                                .collect(),
                        )),
                    });
                }));
            through
        } else {
            0
        };
        self.interaction.install_observer(observer.clone());
        // ---- T1: the mailbox (frozen: an inactive conversation refuses
        //          inbound) ----
        let inbound_pending = if client_projection {
            let page = observation_cut.as_ref().and_then(|cut| {
                match self.store.pending_header_page(
                    &cut.read,
                    None,
                    crate::durable::observation::HEADER_PAGE_MAX,
                ) {
                    Ok(page) => Some(page),
                    Err(error) => {
                        source_failure.get_or_insert(error.to_string());
                        None
                    }
                }
            });
            let pending = page.map_or_else(Vec::new, |page| {
                page.headers
                    .into_iter()
                    .filter_map(|header| match header.message.display {
                        Some(MessageBlock::User(message)) => {
                            Some(InboundItem::from_observation_header(
                                header.revision,
                                header.sequence,
                                message,
                            ))
                        }
                        _ => None,
                    })
                    .collect()
            });
            self.mailbox.install_observer(observer.clone());
            pending
        } else {
            self.mailbox
                .install_observer_and_pending(observer.clone())
                .map_err(|error| RuntimeBootstrapError::Durable(error.to_string()))?
        };
        if let Some(domain) = self.tool_runtime.goal() {
            domain.install_observer(observer.clone());
        }
        // ---- T2: the background registry (frozen: the registry refuses
        //          commits while its mailbox is bound inactive) ----
        let (background, background_cut) = self
            .tool_runtime
            .background()
            .install_observer_and_snapshots(observer.clone());
        // ---- T2b: the subagent registry (frozen by the same inactive
        //           mailbox binding) ----
        let (agents, agents_cut) = self
            .subagents
            .as_ref()
            .map(|subagents| subagents.install_observer_and_agent_snapshots(observer.clone()))
            .unwrap_or_default();
        pending_interactions.extend(child_pending_interactions);
        pending_interactions.sort_by_key(|header| header.source_offset);
        // No fallible subsystem has been mutated after the pending read. The
        // pre-check above and the coordinator lock make this set infallible;
        // keep the explicit branch as a defensive invariant assertion.
        if self
            .pending
            .set(Arc::new(ObservationFanout::new(queue)))
            .is_err()
        {
            return Err(RuntimeBootstrapError::BridgeAlreadyInstalled {
                conversation_id: self.conversation_id.clone(),
            });
        }
        // ---- R: the capability coordinator, the cut itself ----
        let (capabilities, capability_availability) =
            self.capability.install_observer_and_snapshot(observer);
        drop(state);
        Ok(RuntimeBootstrapSnapshot {
            source_failure,
            observation_cut,
            journal_through,
            conversation_id: self.conversation_id.clone(),
            shutting_down,
            messages,
            transcript,
            model,
            approval_mode,
            inbound_pending,
            background,
            background_cut,
            agents,
            agents_cut,
            pending_interactions,
            pending_primary_cut: pending_primary.cut,
            pending_children_cut: child_pending.map_or_else(Default::default, |proof| proof.cut),
            todos,
            goal,
            capabilities,
            capability_availability,
            resources,
        })
    }

    /// Reads only the source-owned maximum Human timestamp and first User
    /// preview, independently of the Runtime Client event queue. Inactive
    /// admission prevents an opening commit from slipping past installation.
    pub(super) fn session_observation_metadata(
        &self,
    ) -> Result<
        tokio::sync::watch::Receiver<crate::runtime::observation::SessionObservationMetadata>,
        RuntimeObservationSubscriptionError,
    > {
        let _state = self.lock_state();
        if self.lifecycle.is_activated() {
            return Err(
                RuntimeObservationSubscriptionError::RuntimeAlreadyActivated {
                    conversation_id: self.conversation_id.clone(),
                },
            );
        }
        let Some(fanout) = self.pending.get() else {
            return Err(RuntimeObservationSubscriptionError::BridgeNotInstalled {
                conversation_id: self.conversation_id.clone(),
            });
        };
        Ok(fanout.session_metadata())
    }
}

impl ConversationRuntime {
    /// Creates the runtime in its **inactive** lifecycle state, unless an
    /// authoritative MCP physical-settlement failure is replayed during
    /// construction; that failure moves the runtime directly into the
    /// explicit draining/failure lifecycle so activation cannot reopen
    /// healthy admission.
    ///
    /// An ordinary inactive runtime is inert: its mailbox refuses inbound, it
    /// has no admission worker, it admits no attempt, and it publishes no
    /// observation. The sole exception is an authoritative MCP settlement
    /// failure replayed during construction, which enters the explicit
    /// draining/failure lifecycle and is never activatable. Otherwise the
    /// composition may now optionally bind a `RuntimeClientHost` over it, and
    /// must then call [`ConversationRuntime::activate`] before semantic
    /// execution can begin. A headless composition simply activates directly.
    ///
    /// This low-level constructor has no durable Session selection owner. Its
    /// model selection can be replaced in memory through [`Self::model_set`].
    /// Local Session composition uses a separate immutable owner boundary and
    /// rejects that direct control, including through Runtime Client handles.
    ///
    /// # One conversation authority
    ///
    /// The conversation identity of the runtime *is*
    /// [`ConversationToolRuntime::conversation_id`]. The configuration
    /// carries no conversation id of its own, so the runtime's identity, the
    /// canonical mailbox, the authoritative background registry, and the
    /// Runtime Client binding identity all name one conversation by
    /// construction. Every conversation-scoped value this runtime derives —
    /// generated inbound message ids, generated attempt ids, and every
    /// [`AgentExecutionRequest`] it issues — uses that one identity.
    ///
    /// The capability coordinator is a *separate* authoritative identity,
    /// so it is still validated explicitly against the runtime.
    ///
    /// # Construction order
    ///
    /// Every fallible validation runs before the runtime exists, so a
    /// rejected construction has no semantic side effect at all.
    ///
    /// # Errors
    ///
    /// Returns [`ConversationRuntimeError::OwnershipMismatch`] when the
    /// capability coordinator and the conversation tool runtime do not
    /// share the same conversation/workspace ownership domain,
    /// [`ConversationRuntimeError::InvalidModelTimeoutPolicy`] when either
    /// runtime model deadline is zero,
    /// [`ConversationRuntimeError::InvalidToolDeadlinePolicy`] when the tool
    /// hard deadline or any idle-liveness window is zero,
    /// [`ConversationRuntimeError::Context`] when the context engine
    /// configuration is impossible,
    /// [`ConversationRuntimeError::InvalidInitialConversation`] when the
    /// initial canonical messages are invalid,
    /// [`ConversationRuntimeError::RuntimeAlreadyBound`] when the tool
    /// runtime or the capability coordinator identity is already bound to a
    /// conversation runtime, and
    /// [`ConversationRuntimeError::ToolRuntimeNotQuiescent`] when the tool
    /// runtime's background plane already holds prepared or committed
    /// background work.
    pub fn new(config: RuntimeConversationConfig) -> Result<Self, ConversationRuntimeError> {
        Self::new_with_monotonic_clock(config, None, ModelSelectionOwner::Conversation)
    }

    /// Durable local roots delegate every selection mutation to their Session
    /// owner. This ownership is immutable and shared by all runtime/client clones.
    pub(crate) fn new_session_owned(
        config: RuntimeConversationConfig,
    ) -> Result<Self, ConversationRuntimeError> {
        Self::new_with_monotonic_clock(config, None, ModelSelectionOwner::Session)
    }

    /// Creates a runtime with one explicitly supplied monotonic clock for a
    /// deterministic composition test. This seam is restricted to the crate
    /// test build and remains at the conversation-runtime composition root:
    /// normal runtime admission still constructs both sibling consumers from
    /// `RuntimeInner`'s one clock field.
    #[cfg(test)]
    pub(crate) fn with_test_monotonic_clock(
        config: RuntimeConversationConfig,
        monotonic_clock: Arc<dyn MonotonicClock>,
    ) -> Result<Self, ConversationRuntimeError> {
        Self::new_with_monotonic_clock(
            config,
            Some(monotonic_clock),
            ModelSelectionOwner::Conversation,
        )
    }

    #[allow(clippy::too_many_lines)]
    pub(super) fn new_with_monotonic_clock(
        config: RuntimeConversationConfig,
        injected_monotonic_clock: Option<Arc<dyn MonotonicClock>>,
        model_selection_owner: ModelSelectionOwner,
    ) -> Result<Self, ConversationRuntimeError> {
        // The one conversation authority at this boundary: every identity
        // this runtime publishes or derives comes from the tool runtime it
        // coordinates, so runtime and tool runtime cannot disagree.
        let conversation_id = config.tool_runtime.conversation_id().clone();

        // ---- Fallible validation: nothing below is observable yet. ----
        // Validate generic runtime execution policy before store
        // initialization, ownership claims, or any other observable work.
        // Admission can therefore rely on this invariant and keep its
        // per-attempt wiring infallible.
        if !config.model_timeout_policy.is_positive() {
            return Err(ConversationRuntimeError::InvalidModelTimeoutPolicy);
        }
        if !config.tool_deadline_policy.is_positive() {
            return Err(ConversationRuntimeError::InvalidToolDeadlinePolicy);
        }
        let snapshot = config.capability.current_snapshot();
        // The coordinator is a separate authoritative identity, so it is
        // still validated explicitly against the runtime's identity.
        if snapshot.conversation_id() != &conversation_id
            || snapshot.workspace_root() != config.tool_runtime.workspace().root()
        {
            return Err(ConversationRuntimeError::OwnershipMismatch {
                capability_conversation: snapshot.conversation_id().clone(),
                runtime_conversation: conversation_id,
            });
        }
        if !Arc::ptr_eq(config.resources.capability(), &snapshot) {
            return Err(ConversationRuntimeError::OwnershipMismatch {
                capability_conversation: config.resources.capability().conversation_id().clone(),
                runtime_conversation: conversation_id,
            });
        }
        // The one native Agent Extension composition invariant (Issue #259).
        //
        // Domain state is Conversation-owned. Configuration selects Plugin
        // capabilities; selected Tools must have real backing owners.
        let composition = config.tool_runtime.extensions();
        let materialized = crate::extensions::NativeAgentExtensions::from_materialized(
            config.context.status_engine.as_ref(),
            config.tool_runtime.todos(),
            config.tool_runtime.goal(),
        );
        let required = composition.expected_tool_plane();
        let configured_plane = config.capability.extension_tool_plane_shape();
        let active_plane = crate::extensions::ExtensionToolPlaneShape::of_published_registry(
            snapshot.tool_registry(),
        );
        if (configured_plane.todo && materialized.todo().is_none())
            || (configured_plane.goal && materialized.goal().is_none())
            || configured_plane.invalid_goal
            || active_plane != required
            || (required.todo && materialized.todo().is_none())
            || (required.goal && materialized.goal().is_none())
            || materialized.agent_status() != composition.agent_status()
        {
            return Err(ConversationRuntimeError::ExtensionCompositionMismatch {
                conversation_id,
                composed_todo: composition.todo().is_some(),
                materialized_todo_state: config.tool_runtime.todos().is_some(),
                configured_todo_tool: configured_plane.todo,
                active_todo_tool: active_plane.todo,
                agent_status_agrees: materialized.agent_status() == composition.agent_status(),
                goal_agrees: materialized.goal() == composition.goal()
                    && configured_plane.goal == required.goal
                    && active_plane.goal == required.goal
                    && !configured_plane.invalid_goal
                    && !active_plane.invalid_goal,
            });
        }
        // The subagent registry is a conversation-owned logical plane: the
        // runtime must not adopt a registry that belongs to another
        // conversation/agent domain or another canonical mailbox. The typed
        // ownership domain is validated here, before any claim, so a
        // rejected construction consumes nothing (Issue #60 hardening).
        // Pristine authority is deliberately **not** decided here: a
        // standalone child ownership commit can still be in flight, so the
        // authoritative pristine check runs after the mailbox ownership
        // transfer below (the one total-order point).
        if let Some(subagents) = &config.subagents {
            let registry_conversation = subagents.conversation_id().clone();
            if registry_conversation != conversation_id
                || subagents.parent_agent_id() != &config.agent_id
                || !subagents.shares_mailbox_domain(&config.tool_runtime.mailbox())
            {
                return Err(ConversationRuntimeError::SubagentOwnershipMismatch {
                    registry_conversation,
                    runtime_conversation: conversation_id,
                });
            }
        }
        // The initial session model must be able to run under the session
        // context policy. Validating here (and again in `model_set`) is what
        // makes the per-attempt context runtime construction infallible at
        // admission, where there is no caller left to report to.
        validate_model_context(&config.context.policy, &config.model)
            .map_err(|error| ConversationRuntimeError::Context(error.message))?;
        // The durable store is composed once by the tool runtime's
        // `ConversationStoreBinding`. The runtime receives the full handle
        // from that binding, while the mailbox/background plane receives
        // only the derived narrow capability. An independently selected
        // mailbox and store therefore have no production construction path.
        let store = config.tool_runtime.durable_store();
        store
            .initialize(&config.initial_messages)
            .map_err(|error| ConversationRuntimeError::Storage(error.to_string()))?;
        let clock = config
            .clock
            .unwrap_or_else(|| Arc::new(SystemClock) as Arc<dyn RuntimeClock>);
        // Activation spawns the admission worker, and a runtime with no
        // worker would silently never admit anything. The execution
        // runtime is required — and captured — here, still in the fallible
        // section and before any claim, so `activate` spawns the worker
        // unconditionally and the impossible composition is rejected as
        // early as it can be seen.
        let Ok(executor) = tokio::runtime::Handle::try_current() else {
            return Err(ConversationRuntimeError::NoExecutionRuntime);
        };

        // ---- Ownership commit: the one tool-runtime ownership transfer. ----
        //
        // The conversation runtime composes the one shared activation
        // lifecycle and claims the tool runtime through one
        // ownership-transfer contract (Issue #61): the transfer runs under
        // the background registry synchronization boundary, requires a
        // pristine background plane (no prepared dispatch, no committed
        // record), claims the one-time coordinator binding, and binds the
        // canonical mailbox runtime-owned with this fresh `Inactive`
        // lifecycle at the same linearization point. A standalone
        // background commit therefore either wins first (this construction
        // fails typed with `ToolRuntimeNotQuiescent`, and the claim is
        // never consumed) or this transfer wins first (a later commit
        // observes the runtime-owned inactive mailbox and is refused with
        // `BackgroundDispatchError::ConversationInactive`). An inactive
        // runtime can never inherit detached background work.
        let lifecycle = ConversationLifecycle::new();
        match config
            .tool_runtime
            .claim_conversation_runtime_inactive(&lifecycle)
        {
            Ok(()) => {}
            Err(crate::tools::runtime::ConversationRuntimeClaimError::AlreadyBound) => {
                return Err(ConversationRuntimeError::RuntimeAlreadyBound { conversation_id });
            }
            Err(crate::tools::runtime::ConversationRuntimeClaimError::NotQuiescent) => {
                return Err(ConversationRuntimeError::ToolRuntimeNotQuiescent { conversation_id });
            }
        }
        // The capability coordinator is a separate identity: it claims the
        // same shared lifecycle under its own state lock, so its commit
        // gate observes exactly the activation decision the mailbox and
        // the background registry observe.
        let Some(capability_publication) = config.capability.claim_conversation_runtime(&lifecycle)
        else {
            // Transactional construction: the tool-runtime ownership
            // transfer is rolled back to its exact previous standalone
            // state — mailbox unbound, coordinator claim released — so a
            // rejected construction leaves no trace.
            config.tool_runtime.release_conversation_runtime_claim();
            return Err(ConversationRuntimeError::RuntimeAlreadyBound { conversation_id });
        };

        // ---- Final subagent ownership arbitration (Issue #60) ----
        //
        // The tool-runtime/mailbox ownership claim above bound the canonical
        // mailbox to this runtime's `Inactive` lifecycle. A standalone
        // `SubagentRegistry::commit` can therefore no longer enter: its
        // mailbox `begin_running_admission`/`with_running_commit` observes
        // the runtime-owned `Inactive` lifecycle and is refused. This is
        // the deterministic total-order point against any standalone child
        // ownership commit that won the race before the bind. That commit
        // installs an exact ownership_committing claim before its off-lock
        // durable write. This pristine check observes that claim or the
        // published record atomically; it never waits on SQLite or silently
        // adopts a child started outside this runtime's ownership transfer.
        if let Some(subagents) = &config.subagents
            && !subagents.is_pristine()
        {
            // Transactional construction: roll back every claim acquired so
            // far — the capability claim and the tool-runtime claim (which
            // also unbinds the mailbox) — so the failed construction leaves
            // no runtime/mailbox/capability residue and a later correct
            // construction with a fresh plane remains possible.
            config.capability.release_conversation_runtime_claim();
            config.tool_runtime.release_conversation_runtime_claim();
            return Err(ConversationRuntimeError::SubagentRegistryNotPristine { conversation_id });
        }

        // ---- Startup recovery (Issue #12, M9a) ----
        //
        // Recovery runs **after** the ownership transfer and **before** the
        // runtime object exists. Both halves of that placement are
        // load-bearing:
        //
        // - *after* the claim, because reconciliation commits new durable
        //   facts. A construction that loses the ownership race must leave no
        //   trace, so it must never have reconciled anything; and the claim's
        //   pristine-background-plane precondition is exactly what proves the
        //   durably-owned-but-unpublished background executions this phase
        //   terminalizes have **no** live in-process record — they really are
        //   remnants of a dead process, never work this process owns.
        // - *before* the runtime exists, because activation and admission
        //   must not race an unfinished reconciliation. There is no
        //   coordinator lock yet, so no `SQLite` work here can ever be
        //   performed under the admission mutex.
        //
        // The conversation runtime is the recovery-policy owner and runs the
        // complete pipeline:
        //
        //     reconstruct -> classify -> reconcile -> recovered state
        //
        // The store contributes durable evidence and semantic transactions
        // only; it never decides whether an ambiguous request is replayable.
        //
        // A recovery failure rolls the ownership transfer back to its exact
        // previous standalone state and returns typed, so no runtime exists
        // that could admit work as though recovery had completed.
        let recovery = match crate::runtime::recovery::recover(store.as_ref(), clock.as_ref()) {
            Ok(recovery) => recovery,
            Err(error) => {
                config.capability.release_conversation_runtime_claim();
                config.tool_runtime.release_conversation_runtime_claim();
                return Err(match error {
                    crate::runtime::recovery::RecoveryError::Durable(detail) => {
                        ConversationRuntimeError::Storage(detail)
                    }
                    crate::runtime::recovery::RecoveryError::Unrecoverable(reason) => {
                        ConversationRuntimeError::RecoveryRequired { reason }
                    }
                });
            }
        };
        // The recovered hot read model is built from the durable head **after**
        // reconciliation, so it reflects the repaired canonical structure
        // rather than the crash-time one.
        let conversation = match Self::recovered_conversation(store.as_ref()) {
            Ok(conversation) => conversation,
            Err(error) => {
                config.capability.release_conversation_runtime_claim();
                config.tool_runtime.release_conversation_runtime_claim();
                return Err(error);
            }
        };

        // ---- Infallible wiring: from here construction always succeeds. ----
        let mailbox = config.tool_runtime.mailbox();
        // The conversation is inert until `activate`: the ownership
        // transfer already bound its mailbox with the Inactive lifecycle,
        // so nothing can be admitted and nothing can be observed while the
        // optional Runtime Client host binds.
        //
        // The subagent ordinal is the same kind of durable identity domain
        // (Issue #60): reseed above every ordinal already in durable
        // authority before any start can race this.
        if let Some(subagents) = &config.subagents {
            subagents.restore_sequence_watermark(recovery.highest_subagent_ordinal());
        }
        let recovered_continuation = match recovery.resume() {
            crate::runtime::recovery::ResumeDisposition::ContinueAdoptedTurn { goal } => {
                Some(match goal {
                    Some(goal) => AttemptProvenance::RecoveredGoalContinuation(goal),
                    None => AttemptProvenance::RecoveredContinuation,
                })
            }
            _ => None,
        };
        let next_attempt_seq = recovery.next_attempt_ordinal();
        // The coordinator receives the narrow interaction audit capability
        // only (Issue #109): it may commit the requested/settled facts of its
        // own interactions and reach no other durable domain.
        let interaction = Arc::new(InteractionCoordinator::new(
            conversation_id.clone(),
            lifecycle.clone(),
            crate::durable::interaction_audit_capability(store.clone()),
        ));
        // The shared durability frontier: one gate carries the
        // `DurabilityFailed` fact to the conversation-owned registries so
        // their new durable ownership commits linearize against it (Issue
        // #60). It is created here and installed on the registries below,
        // after the ownership transfer and the pristine arbitration, while
        // the runtime is still inactive — no ownership commit can race the
        // installation.
        let durability_gate = Arc::new(DurabilityGate::new());
        let monotonic_clock = injected_monotonic_clock
            .unwrap_or_else(|| Arc::new(SystemMonotonicClock::new()) as Arc<dyn MonotonicClock>);
        let auxiliary = crate::runtime::auxiliary::executor::AuxiliaryExecutor::new(
            store.clone(),
            clock.clone(),
            monotonic_clock.clone(),
            lifecycle.clone(),
            durability_gate.clone(),
            crate::runtime::auxiliary::capacity::ProcessCapacity::shared_process().conversation(),
        )
        .map_err(|error| ConversationRuntimeError::Storage(error.message))?;
        let inner = Arc::new(RuntimeInner {
            configuration_owner: Arc::new(()),
            model_selection_owner,
            conversation_id,
            agent_id: config.agent_id,
            context: config.context,
            tool_runtime: config.tool_runtime,
            mailbox,
            store,
            capability: config.capability,
            capability_publication,
            subagents: config.subagents,
            workflow_output: config.workflow_output,
            interaction,
            lifecycle,
            clock,
            model_timeout_policy: config.model_timeout_policy,
            tool_deadline_policy: config.tool_deadline_policy,
            monotonic_clock,
            durability_gate: durability_gate.clone(),
            auxiliary,
            external_auxiliary: super::auxiliary::ExternalAuxiliary::new(),
            manual_tail: Mutex::new(None),
            session_title_task: Mutex::new(None),
            recovery,
            executor,
            state: Mutex::new(CoordinatorState {
                binding_revision: 1,
                explicit_model: config.explicit_model,
                model: config.model,
                resources: config.resources,
                mcp_settlement_failure: None,
                effective_approval_mode: config.approval_mode,
                conversation: Some(conversation),
                current_attempt: None,
                scheduled: None,
                scheduled_retries: std::collections::BTreeMap::default(),
                manual_compaction: None,
                next_attempt_seq,
                admitted_attempts: 0,
                latest_settlement: None,
                parent_turn_permits: None,
                parent_guidance_sealed: false,
                one_shot_cancel: None,
                recovered_continuation,
                // Durability health after a successful recovery is an
                // explicit transition, not a silent reset (Issue #12, M9a):
                // recovery either established a coherent durable state — in
                // which case the runtime starts a fresh admission cycle — or
                // it failed, in which case construction already returned and
                // no runtime exists to be healthy. A previous process's crash
                // never poisons a runtime whose classification and
                // reconciliation succeeded. The absorbing DurabilityFailed
                // fact itself lives only in the DurabilityGate.
                admission_durability_cycle: AdmissionDurabilityCycle {
                    budget: AdmissionRetryBudget::default(),
                    pending_retry: None,
                },
            }),
            wake: Arc::new(WakeGate::new()),
            worker_started: AtomicBool::new(false),
            published_eligibility: Mutex::new(None),
            drain: std::sync::OnceLock::new(),
            drain_started: AtomicBool::new(false),
            pending: std::sync::OnceLock::new(),
            settlement: tokio::sync::Notify::new(),
            #[cfg(test)]
            probe: Mutex::new(None),
            #[cfg(test)]
            test_pre_tool_policy: Mutex::new(None),
        });
        // Recovery has already durably terminalized every orphaned child.
        // Restore only durable Agent and activation records; no child
        // process, policy, or frozen resource is reconstructed from current
        // configuration.
        if let Some(subagents) = &inner.subagents {
            subagents
                .restore_agents(inner.store.as_ref())
                .map_err(|error| ConversationRuntimeError::Storage(error.to_string()))?;
        }
        // The runtime is the durability-health owner of its background
        // plane (Issue #63): install the narrow failure seam the
        // background settlement owner reports an exhausted
        // terminal-publication budget through. No background execution can
        // exist before activation — the registry refused every commit
        // while the mailbox was bound inactive — so the installation can
        // never race a settlement.
        durability_gate.install_failure_publisher(Arc::new({
            let weak = Arc::downgrade(&inner);
            move |failure| {
                if let Some(inner) = weak.upgrade() {
                    inner.observe(ConversationObservation::DurabilityFailed {
                        operation: failure.operation.as_str().to_owned(),
                        diagnostic: failure.diagnostic,
                    });
                    inner.wake.eligibility.notify_one();
                }
            }
        }));
        inner
            .tool_runtime
            .background()
            .install_failure_sink(Arc::new(BackgroundFailureSink {
                inner: Arc::downgrade(&inner),
            }));
        // The runtime durability frontier is shared with both
        // conversation-owned durable ownership planes: a new subagent or
        // background ownership commit must linearize against the runtime's
        // `DurabilityFailed` commit on the one gate.
        inner
            .tool_runtime
            .background()
            .install_durability_gate(durability_gate.clone());
        // The conversation runtime is also the durability-health owner of
        // its subagent plane. The registry calls this seam only after its
        // bounded terminal-publication retry budget is exhausted, and never
        // while its own registry mutex is held.
        if let Some(subagents) = &inner.subagents {
            subagents.install_failure_sink(Arc::new(SubagentFailureSink {
                inner: Arc::downgrade(&inner),
            }));
            subagents.install_durability_gate(durability_gate.clone());
        }
        // A retired MCP generation whose close reports PhysicalSettlement
        // has not been reclaimed. The registry retains the evidence; this
        // runtime-owned callback closes healthy admission immediately while
        // preserving the newly published logical resource generation for
        // final drain reporting.
        let mcp_failure_callback: Arc<dyn Fn(String) + Send + Sync> = Arc::new({
            let weak = Arc::downgrade(&inner);
            move |detail| {
                if let Some(inner) = weak.upgrade() {
                    inner.fence_mcp_settlement_failure(detail);
                }
            }
        });
        inner
            .capability
            .install_mcp_retirement_failure_callback(&mcp_failure_callback);
        Ok(Self { inner })
    }

    /// Hydrates the bounded hot conversation read model from the durable
    /// Surface head, after startup recovery has reconciled it.
    ///
    /// Only the current active working set is materialized: retired Ledger
    /// facts and historical Surface revisions stay in the durable store and
    /// are read on demand.
    pub(super) fn recovered_conversation(
        store: &dyn ConversationStore,
    ) -> Result<ConversationState, ConversationRuntimeError> {
        let history = store
            .projected_history()
            .map_err(|error| ConversationRuntimeError::Storage(error.to_string()))?;
        let active: Vec<_> = history
            .messages
            .into_iter()
            .filter_map(|m| m.as_canonical().cloned())
            .collect();
        let ids = active.iter().map(|m| m.id().clone()).collect();
        ConversationState::from_durable_head(
            active,
            ids,
            SurfaceRevision::new(history.cut.ledger.0),
            history.cut.activation.0,
        )
        .map_err(|error| ConversationRuntimeError::InvalidInitialConversation(error.to_string()))
    }

    /// Installs the observation bridge shared with the Runtime Client
    /// adapter (or a headless observer) and captures the semantic
    /// bootstrap snapshot, as one linearizable handoff.
    ///
    /// See the module documentation ("Adapter bootstrap linearization")
    /// for the cut contract: every authority contributes its seed under
    /// its own lock in the same section that installs its observation
    /// seam, and the queue is installed under the same section that
    /// captures the coordinator-owned facts, so a transition can never be
    /// lost between a seed and the live observation stream and can never
    /// be applied twice.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeBootstrapError::BridgeAlreadyInstalled`] when an
    /// observation bridge was already installed (a previous headless
    /// consumer). The one-time Runtime Client binding claim guarantees
    /// this cannot happen for a production adapter, and a failed host
    /// construction releases that claim again.
    #[cfg(test)]
    pub(crate) fn install_observation_bridge(
        &self,
        queue: Arc<PendingObservations>,
    ) -> Result<RuntimeBootstrapSnapshot, RuntimeBootstrapError> {
        self.inner.install_observation_bridge(queue, false)
    }

    pub(crate) fn install_publication_projection(
        &self,
        projection: Arc<dyn crate::runtime::observation::PublicationProjectionCommit>,
    ) {
        self.inner
            .pending
            .get()
            .expect("native bridge installed before activation")
            .install_publication_projection(projection);
    }

    /// Runtime Client composition additionally stages durable publication receipts.
    /// Native/headless consumers retain ordinary semantic observation delivery.
    pub(crate) fn install_client_observation_bridge(
        &self,
        queue: Arc<PendingObservations>,
    ) -> Result<RuntimeBootstrapSnapshot, RuntimeBootstrapError> {
        self.inner.install_observation_bridge(queue, true)
    }

    /// Reads the native maximum Human activity and first User boundary slot
    /// before activation; this does not allocate an observation FIFO.
    pub(crate) fn session_observation_metadata(
        &self,
    ) -> Result<
        tokio::sync::watch::Receiver<crate::runtime::observation::SessionObservationMetadata>,
        RuntimeObservationSubscriptionError,
    > {
        self.inner.session_observation_metadata()
    }

    /// Test-only: the observation bridge installed through
    /// [`ConversationRuntime::install_observation_bridge`], when one is.
    ///
    /// Boundary tests park/unpark the parent's projection input through
    /// this handle to own the fold schedule deterministically.
    #[cfg(test)]
    pub(crate) fn installed_observation_bridge(&self) -> Option<Arc<PendingObservations>> {
        self.inner.pending.get().map(|fanout| fanout.primary())
    }

    /// Claims the one-time Runtime Client binding of the tool runtime and of
    /// the capability coordinator.
    ///
    /// The Runtime Client protocol binds one runtime identity to at most one
    /// adapter for that identity's lifetime, so cloning a runtime never
    /// yields a second bindable identity and dropping the bound adapter
    /// never makes it bindable again. Reconnect replaces the attachment,
    /// not the adapter.
    pub(crate) fn claim_client_binding(&self) -> bool {
        if !self.inner.tool_runtime.claim_runtime_client() {
            return false;
        }
        if !self.inner.capability.claim_runtime_client() {
            self.inner.tool_runtime.release_runtime_client_claim();
            return false;
        }
        true
    }

    /// Releases a Runtime Client binding claimed by a host construction
    /// that then failed.
    ///
    /// This exists only so a rejected `RuntimeClientHost::new` (whose
    /// observation bridge install failed after the claim) leaves no trace;
    /// it is never called on host drop, and a successfully constructed
    /// host never releases its binding.
    pub(crate) fn release_client_binding(&self) {
        self.inner.tool_runtime.release_runtime_client_claim();
        self.inner.capability.release_runtime_client_claim();
    }

    /// Activates the runtime: semantic execution may begin.
    ///
    /// This is the one explicit lifecycle boundary of Issue #61: the single
    /// `Inactive -> Running` transition of the shared
    /// [`ConversationLifecycle`](crate::runtime::types::ConversationLifecycle)
    /// every runtime-owned semantic boundary observes. Before it, the
    /// runtime is inert and a `RuntimeClientHost` may bind over it; at it,
    /// the Runtime Client host-binding decision is frozen. Activation is
    /// the bootstrap cut a bound Runtime Client projection is seeded
    /// against.
    ///
    /// The `compare_exchange` runs while the one coordinator lock is held and
    /// the lifecycle's native commit boundary is acquired, so the
    /// host-binding decision races atomically against activation and the
    /// lifecycle also has a total order with non-coordinator runtime commits.
    /// A bootstrap that acquires the coordinator lock first sees `Inactive`
    /// and completes before activation, one that acquires it after sees
    /// `Running` and is refused with
    /// [`RuntimeBootstrapError::RuntimeAlreadyActivated`]. The transition
    /// itself is the linearization point; spawning the admission worker and
    /// the initial admission kick are the one-time post-transition steps of
    /// the single winning caller.
    ///
    /// Runtime Client *attachments* remain fully dynamic afterwards: this
    /// boundary freezes only which adapter (if any) observes the runtime,
    /// never how long a client stays attached.
    ///
    /// Activating twice is a no-op: exactly one concurrent call commits the
    /// transition and performs the one-time post-activation work; every
    /// other call observes `Running` and returns without changing anything.
    /// An authoritative MCP physical-settlement failure is a separate
    /// terminal fence: activation observes it under the coordinator lock and
    /// returns without transitioning the lifecycle to `Running`.
    ///
    /// # Panics
    ///
    /// Panics only if the test-only coordinator probe lock is poisoned,
    /// which would mean a previous test hook panicked while holding it.
    pub fn activate(&self) {
        // Test-only activation gate: parked before the coordinator lock and
        // before the lifecycle transition, so while the park holds the
        // conversation is provably still Inactive and every competing
        // runtime-owned commit or host bind can still proceed. The gate
        // parks only when armed and disarms after one park. The gate handle
        // is extracted before the park so the probe mutex is not held while
        // parked.
        #[cfg(test)]
        let activation_gate = self
            .inner
            .probe
            .lock()
            .expect("coordinator probe lock poisoned")
            .as_ref()
            .and_then(|probe| probe.activation_gate.clone());
        #[cfg(test)]
        if let Some(gate) = activation_gate {
            gate.enter();
        }
        {
            // The lock serializes the lifecycle transition against the
            // host-binding handshake; the CAS is the transition itself.
            let state = self.inner.lock_state();
            if state.mcp_settlement_failure.is_some() {
                // An authoritative physical-settlement failure may have been
                // replayed while the runtime was inactive. The callback has
                // either already moved the lifecycle to Draining or is
                // ordered immediately before this check; in neither case may
                // activation open healthy admission.
                return;
            }
            if !self.inner.lifecycle.activate() {
                return;
            }
            // Spawn the worker before releasing the same lock that drain
            // takes. This closes the activation-vs-drain window in which a
            // successful activation could otherwise leave `worker_started`
            // false while drain observes quiescence.
            self.inner.ensure_worker();
        }
        // Any inbound published before activation (there can be none: the
        // mailbox refused it) and any inbound racing this activation is
        // admitted here rather than depending on a wake permit.
        if !self.inner.admit_next_scheduled() {
            self.inner.admit_next_attempt();
        }
        // Activation publishes the first eligibility synchronously: once
        // `activate` returns, the projection's `unavailable` bootstrap value has
        // been answered, independent of when the worker first runs.
        self.inner.publish_adoption_eligibility();
    }

    /// Whether this runtime has left the inactive lifecycle, including an
    /// explicit MCP-failure drain that never opened healthy admission.
    #[must_use]
    pub fn is_activated(&self) -> bool {
        self.inner.lifecycle.is_activated()
    }
}

#[cfg(test)]
mod tests;
