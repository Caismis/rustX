//! Finite foreground Tool occurrence descriptors keyed by canonical identity.
use crate::durable::observation::{NativeToolObservationHeader, NativeToolObservationPage};
use crate::message::types::ContentBlockIndex;
use crate::runtime::identity::{MessageId, ToolCallId, ToolId};
use crate::tools::types::{ToolExecutionResult, ToolExecutionStatus, ToolProgress};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientToolOccurrence {
    pub locator: super::content::CanonicalContentLocator,
    pub block_index: ContentBlockIndex,
}
pub(crate) fn inline_tool_id(id: &ToolId) -> Option<ToolId> {
    (id.as_str().len() <= 256).then(|| id.clone())
}
pub(crate) fn inline_message_id(identity: &MessageId) -> Option<MessageId> {
    (identity.as_str().len() <= 256).then(|| identity.clone())
}
pub(crate) fn inline_call_id(id: &ToolCallId) -> Option<ToolCallId> {
    (id.as_str().len() <= 256).then(|| id.clone())
}
/// An exact immutable native Journal field authorized by the checkpoint read cut.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JournalFieldRef {
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub sequence: u64,
    pub path: Vec<String>,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub encoded_bytes: u64,
}
/// The foreground tool execution read model of one logical tool call.
///
/// Keyed by the canonical logical tool-call identity, so parallel physical
/// completion timing can never corrupt logical identities or canonical
/// ordering. Native, MCP, and Python foreground executions converge through
/// this one shape.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct ForegroundToolExecution {
    pub occurrence: RuntimeClientToolOccurrence,
    /// Exact inline Assistant identity when bounded; `occurrence` remains authoritative.
    pub message_id: Option<MessageId>,
    /// Canonical Assistant content block (also used by live publication frames).
    pub block_index: ContentBlockIndex,
    /// The logical tool-call identity.
    pub call_id: Option<ToolCallId>,
    pub metadata_ref: Option<super::content::ContentRef>,
    pub assembly_complete: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_source: Option<JournalFieldRef>,
    /// Canonical-only settlement metadata, including before-Start outcomes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result_metadata_ref: Option<super::content::ContentRef>,
    /// A committed canonical result is independent of execution-terminal Journal publication.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub committed_result: Option<ToolExecutionResult>,
    /// The committed C result body is available through its exact metadata/payload sources.
    pub committed_result_deferred: bool,
    /// The canonical tool identity.
    pub tool_id: Option<ToolId>,
    /// The model-facing tool name at call time.
    pub name: String,
    /// The externally meaningful execution state.
    pub state: ForegroundToolState,
}
/// The externally meaningful state of one foreground tool execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum ForegroundToolState {
    /// The call is known and its arguments are assembled; execution has
    /// not started.
    Assembled {
        /// The assembled JSON arguments.
        arguments: String,
    },
    /// The execution is running.
    Running {
        /// The assembled JSON arguments.
        arguments: String,
        /// The latest bounded progress, when any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        progress: Option<ToolProgress>,
    },
    /// The execution settled with its normalized result.
    Settled {
        /// The assembled JSON arguments.
        arguments: String,
        /// The normalized execution result.
        result: ToolExecutionResult,
    },
}

