//! Bounded editor recovery from an immutable fork origin. The source canonical
//! message remains the byte authority; destination uploads own their receipts.
use super::{
    connection::{client_error, domain},
    protocol::{ErrorData, MethodResult, RpcError},
};
use crate::{
    durable::ConversationStore,
    local_runtime::{
        session::{LineageSide, SessionId, SessionNodeId},
        session_controller::SessionController,
    },
    runtime_client::content::{ContentChunk, ContentRepresentation, ContentSource},
};
use base64::Engine;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditorLocator {
    pub session_id: SessionId,
    pub node_id: SessionNodeId,
    pub source_session: SessionId,
    pub source_node: SessionNodeId,
    pub source_surface_revision: crate::conversation::SurfaceRevision,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EditorPage {
    pub locator: EditorLocator,
    pub block_index: u32,
    pub chunk: Option<ContentChunk>,
    pub upload: Option<crate::local_runtime::session::uploads::UploadReceipt>,
    pub next_block: Option<u32>,
}
pub(super) fn locator(
    session_id: SessionId,
    node: &crate::local_runtime::session::observation::SessionNodeObservation,
) -> Option<EditorLocator> {
    let crate::local_runtime::session::observation::SessionNodeOriginObservation::Fork {
        source_session,
        source_node,
        source_surface_revision,
        side: LineageSide::Before,
        ..
    } = &node.origin
    else {
        return None;
    };
    Some(EditorLocator {
        session_id,
        node_id: node.id.clone(),
        source_session: source_session.clone(),
        source_node: source_node.clone(),
        source_surface_revision: *source_surface_revision,
    })
}
pub(super) async fn read(
    sessions: SessionController,
    requested: EditorLocator,
    block_index: u32,
    offset: u64,
    max_bytes: u32,
) -> Result<MethodResult, RpcError> {
    if max_bytes == 0
        || max_bytes > crate::runtime_client::content::CONTENT_READ_MAX_BYTES
        || offset > crate::runtime_client::content::CONTENT_SAFE_INTEGER_MAX
    {
        return Err(domain(ErrorData::InvalidParams));
    }
    let (destination, origin) = sessions
        .acquire_read_origin(&requested.session_id, &requested.node_id)
        .await
        .map_err(super::connection::session_error)?;
    if origin.side != LineageSide::Before
        || origin.source_session != requested.source_session
        || origin.source_node != requested.source_node
        || origin.source_surface_revision != requested.source_surface_revision
    {
        return Err(domain(ErrorData::InvalidParams));
    }
    let source_locator = origin.source_locator;
    if !matches!(
        source_locator,
        crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition { .. }
    ) {
        return Err(domain(ErrorData::InvalidParams));
    }
    let source = sessions
        .acquire_read(&requested.source_session, Some(&requested.source_node))
        .await
        .map_err(super::connection::session_error)?;
    let _destination = destination;
    let (mut page, upload_block) = tokio::task::spawn_blocking(move || {
        let store = crate::durable::SqliteConversationStore::open_existing(
            source.conversation_id.clone(),
            &source.database_path,
        )
        .map_err(store_error)?
        .with_lifecycle(source.allocation);
        let index = crate::message::types::ContentBlockIndex::new(block_index);
        let content_source = ContentSource::CanonicalMessageBlock {
            component: crate::runtime_client::content::CanonicalContentComponent::Payload,
            locator: source_locator.clone(),
            block_index: index,
        };
        let text = store
            .content_descriptor(&content_source, ContentRepresentation::Utf8Text)
            .map_err(store_error)?;
        let (chunk, upload_block) = if let Some(reference) = text {
            (
                Some(
                    store
                        .content_read(&reference, offset, max_bytes)
                        .map_err(store_error)?,
                ),
                None,
            )
        } else {
            if offset != 0 {
                return Err(
                    crate::runtime_client::types::RuntimeClientError::InvalidRequest {
                        message: "upload descriptors do not have byte offsets".into(),
                    },
                );
            }
            (None, Some(read_upload(&store, &content_source)?))
        };
        let next_block = next_block(&store, &source_locator, block_index)?;
        Ok((
            EditorPage {
                locator: requested,
                block_index,
                chunk,
                upload: None,
                next_block,
            },
            upload_block,
        ))
    })
    .await
    .map_err(|_| domain(ErrorData::OperationFailed))?
    .map_err(client_error)?;
    if let Some(block) = upload_block {
        page.upload = Some(
            sessions
                .catalog
                .lock()
                .await
                .editor_upload_receipt(&page.locator.session_id, &block)
                .map_err(super::connection::session_error)?,
        );
    }
    Ok(MethodResult::EditorPage { page })
}

fn next_block(
    store: &dyn crate::durable::ConversationStore,
    locator: &crate::runtime_client::content::CanonicalContentLocator,
    block_index: u32,
) -> Result<Option<u32>, crate::runtime_client::RuntimeClientError> {
    let Some(index) = block_index.checked_add(1) else {
        return Ok(None);
    };
    let source = ContentSource::CanonicalMessageBlock {
        component: crate::runtime_client::content::CanonicalContentComponent::Payload,
        locator: locator.clone(),
        block_index: crate::message::types::ContentBlockIndex::new(index),
    };
    for representation in [
        ContentRepresentation::Utf8Text,
        ContentRepresentation::CanonicalJsonV1,
    ] {
        if store
            .content_descriptor(&source, representation)
            .map_err(store_error)?
            .is_some()
        {
            return Ok(Some(index));
        }
    }
    Ok(None)
}

fn read_upload(
    store: &dyn crate::durable::ConversationStore,
    content_source: &ContentSource,
) -> Result<crate::message::types::UserContentBlock, crate::runtime_client::types::RuntimeClientError>
{
    let reference = store
        .content_descriptor(content_source, ContentRepresentation::CanonicalJsonV1)
        .map_err(store_error)?
        .ok_or_else(
            || crate::runtime_client::types::RuntimeClientError::InvalidRequest {
                message: "editor block unavailable".into(),
            },
        )?;
    if reference.readable_end > u64::from(crate::runtime_client::content::CONTENT_READ_MAX_BYTES) {
        return Err(
            crate::runtime_client::types::RuntimeClientError::ObservationUnavailable {
                message: "editor upload descriptor exceeds bounded read".into(),
            },
        );
    }
    let bytes = store
        .content_read(
            &reference,
            0,
            crate::runtime_client::content::CONTENT_READ_MAX_BYTES,
        )
        .map_err(store_error)?;
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(bytes.data_base64)
        .map_err(
            |_| crate::runtime_client::types::RuntimeClientError::ObservationUnavailable {
                message: "invalid canonical editor bytes".into(),
            },
        )?;
    let block: crate::message::types::UserContentBlock =
        serde_json::from_slice(&bytes).map_err(|_| {
            crate::runtime_client::types::RuntimeClientError::ObservationUnavailable {
                message: "invalid canonical editor block".into(),
            }
        })?;
    if !matches!(
        block,
        crate::message::types::UserContentBlock::UploadedFile(_)
    ) {
        return Err(
            crate::runtime_client::types::RuntimeClientError::InvalidRequest {
                message: "editor block is not an upload".into(),
            },
        );
    }
    Ok(block)
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "Error conversion consumes the native Result carrier"
)]
fn store_error(
    error: crate::durable::ConversationStoreError,
) -> crate::runtime_client::types::RuntimeClientError {
    crate::runtime_client::types::RuntimeClientError::ObservationUnavailable {
        message: format!("editor canonical source unavailable: {error}"),
    }
}
