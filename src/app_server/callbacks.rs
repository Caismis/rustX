//! Bounded live-incarnation callbacks. Selection is not native settlement.
use super::protocol::{
    ClientReply, JsonRpcVersion, RequestId, ServerMessage, ServerRequest, ServerRequestMethod,
};
use crate::runtime::identity::ConversationId;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

const MAX_PENDING: usize = 128;
// One full live replay and its resolutions must fit before a new reader runs.
const CLIENT_QUEUE: usize = 2 * MAX_PENDING;

struct Participant {
    threads: BTreeMap<ConversationId, Subscription>,
    sender: mpsc::Sender<Queued>,
    waker: Arc<futures_util::task::AtomicWaker>,
    closed: CancellationToken,
    overloaded: CancellationToken,
}

#[derive(Clone)]
struct Subscription {
    id: String,
    lifecycle: crate::runtime::types::ConversationLifecycle,
}

struct Pending {
    validity: Option<Validity>,
    dispatched: Arc<std::sync::atomic::AtomicBool>,
    thread: ConversationId,
    request: ServerRequest,
    recipients: BTreeMap<String, String>,
    reply: oneshot::Sender<ClientReply>,
}

#[derive(Clone)]
enum Validity {
    Interaction {
        owner: std::sync::Weak<crate::runtime_client::host::ClientInner>,
        interaction: crate::runtime::interaction::InteractionRef,
    },
    Tool(crate::runtime::cancellation::ExecutionCancellation),
}

#[derive(Default)]
struct State {
    next: u64,
    clients: BTreeMap<String, Participant>,
    pending: BTreeMap<RequestId, Pending>,
}

pub(crate) struct Callbacks {
    incarnation: String,
    state: Mutex<State>,
    interactions: Mutex<BTreeMap<crate::runtime::interaction::InteractionRef, RequestId>>,
}

impl std::fmt::Debug for Callbacks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Callbacks").finish_non_exhaustive()
    }
}
impl crate::runtime_client::host::InteractionLifecycleSink for Arc<Callbacks> {
    fn pending(
        &self,
        thread: &ConversationId,
        header: &crate::observation::interaction::RuntimeClientInteractionHeader,
        owner: std::sync::Weak<crate::runtime_client::host::ClientInner>,
    ) {
        self.publish_interaction(thread, header, owner);
    }

    fn retired(&self, interaction: &crate::runtime::interaction::InteractionRef) {
        self.interaction_settled(interaction);
    }
    fn origin_retired(&self, conversation: &ConversationId) {
        let mut interactions = self.interactions.lock().expect("interaction callbacks");
        interactions.retain(|interaction, id| {
            if interaction.conversation_id == *conversation {
                self.resolve(id);
                false
            } else {
                true
            }
        });
    }
}

pub(crate) struct Queued {
    pub(crate) message: ServerMessage,
    pub(crate) publication: Option<Publication>,
}

/// Request publication shares native validity and subscription fences with
/// reply acquisition. A canceled or replaced request cannot start at a slow
/// carrier after retirement. Already accepted frames remain externally uncertain.
pub(crate) struct Publication {
    callbacks: Arc<Callbacks>,
    client: String,
    id: RequestId,
    subscription: Subscription,
    validity: Option<Validity>,
    waker: Arc<futures_util::task::AtomicWaker>,
    dispatched: Arc<std::sync::atomic::AtomicBool>,
}
impl Publication {
    fn with_live<T>(&self, action: impl FnOnce() -> T) -> Option<T> {
        let current = || {
            let state = self.callbacks.state.lock().expect("callbacks");
            let pending = state.pending.get(&self.id)?;
            let participant = state.clients.get(&self.client)?;
            if participant.closed.is_cancelled()
                || participant
                    .threads
                    .get(&pending.thread)
                    .map(|value| &value.id)
                    != Some(&self.subscription.id)
                || pending.recipients.get(&self.client) != Some(&self.subscription.id)
            {
                return None;
            }
            Some(action())
        };
        // Native incarnation retirement precedes callback validity and broker
        // correlation. Never acquire a native gate while holding the broker map.
        self.subscription
            .lifecycle
            .with_running_commit(|| match &self.validity {
                Some(Validity::Interaction { owner, interaction }) => owner
                    .upgrade()?
                    .with_pending_interaction_header(interaction, |_| current())
                    .flatten(),
                Some(Validity::Tool(cancellation)) => cancellation.with_active(current).flatten(),
                None => current(),
            })
            .ok()
            .flatten()
    }
    pub(crate) fn discarded(&self, cx: &std::task::Context<'_>) -> bool {
        self.waker.register(cx.waker());
        self.with_live(|| ()).is_none()
    }
    pub(crate) fn hand_off<T>(
        &self,
        record: &str,
        accept: impl FnOnce(&str) -> std::task::Poll<std::io::Result<T>>,
    ) -> super::registration::Acceptance<T> {
        use super::registration::Acceptance;
        match self.with_live(|| {
            let result = accept(record);
            if matches!(&result, std::task::Poll::Ready(Ok(_))) {
                self.dispatched
                    .store(true, std::sync::atomic::Ordering::Release);
            }
            result
        }) {
            None => Acceptance::Discarded,
            Some(std::task::Poll::Pending) => Acceptance::Pending,
            Some(std::task::Poll::Ready(Err(error))) => Acceptance::Failed(error),
            Some(std::task::Poll::Ready(Ok(value))) => {
                Acceptance::Accepted(record.to_owned(), value)
            }
        }
    }
}