pub(crate) fn foreground(
    page: &NativeToolObservationPage,
    header: &NativeToolObservationHeader,
) -> Result<ForegroundToolExecution, crate::runtime_client::RuntimeClientError> {
    let association = &header.association;
    let committed_result = header
        .committed_result
        .as_ref()
        .map(|value| value.result.clone());
    let result_metadata_ref = header
        .committed_result
        .as_ref()
        .and(association.result.as_ref())
        .and_then(|result| {
            result.content.iter().find_map(|binding| {
                matches!(
                    binding.content_ref.source,
                    super::content::ContentSource::CanonicalMessageBlock {
                        component: super::content::CanonicalContentComponent::MessageMetadata,
                        ..
                    }
                )
                .then(|| binding.content_ref.clone())
            })
        });
    let arguments = header.arguments_json.clone().unwrap_or_default();
    let (state, result_source) = tool_state(header, arguments, committed_result.as_ref())?;
    let mut view = ForegroundToolExecution {
        occurrence: RuntimeClientToolOccurrence {
            locator: page.locator.clone(),
            block_index: association.block_index,
        },
        message_id: page.message_id.clone(),
        block_index: association.block_index,
        call_id: association.call_id.clone(),
        tool_id: association.tool_id.clone(),
        name: header.name.clone(),
        metadata_ref: association.call.content.iter().find_map(|binding| {
            matches!(
                binding.content_ref.source,
                super::content::ContentSource::CanonicalMessageBlock {
                    component: super::content::CanonicalContentComponent::BlockMetadata,
                    ..
                }
            )
            .then(|| binding.content_ref.clone())
        }),
        assembly_complete: true,
        committed_result_deferred: header
            .committed_result
            .as_ref()
            .is_some_and(|value| value.result_deferred),
        committed_result,
        result_source,
        result_metadata_ref,
        state,
    };
    if super::bounded::bounded_bytes(&view, 8_192).is_none() {
        if let Some(result) = &mut view.committed_result {
            defer_result(result);
        }
        if let ForegroundToolState::Settled { result, .. } = &mut view.state {
            defer_result(result);
        }
        view.committed_result_deferred = view.committed_result.is_some();
        view.result_metadata_ref = association.result.as_ref().and_then(|result| {
            result.content.iter().find_map(|binding| {
                matches!(
                    binding.content_ref.source,
                    super::content::ContentSource::CanonicalMessageBlock {
                        component: super::content::CanonicalContentComponent::MessageMetadata,
                        ..
                    }
                )
                .then(|| binding.content_ref.clone())
            })
        });
    }
    Ok(view)
}
fn tool_state(
    header: &NativeToolObservationHeader,
    arguments: String,
    committed_result: Option<&crate::tools::types::ToolExecutionResult>,
) -> Result<
    (ForegroundToolState, Option<super::tool::JournalFieldRef>),
    crate::runtime_client::RuntimeClientError,
