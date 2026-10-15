//! Immutable historical context content and its structural compiler.
//!
//! A checkpoint decides all eligible history through its base Ledger frontier.
//! Its occurrence coordinates are separate from physical canonical Tool calls,
//! actual-request coordinates and provider wire correlation strings.
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::resources::{LedgerFrontier, ResourceReadCut};
use crate::message::types::{
    AssistantContentBlock, ContentBlockIndex, MessageBlock, ToolCallOccurrenceRef,
};
use crate::model::input::ModelInputMessage;
use crate::model::projected::{
    CanonicalMessageRef, DerivedModelMessage, ModelMessageView, ProjectedMessage,
};
use crate::runtime::identity::{CheckpointId, ConversationId, RequestId, ToolCallId, ToolId};

/// Checkpoint activation order, independent of the Ledger and Journal domains.
#[derive(
    Debug,
    Default,
    Clone,
    Copy,
    PartialEq,
    Eq,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(transparent)]
pub struct ProjectionActivationId(pub u64);

#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct CheckpointMessageOccurrence {
    pub checkpoint_id: CheckpointId,
    pub item_index: u32,
}
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
pub struct RequestMessageOccurrence {
    pub request_id: RequestId,
    pub item_index: u32,
}

/// A projected block coordinate. It cannot be substituted for a canonical block index.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct CheckpointBlockIndex(pub u32);
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(transparent)]
pub struct RequestBlockIndex(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct CheckpointCallCoordinate {
    pub message: CheckpointMessageOccurrence,
    pub block: CheckpointBlockIndex,
}

/// Explicit physical source for a Derived call block, including the original coordinate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectedCallSource {
    pub block: CheckpointBlockIndex,
    pub assistant: CanonicalMessageRef,
    pub canonical_block: ContentBlockIndex,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum CheckpointMessage {
    Original(CanonicalMessageRef),
    Derived(Box<DerivedModelMessage>),
}

/// Tool associations are algorithm-authored coordinates validated against typed source facts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointItem {
    pub message: CheckpointMessage,
    pub call_sources: Vec<ProjectedCallSource>,
    /// Required for Derived `ToolResults`. Originals may explicitly disambiguate repeated copies.
    pub result_call: Option<CheckpointCallCoordinate>,
}

/// Complete historical inclusion/exclusion decision. No current execution state is stored here.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointCandidate {
    pub checkpoint_id: CheckpointId,
    pub expected_head: Option<CheckpointId>,
    pub read_cut: ResourceReadCut,
    pub base_frontier: LedgerFrontier,
    pub items: Vec<CheckpointItem>,
    /// Trusted cumulative file facts, validated from canonical sources and earlier checkpoints.
    pub summary_metadata: Option<crate::message::types::CompactionSummaryMetadata>,
}

impl CheckpointCandidate {
    /// Candidate identity is distinct from a final semantic model-request fingerprint.
    /// # Panics
    /// Only a non-serializable runtime value could violate this contract.
    #[must_use]
    pub fn fingerprint(&self) -> String {
        let mut hash = Sha256::new();
        serde_json::to_writer(crate::model::projected::DigestWriter(&mut hash), self)
            .expect("checkpoint content serializes");
        format!("{:x}", hash.finalize())
    }

    /// Validate the complete materialization budget before resolving source bodies.
    /// # Errors
    /// Oversized content, provenance, item count or an invalid base frontier fail explicitly.
    pub fn validate_budget(&self) -> Result<(), String> {
        if self.base_frontier > self.read_cut.ledger || self.items.len() > 8192 {
            return Err("checkpoint base frontier or item count exceeds authority".into());
        }
        let mut refs = 0_usize;
        for item in &self.items {
            refs = refs
                .checked_add(item.call_sources.len())
                .ok_or("checkpoint provenance overflow")?;
            if let CheckpointMessage::Derived(derived) = &item.message {
                refs = refs
                    .checked_add(derived.sources.len())
                    .and_then(|n| n.checked_add(derived.checkpoint_sources.len()))
                    .ok_or("checkpoint provenance overflow")?;
            }
            if refs > 1024 {
                return Err("checkpoint inline provenance budget exceeded".into());
            }
        }
        serde_json::to_writer(ByteBudget(8 * 1024 * 1024), self)
            .map_err(|_| "checkpoint body byte budget exceeded".to_owned())
    }
}

