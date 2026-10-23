//! Bounded Catalog reads: list and tree pages, summaries, snapshots, lineage
//! and file-source lookups, and the persisted display projections they read.
//! No read here opens a conversation store unless the query requires it.

use super::PersistedSession;
use super::SESSION_LIST_PAGE_LIMIT;
use super::SESSION_PREVIEW_LIMIT;
use super::SESSION_TREE_PAGE_LIMIT;
use super::SessionCatalog;
use super::SessionError;
use super::SessionListPage;
use super::SessionNode;
use super::SessionNodeOrigin;
use super::SessionNodePage;
use super::SessionPersistentState;
use super::SessionSnapshot;
use super::SessionSummary;
use super::creation::conversation_database_path;
use crate::conversation::SurfaceOp;
use crate::conversation::SurfaceRevision;
use crate::conversation::message_id_of;
use crate::durable::CanonicalStore;
use crate::durable::LineageSeed;
use crate::durable::inbox::InboxStore;
use crate::message::types::InboundKind;
use crate::message::types::MessageBlock;
use crate::message::types::UserContentBlock;
use crate::message::types::UserMessageBlock;
use crate::runtime::identity::ConversationId;
use crate::runtime::identity::MessageId;
use crate::runtime::identity::SessionId;
use crate::runtime::identity::SessionNodeId;
use chrono::DateTime;
use chrono::Utc;
use std::collections::BTreeMap;
use std::path::Path;
use std::path::PathBuf;

impl SessionCatalog {
    pub(crate) fn schedule_bindings(&self) -> Vec<crate::scheduler::ScheduleBinding> {
        self.document
            .sessions
            .values()
            .filter(|session| !self.document.deletions.contains_key(&session.id))
            .flat_map(|session| {
                session
                    .nodes
                    .values()
                    .map(|node| crate::scheduler::ScheduleBinding {
                        session_id: session.id.clone(),
                        node_id: node.id.clone(),
                        conversation_id: node.conversation_id.clone(),
                    })
            })
            .collect()
    }

    /// Returns one bounded, searchable page of all durable Sessions.
    /// Usage classification does not filter visibility or manufacture client focus.
    ///
    /// Ordering is descending human-message time, then ascending Session identity.
    /// The offset is a domain-specific
    /// continuation: there is no global maximum number of Sessions, and
    /// callers can reach older matching rows by requesting the returned
    /// offset. Only the requested page is materialized for the projection.
    ///
    /// The projection reads catalog metadata only. Every row's `preview` is
    /// the persisted `display_preview`; this function never opens a
    /// conversation store, and a missing projection yields `preview: None`
    /// rather than a repair — repair is an explicit seam, never a list-time
    /// side effect.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when the requested page size is outside the
    /// native bound.
    pub fn list_page(
        &self,
        query: Option<&str>,
        offset: usize,
        limit: usize,
    ) -> Result<SessionListPage, SessionError> {
        validate_page_limit(limit, SESSION_LIST_PAGE_LIMIT)?;
        let query = query.map(|value| value.trim().to_lowercase());
        let mut matching = 0_usize;
        let mut page = Vec::with_capacity(limit);
        let mut has_more = false;
        let mut ordered: Vec<_> = self.document.sessions.values().collect();
        ordered.sort_by(|a, b| {
            activity_at(b)
                .cmp(&activity_at(a))
                .then_with(|| a.id.cmp(&b.id))
        });
        for session in ordered {
            // A row is searched by what it shows. An unnamed Session shows
            // its first user message, so matching only identity and name
            // would hide exactly the rows a user has to recognize by their
            // content.
            let summary = project_summary(session);
            let matches = query.as_ref().is_none_or(|query| {
                session.id.as_str().to_lowercase().contains(query)
                    || session
                        .name
                        .as_ref()
                        .is_some_and(|name| name.to_lowercase().contains(query))
                    || summary
                        .generated_title
                        .as_ref()
                        .is_some_and(|title| title.text.to_lowercase().contains(query))
                    || summary
                        .preview
                        .as_ref()
                        .is_some_and(|preview| preview.to_lowercase().contains(query))
            });
            if !matches {
                continue;
            }
            if matching < offset {
                matching += 1;
                continue;
            }
            if page.len() == limit {
                has_more = true;
                break;
            }
            page.push(summary);
            matching += 1;
        }
        let next_offset = has_more.then_some(offset + page.len());
        Ok(SessionListPage {
            sessions: page,
            next_offset,
        })
    }