pub(crate) struct Client {
    pub(crate) id: String,
    pub(crate) messages: tokio::sync::Mutex<mpsc::Receiver<Queued>>,
    pub(crate) overloaded: CancellationToken,
    pub(crate) waker: Arc<futures_util::task::AtomicWaker>,
    callbacks: Arc<Callbacks>,
}

impl Drop for Client {
    fn drop(&mut self) {
        self.callbacks.disconnect(&self.id);
    }
}

/// Invocation-local ownership. Drop retires pending state on cancellation,
/// deadline, native terminal cleanup, or abandonment; it never replays work.
pub(crate) struct Ticket {
    pub(crate) id: RequestId,
    pub(crate) reply: oneshot::Receiver<ClientReply>,
    dispatched: Arc<std::sync::atomic::AtomicBool>,
    callbacks: Arc<Callbacks>,
}
impl Drop for Ticket {
    fn drop(&mut self) {
        self.callbacks.resolve(&self.id);
    }
}

impl Default for Callbacks {
    fn default() -> Self {
        Self {
            incarnation: uuid::Uuid::new_v4().to_string(),
            state: Mutex::default(),
            interactions: Mutex::default(),
        }
    }
}

impl Callbacks {
    pub(crate) fn interaction(
        self: &Arc<Self>,
        thread: &ConversationId,
        interaction: &crate::runtime::interaction::InteractionRef,
        owner: &Arc<crate::runtime_client::host::ClientInner>,
    ) {
        owner.with_pending_interaction_header(interaction, |header| {
            self.publish_interaction(thread, header, Arc::downgrade(owner));
        });
    }

    fn publish_interaction(
        self: &Arc<Self>,
        thread: &ConversationId,
        header: &crate::observation::interaction::RuntimeClientInteractionHeader,
        owner: std::sync::Weak<crate::runtime_client::host::ClientInner>,
    ) {
        let interaction = &header.interaction;
        let mut interactions = self.interactions.lock().expect("interaction callbacks");
        if interactions.contains_key(interaction) {
            return;
        }
        let Ok(mut ticket) = self.request(
            thread.clone(),
            ServerRequestMethod::Interaction {
                thread_id: thread.clone(),
                interaction: Box::new(header.clone()),
            },
            Some(Validity::Interaction {
                owner: owner.clone(),
                interaction: interaction.clone(),
            }),
        ) else {
            let interaction = interaction.clone();
            tokio::spawn(async move {
                if let Some(owner) = owner.upgrade() {
                    let _ = owner.cancel_interaction(&interaction).await;
                }
            });
            return;
        };
        interactions.insert(interaction.clone(), ticket.id.clone());
        let interaction = interaction.clone();
        tokio::spawn(async move {
            if let Ok(reply) = (&mut ticket.reply).await {
                let Some(owner) = owner.upgrade() else {
                    return;
                };
                let response = match reply {
                    ClientReply::Success(reply) => {
                        super::wire::interaction_response_from_value(reply.result).ok()
                    }
                    ClientReply::Error(_) => None,
                };
                if let Some(response) = response {
                    if owner
                        .respond_interaction(&interaction, response)
                        .await
                        .is_err()
                    {
                        let _ = owner.cancel_interaction(&interaction).await;
                    }
                } else {
                    let _ = owner.cancel_interaction(&interaction).await;
                }
            }
        });
    }