struct ByteBudget(usize);
impl std::io::Write for ByteBudget {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_sub(bytes.len())
            .ok_or_else(|| std::io::Error::other("checkpoint byte bound"))?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// No Tool execution is inferred from this association; it only compiles model input.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompiledToolAssociation {
    pub call: CheckpointCallCoordinate,
    pub result: CheckpointMessageOccurrence,
    pub canonical_source: ToolCallOccurrenceRef,
    pub wire_id: ToolCallId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledCheckpoint {
    pub messages: Vec<ModelInputMessage>,
    pub tools: Vec<CompiledToolAssociation>,
}

struct Call {
    source: ToolCallOccurrenceRef,
    wire_id: ToolCallId,
    tool_id: ToolId,
}

/// Detached integrity header with an optional body. Provenance-only reads do
/// not hydrate retired facts; visible Originals and native Tool/reasoning
/// validation explicitly require their immutable bodies.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CanonicalDependency {
    pub frontier: LedgerFrontier,
    pub reference: CanonicalMessageRef,
    pub body: Option<MessageBlock>,
}

/// Resolve immutable dependencies and compile all Original/Derived pairings.
/// # Errors
/// Rejects missing dependencies, forged provenance, changed opaque reasoning,
/// orphan/ambiguous results, split batches and invalid projected protocol order.
pub fn compile_checkpoint(
    candidate: &CheckpointCandidate,
    mut resolve: impl FnMut(&CanonicalMessageRef, bool) -> Result<CanonicalDependency, String>,
) -> Result<CompiledCheckpoint, String> {
    candidate.validate_budget()?;
    let mut originals = BTreeMap::new();
    let mut source_budget = ByteBudget(16 * 1024 * 1024);
    let mut messages = Vec::with_capacity(candidate.items.len());
    let mut output_budget = ByteBudget(16 * 1024 * 1024);
    for item in &candidate.items {
        let message = match &item.message {
            CheckpointMessage::Original(reference) => ModelInputMessage::Canonical(original(
                candidate,
                reference,
                &mut resolve,
                &mut originals,
                &mut source_budget,
            )?),
            CheckpointMessage::Derived(derived) => {
                if derived.sources.is_empty() && derived.checkpoint_sources.is_empty() {
                    return Err("historical Derived message requires immutable provenance".into());
                }
                if derived.checkpoint_sources.iter().any(|source| {
                    source.conversation_id != candidate.read_cut.conversation_id
                        || source.checkpoint_id == candidate.checkpoint_id
                }) {
                    return Err("foreign or self-referential checkpoint provenance".into());
                }
                if let ProjectedMessage::User {
                    kind: crate::model::projected::ProjectedUserKind::CompactionContext,
                    source,
                    ..
                } = &derived.message
                    && (*source != crate::message::types::UserSource::Runtime
                        || !derived
                            .transformations
                            .contains(&crate::model::projected::SemanticTransformation::Summary))
                {
                    return Err(
                        "compaction context requires Runtime origin and Summary provenance".into(),
                    );
                }
                let needs_native_body = matches!(&derived.message, ProjectedMessage::Assistant { content } if content.iter().any(|b| matches!(b, AssistantContentBlock::Reasoning(r) if r.provider_state.is_some())));
                for source in &derived.sources {
                    if needs_native_body {
                        original(
                            candidate,
                            source,
                            &mut resolve,
                            &mut originals,
                            &mut source_budget,
                        )?;
                    } else {
                        let dependency = resolve(source, false)?;
                        verify_dependency(candidate, source, &dependency)?;
                    }
                }
                ModelInputMessage::Derived(derived.as_ref().clone())
            }
        };
        for source in &item.call_sources {
            original(
                candidate,
                &source.assistant,
                &mut resolve,
                &mut originals,
                &mut source_budget,
            )?;
        }
        validate_native_reasoning(&message, &originals)?;
        serde_json::to_writer(&mut output_budget, &message)
            .map_err(|_| "compiled checkpoint cumulative materialization budget exceeded")?;
        messages.push(message);
    }
    let calls = collect_calls(candidate, &messages, &originals)?;
    let tools = pair_results(candidate, &messages, &calls)?;
    Ok(CompiledCheckpoint { messages, tools })
}

