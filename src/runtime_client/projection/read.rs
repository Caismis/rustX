//! The read interface over source-owned cuts: checkpoint seeds and cuts, native
//! transcript/journal source prefixes, domain sources and checkpoint pages.
#[cfg(test)]
use super::AttemptId;
#[cfg(test)]
use super::ContentBlockIndex;
#[cfg(test)]
use super::ForegroundToolState;
use super::{
    Arc, CheckpointSeed, InboundDrainView, InboundItemView, MessageBlock, MessageId,
    RuntimeClientCursor, RuntimeClientError, RuntimeClientEvent, RuntimeClientProjection,
    RuntimeClientTranscriptCursor,
};

impl RuntimeClientProjection {
    pub(crate) fn set_inspection_frontier(&mut self, through: u64) {
        self.journal_through = through;
        self.native_journal_highwater = through;
        self.read_through = through;
    }

    pub(crate) fn checkpoint_seed(&self) -> Arc<CheckpointSeed> {
        self.published_seed
            .as_ref()
            .map_or_else(|| Arc::new(self.capture_working_seed()), Arc::clone)
    }

    pub(super) fn capture_working_seed(&self) -> CheckpointSeed {
        CheckpointSeed {
            snapshot: self.snapshot.clone(),
            capability_head: self.capability_head.clone(),
            trace_head: self.trace_head.clone(),
            cursor: self.cursor,
            read_cut: self.working_read_cut(),
            exhausted: self.exhausted,
            journal_failure: self.source_state.journal_failure,
            source_pending: !self.source_state.source_layout_failure
                && !self.source_state.journal_failure
                && (self.native_source_pending()
                    || (self.source_state.source_unavailable && self.source_cut.is_some())),
            source_cut: self.source_cut.clone(),
            head_bindings: self.head_bindings.clone(),
            foreground_content: self.foreground_content.clone(),
            source_frontiers: self.source_frontiers.clone(),
            collection_cuts: self.collection_cuts.clone(),
            domain_revisions: self.domain_revisions.clone(),
            content_metadata: self.content_metadata.clone(),
            publication_start: self.publication_start.clone(),
            live_frontiers: self.live_frontiers.clone(),
            live_block_total: self.live_block_total,
            live_tool_total: self.live_tool_total,
            canonical_foreground: self.canonical_foreground.clone(),
        }
    }

    pub(crate) fn checkpoint_candidate(
        &self,
    ) -> Result<super::super::checkpoint::RuntimeClientCheckpoint, RuntimeClientError> {
        self.checkpoint_seed().checkpoint_candidate()
    }

    pub(crate) fn checkpoint_read_cut(&self) -> crate::observation::cut::CheckpointReadCut {
        self.published_seed
            .as_ref()
            .map_or_else(|| self.working_read_cut(), |seed| seed.read_cut.clone())
    }

    pub(super) fn working_read_cut(&self) -> crate::observation::cut::CheckpointReadCut {
        crate::observation::cut::CheckpointReadCut {
            canonical_foreground: self.canonical_foreground.clone(),
            live_publication: self.current_live_cut(),
            conversation_id: self.snapshot.conversation_id.clone(),
            activation_id: self.activation_id.clone(),
            revision: self.cursor.get(),
            through: self.read_through,
            collection_cuts: self.collection_cuts.clone(),
            domain_revisions: self
                .domain_revisions
                .iter()
                .map(|(key, revision)| ((*key).to_owned(), *revision))
                .collect(),
            source_cut: (!self.source_state.source_unavailable
                && self.validate_source_prefix().is_ok())
            .then_some(self.source_cut.as_ref())
            .flatten()
            .map(|cut| {
                let mut cut = cut.clone();
                cut.read.journal = self.read_through;
                cut.read.ledger_through = cut.ledger_through;
                cut.read.surface_revision = cut.surface_revision;
                cut.read.transcript_total = cut.transcript_total;
                cut
            }),
        }
    }

