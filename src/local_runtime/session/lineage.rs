//! Clone, fork and tree-node lineage: exact read cuts of a source conversation,
//! seed remapping under destination identities, inherited projections, and
//! publication of the prepared destination.

use super::CatalogDocument;
use super::LineageSide;
use super::SessionCatalog;
use super::SessionError;
use super::SessionNode;
use super::SessionNodeOrigin;
use super::SessionPersistentState;
use super::SessionSnapshot;
#[cfg(test)]
use super::create_profile;
use super::creation::conversation_database_path;
use super::creation::initialize_database;
use super::projection::seed_preview;
use super::uploads;
use crate::conversation::SurfaceOp;
use crate::conversation::message_id_of;
use crate::durable::LineageSeed;
use crate::durable::reading::LineageReadCut;
use crate::message::types::AssistantMessageBlock;
use crate::message::types::InboundKind;
use crate::message::types::MessageBlock;
use crate::message::types::ToolMessageBlock;
use crate::message::types::UserContentBlock;
use crate::message::types::UserMessageBlock;
use crate::runtime::identity::ConversationId;
use crate::runtime::identity::MessageId;
use crate::runtime::identity::SessionId;
use crate::runtime::identity::SessionNodeId;
use std::collections::BTreeMap;
use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

/// A private, already-seeded destination waiting for catalog publication.
///
/// It is intentionally not visible through the Runtime Client protocol. A
/// prepared lineage is either published by `SessionCatalog` or remains an
/// unreferenced private directory.
#[derive(Debug, Clone)]
pub(crate) struct PreparedLineage {
    pub(crate) uploads: uploads::UploadRegistry,
    pub(crate) session_id: SessionId,
    pub(crate) node_id: SessionNodeId,
    pub(crate) conversation_id: ConversationId,
    pub(crate) state: SessionPersistentState,
    /// The display projection derived from the frozen seed, published with
    /// the Session at the visibility commit. Branch-node preparations carry
    /// `None`; [`SessionCatalog::build_node_document`] never reads it,
    /// because the projection is Session-level root-lineage metadata.
    pub(crate) display_preview: Option<String>,
    pub(crate) database_path: PathBuf,
}

/// Resolve an explicit routing node without consulting the graph default.
pub(super) fn lineage_of(
    document: &CatalogDocument,
    session_id: &SessionId,
    node_id: &SessionNodeId,
) -> Result<(SessionId, SessionNode, SessionPersistentState), SessionError> {
    let session =
        document
            .sessions
            .get(session_id)
            .ok_or_else(|| SessionError::UnknownSession {
                session_id: session_id.clone(),
            })?;
    let node = session
        .nodes
        .get(node_id)
        .ok_or_else(|| SessionError::UnknownNode {
            session_id: session.id.clone(),
            node_id: node_id.clone(),
        })?;
    Ok((session.id.clone(), node.clone(), session.state.clone()))
}

impl SessionCatalog {
    /// Prepares a new independent Session with a fresh `ConversationId`.
    pub(crate) fn prepare_session(
        &self,
        template: &SessionPersistentState,
        seed: &[MessageBlock],
    ) -> Result<PreparedLineage, SessionError> {
        let (session_id, node_id, conversation_id) = self.allocate_ids()?;
        self.prepare_session_with_ids(
            template,
            session_id,
            node_id,
            conversation_id,
            &LineageSeed::history(seed.to_vec()),
            None,
        )
    }

