//! Composed immutable lineage Trace reads.
//!
//! A Session version tree shares one canonical Ledger coordinate space: a
//! forked node seeds the source prefix and commits only its own suffix. This
//! module walks that Fork chain and serves one bounded page that joins each
//! ancestor's source-owned prefix records with the selected node's own
//! suffix, without attaching, switching or starting any earlier node.
//!
//! Composed cursors are `slot * 2^48 + local journal sequence`. Journal
//! sequences never approach the slot base, so `trace:N >= 2^48` unambiguously
//! names a lineage-owned record while smaller identities stay node-local and
//! keep their original detail routing. Prefix records carry rewritten stable
//! identities only; every field, collection, request and content authority
//! still resolves inside the owning node's store under its own bounded cut.

use super::{SessionController, SessionError, SessionId, SessionNodeId, SessionReadAccess};
use crate::durable::observation::ObservationReadCut;
use crate::durable::store::LineageRequest;
use crate::durable::{ProjectionStore, RequestStore};
use crate::observation::content::CanonicalContentLocator;
use crate::observation::cut::CheckpointReadCut;
use crate::observation::metadata::MetadataFragment;
use crate::observation::trace::{TraceCursor, TracePage};
use crate::runtime::identity::ConversationId;
use crate::runtime_client::RuntimeClientError;
use crate::runtime_client::trace::TraceProjection;
use std::collections::BTreeSet;
use std::sync::Arc;

/// Composed cursor slot base. Real journal sequences stay far below it.
const SLOT_BASE: u64 = 1 << 48;
/// Defensive bound on Fork-chain depth; real version trees are far smaller.
const MAX_LINKS: usize = 256;

/// One node on the selected node's immutable Fork ancestry.
struct Link {
    session: SessionId,
    node: SessionNodeId,
    /// Canonical positions at or below this count were seeded into this node,
    /// so it owns no journal facts for them.
    retained: u64,
}

/// One source-owned read segment, oldest ancestor first, selected node last.
struct Segment {
    session: SessionId,
    node: SessionNodeId,
    access: SessionReadAccess,
    /// Inclusive upper canonical position this node contributes to the viewed
    /// prefix; `u64::MAX` marks the selected node's own unbounded suffix.
    bound: u64,
}

fn open_store(
    access: &SessionReadAccess,
) -> Result<crate::durable::SqliteConversationStore, SessionError> {
    Ok(crate::durable::SqliteConversationStore::open_existing(
        access.conversation_id.clone(),
        &access.database_path,
    )
    .map_err(SessionError::Store)?
    .with_lifecycle(access.allocation.clone()))
}

fn preceding_request(segments: &[Segment]) -> Result<Option<Arc<LineageRequest>>, SessionError> {
    let mut permission_continuous = true;
    for segment in segments.iter().rev() {
        let store = open_store(&segment.access)?;
        let cut = store
            .observation_prefix_cut(segment.bound)
            .map_err(SessionError::Store)?;
        let through = cut.read.journal;
        let activation_at = |journal| {
            store
                .projection_at_journal_cut(journal)
                .map(|head| head.map_or_default(|head| head.activation.activation))
                .map_err(SessionError::Store)
        };
        let active = activation_at(through)?;
        if let Some(header) = store
            .lineage_previous_request(through)
            .map_err(SessionError::Store)?
        {
            let permission_mode = if let Some(id) = &header.request_id
                && permission_continuous
            {
                let state = store
                    .load_request_permission_state(id)
                    .map_err(SessionError::Store)?;
                (state.projection_activation == active).then_some(state.permission_mode)
            } else {
                None
            };
            return Ok(Some(Arc::new(LineageRequest {
                store: Arc::new(store),
                header,
                through,
                permission_mode,
            })));
        }
        permission_continuous &= active == activation_at(0)?;
    }
    Ok(None)
}

fn history_cut(native: &ObservationReadCut) -> CheckpointReadCut {
    CheckpointReadCut {
        canonical_foreground: None,
        live_publication: None,
        collection_cuts: MetadataFragment::default(),
        conversation_id: native.read.conversation_id.clone(),
        activation_id: None,
        revision: 0,
        through: native.read.journal,
        source_cut: Some(native.clone()),
        domain_revisions: MetadataFragment::default(),
    }
}

