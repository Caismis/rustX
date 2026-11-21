//! Bounded control queues, validated receipts, and independent lane workers.

use super::*;

/// The shared state behind the cloneable [`ControlHandle`]: the bounded
/// command queue, the one in-flight command awaiting its receipt, and
/// the terminal error every orphan waiter is resolved with.
pub(super) struct ControlShared {
    state: Mutex<ControlState>,
    ready: Condvar,
    capacity: Condvar,
    #[cfg(test)]
    pub(super) before_capacity_wait: Mutex<Option<mpsc::Sender<()>>>,
}

pub(super) struct ControlState {
    /// Queued commands not yet delivered to the peer — FIFO, one per
    /// answered poll.
    pub(super) queue: VecDeque<ControlCommand>,
    /// The command the last turn delivered, awaiting its receipt.
    inflight: Option<ControlCommand>,
    /// The worker's stop handle — set once at spawn.
    stop: Option<ChannelShutdown>,
    /// Whether [`ControlHandle::stop`] requested an orderly end — the
    /// worker exits without detaching the pair.
    stop_requested: bool,
    /// The terminal error new and orphan waiters resolve to once the
    /// lane is dead.
    closed: Option<io::Error>,
}

/// One queued or in-flight server command and its private waiter.
pub(super) struct ControlCommand {
    request: ControlRequest,
    responder: mpsc::Sender<io::Result<ControlReply>>,
}

impl ControlShared {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(ControlState {
                queue: VecDeque::new(),
                inflight: None,
                stop: None,
                stop_requested: false,
                closed: None,
            }),
            ready: Condvar::new(),
            capacity: Condvar::new(),
            #[cfg(test)]
            before_capacity_wait: Mutex::new(None),
        }
    }

    pub(super) fn lock(&self) -> MutexGuard<'_, ControlState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Queues `request` and returns the receipt future — refused
    /// immediately when the bounded queue is full or the lane is dead.
    pub(super) fn enqueue(
        &self,
        request: ControlRequest,
    ) -> io::Result<mpsc::Receiver<io::Result<ControlReply>>> {
        self.enqueue_waiting(request, None)
    }

    fn enqueue_waiting(
        &self,
        request: ControlRequest,
        deadline: Option<Instant>,
    ) -> io::Result<mpsc::Receiver<io::Result<ControlReply>>> {
        let refused =
            || io::Error::new(io::ErrorKind::WouldBlock, "control command budget is full");
        let mut state = self.lock();
        loop {
            if let Some(closed) = &state.closed {
                return Err(io::Error::new(closed.kind(), "control lane is closed"));
            }
            if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
                return Err(refused());
            }
            let in_flight = state.queue.len() + usize::from(state.inflight.is_some());
            if in_flight < MAX_CONTROL_COMMANDS {
                let (responder, receipt) = mpsc::channel();
                state.queue.push_back(ControlCommand { request, responder });
                self.ready.notify_one();
                return Ok(receipt);
            }
            let remaining = deadline
                .ok_or_else(refused)?
                .saturating_duration_since(Instant::now());
            #[cfg(test)]
            if let Some(entered) = self.before_capacity_wait.lock().unwrap().take() {
                let _ = entered.send(());
            }
            state = self
                .capacity
                .wait_timeout(state, remaining)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Delivers the poll's receipt to the in-flight waiter. Every legal
    /// shape is checked against the command it answers: a receipt with
    /// no in-flight command, a missing receipt for a delivered one, or a
    /// receipt the wrong shape or echo for its request is a protocol
    /// violation that fails the lane, not just the waiter.
    pub(super) fn resolve(&self, receipt: Option<ControlReply>) -> bool {
        let mut state = self.lock();
        let command = state.inflight.take();
        if command.is_some() {
            self.capacity.notify_all();
        }
        match (command, receipt) {
            (None, None) => true,
            (Some(command), Some(reply)) => {
                if receipt_matches(&command.request, &reply) {
                    let _ = command.responder.send(Ok(reply));
                    true
                } else {
                    // The dishonest receipt still consumes the command:
                    // the waiter learns the lane failed, never a false
                    // success.
                    let _ = command.responder.send(Err(io::Error::new(
                        io::ErrorKind::InvalidData,
                        "control receipt does not match its request",
                    )));
                    false
                }
            }
            (Some(command), None) => {
                let _ = command.responder.send(Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "control poll carries no receipt for a delivered request",
                )));
                false
            }
            (None, Some(_)) => false,
        }
    }

    /// Moves the next queued command into the in-flight slot — only
    /// while nothing is in flight: at most one server command is ever
    /// outstanding on the lane.
    fn next_command(&self) -> Option<ControlRequest> {
        let mut state = self.lock();
        if state.inflight.is_some() {
            return None;
        }
        state.inflight = state.queue.pop_front();
        state
            .inflight
            .as_ref()
            .map(|command| command.request.clone())
    }

    pub(super) fn wait_next_command(&self) -> Option<ControlRequest> {
        let mut state = self
            .ready
            .wait_while(self.lock(), |state| {
                state.closed.is_none() && state.queue.is_empty()
            })
            .unwrap_or_else(PoisonError::into_inner);
        if state.closed.is_some() {
            return None;
        }
        state.inflight = state.queue.pop_front();
        state
            .inflight
            .as_ref()
            .map(|command| command.request.clone())
    }

    pub(super) fn reject(&self, error: io::Error) {
        if let Some(command) = self.lock().inflight.take() {
            let _ = command.responder.send(Err(error));
            self.capacity.notify_all();
        }
    }

    /// Arms the worker's stop handle — once, at spawn.
    pub(super) fn arm_stop(&self, stop: ChannelShutdown) {
        self.lock().stop = Some(stop);
    }

    /// Orderly stop: the worker exits without detaching the pair —
    /// the pair's own teardown is the caller's. Every waiter resolves
    /// closed and the worker's read wakes.
    fn close(&self, error: io::Error) {
        let mut state = self.lock();
        state.stop_requested = true;
        Self::drain(&mut state, error);
        self.capacity.notify_all();
        self.ready.notify_one();
        if let Some(stop) = state.stop.take() {
            stop.shutdown();
        }
    }

    /// The worker's teardown: resolve every orphan waiter — commands
    /// never delivered and the in-flight one never answered — then mark
    /// the lane closed so late callers fail instead of queueing. This
    /// is never an orderly stop: the pair's fate is decided by the
    /// caller that observes the lane ended.
    pub(super) fn terminate(&self, error: io::Error) {
        let mut state = self.lock();
        Self::drain(&mut state, error);
        self.capacity.notify_all();
    }

    /// Resolves every outstanding waiter with `error` and marks the
    /// lane closed — under the caller's existing hold.
    fn drain(state: &mut ControlState, error: io::Error) {
        state.closed.get_or_insert(error);
        for command in state.queue.drain(..).chain(state.inflight.take()) {
            let _ = command.responder.send(Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "control lane is closed",
            )));
        }
    }

    /// Whether the shutdown was requested through [`Self::close`].
    fn stop_requested(&self) -> bool {
        self.lock().stop_requested
    }
}