    /// Prepares a clone from captured native authority (R, C). Structural
    /// revision, message bodies and temporal provenance are immutable inputs
    /// to this preparation; a later read of R may capture a different C.
    ///
    /// The clone preserves the canonical conversation/domain state in effect
    /// at that cut and the Surface provenance needed for later lineage
    /// operations. It creates a new `ConversationId` and therefore a fresh
    /// execution epoch: attempt-, request-, Event-Journal-, and
    /// execution-derived reminder/cooldown state are not inherited. See
    /// [`LineageReadCut`] and [`lineage_cut`].
    pub(crate) fn prepare_clone_session(
        &self,
        template: &SessionPersistentState,
        source: &LineageReadCut,
    ) -> Result<PreparedLineage, SessionError> {
        let (session_id, node_id, conversation_id) = self.allocate_ids()?;
        let seed = lineage_cut(
            &conversation_id,
            source,
            None,
            crate::local_runtime::session::LineageSide::Before,
        )?;
        self.prepare_session_with_ids(
            template,
            session_id,
            node_id,
            conversation_id,
            &seed,
            self.lineage_artifacts(source, &seed)?.as_ref(),
        )
    }

    /// Prepares an independent Session fork and returns the selected original
    /// user prompt for uncommitted editor restoration.
    pub(crate) fn prepare_fork_session(
        &self,
        template: &SessionPersistentState,
        source: &LineageReadCut,
        message_id: &MessageId,
        side: LineageSide,
    ) -> Result<
        (
            PreparedLineage,
            Vec<crate::message::types::UserContentBlock>,
        ),
        SessionError,
    > {
        let editor_content = match side {
            LineageSide::Before => {
                restorable_editor_content(active_user_boundary(source, message_id)?)?
            }
            LineageSide::After => {
                if !source.messages.iter().any(|message| matches!(message, MessageBlock::Assistant(assistant) if assistant.id == *message_id)) {
                    return Err(SessionError::UnknownBoundary { message_id: message_id.clone() });
                }
                Vec::new()
            }
        };
        let (session_id, node_id, conversation_id) = self.allocate_ids()?;
        let seed = lineage_cut(&conversation_id, source, Some(message_id), side)?;
        let prepared = self.prepare_session_with_ids(
            template,
            session_id,
            node_id,
            conversation_id,
            &seed,
            self.lineage_artifacts(source, &seed)?.as_ref(),
        )?;
        Ok((prepared, editor_content))
    }

