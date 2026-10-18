//! Same-C selected delivery spans within the existing native byte layout.
use super::{Connection, ConversationStoreError, OptionalExtension, params, storage};
use crate::message::types::MessageBlock;
use crate::runtime::identity::ConversationId;
use crate::runtime_client::content::CanonicalContentLocator;

pub(super) fn index(
    connection: &Connection,
    position: u64,
    message: &MessageBlock,
) -> Result<(), ConversationStoreError> {
    let tool = match message {
        MessageBlock::Tool(tool)
            if tool.result.status == crate::tools::types::ToolExecutionStatus::Success =>
        {
            Some(tool)
        }
        _ => None,
    };
    let total = tool.map_or(0, |tool| tool.result.deliveries.len());
    let total = u64::try_from(total).map_err(|_| storage("native delivery count exhausted"))?;
    connection.execute("UPDATE message_ledger SET observation_delivery_success=?2,observation_delivery_total=?3 WHERE position=?1", params![position,tool.is_some(),total]).map_err(|error|storage(error.to_string()))?;
    if let Some(tool) = tool {
        let key = format!("ledger:{position}");
        for (index, file) in tool.result.deliveries.iter().enumerate() {
            let ordinal =
                u32::try_from(index).map_err(|_| storage("native delivery ordinal exhausted"))?;
            super::content_encoding::put_json(
                connection,
                (
                    "canonical_file_delivery",
                    &key,
                    ordinal,
                    crate::runtime_client::content::ContentRepresentation::CanonicalJsonV1,
                ),
                1,
                file,
                false,
            )?;
        }
    }
    Ok(())
}

