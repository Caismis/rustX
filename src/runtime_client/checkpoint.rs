//! Bounded native observation cuts. Capture copies finite display descriptors;
//! source-owned bytes are materialized only after releasing the Projection lock.
use super::{content::ContentRef, snapshot::RuntimeClientSnapshot, types::RuntimeClientCursor};
use serde::{Deserialize, Serialize, de::DeserializeOwned};

pub(crate) mod metadata_fragment;
pub use metadata_fragment::MetadataFragment;
mod prepared_trace;
pub(super) use prepared_trace::PreparedTraceHead;
pub(crate) mod prepared_transcript;
pub(crate) use prepared_transcript::{PreparedTranscriptHead, PreparedTranscriptRow};

pub const CHECKPOINT_TARGET_BYTES: usize = 786_432;
pub const CHECKPOINT_PAGE_ITEMS: usize = 32;
/// Each indexed native row includes bodies and locator metadata within16KiB.
/// Separate8-row current heads reserve256KiB before other domains/annotations.
pub(crate) const CHECKPOINT_NATIVE_HEAD_ITEMS: usize = 8;
pub const CHECKPOINT_ITEM_BYTES: usize = 16_384;
pub const CHECKPOINT_ATTEMPTS: usize = 2;
pub const CHECKPOINT_MATERIALIZATION_SECONDS: u64 = 5;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LivePublicationReadCut {
    pub source: super::content::ContentSource,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub revision: u64,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub block_total: u64,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub tool_total: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CanonicalForegroundReadCut {
    pub locator: super::content::CanonicalContentLocator,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub total: u64,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckpointReadCut {
    pub canonical_foreground: Option<CanonicalForegroundReadCut>,
    pub live_publication: Option<LivePublicationReadCut>,
    pub conversation_id: crate::runtime::identity::ConversationId,
    pub activation_id: Option<crate::runtime::identity::SubagentId>,
    pub revision: u64,
    pub through: u64,
    /// Immutable durable descriptor frontier captured with this display cut.
    pub source_cut: Option<crate::durable::observation::ObservationReadCut>,
    pub domain_revisions: std::collections::BTreeMap<String, u64>,
    pub collection_cuts:
        std::collections::BTreeMap<String, crate::runtime::identity::ObservationCollectionCut>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContentBinding {
    /// Projection-owned structural selection inside `CanonicalJsonV1` bytes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_path: Option<Vec<String>>,
    pub path: Vec<String>,
    pub content_ref: ContentRef,
    pub deferred: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CollectionFrontier {
    pub domain: String,
    pub revision: u64,
    pub total: u64,
    pub loaded: u64,
    pub next_offset: Option<u64>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ProjectionMetadata {
    pub read_cut: CheckpointReadCut,
    pub collections: Vec<MetadataFragment<CollectionFrontier>>,
    pub content: Vec<MetadataFragment<ContentBinding>>,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientCheckpoint {
    pub snapshot: RuntimeClientSnapshot,
    pub cursor: RuntimeClientCursor,
    pub read_cut: CheckpointReadCut,
    pub content: Vec<ContentBinding>,
    pub collections: Vec<CollectionFrontier>,
}
/// Final complete checkpoint budget, after off-lock source enrichment. Only
/// loaded read caches are evicted; authoritative cuts, descriptors, current
/// execution and interaction authority remain in the candidate.
pub(crate) fn fit_checkpoint(
    checkpoint: &mut RuntimeClientCheckpoint,
) -> Result<bool, super::types::RuntimeClientError> {
    fit_checkpoint_with_budget(checkpoint, None)
}

pub(super) fn sort_metadata(
    collections: &mut [CollectionFrontier],
    content: &mut [ContentBinding],
) {
    collections.sort_by(|left, right| left.domain.cmp(&right.domain));
    content.sort_by(|left, right| left.path.cmp(&right.path));
}

pub(super) fn certified_checkpoint_fit(budget: Option<usize>) -> bool {
    budget.is_some_and(|budget| budget <= CHECKPOINT_TARGET_BYTES - CHECKPOINT_ITEM_BYTES)
}

pub(super) fn fit_checkpoint_with_budget(
    checkpoint: &mut RuntimeClientCheckpoint,
    prepared_budget: Option<usize>,
) -> Result<bool, super::types::RuntimeClientError> {
    fn fits(checkpoint: &RuntimeClientCheckpoint) -> bool {
        #[cfg(test)]
        record_capture_work(3);
        let limit = CHECKPOINT_TARGET_BYTES - CHECKPOINT_ITEM_BYTES;
        // The raw admission charges 24 bytes per number. Safe integers can
        // gain at most two quotes in the structural codec (at most 18 bytes),
        // with every other token unchanged. Ambiguous keys and wide native
        // quantities still require the existing structural admission below.
        if super::preflight::bounded_safe_integer_fit(checkpoint, limit) == Some(true) {
            return true;
        }
        let schema = super::structural_json::root::<RuntimeClientCheckpoint>();
        let encoded = super::structural_json::Encoded {
            value: checkpoint,
            schema: &schema,
            root: &schema,
        };
        super::preflight::bounded_fit(&encoded, limit)
            .unwrap_or_else(|| bounded_bytes(&encoded, limit).is_some())
    }
    fn loaded(
        checkpoint: &mut RuntimeClientCheckpoint,
        domain: &str,
        count: u64,
        next: Option<u64>,
    ) {
        if let Some(frontier) = checkpoint
            .collections
            .iter_mut()
            .find(|frontier| frontier.domain == domain)
        {
            frontier.loaded = count;
            frontier.next_offset = next;
        }
    }
    sort_metadata(&mut checkpoint.collections, &mut checkpoint.content);
    if certified_checkpoint_fit(prepared_budget) || fits(checkpoint) {
        return Ok(false);
    }
    defer_pending_bodies(checkpoint);
    if fits(checkpoint) {
        return Ok(true);
    }
    checkpoint.snapshot.trace.records.clear();
    checkpoint.snapshot.trace.updates.clear();
    for domain in ["trace.records", "trace.updates"] {
        loaded(checkpoint, domain, 0, Some(0));
    }
    if fits(checkpoint) {
        return Ok(true);
    }
    checkpoint.snapshot.messages.clear();
    checkpoint.content.retain(|binding| {
        binding
            .path
            .first()
            .is_none_or(|domain| domain != "messages")
    });
    loaded(checkpoint, "messages", 0, Some(0));
    if fits(checkpoint) {
        return Ok(true);
    }
    checkpoint.snapshot.transcript.entries.clear();
    checkpoint.snapshot.transcript.content.retain(|binding| {
        binding
            .path
            .first()
            .is_none_or(|domain| domain != "entries")
    });
    checkpoint
        .snapshot
        .transcript
        .collections
        .retain(|frontier| !frontier.domain.starts_with("transcript.entries."));
    checkpoint.content.retain(|binding| {
        binding
            .path
            .first()
            .is_none_or(|domain| domain != "transcript")
            || binding.path.get(1).is_none_or(|domain| domain != "entries")
    });
    // Zero means the newest page at this captured cut. A positive offset is
    // an exclusive transcript cursor and would skip the newest evicted item.
    loaded(checkpoint, "transcript.entries", 0, Some(0));
    if fits(checkpoint) {
        return Ok(true);
    }
    Err(super::types::RuntimeClientError::ObservationUnavailable {
        message: "finite authoritative checkpoint descriptors exceed the native checkpoint budget"
            .into(),
    })
}

/// Evict optional body caches while retaining every native decision header.
fn defer_pending_bodies(checkpoint: &mut RuntimeClientCheckpoint) {
    // Headers keep every actionable identity and native source ordinal. Only
    // the optional loaded request body is evicted; the exact captured domain
    // remains readable with the same interaction/cut evidence.
    let mut deferred = Vec::new();
    for (index, interaction) in checkpoint
        .snapshot
        .pending_interactions
        .iter_mut()
        .enumerate()
    {
        if interaction.body.take().is_some() {
            interaction.body_complete = false;
            deferred.push(format!("pending_interactions.{index}.body"));
        }
    }
    for domain in deferred {
        if let Some(frontier) = checkpoint
            .collections
            .iter_mut()
            .find(|frontier| frontier.domain == domain)
        {
            frontier.loaded = 0;
            frontier.next_offset = Some(0);
        } else {
            checkpoint.collections.push(CollectionFrontier {
                domain,
                revision: checkpoint
                    .read_cut
                    .domain_revisions
                    .get("pending_interactions")
                    .copied()
                    .unwrap_or(checkpoint.read_cut.revision),
                total: 1,
                loaded: 0,
                next_offset: Some(0),
            });
        }
    }
    checkpoint
        .collections
        .sort_by(|left, right| left.domain.cmp(&right.domain));
    for binding in &mut checkpoint.content {
        if binding
            .path
            .first()
            .is_some_and(|domain| domain == "pending_interactions")
            && binding.path.get(2).is_some_and(|field| field == "body")
        {
            binding.deferred = true;
        }
    }
}

/// Original JSON key evidence for a field selected by a bounded ordinal locator.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckpointKey {
    pub path: Vec<String>,
    pub preview: String,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub encoded_bytes: u64,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckpointPageItem {
    /// Whether a finite actionable summary includes its complete request body.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_complete: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key: Option<CheckpointKey>,
    pub offset: u64,
    pub identity: String,
    pub value: Option<serde_json::Value>,
    pub content: Vec<ContentBinding>,
    pub deferred: bool,
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckpointPage {
    /// Nested advancing domain frontiers represented by this exact page cut.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub collections: Vec<CollectionFrontier>,
    pub cursor: RuntimeClientCursor,
    pub read_cut: CheckpointReadCut,
    pub domain: String,
    pub total: u64,
    pub items: Vec<CheckpointPageItem>,
    pub next_offset: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum CheckpointValueRepresentation {
    Utf8Text,
    CanonicalJsonV1,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CheckpointValueChunk {
    pub representation: CheckpointValueRepresentation,
    pub cut: CheckpointReadCut,
    pub domain: String,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub item_offset: u64,
    pub path: Vec<String>,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub offset: u64,
    pub bytes_base64: String,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub next_offset: Option<u64>,
    #[schemars(range(max = 9_007_199_254_740_991_u64))]
    pub readable_end: u64,
}

struct Buffer {
    bytes: Vec<u8>,
    limit: usize,
}
impl std::io::Write for Buffer {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("bounded display item"));
        }
        let required = self.bytes.len() + bytes.len();
        if required > self.bytes.capacity() {
            let capacity = self
                .bytes
                .capacity()
                .max(8)
                .saturating_mul(2)
                .max(required)
                .min(self.limit);
            self.bytes.reserve_exact(capacity - self.bytes.len());
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// A capped serializer bounds both encoded length and buffer capacity.
/// The allocation depends only on the selected display budget, not source size.
pub(crate) fn bounded_bytes<T: Serialize + ?Sized>(value: &T, limit: usize) -> Option<Vec<u8>> {
    if !super::preflight::fits(value, limit) {
        return None;
    }
    encode_admitted_bytes(value, limit)
}

fn encode_admitted_bytes<T: Serialize + ?Sized>(value: &T, limit: usize) -> Option<Vec<u8>> {
    let mut buffer = Buffer {
        bytes: Vec::new(),
        limit,
    };
    serde_json::to_writer(&mut buffer, value).ok()?;
    Some(buffer.bytes)
}

struct Length {
    bytes: usize,
    limit: usize,
}
impl std::io::Write for Length {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes) {
            return Err(std::io::Error::other("bounded display item"));
        }
        self.bytes += bytes.len();
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// The same native admission and exact JSON length as `bounded_bytes`, without
/// retaining a byte buffer when the caller only needs its length.
pub(crate) fn bounded_len<T: Serialize + ?Sized>(value: &T, limit: usize) -> Option<usize> {
    if !super::preflight::fits(value, limit) {
        return None;
    }
    let mut length = Length { bytes: 0, limit };
    serde_json::to_writer(&mut length, value).ok()?;
    Some(length.bytes)
}

/// The exact structural wire length when the caller retains the typed value,
/// not its encoded bytes. A positive raw admission avoids repeating the schema
/// walk; wide native quantities and uncertain keys keep structural admission.
/// The final schema-aware serializer still validates every encoded value.
#[cfg(test)]
pub(crate) fn bounded_structural_len<T: Serialize + schemars::JsonSchema>(
    value: &T,
    limit: usize,
) -> Option<usize> {
    bounded_structural_len_with_budget(value, limit, None)
}

/// An assembled admission certificate avoids another traversal, but never
/// replaces exact schema-aware length or native quantity validation.
pub(super) fn bounded_structural_len_with_budget<T: Serialize + schemars::JsonSchema>(
    value: &T,
    limit: usize,
    prepared_budget: Option<usize>,
) -> Option<usize> {
    let schema = super::structural_json::root::<T>();
    let encoded = super::structural_json::Encoded {
        value,
        schema: &schema,
        root: &schema,
    };
    if prepared_budget.is_none_or(|budget| budget > limit) {
        #[cfg(test)]
        record_capture_work(5);
        if super::preflight::bounded_safe_integer_fit(value, limit) != Some(true)
            && !super::preflight::fits(&encoded, limit)
        {
            return None;
        }
    }
    let mut length = Length { bytes: 0, limit };
    serde_json::to_writer(&mut length, &encoded).ok()?;
    Some(length.bytes)
}
/// Decodes one source-typed native domain value, preserving exact native u64 fields.
/// Opaque JSON fields retain their original JSON semantics.
/// # Errors
/// Returns the native DTO's validation or shape error.
pub fn decode_domain_value<T: serde::de::DeserializeOwned + schemars::JsonSchema>(
    value: serde_json::Value,
) -> serde_json::Result<T> {
    super::structural_json::decode(value)
}

pub(crate) fn structural_value<T: serde::Serialize + schemars::JsonSchema>(
    value: &T,
) -> Option<serde_json::Value> {
    let schema = super::structural_json::root::<T>();
    let encoded = super::structural_json::Encoded {
        value,
        schema: &schema,
        root: &schema,
    };
    bounded_item(&encoded, CHECKPOINT_ITEM_BYTES).map(|(_, value)| value)
}

pub(crate) fn bounded_item_bytes(value: &impl Serialize, limit: usize) -> Option<Vec<u8>> {
    bounded_item(value, limit).map(|(bytes, _)| bytes)
}

// Admission already parses the capped bytes to check every container's item
// count. Callers needing JSON retain that same value instead of parsing twice.
fn bounded_item(value: &impl Serialize, limit: usize) -> Option<(Vec<u8>, serde_json::Value)> {
    if !super::preflight::fits(value, limit) {
        return None;
    }
    bounded_item_admitted(value, limit)
}

// The caller just admitted this same immutable value. Only that repeated walk
// is omitted: the capped writer, JSON parse and every container limit remain.
fn bounded_item_admitted(
    value: &impl Serialize,
    limit: usize,
) -> Option<(Vec<u8>, serde_json::Value)> {
    fn bounded(value: &serde_json::Value) -> bool {
        match value {
            serde_json::Value::Array(items) => {
                items.len() <= CHECKPOINT_PAGE_ITEMS && items.iter().all(bounded)
            }
            serde_json::Value::Object(items) => {
                items.len() <= CHECKPOINT_PAGE_ITEMS && items.values().all(bounded)
            }
            _ => true,
        }
    }
    #[cfg(test)]
    record_item_work(0);
    let bytes = encode_admitted_bytes(value, limit)?;
    #[cfg(test)]
    record_item_work(1);
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    bounded(&value).then_some((bytes, value))
}
/// One immutable DTO's existing admission and normalization, prepared once by
/// its bounded native header owner. Both thresholds preserve later admission.
pub(super) fn prepare_capture_item<T: Serialize + DeserializeOwned>(
    item: &T,
) -> Option<(T, usize, usize)> {
    let required = super::preflight::required_budget(item, CHECKPOINT_ITEM_BYTES)?;
    let (bytes, value) = bounded_item_admitted(item, CHECKPOINT_ITEM_BYTES)?;
    let encoded = bytes.len();
    Some((
        serde_json::from_value(value).ok()?,
        encoded,
        required.max(encoded),
    ))
}

pub(crate) fn bounded_clone<T: Serialize + DeserializeOwned>(value: &T, limit: usize) -> Option<T> {
    let bytes = bounded_item_bytes(value, limit)?;
    #[cfg(test)]
    record_item_work(2);
    serde_json::from_slice(&bytes).ok()
}

/// Opaque JSON is never rewritten with the typed protocol's integer codec.
/// A browser-unsafe integer is deferred to its exact authoritative UTF-8 bytes.
pub(crate) fn bounded_json_value(
    value: &serde_json::Value,
    limit: usize,
) -> Option<serde_json::Value> {
    fn safe(value: &serde_json::Value, remaining: &mut usize) -> bool {
        if *remaining == 0 {
            return false;
        }
        *remaining -= 1;
        match value {
            serde_json::Value::Number(number) => {
                number
                    .as_u64()
                    .is_none_or(|value| value <= 9_007_199_254_740_991)
                    && number
                        .as_i64()
                        .is_none_or(|value| value.unsigned_abs() <= 9_007_199_254_740_991)
            }
            serde_json::Value::Array(values) => {
                values.len() <= CHECKPOINT_PAGE_ITEMS
                    && values.iter().all(|value| safe(value, remaining))
            }
            serde_json::Value::Object(values) => {
                values.len() <= CHECKPOINT_PAGE_ITEMS
                    && values.values().all(|value| safe(value, remaining))
            }
            _ => true,
        }
    }
    safe(value, &mut 512)
        .then(|| bounded_clone(value, limit))
        .flatten()
}
fn message_json_safe(message: &crate::message::types::MessageBlock) -> bool {
    use crate::message::types::{AssistantContentBlock, MessageBlock};
    match message {
        MessageBlock::Assistant(message) => {
            message
                .content
                .iter()
                .take(CHECKPOINT_PAGE_ITEMS)
                .all(|block| match block {
                    AssistantContentBlock::ToolCall(call) => {
                        bounded_json_value(&call.arguments, CHECKPOINT_ITEM_BYTES).is_some()
                    }
                    _ => true,
                })
        }
        MessageBlock::Tool(message) => message
            .result
            .content
            .iter()
            .take(CHECKPOINT_PAGE_ITEMS)
            .all(|block| match block {
                crate::tools::types::ToolResultContent::Json { value } => {
                    bounded_json_value(value, CHECKPOINT_ITEM_BYTES).is_some()
                }
                _ => true,
            }),
        MessageBlock::User(_) => true,
    }
}

fn bounded_inbound_kind(
    kind: &crate::message::types::InboundKind,
) -> crate::message::types::InboundKind {
    use crate::message::types::{ContextKind, InboundKind};
    match kind {
        InboundKind::Context(ContextKind::GoalStatus(goal)) => InboundKind::Context(
            ContextKind::GoalStatus(Box::new(crate::goal::GoalSnapshot {
                reference: goal.reference.clone(),
                objective: goal.objective.chars().take(256).collect(),
                phase: goal.phase,
                blocked_reason: goal
                    .blocked_reason
                    .as_ref()
                    .map(|reason| reason.chars().take(256).collect()),
                autonomous_round_budget: goal.autonomous_round_budget,
                autonomous_rounds_consumed: goal.autonomous_rounds_consumed,
                origin: goal.origin.clone(),
                last_round_message_id: goal.last_round_message_id.clone(),
            })),
        ),
        InboundKind::Message | InboundKind::GoalContinuation(_) | InboundKind::Context(_) => {
            kind.clone()
        }
    }
}

#[cfg(test)]
thread_local! {
    static CAPTURE_WORK: std::cell::Cell<[u64; 6]> = const { std::cell::Cell::new([0; 6]) };
    // Capped JSON encodes, structural byte parses, typed byte parses.
    static ITEM_WORK: std::cell::Cell<[u64; 3]> = const { std::cell::Cell::new([0; 3]) };
    static PREPARED_FIT_WORK: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}
#[cfg(test)]
fn record_item_work(index: usize) {
    ITEM_WORK.with(|counts| {
        let mut value = counts.get();
        value[index] += 1;
        counts.set(value);
    });
}
#[cfg(test)]
pub(super) fn take_item_work() -> [u64; 3] {
    ITEM_WORK.with(|counts| counts.replace([0; 3]))
}
#[cfg(test)]
pub(super) fn take_prepared_fit_work() -> u64 {
    PREPARED_FIT_WORK.with(|work| work.replace(0))
}
#[cfg(test)]
pub(super) fn record_capture_work(index: usize) {
    CAPTURE_WORK.with(|counts| {
        let mut value = counts.get();
        value[index] += 1;
        counts.set(value);
    });
}
#[cfg(test)]
pub(crate) fn take_capture_work() -> [u64; 6] {
    CAPTURE_WORK.with(|counts| counts.replace([0; 6]))
}

pub(super) struct PreparedCapture<T> {
    value: T,
    capture: Capture,
    fit: Option<usize>,
}

pub(crate) struct Capture {
    pub collections: Vec<CollectionFrontier>,
    pub content: Vec<MetadataFragment<ContentBinding>>,
    pub revision: u64,
    remaining: usize,
    required: usize,
    pub(crate) evicted: bool,
}
impl Capture {
    #[cfg(test)]
    pub(super) fn admission_state(&self) -> (usize, usize, bool) {
        (self.remaining, self.required, self.evicted)
    }

    #[cfg(test)]
    pub(super) fn with_remaining(revision: u64, remaining: usize) -> Self {
        assert!(remaining <= CHECKPOINT_TARGET_BYTES / 8);
        Self {
            remaining,
            ..Self::new(revision)
        }
    }

    pub fn new(revision: u64) -> Self {
        Self {
            collections: Vec::new(),
            content: Vec::new(),
            revision,
            remaining: CHECKPOINT_TARGET_BYTES / 8,
            required: 0,
            evicted: false,
        }
    }
    fn item_budget(&mut self) -> usize {
        let limit = CHECKPOINT_ITEM_BYTES.min(self.remaining);
        self.required = self
            .required
            .max(CHECKPOINT_TARGET_BYTES / 8 - self.remaining + limit);
        limit
    }

    /// Replay one immutable capture only while its original admission decisions
    /// have enough budget. Borderline heads execute their ordinary capture.
    pub(super) fn prepared<'a, T: Clone + Serialize + Send + Sync + 'static>(
        &mut self,
        owner: &'a std::sync::OnceLock<PreparedCapture<T>>,
        build: impl Fn(&mut Self) -> T,
    ) -> (std::borrow::Cow<'a, T>, Option<usize>) {
        let prepared = owner.get_or_init(|| {
            #[cfg(test)]
            record_capture_work(0);
            let mut capture = Self::new(0);
            let value = build(&mut capture);
            #[cfg(test)]
            PREPARED_FIT_WORK.with(|work| work.set(work.get() + 1));
            let fit =
                super::preflight::required_safe_integer_budget(&value, CHECKPOINT_TARGET_BYTES);
            PreparedCapture {
                value,
                capture,
                fit,
            }
        });
        if !self.try_replay(&prepared.capture) {
            #[cfg(test)]
            record_capture_work(2);
            let value = build(self);
            #[cfg(test)]
            PREPARED_FIT_WORK.with(|work| work.set(work.get() + 1));
            let fit =
                super::preflight::required_safe_integer_budget(&value, CHECKPOINT_TARGET_BYTES);
            return (std::borrow::Cow::Owned(value), fit);
        }
        #[cfg(test)]
        record_capture_work(1);
        (std::borrow::Cow::Borrowed(&prepared.value), prepared.fit)
    }
    /// Reuse admission and generated metadata only when the complete original
    /// decision sequence fits. A rejected certificate leaves this capture intact.
    pub(super) fn try_replay(&mut self, prepared: &Self) -> bool {
        if self.remaining < prepared.required {
            return false;
        }
        // Zero means the original capture never requested item admission.
        // Its metadata alone must not add a requirement at the current offset.
        if prepared.required != 0 {
            self.required = self
                .required
                .max(CHECKPOINT_TARGET_BYTES / 8 - self.remaining + prepared.required);
        }
        self.remaining -= CHECKPOINT_TARGET_BYTES / 8 - prepared.remaining;
        self.evicted |= prepared.evicted;
        self.content.extend(prepared.content.iter().cloned());
        self.collections
            .extend(prepared.collections.iter().cloned().map(|mut frontier| {
                frontier.revision = self.revision;
                frontier
            }));
        true
    }
    /// Admission for an already normalized immutable display row. The
    /// certificate belongs to that exact row, never a revision-only lookup.
    pub(super) fn admit_prepared(&mut self, encoded: usize, required: usize) -> bool {
        if required > self.item_budget() || encoded > required {
            self.evicted = true;
            return false;
        }
        self.remaining -= encoded;
        true
    }
    /// Live blocks are finite DTOs: arguments are strings, never lexical JSON
    /// carriers. Their normalized form has the same admission charge, so one
    /// capped encoding supplies both item admission and the shared-body debit.
    /// A first-stage rejection still leaves shared eviction state untouched.
    pub(super) fn live_block(
        &mut self,
        block: &super::snapshot::InFlightBlock,
    ) -> Option<super::snapshot::InFlightBlock> {
        let (value, encoded, required) = prepare_capture_item(block)?;
        self.admit_prepared(encoded, required).then_some(value)
    }
    pub fn reserve(&mut self, item: &impl Serialize) -> bool {
        let Some(bytes) = bounded_item_bytes(item, self.item_budget()) else {
            self.evicted = true;
            return false;
        };
        self.remaining -= bytes.len();
        true
    }
    /// Finite display DTOs only. Lexical JSON carriers use `bounded_clone`
    /// instead: their deserializer requires the original capped JSON bytes.
    pub fn item<T: Serialize + DeserializeOwned>(&mut self, domain: &str, item: &T) -> Option<T> {
        let value = bounded_item(item, self.item_budget());
        self.evicted |= value.is_none();
        let loaded = u64::from(value.is_some());
        self.frontier(domain, 1, loaded, if loaded == 1 { None } else { Some(0) });
        let (bytes, value) = value?;
        self.remaining -= bytes.len();
        serde_json::from_value(value).ok()
    }
    /// Copies only the admitted DTO head; raw lexical JSON is not a DTO page.
    pub fn page<T: Serialize + DeserializeOwned>(&mut self, domain: &str, items: &[T]) -> Vec<T> {
        let mut result = Vec::new();
        let mut advanced = 0;
        for item in items.iter().take(CHECKPOINT_PAGE_ITEMS) {
            let Some((bytes, value)) = bounded_item(item, self.item_budget()) else {
                self.evicted = true;
                break;
            };
            self.remaining -= bytes.len();
            result.push(serde_json::from_value(value).expect("serialized native display value"));
            advanced += 1;
        }
        self.frontier(
            domain,
            items.len() as u64,
            result.len() as u64,
            (advanced < items.len()).then_some(advanced as u64),
        );
        result
    }
    pub fn frontier(&mut self, domain: &str, total: u64, loaded: u64, next_offset: Option<u64>) {
        self.collections.push(CollectionFrontier {
            domain: domain.into(),
            revision: self.revision,
            total,
            loaded,
            next_offset,
        });
    }
}

impl Capture {
    pub fn message_page(
        &mut self,
        domain: &str,
        messages: &[crate::message::types::MessageBlock],
        conversation: &crate::runtime::identity::ConversationId,
    ) -> Vec<crate::message::types::MessageBlock> {
        let mut result = Vec::new();
        for (index, message) in messages.iter().take(CHECKPOINT_PAGE_ITEMS).enumerate() {
            let before = self.content.len();
            let projected =
                self.message(message, conversation, &[domain.into(), index.to_string()]);
            let Some(length) = bounded_len(&projected, self.item_budget()) else {
                self.content.truncate(before);
                break;
            };
            self.remaining -= length;
            result.push(projected);
        }
        self.frontier(
            domain,
            messages.len() as u64,
            result.len() as u64,
            (result.len() < messages.len()).then_some(result.len() as u64),
        );
        result
    }
    pub fn message(
        &mut self,
        message: &crate::message::types::MessageBlock,
        conversation: &crate::runtime::identity::ConversationId,
        path: &[String],
    ) -> crate::message::types::MessageBlock {
        use crate::message::types::MessageBlock;
        let block_count = match message {
            MessageBlock::User(user) => user.content.len(),
            MessageBlock::Assistant(assistant) => assistant.content.len(),
            MessageBlock::Tool(tool) => tool.result.content.len(),
        };
        if block_count <= CHECKPOINT_PAGE_ITEMS
            && message_json_safe(message)
            && let Some(copy) = bounded_clone(message, CHECKPOINT_ITEM_BYTES)
        {
            return copy;
        }
        self.frontier(
            &format!("{}/content", path.join("/")),
            block_count as u64,
            block_count.min(CHECKPOINT_PAGE_ITEMS) as u64,
            (block_count > CHECKPOINT_PAGE_ITEMS).then_some(CHECKPOINT_PAGE_ITEMS as u64),
        );
        match message {
            MessageBlock::User(user) => self.user_message(user, conversation, path),
            MessageBlock::Assistant(assistant) => {
                self.assistant_message(assistant, conversation, path)
            }
            MessageBlock::Tool(tool) => self.tool_message(tool, conversation, path),
        }
    }
    pub(crate) fn user_message(
        &mut self,
        user: &crate::message::types::UserMessageBlock,
        conversation: &crate::runtime::identity::ConversationId,
        path: &[String],
    ) -> crate::message::types::MessageBlock {
        use super::content::ContentRepresentation as Rep;
        use crate::message::types::{MessageBlock, UserContentBlock};
        let id = &user.id;

        MessageBlock::User(crate::message::types::UserMessageBlock {
            id: user.id.clone(),
            source: user.source.clone(),
            kind: bounded_inbound_kind(&user.kind),
            timestamp: user.timestamp,
            content: user
                .content
                .iter()
                .take(CHECKPOINT_PAGE_ITEMS)
                .enumerate()
                .map(|(index, block)| {
                    let mut field = path.to_vec();
                    field.extend(["content".into(), index.to_string()]);
                    match block {
                        UserContentBlock::Text(text) => {
                            field.push("text".into());
                            UserContentBlock::Text(crate::message::content::TextBlock {
                                text: self.text(
                                    &text.text,
                                    conversation,
                                    id,
                                    index,
                                    field,
                                    Rep::Utf8Text,
                                ),
                            })
                        }
                        other => bounded_clone(other, 1024).unwrap_or_else(|| {
                            self.reference(conversation, id, index, field, Rep::CanonicalJsonV1, 0);
                            UserContentBlock::Text(crate::message::content::TextBlock {
                                text: String::new(),
                            })
                        }),
                    }
                })
                .collect(),
        })
    }

    fn assistant_message(
        &mut self,
        assistant: &crate::message::types::AssistantMessageBlock,
        conversation: &crate::runtime::identity::ConversationId,
        path: &[String],
    ) -> crate::message::types::MessageBlock {
        use super::content::ContentRepresentation as Rep;
        use crate::message::types::{AssistantContentBlock, MessageBlock};
        let id = &assistant.id;

        MessageBlock::Assistant(crate::message::types::AssistantMessageBlock {
            id: assistant.id.clone(),
            content: assistant
                .content
                .iter()
                .take(CHECKPOINT_PAGE_ITEMS)
                .enumerate()
                .map(|(index, block)| {
                    let mut field = path.to_vec();
                    field.extend(["content".into(), index.to_string()]);
                    match block {
                        AssistantContentBlock::Text(text) => {
                            field.push("text".into());
                            AssistantContentBlock::Text(crate::message::content::TextBlock {
                                text: self.text(
                                    &text.text,
                                    conversation,
                                    id,
                                    index,
                                    field,
                                    Rep::Utf8Text,
                                ),
                            })
                        }
                        AssistantContentBlock::Reasoning(reasoning) => {
                            field.push("text".into());
                            AssistantContentBlock::Reasoning(
                                crate::message::types::ReasoningBlock {
                                    text: reasoning.text.as_ref().map(|text| {
                                        self.text(
                                            text,
                                            conversation,
                                            id,
                                            index,
                                            field,
                                            Rep::Utf8Text,
                                        )
                                    }),
                                    provider_state: reasoning
                                        .provider_state
                                        .as_ref()
                                        .and_then(|state| bounded_clone(state, 1024)),
                                },
                            )
                        }
                        AssistantContentBlock::Refusal(refusal) => {
                            field.push("text".into());
                            AssistantContentBlock::Refusal(crate::message::types::RefusalBlock {
                                text: self.text(
                                    &refusal.text,
                                    conversation,
                                    id,
                                    index,
                                    field,
                                    Rep::Utf8Text,
                                ),
                            })
                        }
                        AssistantContentBlock::ToolCall(call) => {
                            field.push("arguments".into());
                            let arguments = bounded_json_value(&call.arguments, 1024)
                                .unwrap_or_else(|| {
                                    self.reference(
                                        conversation,
                                        id,
                                        index,
                                        field,
                                        Rep::CanonicalJsonV1,
                                        0,
                                    );
                                    serde_json::Value::Null
                                });
                            AssistantContentBlock::ToolCall(crate::tools::types::ToolCall {
                                id: call.id.clone(),
                                tool_id: call.tool_id.clone(),
                                name: call.name.clone(),
                                arguments,
                            })
                        }
                        other @ AssistantContentBlock::Image(_) => bounded_clone(other, 1024)
                            .unwrap_or_else(|| {
                                self.reference(
                                    conversation,
                                    id,
                                    index,
                                    field,
                                    Rep::CanonicalJsonV1,
                                    0,
                                );
                                AssistantContentBlock::Text(crate::message::content::TextBlock {
                                    text: String::new(),
                                })
                            }),
                    }
                })
                .collect(),
        })
    }

    fn tool_message(
        &mut self,
        tool: &crate::message::types::ToolMessageBlock,
        conversation: &crate::runtime::identity::ConversationId,
        path: &[String],
    ) -> crate::message::types::MessageBlock {
        use super::content::ContentRepresentation as Rep;
        use crate::message::types::MessageBlock;
        let id = &tool.id;

        MessageBlock::Tool(crate::message::types::ToolMessageBlock {
            id: tool.id.clone(),
            occurrence: tool.occurrence.clone(),
            tool_call_id: tool.tool_call_id.clone(),
            tool_id: tool.tool_id.clone(),
            result: crate::tools::types::ToolExecutionResult {
                status: tool.result.status.clone(),
                duration_ms: tool.result.duration_ms,
                exit_code: tool.result.exit_code,
                content: tool
                    .result
                    .content
                    .iter()
                    .take(CHECKPOINT_PAGE_ITEMS)
                    .enumerate()
                    .map(|(index, block)| {
                        let mut field = path.to_vec();
                        field.extend(["result".into(), "content".into(), index.to_string()]);
                        match block {
                            crate::tools::types::ToolResultContent::Text(text) => {
                                field.push("text".into());
                                crate::tools::types::ToolResultContent::Text(
                                    crate::message::content::TextBlock {
                                        text: self.text(
                                            &text.text,
                                            conversation,
                                            id,
                                            index,
                                            field,
                                            Rep::Utf8Text,
                                        ),
                                    },
                                )
                            }
                            crate::tools::types::ToolResultContent::Json { value } => {
                                field.push("value".into());
                                crate::tools::types::ToolResultContent::Json {
                                    value: bounded_json_value(value, 1024).unwrap_or_else(|| {
                                        self.reference(
                                            conversation,
                                            id,
                                            index,
                                            field,
                                            Rep::CanonicalJsonV1,
                                            0,
                                        );
                                        serde_json::Value::Null
                                    }),
                                }
                            }
                            other => bounded_clone(other, 1024).unwrap_or_else(|| {
                                self.reference(
                                    conversation,
                                    id,
                                    index,
                                    field,
                                    Rep::CanonicalJsonV1,
                                    0,
                                );
                                crate::tools::types::ToolResultContent::Text(
                                    crate::message::content::TextBlock {
                                        text: String::new(),
                                    },
                                )
                            }),
                        }
                    })
                    .collect(),
                workflow: bounded_clone(&tool.result.workflow, 1024).flatten(),
                artifacts: self.page(
                    &format!("{}/artifacts", path.join("/")),
                    &tool.result.artifacts,
                ),
                deliveries: self.page(
                    &format!("{}/deliveries", path.join("/")),
                    &tool.result.deliveries,
                ),
                truncation: bounded_clone(&tool.result.truncation, 1024).flatten(),
                managed_output: bounded_clone(&tool.result.managed_output, 1024).flatten(),
            },
        })
    }

    fn text(
        &mut self,
        text: &str,
        conversation: &crate::runtime::identity::ConversationId,
        id: &crate::runtime::identity::MessageId,
        index: usize,
        path: Vec<String>,
        representation: super::content::ContentRepresentation,
    ) -> String {
        if text.len() <= 1024 {
            return text.to_owned();
        }
        self.reference(
            conversation,
            id,
            index,
            path,
            representation,
            text.len() as u64,
        );
        String::new()
    }
    fn reference(
        &mut self,
        conversation: &crate::runtime::identity::ConversationId,
        id: &crate::runtime::identity::MessageId,
        index: usize,
        path: Vec<String>,
        representation: super::content::ContentRepresentation,
        readable_end: u64,
    ) {
        self.content.push(
            ContentBinding {
                source_path: None,
                path,
                deferred: true,
                content_ref: ContentRef {
                    conversation_id: conversation.clone(),
                    source: super::content::ContentSource::CanonicalMessageBlock {
                        component:
                            crate::runtime_client::content::CanonicalContentComponent::Payload,
                        locator:
                            crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
                                message_id: id.clone(),
                            },
                        block_index: crate::message::types::ContentBlockIndex::new(
                            u32::try_from(index).expect("bounded block index"),
                        ),
                    },
                    representation,
                    version: 1,
                    content_revision: 1,
                    readable_end,
                },
            }
            .into(),
        );
    }
}

pub(crate) fn page<T: Serialize + schemars::JsonSchema>(
    domain: &str,
    cut: CheckpointReadCut,
    cursor: RuntimeClientCursor,
    items: &[T],
    offset: u64,
    limit: usize,
    identity: impl Fn(&T) -> String,
) -> Result<CheckpointPage, super::types::RuntimeClientError> {
    if limit == 0 || limit > CHECKPOINT_PAGE_ITEMS || offset > items.len() as u64 {
        return Err(super::types::RuntimeClientError::InvalidRequest {
            message: "invalid checkpoint page bounds".into(),
        });
    }
    let start =
        usize::try_from(offset).map_err(|_| super::types::RuntimeClientError::InvalidRequest {
            message: "invalid page offset".into(),
        })?;
    let mut page = Vec::new();
    let mut bytes = 0;
    for (index, item) in items[start..].iter().enumerate().take(limit) {
        let root = super::structural_json::root::<T>();
        let encoded = bounded_item_bytes(
            &super::structural_json::Encoded {
                value: item,
                schema: &root,
                root: &root,
            },
            CHECKPOINT_ITEM_BYTES,
        );
        let weight = encoded.as_ref().map_or(512, Vec::len);
        if bytes + weight > CHECKPOINT_TARGET_BYTES / 2 {
            break;
        }
        bytes += weight;
        let value = encoded.and_then(|bytes| serde_json::from_slice(&bytes).ok());
        page.push(CheckpointPageItem {
            body_complete: None,
            key: None,
            offset: (start + index) as u64,
            identity: identity(item),
            deferred: value.is_none(),
            value,
            content: Vec::new(),
        });
    }
    let next = offset + page.len() as u64;
    Ok(CheckpointPage {
        collections: Vec::new(),
        cursor,
        read_cut: cut,
        domain: domain.into(),
        total: items.len() as u64,
        items: page,
        next_offset: (next < items.len() as u64).then_some(next),
    })
}

impl Capture {
    #[cfg(test)]
    pub fn transcript_page(
        &mut self,
        entries: &[super::snapshot::RuntimeClientTranscriptEntry],
        conversation: &crate::runtime::identity::ConversationId,
    ) -> Vec<super::snapshot::RuntimeClientTranscriptEntry> {
        let mut result = Vec::new();
        for (index, entry) in entries.iter().take(CHECKPOINT_PAGE_ITEMS).enumerate() {
            let Some(projected) = self.transcript_row(entry, conversation, index) else {
                break;
            };
            result.push(projected);
        }
        self.frontier(
            "transcript.entries",
            entries.len() as u64,
            result.len() as u64,
            (result.len() < entries.len()).then_some(result.len() as u64),
        );
        result
    }
    #[cfg(test)]
    fn transcript_row(
        &mut self,
        entry: &super::snapshot::RuntimeClientTranscriptEntry,
        conversation: &crate::runtime::identity::ConversationId,
        index: usize,
    ) -> Option<super::snapshot::RuntimeClientTranscriptEntry> {
        let before = self.content.len();
        let result = self.transcript_entry(
            entry,
            conversation,
            &["transcript".into(), "entries".into(), index.to_string()],
        );
        if let Some(projected) = result
            && let Some(length) = bounded_len(&projected, self.item_budget())
        {
            self.remaining -= length;
            return Some(projected);
        }
        self.content.truncate(before);
        None
    }
    fn audit_block(
        &mut self,
        audit: &crate::publication::PublicationAudit,
        block: &crate::publication::PublicationAuditBlock,
        index: usize,
        conversation: &crate::runtime::identity::ConversationId,
        path: &[String],
    ) -> crate::publication::PublicationAuditBlock {
        use crate::publication::PublicationAuditBlock as Block;
        let mut field = path.to_vec();
        field.extend([
            "item".into(),
            "audit".into(),
            "content".into(),
            index.to_string(),
        ]);
        let (block_index, text, representation) = match block {
            Block::Text { block_index, text }
            | Block::Reasoning { block_index, text }
            | Block::Refusal { block_index, text } => (
                *block_index,
                text,
                super::content::ContentRepresentation::Utf8Text,
            ),
            Block::ProposedToolCall {
                block_index,
                arguments,
                ..
            } => (
                *block_index,
                arguments,
                super::content::ContentRepresentation::RawJsonUtf8,
            ),
        };
        let displayed = if text.len() <= 1024 {
            text.clone()
        } else {
            field.push(
                if representation == super::content::ContentRepresentation::RawJsonUtf8 {
                    "arguments"
                } else {
                    "text"
                }
                .into(),
            );
            self.content.push(ContentBinding {
                source_path: None,
                path: field,
                deferred: true,
                content_ref: ContentRef {
                    conversation_id: conversation.clone(),
                    source: super::content::ContentSource::DurableTranscriptAudit {
                        component:
                            crate::runtime_client::content::PublicationContentComponent::Payload,
                        stream_id: audit.stream_id.clone(),
                        message_id: audit.message_id.clone(),
                        attempt_id: audit.attempt_id.clone(),
                        turn_id: audit.turn_id.clone(),
                        publication_request_id: audit.request_id.clone(),
                        block_index,
                    },
                    representation,
                    version: 1,
                    content_revision: 1,
                    readable_end: text.len() as u64,
                },
            }.into());
            String::new()
        };
        match block {
            Block::Text { block_index, .. } => Block::Text {
                block_index: *block_index,
                text: displayed,
            },
            Block::Reasoning { block_index, .. } => Block::Reasoning {
                block_index: *block_index,
                text: displayed,
            },
            Block::Refusal { block_index, .. } => Block::Refusal {
                block_index: *block_index,
                text: displayed,
            },
            Block::ProposedToolCall {
                block_index,
                call_id,
                tool_id,
                name,
                complete,
                ..
            } => Block::ProposedToolCall {
                block_index: *block_index,
                call_id: call_id.clone(),
                tool_id: tool_id.clone(),
                name: name.chars().take(256).collect(),
                arguments: displayed,
                complete: *complete,
            },
        }
    }
    pub(crate) fn audit(
        &mut self,
        audit: &crate::publication::PublicationAudit,
        conversation: &crate::runtime::identity::ConversationId,
        path: &[String],
    ) -> Option<crate::publication::PublicationAudit> {
        if [
            audit.stream_id.as_str(),
            audit.message_id.as_str(),
            audit.attempt_id.as_str(),
            audit.turn_id.as_str(),
            audit.request_id.as_str(),
        ]
        .iter()
        .any(|id| id.len() > 256)
        {
            return None;
        }
        let mut body = audit
            .content
            .iter()
            .take(CHECKPOINT_PAGE_ITEMS)
            .take_while(|block| !matches!(block,crate::publication::PublicationAuditBlock::ProposedToolCall{call_id,tool_id,..} if call_id.as_str().len()>256 || tool_id.as_str().len()>256))
            .enumerate()
            .map(|(index,block)|self.audit_block(audit,block,index,conversation,path))
            .collect::<Vec<_>>();
        self.frontier(
            &format!("{}/item/audit/content", path.join("/")),
            audit.content.len() as u64,
            body.len() as u64,
            (body.len() < audit.content.len()).then_some(body.len() as u64),
        );
        Some(crate::publication::PublicationAudit {
            stream_id: audit.stream_id.clone(),
            attempt_id: audit.attempt_id.clone(),
            turn_id: audit.turn_id.clone(),
            request_id: audit.request_id.clone(),
            message_id: audit.message_id.clone(),
            kind: audit.kind,
            content: std::mem::take(&mut body),
            settled_at: audit.settled_at,
        })
    }
    pub fn transcript_entry(
        &mut self,
        entry: &super::snapshot::RuntimeClientTranscriptEntry,
        conversation: &crate::runtime::identity::ConversationId,
        path: &[String],
    ) -> Option<super::snapshot::RuntimeClientTranscriptEntry> {
        use super::snapshot::RuntimeClientTranscriptItem as Item;
        if let Some(copy) = bounded_clone(entry, CHECKPOINT_ITEM_BYTES) {
            return Some(copy);
        }
        let item = match &entry.item {
            Item::Message { message } => {
                let mut path = path.to_vec();
                path.extend(["item".into(), "message".into()]);
                Item::Message {
                    message: self.message(message, conversation, &path),
                }
            }
            Item::PublicationAudit { audit } => Item::PublicationAudit {
                audit: self.audit(audit, conversation, path)?,
            },
            other => bounded_clone(other, CHECKPOINT_ITEM_BYTES)?,
        };
        Some(super::snapshot::RuntimeClientTranscriptEntry {
            cursor: entry.cursor,
            item,
            response_pending: entry.response_pending,
            turn_process: bounded_clone(&entry.turn_process, 2048).flatten(),
            completed_response: bounded_clone(&entry.completed_response, 2048).flatten(),
            tool_calls: self.page(&format!("{}/tool_calls", path.join("/")), &entry.tool_calls),
        })
    }
}

/// Fixed-size source metadata is fetched before the Projection lock is taken.
/// Lookup reads indexes only; it never reads or serializes canonical bodies.
pub(crate) fn message_metadata(
    store: &dyn crate::durable::ConversationStore,
    message: &crate::message::types::MessageBlock,
) -> Vec<ContentRef> {
    use super::content::{ContentRepresentation as Rep, ContentSource};
    use crate::message::types::{AssistantContentBlock, MessageBlock, UserContentBlock};
    let reps: Vec<_> = match message {
        MessageBlock::User(user) => user
            .content
            .iter()
            .take(CHECKPOINT_PAGE_ITEMS)
            .map(|block| match block {
                UserContentBlock::Text(_) => Rep::Utf8Text,
                _ => Rep::CanonicalJsonV1,
            })
            .collect(),
        MessageBlock::Assistant(assistant) => assistant
            .content
            .iter()
            .take(CHECKPOINT_PAGE_ITEMS)
            .map(|block| match block {
                AssistantContentBlock::Text(_)
                | AssistantContentBlock::Reasoning(_)
                | AssistantContentBlock::Refusal(_) => Rep::Utf8Text,
                _ => Rep::CanonicalJsonV1,
            })
            .collect(),
        MessageBlock::Tool(tool) => tool
            .result
            .content
            .iter()
            .take(CHECKPOINT_PAGE_ITEMS)
            .map(|block| match block {
                crate::tools::types::ToolResultContent::Text(_) => Rep::Utf8Text,
                _ => Rep::CanonicalJsonV1,
            })
            .collect(),
    };
    reps.into_iter()
        .enumerate()
        .flat_map(|(index, representation)| {
            let source = ContentSource::CanonicalMessageBlock {
                component: crate::runtime_client::content::CanonicalContentComponent::Payload,
                locator: crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
                    message_id: message.id().clone(),
                },
                block_index: crate::message::types::ContentBlockIndex::new(
                    u32::try_from(index).expect("bounded block index"),
                ),
            };
            let mut references = Vec::new();
            let metadata_source = ContentSource::CanonicalMessageBlock {
                component: crate::runtime_client::content::CanonicalContentComponent::BlockMetadata,
                locator: crate::runtime_client::content::CanonicalContentLocator::ByMessageId {
                    message_id: message.id().clone(),
                },
                block_index: crate::message::types::ContentBlockIndex::new(
                    u32::try_from(index).expect("bounded block index"),
                ),
            };
            if let Some(reference) = store
                .content_descriptor(&metadata_source, Rep::CanonicalJsonV1)
                .ok()
                .flatten()
            {
                references.push(reference);
            }
            if let Some(reference) = store
                .content_descriptor(&source, representation)
                .ok()
                .flatten()
            {
                references.push(reference);
            }
            if let Some(reference) = store
                .content_descriptor(&source, Rep::RawJsonUtf8)
                .ok()
                .flatten()
            {
                references.push(reference);
            }
            references
        })
        .collect()
}
pub(crate) fn observation_metadata(
    store: &dyn crate::durable::ConversationStore,
    observation: &crate::runtime::observation::ConversationObservation,
) -> Vec<ContentRef> {
    use crate::runtime::observation::ConversationObservation as Observation;
    #[cfg(not(test))]
    let _ = store;
    observation
        .facts()
        .flat_map(|fact| match fact {
            Observation::CanonicalCommitted { receipt, .. } => receipt
                .observation
                .head
                .iter()
                .flat_map(|header| &header.content)
                .map(|binding| binding.content_ref.clone())
                .collect::<Vec<_>>(),
            Observation::AuditSettled { receipt, .. } => receipt
                .header
                .content
                .iter()
                .map(|binding| binding.content_ref.clone())
                .collect::<Vec<_>>(),
            #[cfg(test)]
            Observation::Committed { block, .. } => message_metadata(store, block),
            _ => Vec::new(),
        })
        .take(CHECKPOINT_PAGE_ITEMS * CHECKPOINT_PAGE_ITEMS)
        .collect()
}

/// Finite native frame read evidence; computed before acquiring Projection synchronization.
#[derive(Clone)]
pub(crate) struct PreparedPublicationContent {
    pub suffix: bool,
    pub base_length: Option<u64>,
    pub stream_id: crate::runtime::identity::PublicationStreamId,
    pub block_index: crate::message::types::ContentBlockIndex,
    pub revision: u64,
    pub component: super::content::PublicationContentComponent,
    pub representation: super::content::ContentRepresentation,
    pub readable_end: u64,
}

pub(crate) fn publication_content_evidence(
    observation: &crate::runtime::observation::ConversationObservation,
) -> Vec<PreparedPublicationContent> {
    observation
        .facts()
        .flat_map(|fact| match fact {
            crate::runtime::observation::ConversationObservation::Publication { frame, .. } => {
                publication_frame_evidence(frame)
            }
            _ => Vec::new(),
        })
        .take(1024)
        .collect()
}

#[derive(serde::Serialize)]
struct PublicationMetadata<'a> {
    r#type: &'static str,
    id: &'a crate::runtime::identity::ToolCallId,
    tool_id: &'a crate::runtime::identity::ToolId,
    name: &'a str,
}
struct PublicationLength(u64);
impl std::io::Write for PublicationLength {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self
            .0
            .checked_add(bytes.len() as u64)
            .filter(|length| *length <= 9_007_199_254_740_991)
            .ok_or_else(|| {
                std::io::Error::other("publication content exceeds native byte range")
            })?;
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
fn publication_encoded_length(value: &impl serde::Serialize) -> Option<u64> {
    let mut writer = PublicationLength(0);
    serde_json::to_writer(&mut writer, value).ok()?;
    Some(writer.0)
}

fn publication_frame_evidence(
    frame: &crate::publication::PublicationFrame,
) -> Vec<PreparedPublicationContent> {
    use crate::publication::PublicationPayload;
    let suffix = match &frame.payload {
        PublicationPayload::TextSuffix {
            block_index,
            suffix,
        }
        | PublicationPayload::ReasoningSuffix {
            block_index,
            suffix,
        }
        | PublicationPayload::RefusalSuffix {
            block_index,
            suffix,
        } => Some((
            *block_index,
            suffix,
            super::content::ContentRepresentation::Utf8Text,
        )),
        PublicationPayload::ProposedToolCallArgumentsSuffix {
            block_index,
            suffix,
            ..
        } => Some((
            *block_index,
            suffix,
            super::content::ContentRepresentation::RawJsonUtf8,
        )),
        _ => None,
    };
    if let Some((block_index, suffix, representation)) = suffix {
        return vec![PreparedPublicationContent {
            suffix: true,
            base_length: None,
            stream_id: frame.stream_id.clone(),
            block_index,
            revision: frame.sequence + 1,
            component: super::content::PublicationContentComponent::Payload,
            representation,
            readable_end: suffix.len() as u64,
        }];
    }
    let (block, call_id, tool_id, name, value) = match &frame.payload {
        PublicationPayload::ProposedToolCallStarted { block_index, call } => (
            *block_index,
            &call.id,
            &call.tool_id,
            call.name.as_str(),
            None,
        ),
        PublicationPayload::ProposedToolCallCompleted { block_index, call } => (
            *block_index,
            &call.id,
            &call.tool_id,
            call.name.as_str(),
            Some(&call.arguments),
        ),
        _ => return Vec::new(),
    };
    let make = |component, readable_end| PreparedPublicationContent {
        suffix: false,
        base_length: None,
        stream_id: frame.stream_id.clone(),
        block_index: block,
        revision: frame
            .sequence
            .checked_add(1)
            .expect("native frame revision"),
        component,
        representation: super::content::ContentRepresentation::CanonicalJsonV1,
        readable_end,
    };
    let mut evidence = Vec::with_capacity(2);
    if let Some(end) = publication_encoded_length(&PublicationMetadata {
        r#type: "tool_call",
        id: call_id,
        tool_id,
        name,
    }) {
        evidence.push(make(
            super::content::PublicationContentComponent::BlockMetadata,
            end,
        ));
    }
    if let Some(value) = value.and_then(publication_encoded_length) {
        evidence.push(make(
            super::content::PublicationContentComponent::Payload,
            value,
        ));
    }
    evidence
}

/// Copies only the finite display prefix, leaving native Frame bodies source-owned.
pub(crate) fn bounded_publication_frame(
    frame: &crate::publication::PublicationFrame,
) -> crate::publication::PublicationFrame {
    use crate::publication::PublicationPayload as P;
    let prefix = |text: &str| {
        text.chars()
            .take(CHECKPOINT_ITEM_BYTES / 4)
            .collect::<String>()
    };
    let payload = match &frame.payload {
        P::TextSuffix {
            block_index,
            suffix,
        } => P::TextSuffix {
            block_index: *block_index,
            suffix: prefix(suffix),
        },
        P::ReasoningSuffix {
            block_index,
            suffix,
        } => P::ReasoningSuffix {
            block_index: *block_index,
            suffix: prefix(suffix),
        },
        P::RefusalSuffix {
            block_index,
            suffix,
        } => P::RefusalSuffix {
            block_index: *block_index,
            suffix: prefix(suffix),
        },
        P::ProposedToolCallArgumentsSuffix {
            block_index,
            call_id,
            suffix,
        } => P::ProposedToolCallArgumentsSuffix {
            block_index: *block_index,
            call_id: call_id.clone(),
            suffix: prefix(suffix),
        },
        P::ProposedToolCallStarted { block_index, call } => P::ProposedToolCallStarted {
            block_index: *block_index,
            call: crate::tools::types::ToolCallStart {
                id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                name: prefix(&call.name),
            },
        },
        P::ProposedToolCallCompleted { block_index, call } => P::ProposedToolCallCompleted {
            block_index: *block_index,
            call: crate::tools::types::ToolCall {
                id: call.id.clone(),
                tool_id: call.tool_id.clone(),
                name: prefix(&call.name),
                arguments: bounded_json_value(&call.arguments, 1024)
                    .unwrap_or(serde_json::Value::Null),
            },
        },
        P::TerminalOnly => P::TerminalOnly,
    };
    crate::publication::PublicationFrame {
        payload,
        stream_id: frame.stream_id.clone(),
        message_id: frame.message_id.clone(),
        sequence: frame.sequence,
    }
}

pub(crate) fn bound_publication_payload(
    observation: &mut crate::runtime::observation::ConversationObservation,
) {
    use crate::publication::PublicationPayload as P;
    use crate::runtime::observation::ConversationObservation as O;
    let prefix = |text: &mut String| {
        if text.len() > CHECKPOINT_ITEM_BYTES {
            let mut end = CHECKPOINT_ITEM_BYTES;
            while !text.is_char_boundary(end) {
                end -= 1;
            }
            text.truncate(end);
        }
    };
    match observation {
        O::Publication { frame, .. } => match &mut frame.payload {
            P::TextSuffix { suffix, .. }
            | P::ReasoningSuffix { suffix, .. }
            | P::RefusalSuffix { suffix, .. }
            | P::ProposedToolCallArgumentsSuffix { suffix, .. } => prefix(suffix),
            P::ProposedToolCallStarted { call, .. } => prefix(&mut call.name),
            P::ProposedToolCallCompleted { call, .. } => {
                prefix(&mut call.name);
                call.arguments =
                    bounded_json_value(&call.arguments, 1024).unwrap_or(serde_json::Value::Null);
            }
            P::TerminalOnly => {}
        },
        O::Published { observation, .. } => {
            bound_publication_payload(observation);
        }
        O::JournalBatch { observations, .. } => {
            observations.iter_mut().for_each(bound_publication_payload);
        }
        _ => {}
    }
}

/// Constructs only finite identity metadata under the Projection boundary.
/// Full immutable native bodies are source-owned and read independently.
#[cfg(test)]
pub(crate) fn interaction_header(
    value: &crate::runtime::interaction::RoutedInteraction,
) -> super::snapshot::RuntimeClientInteractionHeader {
    interaction_request_header(&value.request, value.source.clone())
}
pub(crate) fn interaction_request_header(
    request: &crate::runtime::interaction::InteractionRequest,
    source: crate::runtime::interaction::InteractionSource,
) -> super::snapshot::RuntimeClientInteractionHeader {
    use super::snapshot::{
        RuntimeClientInteractionHeader as Header, RuntimeClientInteractionKind as Kind,
    };
    use crate::runtime::interaction::InteractionKind;
    let (kind, original_node, node_path) = match &request.kind {
        InteractionKind::Approval { invocation_id, .. } => (
            Kind::Approval,
            match invocation_id {
                crate::tools::types::ToolInvocationId::Workflow { node } => Some(node.as_ref()),
                crate::tools::types::ToolInvocationId::Agent { .. } => None,
            },
            vec![
                "request".into(),
                "kind".into(),
                "invocation_id".into(),
                "node".into(),
            ],
        ),
        InteractionKind::Questionnaire { invocation_id, .. } => (
            Kind::Questionnaire,
            match invocation_id {
                crate::tools::types::ToolInvocationId::Workflow { node } => Some(node.as_ref()),
                crate::tools::types::ToolInvocationId::Agent { .. } => None,
            },
            vec![
                "request".into(),
                "kind".into(),
                "invocation_id".into(),
                "node".into(),
            ],
        ),
        InteractionKind::Review { review, .. } => (
            Kind::Review,
            Some(review.instance.as_ref()),
            vec![
                "request".into(),
                "kind".into(),
                "review".into(),
                "instance".into(),
            ],
        ),
    };
    let workflow_node = original_node
        .and_then(|node| bounded_clone(node, 1024))
        .map(Box::new);
    let workflow_node_deferred = original_node.is_some() && workflow_node.is_none();
    Header {
        source_offset: 0,
        interaction: request.interaction_ref(),
        source,
        request_id: request.id.clone(),
        conversation_id: request.conversation_id.clone(),
        attempt_id: request.attempt_id.clone(),
        turn: request.turn,
        kind,
        approval: match &request.kind {
            InteractionKind::Approval {
                tool_id,
                tool_name,
                reason,
                mode,
                ..
            } => Some(super::snapshot::RuntimeClientApprovalSummary {
                tool_id: super::snapshot::inline_tool_id(tool_id),
                tool_name: tool_name.chars().take(256).collect(),
                reason: reason.chars().take(256).collect(),
                mode: *mode,
                deferred_fields: {
                    let mut fields =
                        vec![vec!["request".into(), "kind".into(), "arguments".into()]];
                    if tool_id.as_str().len() > 256 {
                        fields.push(vec!["request".into(), "kind".into(), "tool_id".into()]);
                    }
                    if tool_name.len() > 256 {
                        fields.push(vec!["request".into(), "kind".into(), "tool_name".into()]);
                    }
                    if reason.len() > 256 {
                        fields.push(vec!["request".into(), "kind".into(), "reason".into()]);
                    }
                    fields
                },
            }),
            _ => None,
        },
        body: None,
        body_complete: false,
        workflow_node,
        workflow_node_deferred,
        workflow_node_path: workflow_node_deferred.then_some(node_path),
    }
}
impl Capture {
    pub(crate) fn interaction_page(
        &mut self,
        items: &[super::snapshot::RuntimeClientInteractionHeader],
    ) -> Vec<super::snapshot::RuntimeClientInteractionHeader> {
        let mut result = Vec::new();
        for (index, header) in items.iter().take(CHECKPOINT_PAGE_ITEMS).enumerate() {
            if !self.reserve(header) {
                break;
            }
            if !header.body_complete {
                self.frontier(&format!("pending_interactions.{index}.body"), 1, 0, Some(0));
            }
            result.push(header.clone());
        }
        self.frontier(
            "pending_interactions",
            items.len() as u64,
            result.len() as u64,
            (result.len() < items.len()).then_some(result.len() as u64),
        );
        result
    }
}

#[cfg(test)]
mod prepared_capability_tests;

#[cfg(test)]
mod structural_length_tests;

#[cfg(test)]
mod native_length_tests;

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn t20_checkpoint_encoder_capacity_obeys_its_physical_byte_budget() {
        use std::io::Write;

        let value = serde_json::json!({
            "body": "x".repeat(600_000),
            "items": (0..30_000).collect::<Vec<u32>>(),
            "escaped": "\n\"\\\t水",
        });
        let mut buffer = Buffer {
            bytes: Vec::new(),
            limit: CHECKPOINT_TARGET_BYTES,
        };
        serde_json::to_writer(&mut buffer, &value).unwrap();
        assert_eq!(buffer.bytes, serde_json::to_vec(&value).unwrap());
        assert!(buffer.bytes.len() < buffer.limit);
        assert!(buffer.bytes.capacity() <= buffer.limit);
        let remaining = buffer.limit - buffer.bytes.len();
        buffer.write_all(&vec![b' '; remaining]).unwrap();
        let capacity = buffer.bytes.capacity();
        assert_eq!(buffer.bytes.len(), buffer.limit);
        assert!(capacity <= buffer.limit);
        assert!(buffer.write_all(b" ").is_err());
        assert_eq!(buffer.bytes.len(), buffer.limit);
        assert_eq!(buffer.bytes.capacity(), capacity);
    }

    #[test]
    fn t20_checkpoint_encoder_preserves_exact_bytes_at_small_budgets() {
        use std::io::Write;

        let value = "\n\"\\\t水";
        let expected = serde_json::to_vec(value).unwrap();
        let bytes = bounded_bytes(value, expected.len()).unwrap();
        assert_eq!(bytes, expected);
        assert!(bytes.capacity() <= expected.len());
        assert!(bounded_bytes(value, expected.len() - 1).is_none());
        for limit in [0, 1, 7, 17] {
            let mut buffer = Buffer {
                bytes: Vec::new(),
                limit,
            };
            for _ in 0..limit {
                buffer.write_all(b"x").unwrap();
                assert!(buffer.bytes.capacity() <= limit);
            }
            let capacity = buffer.bytes.capacity();
            assert!(buffer.write_all(b"x").is_err());
            assert_eq!(buffer.bytes.len(), limit);
            assert_eq!(buffer.bytes.capacity(), capacity);
        }
    }

    #[test]
    fn structural_item_reuses_admission_without_changing_bytes_counts_or_integer_domains() {
        #[derive(Serialize, schemars::JsonSchema)]
        struct Header {
            sequence: u64,
            opaque: serde_json::Value,
            items: Vec<u64>,
        }
        let mut header = Header {
            sequence: u64::MAX,
            opaque: serde_json::json!({"signed": -17, "fraction": 1.5, "text": "水\n\\\""}),
            items: (0..CHECKPOINT_PAGE_ITEMS as u64).collect(),
        };
        let projected = structural_value(&header).unwrap();
        assert_eq!(projected["sequence"], u64::MAX.to_string());
        assert_eq!(projected["opaque"], header.opaque);
        assert_eq!(
            projected["items"].as_array().unwrap().len(),
            CHECKPOINT_PAGE_ITEMS
        );
        assert_eq!(projected["items"][31], "31");
        header.items.push(32);
        assert!(structural_value(&header).is_none());

        let exact = "x".repeat(CHECKPOINT_ITEM_BYTES - 2);
        assert_eq!(structural_value(&exact), Some(serde_json::json!(exact)));
        assert!(structural_value(&(exact + "x")).is_none());
        let opaque = serde_json::json!({"integer": u64::MAX});
        assert!(bounded_json_value(&opaque, CHECKPOINT_ITEM_BYTES).is_none());
        assert_eq!(
            bounded_json_value(&header.opaque, CHECKPOINT_ITEM_BYTES),
            Some(header.opaque)
        );
        let map: std::collections::BTreeMap<_, _> = (0..=CHECKPOINT_PAGE_ITEMS)
            .map(|index| (index.to_string(), "original"))
            .collect();
        assert!(structural_value(&map).is_none());
    }

    fn byte_oracle_item<T: Serialize + DeserializeOwned>(
        capture: &mut Capture,
        domain: &str,
        item: &T,
    ) -> Option<T> {
        let bytes = bounded_item_bytes(item, CHECKPOINT_ITEM_BYTES.min(capture.remaining));
        capture.evicted |= bytes.is_none();
        let loaded = u64::from(bytes.is_some());
        capture.frontier(domain, 1, loaded, (loaded == 0).then_some(0));
        let bytes = bytes?;
        capture.remaining -= bytes.len();
        serde_json::from_slice(&bytes).ok()
    }

    fn byte_oracle_page<T: Serialize + DeserializeOwned>(
        capture: &mut Capture,
        domain: &str,
        items: &[T],
    ) -> Vec<T> {
        let mut result = Vec::new();
        for item in items.iter().take(CHECKPOINT_PAGE_ITEMS) {
            let Some(bytes) =
                bounded_item_bytes(item, CHECKPOINT_ITEM_BYTES.min(capture.remaining))
            else {
                capture.evicted = true;
                break;
            };
            capture.remaining -= bytes.len();
            result.push(serde_json::from_slice(&bytes).unwrap());
        }
        let loaded = result.len() as u64;
        capture.frontier(
            domain,
            items.len() as u64,
            loaded,
            (loaded < items.len() as u64).then_some(loaded),
        );
        result
    }

    #[test]
    fn dto_capture_value_decode_matches_original_bytes_at_every_admission_boundary() {
        #[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Header {
            sequence: u64,
            opaque: serde_json::Value,
            #[serde(default, skip_serializing_if = "Option::is_none")]
            optional: Option<String>,
            #[serde(default, skip_serializing_if = "Vec::is_empty")]
            defaults: Vec<u32>,
            progress: crate::tools::types::ToolProgress,
        }
        let original = Header {
            sequence: u64::MAX,
            opaque: serde_json::json!({"signed": -17, "fraction": 1.5, "text": "水\n\\\""}),
            optional: None,
            defaults: Vec::new(),
            progress: crate::tools::types::ToolProgress {
                completed: Some(f64::NAN),
                ..Default::default()
            },
        };
        let mut unsafe_json = original.clone();
        unsafe_json.opaque = serde_json::json!({"integer": u64::MAX});
        let mut too_many = original.clone();
        too_many.defaults = vec![1; CHECKPOINT_PAGE_ITEMS + 1];
        let mut too_large = original.clone();
        too_large.optional = Some("x".repeat(CHECKPOINT_ITEM_BYTES));
        let cases = [original, unsafe_json, too_many, too_large];
        for budget in [
            0,
            3,
            4,
            16,
            256,
            1024,
            CHECKPOINT_ITEM_BYTES - 1,
            CHECKPOINT_ITEM_BYTES,
            32768,
            CHECKPOINT_TARGET_BYTES / 8,
        ] {
            for header in &cases {
                let mut actual = Capture::new(7);
                let mut oracle = Capture::new(7);
                actual.remaining = budget;
                oracle.remaining = budget;
                assert_eq!(
                    actual.item("none", &None::<Header>),
                    byte_oracle_item(&mut oracle, "none", &None::<Header>)
                );
                let value = actual.item("one", header);
                assert_eq!(value, byte_oracle_item(&mut oracle, "one", header));
                if let Some(value) = value {
                    assert_eq!(value.sequence, u64::MAX);
                    assert!(
                        value.progress.completed.is_none(),
                        "DTO deserialization still runs"
                    );
                }
                assert_eq!(
                    actual.page("rows", &cases),
                    byte_oracle_page(&mut oracle, "rows", &cases)
                );
                let repeated = vec![cases[0].clone(); CHECKPOINT_PAGE_ITEMS + 1];
                let loaded = actual.page("bounded_rows", &repeated);
                assert_eq!(
                    loaded,
                    byte_oracle_page(&mut oracle, "bounded_rows", &repeated)
                );
                if budget == CHECKPOINT_TARGET_BYTES / 8 {
                    assert_eq!(loaded.len(), CHECKPOINT_PAGE_ITEMS);
                    assert_eq!(actual.collections.last().unwrap().next_offset, Some(32));
                }
                assert_eq!(actual.remaining, oracle.remaining);
                assert_eq!(actual.collections, oracle.collections);
                assert_eq!(actual.content, oracle.content);
                assert_eq!(actual.evicted, oracle.evicted);
            }
        }
    }

    fn aggregate_checkpoint() -> RuntimeClientCheckpoint {
        let mut checkpoint = super::super::projection::RuntimeClientProjection::new(
            crate::runtime::identity::ConversationId::new(
                "conv_77621431-ad5b-7155-9eca-9f21014c0bc9",
            ),
            Vec::new(),
            serde_json::from_value(serde_json::json!({"configured_tools":[],"revision":1}))
                .unwrap(),
            None,
            64,
        )
        .checkpoint_candidate()
        .unwrap();
        let mut request: crate::runtime::interaction::InteractionRequest = serde_json::from_str(
            include_str!("../../tests/fixtures/runtime-client/review-v21.json"),
        )
        .unwrap();
        let crate::runtime::interaction::InteractionKind::Review { review, .. } = &mut request.kind
        else {
            panic!("native review fixture");
        };
        review.context = vec![serde_json::Value::String("x".repeat(14_000))];
        for ordinal in 0..CHECKPOINT_PAGE_ITEMS {
            request.id = crate::runtime::identity::InteractionId::new(format!("review-{ordinal}"));
            let mut header = interaction_request_header(
                &request,
                crate::runtime::interaction::InteractionSource::Primary,
            );
            header.source_offset = ordinal as u64;
            header.body = Some(request.clone());
            header.body_complete = true;
            assert!(
                bounded_bytes(&header, CHECKPOINT_ITEM_BYTES).is_some(),
                "individual header has {} actual JSON bytes",
                serde_json::to_vec(&header).unwrap().len()
            );
            checkpoint.snapshot.pending_interactions.push(header);
            checkpoint
                .snapshot
                .messages
                .push(crate::message::types::MessageBlock::Assistant(
                    crate::message::types::AssistantMessageBlock {
                        id: crate::runtime::identity::MessageId::new(format!("message-{ordinal}")),
                        content: vec![crate::message::types::AssistantContentBlock::Text(
                            crate::message::content::TextBlock {
                                text: "y".repeat(14_000),
                            },
                        )],
                    },
                ));
        }
        checkpoint.collections.push(CollectionFrontier {
            domain: "pending_interactions".into(),
            revision: 0,
            total: 32,
            loaded: 32,
            next_offset: None,
        });
        checkpoint
    }

    fn aggregate_texts(checkpoint: &mut RuntimeClientCheckpoint) -> Vec<&mut String> {
        let mut result = Vec::new();
        for header in &mut checkpoint.snapshot.pending_interactions {
            let crate::runtime::interaction::InteractionKind::Review { review, .. } =
                &mut header.body.as_mut().unwrap().kind
            else {
                panic!("native review fixture");
            };
            let serde_json::Value::String(text) = &mut review.context[0] else {
                panic!("native review text");
            };
            result.push(text);
        }
        for message in &mut checkpoint.snapshot.messages {
            let crate::message::types::MessageBlock::Assistant(message) = message else {
                panic!("native assistant fixture");
            };
            let crate::message::types::AssistantContentBlock::Text(text) = &mut message.content[0]
            else {
                panic!("native assistant text");
            };
            result.push(&mut text.text);
        }
        result
    }

    #[test]
    fn checkpoint_proof_matches_structural_eviction_and_bytes_at_the_aggregate_limit() {
        let limit = CHECKPOINT_TARGET_BYTES - CHECKPOINT_ITEM_BYTES;
        let schema = super::super::structural_json::root::<RuntimeClientCheckpoint>();
        let encode = |checkpoint: &RuntimeClientCheckpoint| {
            bounded_bytes(
                &super::super::structural_json::Encoded {
                    value: checkpoint,
                    schema: &schema,
                    root: &schema,
                },
                2 * CHECKPOINT_TARGET_BYTES,
            )
            .unwrap()
        };
        for wide in [false, true] {
            for delta in [-1_isize, 0, 1] {
                let mut original = aggregate_checkpoint();
                for text in aggregate_texts(&mut original) {
                    text.clear();
                }
                if wide {
                    original.read_cut.through = u64::MAX;
                    original.read_cut.revision = u64::MAX;
                    original
                        .read_cut
                        .domain_revisions
                        .insert("wide".into(), u64::MAX);
                    original.collections[0].total = u64::MAX;
                }
                let empty_budget = super::super::preflight::required_budget(
                    &super::super::structural_json::Encoded {
                        value: &original,
                        schema: &schema,
                        root: &schema,
                    },
                    usize::MAX,
                )
                .unwrap();
                let target = limit.checked_add_signed(delta).unwrap();
                let remaining = target.checked_sub(empty_budget).unwrap();
                let mut texts = aggregate_texts(&mut original);
                let count = texts.len();
                for (index, text) in texts.iter_mut().enumerate() {
                    **text = "x".repeat(remaining / count + usize::from(index < remaining % count));
                }
                drop(texts);
                for header in &original.snapshot.pending_interactions {
                    assert!(bounded_bytes(header, CHECKPOINT_ITEM_BYTES).is_some());
                }
                let mut expected = original.clone();
                let mut actual = original;
                let legacy = super::super::preflight::with_structural_admission_only(|| {
                    fit_checkpoint(&mut expected)
                });
                let candidate = fit_checkpoint(&mut actual);
                assert_eq!(legacy.unwrap(), delta > 0);
                assert_eq!(candidate.unwrap(), delta > 0);
                assert_eq!(actual, expected, "wide={wide}, delta={delta}");
                assert_eq!(encode(&actual), encode(&expected));
                if wide {
                    assert!(
                        String::from_utf8(encode(&actual))
                            .unwrap()
                            .contains("\"18446744073709551615\"")
                    );
                }
            }
        }
        let mut expected = aggregate_checkpoint();
        expected
            .read_cut
            .domain_revisions
            .insert("x".repeat(CHECKPOINT_TARGET_BYTES), 1);
        let mut actual = expected.clone();
        let legacy = super::super::preflight::with_structural_admission_only(|| {
            fit_checkpoint(&mut expected)
        });
        let candidate = fit_checkpoint(&mut actual);
        assert_eq!(legacy.unwrap_err(), candidate.unwrap_err());
        assert_eq!(actual, expected);
        assert_eq!(encode(&actual), encode(&expected));
    }

    #[test]
    fn t14_t15_aggregate_budget_defers_bodies_without_losing_actionable_headers_or_cut() {
        let mut checkpoint = aggregate_checkpoint();
        let before = checkpoint.read_cut.clone();
        let identities: Vec<_> = checkpoint
            .snapshot
            .pending_interactions
            .iter()
            .map(|row| {
                (
                    row.interaction.clone(),
                    row.source_offset,
                    row.workflow_node.clone(),
                    row.workflow_node_deferred,
                    row.workflow_node_path.clone(),
                )
            })
            .collect();
        assert!(
            fit_checkpoint(&mut checkpoint).unwrap(),
            "aggregate bytes require one explicit cache replacement"
        );
        assert_eq!(checkpoint.read_cut, before);
        assert_eq!(
            checkpoint
                .snapshot
                .pending_interactions
                .iter()
                .map(|row| (
                    row.interaction.clone(),
                    row.source_offset,
                    row.workflow_node.clone(),
                    row.workflow_node_deferred,
                    row.workflow_node_path.clone()
                ))
                .collect::<Vec<_>>(),
            identities
        );
        assert!(
            checkpoint
                .snapshot
                .pending_interactions
                .iter()
                .all(|row| row.body.is_none() && !row.body_complete)
        );
        assert_eq!(
            checkpoint.snapshot.messages.len(),
            32,
            "canonical loaded bodies fit after deferring request bodies"
        );
        assert_eq!(
            checkpoint
                .collections
                .iter()
                .filter(
                    |frontier| frontier.domain.starts_with("pending_interactions.")
                        && frontier.loaded == 0
                        && frontier.next_offset == Some(0)
                )
                .count(),
            32
        );
        let schema = super::super::structural_json::root::<RuntimeClientCheckpoint>();
        assert!(
            bounded_bytes(
                &super::super::structural_json::Encoded {
                    value: &checkpoint,
                    schema: &schema,
                    root: &schema
                },
                CHECKPOINT_TARGET_BYTES - CHECKPOINT_ITEM_BYTES
            )
            .is_some()
        );
        assert!(
            !fit_checkpoint(&mut checkpoint).unwrap(),
            "the same bounded cut does not resync again"
        );
    }
}

#[cfg(test)]
mod prepared_trace_tests;

#[cfg(test)]
mod incremental_capture_tests;

#[cfg(test)]
mod live_capture_tests;

#[cfg(test)]
mod metadata_fragment_tests;