    pub(crate) fn install_content_metadata(
        &mut self,
        references: Vec<crate::observation::content::ContentRef>,
    ) {
        for reference in references {
            if let crate::observation::content::ContentSource::CanonicalMessageBlock {
                component,
                locator,
                block_index,
            } = &reference.source
            {
                let key = (
                    component.key(),
                    locator.key().into_owned(),
                    block_index.get(),
                    reference.representation.key(),
                );
                self.content_metadata.insert(key, Arc::new(reference));
            }
        }
        // This cache contains finite read evidence only. The immutable Store is
        // the authority, and omitted indexes are materialized outside this lock.
        while self.content_metadata.len() > 4096 {
            let unneeded = self
                .content_metadata
                .keys()
                .find(|(_, message, _, _)| {
                    !self
                        .snapshot
                        .messages
                        .iter()
                        .take(crate::observation::bounded::CHECKPOINT_PAGE_ITEMS)
                        .any(|block| block.id().as_str() == message)
                })
                .cloned();
            match unneeded {
                Some(key) => {
                    self.content_metadata.remove(&key);
                }
                None => break,
            }
        }
    }

    /// Installs a finite native descriptor head at the frozen bootstrap cut.
    /// Called during construction, before the Projection synchronization boundary exists.
    pub(crate) fn initialize_descriptor_heads(
        &mut self,
        store: &dyn crate::durable::ConversationStore,
        cut: crate::durable::observation::ObservationReadCut,
    ) -> Result<(), RuntimeClientError> {
        let messages = store
            .message_header_page(
                &cut,
                None,
                super::super::checkpoint::CHECKPOINT_NATIVE_HEAD_ITEMS,
            )
            .map_err(|error| RuntimeClientError::ObservationUnavailable {
                message: error.to_string(),
            })?;
        let transcript = store
            .transcript_header_page(
                &cut,
                None,
                super::super::checkpoint::CHECKPOINT_NATIVE_HEAD_ITEMS,
            )
            .map_err(|error| RuntimeClientError::ObservationUnavailable {
                message: error.to_string(),
            })?;
        self.head_bindings = messages
            .headers
            .iter()
            .filter_map(|header| {
                header
                    .identity
                    .as_ref()
                    .map(|identity| (identity.clone(), header.content.clone()))
            })
            .collect();
        self.snapshot.message_descriptors = messages
            .headers
            .iter()
            .filter_map(crate::observation::transcript::RuntimeClientMessageDescriptor::from_header)
            .collect();
        self.snapshot.messages = messages
            .headers
            .iter()
            .filter_map(|header| header.display.clone())
            .collect();
        self.snapshot.transcript =
            crate::observation::transcript::transcript_header_view(transcript);
        self.snapshot.publication_settlements = store
            .publication_settlement_head(&cut)
            .map_err(RuntimeClientError::observation_store_error)?;
        let references = messages
            .headers
            .iter()
            .flat_map(|header| {
                header
                    .content
                    .iter()
                    .map(|binding| binding.content_ref.clone())
            })
            .collect();
        self.install_content_metadata(references);
        if self.snapshot.todos.is_some()
            && let Some(todos) = store
                .todo_observation_head(&cut)
                .map_err(RuntimeClientError::observation_store_error)?
        {
            self.install_todo_head(&todos);
        }
        self.transcript_prefix = cut.read.transcript;
        self.pending_mutation_revision = cut.read.mutation_revision;
        self.canonical_observed_through = cut.ledger_through;
        self.native_canonical_highwater = cut.ledger_through;
        self.source_cut = Some(cut);
        Ok(())
    }

    pub(super) fn observe_transcript_position(&mut self, cursor: crate::durable::TranscriptCursor) {
        self.observe_transcript_range(cursor.get(), cursor.get());
    }

    pub(super) fn observe_transcript_range(&mut self, first: u64, last: u64) {
        if first > last || last <= self.transcript_prefix {
            return;
        }
        if let Some(cut) = &mut self.source_cut {
            if last > cut.read.transcript {
                cut.transcript_total = cut
                    .transcript_total
                    .saturating_add(last - cut.read.transcript);
            }
            cut.read.transcript = cut.read.transcript.max(last);
        }
        if first <= self.transcript_prefix.saturating_add(1) {
            self.transcript_prefix = self.transcript_prefix.max(last);
        }
    }

