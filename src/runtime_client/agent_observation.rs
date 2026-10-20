//! Per-request routing to a Child's sole native Projection. No Parent snapshot
//! mirror, observation execution state, event repair by snapshot, or retry budget.
use super::{
    checkpoint::RuntimeClientCheckpoint,
    host::ClientInner,
    types::{
        RUNTIME_CLIENT_PROTOCOL_VERSION, RequestId, RuntimeClientCursor, RuntimeClientError,
        RuntimeClientProtocolEvent, RuntimeClientRequest, RuntimeClientResponse,
        RuntimeClientResult,
    },
};
use crate::durable::ConversationStore;
use crate::runtime::identity::{AgentId, ConversationId, SubagentId};
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AgentObservationCursor {
    pub agent_id: AgentId,
    pub conversation_id: ConversationId,
    pub activation_id: SubagentId,
    pub cursor: RuntimeClientCursor,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationSource {
    Live,
    DurableOnly,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AgentObservation {
    Checkpoint {
        agent_id: AgentId,
        conversation_id: ConversationId,
        activation_id: SubagentId,
        source: ObservationSource,
        cursor: Option<AgentObservationCursor>,
        checkpoint: Box<RuntimeClientCheckpoint>,
    },
    Events {
        after: AgentObservationCursor,
        cursor: AgentObservationCursor,
        events: Vec<RuntimeClientProtocolEvent>,
    },
    ResyncRequired {
        agent_id: AgentId,
        conversation_id: ConversationId,
        activation_id: SubagentId,
        source: ObservationSource,
        reason: String,
    },
}
fn failed(error: impl std::fmt::Display) -> RuntimeClientError {
    let error = error.to_string();
    let mut end = error.len().min(512);
    while !error.is_char_boundary(end) {
        end -= 1;
    }
    RuntimeClientError::ObservationUnavailable {
        message: format!("child observation: {}", &error[..end]),
    }
}
struct Peer {
    stream: BufReader<UnixStream>,
    pending_record: Vec<u8>,
    id: u64,
    conversation_id: ConversationId,
    activation_id: SubagentId,
    // Declared last: the Unix stream closes before capacity becomes reusable.
    _permit: tokio::sync::OwnedSemaphorePermit,
}
impl Peer {
    async fn open(
        path: std::path::PathBuf,
        conversation_id: &ConversationId,
        expected_activation: &SubagentId,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Result<Self, RuntimeClientError> {
        let mut peer = Self {
            stream: BufReader::new(UnixStream::connect(path).await.map_err(failed)?),
            pending_record: Vec::new(),
            id: 0,
            conversation_id: conversation_id.clone(),
            activation_id: expected_activation.clone(),
            _permit: permit,
        };
        let id = peer.next_id();
        match peer
            .request(RuntimeClientRequest::Initialize {
                id,
                protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION,
            })
            .await?
        {
            RuntimeClientResult::Initialized {
                conversation_id: actual,
                activation_id: Some(activation),
                read_only: true,
                protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION,
                ..
            } if actual == *conversation_id && activation == *expected_activation => Ok(peer),
            _ => Err(failed(
                "unexpected initialization identity or capability receipt",
            )),
        }
    }
    async fn record(&mut self) -> Result<serde_json::Value, RuntimeClientError> {
        loop {
            let bytes = self.stream.fill_buf().await.map_err(failed)?;
            if bytes.is_empty() {
                return Err(failed("observation endpoint closed"));
            }
            let end = bytes.iter().position(|byte| *byte == b'\n');
            let count = end.map_or(bytes.len(), |position| position + 1);
            if self.pending_record.len() + count
                > super::transport::stdio::STDIO_JSONL_MAX_RECORD_BYTES + 1
            {
                return Err(failed("observation record exceeds native bound"));
            }
            self.pending_record.extend_from_slice(&bytes[..count]);
            self.stream.consume(count);
            if end.is_some() {
                let result = serde_json::from_slice(&self.pending_record).map_err(failed);
                self.pending_record.clear();
                return result;
            }
        }
    }
    async fn request(
        &mut self,
        request: RuntimeClientRequest,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let expected = request.id();
        let mut bytes = super::checkpoint::bounded_bytes(
            &request,
            super::transport::stdio::STDIO_JSONL_MAX_RECORD_BYTES,
        )
        .ok_or_else(|| failed("request exceeds native bound"))?;
        bytes.push(b'\n');
        self.stream
            .get_mut()
            .write_all(&bytes)
            .await
            .map_err(failed)?;
        let record = self.record().await?;
        // This protocol uses correlated finite EventBatch requests, so it does
        // not install a push subscription. An unsolicited event is a protocol
        // error, never ignored on the premise of later full-snapshot repair.
        if record.get("id").is_none() {
            return Err(failed(
                "unexpected unsolicited native observation; continuity was not discarded",
            ));
        }
        let response: RuntimeClientResponse = serde_json::from_value(record).map_err(failed)?;
        if response.id != expected {
            return Err(failed("unexpected response identity"));
        }
        response
            .result
            .ok_or_else(|| response.error.unwrap_or_else(|| failed("missing result")))
    }
    fn next_id(&mut self) -> RequestId {
        self.id += 1;
        RequestId::new(self.id)
    }
    async fn trace_page(
        &mut self,
        before: Option<super::trace::TraceCursor>,
        limit: usize,
        records: Vec<super::trace::TraceCursor>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let id = self.next_id();
        let result = self
            .request(RuntimeClientRequest::TracePageGet {
                id,
                before,
                limit,
                records,
            })
            .await?;
        let RuntimeClientResult::TracePage { page } = &result else {
            return Err(failed("unexpected native Trace page response"));
        };
        let cut = page
            .read_cut
            .as_ref()
            .ok_or_else(|| failed("native Trace page omitted its read evidence"))?;
        self.validate_cut(cut)?;
        if page.records.len() > 32
            || page.updates.len() > super::trace::TRACE_RECORD_LIMIT
            || super::checkpoint::bounded_bytes(&result, super::checkpoint::CHECKPOINT_TARGET_BYTES)
                .is_none()
        {
            return Err(RuntimeClientError::ObservationCapacityExceeded);
        }
        Ok(result)
    }
    async fn trace_locate_tool(
        &mut self,
        locator: &super::trace::TraceToolLocator,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let id = self.next_id();
        let result = self
            .request(RuntimeClientRequest::TraceLocateTool {
                id,
                locator: locator.clone(),
            })
            .await?;
        let RuntimeClientResult::TraceToolLocation { location } = &result else {
            return Err(failed("unexpected native Tool locator response"));
        };
        if let Some(location) = location {
            self.validate_cut(
                location
                    .page
                    .read_cut
                    .as_ref()
                    .ok_or_else(|| failed("native Tool location omitted its read evidence"))?,
            )?;
        }
        if location.as_ref().is_some_and(|location| {
            location.record_id.len() > 256
                || location.page.records.len() > 32
                || location.page.updates.len() > 32
        }) || super::checkpoint::bounded_bytes(
            &result,
            super::checkpoint::CHECKPOINT_TARGET_BYTES,
        )
        .is_none()
        {
            return Err(failed(
                "native Tool locator result exceeded bounded source contract",
            ));
        }
        Ok(result)
    }
    fn validate_cut(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
    ) -> Result<(), RuntimeClientError> {
        if cut.conversation_id != self.conversation_id
            || cut.activation_id.as_ref() != Some(&self.activation_id)
        {
            return Err(failed(
                "native observation read cut changed conversation or activation",
            ));
        }
        Ok(())
    }
    async fn checkpoint(&mut self) -> Result<RuntimeClientCheckpoint, RuntimeClientError> {
        let id = self.next_id();
        match self
            .request(RuntimeClientRequest::CheckpointGet { id })
            .await?
        {
            RuntimeClientResult::Checkpoint { checkpoint } => {
                self.validate_cut(&checkpoint.read_cut)?;
                if checkpoint.snapshot.conversation_id != self.conversation_id {
                    return Err(failed("checkpoint changed native conversation identity"));
                }
                Ok(*checkpoint)
            }
            _ => Err(failed("unexpected checkpoint response")),
        }
    }
    async fn interaction_fields(
        &mut self,
        interaction: &crate::runtime::interaction::InteractionRef,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let id = self.next_id();
        let result = self
            .request(RuntimeClientRequest::InteractionPageGet {
                id,
                interaction: interaction.clone(),
                path: path.to_vec(),
                offset,
                limit,
            })
            .await?;
        let RuntimeClientResult::InteractionFields {
            interaction: actual,
            path: actual_path,
            total,
            fields,
            next_offset,
        } = &result
        else {
            return Err(failed("unexpected pending interaction fields response"));
        };
        let end = offset
            .checked_add(fields.len() as u64)
            .ok_or_else(|| failed("pending page overflow"))?;
        if actual != interaction
            || actual_path != path
            || fields.len() > limit
            || fields.len() > 32
            || *total > super::content::CONTENT_SAFE_INTEGER_MAX
            || end > *total
            || *next_offset != (end < *total).then_some(end)
            || (offset < *total && fields.is_empty())
            || fields.iter().enumerate().any(|(index, field)| {
                field.offset != offset + index as u64
                    || field.key.as_ref().is_some_and(|key| key.len() > 256)
                    || field.encoded_bytes > super::content::CONTENT_SAFE_INTEGER_MAX
                    || field.children > super::content::CONTENT_SAFE_INTEGER_MAX
                    || field
                        .key_bytes
                        .is_some_and(|bytes| bytes > super::content::CONTENT_SAFE_INTEGER_MAX)
                    || field.value.as_ref().is_some_and(|value| {
                        field.children > 32
                            || super::checkpoint::bounded_item_bytes(
                                value,
                                super::checkpoint::CHECKPOINT_ITEM_BYTES,
                            )
                            .is_none()
                    })
            })
        {
            return Err(failed(
                "pending interaction fields do not match admitted source bounds",
            ));
        }
        Ok(result)
    }
    async fn interaction_value_read(
        &mut self,
        interaction: &crate::runtime::interaction::InteractionRef,
        path: &[String],
        offset: u64,
        max_bytes: u32,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        use base64::Engine as _;
        let id = self.next_id();
        let result = self
            .request(RuntimeClientRequest::InteractionValueRead {
                id,
                interaction: interaction.clone(),
                path: path.to_vec(),
                offset,
                max_bytes,
            })
            .await?;
        let RuntimeClientResult::InteractionValueChunk {
            interaction: actual,
            path: actual_path,
            offset: actual_offset,
            bytes_base64,
            readable_end,
            next_offset,
        } = &result
        else {
            return Err(failed("unexpected pending interaction value response"));
        };
        if actual != interaction
            || actual_path != path
            || *actual_offset != offset
            || bytes_base64.len() > 87_384
            || bytes_base64.len() % 4 != 0
            || bytes_base64.len() / 4 * 3
                - if bytes_base64.ends_with("==") {
                    2
                } else {
                    usize::from(bytes_base64.ends_with('='))
                }
                > max_bytes as usize
            || *readable_end > super::content::CONTENT_SAFE_INTEGER_MAX
        {
            return Err(failed("pending interaction value source identity changed"));
        }
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(bytes_base64)
            .map_err(failed)?;
        let end = offset
            .checked_add(bytes.len() as u64)
            .ok_or_else(|| failed("pending range overflow"))?;
        if bytes.len() > max_bytes as usize
            || bytes.len() > 65_536
            || end > *readable_end
            || (offset < *readable_end && bytes.is_empty())
            || *next_offset != (end < *readable_end).then_some(end)
        {
            return Err(failed(
                "pending interaction value did not advance exact bounded range",
            ));
        }
        Ok(result)
    }
    async fn events(
        &mut self,
        after_cursor: RuntimeClientCursor,
    ) -> Result<(Vec<RuntimeClientProtocolEvent>, RuntimeClientCursor), RuntimeClientError> {
        let id = self.next_id();
        match self
            .request(RuntimeClientRequest::ObserveEvents { id, after_cursor })
            .await?
        {
            RuntimeClientResult::EventBatch {
                after_cursor: actual,
                cursor,
                events,
            } if actual == after_cursor => {
                let mut consumed = after_cursor;
                for event in &events {
                    if event.cursor.get()
                        != consumed
                            .get()
                            .checked_add(1)
                            .ok_or(RuntimeClientError::ProjectionExhausted)?
                    {
                        return Err(failed("noncontiguous native event batch"));
                    }
                    match &event.event {
                        super::event::RuntimeClientEvent::ProjectionTransition {
                            metadata, ..
                        } => self.validate_cut(&metadata.read_cut)?,
                        super::event::RuntimeClientEvent::CheckpointAdvanced {
                            checkpoint, ..
                        } => self.validate_cut(&checkpoint.read_cut)?,
                        _ => return Err(failed("native event lacks complete read-cut evidence")),
                    }
                    consumed = event.cursor;
                }
                if cursor != consumed {
                    return Err(failed(
                        "batch cursor does not cover exactly its complete events",
                    ));
                }
                Ok((events, cursor))
            }
            _ => Err(failed("unexpected events response")),
        }
    }
}
async fn live_observation(
    id: &AgentId,
    conversation_id: &ConversationId,
    activation_id: &SubagentId,
    path: std::path::PathBuf,
    after: Option<AgentObservationCursor>,
    permit: tokio::sync::OwnedSemaphorePermit,
) -> Result<AgentObservation, RuntimeClientError> {
    let mut peer = Peer::open(path, conversation_id, activation_id, permit).await?;
    Ok(if let Some(after) = after {
        match peer.events(after.cursor).await {
            Ok((events, cursor)) => AgentObservation::Events {
                cursor: AgentObservationCursor {
                    cursor,
                    ..after.clone()
                },
                after,
                events,
            },
            Err(RuntimeClientError::ResyncRequired { .. }) => {
                return Ok(AgentObservation::ResyncRequired {
                    agent_id: id.clone(),
                    conversation_id: conversation_id.clone(),
                    activation_id: activation_id.clone(),
                    source: ObservationSource::Live,
                    reason: "native replay cursor expired".into(),
                });
            }
            Err(error) => return Err(error),
        }
    } else {
        let checkpoint = peer.checkpoint().await?;
        AgentObservation::Checkpoint {
            agent_id: id.clone(),
            conversation_id: conversation_id.clone(),
            activation_id: activation_id.clone(),
            source: ObservationSource::Live,
            cursor: Some(AgentObservationCursor {
                agent_id: id.clone(),
                conversation_id: conversation_id.clone(),
                activation_id: activation_id.clone(),
                cursor: checkpoint.cursor,
            }),
            checkpoint: Box::new(checkpoint),
        }
    })
}

fn durable_host(
    registry: &crate::runtime::subagent::SubagentRegistry,
    activation: &SubagentId,
) -> Result<super::host::RuntimeClientHost, RuntimeClientError> {
    let store = registry
        .transcript_store(activation)
        .map_err(|error| failed(format!("{error:?}")))?;
    super::host::RuntimeClientHost::new_durable(std::sync::Arc::new(store), None).map_err(failed)
}

// Cold Trace owns an immutable historical cut, authenticated by the Registry
// activation. It does not carry a durable Host's publication/cache cut.
fn cold_trace_selection(cut: &super::checkpoint::CheckpointReadCut, domain: &str) -> bool {
    cut.revision == 0
        && cut.canonical_foreground.is_none()
        && cut.live_publication.is_none()
        && cut.domain_revisions.is_empty()
        && cut.collection_cuts.is_empty()
        && (super::trace_observation::selection(domain).is_some()
            || super::trace_observation::messages_selection(domain).is_some()
            || super::trace_observation::canonical_selection(domain)
            || super::trace_observation::context_selection(domain).is_some()
            || super::journal_observation::selection(domain).is_some()
            || super::delivery::selection(domain).is_some()
            || super::occupancy::selection(domain).is_some()
            || super::attempt_identity::selection(domain).is_some()
            || super::projection_history::selection(domain).is_some())
}

fn cold_agent_value_read(
    registry: &crate::runtime::subagent::SubagentRegistry,
    activation: &SubagentId,
    cut: &super::checkpoint::CheckpointReadCut,
    domain: &str,
    item_offset: u64,
    path: &[String],
    range: (u64, u32),
) -> Result<super::checkpoint::CheckpointValueChunk, RuntimeClientError> {
    if cold_trace_selection(cut, domain) {
        let store = registry
            .transcript_store(activation)
            .map_err(|_| RuntimeClientError::ContentSourceUnavailable)?;
        return super::trace_observation::history_value_read(
            &store,
            cut,
            domain,
            item_offset,
            path,
            range,
        );
    }
    let host = durable_host(registry, activation)?;
    host.inner
        .checkpoint_value_read(cut, domain, item_offset, path, range.0, range.1)
}

fn cold_agent_page(
    registry: &crate::runtime::subagent::SubagentRegistry,
    activation: &SubagentId,
    cut: &super::checkpoint::CheckpointReadCut,
    domain: &str,
    offset: u64,
    limit: usize,
) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
    if cold_trace_selection(cut, domain) {
        let store = registry
            .transcript_store(activation)
            .map_err(|_| RuntimeClientError::ContentSourceUnavailable)?;
        return super::trace_observation::history_page(&store, cut, domain, offset, limit);
    }
    let host = durable_host(registry, activation)?;
    host.inner.checkpoint_page(cut, domain, offset, limit)
}

fn bind_cold_agent_trace(
    page: &mut super::trace::TracePage,
    agent: &super::snapshot::RuntimeClientAgent,
) -> Result<(), RuntimeClientError> {
    let cut = page
        .read_cut
        .as_mut()
        .ok_or(RuntimeClientError::ReadCutExpired)?;
    if cut.conversation_id != agent.child_conversation_id || cut.activation_id.is_some() {
        return Err(RuntimeClientError::ReadCutExpired);
    }
    cut.activation_id = Some(agent.activation_id.clone());
    super::trace::ensure_page_bound(page).map_err(RuntimeClientError::observation_store_error)
}

async fn physical_read<T: Send + 'static>(
    permit: tokio::sync::OwnedSemaphorePermit,
    read: impl FnOnce() -> Result<T, RuntimeClientError> + Send + 'static,
) -> Result<T, RuntimeClientError> {
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        read()
    })
    .await
    .map_err(failed)?
}