fn original(
    candidate: &CheckpointCandidate,
    source: &CanonicalMessageRef,
    resolve: &mut impl FnMut(&CanonicalMessageRef, bool) -> Result<CanonicalDependency, String>,
    cache: &mut BTreeMap<crate::runtime::identity::MessageId, MessageBlock>,
    source_budget: &mut ByteBudget,
) -> Result<MessageBlock, String> {
    if source.conversation_id != candidate.read_cut.conversation_id {
        return Err("foreign checkpoint Original source".into());
    }
    if let Some(message) = cache.get(&source.message_id) {
        if !source.matches(message) {
            return Err("conflicting checkpoint Original integrity".into());
        }
        return Ok(message.clone());
    }
    let dependency = resolve(source, true)?;
    verify_dependency(candidate, source, &dependency)?;
    let message = dependency
        .body
        .ok_or("missing required immutable Original body")?;
    if !source.matches(&message) {
        return Err("checkpoint Original body integrity mismatch".into());
    }
    serde_json::to_writer(source_budget, &message)
        .map_err(|_| "checkpoint immutable dependency materialization exceeds budget")?;
    cache.insert(source.message_id.clone(), message.clone());
    Ok(message)
}

fn verify_dependency(
    candidate: &CheckpointCandidate,
    expected: &CanonicalMessageRef,
    dependency: &CanonicalDependency,
) -> Result<(), String> {
    if dependency.frontier.0 == 0
        || dependency.frontier > candidate.base_frontier
        || dependency.reference != *expected
        || expected.conversation_id != candidate.read_cut.conversation_id
    {
        return Err("checkpoint Original is outside base or changed".into());
    }
    Ok(())
}

fn occurrence(
    candidate: &CheckpointCandidate,
    index: usize,
) -> Result<CheckpointMessageOccurrence, String> {
    Ok(CheckpointMessageOccurrence {
        checkpoint_id: candidate.checkpoint_id.clone(),
        item_index: u32::try_from(index).map_err(|_| "checkpoint coordinate overflow")?,
    })
}