    /// Prepares a tree node at the exact selected cut, with the same prefix
    /// semantics as independent Fork. Only a pre-input cut restores the editor.
    pub(crate) fn prepare_tree_node(
        &self,
        session_id: &SessionId,
        template: &SessionPersistentState,
        source: &LineageReadCut,
        message_id: &MessageId,
        side: LineageSide,
    ) -> Result<
        (
            PreparedLineage,
            Vec<crate::message::types::UserContentBlock>,
        ),
        SessionError,
    > {
        self.snapshot(session_id)?;
        let editor_content = match side {
            LineageSide::Before => {
                restorable_editor_content(active_user_boundary(source, message_id)?)?
            }
            LineageSide::After => {
                if !source.messages.iter().any(|message| matches!(message, MessageBlock::Assistant(assistant) if assistant.id == *message_id)) {
                    return Err(SessionError::UnknownBoundary { message_id: message_id.clone() });
                }
                Vec::new()
            }
        };
        let mut chosen = None;
        for _ in 0..16 {
            let node_id = SessionNodeId::from_uuid(self.identities.next_uuid())
                .map_err(|detail| SessionError::Catalog { detail })?;
            let uuid = self.identities.next_uuid();
            if !crate::runtime::identity::is_uuid_v7(uuid) {
                return Err(SessionError::Catalog {
                    detail: "expected UUIDv7".into(),
                });
            }
            let conversation_id = ConversationId::from_uuid(uuid)
                .map_err(|detail| SessionError::Catalog { detail })?;
            let database_path =
                conversation_database_path(&self.root, session_id, &conversation_id);
            if self
                .validate_new_node_identity(&node_id, &conversation_id)
                .is_ok()
                && self
                    .reject_pending_identity(session_id, &node_id, &conversation_id)
                    .is_ok()
                && !database_path
                    .parent()
                    .expect("conversation directory")
                    .exists()
            {
                let reservation = profile_stage!(
                    |times: &mut create_profile::StageTimes| &mut times.reserve_ns,
                    self.product.reserve_conversation(&conversation_id)
                );
                match reservation {
                    Ok(_reservation) => {
                        chosen = Some((node_id, conversation_id, database_path));
                        break;
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => {
                        return Err(SessionError::Io {
                            path: self.product.root().join("conversation-reservations"),
                            detail: error.to_string(),
                        });
                    }
                }
            }
        }
        let (node_id, conversation_id, database_path) =
            chosen.ok_or_else(|| SessionError::Catalog {
                detail: "identity reservation exhausted".into(),
            })?;
        let seed = lineage_cut(&conversation_id, source, Some(message_id), side)?;
        let source_artifacts = self.lineage_artifacts(source, &seed)?;
        let prepared = PreparedLineage {
            uploads: uploads::UploadRegistry::default(),
            session_id: session_id.clone(),
            node_id,
            conversation_id,
            state: template.clone(),
            // A branch node never re-projects its Session's root-lineage
            // display preview; `build_node_document` ignores this field.
            display_preview: None,
            database_path,
        };
        if let Err(error) = initialize_database(
            &self.product,
            &prepared.database_path,
            &prepared.conversation_id,
            &seed,
            source_artifacts.as_ref(),
        ) {
            self.discard_prepared_node(&prepared)?;
            return Err(error);
        }
        Ok((prepared, editor_content))
    }

    pub(super) fn prepare_session_with_ids(
        &self,
        template: &SessionPersistentState,
        session_id: SessionId,
        node_id: SessionNodeId,
        conversation_id: ConversationId,
        seed: &LineageSeed,
        source_artifacts: Option<&crate::tools::artifacts::ArtifactStore>,
    ) -> Result<PreparedLineage, SessionError> {
        self.reject_pending_identity(&session_id, &node_id, &conversation_id)?;
        self.validate_new_session_identity(&session_id, &node_id, &conversation_id)?;
        crate::runtime::local_storage::logical_operations::record_session_allocation_mkdir();
        fs::create_dir(self.root.join(session_id.as_str())).map_err(|error| {
            SessionError::Catalog {
                detail: format!("cannot reserve Session identity: {error}"),
            }
        })?;
        let database_path = conversation_database_path(&self.root, &session_id, &conversation_id);
        let prepared = PreparedLineage {
            uploads: uploads::UploadRegistry::default(),
            session_id,
            node_id,
            conversation_id,
            state: template.clone(),
            display_preview: seed_preview(seed),
            database_path,
        };
        if let Err(error) = initialize_database(
            &self.product,
            &prepared.database_path,
            &prepared.conversation_id,
            seed,
            source_artifacts,
        ) {
            self.discard_prepared_session(&prepared)?;
            return Err(error);
        }
        Ok(prepared)
    }

    /// Publishes a prepared independent Session and makes it active.
    ///
    /// The `publish_session` process-death boundaries bracket the **catalog
    /// visibility commit** — the atomic rename inside [`Self::commit`] — and
    /// nothing else. The destination database was already seeded by
    /// `prepare_*`, so the two sides are exactly the two durable worlds a
    /// crash can leave: a seeded destination the catalog does not name (the
    /// source lineage is still active and the seed is an inert orphan), or a
    /// catalog that names the complete new lineage. There is no third state.
    pub(crate) fn publish_session(
        &mut self,
        prepared: &PreparedLineage,
        origin: SessionNodeOrigin,
    ) -> Result<SessionSnapshot, SessionError> {
        let session_id = prepared.session_id.clone();
        let next = self.build_session_document(prepared, origin)?;
        crate::runtime::process_death::reach("before:publish_session");
        self.commit(next)?;
        crate::runtime::process_death::reach("after:publish_session");
        self.snapshot(&session_id)
    }

    /// Publishes a prepared branch node inside an existing Session and makes
    /// it the active node.
    ///
    /// The `publish_node` boundaries bracket the same visibility commit for a
    /// branch node. They are separate from the `publish_session` ones on
    /// purpose: a node publication is a different catalog transaction, with a
    /// different parent linkage and a different active-selection rule, so
    /// proving one atomic says nothing about the other.
    pub(crate) fn publish_node(
        &mut self,
        session_id: &SessionId,
        prepared: &PreparedLineage,
        parent: SessionNodeId,
        origin: SessionNodeOrigin,
    ) -> Result<SessionSnapshot, SessionError> {
        let next = self.build_node_document(session_id, prepared, parent, origin)?;
        crate::runtime::process_death::reach("before:publish_node");
        self.commit(next)?;
        crate::runtime::process_death::reach("after:publish_node");
        self.snapshot(session_id)
    }
}

/// Resolve an ordinary User boundary in the selected canonical append prefix.
/// A future Ledger message cannot supply a boundary. Projection inclusion is
/// selected independently by the inherited activation timeline.
fn active_user_boundary<'a>(
    source: &'a LineageReadCut,
    message_id: &MessageId,
) -> Result<&'a UserMessageBlock, SessionError> {
    source
        .messages
        .iter()
        .find_map(|message| match message {
            MessageBlock::User(user)
                if user.id == *message_id && user.kind == InboundKind::Message =>
            {
                Some(user)
            }
            _ => None,
        })
        .ok_or_else(|| SessionError::UnknownBoundary {
            message_id: message_id.clone(),
        })
}

