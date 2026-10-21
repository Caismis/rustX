//! Subscribers: consumed cursors into the one bounded replay ring plus wakeups.
use super::{
    Arc, Notify, RuntimeClientCursor, RuntimeClientError, RuntimeClientProjection,
    RuntimeClientProtocolEvent, RuntimeClientSnapshot, Subscriber, SubscriberPoll,
};

impl Drop for Subscriber {
    fn drop(&mut self) {
        // Wake a parked receiver on removal or destruction of the projection.
        // The receiver holds only a weak host reference and observes Closed.
        self.notify.notify_waiters();
        self.notify.notify_one();
    }
}

impl RuntimeClientProjection {
    /// The oldest cursor a subscription can still serve: one before the
    /// oldest retained replay entry, or the current cursor when the ring
    /// is empty.
    pub(super) fn earliest_serviceable(&self) -> RuntimeClientCursor {
        match self.replay.front() {
            Some((cursor, _)) => RuntimeClientCursor::new(cursor.get().saturating_sub(1)),
            None => self.cursor,
        }
    }

    /// Registers a subscriber consuming from `after_cursor` and returns its
    /// registration identity and wakeup handle.
    ///
    /// No events are copied into the subscriber: the bounded replay ring is
    /// the one retained backlog and the subscriber is a cursor into it.
    /// Serviceability is checked at registration under the caller's
    /// synchronization boundary, so a resume after a serviceable cursor has
    /// no gap and an expired cursor fails explicitly with
    /// `resync_required`.
    pub(crate) fn subscribe(
        &mut self,
        after_cursor: RuntimeClientCursor,
    ) -> Result<(u64, Arc<Notify>), RuntimeClientError> {
        if self.exhausted {
            return Err(RuntimeClientError::ProjectionExhausted);
        }
        if let Some(message) = &self.read_failure {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: message.clone(),
            });
        }
        let earliest = self.earliest_serviceable();
        if after_cursor > self.cursor || after_cursor < earliest {
            return Err(RuntimeClientError::ResyncRequired {
                after_cursor,
                earliest_serviceable: earliest,
            });
        }
        let subscriber_id = self.next_subscriber_id;
        self.next_subscriber_id = self
            .next_subscriber_id
            .checked_add(1)
            .ok_or(RuntimeClientError::ProjectionExhausted)?;
        let notify = Arc::new(Notify::new());
        self.subscribers.push(Subscriber {
            id: subscriber_id,
            consumed: after_cursor,
            notify: notify.clone(),
        });
        // A subscriber registered behind the current cursor already has
        // retained work: arm its first wakeup so a parked consumer never
        // waits for the next publication to observe the existing gap.
        if after_cursor < self.cursor {
            notify.notify_one();
        }
        Ok((subscriber_id, notify))
    }

    /// Polls one registered subscriber for the next retained event after
    /// its consumed cursor, advancing that cursor on delivery.
    ///
    /// This is the one delivery path: exactly one retained ring entry is
    /// handed out per call, so a consumer can never observe a
    /// non-contiguous cursor sequence. If the events the subscriber still
    /// needs were evicted, the poll reports [`SubscriberPoll::Lagged`]
    /// without advancing the cursor — the condition is stable and terminal
    /// until the client re-subscribes.
    pub(crate) fn poll_subscriber(&mut self, subscriber_id: u64) -> SubscriberPoll {
        if self.exhausted {
            return SubscriberPoll::Exhausted;
        }
        let earliest = self.earliest_serviceable();
        let Some(subscriber) = self
            .subscribers
            .iter_mut()
            .find(|subscriber| subscriber.id == subscriber_id)
        else {
            return SubscriberPoll::Closed;
        };
        let consumed = subscriber.consumed;
        if let Some(message) = &self.read_failure {
            return SubscriberPoll::Unavailable {
                message: message.clone(),
            };
        }
        if consumed >= self.cursor {
            return SubscriberPoll::Pending;
        }
        if consumed < earliest {
            return SubscriberPoll::Lagged {
                after_cursor: consumed,
                earliest_serviceable: earliest,
            };
        }
        let Some((cursor, event)) = self
            .replay
            .iter()
            .find(|(cursor, _)| *cursor > consumed)
            .map(|(cursor, event)| (*cursor, event.clone()))
        else {
            // Unreachable while `consumed < self.cursor` and
            // `consumed >= earliest_serviceable`; treated as lag rather
            // than as a silent skip.
            return SubscriberPoll::Lagged {
                after_cursor: consumed,
                earliest_serviceable: earliest,
            };
        };
        subscriber.consumed = cursor;
        SubscriberPoll::Event(RuntimeClientProtocolEvent { cursor, event })
    }

    /// Removes one registered subscriber (attachment detach, re-subscribe,
    /// or a dropped subscription handle) and wakes it so a parked consumer
    /// observes the closure instead of waiting forever.
    pub(crate) fn remove_subscriber(&mut self, subscriber_id: u64) {
        if let Some(index) = self
            .subscribers
            .iter()
            .position(|subscriber| subscriber.id == subscriber_id)
        {
            let subscriber = self.subscribers.remove(index);
            subscriber.notify.notify_one();
        }
    }

    /// The current cursor (host observability/tests).
    #[cfg(test)]
    pub(crate) fn cursor(&self) -> RuntimeClientCursor {
        self.cursor
    }

    /// Forces the cursor near exhaustion (overflow tests).
    #[cfg(test)]
    pub(crate) fn force_cursor_for_test(&mut self, value: u64) {
        self.cursor = RuntimeClientCursor::new(value);
    }

    /// The current snapshot read model reference (host/tests).
    pub(crate) fn snapshot_ref(&self) -> &RuntimeClientSnapshot {
        self.published_seed
            .as_ref()
            .map_or(&self.snapshot, |seed| &seed.snapshot)
    }

    /// The current snapshot read model reference, failing explicitly once
    /// the observation stream is exhausted.
    pub(crate) fn snapshot_ref_checked(
        &self,
    ) -> Result<&RuntimeClientSnapshot, RuntimeClientError> {
        if self.exhausted {
            return Err(RuntimeClientError::ProjectionExhausted);
        }
        if let Some(message) = &self.read_failure {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: message.clone(),
            });
        }
        Ok(self.snapshot_ref())
    }

    /// The retained replay ring (tests).
    #[cfg(test)]
    #[allow(dead_code)] // asserted by the bounded-replay regression tests
    pub(crate) fn replay_len(&self) -> usize {
        self.replay.len()
    }

    #[cfg(test)]
    pub(crate) fn probe_snapshot_enter(&self) {
        if let Some(probe) = &self.probe {
            probe.snapshot_enter();
        }
    }
}
