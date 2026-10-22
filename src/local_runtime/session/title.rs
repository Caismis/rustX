//! Generated labels are independent Catalog metadata. Publication cannot rename a
//! Session, recreate a deleted row or grant permission for another model call.

use super::{
    PersistedSession, PreparedLineage, SESSION_NAME_LIMIT, SESSION_TITLE_BYTE_LIMIT,
    SessionCatalog, SessionError, SessionGeneratedTitle, SessionId, SessionTitleProvenance,
};
use crate::runtime::identity::{AuxiliaryCallId, ConversationId, MessageId};

/// Domain acceptance is deliberately stricter than manual-name normalization:
/// a model must return one nonempty, bounded line, not prose to truncate.
pub(crate) fn accept_title(text: &str) -> Result<String, SessionError> {
    let title = text.trim();
    if title.is_empty()
        || title.len() > SESSION_TITLE_BYTE_LIMIT
        || title.chars().count() > SESSION_NAME_LIMIT
        || title.chars().any(char::is_control)
        || title.contains(['\u{2028}', '\u{2029}'])
    {
        return Err(SessionError::InvalidName);
    }
    Ok(title.to_owned())
}

pub(super) fn validate_title(
    title: &SessionGeneratedTitle,
    session: &PersistedSession,
) -> Result<(), SessionError> {
    if accept_title(&title.text)? != title.text {
        return Err(SessionError::InvalidName);
    }
    if let SessionTitleProvenance::Local {
        conversation_id, ..
    } = &title.provenance
        && !session
            .nodes
            .values()
            .any(|node| node.parent.is_none() && &node.conversation_id == conversation_id)
    {
        return Err(SessionError::Catalog {
            detail: "generated title source does not belong to the Session root".into(),
        });
    }
    Ok(())
}

impl SessionCatalog {
    /// A bounded Catalog-only recheck at native admission and dispatch.
    pub(crate) fn title_eligible(&self, id: &SessionId, conversation: &ConversationId) -> bool {
        !self.document.deletions.contains_key(id)
            && self.document.sessions.get(id).is_some_and(|session| {
                session.name.is_none()
                    && session.generated_title.is_none()
                    && session
                        .nodes
                        .values()
                        .any(|node| node.parent.is_none() && &node.conversation_id == conversation)
            })
    }

    /// Called only after native execution has confirmed its local settlement.
    /// Exact replay is idempotent, including a prior post-rename error. All
    /// source identity comes from the admitted operation, never a display read.
    pub(crate) fn publish_generated_title(
        &mut self,
        id: &SessionId,
        conversation_id: &ConversationId,
        message_id: &MessageId,
        call_id: &AuxiliaryCallId,
        text: &str,
    ) -> Result<bool, SessionError> {
        let candidate = SessionGeneratedTitle {
            text: accept_title(text)?,
            provenance: SessionTitleProvenance::Local {
                conversation_id: conversation_id.clone(),
                message_id: message_id.clone(),
                call_id: call_id.clone(),
            },
        };
        if self
            .document
            .sessions
            .get(id)
            .is_some_and(|session| session.generated_title.as_ref() == Some(&candidate))
        {
            // Exact candidate replay confirms the already-visible document's
            // durability without allocating another Catalog generation.
            self.confirm_catalog_durability()
                .map_err(|error| SessionError::CatalogCommit {
                    error: super::CatalogCommitError::CommittedButDurabilityUncertain {
                        path: self.path.clone(),
                        detail: error.to_string(),
                    },
                })?;
            return Ok(false);
        }
        if !self.title_eligible(id, conversation_id) {
            return Ok(false);
        }
        let mut next = self.document.clone();
        next.sessions
            .get_mut(id)
            .expect("eligible Session")
            .generated_title = Some(candidate);
        crate::runtime::process_death::reach("before:publish_generated_title");
        let committed = self.commit(next);
        if committed.is_ok() || committed.as_ref().is_err_and(SessionError::committed) {
            self.summary_invalidations.record(id);
        }
        committed?;
        crate::runtime::process_death::reach("after:publish_generated_title");
        Ok(true)
    }

    /// Copy the saved label at preparation, independently of the historical
    /// message cut. No source execution identity or quota enters the target.
    pub(crate) fn copy_saved_label(
        &self,
        source: &SessionId,
        prepared: &mut PreparedLineage,
    ) -> Result<(), SessionError> {
        let source_record =
            self.document
                .sessions
                .get(source)
                .ok_or_else(|| SessionError::UnknownSession {
                    session_id: source.clone(),
                })?;
        prepared.name.clone_from(&source_record.name);
        prepared.generated_title =
            source_record
                .generated_title
                .as_ref()
                .map(|title| SessionGeneratedTitle {
                    text: title.text.clone(),
                    provenance: SessionTitleProvenance::Inherited {
                        source_session: source.clone(),
                    },
                });
        Ok(())
    }
}

#[cfg(test)]
mod tests;