/// Retain the exact supported canonical editor facts. The controller copies
/// required uploads before independent publication and converts these facts to
/// destination-owned draft receipts. Managed artifact blocks have no human draft
/// admission contract and are rejected explicitly.
fn restorable_editor_content(
    user: &UserMessageBlock,
) -> Result<Vec<UserContentBlock>, SessionError> {
    if user.content.iter().any(|content| {
        !matches!(
            content,
            UserContentBlock::Text(_) | UserContentBlock::UploadedFile(_)
        )
    }) {
        return Err(SessionError::Seed {
            detail: "fork/tree editor restoration supports human text and Session uploads only"
                .to_owned(),
        });
    }
    Ok(user.content.clone())
}

/// Cuts one source lineage at the boundary a `prepare_*` selected, and
/// reconstructs it as a destination-owned [`LineageSeed`].
///
/// The cut is the whole point, and it is taken over the source's *Surface
/// operation history*, not over its final Surface projection. `boundary` is
/// the message the destination stops before/after — `None` for a clone, which
/// stops before nothing.
/// The resulting seed contains only canonical conversation/domain state and
/// Surface and response provenance; the destination's new `ConversationId` starts a fresh
/// execution epoch rather than inheriting source execution or reminder state.
///
/// One forward replay of the source history decides two things at once:
///
/// - which operations the destination inherits. An operation is dropped when
///   it introduces an excluded identity: an `Append` of a message committed
///   at or after the boundary, or a `Replace` whose span holds anything
///   already excluded — a summary of work at or after the boundary is itself
///   work at or after the boundary. A `Replace` whose span is entirely below
///   the boundary is inherited, so a compaction the source already performed
///   over the copied prefix stays performed;
/// - which canonical rows the destination inherits: exactly those the
///   retained operations name. That is the closure the Surface needs — a
///   retained summary drags along the facts it retired — and it is why a
///   compaction that retired a `todo` result carries the result into the
///   destination while leaving it off the destination's Surface.
///
/// Cutting this way is what makes copying *closed* under the operations that
/// follow it. The destination's retained operations are the source's, so the
/// destination's own historical boundaries are the source's boundaries, and
/// a fork of a copy at a copied boundary means what a fork of the source at
/// that boundary means. Rebuilding the destination from the final projection
/// instead — one append per active message — yields the same Surface and a
/// history that never happened, in which a compaction summary appears to
/// predate a user message it actually postdates; forking the copy there then
/// silently carries that user message into the canonical prefix it was
/// supposed to cut before.
///
/// A boundary that excludes everything (a fork at the very first user
/// message) retains no operation and cuts to the empty lineage, which is
/// exactly the fresh conversation such a fork means.
pub(crate) fn lineage_cut(
    destination: &ConversationId,
    source: &LineageReadCut,
    boundary: Option<&MessageId>,
    side: LineageSide,
) -> Result<LineageSeed, SessionError> {
    let position = |id: &MessageId| {
        source
            .canonical
            .iter()
            .position(|message| message_id_of(message) == *id)
            .ok_or_else(|| SessionError::Seed {
                detail: format!(
                    "the source Surface history names {id}, which its Ledger does not carry"
                ),
            })
    };
    // A clone stops before nothing, so nothing is committed at or after its
    // boundary.
    let cut_at = match boundary {
        Some(id) => position(id)? + usize::from(side == LineageSide::After),
        None => source.canonical.len(),
    };

    let canonical = source.canonical[..cut_at].to_vec();
    let retained = source.surface_history[..cut_at].to_vec();
    let referenced: BTreeSet<MessageId> = canonical
        .iter()
        .map(|message| message.id().clone())
        .collect();
    let seed = remap_seed(
        destination,
        &canonical,
        &retained,
        &source.completed_responses,
        &source
            .turns
            .iter()
            .filter(|turn| {
                !turn.process_message_ids.is_empty()
                    || boundary.is_none()
                    || turn
                        .preceding_message_id
                        .as_ref()
                        .is_some_and(|id| referenced.contains(id))
            })
            .cloned()
            .collect::<Vec<_>>(),
        &source.raw_tool_arguments,
    )?;
    let projections = inherit_projections(
        destination,
        &canonical,
        seed.canonical(),
        &source.projections,
    )?;
    seed.with_projections(projections)
        .map_err(|error| SessionError::Seed {
            detail: error.to_string(),
        })
}