> {
    let value = if let Some(terminal) = &header.terminal {
        let Some(mut result) = super::tool::tool_witness_result(terminal) else {
            return Err(
                crate::runtime_client::RuntimeClientError::ObservationUnavailable {
                    message: "native Tool terminal status unavailable".into(),
                },
            );
        };
        if let Some(committed) = committed_result {
            result.content.clone_from(&committed.content);
            result.artifacts.clone_from(&committed.artifacts);
            result.deliveries.clone_from(&committed.deliveries);
            result.workflow.clone_from(&committed.workflow);
            result.managed_output.clone_from(&committed.managed_output);
        }
        let field = if terminal.event_kind == "tool_execution_failed" {
            "error"
        } else {
            "result"
        };
        let source = terminal
            .fields
            .iter()
            .find(|entry| entry.path == ["event", field] && entry.encoded_bytes > 0)
            .map(|entry| super::tool::JournalFieldRef {
                sequence: terminal.sequence,
                path: entry.path.clone(),
                encoded_bytes: entry.encoded_bytes,
            });
        (ForegroundToolState::Settled { arguments, result }, source)
    } else if header.started.is_some() {
        (
            ForegroundToolState::Running {
                arguments,
                progress: None,
            },
            None,
        )
    } else {
        (ForegroundToolState::Assembled { arguments }, None)
    };
    Ok(value)
}
pub(crate) fn has_result_body(result: &crate::tools::types::ToolExecutionResult) -> bool {
    !result.content.is_empty()
        || !result.artifacts.is_empty()
        || !result.deliveries.is_empty()
        || result.workflow.is_some()
        || result.managed_output.is_some()
}
pub(crate) fn defer_result(result: &mut crate::tools::types::ToolExecutionResult) {
    result.content.clear();
    result.artifacts.clear();
    result.deliveries.clear();
    result.workflow = None;
    result.managed_output = None;
}
pub(crate) fn content(
    header: &NativeToolObservationHeader,
    view: &ForegroundToolExecution,
) -> Vec<super::cut::ContentBinding> {
    let mut result = Vec::new();
    if let Some(binding) = header
        .association
        .call
        .content
        .iter()
        .find(|binding| {
            binding.content_ref.representation == super::content::ContentRepresentation::RawJsonUtf8
        })
        .or_else(|| {
            header.association.call.content.iter().find(|binding| {
                binding.content_ref.representation
                    == super::content::ContentRepresentation::CanonicalJsonV1
                    && matches!(
                        binding.content_ref.source,
                        super::content::ContentSource::CanonicalMessageBlock {
                            component: super::content::CanonicalContentComponent::Payload,
                            ..
                        }
                    )
            })
        })
    {
        let mut binding = binding.clone();
        binding.path = vec!["state".into(), "arguments".into()];
        binding.deferred = header.arguments_json.is_none();
        result.push(binding);
    }
    if let Some(reference) = &view.result_metadata_ref {
        result.push(super::cut::ContentBinding {
            path: vec!["committed_result".into()],
            source_path: Some(vec!["result".into()]),
            content_ref: reference.clone(),
            deferred: view.committed_result_deferred,
        });
    }
    if let Some(source) = &header.association.result {
        for binding in source.content.iter().filter(|binding| {
            matches!(
                binding.content_ref.source,
                super::content::ContentSource::CanonicalMessageBlock {
                    component: super::content::CanonicalContentComponent::Payload,
                    ..
                }
            )
        }) {
            let Some(index) = binding.path.iter().position(|field| field == "content") else {
                continue;
            };
            for prefix in [
                vec!["committed_result".to_owned()],
                vec!["state".to_owned(), "result".to_owned()],
            ] {
                if prefix.first().is_some_and(|field| field == "state")
                    && !matches!(view.state, ForegroundToolState::Settled { .. })
                {
                    continue;
                }
                let mut selected = binding.clone();
                selected.path = prefix
                    .into_iter()
                    .chain(binding.path[index..].iter().cloned())
                    .collect();
                result.push(selected);
            }
        }
    }
    result
}
pub(crate) fn collections(
    header: &NativeToolObservationHeader,
) -> Vec<super::cut::CollectionFrontier> {
    let Some(result) = header.committed_result.as_ref() else {
        return Vec::new();
    };
    let Some(source) = &header.association.result else {
        return Vec::new();
    };
    let Some(position) = source.ledger_position else {
        return Vec::new();
    };
    let domain = format!("messages.ledger:{position}.blocks");
    let frontier = source
        .collections
        .iter()
        .find(|frontier| frontier.domain == domain)
        .cloned()
        .unwrap_or(super::cut::CollectionFrontier {
            domain,
            revision: 0,
            total: result.content_total,
            loaded: 0,
            next_offset: (result.content_total > 0).then_some(0),
        });
    vec![frontier]
}
pub(crate) fn enrich_message(
    header: &mut crate::durable::observation::MessageHeader,
    tools: &NativeToolObservationPage,
) -> Result<(), crate::runtime_client::RuntimeClientError> {
    if header.role != "assistant" || tools.total == 0 {
        return Ok(());
    }
    let domain = format!("messages.{}.tools", tools.locator.key());
    header
        .collections
        .retain(|frontier| frontier.domain != domain);
    header.tool_calls.clear();
    let mut frontier = super::cut::CollectionFrontier {
        domain,
        revision: tools.cut.read.journal,
        total: tools.total,
        loaded: 0,
        next_offset: tools
            .headers
            .first()
            .map(|tool| u64::from(tool.association.block_index.get()))
            .or(tools.next_offset),
    };
    header.collections.push(frontier.clone());
    for tool in &tools.headers {
        let view = foreground(tools, tool)?;
        let mut candidate = header.clone();
        let index = candidate.tool_calls.len();
        for mut binding in content(tool, &view) {
            binding
                .path
                .splice(0..0, ["tool_calls".into(), index.to_string()]);
            candidate.content.push(binding);
        }
        candidate.tool_calls.push(view);
        candidate.collections.extend(collections(tool));
        if super::bounded::bounded_bytes(&candidate, 14_336).is_none() {
            break;
        }
        *header = candidate;
    }
    frontier.loaded = header.tool_calls.len() as u64;
    frontier.next_offset = tools
        .headers
        .get(header.tool_calls.len())
        .map(|tool| u64::from(tool.association.block_index.get()))
        .or(tools.next_offset);
    if let Some(current) = header
        .collections
        .iter_mut()
        .find(|current| current.domain == frontier.domain)
    {
        *current = frontier;
    }
    Ok(())
}
pub(crate) fn enrich_transcript_header(
    header: &mut crate::durable::observation::TranscriptHeader,
    tools: &NativeToolObservationPage,
) -> Result<(), crate::runtime_client::RuntimeClientError> {
    if let Some(message) = &mut header.message {
        enrich_message(message, tools)?;
        message.tool_calls.clear();
        message.content.retain(|binding| {
            binding
                .path
                .first()
                .is_none_or(|field| field != "tool_calls")
        });
    }
    enrich_loaded_transcript_header(header, tools)
}
pub(crate) fn enrich_loaded_transcript_header(
    header: &mut crate::durable::observation::TranscriptHeader,
    tools: &NativeToolObservationPage,
) -> Result<(), crate::runtime_client::RuntimeClientError> {
    header.tool_calls.clear();
    if let Some(display) = &mut header.display {
        display.tool_calls.clear();
    }
    let domain = format!("messages.{}.tools", tools.locator.key());
    header
        .collections
        .retain(|frontier| frontier.domain != domain);
    header
        .content
        .retain(|binding| !binding.path.iter().any(|field| field == "tool_calls"));
    header.collections.push(super::cut::CollectionFrontier {
        domain: domain.clone(),
        revision: tools.cut.read.journal,
        total: tools.total,
        loaded: 0,
        next_offset: tools
            .headers
            .first()
            .map(|tool| u64::from(tool.association.block_index.get()))
            .or(tools.next_offset),
    });
    for tool in &tools.headers {
        let view = foreground(tools, tool)?;
        let mut candidate = header.clone();
        let index = candidate.tool_calls.len();
        for mut binding in content(tool, &view) {
            let mut prefix = vec!["tool_calls".into(), index.to_string()];
            if candidate.display.is_some() {
                prefix.insert(0, "display".into());
            }
            binding.path.splice(0..0, prefix);
            candidate.content.push(binding);
        }
        candidate.collections.extend(collections(tool));
        candidate.tool_calls.push(view);
        if super::bounded::bounded_bytes(&candidate, 16_384).is_none() {
            break;
        }
        *header = candidate;
    }
    let loaded = header.tool_calls.len() as u64;
    let next_offset = tools
        .headers
        .get(header.tool_calls.len())
        .map(|tool| u64::from(tool.association.block_index.get()))
        .or(tools.next_offset);
    for frontier in header.collections.iter_mut().chain(
        header
            .message
            .iter_mut()
            .flat_map(|message| &mut message.collections),
    ) {
        if frontier.domain == domain {
            frontier.loaded = loaded;
            frontier.next_offset = next_offset;
        }
    }
    Ok(())
}