fn collect_calls(
    candidate: &CheckpointCandidate,
    messages: &[ModelInputMessage],
    originals: &BTreeMap<crate::runtime::identity::MessageId, MessageBlock>,
) -> Result<BTreeMap<CheckpointCallCoordinate, Call>, String> {
    let mut calls = BTreeMap::new();
    for (index, message) in messages.iter().enumerate() {
        let item = &candidate.items[index];
        let ModelMessageView::Assistant(content) = message.view() else {
            if !item.call_sources.is_empty() {
                return Err("call source belongs to a non-Assistant item".into());
            }
            continue;
        };
        let mut used_sources = BTreeSet::new();
        let mut batch_ids = BTreeSet::new();
        for (block, value) in content.iter().enumerate() {
            let AssistantContentBlock::ToolCall(call) = value else {
                continue;
            };
            if !batch_ids.insert(call.id.clone()) {
                return Err("duplicate wire ID inside projected batch".into());
            }
            let block =
                CheckpointBlockIndex(u32::try_from(block).map_err(|_| "projected block overflow")?);
            let source = match message {
                ModelInputMessage::Canonical(MessageBlock::Assistant(assistant)) => {
                    if !item.call_sources.is_empty() {
                        return Err("Original call has rewritten block coordinates".into());
                    }
                    ToolCallOccurrenceRef::new(
                        assistant.id.clone(),
                        ContentBlockIndex::new(block.0),
                    )
                }
                ModelInputMessage::Derived(derived) => {
                    let sources = item
                        .call_sources
                        .iter()
                        .filter(|source| source.block == block)
                        .collect::<Vec<_>>();
                    if sources.len() != 1 {
                        return Err("Derived call needs one explicit canonical block source".into());
                    }
                    let source = sources[0];
                    if !derived.sources.contains(&source.assistant) {
                        return Err("Derived call source is absent from scoped provenance".into());
                    }
                    let Some(MessageBlock::Assistant(assistant)) =
                        originals.get(&source.assistant.message_id)
                    else {
                        return Err("Derived call source is not Assistant".into());
                    };
                    let Some(AssistantContentBlock::ToolCall(native)) =
                        assistant.content.get(source.canonical_block.get() as usize)
                    else {
                        return Err("Derived call source is not a canonical ToolCall".into());
                    };
                    if native.tool_id != call.tool_id {
                        return Err("Derived call changes canonical Tool ownership".into());
                    }
                    used_sources.insert(block);
                    ToolCallOccurrenceRef::new(assistant.id.clone(), source.canonical_block)
                }
                _ => unreachable!("Assistant semantic view belongs to Assistant content"),
            };
            calls.insert(
                CheckpointCallCoordinate {
                    message: occurrence(candidate, index)?,
                    block,
                },
                Call {
                    source,
                    wire_id: call.id.clone(),
                    tool_id: call.tool_id.clone(),
                },
            );
        }
        if !item.call_sources.is_empty() && used_sources.len() != item.call_sources.len() {
            return Err("unused or duplicate Derived call source".into());
        }
    }
    Ok(calls)
}

fn pair_results(
    candidate: &CheckpointCandidate,
    messages: &[ModelInputMessage],
    calls: &BTreeMap<CheckpointCallCoordinate, Call>,
) -> Result<Vec<CompiledToolAssociation>, String> {
    let mut paired = BTreeSet::new();
    let mut tools = Vec::new();
    let mut pending = BTreeSet::new();
    for (index, message) in messages.iter().enumerate() {
        let item = &candidate.items[index];
        let here = occurrence(candidate, index)?;
        if let ModelMessageView::Tool(wire_id, _) = message.view() {
            let tool_id = match message {
                ModelInputMessage::Canonical(MessageBlock::Tool(tool)) => &tool.tool_id,
                ModelInputMessage::Derived(DerivedModelMessage {
                    message: ProjectedMessage::Tool { tool_id, .. },
                    ..
                }) => tool_id,
                _ => unreachable!("Tool view has typed Tool body"),
            };
            let coordinate = if let Some(target) = &item.result_call {
                target.clone()
            } else {
                let ModelInputMessage::Canonical(MessageBlock::Tool(tool)) = message else {
                    return Err("Derived result needs explicit projected call coordinate".into());
                };
                let owners = calls
                    .iter()
                    .filter(|(_, call)| call.source == tool.occurrence)
                    .map(|(coordinate, _)| coordinate)
                    .collect::<Vec<_>>();
                if owners.len() != 1 {
                    return Err("orphan or ambiguous Original ToolResult".into());
                }
                owners[0].clone()
            };
            let call = calls
                .get(&coordinate)
                .ok_or("orphan projected ToolResult")?;
            if coordinate.message.item_index >= here.item_index
                || !pending.remove(&coordinate)
                || !paired.insert(coordinate.clone())
            {
                return Err(
                    "projected result is duplicate or outside its complete Tool batch".into(),
                );
            }
            if let ModelInputMessage::Canonical(MessageBlock::Tool(tool)) = message
                && tool.occurrence != call.source
            {
                return Err("Original result maps to another physical ToolCall".into());
            }
            if tool_id != &call.tool_id || wire_id != &call.wire_id {
                return Err(
                    "projected ToolResult disagrees with call correlation or Tool ownership".into(),
                );
            }
            tools.push(CompiledToolAssociation {
                call: coordinate,
                result: here,
                canonical_source: call.source.clone(),
                wire_id: call.wire_id.clone(),
            });
        } else {
            if item.result_call.is_some() {
                return Err("result coordinate belongs to a non-Tool item".into());
            }
            if !pending.is_empty() {
                return Err("historical projection splits an unresolved Tool batch".into());
            }
            for coordinate in calls.keys().filter(|coordinate| coordinate.message == here) {
                pending.insert(coordinate.clone());
            }
        }
    }
    if paired.len() != calls.len() {
        return Err("historical projection contains orphan ToolCalls".into());
    }
    Ok(tools)
}