/// Remap immutable projection closure into the destination's own canonical and checkpoint domains.
#[allow(clippy::too_many_lines)] // One complete immutable dependency and occurrence remapping fold.
fn inherit_projections(
    destination: &ConversationId,
    originals: &[MessageBlock],
    mapped: &[MessageBlock],
    projections: &[crate::durable::projection::ProjectionCheckpoint],
) -> Result<Vec<crate::durable::projection::ProjectionCheckpoint>, SessionError> {
    use crate::context::checkpoint::CheckpointMessage;
    use crate::model::projected::CanonicalMessageRef;
    let fail = |detail: &str| SessionError::Seed {
        detail: detail.into(),
    };
    let messages: BTreeMap<_, _> = originals
        .iter()
        .zip(mapped)
        .map(|(a, b)| (a.id().clone(), b))
        .collect();
    let ids: BTreeMap<_, _> = projections
        .iter()
        .filter(|p| {
            p.candidate.base_frontier.0 <= mapped.len() as u64
                && p.activation.ledger.0 <= mapped.len() as u64
        })
        .map(|p| {
            (
                p.candidate.checkpoint_id.clone(),
                crate::runtime::identity::CheckpointId::new(format!(
                    "{destination}-checkpoint-{}",
                    p.activation.activation.0
                )),
            )
        })
        .collect();
    let installation =
        crate::runtime::identity::LineageInstallationId::new(format!("installation:{destination}"));
    let mut fingerprints = BTreeMap::new();
    let mut copied = Vec::new();
    let remap_ref = |reference: &mut CanonicalMessageRef| -> Result<(), SessionError> {
        let original = originals
            .iter()
            .find(|m| m.id() == &reference.message_id)
            .ok_or_else(|| fail("missing inherited Original"))?;
        if !reference.matches(original) {
            return Err(fail("changed inherited Original"));
        }
        let message = messages
            .get(&reference.message_id)
            .ok_or_else(|| fail("missing destination Original"))?;
        *reference = CanonicalMessageRef::of(destination, message);
        Ok(())
    };
    for source in projections
        .iter()
        .filter(|p| ids.contains_key(&p.candidate.checkpoint_id))
    {
        let mut checkpoint = source.clone();
        checkpoint.candidate.checkpoint_id = ids[&source.candidate.checkpoint_id].clone();
        checkpoint.candidate.expected_head = source
            .candidate
            .expected_head
            .as_ref()
            .map(|id| {
                ids.get(id)
                    .cloned()
                    .ok_or_else(|| fail("missing inherited preceding checkpoint"))
            })
            .transpose()?;
        checkpoint.candidate.read_cut.conversation_id = destination.clone();
        checkpoint.candidate.read_cut.ledger =
            crate::context::resources::LedgerFrontier(mapped.len() as u64);
        checkpoint.candidate.read_cut.journal = crate::context::resources::JournalFrontier(0);
        checkpoint.candidate.read_cut.transcript = crate::durable::TranscriptCursor::new(0);
        checkpoint.candidate.read_cut.projection_activation =
            source.candidate.read_cut.projection_activation;
        for item in &mut checkpoint.candidate.items {
            match &mut item.message {
                CheckpointMessage::Original(reference) => remap_ref(reference)?,
                CheckpointMessage::Derived(derived) => {
                    for reference in &mut derived.sources {
                        remap_ref(reference)?;
                    }
                    for reference in &mut derived.checkpoint_sources {
                        let old = reference.checkpoint_id.clone();
                        reference.checkpoint_id = ids
                            .get(&old)
                            .cloned()
                            .ok_or_else(|| fail("missing inherited checkpoint dependency"))?;
                        reference.content_digest = fingerprints
                            .get(&old)
                            .cloned()
                            .ok_or_else(|| fail("out-of-order inherited checkpoint dependency"))?;
                        reference.conversation_id = destination.clone();
                    }
                }
            }
            for source in &mut item.call_sources {
                remap_ref(&mut source.assistant)?;
            }
            if let Some(call) = &mut item.result_call {
                call.message.checkpoint_id = checkpoint.candidate.checkpoint_id.clone();
            }
        }
        checkpoint.fingerprint = checkpoint.candidate.fingerprint();
        fingerprints.insert(
            source.candidate.checkpoint_id.clone(),
            checkpoint.fingerprint.clone(),
        );
        checkpoint.activation.checkpoint_id = checkpoint.candidate.checkpoint_id.clone();
        // Origin stays in the source coordinate domain across every copy.
        checkpoint.activation.destination_installation = Some(installation.clone());
        copied.push(checkpoint);
    }
    Ok(copied)
}