fn pending_fields_page(
    cut: &super::checkpoint::CheckpointReadCut,
    domain: &str,
    result: RuntimeClientResult,
) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
    let RuntimeClientResult::InteractionFields {
        total,
        fields,
        next_offset,
        ..
    } = result
    else {
        return Err(failed("unexpected pending fields carrier"));
    };
    let revision = *cut
        .domain_revisions
        .get("pending_interactions")
        .ok_or(RuntimeClientError::ReadCutExpired)?;
    let mut collections = Vec::new();
    let items = fields
        .into_iter()
        .map(|field| {
            if field.children > 0 {
                collections.push(super::checkpoint::CollectionFrontier {
                    domain: format!("{domain}.{}", field.offset),
                    revision,
                    total: field.children,
                    loaded: 0,
                    next_offset: Some(0),
                });
            }
            super::checkpoint::CheckpointPageItem {
                offset: field.offset,
                identity: field.offset.to_string(),
                key: field
                    .key
                    .zip(field.key_bytes)
                    .map(
                        |(preview, encoded_bytes)| super::checkpoint::CheckpointKey {
                            path: vec!["$key".into()],
                            preview,
                            encoded_bytes,
                        },
                    ),
                value: field.value,
                deferred: field.deferred,
                content: Vec::new(),
                body_complete: None,
            }
        })
        .collect();
    Ok(super::checkpoint::CheckpointPage {
        read_cut: cut.clone(),
        cursor: RuntimeClientCursor::new(cut.revision),
        domain: domain.into(),
        total,
        items,
        next_offset,
        collections,
    })
}