    pub(super) fn observe_transcript_source_prefix(
        &mut self,
        prefix: u64,
        proof: &crate::durable::observation::ObservationReadCut,
    ) -> bool {
        if prefix <= self.transcript_prefix || prefix > proof.read.transcript {
            return false;
        }
        let Some(current) = &self.source_cut else {
            return false;
        };
        if proof.read.conversation_id != self.snapshot.conversation_id
            || proof.read.mutation_revision != self.pending_mutation_revision
            || current.read.mutation_revision != proof.read.mutation_revision
            || self.canonical_observed_through < proof.ledger_through
            || self.journal_through < proof.read.journal
            || current.surface_revision < proof.surface_revision
            || current.read.transcript < prefix
        {
            return false;
        }
        self.transcript_prefix = prefix;
        true
    }

    pub(super) fn observe_released_transcript_prefix(
        &mut self,
        ready: Option<u64>,
        proof: &crate::durable::observation::ObservationReadCut,
    ) {
        match ready {
            Some(prefix) => {
                self.observe_transcript_source_prefix(prefix, proof);
            }
            None => self.mark_observation_unavailable(
                "native transcript release prefix unavailable".into(),
            ),
        }
    }

    pub(super) fn install_pending_receipt_head(
        &mut self,
        receipt: &crate::durable::inbox::PendingObservationReceipt,
    ) {
        let mut pending = Vec::with_capacity(receipt.head.len());
        for header in &receipt.head {
            if let Some(MessageBlock::User(message)) = &header.message.display {
                pending.push(InboundItemView {
                    revision: header.revision,
                    sequence: crate::runtime::inbound::InboundSequence::new(header.sequence),
                    deferred: header.message.deferred,
                    message: message.clone(),
                });
            }
        }
        self.snapshot.inbound.pending = pending;
        if let Some(header) = &receipt.selected
            && let Some(cursor) = header.message.transcript_cursor
        {
            self.observe_transcript_position(cursor);
        }
        if let Some(cursor) = receipt.removed_transcript {
            self.observe_transcript_position(cursor);
        }
    }

    pub(crate) fn apply_pending_receipt(
        &mut self,
        receipt: Option<&crate::durable::inbox::PendingObservationReceipt>,
    ) {
        let Some(receipt) = receipt else {
            self.mark_observation_unavailable("native pending display source unavailable".into());
            return;
        };
        self.observe_released_journal_prefix(receipt.ready_through);
        if receipt.read_cut.mutation_revision < self.pending_mutation_revision {
            return;
        }
        self.pending_mutation_revision = receipt.read_cut.mutation_revision;
        if let Some(expected) = self.pending_epoch_wait {
            if receipt.read_cut.mutation_revision < expected {
                return;
            }
            self.pending_epoch_wait = None;
        }
        self.install_pending_receipt_head(receipt);
        self.native_journal_highwater = self.native_journal_highwater.max(receipt.read_cut.journal);
        if let Some(mut cut) = self.source_cut.clone() {
            cut.read.clone_from(&receipt.read_cut);
            cut.transcript_total = receipt.read_cut.transcript_total;
            cut.surface_revision = receipt.read_cut.surface_revision;
            cut.ledger_through = receipt.read_cut.ledger_through;
            self.record_native_cut(cut);
        }
        if let Some(cut) = self.source_cut.clone() {
            self.observe_released_transcript_prefix(receipt.ready_t_through, &cut);
        }
        self.source_state.source_unavailable = self.source_cut.is_none();
        self.read_domains_dirty = true;
        let Some(revision) = self.read_revision.checked_add(1) else {
            self.publication_failed(&RuntimeClientError::ProjectionExhausted);
            return;
        };
        self.read_revision = revision;
        for owner in ["inbound", "transcript"] {
            let revision = self.domain_revisions.entry(owner).or_default();
            let Some(next) = revision.checked_add(1) else {
                self.publication_failed(&RuntimeClientError::ProjectionExhausted);
                return;
            };
            *revision = next;
        }
        let loaded = self.snapshot.inbound.pending.len() as u64;
        self.source_frontiers.insert(
            "inbound.pending".into(),
            crate::observation::cut::CollectionFrontier {
                domain: "inbound.pending".into(),
                revision: self.read_revision,
                total: receipt.total,
                loaded,
                next_offset: (loaded < receipt.total).then(|| {
                    if loaded == receipt.head.len() as u64 {
                        receipt.head.last().map_or(0, |item| item.sequence)
                    } else {
                        0
                    }
                }),
            },
        );
        let mut events = vec![RuntimeClientEvent::PendingInboundChanged {
            pending: self.snapshot.inbound.pending.clone(),
        }];
        if let Some(header) = &receipt.selected
            && receipt
                .head
                .iter()
                .any(|item| item.sequence == header.sequence)
            && let Some(MessageBlock::User(message)) = &header.message.display
        {
            events.push(RuntimeClientEvent::InboundEnqueued {
                sequence: crate::runtime::inbound::InboundSequence::new(header.sequence),
                message: message.clone(),
                transcript_cursor: header
                    .message
                    .transcript_cursor
                    .map(RuntimeClientTranscriptCursor::from),
            });
        }
        if let (Some(watermark), Some(count)) = (receipt.adopted_through, receipt.adopted_count) {
            let watermark = crate::runtime::inbound::InboundSequence::new(watermark);
            let count = usize::try_from(count).expect("native adoption count fits usize");
            self.snapshot.inbound.last_drain = Some(InboundDrainView { watermark, count });
            events.push(RuntimeClientEvent::InboundDrained {
                watermark,
                count,
                message_ids: Vec::new(),
            });
        }
        self.publish(RuntimeClientEvent::ProjectionTransition {
            events,
            metadata: self.projection_metadata(),
        });
    }