/// Reconstructs a destination seed with destination-owned canonical message/occurrence
/// identities. Provider correlation strings remain unchanged. Response origins
/// retain their original execution owner as provenance, never destination execution state.
///
/// `canonical` is the destination's whole Ledger cut and `surface_history` is
/// the operation log that projects it, so the one identity map remaps both:
/// a `Replace` and the retired messages it names receive destination
/// identities that still agree.
pub(crate) fn remap_seed(
    destination: &ConversationId,
    canonical: &[MessageBlock],
    surface_history: &[SurfaceOp],
    completed_responses: &[crate::durable::response::CompletedResponseProvenance],
    turns: &[crate::durable::reading::TurnReadingProvenance],
    raw_tool_arguments: &[crate::durable::CanonicalRawToolArguments],
) -> Result<LineageSeed, SessionError> {
    let messages = canonical;
    let mut message_ids = BTreeMap::new();
    for (index, message) in messages.iter().enumerate() {
        let old = message_id_of(message);
        let new = MessageId::new(format!("{destination}-message-{}", index + 1));
        if message_ids.insert(old.clone(), new).is_some() {
            return Err(SessionError::Seed {
                detail: format!("source seed repeats MessageId {old}"),
            });
        }
    }

    let canonical = messages
        .iter()
        .map(|message| remap_message(message, &message_ids))
        .collect::<Result<Vec<_>, SessionError>>()?;
    let seeded = |id: &MessageId| {
        message_ids
            .get(id)
            .cloned()
            .ok_or_else(|| SessionError::Seed {
                detail: format!(
                    "the seeded Surface history names {id}, which the seeded Ledger does not carry"
                ),
            })
    };
    let surface_history = surface_history
        .iter()
        .map(|operation| match operation {
            SurfaceOp::Append { message_id } => Ok(SurfaceOp::Append {
                message_id: seeded(message_id)?,
            }),
        })
        .collect::<Result<Vec<_>, SessionError>>()?;
    let responses = completed_responses
        .iter()
        .filter_map(|response| {
            let closing = message_ids.get(&response.closing_message_id)?;
            let mut response = response.clone();
            response.closing_message_id = closing.clone();
            response.process_message_ids = response
                .process_message_ids
                .iter()
                .filter_map(|id| message_ids.get(id).cloned())
                .collect();
            response.retry_message_id = response
                .retry_message_id
                .as_ref()
                .and_then(|input| message_ids.get(input).cloned());
            Some(response)
        })
        .collect();
    let turns = turns
        .iter()
        .filter_map(|turn| {
            let mut copied = turn.clone();
            copied.process_message_ids = turn
                .process_message_ids
                .iter()
                .filter_map(|id| message_ids.get(id).cloned())
                .collect();
            if !turn.process_message_ids.is_empty() && copied.process_message_ids.is_empty() {
                return None;
            }
            copied.preceding_message_id = turn
                .preceding_message_id
                .as_ref()
                .and_then(|id| message_ids.get(id).cloned());
            copied.prompt_message_id = turn
                .prompt_message_id
                .as_ref()
                .and_then(|id| message_ids.get(id).cloned());
            Some(copied)
        })
        .collect();
    let raw = remap_raw_tool_arguments(raw_tool_arguments, &message_ids);
    LineageSeed::replayed(canonical, surface_history)
        .and_then(|seed| seed.with_completed_responses(responses))
        .and_then(|seed| seed.with_turns(turns))
        .and_then(|seed| seed.with_raw_tool_arguments(raw))
        .map_err(|error| SessionError::Seed {
            detail: error.to_string(),
        })
}