async fn peer_pending_bytes(
    peer: &mut Peer,
    interaction: &crate::runtime::interaction::InteractionRef,
    path: &[String],
    range: (u64, u32),
) -> Result<(Vec<u8>, u64, Option<u64>), RuntimeClientError> {
    use base64::Engine as _;
    let RuntimeClientResult::InteractionValueChunk {
        bytes_base64,
        readable_end,
        next_offset,
        ..
    } = peer
        .interaction_value_read(interaction, path, range.0, range.1)
        .await?
    else {
        return Err(failed("unexpected pending bytes carrier"));
    };
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(bytes_base64)
        .map_err(failed)?;
    Ok((bytes, readable_end, next_offset))
}

async fn peer_routed_body(
    peer: &mut Peer,
    header: &super::snapshot::RuntimeClientInteractionHeader,
    range: (u64, u32),
) -> Result<(Vec<u8>, u64, Option<u64>), RuntimeClientError> {
    let (_, source_end, _) = peer_pending_bytes(peer, &header.interaction, &[], (0, 1)).await?;
    let plan = super::pending_observation::routed_range(header, source_end, range.0, range.1)?;
    let (bytes, actual_end, _) = peer_pending_bytes(
        peer,
        &header.interaction,
        &[],
        (plan.native_offset, plan.native_max_bytes),
    )
    .await?;
    if source_end != actual_end {
        return Err(RuntimeClientError::ReadCutExpired);
    }
    super::pending_observation::compose_routed(&plan, &bytes)
}

fn validate_pending_bounds(
    domain: &str,
    path: &[String],
    offset: u64,
) -> Result<(), RuntimeClientError> {
    if domain.len() > 256
        || path.len() > 32
        || path.iter().any(|part| part.len() > 256)
        || offset > super::content::CONTENT_SAFE_INTEGER_MAX
    {
        return Err(RuntimeClientError::InvalidRequest {
            message: "invalid pending source read bounds".into(),
        });
    }
    Ok(())
}

