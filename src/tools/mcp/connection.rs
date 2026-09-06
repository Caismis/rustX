//! MCP connection generations and bounded local reconnection (Issue #205).
//!
//! # What a connection generation is
//!
//! One [`McpConnectionGeneration`] is exactly **one concrete negotiated
//! transport/session authority**: one spawned stdio unit or one Streamable
//! HTTP session, its completed protocol handshake, its negotiated revision,
//! its rmcp peer, and its own protocol-corruption observation seam. A
//! generation is never repaired: it is either live or dead, and a dead one
//! is retired and replaced.
//!
//! # Why the connection is a separate owner
//!
//! Before Issue #205 an MCP executor captured one `Arc<McpServerRuntime>`
//! permanently, so a transport that died left every discovered tool of that
//! server pointing at a corpse forever. The catalog and the transport were
//! also the same mutable value, which collapses two facts that must stay
//! apart:
//!
//! ```text
//! capability knowledge   -> the published capability generation
//!                           (last-known-good; owned by the capability
//!                            coordinator's candidate/commit transaction)
//!
//! transport availability -> the current connection generation
//!                           (owned here; replaceable without touching
//!                            capability knowledge at all)
//! ```
//!
//! [`McpConnection`] is the **stable connection owner** that closes that
//! gap. Executors bind to the connection, never to a transport generation,
//! and resolve the authoritative generation at the instant they dispatch.
//! A published capability generation therefore keeps serving tool calls
//! across a transport replacement, and a dead transport never becomes a
//! reason to erase validated capability knowledge.
//!
//! # Reconnect is not replay
//!
//! ```text
//! Generation N
//!   |
//!   +-- ToolCall A dispatched            (effect frontier crossed)
//!   |
//!   X   transport lost
//!   |
//!   +-- A settles OutcomeUnknown         (the call's own future; terminal)
//!   |
//!   v
//! Generation N+1 established
//!   |
//!   `-- ToolCall B dispatched normally
//! ```
//!
//! Reconnection is **only** reachable from [`McpConnection::acquire`], which
//! is called by a *new* dispatch **before** that dispatch crosses its own
//! effect frontier. There is no queue of in-flight requests, no
//! correlation-id carry-over, and no resubmission path of any kind: a
//! request that already crossed the frontier lives entirely inside its own
//! execution future, which has already reached a terminal classification by
//! the time any replacement generation exists. No code path can hand a
//! previously dispatched `tools/call` to a new generation.
//!
//! # Boundedness and ownership
//!
//! Reconnection is bounded by construction: **at most one connect attempt
//! per dispatch**, driven inline by the dispatching execution future. There
//! is no reconnect loop, no backoff timer, no detached reconnect task, and
//! no background health prober — so there is no task lifetime to own beyond
//! the tool execution that asked for a transport. A failed attempt fails
//! that one dispatch (pre-frontier, an ordinary `Failed`), and the next
//! dispatch may try again.
//!
//! Physical ownership of every generation this connection ever established
//! stays with the connection: [`McpConnection::close`] drives the current
//! generation *and* every generation retired but not yet closed to the same
//! physical settlement proof the capability drain has always required.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::identity::McpServerId;
use crate::tools::mcp::{
    McpError, McpInvalidationState, McpServerBinding, McpServerRuntime, OwnedConnect,
};
use crate::tools::workspace::Workspace;

/// How many typed connection facts one connection retains for diagnosis.
///
/// The log is a bounded diagnostic ring, never authority: nothing reads it
/// to make a decision, and it can never grow with connection churn.
const MAX_RETAINED_CONNECTION_FACTS: usize = 32;

/// How many of those facts one diagnostic renders.
const MAX_RENDERED_CONNECTION_FACTS: usize = 4;

/// One concrete negotiated transport/session authority of a configured MCP
/// server.
///
/// The identity is the pair (server identity, generation number). Generation
/// numbers start at 1 and increase by one per established transport; they
/// are process-local diagnostic identity and are deliberately **not**
/// cross-process capability identity (that remains `McpToolIdentity`).
#[derive(Debug)]
pub(crate) struct McpConnectionGeneration {
    generation: u64,
    runtime: Arc<McpServerRuntime>,
}

impl McpConnectionGeneration {
    /// The process-local generation number of this transport authority.
    pub(crate) const fn generation(&self) -> u64 {
        self.generation
    }

    /// The negotiated transport authority itself.
    pub(crate) fn runtime(&self) -> &Arc<McpServerRuntime> {
        &self.runtime
    }
}