    pub(crate) fn mark_observation_unavailable(&mut self, mut message: String) {
        self.source_state.source_layout_failure = true;
        self.source_state.source_unavailable = true;
        let mut end = message.len().min(1024);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        self.read_failure = Some(message);
        self.wake_subscribers();
    }

    pub(crate) fn publication_commit_scope(&mut self, active: bool, _complete: bool) {
        // Queue occupancy cannot establish or invalidate a native source cut.
        // The frozen C/J/T receipts and their contiguous owner prefix do that.
        self.publication_state.publication_commit = active;
    }

    #[cfg(test)]
    pub(crate) fn source_prefix_debug(&self) -> String {
        format!(
            "cursor={} J={}/{} L={} T={} M={} wait={:?} source={:?} published={:?} unavailable={} layout={} journal={} read_failure={:?} working_progress={}",
            self.cursor.get(),
            self.journal_through,
            self.native_journal_highwater,
            self.canonical_observed_through,
            self.transcript_prefix,
            self.pending_mutation_revision,
            self.pending_epoch_wait,
            self.source_cut,
            self.published_seed.as_ref().map(|seed| &seed.read_cut),
            self.source_state.source_unavailable,
            self.source_state.source_layout_failure,
            self.source_state.journal_failure,
            self.read_failure.as_ref().map(|message| {
                let mut end = message.len().min(256);
                while !message.is_char_boundary(end) {
                    end -= 1;
                }
                &message[..end]
            }),
            self.snapshot.attempt.as_ref().map_or(0, |attempt| attempt
                .foreground
                .iter()
                .filter(|slot| matches!(
                    slot.state,
                    ForegroundToolState::Running {
                        progress: Some(_),
                        ..
                    }
                ))
                .count()),
        )
    }