impl ClientInner {
    pub(crate) async fn agent_trace_via_inspection(
        self: &std::sync::Arc<Self>,
        id: &AgentId,
        before: Option<super::trace::TraceCursor>,
        limit: usize,
        records: Vec<super::trace::TraceCursor>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        if !(1..=super::trace::TRACE_PAGE_LIMIT).contains(&limit)
            || records.len() > super::trace::TRACE_RECORD_LIMIT
        {
            return Err(RuntimeClientError::InvalidRequest {
                message: "invalid finite Trace page or refresh limit".into(),
            });
        }
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        tokio::time::timeout_at(deadline, async {
            let permit = self.reserve_inspection()?;
            let agent = self.child_view(id)?;
            let result = if matches!(
                agent.state,
                crate::runtime::subagent::AgentState::Inactive
                    | crate::runtime::subagent::AgentState::Unavailable
            ) {
                let registry = self.agent_registry()?.clone();
                let activation = agent.activation_id.clone();
                let reading_agent = agent.clone();
                physical_read(permit, move || {
                    let store = registry
                        .transcript_store(&activation)
                        .map_err(|_| RuntimeClientError::ContentSourceUnavailable)?;
                    let projection = super::trace::TraceProjection::new(&store)
                        .map_err(RuntimeClientError::observation_store_error)?;
                    let mut page = projection
                        .page(before.as_ref(), limit)
                        .map_err(RuntimeClientError::observation_store_error)?;
                    super::trace::repair_agent_records(&mut page.records, &reading_agent);
                    page.updates = projection
                        .refresh_agent(&records, &reading_agent)
                        .map_err(RuntimeClientError::observation_store_error)?;
                    bind_cold_agent_trace(&mut page, &reading_agent)?;
                    Ok(RuntimeClientResult::TracePage { page })
                })
                .await?
            } else {
                let path = self
                    .agent_registry()?
                    .agent_inspection_path(id)
                    .ok_or(RuntimeClientError::ContentSourceUnavailable)?;
                let mut peer = Peer::open(
                    path,
                    &agent.child_conversation_id,
                    &agent.activation_id,
                    permit,
                )
                .await?;
                peer.trace_page(before, limit, records).await?
            };
            let current = self.child_view(id)?;
            if current.activation_id != agent.activation_id
                || current.child_conversation_id != agent.child_conversation_id
            {
                return Err(RuntimeClientError::ReadCutExpired);
            }
            Ok(result)
        })
        .await
        .map_err(|_| failed("total Child Trace page deadline exhausted"))?
    }

    pub(crate) async fn agent_trace_locate_via_inspection(
        self: &std::sync::Arc<Self>,
        id: &AgentId,
        locator: super::trace::TraceToolLocator,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        tokio::time::timeout_at(deadline, async {
            let permit = self.reserve_inspection()?;
            let agent = self.child_view(id)?;
            let result = if matches!(
                agent.state,
                crate::runtime::subagent::AgentState::Inactive
                    | crate::runtime::subagent::AgentState::Unavailable
            ) {
                let registry = self.agent_registry()?.clone();
                let activation = agent.activation_id.clone();
                let reading_agent = agent.clone();
                physical_read(permit, move || {
                    let store = registry
                        .transcript_store(&activation)
                        .map_err(|error| failed(format!("{error:?}")))?;
                    let projection = super::trace::TraceProjection::new(&store).map_err(failed)?;
                    let mut location = projection.locate_tool(&locator).map_err(failed)?;
                    if let Some(location) = &mut location {
                        bind_cold_agent_trace(&mut location.page, &reading_agent)?;
                    }
                    Ok(RuntimeClientResult::TraceToolLocation { location })
                })
                .await?
            } else {
                let path = self
                    .agent_registry()?
                    .agent_inspection_path(id)
                    .ok_or(RuntimeClientError::ContentSourceUnavailable)?;
                let mut peer = Peer::open(
                    path,
                    &agent.child_conversation_id,
                    &agent.activation_id,
                    permit,
                )
                .await?;
                peer.trace_locate_tool(&locator).await?
            };
            let current = self.child_view(id)?;
            if current.activation_id != agent.activation_id
                || current.child_conversation_id != agent.child_conversation_id
            {
                return Err(RuntimeClientError::ReadCutExpired);
            }
            Ok(result)
        })
        .await
        .map_err(|_| failed("total Child Tool locator deadline exhausted"))?
    }

    pub(crate) async fn interaction_fields_async(
        self: &std::sync::Arc<Self>,
        interaction: &crate::runtime::interaction::InteractionRef,
        path: &[String],
        offset: u64,
        limit: usize,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        if interaction.conversation_id == self.conversation_id {
            return self.interaction_fields(interaction, path, offset, limit);
        }
        let source = self
            .agent_registry()?
            .interaction_inspection_source(interaction)
            .ok_or(RuntimeClientError::ContentSourceUnavailable)?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        tokio::time::timeout_at(deadline, async {
            let permit = self.reserve_inspection()?;
            let mut peer = Peer::open(source.0.clone(), &source.1, &source.2, permit).await?;
            let result = peer
                .interaction_fields(interaction, path, offset, limit)
                .await?;
            match self
                .agent_registry()?
                .interaction_inspection_source(interaction)
            {
                None => return Err(RuntimeClientError::ContentSourceUnavailable),
                Some(current) if current != source => {
                    return Err(RuntimeClientError::ReadCutExpired);
                }
                Some(_) => {}
            }
            Ok(result)
        })
        .await
        .map_err(|_| failed("total nested interaction page deadline exhausted"))?
    }

    pub(crate) async fn interaction_value_read_async(
        self: &std::sync::Arc<Self>,
        interaction: &crate::runtime::interaction::InteractionRef,
        path: &[String],
        range: (u64, u32),
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        if interaction.conversation_id == self.conversation_id {
            return self.interaction_value_read(interaction, path, range.0, range.1);
        }
        let source = self
            .agent_registry()?
            .interaction_inspection_source(interaction)
            .ok_or(RuntimeClientError::ContentSourceUnavailable)?;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        tokio::time::timeout_at(deadline, async {
            let permit = self.reserve_inspection()?;
            let mut peer = Peer::open(source.0.clone(), &source.1, &source.2, permit).await?;
            let result = peer
                .interaction_value_read(interaction, path, range.0, range.1)
                .await?;
            match self
                .agent_registry()?
                .interaction_inspection_source(interaction)
            {
                None => return Err(RuntimeClientError::ContentSourceUnavailable),
                Some(current) if current != source => {
                    return Err(RuntimeClientError::ReadCutExpired);
                }
                Some(_) => {}
            }
            Ok(result)
        })
        .await
        .map_err(|_| failed("total nested interaction value deadline exhausted"))?
    }

    async fn pending_peer(
        &self,
        header: &super::snapshot::RuntimeClientInteractionHeader,
    ) -> Result<Peer, RuntimeClientError> {
        let crate::runtime::interaction::InteractionSource::Subagent {
            child_conversation_id,
            ..
        } = &header.source
        else {
            return Err(RuntimeClientError::ReadCutExpired);
        };
        if *child_conversation_id != header.interaction.conversation_id {
            return Err(RuntimeClientError::ReadCutExpired);
        }
        let (endpoint, conversation, activation) = self
            .agent_registry()?
            .interaction_inspection_source(&header.interaction)
            .ok_or(RuntimeClientError::ContentSourceUnavailable)?;
        let permit = self.reserve_inspection()?;
        Peer::open(endpoint, &conversation, &activation, permit).await
    }

    fn validate_pending_header(
        &self,
        cut: &super::checkpoint::CheckpointReadCut,
        ordinal: u64,
        expected: &super::snapshot::RuntimeClientInteractionHeader,
    ) -> Result<(), RuntimeClientError> {
        let current = self.pending_read_header(cut, ordinal)?;
        if current != *expected {
            return Err(RuntimeClientError::ReadCutExpired);
        }
        if let crate::runtime::interaction::InteractionSource::Subagent { .. } = &current.source
            && self
                .agent_registry()?
                .interaction_inspection_source(&current.interaction)
                .is_none()
        {
            return Err(RuntimeClientError::ContentSourceUnavailable);
        }
        Ok(())
    }

