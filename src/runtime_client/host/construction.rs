//! Host construction over a live runtime, a child activation or durable authorities.
use super::bootstrap::bootstrap_live_projection;
use super::bootstrap::durable_projection;
use super::bootstrap::install_bootstrap_bridge;
use super::{
    Arc, AtomicBool, BTreeMap, ClientInner, ClientState, ConversationStore, HostConstructionError,
    Mutex, NativePublicationProjection, PendingObservations, ProjectionWorker, PublicationGate,
    RootInteractionPublicationAuthority, RuntimeClientHost, RuntimeClientHostConfig,
};

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
    /// [`RuntimeClientEndpoint`](super::super::endpoint::RuntimeClientEndpoint)
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
            replay_limit.unwrap_or(super::super::projection::RUNTIME_CLIENT_REPLAY_LIMIT_DEFAULT);
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

    pub(super) fn construct(
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
            .unwrap_or(super::super::projection::RUNTIME_CLIENT_REPLAY_LIMIT_DEFAULT);
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
        projection.set_effective_plugins(super::super::settings::EffectivePlugins::project(
            &config.runtime.native_extensions(),
        ));
        if config.runtime.model_is_frozen() {
            projection.set_settings_evidence(super::super::settings::SettingsEvidence::FrozenChild);
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
        probe: super::super::test_sync::ProjectionProbe,
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
}
