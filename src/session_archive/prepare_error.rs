//! Safe semantic preparation failures. Never carry raw OS/provider diagnostics.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionArchivePrepareError {
    UnknownSession,
    Busy,
    DescendantUnavailable,
    ConversationUnavailable,
    ArtifactUnavailable,
    CorruptAuthority,
    Storage,
    Cancelled,
}
impl std::fmt::Display for SessionArchivePrepareError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::UnknownSession => "Cannot export: the Session does not exist",
            Self::Busy => "Cannot prepare archive: export capacity is full; retry after an active export finishes",
            Self::DescendantUnavailable => "Cannot export complete Session: a required descendant is missing or unreadable",
            Self::ConversationUnavailable => "Cannot export: required Conversation history is missing or unreadable",
            Self::ArtifactUnavailable => "Cannot export complete Session: required artifact content is missing, unreadable, changed, or still being written; wait for active tools to finish and retry",
            Self::CorruptAuthority => "Cannot export: durable history or lineage is invalid; inspect native storage diagnostics",
            Self::Storage => "Cannot prepare archive: storage or cut acquisition failed; check storage availability and retry",
            Self::Cancelled => "Archive preparation was cancelled",
        })
    }
}
impl std::error::Error for SessionArchivePrepareError {}
impl From<std::io::Error> for SessionArchivePrepareError {
    fn from(error: std::io::Error) -> Self {
        if error.kind() == std::io::ErrorKind::Interrupted {
            Self::Cancelled
        } else {
            Self::Storage
        }
    }
}
impl From<crate::local_runtime::session::SessionError> for SessionArchivePrepareError {
    fn from(error: crate::local_runtime::session::SessionError) -> Self {
        use crate::local_runtime::session::SessionError;
        match error {
            SessionError::UnknownSession { .. } | SessionError::DeletingSession { .. } => {
                Self::UnknownSession
            }
            SessionError::Catalog { .. } => Self::CorruptAuthority,
            _ => Self::Storage,
        }
    }
}

impl From<crate::local_runtime::session_ownership::OwnershipInspectionError>
    for SessionArchivePrepareError
{
    fn from(error: crate::local_runtime::session_ownership::OwnershipInspectionError) -> Self {
        use crate::local_runtime::session_ownership::OwnershipInspectionError as Ownership;
        match error {
            Ownership::Invalid | Ownership::DeletedConversation => Self::CorruptAuthority,
            Ownership::ConversationUnavailable { descendant: true } => Self::DescendantUnavailable,
            Ownership::ConversationUnavailable { descendant: false } => {
                Self::ConversationUnavailable
            }
            Ownership::Cancelled => Self::Cancelled,
            Ownership::UnknownSession => Self::UnknownSession,
        }
    }
}