/// One bounded typed connection fact.
///
/// These are MCP-specific transport facts for diagnosis. They deliberately
/// never become canonical conversation state (an explicit Issue #205
/// non-goal) and never redefine generic Tool status: the canonical outcome
/// of a call is owned by the Agent Loop's generic lifecycle, and the
/// per-call MCP diagnostic travels in the typed `ToolExecutionResult` the
/// Event Journal already commits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum McpConnectionFact {
    /// A transport generation completed its handshake and became the
    /// authoritative transport of this server.
    GenerationEstablished {
        /// The established generation number.
        generation: u64,
        /// The MCP revision that generation negotiated.
        protocol_version: String,
    },
    /// A transport generation was proven unusable and retired. It never
    /// serves another request as healthy.
    GenerationLost {
        /// The retired generation number.
        generation: u64,
        /// The bounded evidence that proved it unusable.
        reason: String,
    },
    /// A replacement transport generation could not be established.
    GenerationUnavailable {
        /// The bounded connect failure.
        reason: String,
    },
}

/// The inputs one connection needs to establish a replacement generation.
///
/// A connection without this authority is *fixed*: it owns exactly the one
/// transport it was constructed with and never reconnects. That is the
/// contract of the public [`crate::tools::mcp::McpToolExecutor::new`] entry
/// point, whose caller owns and closes its own runtime.
struct McpReconnectAuthority {
    binding: McpServerBinding,
    workspace: Workspace,
    invalidation: Arc<McpInvalidationState>,
    /// The connection's own ownership cancellation root. Closing the
    /// connection cancels it first, so a connect in flight when drain starts
    /// drives its physical process to settlement and returns instead of
    /// outliving the owner.
    cancellation: CancellationSignal,
}

struct ConnectionState {
    /// The authoritative transport generation, when one exists.
    current: Option<Arc<McpConnectionGeneration>>,
    /// Generations proven unusable whose physical close has not run yet.
    /// They are closed before a replacement is established, and again by
    /// [`McpConnection::close`], so no generation this connection created
    /// escapes its physical settlement proof.
    retired: Vec<Arc<McpConnectionGeneration>>,
    /// The next generation number to hand out.
    next_generation: u64,
    /// Set by [`McpConnection::close`]. An acquire after this point never
    /// establishes a new generation.
    closed: bool,
}

/// The stable connection owner of one configured MCP server.
pub(crate) struct McpConnection {
    server_id: McpServerId,
    reconnect: Option<McpReconnectAuthority>,
    /// One asynchronous mutex serializes health arbitration, retirement,
    /// replacement, and close. Concurrent dispatches therefore share exactly
    /// one connect attempt rather than racing to spawn several servers, and
    /// close linearizes against an attempt already in flight.
    state: tokio::sync::Mutex<ConnectionState>,
    facts: Mutex<VecDeque<McpConnectionFact>>,
    /// The most recently established transport, retained for the
    /// deterministic close/retirement probe seam only.
    #[cfg(test)]
    probe_runtime: Mutex<Option<Arc<McpServerRuntime>>>,
}

impl std::fmt::Debug for McpConnection {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("McpConnection")
            .field("server_id", &self.server_id)
            .field("reconnectable", &self.reconnect.is_some())
            .finish_non_exhaustive()
    }
}

impl McpConnection {
    /// A connection that owns exactly one transport and never reconnects.
    ///
    /// This is the shape of the public adapter entry point: the caller
    /// connected the runtime itself and remains responsible for closing it,
    /// so this connection must not close it and must not replace it.
    pub(crate) fn fixed(runtime: Arc<McpServerRuntime>) -> Arc<Self> {
        let connection = Self::empty(runtime.server_id().clone(), None);
        connection.install_initial(runtime);
        connection
    }

    /// A conversation-owned connection that may establish a bounded
    /// replacement generation for later requests.
    pub(crate) fn reconnectable(
        runtime: Arc<McpServerRuntime>,
        binding: McpServerBinding,
        workspace: Workspace,
        invalidation: Arc<McpInvalidationState>,
        cancellation: CancellationSignal,
    ) -> Arc<Self> {
        let connection = Self::empty(
            runtime.server_id().clone(),
            Some(McpReconnectAuthority {
                binding,
                workspace,
                invalidation,
                cancellation,
            }),
        );
        connection.install_initial(runtime);
        connection
    }

