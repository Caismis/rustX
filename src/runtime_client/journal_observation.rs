//! Read evidence for deferred execution diagnostics at a published native J cut.
use super::RuntimeClientCursor;
use super::checkpoint::{CheckpointPage, CheckpointValueChunk, CheckpointValueRepresentation};
use super::types::RuntimeClientError;
use crate::durable::ConversationStore;
use crate::observation::cut::{CheckpointPageItem, CheckpointReadCut};

pub(crate) fn selection(domain: &str) -> Option<(u64, Vec<String>)> {
    let tail = domain.strip_prefix("journal.")?;
    let mut parts = tail.split('.');
    let sequence = parts.next()?.parse::<u64>().ok().filter(|seq| *seq > 0)?;
    Some((sequence, parts.map(str::to_owned).collect()))
}
fn authorize(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    sequence: u64,
    path: &[String],
) -> Result<(), RuntimeClientError> {
    if sequence > cut.through
        || cut
            .source_cut
            .as_ref()
            .is_none_or(|source| source.read.journal != cut.through)
        || &cut.conversation_id != store.conversation_id()
    {
        return Err(RuntimeClientError::ReadCutExpired);
    }
    let event_kind = store
        .observation_event_kind(sequence, cut.through)
        .map_err(RuntimeClientError::observation_store_error)?
        .ok_or(RuntimeClientError::ReadCutExpired)?;
    let normalized_failure = path == ["normalized_failure"] && event_kind == "attempt_failed";
    let finish_reason = path == ["event", "finish_reason"] && event_kind == "attempt_completed";
    let tool_field = path.len() >= 2
        && path[0] == "event"
        && matches!(path[1].as_str(), "result" | "error")
        && matches!(
            event_kind.as_str(),
            "tool_execution_completed" | "tool_execution_failed"
        );
    let attempt_identity = path == ["attempt_id"]
        && matches!(
            event_kind.as_str(),
            "attempt_started"
                | "model_request_started"
                | "assistant_message_committed"
                | "attempt_completed"
                | "attempt_failed"
                | "attempt_cancelled"
                | "attempt_timed_out"
                | "attempt_limit_exceeded"
        );
    if normalized_failure || finish_reason || tool_field || attempt_identity {
        return Ok(());
    }
    let trace_field = if let Some(kind) =
        crate::durable::observation::TraceFactKind::parse(&event_kind)
    {
        let facts = store
            .observation_trace_facts(
                cut.source_cut
                    .as_ref()
                    .ok_or(RuntimeClientError::ReadCutExpired)?,
                &crate::durable::observation::TraceFactQuery {
                    scope: crate::durable::observation::TraceFactScope::All,
                    kinds: vec![kind],
                    before: sequence.checked_add(1),
                    after: sequence - 1,
                    ascending: true,
                    limit: 1,
                },
            )
            .map_err(RuntimeClientError::observation_store_error)?;
        facts.iter().any(|fact|fact.sequence==sequence && (fact.fields.iter().chain(fact.location.step_id_source.iter()).any(|field|matches!(field,crate::observation::trace::TraceFieldSource::Value{domain,item_offset:0,path:selected,..} if domain==&format!("journal.{sequence}") && selected==path))))
    } else {
        false
    };
    if !(normalized_failure || finish_reason || tool_field || attempt_identity || trace_field) {
        return Err(RuntimeClientError::InvalidRequest {
            message: "unadvertised native Journal field".into(),
        });
    }
    Ok(())
}
pub(crate) fn value_read(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    domain: &str,
    item_offset: u64,
    path: &[String],
    offset: u64,
    max_bytes: u32,
) -> Result<CheckpointValueChunk, RuntimeClientError> {
    use base64::Engine;
    let (sequence, selected) = selection(domain).ok_or(RuntimeClientError::ReadCutExpired)?;
    if item_offset != 0 || !selected.is_empty() {
        return Err(RuntimeClientError::InvalidRequest {
            message: "Journal field reader requires its advertised source path".into(),
        });
    }
    authorize(store, cut, sequence, path)?;
    let chunk = if path == ["normalized_failure"] {
        store
            .observation_failure_read_at(
                cut.source_cut
                    .as_ref()
                    .ok_or(RuntimeClientError::ReadCutExpired)?,
                sequence,
                offset,
                max_bytes,
            )
            .map_err(RuntimeClientError::observation_store_error)?
    } else {
        store
            .event_value_read(sequence, cut.through, path, offset, max_bytes)
            .map_err(RuntimeClientError::observation_store_error)?
    };
    Ok(CheckpointValueChunk {
        representation: CheckpointValueRepresentation::CanonicalJsonV1,
        cut: cut.clone(),
        domain: domain.into(),
        item_offset,
        path: path.to_vec(),
        offset,
        bytes_base64: base64::engine::general_purpose::STANDARD.encode(chunk.bytes),
        readable_end: chunk.readable_end,
        next_offset: chunk.next_offset,
    })
}
pub(crate) fn page(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    domain: &str,
    offset: u64,
    limit: usize,
) -> Result<CheckpointPage, RuntimeClientError> {
    let (sequence, mut path) = selection(domain).ok_or(RuntimeClientError::ReadCutExpired)?;
    if path != ["attempt_id"] && path.first().is_none_or(|field| field != "event") {
        path.insert(0, "event".into());
    }
    authorize(store, cut, sequence, &path)?;
    let page = store
        .event_value_page(sequence, cut.through, &path, offset, limit)
        .map_err(RuntimeClientError::observation_store_error)?;
    let items=page.fields.into_iter().enumerate().map(|(index,field)|CheckpointPageItem {offset:offset+index as u64,identity:format!("journal:{sequence}:{}",offset+index as u64),value:Some(serde_json::json!({"path":field.path,"key":field.key,"encoded_bytes":field.encoded_bytes.to_string(),"children":field.children_total.to_string(),"value":(!field.deferred).then_some(field.value)})),deferred:field.deferred,content:field.content,key:None,body_complete:None}).collect();
    Ok(CheckpointPage {
        cursor: RuntimeClientCursor::new(cut.revision),
        read_cut: cut.clone(),
        domain: domain.into(),
        total: page.total,
        items,
        next_offset: page.next_offset,
        collections: Vec::new(),
    })
}
