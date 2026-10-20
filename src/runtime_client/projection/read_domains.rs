//! Durable read-domain cuts (transcript, occupancy, Trace, projection head)
//! installed off-lock and fenced by revision.
#[cfg(test)]
use super::ConversationObservation;
#[cfg(test)]
use super::InboundItem;
#[cfg(test)]
use super::RuntimeClientCursor;
#[cfg(test)]
use super::RuntimeClientError;
#[cfg(test)]
use super::RuntimeClientSnapshot;
#[cfg(test)]
use super::SnapshotCandidate;
#[cfg(test)]
use super::views::inbound_item_view;
use super::{
    ReadDomainCut, ReadDomainValues, ReadDomains, RuntimeClientEvent, RuntimeClientProjection,
};

impl ReadDomainCut {
    pub(crate) fn read(&self, store: &dyn crate::durable::ConversationStore) -> ReadDomains {
        let mut page = if let Some(cut) = &self.source_cut {
            crate::observation::transcript::transcript_header_view(store.transcript_header_page(
                cut,
                None,
                super::super::checkpoint::CHECKPOINT_NATIVE_HEAD_ITEMS,
            )?)
        } else {
            return Err(crate::durable::ConversationStoreError::InvalidReference(
                "native read-domain source cut unavailable".into(),
            ));
        };
        super::super::response::decorate_through(store, &mut page, self.through)?;
        let occupancy = crate::observation::occupancy::read(store, self.through)?;
        let trace_cut = self
            .source_cut
            .as_ref()
            .ok_or(crate::durable::ConversationStoreError::ObservationCutExpired)?;
        let mut trace = super::super::trace::TraceProjection::at_cut(store, trace_cut)?
            .page(None, super::super::trace::TRACE_PAGE_LIMIT)?;
        trace.read_cut = None;
        let trace_total = store.observation_trace_total(self.through)?;
        Ok(ReadDomainValues {
            projection_head: projection_head_view(store, self.through)?,
            transcript: page,
            occupancy,
            trace,
            trace_total,
        })
    }
}
#[cfg(test)]
impl SnapshotCandidate {
    pub(crate) fn read_domains(
        &self,
        store: &dyn crate::durable::ConversationStore,
    ) -> ReadDomains {
        self.read_cut.read(store)
    }

    pub(crate) fn materialize(
        mut self,
        store: &dyn crate::durable::ConversationStore,
    ) -> Result<Self, RuntimeClientError> {
        let domains = self
            .read_domains(store)
            .map_err(|e| RuntimeClientError::RuntimeFailure {
                message: e.to_string(),
            })?;
        self.snapshot.transcript = domains.transcript;
        self.snapshot.context.occupancy = domains.occupancy;
        self.snapshot.context.projection_head = domains.projection_head;
        self.snapshot.trace = domains.trace;
        Ok(self)
    }
}
pub(super) fn projection_head_view(
    store: &dyn crate::durable::ConversationStore,
    through: u64,
) -> Result<
    Option<crate::observation::head::RuntimeClientProjectionHeadView>,
    crate::durable::ConversationStoreError,
> {
    store.observation_projection_head(through)
}

impl RuntimeClientProjection {
    /// Capture one semantic cut. Completion never consults the moving live head.
    #[cfg(test)]
    pub(crate) fn snapshot_candidate(&self) -> Result<SnapshotCandidate, RuntimeClientError> {
        #[cfg(test)]
        self.probe_snapshot_enter();
        self.capture_candidate()
    }

    pub(crate) fn read_domain_candidate(&self) -> Option<ReadDomainCut> {
        self.read_domain_cut()
    }

    #[cfg(test)]
    pub(super) fn capture_candidate(&self) -> Result<SnapshotCandidate, RuntimeClientError> {
        if self.exhausted {
            return Err(RuntimeClientError::ProjectionExhausted);
        }
        Ok(SnapshotCandidate {
            snapshot: self.checkpoint_candidate()?.snapshot,
            cursor: self.cursor,
            trace_through: self.journal_through,
            read_cut: self.read_domain_fence(),
        })
    }

    /// Capture a finite derived cut; Store work must happen outside the host lock.
    pub(crate) fn read_domain_fence(&self) -> ReadDomainCut {
        ReadDomainCut {
            revision: self.read_revision,
            through: self.read_through,
            source_cut: self.working_read_cut().source_cut,
        }
    }

    #[cfg(test)]
    pub(crate) fn request_read_domains_for_test(&mut self) {
        self.read_domains_dirty = true;
    }

    /// An explicit checkpoint may request one retry of a failed derived read.
    /// Native prefix/layout failures require their own original receipt repair.
    pub(crate) fn request_failed_read_repair(&mut self) -> Option<(ReadDomainCut, bool)> {
        if self.exhausted || self.read_failure.is_none() || self.validate_source_prefix().is_err() {
            return None;
        }
        let newly_requested = !self.read_domains_dirty;
        self.read_domains_dirty = true;
        self.read_domain_cut().map(|cut| (cut, newly_requested))
    }