    fn empty(server_id: McpServerId, reconnect: Option<McpReconnectAuthority>) -> Arc<Self> {
        Arc::new(Self {
            server_id,
            reconnect,
            state: tokio::sync::Mutex::new(ConnectionState {
                current: None,
                retired: Vec::new(),
                next_generation: 1,
                closed: false,
            }),
            facts: Mutex::new(VecDeque::new()),
            #[cfg(test)]
            probe_runtime: Mutex::new(None),
        })
    }

    /// Installs the connection's first transport generation. The connect
    /// itself already happened; this only publishes it as generation 1.
    fn install_initial(self: &Arc<Self>, runtime: Arc<McpServerRuntime>) {
        let mut state = self
            .state
            .try_lock()
            .expect("a freshly constructed connection is unshared");
        let generation = state.next_generation;
        state.next_generation += 1;
        let protocol_version = runtime.protocol_version().to_string();
        #[cfg(test)]
        {
            *self.probe_runtime.lock().expect("MCP probe runtime lock") = Some(runtime.clone());
        }
        state.current = Some(Arc::new(McpConnectionGeneration {
            generation,
            runtime,
        }));
        drop(state);
        self.record(McpConnectionFact::GenerationEstablished {
            generation,
            protocol_version,
        });
    }

    /// Resolves the authoritative transport generation for one **new**
    /// request, establishing a replacement when the current generation is
    /// proven unusable.
    ///
    /// This is the single entry point through which any MCP operation
    /// obtains a transport, and it is always reached **before** the calling
    /// operation crosses its own external-effect frontier. A failure here is
    /// therefore an ordinary pre-frontier failure of that one operation: no
    /// remote side effect was possible, and nothing is retried or replayed.
    ///
    /// # Errors
    ///
    /// Returns an error when the connection is closed, when the current
    /// generation is unusable and this connection cannot reconnect, or when
    /// the single bounded connect attempt failed.
    pub(crate) async fn acquire(&self) -> Result<Arc<McpConnectionGeneration>, McpError> {
        let mut state = self.state.lock().await;
        if state.closed {
            return Err(McpError::Execution(format!(
                "the MCP connection to server '{}' is closed",
                self.server_id
            )));
        }
        // Health arbitration: a generation stops being authoritative the
        // moment its own transport proves it unusable — a closed runtime, a
        // confirmed protocol violation, or an observed transport-loss fact
        // recorded by an earlier operation. This is the connection's
        // linearization point for "generation becomes dead".
        if let Some(current) = state.current.clone()
            && let Some(reason) = current.runtime.unusable_reason()
        {
            state.current = None;
            state.retired.push(current.clone());
            self.record(McpConnectionFact::GenerationLost {
                generation: current.generation,
                reason,
            });
        }
        if let Some(current) = &state.current {
            return Ok(current.clone());
        }
        let Some(reconnect) = &self.reconnect else {
            let reason = format!(
                "the MCP connection to server '{}' has no live transport and this connection \
                 does not own reconnection",
                self.server_id
            );
            self.record(McpConnectionFact::GenerationUnavailable {
                reason: reason.clone(),
            });
            return Err(McpError::Execution(reason));
        };
        // Every generation this connection retired is driven to its physical
        // settlement proof before a replacement exists, so at most one
        // server process of this connection is alive at any time and drain
        // never inherits an unsettled corpse.
        Self::close_retired(&mut state).await;
        if reconnect.cancellation.is_cancelled() {
            let reason = format!(
                "MCP reconnection to server '{}' was refused: the connection owner is \
                 cancelled",
                self.server_id
            );
            self.record(McpConnectionFact::GenerationUnavailable {
                reason: reason.clone(),
            });
            return Err(McpError::Execution(reason));
        }
        // Exactly one connect attempt, owned by the dispatch that asked for
        // a transport. A reconnect makes previously frozen capability
        // knowledge stale — the replacement server may publish a different
        // catalog — so the shared invalidation epoch advances. That marks
        // knowledge as needing revalidation; it never erases the published
        // last-known-good capability generation, which stays authoritative
        // until a complete validated candidate commits.
        reconnect.invalidation.lock().advance(&self.server_id);
        let connected = McpServerRuntime::connect_owned(OwnedConnect::new(
            &self.server_id,
            &reconnect.binding,
            &reconnect.workspace,
            reconnect.invalidation.clone(),
            reconnect.cancellation.child(),
        ))
        .await;
        match connected {
            Ok(runtime) => {
                let generation = state.next_generation;
                state.next_generation += 1;
                let protocol_version = runtime.protocol_version().to_string();
                #[cfg(test)]
                {
                    *self.probe_runtime.lock().expect("MCP probe runtime lock") =
                        Some(runtime.clone());
                }
                let established = Arc::new(McpConnectionGeneration {
                    generation,
                    runtime,
                });
                state.current = Some(established.clone());
                drop(state);
                self.record(McpConnectionFact::GenerationEstablished {
                    generation,
                    protocol_version,
                });
                Ok(established)
            }
            Err(error) => {
                drop(state);
                let reason = error.to_string();
                self.record(McpConnectionFact::GenerationUnavailable {
                    reason: reason.clone(),
                });
                Err(error)
            }
        }
    }