    pub(crate) fn interaction_settled(
        &self,
        interaction: &crate::runtime::interaction::InteractionRef,
    ) {
        if let Some(id) = self
            .interactions
            .lock()
            .expect("interaction callbacks")
            .remove(interaction)
        {
            self.resolve(&id);
        }
    }
    pub(crate) fn connect(self: &Arc<Self>) -> Arc<Client> {
        let id = uuid::Uuid::new_v4().to_string();
        let (sender, receiver) = mpsc::channel(CLIENT_QUEUE);
        let closed = CancellationToken::new();
        let overloaded = CancellationToken::new();
        let waker = Arc::new(futures_util::task::AtomicWaker::new());
        self.state.lock().expect("callbacks").clients.insert(
            id.clone(),
            Participant {
                threads: BTreeMap::new(),
                sender,
                waker: waker.clone(),
                closed: closed.clone(),
                overloaded: overloaded.clone(),
            },
        );
        Arc::new(Client {
            id,
            messages: tokio::sync::Mutex::new(receiver),
            overloaded,
            waker,
            callbacks: self.clone(),
        })
    }

    pub(crate) fn subscribe(
        self: &Arc<Self>,
        client: &str,
        thread: &ConversationId,
        lifecycle: crate::runtime::types::ConversationLifecycle,
    ) {
        let mut state = self.state.lock().expect("callbacks");
        let Some(participant) = state.clients.get_mut(client) else {
            return;
        };
        if participant.closed.is_cancelled()
            || (lifecycle.is_activated() && !lifecycle.is_running())
        {
            return;
        }
        // A subscription replacement is not another external execution.
        if participant
            .threads
            .get(thread)
            .is_some_and(|value| value.lifecycle.is_running())
        {
            return;
        }
        let subscription = Subscription {
            id: uuid::Uuid::new_v4().to_string(),
            lifecycle,
        };
        participant
            .threads
            .insert(thread.clone(), subscription.clone());
        let sender = participant.sender.clone();
        let closed = participant.closed.clone();
        let overloaded = participant.overloaded.clone();
        let waker = participant.waker.clone();
        for pending in state
            .pending
            .values_mut()
            .filter(|pending| pending.thread == *thread)
        {
            if sender
                .try_send(Queued {
                    message: ServerMessage::Request(pending.request.clone()),
                    publication: Some(Publication {
                        callbacks: self.clone(),
                        client: client.to_owned(),
                        id: pending.request.id.clone(),
                        subscription: subscription.clone(),
                        validity: pending.validity.clone(),
                        dispatched: pending.dispatched.clone(),
                        waker: waker.clone(),
                    }),
                })
                .is_ok()
            {
                pending
                    .recipients
                    .insert(client.to_owned(), subscription.id.clone());
                waker.wake();
            } else {
                closed.cancel();
                overloaded.cancel();
                break;
            }
        }
    }

    pub(crate) fn unsubscribe(&self, client: &str, thread: &ConversationId) {
        let mut state = self.state.lock().expect("callbacks");
        if let Some(participant) = state.clients.get_mut(client) {
            participant.threads.remove(thread);
            participant.waker.wake();
        }
        for pending in state
            .pending
            .values_mut()
            .filter(|pending| pending.thread == *thread)
        {
            pending.recipients.remove(client);
        }
    }

    pub(crate) fn disconnect(&self, client: &str) {
        let mut state = self.state.lock().expect("callbacks");
        if let Some(participant) = state.clients.remove(client) {
            participant.closed.cancel();
            participant.waker.wake();
        }
        for pending in state.pending.values_mut() {
            pending.recipients.remove(client);
        }
        // An accepted invocation remains server-owned, even with no clients.
    }