    /// Read display metadata by exact durable identity, without runtime composition.
    ///
    /// The projection is metadata-only: `preview` is the persisted
    /// `display_preview`, never a conversation-store read.
    ///
    /// # Errors
    /// Unknown/deleting identities are returned.
    pub fn summary(&self, id: &SessionId) -> Result<SessionSummary, SessionError> {
        if self.document.deletions.contains_key(id) {
            return Err(SessionError::DeletingSession {
                session_id: id.clone(),
            });
        }
        let session =
            self.document
                .sessions
                .get(id)
                .ok_or_else(|| SessionError::UnknownSession {
                    session_id: id.clone(),
                })?;
        Ok(project_summary(session))
    }

    /// All persisted identities in deterministic order, including unused Sessions.
    #[must_use]
    pub fn persisted_session_ids(&self) -> Vec<SessionId> {
        {
            let mut sessions: Vec<_> = self.document.sessions.values().collect();
            sessions.sort_by_key(|session| session.ordinal);
            sessions
                .into_iter()
                .map(|session| session.id.clone())
                .collect()
        }
    }

    /// Classifies usage by explicit identity; this never chooses a Session.
    /// # Errors
    /// Invalid identities, revisions or storage are returned explicitly.
    pub fn is_session_unused(&self, id: &SessionId) -> Result<bool, SessionError> {
        let session =
            self.document
                .sessions
                .get(id)
                .ok_or_else(|| SessionError::UnknownSession {
                    session_id: id.clone(),
                })?;
        self.is_unused(session)
    }

    /// Whether one persisted Session is an untouched empty shell.
    /// This monotonic usage fact does not control catalog visibility or allocation.
    ///
    /// The classification is deliberately narrow. A Session is unused only
    /// when *all* of the following hold:
    ///
    /// - it has exactly one root node, created as [`SessionNodeOrigin::New`]
    ///   with no parent/branch lineage;
    /// - that node's durable conversation is still at its initial Surface
    ///   revision, with no canonical message;
    /// - that conversation has never committed a durable inbound acceptance.
    ///
    /// The last condition is the monotonic usage fact owned by the
    /// conversation's durable authority (`ConversationStore::has_accepted_inbound`),
    /// not a pair of independently changing projections. Reading the current
    /// Surface head and the current Pending Inbox separately could assemble a
    /// state that never existed — a pre-adoption head plus a post-adoption
    /// empty inbox — and classify a Session whose prompt was already durably
    /// accepted as unused. The acceptance watermark cannot do that: durable
    /// acceptance is the used transition, adoption never rewinds it, and once
    /// a Session is used it never classifies as unused again.
    ///
    /// Provenance is equally decisive: a clone, fork, or tree branch is user
    /// work by construction, so it is used even when its destination
    /// conversation is legitimately empty at the selected cut (a fork at the
    /// very first user message seeds the empty lineage). Session-local
    /// metadata — a name, a model choice — never counts as use.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when the Session's durable conversation
    /// cannot be read.
    pub(super) fn is_unused(&self, session: &PersistedSession) -> Result<bool, SessionError> {
        if session.nodes.len() != 1 {
            return Ok(false);
        }
        let Some(node) = session.nodes.values().next() else {
            return Ok(false);
        };
        if node.parent.is_some() || node.origin != SessionNodeOrigin::New {
            return Ok(false);
        }
        let path = self.database_path(&session.id, &node.conversation_id);
        let store = self
            .inspect_store(node.conversation_id.clone(), &path)
            .map_err(SessionError::Store)?;
        let head = store.load_head().map_err(SessionError::Store)?;
        if head.revision != SurfaceRevision::INITIAL || !head.active_message_ids.is_empty() {
            return Ok(false);
        }
        #[cfg(test)]
        if let Some(gate) = &self.classification_gate {
            gate.enter();
        }
        // The decisive read is last and is a single atomic query against a
        // monotonic watermark: every acceptance committed before this read
        // is observed, however adoption raced the head read above, and an
        // acceptance committed only after it legitimately postdates this
        // classification.
        Ok(!store.has_accepted_inbound().map_err(SessionError::Store)?)
    }

    /// Arms the test-only classification boundary gate, parking the next
    /// `is_unused` read between its Surface-head observation and its
    /// acceptance-watermark read.
    #[cfg(test)]
    pub(crate) fn arm_classification_gate(
        &mut self,
    ) -> std::sync::Arc<crate::runtime::conversation_runtime::Gate> {
        let gate = std::sync::Arc::new(crate::runtime::conversation_runtime::Gate::default());
        gate.arm();
        self.classification_gate = Some(gate.clone());
        gate
    }

    /// Returns one Session snapshot.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError::UnknownSession`] when `id` is not persisted.
    pub fn snapshot(&self, id: &SessionId) -> Result<SessionSnapshot, SessionError> {
        if self.document.deletions.contains_key(id) {
            return Err(SessionError::DeletingSession {
                session_id: id.clone(),
            });
        }

        let session =
            self.document
                .sessions
                .get(id)
                .ok_or_else(|| SessionError::UnknownSession {
                    session_id: id.clone(),
                })?;
        snapshot_of(session)
    }

