//! Destination ownership for the managed references in an exact lineage cut.
use super::{
    Arc, BTreeMap, BTreeSet, ConversationId, LineageReadCut, LineageSeed, MessageBlock, Path,
    SessionCatalog, SessionError, UserContentBlock,
};
use crate::message::types::AssistantContentBlock;
use crate::runtime::ArtifactId;
use crate::tools::ArtifactStore;
use crate::tools::ToolResultContent;

fn references(messages: &mut [MessageBlock], mut visit: impl FnMut(&mut ArtifactId)) {
    for message in messages {
        match message {
            MessageBlock::User(user) => {
                for block in &mut user.content {
                    match block {
                        UserContentBlock::File(r) => visit(&mut r.artifact_id),
                        UserContentBlock::Image(r) => visit(&mut r.artifact_id),
                        _ => {}
                    }
                }
            }
            MessageBlock::Assistant(assistant) => {
                for block in &mut assistant.content {
                    if let AssistantContentBlock::Image(r) = block {
                        visit(&mut r.artifact_id);
                    }
                }
            }
            MessageBlock::Tool(tool) => {
                for r in &mut tool.result.artifacts {
                    visit(&mut r.artifact_id);
                }
                for block in &mut tool.result.content {
                    match block {
                        ToolResultContent::File(r) => visit(&mut r.artifact_id),
                        ToolResultContent::Image(r) => visit(&mut r.artifact_id),
                        _ => {}
                    }
                }
            }
        }
    }
}

impl SessionCatalog {
    pub(super) fn lineage_artifacts(
        &self,
        source: &LineageReadCut,
        seed: &LineageSeed,
    ) -> Result<Option<ArtifactStore>, SessionError> {
        let mut needed = false;
        references(&mut seed.canonical().to_vec(), |_| needed = true);
        if !needed {
            return Ok(None);
        }
        let session = self
            .document
            .sessions
            .values()
            .find(|session| {
                session
                    .nodes
                    .values()
                    .any(|node| node.conversation_id == source.conversation_id)
            })
            .ok_or_else(|| SessionError::Catalog {
                detail: "lineage artifact source is not owned by this catalog".into(),
            })?;
        let path = self.database_path(&session.id, &source.conversation_id);
        let root = path.parent().expect("conversation directory");
        let access =
            crate::runtime::local_storage::ConversationAccess::existing(&self.product, root)
                .map_err(|e| SessionError::Io {
                    path: root.into(),
                    detail: e.to_string(),
                })?;
        ArtifactStore::new(source.conversation_id.clone(), root)
            .map(|store| Some(store.with_lifecycle(Some(Arc::new(access)))))
            .map_err(|e| SessionError::Io {
                path: root.into(),
                detail: e.to_string(),
            })
    }
}

pub(super) fn materialize(
    source: &ArtifactStore,
    conversation: &ConversationId,
    root: &Path,
    access: Arc<crate::runtime::local_storage::ConversationAccess>,
    seed: &LineageSeed,
) -> Result<LineageSeed, SessionError> {
    let copy = || -> Result<LineageSeed, Box<dyn std::error::Error>> {
        let destination =
            ArtifactStore::new(conversation.clone(), root)?.with_lifecycle(Some(access));
        let mut canonical = seed.canonical().to_vec();
        let mut ids = BTreeSet::new();
        references(&mut canonical, |id| {
            ids.insert(id.clone());
        });
        // Finite preparation work, independently of presentation/transport limits.
        if ids.len() > 256 {
            return Err("lineage artifact count exceeds 256".into());
        }
        let mut remaining = 64 * 1024 * 1024;
        let mut remapped = BTreeMap::new();
        for id in ids {
            remapped.insert(
                id.clone(),
                destination.copy_from(source, &id, &mut remaining)?,
            );
        }
        references(&mut canonical, |id| *id = remapped[id].clone());
        Ok(LineageSeed::replayed(
            canonical,
            seed.surface_history().to_vec(),
            seed.checkpoints().clone(),
        )?
        .with_completed_responses(seed.completed_responses().to_vec())?)
    };
    copy().map_err(|e| SessionError::Io {
        path: root.into(),
        detail: e.to_string(),
    })
}
