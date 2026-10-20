//! Session metadata publication: names, display previews, activity, the
//! Session-owned model selection and settings, and the current node. Every
//! change goes through the Catalog visibility commit.

use super::DisplayPreviewSubject;
use super::SessionCatalog;
use super::SessionError;
use super::SessionPersistentState;
use super::SessionSnapshot;
use super::catalog_commit::normalize_name;
use super::invalidations::SessionSummaryInvalidations;
use super::projection::activity_at;
use super::projection::preview_of;
use crate::durable::inbox::ConversationStore;
use crate::runtime::identity::SessionId;
use crate::runtime::identity::SessionNodeId;
use chrono::DateTime;
use chrono::Utc;
use std::sync::Arc;

impl SessionCatalog {
    /// Atomically names a Session. This touches metadata only.
    ///
    /// Naming never moves, rewrites, or re-identifies anything: the Session
    /// keeps its identity, its graph, and its conversations, and gains only
    /// the label `/resume` shows in place of its first message.
    pub(crate) fn rename(
        &mut self,
        session_id: &SessionId,
        name: &str,
    ) -> Result<SessionSnapshot, SessionError> {
        let name = Some(normalize_name(name)?);
        let mut next = self.document.clone();
        let session =
            next.sessions
                .get_mut(session_id)
                .ok_or_else(|| SessionError::UnknownSession {
                    session_id: session_id.clone(),
                })?;
        session.name = name;
        session.updated_at = Utc::now();
        self.commit(next)?;
        self.snapshot(session_id)
    }

    /// Publishes the Session's derived display projection, once.
    ///
    /// The field is set only when currently absent: an already-present
    /// projection returns `Ok(false)` and performs **no** commit — no
    /// generation bump, no write — so racing publishers and repeated repairs
    /// are idempotent by construction. Like `commit_uploads`, this touches
    /// exactly one field: `updated_at`, `settings_revision`, and every other
    /// fact stay as they were. An unknown (e.g. already deleted) Session
    /// propagates [`SessionError::UnknownSession`]; publication never
    /// resurrects one.
    ///
    /// The `publish_display_preview` process-death boundaries bracket the
    /// same catalog visibility commit as [`Self::publish_session`]: either
    /// the old document without the projection is authoritative, or the new
    /// one with it is; there is no third state.
    pub(crate) fn publish_display_preview(
        &mut self,
        session_id: &SessionId,
        preview: &str,
    ) -> Result<bool, SessionError> {
        let mut next = self.document.clone();
        let session =
            next.sessions
                .get_mut(session_id)
                .ok_or_else(|| SessionError::UnknownSession {
                    session_id: session_id.clone(),
                })?;
        if session.display_preview.is_some() {
            return Ok(false);
        }
        session.display_preview = Some(preview.to_owned());
        crate::runtime::process_death::reach("before:publish_display_preview");
        // The invalidation follows the *visibility* point, never precedes it:
        // a pre-visibility failure announces nothing, while the distinct
        // post-visibility durability-uncertain outcome is a visible metadata
        // change and is announced before its error is propagated.
        let committed = self.commit(next);
        if committed.is_ok() || committed.as_ref().is_err_and(SessionError::committed) {
            self.summary_invalidations.record(session_id);
        }
        committed?;
        crate::runtime::process_death::reach("after:publish_display_preview");
        Ok(true)
    }

    /// Publish a committed human-message timestamp, independent of metadata edits.
    /// Replayed or delayed observations never move the row backwards.
    pub(crate) fn publish_activity(
        &mut self,
        id: &SessionId,
        timestamp: DateTime<Utc>,
    ) -> Result<bool, SessionError> {
        let mut next = self.document.clone();
        let session = next
            .sessions
            .get_mut(id)
            .ok_or_else(|| SessionError::UnknownSession {
                session_id: id.clone(),
            })?;
        if timestamp <= activity_at(session) {
            return Ok(false);
        }
        session.last_prompt_at = Some(timestamp);
        let committed = self.commit(next);
        if committed.is_ok() || committed.as_ref().is_err_and(SessionError::committed) {
            // Ordering changes page membership even when no Session was created.
            self.summary_invalidations.record_change(id, true);
        }
        committed?;
        Ok(true)
    }

    /// Explicit recovery projection. Never called by list/summary reads.
    pub(crate) fn activity_subject(
        &self,
        id: &SessionId,
    ) -> Result<Option<DateTime<Utc>>, SessionError> {
        let session =
            self.document
                .sessions
                .get(id)
                .ok_or_else(|| SessionError::UnknownSession {
                    session_id: id.clone(),
                })?;
        let mut latest = None;
        for node in session.nodes.values() {
            let path = self.database_path(id, &node.conversation_id);
            let store = self
                .inspect_store(node.conversation_id.clone(), &path)
                .map_err(SessionError::Store)?;
            let mut position = None;
            loop {
                let page = store
                    .load_canonical_page(position, 128)
                    .map_err(SessionError::Store)?;
                for message in page.messages {
                    if let crate::message::types::MessageBlock::User(user) = message
                        && let Some(time) =
                            super::super::session_activity::human_message_time(&user)
                    {
                        latest =
                            Some(latest.map_or(time, |previous: DateTime<Utc>| previous.max(time)));
                    }
                }
                match page.next_position {
                    Some(next) => position = Some(next),
                    None => break,
                }
            }
        }
        Ok(latest)
    }

