//! Bounded native first-Human source metadata, independent of optional observation.
use super::{
    Connection, ConversationId, ConversationStoreError, MessageBlock, MessageId, OptionalExtension,
    storage,
};
use crate::durable::store::canonical::FirstHumanSource;

pub(in crate::durable::sqlite) const FIRST_HUMAN_SQL: &str = "SELECT l.position,l.message_id,l.title_source_text,l.position>(SELECT message_count FROM bootstrap_identity WHERE id=1),s.consumed,s.call_id,EXISTS(SELECT 1 FROM auxiliary_title_calls b WHERE b.message_id=l.message_id),EXISTS(SELECT 1 FROM auxiliary_requests r WHERE r.call_id=s.call_id),c.settled_sequence FROM message_ledger l INDEXED BY message_ledger_first_human_idx LEFT JOIN auxiliary_title_sources s ON s.message_id=l.message_id LEFT JOIN auxiliary_calls c ON c.call_id=s.call_id WHERE l.ordinary_human=1 ORDER BY l.position LIMIT 1";

pub(in crate::durable::sqlite) fn read(
    connection: &Connection,
    conversation: &ConversationId,
) -> Result<Option<FirstHumanSource>, ConversationStoreError> {
    let row = connection
        .query_row(FIRST_HUMAN_SQL, [], |row| {
            Ok((
                row.get::<_, u64>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, bool>(3)?,
                row.get::<_, Option<bool>>(4)?,
                row.get::<_, Option<String>>(5)?,
                row.get::<_, bool>(6)?,
                row.get::<_, bool>(7)?,
                row.get::<_, Option<u64>>(8)?,
            ))
        })
        .optional()
        .map_err(|error| storage(format!("first Human source: {error}")))?;
    let Some((
        position,
        message_id,
        text,
        local,
        consumed,
        association,
        has_binding,
        has_start,
        settled,
    )) = row
    else {
        return Ok(None);
    };
    if (consumed.is_none() && has_binding)
        || consumed == Some(true) && (association.is_none() || !has_start)
        || consumed == Some(false) && has_start
        || consumed == Some(false) && association.is_some() && settled.is_some()
    {
        return Err(storage(
            "native Title source association disagrees with durable Call/request evidence",
        ));
    }
    Ok(Some(FirstHumanSource {
        conversation_id: conversation.clone(),
        message_id: MessageId::new(message_id),
        position,
        text,
        local,
        eligible: !consumed.unwrap_or(false) && association.is_none(),
    }))
}