    pub(crate) fn read_domain_cut(&self) -> Option<ReadDomainCut> {
        (self.read_domains_dirty && !self.exhausted && !self.native_source_pending())
            .then(|| self.read_domain_fence())
    }

    /// Installation is the linearization point. A newer semantic cut retires
    /// both successful and failed reads, leaving the newest cut dirty for repair.
    pub(crate) fn install_read_domains(
        &mut self,
        cut: &ReadDomainCut,
        read: ReadDomains,
        publish: bool,
    ) -> bool {
        if self.exhausted
            || self.read_domain_fence() != *cut
            || (!self.read_domains_dirty && self.read_failure.is_none())
        {
            return false;
        }
        let ReadDomainValues {
            projection_head,
            transcript,
            occupancy,
            mut trace,
            trace_total,
        } = match read {
            Ok(value) => value,
            Err(error) => {
                self.read_domains_dirty = false;
                let mut message = error.to_string();
                if message.len() > 1024 {
                    let mut end = 1024;
                    while !message.is_char_boundary(end) {
                        end -= 1;
                    }
                    message.truncate(end);
                }
                self.read_failure = Some(message);
                self.wake_subscribers();
                return true;
            }
        };
        let recovered_failure = self.read_failure.take().is_some();
        if recovered_failure {
            self.wake_subscribers();
        }
        self.read_domains_dirty = false;
        super::super::trace::repair_records(&mut trace.records, &self.snapshot);
        // A read result has not been published yet. Compare its contents using
        // the cache's existing cut so an unchanged installation stays a no-op.
        trace.read_cut.clone_from(&self.snapshot.trace.read_cut);
        self.source_frontiers.insert(
            "trace.records".into(),
            super::super::trace::checkpoint_frontier(
                trace_total,
                trace.records.len() as u64,
                trace.next_cursor.as_ref(),
            ),
        );
        if recovered_failure
            || self.snapshot.transcript != transcript
            || self.snapshot.context.occupancy != occupancy
            || self.snapshot.context.projection_head != projection_head
            || self.snapshot.trace != trace
        {
            trace.read_cut = None;
            self.snapshot.transcript.clone_from(&transcript);
            if self.snapshot.trace.records != trace.records {
                self.trace_head = super::super::checkpoint::PreparedTraceHead::default();
            }
            self.snapshot.trace.clone_from(&trace);
            self.snapshot.context.occupancy.clone_from(&occupancy);
            self.snapshot
                .context
                .projection_head
                .clone_from(&projection_head);
            if publish {
                self.publish(RuntimeClientEvent::ReadDomainsUpdated {
                    projection_head,
                    transcript,
                    trace,
                    occupancy,
                    todos: self.snapshot.todos.clone(),
                });
            }
        }
        true
    }

    pub(crate) fn initialize_read_domains(
        &mut self,
        store: &dyn crate::durable::ConversationStore,
    ) {
        self.read_domains_dirty = true;
        if let Some(cut) = self.read_domain_cut() {
            self.install_read_domains(&cut, cut.read(store), false);
        }
    }

    /// The snapshot and its cursor, linearized together.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeClientError::ProjectionExhausted`] once the cursor
    /// space is exhausted: after that point the projection can no longer
    /// fold authoritative transitions, so returning a stale read model
    /// would silently hide dropped observations.
    #[cfg(test)]
    pub(crate) fn snapshot(
        &self,
    ) -> Result<(RuntimeClientSnapshot, RuntimeClientCursor), RuntimeClientError> {
        #[cfg(test)]
        self.probe_snapshot_enter();
        if self.exhausted {
            return Err(RuntimeClientError::ProjectionExhausted);
        }
        if let Some(message) = &self.read_failure {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: message.clone(),
            });
        }
        Ok((self.snapshot_ref().clone(), self.cursor))
    }

    /// Captured under the same host lock as the snapshot and subscription.
    #[cfg(test)]
    pub(crate) fn snapshot_cut(
        &self,
    ) -> Result<(RuntimeClientSnapshot, RuntimeClientCursor, u64), RuntimeClientError> {
        let (snapshot, cursor) = self.snapshot()?;
        Ok((snapshot, cursor, self.journal_through))
    }

    /// Repairs missed committed pending notifications from durable authority.
    #[cfg(test)]
    pub(crate) fn repair_pending(&mut self, items: Vec<InboundItem>) {
        let pending: Vec<_> = items.iter().map(inbound_item_view).collect();
        if self.snapshot.inbound.pending != pending {
            self.apply(ConversationObservation::PendingInboundChanged(items));
        }
    }
}