    /// The post-commit summary invalidation log of this product root.
    ///
    /// Shared by every clone of the catalog, so the observer an App Server
    /// connection holds sees publications made through any handle.
    pub(crate) fn summary_invalidations(&self) -> Arc<SessionSummaryInvalidations> {
        self.summary_invalidations.clone()
    }

    /// Reads the display-preview subject of a Session's root lineage from its
    /// durable conversation store.
    ///
    /// This is the one store-reading half of the explicit repair seam. It is
    /// deliberately *not* called by listing or summary projection: only the
    /// repair algorithm (reopen/compose/recovery) and startup planning use
    /// it. The root node is the subject on purpose: branch nodes are seeded
    /// copies of a source lineage, so the row would otherwise change what it
    /// says whenever the active node moves, while the Session it names is
    /// still the same Session that started with the same message.
    ///
    /// # Errors
    ///
    /// Returns [`SessionError`] when the Session is unknown or its root
    /// conversation store cannot be read.
    pub(crate) fn display_preview_subject(
        &self,
        session_id: &SessionId,
    ) -> Result<DisplayPreviewSubject, SessionError> {
        let session =
            self.document
                .sessions
                .get(session_id)
                .ok_or_else(|| SessionError::UnknownSession {
                    session_id: session_id.clone(),
                })?;
        let root = session
            .nodes
            .values()
            .find(|node| node.parent.is_none())
            .ok_or_else(|| SessionError::Catalog {
                detail: format!("Session {} has no root node", session.id),
            })?;
        let path = self.database_path(&session.id, &root.conversation_id);
        let store = self
            .inspect_store(root.conversation_id.clone(), &path)
            .map_err(SessionError::Store)?;
        let head = store.load_head().map_err(SessionError::Store)?;
        let page = store
            .load_user_message_boundaries_page(head.revision, 0, 1)
            .map_err(SessionError::Store)?;
        Ok(match page.boundaries.first() {
            None => DisplayPreviewSubject::NoBoundary,
            Some(boundary) => match preview_of(&boundary.message) {
                Some(preview) => DisplayPreviewSubject::Derived(preview),
                None => DisplayPreviewSubject::Unrenderable,
            },
        })
    }

    /// Persists the accepted live model configuration for the active
    /// Session. The `ConversationRuntime` remains the live authority; this
    /// record is used when that runtime is replaced and recovered later.
    #[cfg(test)]
    pub(crate) fn persist_model(
        &mut self,
        session_id: &SessionId,
        model: crate::model::session::SessionModelConfig,
    ) -> Result<(), SessionError> {
        let revision = self.settings_revision(session_id)?;
        let (_, mut settings) = self.lineage(session_id, None)?;
        settings.model = Some(model);
        self.replace_settings(session_id, revision, settings)
            .map(|_| ())
    }

    /// Revision of explicit Session selections, independent of graph/name edits.
    /// # Errors
    /// Invalid identities, revisions or storage are returned explicitly.
    pub fn settings_revision(&self, id: &SessionId) -> Result<u64, SessionError> {
        self.document
            .sessions
            .get(id)
            .map(|s| s.settings_revision)
            .ok_or_else(|| SessionError::UnknownSession {
                session_id: id.clone(),
            })
    }

    /// Compare-and-swap at catalog rename visibility. A failed durability barrier
    /// still consumes the revision; retry must reread the authoritative state.
    /// # Errors
    /// Invalid identities, revisions or storage are returned explicitly.
    pub(crate) fn replace_settings(
        &mut self,
        id: &SessionId,
        expected: u64,
        settings: SessionPersistentState,
    ) -> Result<u64, SessionError> {
        let mut next = self.document.clone();
        let session = next
            .sessions
            .get_mut(id)
            .ok_or_else(|| SessionError::UnknownSession {
                session_id: id.clone(),
            })?;
        if session.settings_revision != expected {
            return Err(SessionError::StaleSettings {
                expected,
                actual: session.settings_revision,
            });
        }
        let revision = expected
            .checked_add(1)
            .ok_or_else(|| SessionError::Catalog {
                detail: "Session settings revision exhausted".into(),
            })?;
        session.settings_revision = revision;
        session.state = settings;
        session.updated_at = Utc::now();
        self.commit(next)?;
        Ok(revision)
    }

    /// Atomically changes only the addressed Session graph's default node.
    /// Existing runtime attachments retain their own Conversation identity.
    /// # Errors
    /// Invalid identities, revisions or storage are returned explicitly.
    pub(crate) fn set_current_node(
        &mut self,
        session_id: &SessionId,
        node_id: Option<&SessionNodeId>,
    ) -> Result<SessionSnapshot, SessionError> {
        let next = self.build_current_node_document(session_id, node_id)?;
        self.commit(next)?;
        self.snapshot(session_id)
    }
}