fn invalid_cursor() -> SessionError {
    SessionError::Catalog {
        detail: "invalid lineage Trace cursor".into(),
    }
}

/// Decode a composed cursor into `(slot, local)`. Node-local cursors below the
/// slot base address the selected node's own segment.
fn decode_cursor(
    cursor: Option<&TraceCursor>,
    segments: usize,
) -> Result<(usize, Option<u64>), SessionError> {
    let Some(cursor) = cursor else {
        return Ok((segments, None));
    };
    let sequence = cursor.sequence().map_err(|_| invalid_cursor())?;
    if sequence >= SLOT_BASE {
        let slot = sequence / SLOT_BASE;
        let local = sequence % SLOT_BASE;
        if slot == 0 || slot as usize > segments || local == 0 {
            return Err(invalid_cursor());
        }
        Ok((slot as usize, Some(local)))
    } else {
        Ok((segments, Some(sequence)))
    }
}

impl SessionController {
    /// The immutable Fork ancestry of `node`, selected node first.
    async fn lineage_links(
        &self,
        session: &SessionId,
        node: &SessionNodeId,
    ) -> Result<Vec<Link>, SessionError> {
        let mut links = Vec::new();
        let mut seen = BTreeSet::new();
        let mut current = (session.clone(), node.clone());
        loop {
            if links.len() >= MAX_LINKS || !seen.insert(current.clone()) {
                return Err(SessionError::Catalog {
                    detail: "Session lineage exceeds the bounded Trace chain".into(),
                });
            }
            let catalog = self.catalog.lock().await;
            let observed = catalog.observe_node(&current.0, &current.1)?;
            let fork = matches!(
                observed.origin,
                crate::local_runtime::session::observation::SessionNodeOriginObservation::Fork {
                    ref source_session, ref source_node, ..
                } if *source_session == current.0 && observed.parent.as_ref() == Some(source_node)
            );
            let (retained, parent) = if fork {
                let read = catalog.read_origin(&current.0, &current.1)?;
                let CanonicalContentLocator::ByLedgerPosition { position } = read.source_locator
                else {
                    return Err(SessionError::Catalog {
                        detail: "Fork origin requires a finite native ledger position".into(),
                    });
                };
                (
                    position - u64::from(read.side == super::super::session::LineageSide::Before),
                    Some((read.source_session, read.source_node)),
                )
            } else {
                // New and Clone nodes own every position past their committed
                // bootstrap seed; only Fork parents continue the walk.
                let access = catalog.acquire_read(&current.0, Some(&current.1))?;
                drop(catalog);
                let retained = tokio::task::spawn_blocking(move || -> Result<u64, SessionError> {
                    open_store(&access)?
                        .lineage_seed_count()
                        .map_err(SessionError::Store)
                })
                .await
                .map_err(|error| SessionError::Catalog {
                    detail: error.to_string(),
                })??;
                (retained, None)
            };
            links.push(Link {
                session: current.0.clone(),
                node: current.1.clone(),
                retained,
            });
            match parent {
                Some(parent) => current = parent,
                None => break,
            }
        }
        Ok(links)
    }

    /// Ordered source-owned segments for `node`'s committed lineage, oldest
    /// ancestor first and the selected node last. Ancestors contribute exactly
    /// the canonical range `(retained, bound]` they own inside the viewed
    /// prefix, where `bound` is the smallest retained frontier below them on
    /// the chain; empty ranges and non-Fork sources are omitted.
    async fn lineage_segments(
        &self,
        session: &SessionId,
        node: &SessionNodeId,
    ) -> Result<Vec<Segment>, SessionError> {
        let links = self.lineage_links(session, node).await?;
        // Walk order is selected-first; bounds propagate upward as the running
        // minimum of descendant retained frontiers.
        let mut bounds = vec![u64::MAX; links.len()];
        let mut running = links[0].retained;
        for index in 1..links.len() {
            bounds[index] = running;
            running = running.min(links[index].retained);
        }
        let mut plan = Vec::new();
        for index in (1..links.len()).rev() {
            if bounds[index] > links[index].retained {
                plan.push((
                    links[index].session.clone(),
                    links[index].node.clone(),
                    bounds[index],
                ));
            }
        }
        plan.push((session.clone(), node.clone(), u64::MAX));
        let mut segments = Vec::with_capacity(plan.len());
        for (segment_session, segment_node, bound) in plan {
            let access = self
                .acquire_read(&segment_session, Some(&segment_node))
                .await?;
            segments.push(Segment {
                session: segment_session,
                node: segment_node,
                access,
                bound,
            });
        }
        Ok(segments)
    }