    /// Returns one bounded page of graph nodes.
    pub(crate) fn node_page(
        &self,
        session_id: &SessionId,
        offset: usize,
        limit: usize,
    ) -> Result<SessionNodePage, SessionError> {
        validate_page_limit(limit, SESSION_TREE_PAGE_LIMIT)?;
        let session =
            self.document
                .sessions
                .get(session_id)
                .ok_or_else(|| SessionError::UnknownSession {
                    session_id: session_id.clone(),
                })?;
        let mut ordered: Vec<_> = session.nodes.values().collect();
        ordered.sort_by_key(|node| node.ordinal);
        let nodes = ordered
            .into_iter()
            .skip(offset)
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        let next_offset =
            (offset + nodes.len() < session.nodes.len()).then_some(offset + nodes.len());
        Ok(SessionNodePage { nodes, next_offset })
    }

    /// Returns one named lineage and its Session-local state.
    /// # Errors
    /// Invalid identities, revisions or storage are returned explicitly.
    pub fn lineage(
        &self,
        session_id: &SessionId,
        node_id: Option<&SessionNodeId>,
    ) -> Result<(SessionNode, SessionPersistentState), SessionError> {
        if self.document.deletions.contains_key(session_id) {
            return Err(SessionError::DeletingSession {
                session_id: session_id.clone(),
            });
        }

        let session =
            self.document
                .sessions
                .get(session_id)
                .ok_or_else(|| SessionError::UnknownSession {
                    session_id: session_id.clone(),
                })?;
        let node_id = node_id.unwrap_or(&session.active_node);
        let node = session
            .nodes
            .get(node_id)
            .ok_or_else(|| SessionError::UnknownNode {
                session_id: session_id.clone(),
                node_id: node_id.clone(),
            })?;
        Ok((node.clone(), session.state.clone()))
    }

    /// Resolve an attached Conversation without consulting the graph's default node.
    pub(crate) fn conversation_lineage(
        &self,
        session_id: &SessionId,
        conversation_id: &ConversationId,
    ) -> Result<(SessionNode, SessionPersistentState), SessionError> {
        self.snapshot(session_id)?;
        let session = &self.document.sessions[session_id];
        let node = session
            .nodes
            .values()
            .find(|node| node.conversation_id == *conversation_id)
            .ok_or_else(|| SessionError::Catalog {
                detail: format!(
                    "Conversation {conversation_id} does not belong to Session {session_id}"
                ),
            })?;
        Ok((node.clone(), session.state.clone()))
    }

    /// Historical file delivery resolution never uses the viewing Session's
    /// default node. Deleted/unpublished original lineages are unavailable.
    pub(crate) fn file_source(
        &self,
        conversation: &ConversationId,
    ) -> Result<(SessionId, SessionNodeId), SessionError> {
        for (id, session) in &self.document.sessions {
            if !self.document.deletions.contains_key(id)
                && let Some(node) = session
                    .nodes
                    .values()
                    .find(|node| &node.conversation_id == conversation)
            {
                return Ok((id.clone(), node.id.clone()));
            }
        }
        Err(SessionError::Catalog {
            detail: "original delivery Session unavailable".into(),
        })
    }

    /// Recheck the original published filesystem mapping at each read fence.
    /// A Workspace reassociation cannot finish an in-flight read from the old mapping.
    pub(crate) fn file_mapping_matches(
        &self,
        conversation: &ConversationId,
        session: &SessionId,
        node: &SessionNodeId,
        cwd: &Path,
    ) -> bool {
        self.lineage(session, Some(node))
            .is_ok_and(|(lineage, state)| {
                lineage.conversation_id == *conversation && state.cwd == cwd
            })
    }

    /// Whether any Session or node in this catalog names `conversation`.
    ///
    /// The catalog is the sole reachability authority for a lineage: a
    /// destination database that `prepare_*` seeded but whose publication
    /// never committed exists on disk and is named by nothing, so it is
    /// neither selectable nor resumable.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "conformance reachability probe")
    )]
    pub(crate) fn names_conversation(&self, conversation: &ConversationId) -> bool {
        self.document
            .sessions
            .values()
            .flat_map(|session| session.nodes.values())
            .any(|node| node.conversation_id == *conversation)
    }

    /// Returns the private database path for a known node.
    pub(crate) fn database_path(
        &self,
        session_id: &SessionId,
        conversation_id: &ConversationId,
    ) -> PathBuf {
        conversation_database_path(&self.root, session_id, conversation_id)
    }
}