fn validate_native_reasoning(
    message: &ModelInputMessage,
    originals: &BTreeMap<crate::runtime::identity::MessageId, MessageBlock>,
) -> Result<(), String> {
    let ModelInputMessage::Derived(derived) = message else {
        return Ok(());
    };
    let ProjectedMessage::Assistant { content } = &derived.message else {
        return Ok(());
    };
    for reasoning in content.iter().filter_map(|block| {
        if let AssistantContentBlock::Reasoning(reasoning) = block {
            Some(reasoning)
        } else {
            None
        }
    }) {
        if reasoning.provider_state.is_none() {
            continue;
        }
        if matches!(
            reasoning.provider_state,
            Some(
                crate::runtime::continuation::ProviderContinuationState::OpenAiResponses(
                    crate::runtime::continuation::OpenAiResponsesContinuation::Stateless { .. }
                )
            )
        ) {
            let complete = derived.sources.iter().any(|source| matches!(originals.get(&source.message_id), Some(MessageBlock::Assistant(assistant)) if assistant.content == *content));
            if !complete {
                return Err("changed complete native reasoning/output replay unit".into());
            }
        }
        let preserved = derived.sources.iter().any(|reference| {
            matches!(originals.get(&reference.message_id), Some(MessageBlock::Assistant(assistant))
                if assistant.content.iter().any(|block| matches!(block, AssistantContentBlock::Reasoning(source) if source == reasoning)))
        });
        if !preserved {
            return Err("changed or invented native reasoning replay unit".into());
        }
    }
    Ok(())
}

/// Explicit frozen historical Tail, never inferred from retained items or provenance maxima.
pub fn eligible_tail(
    base: LedgerFrontier,
    request_cut: LedgerFrontier,
    messages: impl IntoIterator<Item = (LedgerFrontier, MessageBlock)>,
) -> Vec<MessageBlock> {
    messages
        .into_iter()
        .filter(|(position, _)| *position > base && *position <= request_cut)
        .map(|(_, message)| message)
        .collect()
}