    pub(crate) async fn checkpoint_page_async(
        self: &std::sync::Arc<Self>,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
        validate_pending_bounds(domain, &[], offset)?;
        if limit == 0 || limit > 32 {
            return Err(RuntimeClientError::InvalidRequest {
                message: "invalid pending page item bound".into(),
            });
        }
        if let Some((ordinal, selected)) = super::interaction_observation::selection(domain) {
            let header = self.pending_read_header(cut, ordinal)?;
            if matches!(
                header.source,
                crate::runtime::interaction::InteractionSource::Subagent { .. }
            ) {
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
                return tokio::time::timeout_at(deadline, async {
                    let mut peer = self.pending_peer(&header).await?;
                    let mut path = vec!["kind".to_owned(), "arguments".to_owned()];
                    path.extend(selected.into_iter().map(|part| format!("@{part}")));
                    let fields = peer
                        .interaction_fields(&header.interaction, &path, offset, limit)
                        .await?;
                    self.validate_pending_header(cut, ordinal, &header)?;
                    pending_fields_page(cut, domain, fields)
                })
                .await
                .map_err(|_| failed("total pending interaction page deadline exhausted"))?;
            }
        }
        let inner = self.clone();
        let cut = cut.clone();
        let domain = domain.to_owned();
        tokio::task::spawn_blocking(move || inner.checkpoint_page(&cut, &domain, offset, limit))
            .await
            .map_err(failed)?
    }

    pub(crate) async fn checkpoint_value_read_async(
        self: &std::sync::Arc<Self>,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        item_offset: u64,
        path: &[String],
        range: (u64, u32),
    ) -> Result<super::checkpoint::CheckpointValueChunk, RuntimeClientError> {
        validate_pending_bounds(domain, path, range.0)?;
        if item_offset > super::content::CONTENT_SAFE_INTEGER_MAX
            || range.1 == 0
            || range.1 > 65_536
            || range
                .0
                .checked_add(u64::from(range.1))
                .is_none_or(|end| end > super::content::CONTENT_SAFE_INTEGER_MAX)
        {
            return Err(RuntimeClientError::InvalidRequest {
                message: "invalid pending byte range".into(),
            });
        }
        let selected = super::interaction_observation::selection(domain);
        if domain == "pending_interactions" || selected.is_some() {
            let ordinal = selected
                .as_ref()
                .map_or(item_offset, |(ordinal, _)| *ordinal);
            let header = self.pending_read_header(cut, ordinal)?;
            if matches!(
                header.source,
                crate::runtime::interaction::InteractionSource::Subagent { .. }
            ) {
                let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
                return tokio::time::timeout_at(deadline, async {
                    let mut peer = self.pending_peer(&header).await?;
                    let (bytes, end, next) = if domain == "pending_interactions" && path.is_empty()
                    {
                        peer_routed_body(&mut peer, &header, range).await?
                    } else {
                        let mut native_path = Vec::new();
                        if let Some((_, selected)) = selected {
                            native_path.extend(["kind".to_owned(), "arguments".to_owned()]);
                            native_path.extend(selected.into_iter().map(|part| format!("@{part}")));
                            native_path.push(format!("@{item_offset}"));
                        }
                        native_path.extend_from_slice(
                            path.strip_prefix(&["request".to_owned()]).unwrap_or(path),
                        );
                        peer_pending_bytes(&mut peer, &header.interaction, &native_path, range)
                            .await?
                    };
                    self.validate_pending_header(cut, ordinal, &header)?;
                    Ok(super::pending_observation::checkpoint_chunk(
                        cut,
                        domain,
                        item_offset,
                        path,
                        range.0,
                        (bytes, end, next),
                    ))
                })
                .await
                .map_err(|_| failed("total pending interaction value deadline exhausted"))?;
            }
        }
        let inner = self.clone();
        let cut = cut.clone();
        let domain = domain.to_owned();
        let path = path.to_vec();
        tokio::task::spawn_blocking(move || {
            inner.checkpoint_value_read(&cut, &domain, item_offset, &path, range.0, range.1)
        })
        .await
        .map_err(failed)?
    }

