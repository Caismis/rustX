//! Finite catalog observation. Immutable metadata remains in the native catalog;
//! byte reads slice the retained original string without cloning a whole Node.
use super::projection::validate_page_limit;
use super::{
    LineageSide, SessionCatalog, SessionError, SessionId, SessionNodeId, SessionNodeOrigin,
};
use crate::{
    conversation::SurfaceRevision,
    runtime::identity::{ConversationId, MessageId},
};
use base64::Engine as _;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionNodeObservation {
    pub ordinal: u64,
    pub id: SessionNodeId,
    pub parent: Option<SessionNodeId>,
    pub conversation_id: ConversationId,
    pub origin: SessionNodeOriginObservation,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionNodeOriginObservation {
    New,
    Clone {
        source_session: SessionId,
        source_node: SessionNodeId,
        source_surface_revision: SurfaceRevision,
    },
    Fork {
        source_session: SessionId,
        source_node: SessionNodeId,
        source_surface_revision: SurfaceRevision,
        source_message: Option<MessageId>,
        source_message_ref: Option<SessionNodeValueRef>,
        side: LineageSide,
    },
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct SessionNodeValueRef {
    pub session_id: SessionId,
    pub node_id: SessionNodeId,
    pub ordinal: u64,
    pub field: SessionNodeValueField,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub readable_end: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SessionNodeValueField {
    OriginSourceMessage,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionNodeValueChunk {
    pub reference: SessionNodeValueRef,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub offset: u64,
    pub bytes_base64: String,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub readable_end: u64,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub next_offset: Option<u64>,
}
/// One historical native Surface boundary, with a finite immutable canonical locator.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SessionUserMessageBoundaryObservation {
    pub surface_revision: SurfaceRevision,
    pub locator: crate::observation::content::CanonicalContentLocator,
    pub message: Option<crate::message::types::UserMessageBlock>,
    pub block_count: u64,
    pub content: Vec<crate::observation::cut::ContentBinding>,
    pub collections: Vec<crate::observation::cut::CollectionFrontier>,
}
impl SessionCatalog {
    fn observation_node(
        &self,
        session_id: &SessionId,
        node_id: &SessionNodeId,
    ) -> Result<&super::SessionNode, SessionError> {
        self.snapshot(session_id)?;
        self.document
            .sessions
            .get(session_id)
            .and_then(|session| session.nodes.get(node_id))
            .ok_or_else(|| SessionError::UnknownNode {
                session_id: session_id.clone(),
                node_id: node_id.clone(),
            })
    }
    pub(crate) fn observe_node(
        &self,
        session_id: &SessionId,
        node_id: &SessionNodeId,
    ) -> Result<SessionNodeObservation, SessionError> {
        let node = self.observation_node(session_id, node_id)?;
        let origin = match &node.origin {
            SessionNodeOrigin::New => SessionNodeOriginObservation::New,
            SessionNodeOrigin::Clone {
                source_session,
                source_node,
                source_surface_revision,
            } => SessionNodeOriginObservation::Clone {
                source_session: source_session.clone(),
                source_node: source_node.clone(),
                source_surface_revision: *source_surface_revision,
            },
            SessionNodeOrigin::Fork {
                source_session,
                source_node,
                source_surface_revision,
                source_message,
                side,
                ..
            } => {
                let inline = source_message.as_str().len() <= 256;
                SessionNodeOriginObservation::Fork {
                    source_session: source_session.clone(),
                    source_node: source_node.clone(),
                    source_surface_revision: *source_surface_revision,
                    source_message: inline.then(|| source_message.clone()),
                    source_message_ref: (!inline).then(|| SessionNodeValueRef {
                        session_id: session_id.clone(),
                        node_id: node_id.clone(),
                        ordinal: node.ordinal,
                        field: SessionNodeValueField::OriginSourceMessage,
                        readable_end: source_message.as_str().len() as u64,
                    }),
                    side: *side,
                }
            }
        };
        Ok(SessionNodeObservation {
            ordinal: node.ordinal,
            id: node.id.clone(),
            parent: node.parent.clone(),
            conversation_id: node.conversation_id.clone(),
            origin,
        })
    }
    pub(crate) fn observe_tree(
        &self,
        session_id: &SessionId,
        offset: usize,
        limit: usize,
    ) -> Result<(Vec<SessionNodeObservation>, Option<usize>), SessionError> {
        validate_page_limit(limit, super::SESSION_TREE_PAGE_LIMIT)?;
        self.snapshot(session_id)?;
        let nodes = self.node_observation_index.get(session_id).ok_or_else(|| {
            SessionError::UnknownSession {
                session_id: session_id.clone(),
            }
        })?;
        let end = offset.saturating_add(limit).min(nodes.len());
        let result = nodes
            .get(offset..end)
            .unwrap_or_default()
            .iter()
            .map(|id| self.observe_node(session_id, id))
            .collect::<Result<Vec<_>, _>>()?;
        let next = (end < nodes.len()).then_some(end);
        Ok((result, next))
    }
    pub(crate) fn node_value_read(
        &self,
        reference: &SessionNodeValueRef,
        offset: u64,
        max_bytes: u32,
    ) -> Result<SessionNodeValueChunk, SessionError> {
        let invalid = || SessionError::Catalog {
            detail: "invalid immutable node value range".into(),
        };
        if max_bytes == 0
            || max_bytes > 65_536
            || offset > 9_007_199_254_740_991
            || reference.readable_end > 9_007_199_254_740_991
            || offset
                .checked_add(u64::from(max_bytes))
                .is_none_or(|end| end > 9_007_199_254_740_991)
        {
            return Err(invalid());
        }
        let node = self.observation_node(&reference.session_id, &reference.node_id)?;
        let SessionNodeOrigin::Fork { source_message, .. } = &node.origin else {
            return Err(invalid());
        };
        let bytes = source_message.as_str().as_bytes();
        if reference.ordinal != node.ordinal
            || reference.readable_end != bytes.len() as u64
            || offset > reference.readable_end
        {
            return Err(invalid());
        }
        let start = usize::try_from(offset).map_err(|_| invalid())?;
        let end = start.saturating_add(max_bytes as usize).min(bytes.len());
        Ok(SessionNodeValueChunk {
            reference: reference.clone(),
            offset,
            bytes_base64: base64::engine::general_purpose::STANDARD.encode(&bytes[start..end]),
            readable_end: reference.readable_end,
            next_offset: (end < bytes.len()).then_some(end as u64),
        })
    }
}

// This derived order is installed with catalog visibility, never rebuilt by readers.
pub(super) fn node_index(
    document: &super::CatalogDocument,
) -> std::collections::BTreeMap<SessionId, Vec<SessionNodeId>> {
    document
        .sessions
        .iter()
        .map(|(id, session)| {
            let mut nodes: Vec<_> = session.nodes.values().collect();
            nodes.sort_by_key(|node| node.ordinal);
            (
                id.clone(),
                nodes.into_iter().map(|node| node.id.clone()).collect(),
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn t13_t15_catalog_node_headers_and_immutable_range_reads_are_bounded() {
        let (root, mut catalog, _) = super::super::tests::open_catalog();
        let session_id = catalog.document.sessions.keys().next().unwrap().clone();
        let source_node = catalog.document.sessions[&session_id].active_node.clone();
        let mut next = catalog.document.clone();
        let original = "é\\\"".repeat(300_000);
        for index in 0_u64..65 {
            let ordinal = next.next_node_ordinal;
            next.next_node_ordinal += 1;
            let id = SessionNodeId::new(format!(
                "node_00000000-0000-7000-8000-{:012x}",
                1000 + index
            ));
            let conversation_id = ConversationId::new(format!(
                "conv_00000000-0000-7000-8000-{:012x}",
                1000 + index
            ));
            next.sessions.get_mut(&session_id).unwrap().nodes.insert(
                id.clone(),
                super::super::SessionNode {
                    ordinal,
                    id,
                    parent: Some(source_node.clone()),
                    conversation_id,
                    origin: SessionNodeOrigin::Fork {
                        source_session: session_id.clone(),
                        source_node: source_node.clone(),
                        source_surface_revision: SurfaceRevision::INITIAL,
                        source_message: MessageId::new(if index == 0 {
                            original.clone()
                        } else {
                            "x".repeat(70_000)
                        }),
                        source_locator:
                            crate::observation::content::CanonicalContentLocator::ByLedgerPosition {
                                position: index + 1,
                            },
                        side: LineageSide::Before,
                    },
                },
            );
        }
        catalog.commit(next).unwrap();
        drop(catalog);
        let catalog = SessionCatalog::open_existing(root.path()).unwrap().unwrap();
        // Observation does not reread or deserialize the persisted catalog per chunk.
        std::fs::rename(&catalog.path, catalog.path.with_extension("parked")).unwrap();
        let (mut offset, mut count) = (0, 0);
        loop {
            let (nodes, next) = catalog.observe_tree(&session_id, offset, 32).unwrap();
            assert!(nodes.len() <= 32);
            assert!(serde_json::to_vec(&nodes).unwrap().len() < 65_536);
            assert!(nodes.iter().all(|node| !matches!(
                &node.origin,
                SessionNodeOriginObservation::Fork {
                    source_message: Some(_),
                    ..
                }
            )));
            count += nodes.len();
            let Some(next) = next else { break };
            assert!(next > offset);
            offset = next;
        }
        assert_eq!(count, 66);
        let node_id = SessionNodeId::new("node_00000000-0000-7000-8000-0000000003e8");
        let node = catalog.observe_node(&session_id, &node_id).unwrap();
        let SessionNodeOriginObservation::Fork {
            source_message_ref: Some(reference),
            ..
        } = node.origin
        else {
            panic!("finite source locator")
        };
        let (mut offset, mut bytes) = (0, Vec::new());
        loop {
            let chunk = catalog.node_value_read(&reference, offset, 65_535).unwrap();
            let value = base64::engine::general_purpose::STANDARD
                .decode(chunk.bytes_base64)
                .unwrap();
            assert!(value.len() <= 65_535);
            assert!(!value.is_empty());
            bytes.extend(value);
            let Some(next) = chunk.next_offset else { break };
            assert!(next > offset);
            offset = next;
        }
        assert_eq!(bytes, original.as_bytes());
        let mut forged = reference.clone();
        forged.ordinal += 1;
        assert!(catalog.node_value_read(&forged, 0, 1).is_err());
        forged = reference.clone();
        forged.readable_end += 1;
        assert!(catalog.node_value_read(&forged, 0, 1).is_err());
        assert!(
            catalog
                .node_value_read(&reference, reference.readable_end + 1, 1)
                .is_err()
        );
    }
}