    fn request(
        self: &Arc<Self>,
        thread: ConversationId,
        method: ServerRequestMethod,
        validity: Option<Validity>,
    ) -> Result<Ticket, &'static str> {
        let mut state = self.state.lock().expect("callbacks");
        if state.pending.len() >= MAX_PENDING {
            return Err("server request capacity exceeded");
        }
        state.next = state
            .next
            .checked_add(1)
            .ok_or("server request identity exhausted")?;
        let id = RequestId::String(format!("{}:{}", self.incarnation, state.next));
        let request = ServerRequest {
            jsonrpc: JsonRpcVersion::V2,
            id: id.clone(),
            call: method,
        };
        // The same complete bound applies before copying into client queues.
        super::transport::serialize_record(&request)
            .map_err(|_| "server request exceeds message limit")?;
        let mut recipients = BTreeMap::new();
        let dispatched = Arc::new(std::sync::atomic::AtomicBool::new(false));
        for (client, participant) in &state.clients {
            if let Some(subscription) = participant.threads.get(&thread)
                && !participant.closed.is_cancelled()
                && subscription.lifecycle.is_running()
            {
                if participant
                    .sender
                    .try_send(Queued {
                        message: ServerMessage::Request(request.clone()),
                        publication: Some(Publication {
                            callbacks: self.clone(),
                            client: client.clone(),
                            id: id.clone(),
                            subscription: subscription.clone(),
                            validity: validity.clone(),
                            dispatched: dispatched.clone(),
                            waker: participant.waker.clone(),
                        }),
                    })
                    .is_ok()
                {
                    recipients.insert(client.clone(), subscription.id.clone());
                    participant.waker.wake();
                } else {
                    participant.closed.cancel();
                    participant.overloaded.cancel();
                }
            }
        }
        if recipients.is_empty() && !matches!(validity, Some(Validity::Interaction { .. })) {
            return Err("no live subscribers accepted the server request");
        }
        let (reply, received) = oneshot::channel();
        state.pending.insert(
            id.clone(),
            Pending {
                validity,
                dispatched: dispatched.clone(),
                thread,
                request,
                recipients,
                reply,
            },
        );
        Ok(Ticket {
            id,
            reply: received,
            dispatched,
            callbacks: self.clone(),
        })
    }

    /// The sole response linearization. Envelope validation precedes this call;
    /// domain validation follows acquisition and cannot reopen this entry.
    pub(crate) fn respond(&self, client: &str, response: ClientReply) -> bool {
        let (validity, subscription) = {
            let state = self.state.lock().expect("callbacks");
            let Some(pending) = state.pending.get(response.id()) else {
                return false;
            };
            let Some(subscription) = state
                .clients
                .get(client)
                .and_then(|value| value.threads.get(&pending.thread))
            else {
                return false;
            };
            if pending.recipients.get(client) != Some(&subscription.id) {
                return false;
            }
            (pending.validity.clone(), subscription.clone())
        };
        // Native lifecycle -> interaction/cancellation -> broker is the same
        // order as physical publication. Retirement never waits for UI delivery.
        subscription
            .lifecycle
            .with_running_commit(|| match validity {
                Some(Validity::Interaction { owner, interaction }) => owner
                    .upgrade()
                    .and_then(|owner| {
                        owner.with_pending_interaction_header(&interaction, |_| {
                            self.claim(client, response)
                        })
                    })
                    .unwrap_or(false),
                Some(Validity::Tool(cancellation)) => cancellation
                    .with_active(|| self.claim(client, response))
                    .unwrap_or(false),
                None => self.claim(client, response),
            })
            .unwrap_or(false)
    }

    fn claim(&self, client: &str, response: ClientReply) -> bool {
        let mut state = self.state.lock().expect("callbacks");
        let id = response.id();
        let Some(pending) = state.pending.get(id) else {
            return false;
        };
        if !pending.recipients.contains_key(client)
            || !state.clients.get(client).is_some_and(|p| {
                !p.closed.is_cancelled()
                    && p.threads.get(&pending.thread).map(|value| &value.id)
                        == pending.recipients.get(client)
            })
        {
            return false;
        }
        let pending = state
            .pending
            .remove(id)
            .expect("pending acquired under one lock");
        Self::resolved(&state, id, &pending);
        // A valid correlated response is itself remote dispatch evidence.
        pending
            .dispatched
            .store(true, std::sync::atomic::Ordering::Release);
        let _ = pending.reply.send(response);
        true
    }

    pub(crate) fn resolve(&self, id: &RequestId) {
        let mut state = self.state.lock().expect("callbacks");
        if let Some(pending) = state.pending.remove(id) {
            Self::resolved(&state, id, &pending);
        }
    }

    fn resolved(state: &State, id: &RequestId, pending: &Pending) {
        let message = ServerMessage::Resolved {
            jsonrpc: JsonRpcVersion::V2,
            method: super::protocol::ServerResolvedMethod::Resolved,
            params: super::protocol::ServerRequestResolved {
                request_id: id.clone(),
                thread_id: pending.thread.clone(),
            },
        };
        for recipient in pending.recipients.keys() {
            if let Some(participant) = state.clients.get(recipient) {
                participant.waker.wake();
                if participant
                    .sender
                    .try_send(Queued {
                        message: message.clone(),
                        publication: None,
                    })
                    .is_err()
                {
                    participant.closed.cancel();
                    participant.overloaded.cancel();
                }
                participant.waker.wake();
            }
        }
    }
}