pub(crate) fn tool_witness_result(
    witness: &crate::durable::observation::ObservationEventWitness,
) -> Option<ToolExecutionResult> {
    let value = &witness.value;
    let status = if witness.event_kind == "tool_execution_failed" {
        ToolExecutionStatus::Failed {
            error: value
                .get("error")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("deferred tool diagnostic")
                .to_owned(),
        }
    } else {
        value
            .get("result")
            .and_then(|result| result.get("status"))
            .and_then(|status| {
                super::structural_json::decode::<ToolExecutionStatus>(status.clone()).ok()
            })?
    };
    let result_value = value.get("result");
    let scalar_u64 = |field: &str| {
        result_value
            .and_then(|result| result.get(field))
            .and_then(|value| value.as_u64().or_else(|| value.as_str()?.parse().ok()))
    };
    Some(ToolExecutionResult {
        status,
        content: Vec::new(),
        duration_ms: scalar_u64("duration_ms").unwrap_or(0),
        exit_code: result_value
            .and_then(|result| result.get("exit_code"))
            .and_then(serde_json::Value::as_i64)
            .and_then(|code| i32::try_from(code).ok()),
        artifacts: Vec::new(),
        deliveries: Vec::new(),
        truncation: result_value
            .and_then(|result| result.get("truncation"))
            .and_then(|value| super::structural_json::decode(value.clone()).ok()),
        workflow: None,
        managed_output: result_value
            .and_then(|result| result.get("managed_output"))
            .and_then(|value| super::structural_json::decode(value.clone()).ok()),
    })
}
