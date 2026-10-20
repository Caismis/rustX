//! Event observation and subscriptions over the projection replay ring.
use super::{
    Arc, AttachmentId, ClientInner, EventDelivery, EventSubscription, RuntimeClientCursor,
    RuntimeClientError, RuntimeClientResult, SubscriberPoll, SubscriptionInner,
};

impl Drop for SubscriptionInner {
    fn drop(&mut self) {
        if let Some(host) = self.host.upgrade() {
            let mut state = host.state.lock().expect("native projection lock");
            state.projection.remove_subscriber(self.subscriber_id);
        }
    }
}
impl core::fmt::Debug for EventSubscription {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("EventSubscription")
            .field("subscriber_id", &self.inner.subscriber_id)
            .finish()
    }
}

impl ClientInner {
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
                    let weight = crate::observation::bounded::bounded_bytes(
                        &event,
                        crate::observation::bounded::CHECKPOINT_TARGET_BYTES,
                    )
                    .ok_or_else(|| RuntimeClientError::InvalidState {
                        message: "projection published an unbounded transition".into(),
                    })?
                    .len();
                    if bytes + weight > crate::observation::bounded::CHECKPOINT_TARGET_BYTES {
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
                    if events.len() == crate::observation::bounded::CHECKPOINT_PAGE_ITEMS {
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