impl crate::tools::host::HostToolDispatcher for Arc<Callbacks> {
    fn call<'a>(
        &'a self,
        conversation: &'a ConversationId,
        attempt: &'a crate::runtime::identity::AttemptId,
        definition: &'a crate::tools::host::HostToolDefinition,
        invocation: crate::tools::ToolInvocation,
        cancellation: crate::runtime::cancellation::ExecutionCancellation,
    ) -> futures_util::future::BoxFuture<'a, crate::tools::ToolExecutionResult> {
        Box::pin(async move {
            use crate::tools::{ToolExecutionStatus as Status, host::result};
            let started = std::time::Instant::now();
            let Some(call_id) = invocation.id.canonical_call_id() else {
                return result(
                    Status::Failed {
                        error: "Host callbacks are not inherited by Workflow scopes".into(),
                    },
                    Vec::new(),
                    0,
                );
            };
            if cancellation.is_cancelled() {
                return result(
                    Status::Cancelled {
                        reason: cancellation.reason(),
                        phase: crate::tools::ToolCancellationPhase::BeforeStart,
                    },
                    Vec::new(),
                    0,
                );
            }
            let request = ServerRequestMethod::ToolCall(super::protocol::HostToolCall {
                thread_id: conversation.clone(),
                turn_id: attempt.clone(),
                call_id: call_id.clone(),
                namespace: definition.namespace.clone(),
                tool: definition.name.clone(),
                arguments: invocation.arguments,
            });
            let mut ticket = match cancellation.with_active(|| {
                self.request(
                    conversation.clone(),
                    request,
                    Some(Validity::Tool(cancellation.clone())),
                )
            }) {
                Some(Ok(ticket)) => ticket,
                None => {
                    return result(
                        Status::Cancelled {
                            reason: cancellation.reason(),
                            phase: crate::tools::ToolCancellationPhase::BeforeStart,
                        },
                        Vec::new(),
                        0,
                    );
                }
                Some(Err(error)) => {
                    return result(
                        Status::Failed {
                            error: error.into(),
                        },
                        Vec::new(),
                        0,
                    );
                }
            };
            let unknown = || {
                Status::OutcomeUnknown { detail: "application callbacks may have external effects; no authoritative valid result was obtained".into() }
            };
            let (status, content) = tokio::select! {
                biased;
                reply = &mut ticket.reply => match reply {
                    Ok(ClientReply::Success(reply)) => match serde_json::from_value::<super::protocol::HostToolResponse>(reply.result) {
                        Ok(response) => (
                            if response.success { Status::Success } else { Status::Failed { error: "application reported a failed tool result".into() } },
                            response.content_items.into_iter().map(|item| match item {
                                super::protocol::HostToolContent::Text { text } => crate::tools::ToolResultContent::Text(crate::message::content::TextBlock { text }),
                            }).collect(),
                        ),
                        Err(_) => (unknown(), Vec::new()),
                    },
                    Ok(ClientReply::Error(_)) | Err(_) => (unknown(), Vec::new()),
                },
                () = cancellation.cancelled() => (
                    if ticket.dispatched.load(std::sync::atomic::Ordering::Acquire) { unknown() }
                    else { Status::Cancelled { reason:cancellation.reason(), phase:crate::tools::ToolCancellationPhase::BeforeStart } },
                    Vec::new()),
            };
            // Ticket retirement synchronously clears callbacks before native
            // Tool Runtime validation, output bounds and result commitment.
            drop(ticket);
            result(
                status,
                content,
                u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_server::protocol::{ClientSuccess, HostToolCall};
    fn running_lifecycle() -> crate::runtime::types::ConversationLifecycle {
        let lifecycle = crate::runtime::types::ConversationLifecycle::new();
        assert!(lifecycle.activate());
        lifecycle
    }
    impl Callbacks {
        fn subscribe_test(self: &Arc<Self>, client: &str, thread: &ConversationId) {
            self.subscribe(client, thread, running_lifecycle());
        }
    }
    fn method(thread: ConversationId) -> ServerRequestMethod {
        ServerRequestMethod::ToolCall(HostToolCall {
            thread_id: thread,
            turn_id: crate::runtime::identity::AttemptId::new("attempt"),
            call_id: crate::runtime::identity::ToolCallId::new("call"),
            namespace: Some("business".into()),
            tool: "lookup".into(),
            arguments: serde_json::json!({}),
        })
    }
    fn reply(id: RequestId, value: serde_json::Value) -> ClientReply {
        ClientReply::Success(ClientSuccess {
            jsonrpc: JsonRpcVersion::V2,
            id,
            result: value,
        })
    }
    #[tokio::test]
    async fn broadcast_first_response_invalid_payload_disconnect_and_replay() {
        let callbacks = Arc::new(Callbacks::default());
        let thread = ConversationId::generate();
        let a = callbacks.connect();
        let b = callbacks.connect();
        let stranger = callbacks.connect();
        callbacks.subscribe_test(&a.id, &thread);
        callbacks.subscribe_test(&b.id, &thread);
        let mut ticket = callbacks
            .request(thread.clone(), method(thread.clone()), None)
            .unwrap();
        let ServerMessage::Request(first) = a.messages.lock().await.recv().await.unwrap().message
        else {
            panic!()
        };
        let ServerMessage::Request(second) = b.messages.lock().await.recv().await.unwrap().message
        else {
            panic!()
        };
        assert_eq!(first.id, second.id);
        assert!(!callbacks.respond(&stranger.id, reply(first.id.clone(), serde_json::json!({}))));
        callbacks.disconnect(&a.id);
        assert!(!callbacks.respond(&a.id, reply(first.id.clone(), serde_json::json!({}))));
        let resumed = callbacks.connect();
        callbacks.subscribe_test(&resumed.id, &thread);
        let ServerMessage::Request(replayed) =
            resumed.messages.lock().await.recv().await.unwrap().message
        else {
            panic!()
        };
        assert_eq!(replayed, first);
        // A syntactically valid but domain-invalid answer still wins.
        assert!(callbacks.respond(
            &b.id,
            reply(first.id.clone(), serde_json::json!("invalid domain"))
        ));
        assert!(!callbacks.respond(
            &resumed.id,
            reply(first.id, serde_json::json!({"success":true}))
        ));
        assert!(matches!(
            (&mut ticket.reply).await.unwrap(),
            ClientReply::Success(_)
        ));
        assert!(matches!(
            resumed
                .messages
                .lock()
                .await
                .recv()
                .await
                .map(|q| q.message),
            Some(ServerMessage::Resolved { .. })
        ));
        callbacks.unsubscribe(&resumed.id, &thread);
        callbacks.subscribe_test(&resumed.id, &thread);
        assert!(resumed.messages.lock().await.try_recv().is_err());
    }

    #[tokio::test]
    async fn reconnect_replays_the_entire_pending_bound_before_the_reader_runs() {
        let callbacks = Arc::new(Callbacks::default());
        let thread = ConversationId::generate();
        let original = callbacks.connect();
        callbacks.subscribe_test(&original.id, &thread);
        let mut tickets = Vec::new();
        for _ in 0..MAX_PENDING {
            tickets.push(
                callbacks
                    .request(thread.clone(), method(thread.clone()), None)
                    .unwrap(),
            );
            original.messages.lock().await.recv().await.unwrap();
        }
        callbacks.disconnect(&original.id);
        let resumed = callbacks.connect();
        callbacks.subscribe_test(&resumed.id, &thread);
        assert!(!resumed.overloaded.is_cancelled());
        let mut expected: std::collections::BTreeSet<_> =
            tickets.iter().map(|ticket| ticket.id.clone()).collect();
        let mut ids = Vec::new();
        for _ in 0..MAX_PENDING {
            let queued = resumed.messages.lock().await.recv().await.unwrap();
            let ServerMessage::Request(request) = queued.message else {
                panic!("same live request")
            };
            assert!(expected.remove(&request.id));
            ids.push(request.id);
            assert_eq!(queued.publication.unwrap().with_live(|| ()), Some(()));
        }
        assert!(expected.is_empty());
        for id in ids {
            assert!(callbacks.respond(&resumed.id, reply(id, serde_json::json!({}))));
        }
        for mut ticket in tickets {
            (&mut ticket.reply).await.unwrap();
        }
        assert!(!resumed.overloaded.is_cancelled());
        for _ in 0..MAX_PENDING {
            assert!(matches!(
                resumed.messages.lock().await.recv().await.unwrap().message,
                ServerMessage::Resolved { .. }
            ));
        }
        assert!(resumed.messages.lock().await.try_recv().is_err());
    }

    #[tokio::test]
    async fn retired_native_incarnation_is_fenced_before_closed_delivery() {
        let callbacks = Arc::new(Callbacks::default());
        let thread = ConversationId::generate();
        let old = callbacks.connect();
        let old_lifecycle = running_lifecycle();
        callbacks.subscribe(&old.id, &thread, old_lifecycle.clone());
        let old_ticket = callbacks
            .request(thread.clone(), method(thread.clone()), None)
            .unwrap();
        let old_queued = old.messages.lock().await.recv().await.unwrap();
        assert!(old_lifecycle.begin_drain());
        assert_eq!(old_queued.publication.unwrap().with_live(|| ()), None);
        assert!(!callbacks.respond(&old.id, reply(old_ticket.id.clone(), serde_json::json!({}))));
        drop(old_ticket);
        old.messages.lock().await.recv().await.unwrap(); // resolved, not native Closed
        let current = callbacks.connect();
        let next_lifecycle = crate::runtime::types::ConversationLifecycle::new();
        callbacks.subscribe(&current.id, &thread, next_lifecycle.clone());
        // Early MCP subscriptions remain registered before native activation.
        assert!(next_lifecycle.activate());
        let mut ticket = callbacks
            .request(thread.clone(), method(thread), None)
            .unwrap();
        assert!(old.messages.lock().await.try_recv().is_err());
        let queued = current.messages.lock().await.recv().await.unwrap();
        assert!(queued.publication.unwrap().with_live(|| ()).is_some());
        assert!(!callbacks.respond(&old.id, reply(ticket.id.clone(), serde_json::json!({}))));
        assert!(callbacks.respond(&current.id, reply(ticket.id.clone(), serde_json::json!({}))));
        (&mut ticket.reply).await.unwrap();
    }

    #[tokio::test]
    async fn first_rpc_error_wins_and_cancellation_retires() {
        let callbacks = Arc::new(Callbacks::default());
        let client = callbacks.connect();
        let thread = ConversationId::generate();
        callbacks.subscribe_test(&client.id, &thread);
        let mut ticket = callbacks
            .request(thread.clone(), method(thread.clone()), None)
            .unwrap();
        let error = ClientReply::Error(super::super::protocol::ClientError {
            jsonrpc: JsonRpcVersion::V2,
            id: ticket.id.clone(),
            error: super::super::protocol::ClientRpcError {
                code: -32000,
                message: "failed".into(),
                data: None,
            },
        });
        assert!(callbacks.respond(&client.id, error));
        assert!(!callbacks.respond(&client.id, reply(ticket.id.clone(), serde_json::json!({}))));
        assert!(matches!(
            (&mut ticket.reply).await.unwrap(),
            ClientReply::Error(_)
        ));
        let next = callbacks
            .request(thread.clone(), method(thread), None)
            .unwrap();
        let id = next.id.clone();
        drop(next);
        assert!(!callbacks.respond(&client.id, reply(id, serde_json::json!({}))));
    }
    #[tokio::test]
    async fn canceled_and_replaced_requests_are_fenced_at_carrier_acceptance() {
        use super::super::registration::Acceptance;
        use crate::runtime::cancellation::{CancellationSignal, ExecutionCancellation};
        use crate::runtime::types::CancellationReason;
        let callbacks = Arc::new(Callbacks::default());
        let client = callbacks.connect();
        let thread = ConversationId::generate();
        callbacks.subscribe_test(&client.id, &thread);
        let signal = CancellationSignal::new();
        let cancellation =
            ExecutionCancellation::detached(signal.clone(), CancellationReason::UserRequested);
        let ticket = callbacks
            .request(
                thread.clone(),
                method(thread.clone()),
                Some(Validity::Tool(cancellation)),
            )
            .unwrap();
        let queued = client.messages.lock().await.recv().await.unwrap();
        signal.cancel();
        assert!(matches!(
            queued.publication.unwrap().hand_off("record", |_| panic!(
                "canceled request must not reach carrier"
            )),
            Acceptance::<()>::Discarded
        ));
        assert!(!callbacks.respond(&client.id, reply(ticket.id.clone(), serde_json::json!({}))));
        drop(ticket);
        // A fresh subscription has a new fence even on the same connection.
        let ticket = callbacks
            .request(thread.clone(), method(thread.clone()), None)
            .unwrap();
        let queued = loop {
            let queued = client.messages.lock().await.recv().await.unwrap();
            if queued.publication.is_some() {
                break queued;
            }
        };
        callbacks.unsubscribe(&client.id, &thread);
        callbacks.subscribe_test(&client.id, &thread);
        assert!(matches!(
            queued.publication.unwrap().hand_off("record", |_| panic!(
                "replaced subscription must not reach carrier"
            )),
            Acceptance::<()>::Discarded
        ));
        let replay = client.messages.lock().await.recv().await.unwrap();
        assert!(matches!(
            replay
                .publication
                .unwrap()
                .hand_off("record", |_| std::task::Poll::Ready(Ok(()))),
            Acceptance::Accepted(_, ())
        ));
        drop(ticket);
    }

    #[tokio::test]
    async fn dispatch_cancellation_and_claim_order_preserve_only_known_evidence() {
        use crate::runtime::cancellation::{CancellationSignal, ExecutionCancellation};
        use crate::runtime::types::CancellationReason;
        use crate::tools::{
            ToolCancellationPhase, ToolExecutionStatus, ToolInvocation, ToolInvocationMode,
            host::{HostToolDefinition, HostToolDispatcher},
            types::ToolInvocationId,
        };
        for order in 0..3 {
            let callbacks = Arc::new(Callbacks::default());
            let client = callbacks.connect();
            let thread = ConversationId::generate();
            callbacks.subscribe_test(&client.id, &thread);
            let attempt = crate::runtime::identity::AttemptId::new("attempt");
            let definition = HostToolDefinition {
                namespace: Some("business".into()),
                name: "lookup".into(),
                description: String::new(),
                input_schema: serde_json::json!({"type":"object"}),
            };
            let signal = CancellationSignal::new();
            let cancellation =
                ExecutionCancellation::detached(signal.clone(), CancellationReason::UserRequested);
            let invocation = ToolInvocation {
                id: ToolInvocationId::Agent {
                    call_id: crate::runtime::identity::ToolCallId::new("call"),
                },
                tool_id: crate::runtime::identity::ToolId::new("host-test"),
                tool_name: "host-test".into(),
                mode: ToolInvocationMode::Foreground,
                arguments: serde_json::json!({}),
            };
            let mut operation =
                callbacks.call(&thread, &attempt, &definition, invocation, cancellation);
            assert!(futures_util::poll!(&mut operation).is_pending());
            let queued = client.messages.lock().await.recv().await.unwrap();
            let ServerMessage::Request(request) = queued.message else {
                panic!()
            };
            if order != 0 {
                assert!(matches!(
                    queued
                        .publication
                        .unwrap()
                        .hand_off("record", |_| std::task::Poll::Ready(
                            Ok::<_, std::io::Error>(())
                        )),
                    super::super::registration::Acceptance::Accepted(_, ())
                ));
            }
            if order == 2 {
                assert!(callbacks.respond(&client.id, reply(request.id.clone(), serde_json::json!({"success":true,"contentItems":[{"type":"inputText","text":"known"}]}))));
            }
            signal.cancel();
            let result = operation.await;
            match order {
                0 => assert!(matches!(
                    result.status,
                    ToolExecutionStatus::Cancelled {
                        phase: ToolCancellationPhase::BeforeStart,
                        ..
                    }
                )),
                1 => assert!(matches!(
                    result.status,
                    ToolExecutionStatus::OutcomeUnknown { .. }
                )),
                2 => assert!(matches!(result.status, ToolExecutionStatus::Success)),
                _ => unreachable!(),
            }
            assert!(!callbacks.respond(
                &client.id,
                reply(
                    request.id,
                    serde_json::json!({"success":true,"contentItems":[]})
                )
            ));
        }
    }

    #[tokio::test]
    async fn slow_clients_are_closed_without_stalling_other_subscribers() {
        let callbacks = Arc::new(Callbacks::default());
        let slow = callbacks.connect();
        let fast = callbacks.connect();
        let thread = ConversationId::generate();
        callbacks.subscribe_test(&slow.id, &thread);
        callbacks.subscribe_test(&fast.id, &thread);
        for _ in 0..=CLIENT_QUEUE {
            let mut ticket = callbacks
                .request(thread.clone(), method(thread.clone()), None)
                .unwrap();
            assert!(matches!(
                fast.messages.lock().await.recv().await.unwrap().message,
                ServerMessage::Request(_)
            ));
            assert!(callbacks.respond(&fast.id, reply(ticket.id.clone(), serde_json::json!({}))));
            (&mut ticket.reply).await.unwrap();
            assert!(matches!(
                fast.messages.lock().await.recv().await.unwrap().message,
                ServerMessage::Resolved { .. }
            ));
        }
        assert!(slow.overloaded.is_cancelled());
        assert!(!fast.overloaded.is_cancelled());
    }
}