/// The cloneable Server command path on the paired control lane —
/// independent of the data lane's dispatch lock. `health` and
/// `cancel_mutation` queue bounded commands delivered on the peer's own
/// turn-taking polls and return only verified receipts;
/// the lane ends with the connection's own lifecycle teardown. Every
/// clone shares the one lane: at
/// most [`MAX_CONTROL_COMMANDS`] queued plus in-flight commands, probes
/// included.
#[derive(Clone)]
pub struct ControlHandle {
    pub(super) shared: Arc<ControlShared>,
}

impl ControlHandle {
    /// A liveness round trip on the control lane: the fresh nonce must
    /// come back echoed — never a replayed proof.
    ///
    /// # Errors
    ///
    /// Returns the queue-full [`io::ErrorKind::WouldBlock`], the closed
    /// lane's terminal error, [`io::ErrorKind::TimedOut`] past the
    /// command budget, or the worker's protocol-violation error.
    pub fn health(&self) -> io::Result<()> {
        let mut nonce = [0u8; 8];
        getrandom::fill(&mut nonce).map_err(io::Error::other)?;
        let nonce = DecimalU64::new(u64::from_be_bytes(nonce));
        let receipt = self.shared.enqueue(ControlRequest::Health { nonce })?;
        match receipt.recv_timeout(CONTROL_COMMAND_TIMEOUT) {
            Ok(Ok(ControlReply::Healthy { nonce: echoed })) if echoed == nonce => Ok(()),
            Ok(Ok(_)) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "control health receipt echoes a foreign nonce",
            )),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "control command outlived its budget",
            )),
        }
    }

    /// Cancels the live mutation commit for `ticket` on the Client:
    /// the disposition is the Client's own arbitration — a
    /// [`CancelDisposition::CancelledBeforeCommit`] proves the publish
    /// never crossed, [`CancelDisposition::AlreadyCrossed`] that it had,
    /// and [`CancelDisposition::Unknown`] that no live ticket matched.
    /// `Unknown` is returned as-is — never retried.
    ///
    /// # Errors
    ///
    /// The same error kinds as [`Self::health`].
    pub fn cancel_mutation(&self, ticket: DecimalU64) -> io::Result<CancelDisposition> {
        self.cancel_mutation_by(ticket, Instant::now() + CONTROL_COMMAND_TIMEOUT)
    }

    /// Cancels a mutation within one absolute budget for queue admission and receipt.
    ///
    /// # Errors
    ///
    /// Returns the same admission and receipt errors as [`Self::cancel_process_by`].
    pub fn cancel_mutation_by(
        &self,
        ticket: DecimalU64,
        deadline: Instant,
    ) -> io::Result<CancelDisposition> {
        let receipt = self.enqueue_until(&ControlRequest::CancelMutation { ticket }, deadline)?;
        match receipt.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Ok(ControlReply::MutationCancelled {
                ticket: echoed,
                disposition,
            })) if echoed == ticket => Ok(disposition),
            Ok(Ok(_)) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "control cancel receipt answers a foreign ticket",
            )),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "control command outlived its budget",
            )),
        }
    }

    /// Cancels one live process operation on the Client by its complete
    /// operation key: the disposition is the Client's own arbitration —
    /// [`ProcessCancelDisposition::CancelledBeforeStart`] proves zero
    /// business spawn, [`ProcessCancelDisposition::CancellationRequested`]
    /// that containment is physically underway (never already complete),
    /// and `Unknown` that no record matched the exact key.
    ///
    /// # Errors
    ///
    /// The same error kinds as [`Self::health`].
    pub fn cancel_process(
        &self,
        key: &rustx_execution_protocol::process::ProcessOperationKey,
    ) -> io::Result<ProcessCancelDisposition> {
        self.cancel_process_by(key, Instant::now() + CONTROL_COMMAND_TIMEOUT)
    }

    /// [`Self::cancel_process`] under one absolute `deadline` covering both
    /// the queue wait and the correlated receipt. Only a refusal the queue
    /// returns before anything is queued — the bounded budget being full — is
    /// waited on inside the deadline; a dead lane fails at once. A
    /// budget still full at the deadline is an explicit
    /// [`io::ErrorKind::WouldBlock`]: the command was never queued.
    ///
    /// # Errors
    ///
    /// [`io::ErrorKind::WouldBlock`] when the budget stayed full to the
    /// deadline, the closed lane's terminal error, or
    /// [`io::ErrorKind::TimedOut`] when the receipt missed the deadline.
    pub fn cancel_process_by(
        &self,
        key: &rustx_execution_protocol::process::ProcessOperationKey,
        deadline: Instant,
    ) -> io::Result<ProcessCancelDisposition> {
        let receipt = self.enqueue_until(
            &ControlRequest::CancelProcess { key: key.clone() },
            deadline,
        )?;
        match receipt.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Ok(ControlReply::ProcessCancelled {
                key: echoed,
                disposition,
            })) if echoed == *key => Ok(disposition),
            Ok(Ok(_)) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "control cancel receipt answers a foreign process key",
            )),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "control command outlived its budget",
            )),
        }
    }

    fn enqueue_until(
        &self,
        request: &ControlRequest,
        deadline: Instant,
    ) -> io::Result<mpsc::Receiver<io::Result<ControlReply>>> {
        self.shared.enqueue_waiting(request.clone(), Some(deadline))
    }

    /// Retire one exact native file observer without acquiring the data lock.
    ///
    /// # Errors
    ///
    /// A lost, late, or negative receipt leaves physical cleanup unconfirmed.
    pub fn stop_watch(&self, id: WatchId) -> io::Result<()> {
        let deadline = Instant::now() + CONTROL_COMMAND_TIMEOUT;
        let receipt = self.enqueue_until(&ControlRequest::StopWatch { id }, deadline)?;
        match receipt.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Ok(ControlReply::WatchStopped {
                id: echoed,
                settled: true,
            })) if echoed == id => Ok(()),
            Ok(Err(error)) => Err(error),
            Ok(Ok(_)) => Err(io::Error::other("native watch cleanup was not confirmed")),
            Err(_) => Err(io::ErrorKind::TimedOut.into()),
        }
    }

    /// Requests terminal revocation from the Client. Its local commits
    /// are fenced before `Revoked` is echoed. The connection owner
    /// applies the local binding fence and closes the data transport.
    ///
    /// # Errors
    ///
    /// The same error kinds as [`Self::health`]. A failed request leaves
    /// Client settlement unconfirmed, so the owner must still fence.
    pub fn request_revoke(&self) -> io::Result<()> {
        self.request_revoke_by(Instant::now() + CONTROL_COMMAND_TIMEOUT)
    }

    /// Requests revocation using one budget for both admission and settlement.
    ///
    /// # Errors
    ///
    /// Returns the same admission and receipt errors as [`Self::cancel_process_by`].
    pub fn request_revoke_by(&self, deadline: Instant) -> io::Result<()> {
        let receipt = self.enqueue_until(&ControlRequest::Revoke, deadline)?;
        match receipt.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(Ok(ControlReply::Revoked)) => Ok(()),
            Ok(Ok(_)) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "control revoke acknowledged with a foreign receipt",
            )),
            Ok(Err(error)) => Err(error),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "control command outlived its budget",
            )),
        }
    }

    /// Whether the lane is still delivering commands.
    #[must_use]
    pub fn is_live(&self) -> bool {
        self.shared.lock().closed.is_none()
    }

    /// Orderly stop: waiters resolve closed and the worker's read wakes.
    /// The pair's teardown is the connection's.
    pub(super) fn stop(&self) {
        self.shared.close(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "control lane is closed",
        ));
    }
}