pub(super) fn selected(
    connection: &Connection,
    conversation: &ConversationId,
    locator: &CanonicalContentLocator,
    index: u64,
) -> Result<crate::durable::observation::NativeFileDelivery, ConversationStoreError> {
    super::content::validate_canonical_locator(locator)?;
    let cut = super::observation::cut(connection, conversation)?;
    let position = super::content::canonical_position(connection, locator)?;
    if position > cut.ledger_through {
        return Err(ConversationStoreError::InvalidReference(
            "delivery beyond original canonical cut".into(),
        ));
    }
    let (success,total):(Option<bool>,Option<u64>) = connection.query_row("SELECT observation_delivery_success,observation_delivery_total FROM message_ledger WHERE position=?1",[position],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|error|storage(error.to_string()))?;
    let success = success.ok_or_else(|| storage("native delivery status evidence unavailable"))?;
    let total = total.ok_or_else(|| storage("native delivery count evidence unavailable"))?;
    if !success {
        return Err(ConversationStoreError::InvalidReference(
            "canonical source is not a successful Tool delivery".into(),
        ));
    }
    if index >= total {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let ordinal = u32::try_from(index).map_err(|_| ConversationStoreError::InvalidContentRange)?;
    let key = format!("ledger:{position}");
    let end:Option<u64>=connection.query_row("SELECT readable_end FROM content_frontiers WHERE source_kind='canonical_file_delivery' AND source_id=?1 AND block_index=?2 AND representation='canonical_json_v1' AND revision=1",params![key,ordinal],|row|row.get(0)).optional().map_err(|error|storage(error.to_string()))?;
    let end = end.ok_or_else(|| storage("native selected delivery span unavailable"))?;
    if end > 65_536 {
        return Err(storage("native selected delivery exceeds finite capacity"));
    }
    let chunk = super::content::structural_field_read(
        connection,
        "canonical_file_delivery",
        &key,
        ordinal,
        0,
        65_536,
    )
    .map_err(|_| storage("native selected delivery span corrupt"))?;
    if chunk.next_offset.is_some() || chunk.end_offset != end {
        return Err(storage("native selected delivery span incomplete"));
    }
    let file: crate::tools::session_files::SessionFileReference =
        serde_json::from_slice(&chunk.bytes)
            .map_err(|_| storage("native selected delivery value corrupt"))?;
    validate_native_reference(&file)?;
    Ok(crate::durable::observation::NativeFileDelivery {
        cut,
        ledger_position: position,
        index,
        file,
    })
}

fn total(connection: &Connection, position: u64) -> Result<u64, ConversationStoreError> {
    let (success,total):(Option<bool>,Option<u64>)=connection.query_row("SELECT observation_delivery_success,observation_delivery_total FROM message_ledger WHERE position=?1",[position],|row|Ok((row.get(0)?,row.get(1)?))).map_err(|_|storage("native delivery evidence unavailable"))?;
    let success = success.ok_or_else(|| storage("native delivery status evidence unavailable"))?;
    if !success {
        return Err(ConversationStoreError::InvalidReference(
            "canonical source is not a successful Tool delivery".into(),
        ));
    }
    total.ok_or_else(|| storage("native delivery count evidence unavailable"))
}
fn metadata_end(
    connection: &Connection,
    position: u64,
    index: u64,
) -> Result<u64, ConversationStoreError> {
    let ordinal = u32::try_from(index).map_err(|_| ConversationStoreError::InvalidContentRange)?;
    connection.query_row("SELECT readable_end FROM content_frontiers WHERE source_kind='canonical_file_delivery' AND source_id=?1 AND block_index=?2 AND representation='canonical_json_v1' AND revision=1",params![format!("ledger:{position}"),ordinal],|row|row.get(0)).map_err(|_|storage("native selected delivery span unavailable"))
}
pub(super) fn read(
    connection: &Connection,
    position: u64,
    index: u64,
    offset: u64,
    max_bytes: u32,
) -> Result<crate::durable::observation::ObservationValueChunk, ConversationStoreError> {
    if index >= total(connection, position)? {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    metadata_end(connection, position, index)?;
    super::content::structural_field_read(
        connection,
        "canonical_file_delivery",
        &format!("ledger:{position}"),
        u32::try_from(index).map_err(|_| ConversationStoreError::InvalidContentRange)?,
        offset,
        max_bytes,
    )
}
pub(super) fn head(
    connection: &Connection,
    position: u64,
    offset: u64,
    limit: usize,
) -> Result<crate::runtime_client::delivery::RuntimeClientDeliveryHead, ConversationStoreError> {
    if limit == 0 || limit > 32 || offset > crate::runtime_client::content::CONTENT_SAFE_INTEGER_MAX
    {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let total = total(connection, position)?;
    if offset > total {
        return Err(ConversationStoreError::InvalidContentRange);
    }
    let end = offset
        .checked_add(u64::try_from(limit.min(8)).expect("bounded delivery head"))
        .ok_or(ConversationStoreError::InvalidContentRange)?
        .min(total);
    let mut items = Vec::with_capacity(limit.min(8));
    for index in offset..end {
        let encoded_bytes = metadata_end(connection, position, index)?;
        let file = if encoded_bytes <= 1024 {
            let chunk = read(connection, position, index, 0, 1024)?;
            Some(
                serde_json::from_slice(&chunk.bytes)
                    .map_err(|_| storage("native delivery inline metadata corrupt"))?,
            )
        } else {
            None
        };
        items.push(crate::runtime_client::delivery::RuntimeClientDeliveryItem {
            index,
            locator: CanonicalContentLocator::ByLedgerPosition { position },
            metadata_deferred: file.is_none(),
            file,
            encoded_bytes,
            metadata_domain: format!("messages.ledger:{position}.deliveries.{index}.metadata"),
        });
    }
    Ok(crate::runtime_client::delivery::RuntimeClientDeliveryHead {
        domain: format!("messages.ledger:{position}.deliveries"),
        total,
        items,
        next_offset: (end < total).then_some(end),
    })
}

fn validate_native_reference(
    file: &crate::tools::session_files::SessionFileReference,
) -> Result<(), ConversationStoreError> {
    let path = std::path::Path::new(&file.path);
    if file.path.is_empty()
        || file.path.len() > 4096
        || file.path.contains('\0')
        || path.is_absolute()
        || path
            .components()
            .any(|component| !matches!(component, std::path::Component::Normal(_)))
        || path.file_name().and_then(|name| name.to_str()) != Some(file.name.as_str())
        || file.name.len() > 255
        || file
            .description
            .as_ref()
            .is_some_and(|value| value.chars().count() > 512)
        || file.mime_type.len() > 128
        || file.scope.device.parse::<u64>().is_err()
        || file.scope.inode.parse::<u64>().is_err()
    {
        return Err(ConversationStoreError::InvalidReference(
            "delivery fields exceed native declared metadata bounds".into(),
        ));
    }
    Ok(())
}
