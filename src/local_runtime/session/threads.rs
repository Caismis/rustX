//! Stable public Thread projections of native Session nodes. No history store.
use super::{SessionCatalog, SessionError, SessionId, SessionNodeId};
use crate::runtime::identity::ConversationId;

#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Thread {
    pub id: ConversationId,
    pub session_id: SessionId,
    pub node_id: SessionNodeId,
    pub name: Option<String>,
    pub archived: bool,
    pub dynamic_tools: Vec<crate::tools::host::HostToolDefinition>,
}

/// Finite catalog rows omit declaration bodies; one Thread read owns those.
#[derive(
    Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ThreadSummary {
    pub id: ConversationId,
    pub session_id: SessionId,
    pub node_id: SessionNodeId,
    pub name: Option<String>,
    pub archived: bool,
}

pub(crate) enum ThreadUpdate {
    Name(Option<String>),
    Archive(bool),
}

impl SessionCatalog {
    pub(crate) fn thread(&self, id: &ConversationId) -> Result<Thread, SessionError> {
        for session in self.document.sessions.values() {
            if let Some(node) = session
                .nodes
                .values()
                .find(|node| node.conversation_id == *id)
            {
                return Ok(Thread {
                    id: id.clone(),
                    session_id: session.id.clone(),
                    node_id: node.id.clone(),
                    name: node.name.clone(),
                    archived: node.archived,
                    dynamic_tools: node.dynamic_tools.clone(),
                });
            }
        }
        Err(SessionError::Catalog {
            detail: "Thread does not exist".into(),
        })
    }

    pub(crate) fn threads(
        &self,
        offset: usize,
        limit: usize,
        archived: bool,
    ) -> Result<(Vec<ThreadSummary>, Option<usize>), SessionError> {
        if limit == 0 || limit > 64 {
            return Err(SessionError::Catalog {
                detail: "Thread page limit must be 1..64".into(),
            });
        }
        let mut nodes: Vec<_> = self
            .document
            .sessions
            .values()
            .flat_map(|session| session.nodes.values().map(move |node| (&session.id, node)))
            .filter(|(_, node)| node.archived == archived)
            .collect();
        nodes.sort_by_key(|(_, node)| node.ordinal);
        let next = offset
            .checked_add(limit)
            .ok_or_else(|| SessionError::Catalog {
                detail: "invalid Thread offset".into(),
            })?;
        let data = nodes
            .iter()
            .skip(offset)
            .take(limit)
            .map(|(session, node)| ThreadSummary {
                id: node.conversation_id.clone(),
                session_id: (*session).clone(),
                node_id: node.id.clone(),
                name: node.name.clone(),
                archived: node.archived,
            })
            .collect();
        Ok((data, (next < nodes.len()).then_some(next)))
    }

    pub(crate) fn update_thread(
        &mut self,
        id: &ConversationId,
        update: ThreadUpdate,
    ) -> Result<Thread, SessionError> {
        let thread = self.thread(id)?;
        if matches!(&update, ThreadUpdate::Name(Some(name)) if name.trim().is_empty() || name.len() > 512)
        {
            return Err(SessionError::Catalog {
                detail: "Thread name must contain 1..512 bytes".into(),
            });
        }
        let mut next = self.document.clone();
        let node = next
            .sessions
            .get_mut(&thread.session_id)
            .expect("resolved Session")
            .nodes
            .get_mut(&thread.node_id)
            .expect("resolved node");
        match update {
            ThreadUpdate::Name(name) => node.name = name,
            ThreadUpdate::Archive(archived) => node.archived = archived,
        }
        self.commit(next)?;
        self.thread(id)
    }
}