/// The control worker's ownership: the handle for callers and the
/// bounded join for teardown.
pub(super) struct ControlWorker {
    pub(super) handle: ControlHandle,
    pub(super) join: JoinHandle<()>,
}

impl ControlWorker {
    /// Spawns the one bounded worker that owns the committed control
    /// channel: it reads [`ControlPoll`]s, resolves the in-flight
    /// command's waiter from each receipt, and answers with
    /// [`ControlTurn`]s carrying the next queued command. Every frame
    /// runs under a per-frame absolute deadline; the lane's own
    /// shutdown handle wakes a blocked read on stop or fence.
    pub(super) fn spawn(
        mut channel: RecordChannel,
        owner: Arc<ConnectionOwner>,
        connection: ConnectionId,
        shared: Arc<ControlShared>,
    ) -> Self {
        if let Ok(stop) = channel.shutdown_handle() {
            shared.arm_stop(stop);
        }
        let worker_shared = shared.clone();
        let worker_owner = owner;
        let join = std::thread::Builder::new()
            .name("execution-control".to_owned())
            .spawn(move || {
                let mut next_sequence = 1u64;
                loop {
                    let deadline = Instant::now() + CONTROL_FRAME_TIMEOUT;
                    let poll: ControlPoll = match channel.receive(Some(deadline)) {
                        Ok(poll) => poll,
                        Err(_) => break,
                    };
                    // The client's own sequence must advance exactly:
                    // a skip or replay is a protocol violation, not a
                    // dropped frame to tolerate.
                    if poll.sequence.get() != next_sequence {
                        break;
                    }
                    next_sequence = next_sequence.saturating_add(1);
                    if !shared.resolve(poll.receipt) {
                        break;
                    }
                    let request = shared.next_command();
                    let turn = ControlTurn {
                        sequence: poll.sequence,
                        health: poll.health,
                        request,
                    };
                    let Ok(encoded) = encode_record(&turn) else {
                        break;
                    };
                    if channel.send(&encoded, Some(deadline)).is_err() {
                        break;
                    }
                }
                shared.terminate(io::Error::new(
                    io::ErrorKind::BrokenPipe,
                    "control lane ended",
                ));
                if !shared.stop_requested() {
                    // A lane that ended without an orderly stop detaches
                    // the pair: the seized data handle interrupts any
                    // blocked exchange, and no dispatch starts again.
                    worker_owner.detach_control(&connection).shutdown();
                }
            })
            .expect("the bounded control worker spawns");
        Self {
            handle: ControlHandle {
                shared: worker_shared,
            },
            join,
        }
    }

    /// Orderly stop: close the lane, wake the worker, join it. The join
    /// is bounded — the worker's own frame deadline plus the shutdown
    /// it just received — and the caller never holds a global lock.
    pub(super) fn stop(self) {
        self.handle.stop();
        let _ = self.join.join();
    }
}

/// Whether a poll receipt is the exact answer the in-flight request
/// requires — shape and echo both.
pub(super) fn receipt_matches(request: &ControlRequest, reply: &ControlReply) -> bool {
    match (request, reply) {
        (ControlRequest::Health { nonce }, ControlReply::Healthy { nonce: echoed }) => {
            nonce == echoed
        }
        (
            ControlRequest::CancelMutation { ticket },
            ControlReply::MutationCancelled { ticket: echoed, .. },
        ) => ticket == echoed,
        (
            ControlRequest::CancelProcess { key },
            ControlReply::ProcessCancelled { key: echoed, .. },
        ) => key == echoed,
        (ControlRequest::Revoke, ControlReply::Revoked) => true,
        (ControlRequest::StopWatch { id }, ControlReply::WatchStopped { id: echoed, .. }) => {
            id == echoed
        }
        _ => false,
    }
}