    pub(crate) async fn lineage_request(
        &self,
        id: &SessionId,
        node: &SessionNodeId,
    ) -> Result<Option<Arc<LineageRequest>>, SessionError> {
        let segments = self.lineage_segments(id, node).await?;
        tokio::task::spawn_blocking(move || preceding_request(&segments[..segments.len() - 1]))
            .await
            .map_err(|error| SessionError::Catalog {
                detail: error.to_string(),
            })?
    }

    /// One bounded composed page: source-owned ancestor prefix records under
    /// rewritten stable identities, then the selected node's own suffix under
    /// its original identities. The returned conversation is always the
    /// selected node's; each record's detail still resolves inside its owning
    /// store through the explicit `(node, cut)` readers.
    /// # Errors
    /// Unknown identities, invalid cursors and storage failures are explicit.
    pub(crate) async fn read_lineage_trace(
        &self,
        id: &SessionId,
        node: Option<&SessionNodeId>,
        before: Option<TraceCursor>,
        limit: usize,
    ) -> Result<(ConversationId, TracePage), SessionError> {
        let selected = {
            let catalog = self.catalog.lock().await;
            catalog.lineage(id, node)?.0
        };
        let segments = if selected.parent.is_some() {
            self.lineage_segments(id, &selected.id).await?
        } else {
            Vec::new()
        };
        if segments.len() <= 1 {
            return self
                .read_trace(id, Some(&selected.id), move |projection| {
                    projection.page(before.as_ref(), limit)
                })
                .await;
        }
        let (start_slot, local_before) = decode_cursor(before.as_ref(), segments.len())?;
        let count = segments.len();
        let result = tokio::task::spawn_blocking(
            move || -> Result<(ConversationId, TracePage), SessionError> {
                let selected = open_store(&segments[count - 1].access)?;
                let selected_cut =
                    crate::runtime_client::trace_observation::history_store_cut(&selected)
                        .map_err(SessionError::Store)?;
                for slot in (1..=start_slot).rev() {
                    let segment = &segments[slot - 1];
                    let store = open_store(&segment.access)?
                        .with_lineage_request(preceding_request(&segments[..slot - 1])?);
                    let local = (slot == start_slot)
                        .then_some(local_before)
                        .flatten()
                        .map(TraceCursor::at);
                    let projection = if segment.bound == u64::MAX {
                        TraceProjection::new(&store)
                    } else {
                        TraceProjection::at_cut(
                            &store,
                            &store
                                .observation_prefix_cut(segment.bound)
                                .map_err(SessionError::Store)?,
                        )
                    }
                    .map_err(SessionError::Store)?;
                    let mut page = projection
                        .page(local.as_ref(), limit)
                        .map_err(SessionError::Store)?;
                    if page.records.is_empty() {
                        continue;
                    }
                    let composed = |cursor: &TraceCursor| -> Result<u64, SessionError> {
                        let local = cursor.sequence().map_err(|_| invalid_cursor())?;
                        if local >= SLOT_BASE {
                            return Err(invalid_cursor());
                        }
                        Ok(slot as u64 * SLOT_BASE + local)
                    };
                    let earlier = page
                        .next_cursor
                        .as_ref()
                        .or_else(|| (slot > 1).then(|| &page.records[0].position));
                    let next = earlier.map(composed).transpose()?;
                    for record in &mut page.records {
                        let sequence = composed(&record.position)?;
                        if slot < count {
                            record.id = format!("trace:{sequence}");
                        }
                        record.position = TraceCursor::at(sequence);
                    }
                    page.read_cut = Some(selected_cut);
                    page.next_cursor = next.map(TraceCursor::at);
                    crate::runtime_client::trace::ensure_page_bound(&page)
                        .map_err(SessionError::Store)?;
                    return Ok((segments[count - 1].access.conversation_id.clone(), page));
                }
                Ok((
                    segments[count - 1].access.conversation_id.clone(),
                    TracePage {
                        read_cut: Some(selected_cut),
                        records: Vec::new(),
                        updates: Vec::new(),
                        next_cursor: None,
                    },
                ))
            },
        )
        .await
        .map_err(|error| SessionError::Catalog {
            detail: error.to_string(),
        })??;
        Ok(result)
    }