pub(super) fn bounded_text(message: &MessageBlock) -> Option<String> {
    let MessageBlock::User(user) = message else {
        return None;
    };
    let mut text = String::new();
    for block in &user.content {
        let crate::message::UserContentBlock::Text(block) = block else {
            continue;
        };
        if block.text.is_empty() {
            continue;
        }
        let separator = usize::from(!text.is_empty());
        if text
            .len()
            .checked_add(separator)?
            .checked_add(block.text.len())?
            > 4096
        {
            return None;
        }
        if separator > 0 {
            text.push('\n');
        }
        text.push_str(&block.text);
    }
    (!text.trim().is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::durable::sqlite::test_support::{store, user_message};
    use crate::durable::{CanonicalStore, InboxStore};
    use crate::message::UserSource;

    #[test]
    fn first_human_is_indexed_exact_timestamp_optional_and_independent_of_preview() {
        let store = store();
        store.initialize(&[]).unwrap();
        let mut runtime = user_message("runtime", "opening preview");
        if let MessageBlock::User(user) = &mut runtime {
            user.source = UserSource::Runtime;
            user.timestamp = None;
        }
        store.append_canonical(&runtime).unwrap();
        assert!(store.first_human_source().unwrap().is_none());
        let mut human = user_message("first", "  exact input\nwith whitespace  ");
        if let MessageBlock::User(user) = &mut human {
            user.timestamp = None;
        }
        let receipt = store.append_canonical(&human).unwrap();
        assert_eq!(
            receipt.observation.opening_preview.as_deref(),
            Some("opening preview")
        );
        assert!(receipt.observation.human_activity_at.is_none());
        assert_eq!(receipt.observation.first_human_position, Some(2));
        let source = store.first_human_source().unwrap().unwrap();
        assert_eq!(source.message_id, MessageId::new("first"));
        assert_eq!(source.position, 2);
        assert!(source.local);
        assert!(source.eligible);
        assert_eq!(
            source.text.as_deref(),
            Some("  exact input\nwith whitespace  ")
        );
        store
            .append_canonical(&user_message("second", "later"))
            .unwrap();
        assert_eq!(store.first_human_source().unwrap().unwrap(), source);
        let connection = store.lock().unwrap();
        let mut statement = connection
            .prepare(&format!("EXPLAIN QUERY PLAN {FIRST_HUMAN_SQL}"))
            .unwrap();
        let plan = statement
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            plan.iter()
                .any(|line| line.contains("message_ledger_first_human_idx"))
        );
        assert!(!plan.iter().any(|line| line.contains("TEMP B-TREE")));
    }
    #[test]
    fn unusable_first_human_and_inherited_source_never_fall_through_to_later_text() {
        for first_text in [String::new(), " ".into(), "界".repeat(1366)] {
            let store = store();
            store.initialize(&[]).unwrap();
            store
                .append_canonical(&user_message("first", &first_text))
                .unwrap();
            store
                .append_canonical(&user_message("second", "usable"))
                .unwrap();
            let source = store.first_human_source().unwrap().unwrap();
            assert_eq!(source.message_id, MessageId::new("first"));
            assert!(source.text.is_none());
        }
        let image_store = store();
        image_store.initialize(&[]).unwrap();
        let mut image = user_message("image-only", "");
        if let MessageBlock::User(user) = &mut image {
            user.content = vec![crate::message::UserContentBlock::Image(
                crate::message::ImageReference {
                    artifact_id: crate::runtime::identity::ArtifactId::new("image-only"),
                    alt: Some("image description is not Human text".into()),
                },
            )];
        }
        image_store.append_canonical(&image).unwrap();
        image_store
            .append_canonical(&user_message("later-text", "usable later text"))
            .unwrap();
        let first = image_store.first_human_source().unwrap().unwrap();
        assert_eq!(first.message_id, MessageId::new("image-only"));
        assert!(first.text.is_none());
        let store = store();
        store
            .initialize(&[user_message("inherited", "saved history")])
            .unwrap();
        store
            .append_canonical(&user_message("local", "later"))
            .unwrap();
        let source = store.first_human_source().unwrap().unwrap();
        assert!(!source.local);
        assert_eq!(source.message_id, MessageId::new("inherited"));
    }
    #[test]
    fn missing_bootstrap_identity_cannot_reclassify_inherited_human_as_local() {
        let store = store();
        store
            .initialize(&[user_message("inherited", "saved history")])
            .unwrap();
        assert!(!store.first_human_source().unwrap().unwrap().local);
        store
            .lock()
            .unwrap()
            .execute("DELETE FROM bootstrap_identity", [])
            .unwrap();
        assert!(store.first_human_source().is_err());
    }

    #[test]
    fn pending_and_context_human_are_not_first_ordinary_boundary() {
        let store = store();
        store.initialize(&[]).unwrap();
        store
            .accept_inbound(crate::durable::sqlite::test_support::draft("pending"))
            .unwrap();
        assert!(store.first_human_source().unwrap().is_none());
        let mut context =
            crate::durable::sqlite::test_support::request_context_message("context", "context");
        if let MessageBlock::User(user) = &mut context {
            user.source = UserSource::Human;
        }
        store.append_canonical(&context).unwrap();
        assert!(store.first_human_source().unwrap().is_none());
    }
}