pub(super) fn activity_at(session: &PersistedSession) -> DateTime<Utc> {
    session
        .created_at
        .max(session.last_prompt_at.unwrap_or(session.created_at))
}

fn project_summary(session: &PersistedSession) -> SessionSummary {
    SessionSummary {
        ownership_generation: session.ownership_generation.to_string(),
        cwd: session.state.cwd.clone(),
        id: session.id.clone(),
        name: session.name.clone(),
        generated_title: session.generated_title.clone(),
        // The persisted derived projection. A missing value is legitimate
        // (no ordinary user message yet, or none with renderable text) and
        // is projected as `None`; repair is explicit, never list-time.
        preview: session.display_preview.clone(),
        updated_at: activity_at(session),
        active_node: session.active_node.clone(),
    }
}

fn snapshot_of(session: &PersistedSession) -> Result<SessionSnapshot, SessionError> {
    let active_node =
        session
            .nodes
            .get(&session.active_node)
            .ok_or_else(|| SessionError::UnknownNode {
                session_id: session.id.clone(),
                node_id: session.active_node.clone(),
            })?;
    Ok(SessionSnapshot {
        id: session.id.clone(),
        name: session.name.clone(),
        generated_title: session.generated_title.clone(),
        created_at: session.created_at,
        updated_at: session.updated_at,
        active_node: session.active_node.clone(),
        active_conversation_id: active_node.conversation_id.clone(),
        node_count: session.nodes.len(),
    })
}

/// Renders one canonical user message as the bounded single line a `/resume`
/// row shows for an unnamed Session. Non-text content contributes nothing:
/// a Session opened with only an image has no first line to show, and saying
/// so with `None` is more honest than inventing one.
pub(crate) fn preview_of(message: &UserMessageBlock) -> Option<String> {
    // Only the first normalized 121 characters can affect the exact 120
    // character preview. Stream the native blocks without joining their bodies.
    let mut preview = String::with_capacity(SESSION_PREVIEW_LIMIT * 4);
    let (mut count, mut gap, mut first) = (0_usize, false, true);
    for block in &message.content {
        let UserContentBlock::Text(block) = block else {
            continue;
        };
        if !first {
            gap = !preview.is_empty();
        }
        first = false;
        for character in block.text.chars() {
            if character.is_whitespace() {
                gap = !preview.is_empty();
                continue;
            }
            if gap {
                preview.push(' ');
                count += 1;
                gap = false;
            }
            if count > SESSION_PREVIEW_LIMIT {
                return Some(truncate(preview, SESSION_PREVIEW_LIMIT));
            }
            preview.push(character);
            count += 1;
            if count > SESSION_PREVIEW_LIMIT {
                return Some(truncate(preview, SESSION_PREVIEW_LIMIT));
            }
        }
    }
    (!preview.is_empty()).then(|| truncate(preview, SESSION_PREVIEW_LIMIT))
}

/// Derives a prepared lineage's display projection from its **frozen seed**,
/// never from the source Session's live state or persisted projection.
///
/// The semantics match `load_user_message_boundaries_page` exactly: the first
/// Surface `Append` of an ordinary (`InboundKind::Message`) user message in
/// the seed's retained operation history is the subject, rendered by
/// [`preview_of`]. A seed that retains no such append — a fresh lineage, or
/// a fork whose cut excludes every ordinary user message — yields `None`,
/// and a first append with no renderable text yields `None` rather than
/// shifting the subject to a later message.
pub(super) fn seed_preview(seed: &LineageSeed) -> Option<String> {
    let ledger: BTreeMap<MessageId, &MessageBlock> = seed
        .canonical()
        .iter()
        .map(|message| (message_id_of(message), message))
        .collect();
    for operation in seed.surface_history() {
        let SurfaceOp::Append { message_id } = operation;
        if let Some(MessageBlock::User(user)) = ledger.get(message_id)
            && user.kind == InboundKind::Message
        {
            return preview_of(user);
        }
    }
    None
}

/// Collapses every run of whitespace — line breaks included — into one space
/// and trims the ends.
pub(super) fn single_line(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Bounds one already single-line label to `limit` characters, marking the
/// cut so a truncated line never reads as the whole message.
pub(super) fn truncate(text: String, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text;
    }
    let kept = text
        .chars()
        .take(limit.saturating_sub(1))
        .collect::<String>();
    format!("{}\u{2026}", kept.trim_end())
}

pub(super) fn validate_page_limit(limit: usize, maximum: usize) -> Result<(), SessionError> {
    if limit == 0 || limit > maximum {
        return Err(SessionError::Catalog {
            detail: format!("page limit must be between 1 and {maximum}"),
        });
    }
    Ok(())
}