/// Immutable source activation. Destination installation is a different coordinate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OriginActivation {
    pub conversation_id: ConversationId,
    pub activation: ProjectionActivationId,
    pub completed_journal: super::resources::JournalFrontier,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProjectionActivation {
    pub activation: ProjectionActivationId,
    pub ledger: LedgerFrontier,
    pub checkpoint_id: CheckpointId,
    pub origin: OriginActivation,
    /// None for a local activation; inherited mappings retain source order separately.
    pub destination_installation: Option<crate::runtime::identity::LineageInstallationId>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::content::TextBlock;
    use crate::message::types::{
        AssistantMessageBlock, InboundKind, ToolMessageBlock, UserContentBlock, UserMessageBlock,
        UserSource,
    };
    use crate::model::projected::SemanticTransformation;
    use crate::runtime::identity::MessageId;
    use crate::tools::types::{ToolCall, ToolExecutionResult};

    fn conversation() -> ConversationId {
        ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18")
    }
    fn candidate(items: Vec<CheckpointItem>) -> CheckpointCandidate {
        CheckpointCandidate {
            checkpoint_id: CheckpointId::new("checkpoint-one"),
            expected_head: None,
            read_cut: ResourceReadCut {
                projection_activation: ProjectionActivationId(0),
                conversation_id: conversation(),
                ledger: LedgerFrontier(100),
                journal: super::super::resources::JournalFrontier(110),
                transcript: crate::durable::TranscriptCursor::new(100),
            },
            base_frontier: LedgerFrontier(100),
            items,
            summary_metadata: None,
        }
    }
    fn canonical_item(message: &MessageBlock) -> CheckpointItem {
        CheckpointItem {
            message: CheckpointMessage::Original(CanonicalMessageRef::of(&conversation(), message)),
            call_sources: Vec::new(),
            result_call: None,
        }
    }
    fn derived_item(message: &MessageBlock) -> CheckpointItem {
        CheckpointItem {
            message: CheckpointMessage::Derived(Box::new(DerivedModelMessage {
                message: ProjectedMessage::from(message),
                sources: vec![CanonicalMessageRef::of(&conversation(), message)],
                checkpoint_sources: Vec::new(),
                transformations: vec![SemanticTransformation::Summary],
            })),
            call_sources: Vec::new(),
            result_call: None,
        }
    }
    fn call(id: &str) -> AssistantContentBlock {
        AssistantContentBlock::ToolCall(ToolCall {
            id: ToolCallId::new(id),
            tool_id: ToolId::new("tool-read"),
            name: "read".into(),
            arguments: serde_json::json!({"path":"file.rs"}),
        })
    }
    fn assistant(id: &str) -> MessageBlock {
        MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new(id),
            content: vec![
                AssistantContentBlock::Text(TextBlock {
                    text: "checking".into(),
                }),
                call("same-wire"),
                call("other-wire"),
            ],
        })
    }
    fn result(id: &str, owner: &str, block: u32, wire: &str) -> MessageBlock {
        MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new(id),
            occurrence: ToolCallOccurrenceRef::new(
                MessageId::new(owner),
                ContentBlockIndex::new(block),
            ),
            tool_call_id: ToolCallId::new(wire),
            tool_id: ToolId::new("tool-read"),
            result: ToolExecutionResult {
                workflow: None,
                status: crate::tools::types::ToolExecutionStatus::Success,
                content: vec![crate::tools::types::ToolResultContent::Text(TextBlock {
                    text: "read".into(),
                })],
                duration_ms: 0,
                exit_code: None,
                artifacts: Vec::new(),
                deliveries: Vec::new(),
                truncation: None,
                managed_output: None,
            },
        })
    }
    fn target(item: u32, block: u32) -> CheckpointCallCoordinate {
        CheckpointCallCoordinate {
            message: CheckpointMessageOccurrence {
                checkpoint_id: CheckpointId::new("checkpoint-one"),
                item_index: item,
            },
            block: CheckpointBlockIndex(block),
        }
    }
    fn compile(
        candidate: &CheckpointCandidate,
        sources: &[MessageBlock],
    ) -> Result<CompiledCheckpoint, String> {
        compile_checkpoint(candidate, |reference, _need_body| {
            sources
                .iter()
                .find(|message| message.id() == &reference.message_id)
                .cloned()
                .map(|message| CanonicalDependency {
                    frontier: LedgerFrontier(90),
                    reference: CanonicalMessageRef::of(&reference.conversation_id, &message),
                    body: Some(message),
                })
                .ok_or_else(|| "missing immutable source".into())
        })
    }

    #[test]
    fn all_four_tool_pairings_preserve_batch_and_distinct_block_coordinates() {
        let assistant = assistant("a");
        let first = result("r1", "a", 1, "same-wire");
        let second = result("r2", "a", 2, "other-wire");
        for derived_call in [false, true] {
            for derived_result in [false, true] {
                let mut call_item = if derived_call {
                    derived_item(&assistant)
                } else {
                    canonical_item(&assistant)
                };
                let (first_block, second_block) = if derived_call {
                    let CheckpointMessage::Derived(body) = &mut call_item.message else {
                        unreachable!()
                    };
                    let ProjectedMessage::Assistant { content } = &mut body.message else {
                        unreachable!()
                    };
                    content.swap(0, 2); // canonical block 2 is now projected block 0.
                    call_item.call_sources = vec![
                        ProjectedCallSource {
                            block: CheckpointBlockIndex(1),
                            assistant: CanonicalMessageRef::of(&conversation(), &assistant),
                            canonical_block: ContentBlockIndex::new(1),
                        },
                        ProjectedCallSource {
                            block: CheckpointBlockIndex(0),
                            assistant: CanonicalMessageRef::of(&conversation(), &assistant),
                            canonical_block: ContentBlockIndex::new(2),
                        },
                    ];
                    (1, 0)
                } else {
                    (1, 2)
                };
                let mut first_item = if derived_result {
                    derived_item(&first)
                } else {
                    canonical_item(&first)
                };
                let mut second_item = if derived_result {
                    derived_item(&second)
                } else {
                    canonical_item(&second)
                };
                if derived_result {
                    first_item.result_call = Some(target(0, first_block));
                    second_item.result_call = Some(target(0, second_block));
                }
                // Results may reorder within a complete legitimate batch.
                let projection = candidate(vec![call_item, second_item, first_item]);
                let compiled = compile(
                    &projection,
                    &[assistant.clone(), first.clone(), second.clone()],
                )
                .unwrap();
                assert_eq!(compiled.tools.len(), 2);
                assert_eq!(
                    compiled.tools[0].call.block,
                    CheckpointBlockIndex(second_block)
                );
                assert_eq!(
                    compiled.tools[0].canonical_source.block_index,
                    ContentBlockIndex::new(2)
                );
                assert_eq!(
                    compiled.tools[1].canonical_source.block_index,
                    ContentBlockIndex::new(1)
                );
                if derived_call {
                    assert!(compiled.messages[0].canonical_id().is_none());
                }
            }
        }
    }

    #[test]
    fn repeated_wire_strings_are_separate_occurrences_and_ambiguous_copies_fail() {
        let a1 = assistant("a1");
        let a2 = assistant("a2");
        let r1 = result("r1", "a1", 1, "same-wire");
        let r2 = result("r2", "a1", 2, "other-wire");
        let r3 = result("r3", "a2", 1, "same-wire");
        let r4 = result("r4", "a2", 2, "other-wire");
        let sources = vec![
            a1.clone(),
            r1.clone(),
            r2.clone(),
            a2.clone(),
            r3.clone(),
            r4.clone(),
        ];
        let projection = candidate(sources.iter().map(canonical_item).collect());
        let compiled = compile(&projection, &sources).unwrap();
        assert_eq!(compiled.tools[0].wire_id, compiled.tools[2].wire_id);
        assert_ne!(compiled.tools[0].call, compiled.tools[2].call);
        assert_ne!(
            compiled.tools[0].canonical_source,
            compiled.tools[2].canonical_source
        );
        let orphan = candidate(vec![canonical_item(&r1)]);
        assert!(compile(&orphan, &sources).unwrap_err().contains("orphan"));
        let ambiguous = candidate(vec![
            canonical_item(&a1),
            canonical_item(&r1),
            canonical_item(&r2),
            canonical_item(&a1),
            canonical_item(&r1),
            canonical_item(&r2),
        ]);
        assert!(
            compile(&ambiguous, &sources)
                .unwrap_err()
                .contains("ambiguous")
        );
        let split = candidate(vec![
            canonical_item(&a1),
            canonical_item(&a2),
            canonical_item(&r1),
            canonical_item(&r2),
            canonical_item(&r3),
            canonical_item(&r4),
        ]);
        assert!(compile(&split, &sources).unwrap_err().contains("splits"));
    }

    #[test]
    fn checkpoint_tail_uses_complete_base_decision_not_last_original_or_provenance() {
        let messages = (1..=101)
            .map(|n| {
                (
                    LedgerFrontier(n),
                    MessageBlock::User(UserMessageBlock {
                        id: MessageId::new(format!("m{n}")),
                        content: vec![UserContentBlock::Text(TextBlock {
                            text: format!("message {n}"),
                        })],
                        source: UserSource::Human,
                        kind: InboundKind::Message,
                        timestamp: None,
                    }),
                )
            })
            .collect::<Vec<_>>();
        let p = candidate(vec![
            derived_item(&messages[69].1),
            canonical_item(&messages[89].1),
            canonical_item(&messages[94].1),
        ]);
        let tail = eligible_tail(p.base_frontier, LedgerFrontier(101), messages);
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].id(), &MessageId::new("m101"));
        assert_eq!(p.base_frontier, LedgerFrontier(100));
    }

    #[test]
    fn changed_native_reasoning_does_not_inherit_signed_or_encrypted_state() {
        use crate::message::types::ReasoningBlock;
        use crate::runtime::continuation::{
            AnthropicContinuation, OpenAiResponsesContinuation, ProviderContinuationState,
        };
        for native in [
            ProviderContinuationState::Anthropic(AnthropicContinuation {
                opaque: serde_json::json!({"signature":"signed"}),
            }),
            ProviderContinuationState::OpenAiResponses(OpenAiResponsesContinuation::Stateless {
                items: vec![serde_json::json!({"type":"reasoning","encrypted_content":"opaque"})],
            }),
        ] {
            let source = MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("reasoning-source"),
                content: vec![AssistantContentBlock::Reasoning(ReasoningBlock {
                    text: Some("original thinking".into()),
                    provider_state: Some(native),
                })],
            });
            let mut p = candidate(vec![derived_item(&source)]);
            compile(&p, std::slice::from_ref(&source)).unwrap();
            let CheckpointMessage::Derived(body) = &mut p.items[0].message else {
                unreachable!()
            };
            let ProjectedMessage::Assistant { content } = &mut body.message else {
                unreachable!()
            };
            let AssistantContentBlock::Reasoning(reasoning) = &mut content[0] else {
                unreachable!()
            };
            reasoning.text = Some("edited thinking".into());
            assert!(
                compile(&p, std::slice::from_ref(&source))
                    .unwrap_err()
                    .contains("native reasoning")
            );
        }
    }
    #[test]
    fn historical_derived_requires_a_real_source_even_with_summary_label() {
        let user = MessageBlock::User(UserMessageBlock {
            id: MessageId::new("u"),
            content: vec![],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        });
        for original in [user, assistant("a"), result("r", "a", 1, "same-wire")] {
            for label in [false, true] {
                let mut item = derived_item(&original);
                let CheckpointMessage::Derived(body) = &mut item.message else {
                    unreachable!()
                };
                body.sources.clear();
                if !label {
                    body.transformations.clear();
                }
                let error = compile(&candidate(vec![item]), &[]).unwrap_err();
                assert!(error.contains("immutable provenance"), "{error}");
            }
        }
    }
    #[test]
    fn historical_derived_compiler_accepts_canonical_and_parent_only_sources_but_rejects_wrong_scope()
     {
        use crate::model::projected::CheckpointSourceRef;
        let user = MessageBlock::User(UserMessageBlock {
            id: MessageId::new("u"),
            content: vec![],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        });
        let canonical = candidate(vec![derived_item(&user)]);
        assert!(compile(&canonical, std::slice::from_ref(&user)).is_ok());
        let mut parent_only = canonical;
        let CheckpointMessage::Derived(body) = &mut parent_only.items[0].message else {
            unreachable!()
        };
        body.sources.clear();
        body.checkpoint_sources.push(CheckpointSourceRef {
            conversation_id: conversation(),
            checkpoint_id: CheckpointId::new("parent"),
            content_digest: "checked by durable owner".into(),
        });
        assert!(compile(&parent_only, &[]).is_ok());
        for foreign in [true, false] {
            let mut invalid = parent_only.clone();
            let CheckpointMessage::Derived(body) = &mut invalid.items[0].message else {
                unreachable!()
            };
            if foreign {
                body.checkpoint_sources[0].conversation_id = ConversationId::generate();
            } else {
                body.checkpoint_sources[0].checkpoint_id = invalid.checkpoint_id.clone();
            }
            assert!(
                compile(&invalid, &[])
                    .unwrap_err()
                    .contains("checkpoint provenance")
            );
        }
    }
}