    pub(crate) fn validate_published_source(&self) -> Result<(), RuntimeClientError> {
        if let Some(message) = &self.read_failure {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: message.clone(),
            });
        }
        if let Some(seed) = &self.published_seed {
            if seed.read_cut.source_cut.is_some() && !seed.journal_failure {
                return Ok(());
            }
            if seed.source_pending
                && !self.source_state.source_layout_failure
                && !self.source_state.journal_failure
            {
                return Err(RuntimeClientError::ReadCutExpired);
            }
            return Err(RuntimeClientError::ObservationUnavailable {
                message: "published native source cut unavailable".into(),
            });
        }
        self.validate_source_prefix()
    }

    /// Completion of this exact finite CPU receipt, independent of source
    /// prefix readiness and external cursor publication.
    pub(crate) fn native_receipt_installation(
        &self,
    ) -> crate::runtime::observation::NativeReceiptInstallation {
        if self.exhausted
            || self.source_state.source_layout_failure
            || self.source_state.journal_failure
        {
            crate::runtime::observation::NativeReceiptInstallation::Unavailable
        } else {
            crate::runtime::observation::NativeReceiptInstallation::Installed
        }
    }

    /// Native append captured this prefix from its publication-readiness
    /// index. It includes intrinsically ready diagnostics, but stops at every
    /// earlier owner or missing index row. It does not grant P byte access.
    pub(super) fn observe_released_journal_prefix(&mut self, ready: Option<u64>) {
        let Some(ready) = ready else {
            self.mark_observation_unavailable("native source release prefix unavailable".into());
            return;
        };
        self.journal_through = self.journal_through.max(ready);
        self.read_through = self.journal_through;
    }

    /// A delayed owner fact cannot roll back a newer native transaction cut.
    pub(super) fn record_native_cut(
        &mut self,
        cut: crate::durable::observation::ObservationReadCut,
    ) {
        self.native_journal_highwater = self.native_journal_highwater.max(cut.read.journal);
        self.native_canonical_highwater = self.native_canonical_highwater.max(cut.ledger_through);
        if cut.read.mutation_revision > self.pending_mutation_revision {
            self.pending_epoch_wait = Some(cut.read.mutation_revision);
        }
        let advances = self.source_cut.as_ref().is_none_or(|previous| {
            cut.read.journal >= previous.read.journal
                && cut.ledger_through >= previous.ledger_through
                && cut.surface_revision >= previous.surface_revision
                && cut.read.mutation_revision >= previous.read.mutation_revision
                && cut.read.transcript >= previous.read.transcript
        });
        if advances {
            self.source_cut = Some(cut);
        }
    }

    pub(super) fn native_source_pending(&self) -> bool {
        self.journal_through < self.native_journal_highwater
            || self.canonical_observed_through < self.native_canonical_highwater
            || self.pending_epoch_wait.is_some()
            || self.source_cut.as_ref().is_some_and(|cut| {
                cut.ledger_through > self.canonical_observed_through
                    || cut.read.transcript != self.transcript_prefix
            })
    }

    pub(crate) fn validate_source_prefix(&self) -> Result<(), RuntimeClientError> {
        if self.native_source_pending() {
            return Err(RuntimeClientError::ReadCutExpired);
        }
        if self.source_state.source_unavailable
            || self.source_state.source_layout_failure
            || self
                .source_cut
                .as_ref()
                .is_some_and(|cut| cut.read.transcript != self.transcript_prefix)
        {
            return Err(RuntimeClientError::ObservationUnavailable {
                message: "native transcript commit awaits its observation owner".into(),
            });
        }
        Ok(())
    }

    #[cfg(test)]
    pub(crate) fn prepare_terminal_positions(
        &mut self,
        positions: Vec<(AttemptId, crate::durable::TranscriptCursor)>,
    ) {
        self.prepared_terminals.extend(positions);
    }

    pub(crate) fn prepare_message_headers(
        &mut self,
        headers: Vec<(
            MessageId,
            crate::durable::observation::MessageHeader,
            Option<crate::conversation::SurfaceRevision>,
        )>,
    ) {
        for (identity, header, revision) in headers {
            self.prepared_messages.insert(identity, (header, revision));
        }
    }

    pub(crate) fn install_domain_sources(
        &mut self,
        sources: Vec<(String, Arc<super::super::domain::DomainSource>, usize)>,
    ) {
        for (key, source, weight) in sources {
            let revision = self.domain_revision(&key);
            if let Some((_, old, _)) = self.domain_sources.insert(key, (source, weight, revision)) {
                self.domain_source_bytes -= old;
            }
            self.domain_source_bytes += weight;
        }
        while self.domain_sources.len() > 4096 || self.domain_source_bytes > 16 * 1024 * 1024 {
            let key = self
                .domain_sources
                .keys()
                .find(|key| key.starts_with("jobs.") || key.starts_with("agents."))
                .cloned()
                .or_else(|| self.domain_sources.keys().next().cloned());
            if let Some(key) = key {
                if let Some((_, weight, _)) = self.domain_sources.remove(&key) {
                    self.domain_source_bytes -= weight;
                }
            } else {
                break;
            }
        }
    }

    pub(crate) fn current_capability_source(
        &self,
    ) -> Option<crate::observation::capability_source::CapabilityReadSource> {
        self.domain_sources
            .get("capabilities")
            .and_then(|(source, _, _)| match source.as_ref() {
                super::super::domain::DomainSource::Capability(value) => {
                    Some(value.value().clone())
                }
                _ => None,
            })
    }

    pub(crate) fn current_model_capabilities(
        &self,
    ) -> Option<crate::model::catalog::ModelCapabilities> {
        self.domain_sources
            .get("model")
            .and_then(|(source, _, _)| match source.as_ref() {
                super::super::domain::DomainSource::Model(value) => {
                    value.value().capabilities().cloned()
                }
                _ => None,
            })
    }

    pub(crate) fn install_resource_header(
        &mut self,
        header: crate::observation::resource_source::RuntimeClientResourcesView,
    ) {
        self.snapshot.resources = header;
    }

    pub(crate) fn install_source_frontiers(
        &mut self,
        frontiers: Vec<crate::observation::cut::CollectionFrontier>,
    ) {
        for frontier in frontiers {
            assert!(
                matches!(
                    frontier.domain.as_str(),
                    "model"
                        | "attempt.model"
                        | "model_catalog.models"
                        | "capabilities.configured_tools"
                        | "capabilities.tools"
                        | "capabilities.available_tools"
                        | "capabilities.skills"
                        | "capabilities.sources"
                        | "resources.inspection"
                        | "resources.context_files"
                ) || frontier.domain == "workflows.runs"
                    || frontier.domain.starts_with("workflows.runs.@")
            );
            self.source_frontiers
                .insert(frontier.domain.clone(), frontier);
        }
    }

    pub(super) fn install_model_frontier(&mut self, inline: bool) {
        self.install_source_frontiers(vec![crate::observation::cut::CollectionFrontier {
            domain: "model".into(),
            revision: 0,
            total: 1,
            loaded: u64::from(inline),
            next_offset: (!inline).then_some(0),
        }]);
    }

    pub(crate) fn validate_domain_version(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
    ) -> Result<(), RuntimeClientError> {
        let owner = domain.split('.').next().unwrap_or(domain);
        if cut.domain_revisions.get(owner).copied().unwrap_or(0)
            != self.published_seed.as_ref().map_or_else(
                || self.domain_revision(domain),
                |seed| seed.domain_revision(domain),
            )
        {
            return Err(RuntimeClientError::ReadCutExpired);
        }
        Ok(())
    }

    pub(crate) fn domain_source(
        &self,
        domain: &str,
        item_offset: u64,
    ) -> Option<Arc<super::super::domain::DomainSource>> {
        let snapshot = self.snapshot_ref();
        let key = match domain {
            "pending_interactions" => snapshot
                .pending_interactions
                .get(usize::try_from(item_offset).ok()?)
                .map(|interaction| format!("pending_interactions.{}", interaction.interaction)),
            "jobs" => snapshot
                .jobs
                .get(usize::try_from(item_offset).ok()?)
                .map(|job| format!("jobs.{}", job.job_id)),
            "agents" => snapshot
                .agents
                .get(usize::try_from(item_offset).ok()?)
                .map(|agent| format!("agents.{}", agent.agent_id)),
            domain if domain.starts_with("model_catalog.") => Some("model_catalog".to_owned()),
            domain if domain.starts_with("capabilities.") => Some("capabilities".to_owned()),
            domain if domain.starts_with("resources.") => Some("resources".to_owned()),
            domain if domain.starts_with("resources.") => Some("resources".to_owned()),
            domain if domain.starts_with("model.") => Some("model".to_owned()),
            domain if domain.starts_with("attempt.model.") => Some("attempt.model".to_owned()),
            domain if domain.starts_with("workflows.") => Some("workflows".to_owned()),
            "todos.tasks" => Some("todos".to_owned()),
            _ => Some(domain.to_owned()),
        }?;
        self.published_sources
            .as_ref()
            .unwrap_or(&self.domain_sources)
            .get(&key)
            .and_then(|(source, _, revision)| {
                (*revision
                    == self.published_seed.as_ref().map_or_else(
                        || self.domain_revision(domain),
                        |seed| seed.domain_revision(domain),
                    ))
                .then(|| Arc::clone(source))
            })
    }

    pub(crate) fn validate_checkpoint_cut(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
    ) -> Result<(), RuntimeClientError> {
        if self.checkpoint_read_cut() == *cut {
            return Ok(());
        }
        // The current bounded Trace cache owns its issued cut independently
        // of replay eviction. Retiring that cache also retires this evidence.
        if self
            .published_seed
            .as_ref()
            .filter(|seed| {
                !seed.snapshot.trace.records.is_empty() || !seed.snapshot.trace.updates.is_empty()
            })
            .and_then(|seed| seed.snapshot.trace.read_cut.as_ref())
            == Some(cut)
        {
            return Ok(());
        }
        if self.replay_cut_cursors.get(&cut.revision) == Some(&RuntimeClientCursor::new(0))
            && self.earliest_serviceable().get() == 0
            && self.initial_read_cut.as_ref() == Some(cut)
        {
            return Ok(());
        }
        if let Some(cursor) = self.replay_cut_cursors.get(&cut.revision)
            && let Some((first, _)) = self.replay.front()
            && let Some(position) = cursor
                .get()
                .checked_sub(first.get())
                .and_then(|position| usize::try_from(position).ok())
            && let Some((retained_cursor, event)) = self.replay.get(position)
            && retained_cursor == cursor
            && Self::retained_event_cut(event) == Some(cut)
        {
            return Ok(());
        }
        Err(RuntimeClientError::ReadCutExpired)
    }

    #[cfg(test)]
    pub(crate) fn publication_source(
        &self,
        block_index: ContentBlockIndex,
    ) -> crate::observation::content::ContentSource {
        let start = self
            .publication_start
            .as_ref()
            .expect("opened native stream");
        crate::observation::content::ContentSource::LivePublicationPrefix {
            component: crate::observation::content::PublicationContentComponent::Payload,
            stream_id: start.stream_id.clone(),
            message_id: start.message_id.clone(),
            attempt_id: start.attempt_id.clone(),
            turn_id: start.turn_id.clone(),
            publication_request_id: start.request_id.clone(),
            block_index,
        }
    }

    #[cfg(test)]
    pub(crate) fn checkpoint_page(
        &self,
        cut: &crate::observation::cut::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::super::checkpoint::CheckpointPage, RuntimeClientError> {
        self.checkpoint_seed()
            .checkpoint_page(cut, domain, offset, limit)
    }

    pub(crate) fn validate_live_content_ref(
        &self,
        reference: &crate::observation::content::ContentRef,
    ) -> Result<(), RuntimeClientError> {
        use crate::observation::content::ContentSource;
        let ContentSource::LivePublicationPrefix { .. } = &reference.source else {
            return Ok(());
        };
        let Some(scope) = self.checkpoint_read_cut().live_publication else {
            return Err(RuntimeClientError::ContentSourceUnavailable);
        };
        let ContentSource::LivePublicationPrefix {
            component,
            stream_id,
            message_id,
            attempt_id,
            turn_id,
            publication_request_id,
            ..
        } = &reference.source
        else {
            unreachable!()
        };
        let ContentSource::LivePublicationPrefix {
            stream_id: expected_stream,
            message_id: expected_message,
            attempt_id: expected_attempt,
            turn_id: expected_turn,
            publication_request_id: expected_request,
            ..
        } = &scope.source
        else {
            unreachable!()
        };
        let _ = component;
        if reference.conversation_id != self.snapshot.conversation_id
            || stream_id != expected_stream
            || message_id != expected_message
            || attempt_id != expected_attempt
            || turn_id != expected_turn
            || publication_request_id != expected_request
            || reference.version != 1
            || reference.content_revision > scope.revision
        {
            return Err(RuntimeClientError::ContentSourceUnavailable);
        }
        Ok(())
    }
}