fn remap_raw_tool_arguments(
    raw_tool_arguments: &[crate::durable::CanonicalRawToolArguments],
    message_ids: &BTreeMap<MessageId, MessageId>,
) -> Vec<crate::durable::CanonicalRawToolArguments> {
    raw_tool_arguments
        .iter()
        .filter_map(|item| {
            message_ids
                .get(&item.message_id)
                .map(|id| crate::durable::CanonicalRawToolArguments {
                    message_id: id.clone(),
                    block_index: item.block_index,
                    bytes: item.bytes.clone(),
                })
        })
        .collect()
}

fn remap_message(
    message: &MessageBlock,
    message_ids: &BTreeMap<MessageId, MessageId>,
) -> Result<MessageBlock, SessionError> {
    let message_id = |id: &MessageId| {
        message_ids
            .get(id)
            .cloned()
            .ok_or_else(|| SessionError::Seed {
                detail: format!("source seed references unknown MessageId {id}"),
            })
    };

    match message {
        MessageBlock::User(user) => Ok(MessageBlock::User(UserMessageBlock {
            id: message_id(&user.id)?,
            content: user.content.clone(),
            source: user.source.clone(),
            kind: user.kind.clone(),
            timestamp: user.timestamp,
        })),
        MessageBlock::Assistant(assistant) => Ok(MessageBlock::Assistant(AssistantMessageBlock {
            id: message_id(&assistant.id)?,
            // Copied calls are history, not newly issued provider invocations.
            // Preserve the provider correlation on both sides of each occurrence.
            content: assistant.content.clone(),
        })),
        MessageBlock::Tool(tool) => Ok(MessageBlock::Tool(ToolMessageBlock {
            id: message_id(&tool.id)?,
            occurrence: crate::message::types::ToolCallOccurrenceRef {
                assistant_message_id: message_id(&tool.occurrence.assistant_message_id)?,
                block_index: tool.occurrence.block_index,
            },
            tool_call_id: tool.tool_call_id.clone(),
            tool_id: tool.tool_id.clone(),
            result: tool.result.clone(),
        })),
    }
}