    fn reserve_inspection(&self) -> Result<tokio::sync::OwnedSemaphorePermit, RuntimeClientError> {
        self.inspection_capacity
            .clone()
            .try_acquire_owned()
            .map_err(|_| RuntimeClientError::ObservationCapacityExceeded)
    }
    async fn durable_agent_checkpoint(
        &self,
        agent: &super::snapshot::RuntimeClientAgent,
        permit: tokio::sync::OwnedSemaphorePermit,
    ) -> Result<RuntimeClientCheckpoint, RuntimeClientError> {
        let registry = self.agent_registry()?.clone();
        let captured = agent.clone();
        let checkpoint = physical_read(permit, move || {
            let host = durable_host(&registry, &captured.activation_id)?;
            let mut checkpoint = host.inner.checkpoint()?;
            if checkpoint.snapshot.conversation_id != captured.child_conversation_id
                || checkpoint.read_cut.conversation_id != captured.child_conversation_id
                || checkpoint.read_cut.activation_id.is_some()
            {
                return Err(RuntimeClientError::ReadCutExpired);
            }
            // Retained Trace keeps its own native source/revision/Journal cut.
            // Bind only the Registry's authenticated Activation, as for the
            // standalone cold Trace path. Empty unobserved heads inherit C.
            let trace = &mut checkpoint.snapshot.trace;
            if trace.read_cut.is_some() || !trace.records.is_empty() || !trace.updates.is_empty() {
                bind_cold_agent_trace(trace, &captured)?;
            }
            checkpoint.read_cut.activation_id = Some(captured.activation_id);
            Ok(checkpoint)
        })
        .await?;
        let current = self.child_view(&agent.agent_id)?;
        if current.activation_id != agent.activation_id
            || current.child_conversation_id != checkpoint.snapshot.conversation_id
        {
            return Err(RuntimeClientError::ReadCutExpired);
        }
        Ok(checkpoint)
    }
    pub(crate) async fn agent_content_read(
        &self,
        id: &AgentId,
        reference: &super::content::ContentRef,
        offset: u64,
        max_bytes: u32,
    ) -> Result<super::content::ContentChunk, RuntimeClientError> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        tokio::time::timeout_at(deadline, async {
            let permit = self.reserve_inspection()?;
            let agent = self.child_view(id)?;
            if reference.conversation_id != agent.child_conversation_id {
                return Err(RuntimeClientError::ContentSourceUnavailable);
            }
            if matches!(
                agent.state,
                crate::runtime::subagent::AgentState::Inactive
                    | crate::runtime::subagent::AgentState::Unavailable
            ) {
                if matches!(
                    reference.source,
                    super::content::ContentSource::LivePublicationPrefix { .. }
                ) {
                    return Err(RuntimeClientError::ContentSourceUnavailable);
                }
                let registry = self.agent_registry()?.clone();
                let activation = agent.activation_id.clone();
                let reference = reference.clone();
                let chunk = physical_read(permit, move || {
                    let store = registry
                        .transcript_store(&activation)
                        .map_err(|error| failed(format!("{error:?}")))?;
                    store
                        .content_read(&reference, offset, max_bytes)
                        .map_err(RuntimeClientError::content_read_error)
                })
                .await?;
                if self.child_view(id)?.activation_id != agent.activation_id {
                    return Err(RuntimeClientError::ReadCutExpired);
                }
                return Ok(chunk);
            }
            let path = self
                .agent_registry()?
                .agent_inspection_path(id)
                .ok_or(RuntimeClientError::ContentSourceUnavailable)?;
            let mut peer = Peer::open(
                path,
                &agent.child_conversation_id,
                &agent.activation_id,
                permit,
            )
            .await?;
            let request_id = peer.next_id();
            let result = peer
                .request(RuntimeClientRequest::ContentRead {
                    id: request_id,
                    content_ref: reference.clone(),
                    offset,
                    max_bytes,
                })
                .await?;
            if self.child_view(id)?.activation_id != agent.activation_id {
                return Err(RuntimeClientError::ReadCutExpired);
            }
            match result {
                RuntimeClientResult::ContentChunk { chunk } => Ok(chunk),
                _ => Err(failed("unexpected content response")),
            }
        })
        .await
        .map_err(|_| failed("total content read deadline exhausted"))?
    }
    pub(crate) async fn agent_checkpoint_value_read(
        &self,
        id: &AgentId,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        item_offset: u64,
        path: &[String],
        range: (u64, u32),
    ) -> Result<super::checkpoint::CheckpointValueChunk, RuntimeClientError> {
        let (offset, max_bytes) = range;
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        tokio::time::timeout_at(deadline, async {
            let permit = self.reserve_inspection()?;
            let agent = self.child_view(id)?;
            if cut.conversation_id != agent.child_conversation_id
                || cut.activation_id.as_ref() != Some(&agent.activation_id)
            {
                return Err(RuntimeClientError::ReadCutExpired);
            }
            let mut native_cut = cut.clone();
            if matches!(
                agent.state,
                crate::runtime::subagent::AgentState::Inactive
                    | crate::runtime::subagent::AgentState::Unavailable
            ) {
                native_cut.activation_id = None;
                let registry = self.agent_registry()?.clone();
                let activation = agent.activation_id.clone();
                let domain = domain.to_owned();
                let path = path.to_vec();
                let mut chunk = physical_read(permit, move || {
                    cold_agent_value_read(
                        &registry,
                        &activation,
                        &native_cut,
                        &domain,
                        item_offset,
                        &path,
                        (offset, max_bytes),
                    )
                })
                .await?;
                let current = self.child_view(id)?;
                if current.activation_id != agent.activation_id
                    || current.child_conversation_id != agent.child_conversation_id
                {
                    return Err(RuntimeClientError::ReadCutExpired);
                }
                chunk.cut.activation_id = Some(agent.activation_id);
                return Ok(chunk);
            }
            let endpoint = self
                .agent_registry()?
                .agent_inspection_path(id)
                .ok_or_else(|| failed("live domain value source unavailable"))?;
            let mut peer = Peer::open(
                endpoint,
                &agent.child_conversation_id,
                &agent.activation_id,
                permit,
            )
            .await?;
            let request_id = peer.next_id();
            let result = peer
                .request(RuntimeClientRequest::CheckpointValueRead {
                    id: request_id,
                    cut: native_cut,
                    domain: domain.into(),
                    item_offset,
                    path: path.to_vec(),
                    offset,
                    max_bytes,
                })
                .await?;
            if self.child_view(id)?.activation_id != agent.activation_id {
                return Err(RuntimeClientError::ReadCutExpired);
            }
            match result {
                RuntimeClientResult::CheckpointValueChunk { chunk } => {
                    peer.validate_cut(&chunk.cut)?;
                    Ok(chunk)
                }
                _ => Err(failed("unexpected domain value response")),
            }
        })
        .await
        .map_err(|_| failed("total domain value deadline exhausted"))?
    }

    pub(crate) async fn agent_checkpoint_page(
        &self,
        id: &AgentId,
        cut: &super::checkpoint::CheckpointReadCut,
        domain: &str,
        offset: u64,
        limit: usize,
    ) -> Result<super::checkpoint::CheckpointPage, RuntimeClientError> {
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        tokio::time::timeout_at(deadline, async {
            let permit = self.reserve_inspection()?;
            let agent = self.child_view(id)?;
            if cut.conversation_id != agent.child_conversation_id
                || cut.activation_id.as_ref() != Some(&agent.activation_id)
            {
                return Err(RuntimeClientError::ReadCutExpired);
            }
            let mut native_cut = cut.clone();
            if matches!(
                agent.state,
                crate::runtime::subagent::AgentState::Inactive
                    | crate::runtime::subagent::AgentState::Unavailable
            ) {
                native_cut.activation_id = None;
                let registry = self.agent_registry()?.clone();
                let activation = agent.activation_id.clone();
                let domain = domain.to_owned();
                let mut page = physical_read(permit, move || {
                    cold_agent_page(&registry, &activation, &native_cut, &domain, offset, limit)
                })
                .await?;
                let current = self.child_view(id)?;
                if current.activation_id != agent.activation_id
                    || current.child_conversation_id != agent.child_conversation_id
                {
                    return Err(RuntimeClientError::ReadCutExpired);
                }
                page.read_cut.activation_id = Some(agent.activation_id.clone());
                return Ok(page);
            }
            let path = self
                .agent_registry()?
                .agent_inspection_path(id)
                .ok_or_else(|| failed("live page source unavailable"))?;
            let mut peer = Peer::open(
                path,
                &agent.child_conversation_id,
                &agent.activation_id,
                permit,
            )
            .await?;
            let request_id = peer.next_id();
            let result = peer
                .request(RuntimeClientRequest::CheckpointPageGet {
                    id: request_id,
                    cut: native_cut,
                    domain: domain.into(),
                    offset,
                    limit,
                })
                .await?;
            if self.child_view(id)?.activation_id != agent.activation_id {
                return Err(RuntimeClientError::ReadCutExpired);
            }
            match result {
                RuntimeClientResult::CheckpointPage { page } => {
                    peer.validate_cut(&page.read_cut)?;
                    Ok(page)
                }
                _ => Err(failed("unexpected page response")),
            }
        })
        .await
        .map_err(|_| failed("total page read deadline exhausted"))?
    }

    pub(crate) async fn agent_observe(
        &self,
        id: &AgentId,
        after: Option<AgentObservationCursor>,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        // One absolute budget covers Registry, IPC, initialization, checkpoint,
        // event wait, and source transition. Nested steps never reset this deadline.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(15);
        self.agent_observe_with_deadline(id, after, deadline).await
    }
    pub(crate) async fn agent_observe_with_deadline(
        &self,
        id: &AgentId,
        after: Option<AgentObservationCursor>,
        deadline: tokio::time::Instant,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        if tokio::time::Instant::now() >= deadline {
            return Err(failed("total observation deadline exhausted"));
        }
        let result =
            tokio::time::timeout_at(deadline, self.agent_observe_until(id, after, deadline))
                .await
                .map_err(|_| failed("total observation deadline exhausted"))?;
        if tokio::time::Instant::now() >= deadline {
            return Err(failed("total observation deadline exhausted"));
        }
        result
    }
    async fn agent_observe_until(
        &self,
        id: &AgentId,
        after: Option<AgentObservationCursor>,
        deadline: tokio::time::Instant,
    ) -> Result<RuntimeClientResult, RuntimeClientError> {
        let permit = self.reserve_inspection()?;
        let registry = self.agent_registry()?;
        let mut agent = self.child_view(id)?;
        if agent.state == crate::runtime::subagent::AgentState::Admitting {
            tokio::time::timeout_at(
                deadline,
                registry.wait_for_agent_state(id, |state| {
                    state != crate::runtime::subagent::AgentState::Admitting
                }),
            )
            .await
            .map_err(|_| failed("activation admission deadline exhausted"))?
            .ok_or_else(|| RuntimeClientError::UnknownAgent {
                agent_id: id.clone(),
            })?;
            agent = self.child_view(id)?;
        }
        let activation_id = agent.activation_id.clone();
        let conversation_id = agent.child_conversation_id.clone();
        let durable = matches!(
            agent.state,
            crate::runtime::subagent::AgentState::Inactive
                | crate::runtime::subagent::AgentState::Unavailable
        );
        let source = if durable {
            ObservationSource::DurableOnly
        } else {
            ObservationSource::Live
        };
        let resync = |reason: &str| RuntimeClientResult::AgentObserve {
            observation: AgentObservation::ResyncRequired {
                agent_id: id.clone(),
                conversation_id: conversation_id.clone(),
                activation_id: activation_id.clone(),
                source,
                reason: reason.into(),
            },
        };
        if let Some(cursor) = &after {
            if cursor.agent_id != *id
                || cursor.conversation_id != conversation_id
                || cursor.activation_id != activation_id
            {
                return Ok(resync("activation or observation domain changed"));
            }
            if durable {
                return Ok(resync(
                    "live source ended; reopen retained authoritative history",
                ));
            }
        }
        let observation = if durable {
            Ok(AgentObservation::Checkpoint {
                agent_id: id.clone(),
                conversation_id: conversation_id.clone(),
                activation_id: activation_id.clone(),
                source,
                cursor: None,
                checkpoint: Box::new(self.durable_agent_checkpoint(&agent, permit).await?),
            })
        } else {
            match registry.agent_inspection_path(id) {
                Some(path) => {
                    live_observation(id, &conversation_id, &activation_id, path, after, permit)
                        .await
                }
                None => Err(failed("live inspection source unavailable")),
            }
        };
        let current = self.child_view(id)?;
        if current.activation_id != activation_id {
            return Ok(resync("activation changed during observation"));
        }
        // A child may seal and close its inspection socket during an admitted
        // read. Only the Registry's actual terminal owner state authorizes the
        // switch to retained history; a transport failure alone never does.
        if !durable
            && matches!(
                current.state,
                crate::runtime::subagent::AgentState::Inactive
                    | crate::runtime::subagent::AgentState::Unavailable
            )
            && matches!(
                &observation,
                Ok(_) | Err(RuntimeClientError::ObservationUnavailable { .. })
            )
        {
            return Ok(RuntimeClientResult::AgentObserve {
                observation: AgentObservation::ResyncRequired {
                    agent_id: id.clone(),
                    conversation_id,
                    activation_id,
                    source: ObservationSource::DurableOnly,
                    reason: "live source ended during observation".into(),
                },
            });
        }
        Ok(RuntimeClientResult::AgentObserve {
            observation: observation?,
        })
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    fn identity_response(
        id: RequestId,
        conversation_id: ConversationId,
        activation_id: SubagentId,
    ) -> Vec<u8> {
        let response = RuntimeClientResponse {
            id,
            result: Some(RuntimeClientResult::Initialized {
                attachment_id: super::super::types::AttachmentId::new("inspection-fixture"),
                conversation_id,
                agent_id: AgentId::new("agent-fixture"),
                activation_id: Some(activation_id),
                protocol_version: RUNTIME_CLIENT_PROTOCOL_VERSION,
                read_only: true,
                capabilities: vec!["checkpoint".into()],
            }),
            error: None,
        };
        let mut response = serde_json::to_vec(&response).unwrap();
        response.push(b'\n');
        assert!(response.len() < 4096);
        response
    }

    #[tokio::test(start_paused = true)]
    async fn t20_real_unix_initialize_and_checkpoint_share_one_absolute_deadline() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("deadline.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let conversation = ConversationId::new("conv_01900000-0000-7000-8000-000000000096");
        let activation = SubagentId::new("sub_01900000-0000-7000-8000-000000000096");
        let response_conversation = conversation.clone();
        let response_activation = activation.clone();
        let (initialized, initialized_rx) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let (checkpoint, checkpoint_rx) = tokio::sync::oneshot::channel();
        // Keep the test clock under explicit control while real Unix readiness
        // is pending. Scheduling never advances the deadline by itself.
        let clock_guard = tokio::spawn(async {
            loop {
                tokio::task::yield_now().await;
            }
        });
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut record = String::new();
            stream.read_line(&mut record).await.unwrap();
            let request: RuntimeClientRequest = serde_json::from_str(&record).unwrap();
            assert!(matches!(request, RuntimeClientRequest::Initialize { .. }));
            initialized.send(()).unwrap();
            released.await.unwrap();
            stream
                .get_mut()
                .write_all(&identity_response(
                    request.id(),
                    response_conversation,
                    response_activation,
                ))
                .await
                .unwrap();
            record.clear();
            stream.read_line(&mut record).await.unwrap();
            let request: RuntimeClientRequest = serde_json::from_str(&record).unwrap();
            assert!(matches!(
                request,
                RuntimeClientRequest::CheckpointGet { .. }
            ));
            checkpoint.send(()).unwrap();
            record.clear();
            assert_eq!(stream.read_line(&mut record).await.unwrap(), 0);
        });
        let capacity = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let permit = capacity.clone().try_acquire_owned().unwrap();
        let start = tokio::time::Instant::now();
        let deadline = start + std::time::Duration::from_secs(15);
        let observing = tokio::spawn(async move {
            tokio::time::timeout_at(deadline, async {
                let mut peer = Peer::open(path, &conversation, &activation, permit).await?;
                peer.checkpoint().await
            })
            .await
        });
        initialized_rx.await.unwrap();
        tokio::time::advance(std::time::Duration::from_secs(10)).await;
        release.send(()).unwrap();
        checkpoint_rx.await.unwrap();
        assert_eq!(
            tokio::time::Instant::now(),
            start + std::time::Duration::from_secs(10)
        );
        tokio::time::advance(std::time::Duration::from_secs(5)).await;
        assert!(observing.await.unwrap().is_err());
        server.await.unwrap();
        assert_eq!(capacity.available_permits(), 1);
        clock_guard.abort();
        assert!(clock_guard.await.unwrap_err().is_cancelled());
    }

    #[tokio::test]
    async fn t20_cancelled_physical_sql_worker_retains_inspection_capacity_until_exit() {
        let capacity = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let permit = capacity.clone().try_acquire_owned().unwrap();
        let (entered, entered_rx) = tokio::sync::oneshot::channel();
        let (release, release_rx) = std::sync::mpsc::channel();
        let waiting = tokio::spawn(physical_read(permit, move || {
            let store = crate::durable::SqliteConversationStore::in_memory(ConversationId::new(
                "conv_01900000-0000-7000-8000-000000000098",
            ))
            .unwrap();
            entered.send(()).unwrap();
            release_rx.recv().unwrap();
            store.observation_read_cut().map_err(failed)
        }));
        entered_rx.await.unwrap();
        waiting.abort();
        assert!(waiting.await.unwrap_err().is_cancelled());
        assert_eq!(capacity.available_permits(), 0);
        assert!(capacity.clone().try_acquire_owned().is_err());
        release.send(()).unwrap();
        let reclaimed = capacity.clone().acquire_owned().await.unwrap();
        drop(reclaimed);
        assert_eq!(capacity.available_permits(), 1);
    }

    #[tokio::test]
    async fn t18_real_unix_initialization_rejects_old_activation_and_releases_capacity() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("inspect.sock");
        let listener = tokio::net::UnixListener::bind(&path).unwrap();
        let conversation = ConversationId::new("conv_01900000-0000-7000-8000-000000000099");
        let current = SubagentId::new("sub_01900000-0000-7000-8000-000000000002");
        let old = SubagentId::new("sub_01900000-0000-7000-8000-000000000001");
        let actual_conversation = conversation.clone();
        let actual_activation = current.clone();
        let task = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            let mut stream = BufReader::new(stream);
            let mut record = String::new();
            stream.read_line(&mut record).await.unwrap();
            let request: RuntimeClientRequest = serde_json::from_str(&record).unwrap();
            assert!(matches!(request, RuntimeClientRequest::Initialize { .. }));
            let response = identity_response(request.id(), actual_conversation, actual_activation);
            stream.get_mut().write_all(&response).await.unwrap();
            let mut later = String::new();
            assert_eq!(
                stream.read_line(&mut later).await.unwrap(),
                0,
                "wrong activation closes before checkpoint/subscription"
            );
        });
        let capacity = std::sync::Arc::new(tokio::sync::Semaphore::new(8));
        let permit = capacity.clone().try_acquire_owned().unwrap();
        let result = Peer::open(path, &conversation, &old, permit).await;
        assert!(matches!(
            result,
            Err(RuntimeClientError::ObservationUnavailable { .. })
        ));
        task.await.unwrap();
        assert_eq!(capacity.available_permits(), 8);
    }

    #[tokio::test]
    async fn peer_requires_bounded_complete_jsonl_records() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let mut peer = Peer {
            stream: BufReader::new(client),
            pending_record: Vec::new(),
            id: 0,
            conversation_id: ConversationId::new("conv_01900000-0000-7000-8000-000000000087"),
            activation_id: SubagentId::new("sub_01900000-0000-7000-8000-000000000002"),
            _permit: std::sync::Arc::new(tokio::sync::Semaphore::new(8))
                .try_acquire_owned()
                .unwrap(),
        };
        server.write_all(b"{\"cut\":1}\n{\"cut\":2}").await.unwrap();
        drop(server);
        assert_eq!(peer.record().await.unwrap(), serde_json::json!({"cut":1}));
        assert!(
            peer.record().await.is_err(),
            "unterminated EOF is never a cut"
        );
    }

    #[tokio::test]
    async fn t08_t12_real_unix_interaction_reads_preserve_bytes_and_fence_source_substitution() {
        use crate::runtime::identity::InteractionId;
        use crate::runtime::interaction::InteractionRef;
        use base64::Engine as _;
        let (client, server) = UnixStream::pair().unwrap();
        let reference = InteractionRef::new(
            ConversationId::new("conv_01900000-0000-7000-8000-000000000087"),
            InteractionId::new("pending-source"),
        );
        let capacity = std::sync::Arc::new(tokio::sync::Semaphore::new(1));
        let mut peer = Peer {
            stream: BufReader::new(client),
            pending_record: Vec::new(),
            id: 0,
            conversation_id: ConversationId::new("conv_01900000-0000-7000-8000-000000000087"),
            activation_id: SubagentId::new("sub_01900000-0000-7000-8000-000000000002"),
            _permit: capacity.clone().try_acquire_owned().unwrap(),
        };
        let task = tokio::spawn(async move {
            let mut server = BufReader::new(server);
            for replace in [false, true] {
                let mut record = String::new();
                server.read_line(&mut record).await.unwrap();
                let request: RuntimeClientRequest = serde_json::from_str(&record).unwrap();
                let id = request.id();
                let RuntimeClientRequest::InteractionValueRead {
                    mut interaction,
                    path,
                    offset,
                    max_bytes,
                    ..
                } = request
                else {
                    panic!("exact interaction read")
                };
                assert_eq!(offset, 0);
                assert_eq!(max_bytes, 3);
                if replace {
                    interaction.interaction_id = InteractionId::new("different-pending-source");
                }
                let result = RuntimeClientResult::InteractionValueChunk {
                    interaction,
                    path,
                    offset,
                    // Actual UTF-8 may cross the exact byte chunk boundary.
                    bytes_base64: base64::engine::general_purpose::STANDARD.encode(b"\"\xf0\x9f"),
                    readable_end: 6,
                    next_offset: Some(3),
                };
                let response = RuntimeClientResponse {
                    id,
                    result: Some(result),
                    error: None,
                };
                let mut bytes = serde_json::to_vec(&response).unwrap();
                bytes.push(b'\n');
                server.get_mut().write_all(&bytes).await.unwrap();
            }
            let mut record = String::new();
            assert_eq!(server.read_line(&mut record).await.unwrap(), 0);
        });
        let RuntimeClientResult::InteractionValueChunk { bytes_base64, .. } = peer
            .interaction_value_read(&reference, &["kind".into(), "arguments".into()], 0, 3)
            .await
            .unwrap()
        else {
            panic!("byte chunk")
        };
        assert_eq!(
            base64::engine::general_purpose::STANDARD
                .decode(bytes_base64)
                .unwrap(),
            b"\"\xf0\x9f"
        );
        assert!(matches!(
            peer.interaction_value_read(&reference, &[], 0, 3).await,
            Err(RuntimeClientError::ObservationUnavailable { .. })
        ));
        drop(peer);
        task.await.unwrap();
        assert_eq!(capacity.available_permits(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn peer_preserves_partial_record_when_watch_wait_expires() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let mut peer = Peer {
            stream: BufReader::new(client),
            pending_record: Vec::new(),
            id: 0,
            conversation_id: ConversationId::new("conv_01900000-0000-7000-8000-000000000087"),
            activation_id: SubagentId::new("sub_01900000-0000-7000-8000-000000000002"),
            _permit: std::sync::Arc::new(tokio::sync::Semaphore::new(8))
                .try_acquire_owned()
                .unwrap(),
        };
        server.write_all(b"{\"cut\":").await.unwrap();
        assert!(
            tokio::time::timeout(std::time::Duration::from_secs(10), peer.record())
                .await
                .is_err()
        );
        server.write_all(b"1}\n{\"cut\":2}\n").await.unwrap();
        assert_eq!(peer.record().await.unwrap(), serde_json::json!({"cut": 1}));
        assert_eq!(peer.record().await.unwrap(), serde_json::json!({"cut": 2}));
    }

    #[tokio::test]
    async fn peer_refuses_oversized_native_records_before_deserialization() {
        let (client, mut server) = UnixStream::pair().unwrap();
        let writing = tokio::spawn(async move {
            let bytes =
                vec![b'x'; super::super::transport::stdio::STDIO_JSONL_MAX_RECORD_BYTES + 2];
            let _ = server.write_all(&bytes).await;
        });
        let mut peer = Peer {
            stream: BufReader::new(client),
            pending_record: Vec::new(),
            id: 0,
            conversation_id: ConversationId::new("conv_01900000-0000-7000-8000-000000000087"),
            activation_id: SubagentId::new("sub_01900000-0000-7000-8000-000000000002"),
            _permit: std::sync::Arc::new(tokio::sync::Semaphore::new(8))
                .try_acquire_owned()
                .unwrap(),
        };
        assert!(
            matches!(peer.record().await, Err(RuntimeClientError::ObservationUnavailable { message }) if message.contains("exceeds native bound"))
        );
        drop(peer);
        writing.await.unwrap();
    }
}