    /// Detail page for one composed record identity (`trace:N`, `N >= 2^48`).
    /// Returns `None` for ordinary node-local identities so callers keep the
    /// existing single-node path. The returned page presents the selected
    /// node's conversation as read owner while its `source_cut` retains the
    /// owning node's immutable bounded cut.
    /// # Errors
    /// Unknown identities, foreign cursors and storage failures are explicit.
    pub(crate) async fn read_lineage_trace_detail(
        &self,
        id: &SessionId,
        node: Option<&SessionNodeId>,
        record_id: &str,
    ) -> Result<
        Option<(
            ConversationId,
            Result<crate::runtime_client::checkpoint::CheckpointPage, RuntimeClientError>,
        )>,
        SessionError,
    > {
        let Some(sequence) = record_id
            .strip_prefix("trace:")
            .and_then(|value| value.parse::<u64>().ok())
            .filter(|value| *value >= SLOT_BASE)
        else {
            return Ok(None);
        };
        let node_id = {
            let catalog = self.catalog.lock().await;
            catalog.lineage(id, node)?.0.id
        };
        let segments = self.lineage_segments(id, &node_id).await?;
        let slot = (sequence / SLOT_BASE) as usize;
        let local = sequence % SLOT_BASE;
        if slot == 0 || slot >= segments.len() || local == 0 {
            return Err(SessionError::Catalog {
                detail: "invalid lineage Trace record identity".into(),
            });
        }
        let self_conversation = segments[segments.len() - 1].access.conversation_id.clone();
        let local_id = format!("trace:{local}");
        let page = tokio::task::spawn_blocking(
            move || -> Result<
                Result<
                    crate::runtime_client::checkpoint::CheckpointPage,
                    RuntimeClientError,
                >,
                SessionError,
            > {
                let segment = &segments[slot - 1];
                let store = open_store(&segment.access)?
                    .with_lineage_request(preceding_request(&segments[..slot - 1])?);
                let native = store
                    .observation_prefix_cut(segment.bound)
                    .map_err(SessionError::Store)?;
                let cut = history_cut(&native);
                Ok(crate::runtime_client::trace_observation::history_detail(
                    &store, &cut, &local_id,
                ))
            },
        )
        .await
        .map_err(|error| SessionError::Catalog {
            detail: error.to_string(),
        })?;
        let page = page?;
        let page = match page {
            Ok(mut page) => {
                // Present the selected node's conversation as the read owner;
                // the nested `source_cut` keeps the physical owning
                // conversation and immutable bounded cut.
                page.read_cut.conversation_id = self_conversation.clone();
                Ok(page)
            }
            Err(error) => Err(error),
        };
        Ok(Some((self_conversation, page)))
    }

    /// Resolve a historical read cut whose `source_cut` names a different
    /// conversation inside this Session's lineage to its owning segment.
    /// Returns the segment's `(session, node, access)`; `None` means the
    /// conversation is not a committed ancestor of the selected node.
    /// # Errors
    /// Unknown identities and storage failures are explicit.
    pub(crate) async fn lineage_trace_source(
        &self,
        id: &SessionId,
        node: Option<&SessionNodeId>,
        source: &ObservationReadCut,
    ) -> Result<Option<(SessionId, SessionNodeId)>, SessionError> {
        let node_id = {
            let catalog = self.catalog.lock().await;
            catalog.lineage(id, node)?.0.id
        };
        let segments = self.lineage_segments(id, &node_id).await?;
        let Some(segment) = segments.into_iter().find(|segment| {
            segment.bound != u64::MAX
                && segment.access.conversation_id == source.read.conversation_id
        }) else {
            return Ok(None);
        };
        let requested = source.clone();
        tokio::task::spawn_blocking(move || {
            let store = open_store(&segment.access)?;
            let upper = store
                .observation_prefix_cut(segment.bound)
                .map_err(SessionError::Store)?;
            Ok((requested.ledger_through == upper.ledger_through
                && requested.read.journal == upper.read.journal
                && requested.read.reconstructible_from(&upper.read)
                && requested.inherited_through == upper.inherited_through)
                .then_some((segment.session, segment.node)))
        })
        .await
        .map_err(|error| SessionError::Catalog {
            detail: error.to_string(),
        })?
    }
}