    /// Closes the connection permanently and drives every generation it
    /// established to its physical settlement proof.
    ///
    /// The ownership cancellation root is fired **before** the state lock is
    /// taken, so a connect attempt already in flight settles its own
    /// physical process and returns rather than blocking drain behind a
    /// handshake. After this returns, no acquire can establish a generation
    /// and no generation of this connection remains unsettled.
    ///
    /// Returns one bounded diagnostic per generation that could not prove
    /// physical terminality.
    pub(crate) async fn close(&self) -> Vec<String> {
        if let Some(reconnect) = &self.reconnect {
            reconnect.cancellation.cancel();
        }
        let mut state = self.state.lock().await;
        state.closed = true;
        let mut failures = Vec::new();
        let current = state.current.take();
        let retired = std::mem::take(&mut state.retired);
        for generation in retired.into_iter().chain(current) {
            // A fixed connection does not own the runtime it was handed: the
            // public adapter caller connected it and closes it itself.
            if self.reconnect.is_none() {
                continue;
            }
            if let Err(error) = generation.runtime.close().await {
                failures.push(error.to_string());
            }
        }
        failures
    }

    /// Closes every retired generation while the state lock is held.
    async fn close_retired(state: &mut ConnectionState) {
        for generation in std::mem::take(&mut state.retired) {
            // A close failure here is not this dispatch's outcome: physical
            // settlement evidence is owned by the capability drain, which
            // closes the connection again and reports every generation that
            // could not prove terminality.
            let _: Result<(), McpError> = generation.runtime.close().await;
        }
    }

    fn record(&self, fact: McpConnectionFact) {
        let mut facts = self.facts.lock().expect("MCP connection fact lock");
        if facts.len() == MAX_RETAINED_CONNECTION_FACTS {
            facts.pop_front();
        }
        facts.push_back(fact);
    }

    /// The bounded rendering of the most recent connection facts.
    ///
    /// This is how the typed connection log reaches a reader: a dispatch
    /// that could not obtain a transport carries this summary into its
    /// `ToolExecutionResult` diagnostic, which the Event Journal already
    /// commits with the call's terminal fact. The connection facts therefore
    /// aid diagnosis exactly where the failure is observed, without becoming
    /// canonical conversation state and without duplicating the generic
    /// Issue #204 lifecycle facts.
    pub(crate) fn recent_diagnostic(&self) -> String {
        let facts = self.facts.lock().expect("MCP connection fact lock");
        facts
            .iter()
            .rev()
            .take(MAX_RENDERED_CONNECTION_FACTS)
            .rev()
            .map(|fact| match fact {
                McpConnectionFact::GenerationEstablished {
                    generation,
                    protocol_version,
                } => format!("generation {generation} established ({protocol_version})"),
                McpConnectionFact::GenerationLost { generation, reason } => {
                    format!("generation {generation} lost: {reason}")
                }
                McpConnectionFact::GenerationUnavailable { reason } => {
                    format!("no replacement generation: {reason}")
                }
            })
            .collect::<Vec<_>>()
            .join("; ")
    }

    /// The most recently established transport of this connection.
    ///
    /// Deterministic ownership/probe tests only: it is the stable identity
    /// of "the transport this connection last negotiated", independent of
    /// whether that transport is still authoritative, so a test can install
    /// a close probe on it and compare generations across publications.
    /// Production code never resolves a transport this way — it goes through
    /// [`McpConnection::acquire`], which arbitrates health.
    #[cfg(test)]
    pub(crate) fn published_runtime(&self) -> Option<Arc<McpServerRuntime>> {
        self.probe_runtime
            .lock()
            .expect("MCP probe runtime lock")
            .clone()
    }
}
