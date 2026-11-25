//! The closed `item/tool/result/*` envelopes and incremental verifier.
//!
//! Upload record budgets are independent of the encoded transport frame limit
//! and the callback broker's admission and lifecycle limits.

use std::fmt;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::protocol::{JsonRpcVersion, RequestId};
use crate::runtime::identity::{AttemptId, ConversationId, ToolCallId};

/// The only type tag this contract defines: one UTF-8 text item.
pub const TYPE_TAG_INPUT_TEXT: u16 = 0x0001;
/// At most this many items per result. Each item is one `HostToolContent`.
pub const MAX_ITEMS: usize = 1024;
/// At most this many raw bytes in one chunk.
pub const MAX_CHUNK_BYTES: usize = 64 * 1024;
/// At most this many raw bytes across all items of one result.
pub const MAX_TOTAL_BYTES: u64 = 16 * 1024 * 1024;

/// A `u64` carried as a canonical decimal string: no sign, no leading zero.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct LosslessU64(u64);

impl LosslessU64 {
    /// The numeric value.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl TryFrom<String> for LosslessU64 {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let canonical = value == "0"
            || (!value.is_empty()
                && !value.starts_with('0')
                && value.bytes().all(|byte| byte.is_ascii_digit()));
        if !canonical {
            return Err(String::from("u64 must be a canonical decimal string"));
        }
        value
            .parse::<u64>()
            .map(Self)
            .map_err(|_| String::from("u64 out of range"))
    }
}

impl From<LosslessU64> for String {
    fn from(value: LosslessU64) -> Self {
        value.0.to_string()
    }
}

/// A SHA-256 digest carried as 64 lowercase hex digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256Hex([u8; 32]);

impl Sha256Hex {
    pub(crate) fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl TryFrom<String> for Sha256Hex {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(String::from("sha256 must be 64 lowercase hex digits"));
        }
        let mut out = [0u8; 32];
        for (slot, pair) in out.iter_mut().zip(value.as_bytes().chunks(2)) {
            let text = std::str::from_utf8(pair).map_err(|_| String::from("invalid hex"))?;
            *slot = u8::from_str_radix(text, 16).map_err(|_| String::from("invalid hex"))?;
        }
        Ok(Self(out))
    }
}

impl From<Sha256Hex> for String {
    fn from(value: Sha256Hex) -> Self {
        use fmt::Write as _;
        let mut out = String::with_capacity(64);
        for byte in value.0 {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }
}

/// An upload identifier: a lowercase canonical UUID string.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct UploadId(String);

impl TryFrom<String> for UploadId {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let shaped = value.len() == 36
            && value.bytes().enumerate().all(|(index, byte)| match index {
                8 | 13 | 18 | 23 => byte == b'-',
                _ => byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte),
            });
        if shaped {
            Ok(Self(value))
        } else {
            Err(String::from("upload id must be a lowercase UUID"))
        }
    }
}

impl From<UploadId> for String {
    fn from(value: UploadId) -> Self {
        value.0
    }
}

/// A bounded opaque token: `[A-Za-z0-9._-]`, 1–128 bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct OpaqueToken(String);

impl TryFrom<String> for OpaqueToken {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        let bounded = !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'));
        if bounded {
            Ok(Self(value))
        } else {
            Err(String::from("invalid opaque token"))
        }
    }
}

impl From<OpaqueToken> for String {
    fn from(value: OpaqueToken) -> Self {
        value.0
    }
}

impl OpaqueToken {
    pub(crate) fn as_str(&self) -> &str {
        &self.0
    }
}

/// Address supplied to a single recipient of an item/tool/call request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostUploadScope {
    /// Incarnation of the callback broker.
    pub callback_incarnation: String,
    /// Incarnation of the receiving Host connection.
    pub connection_incarnation: String,
    /// Incarnation of the Host's subscription to the conversation.
    pub subscription_incarnation: String,
}

/// The identity one upload candidate belongs to. The Server callback
/// incarnation, original request identifier, and tool call identity are the
/// correlation; the Host subscription incarnation and upload identifier bind
/// the candidate to one eligible participant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostUploadCandidate {
    /// The Server callback incarnation that issued the original request.
    #[schemars(with = "String")]
    #[schemars(regex(pattern = "^[A-Za-z0-9._-]{1,128}$"))]
    pub callback_incarnation: OpaqueToken,
    /// The original request identifier.
    pub request_id: RequestId,
    /// The conversation the tool call belongs to.
    pub thread_id: ConversationId,
    /// The attempt the tool call belongs to.
    pub turn_id: AttemptId,
    /// The tool call being answered.
    pub call_id: ToolCallId,
    /// The callback connection incarnation the request was delivered on.
    #[schemars(with = "String")]
    #[schemars(regex(pattern = "^[A-Za-z0-9._-]{1,128}$"))]
    pub connection_incarnation: OpaqueToken,
    /// The selected subscription the request was delivered to.
    #[schemars(with = "String")]
    #[schemars(regex(pattern = "^[A-Za-z0-9._-]{1,128}$"))]
    pub subscription_incarnation: OpaqueToken,
    /// The upload identifier, unique per candidate.
    #[schemars(with = "String")]
    #[schemars(regex(pattern = "^[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}$"))]
    pub upload_id: UploadId,
}

/// One item descriptor: its type tag and declared byte length.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostUploadItem {
    /// The item's type tag; only [`TYPE_TAG_INPUT_TEXT`] is defined.
    #[schemars(range(min = 1, max = 1))]
    pub type_tag: u16,
    /// The declared byte length of this item.
    #[schemars(with = "String")]
    #[schemars(regex(pattern = "^(0|[1-9][0-9]{0,19})$"))]
    pub length: LosslessU64,
}

/// `item/tool/result/begin`: captures the immutable success and descriptors.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostUploadBeginParams {
    /// The candidate being begun.
    pub candidate: HostUploadCandidate,
    /// The tool result's success flag, fixed at begin.
    pub success: bool,
    /// The ordered item descriptors, at most [`MAX_ITEMS`].
    #[schemars(length(max = 1024))]
    pub items: Vec<HostUploadItem>,
}

/// `item/tool/result/chunk`: one ordered, bounded chunk of one item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostUploadChunkParams {
    /// The candidate the chunk feeds.
    pub candidate: HostUploadCandidate,
    /// The current item's index.
    pub item_index: u32,
    /// The strictly increasing chunk sequence, starting at `1`.
    #[schemars(with = "String")]
    #[schemars(regex(pattern = "^(0|[1-9][0-9]{0,19})$"))]
    pub sequence: LosslessU64,
    /// The byte offset within the item.
    #[schemars(with = "String")]
    #[schemars(regex(pattern = "^(0|[1-9][0-9]{0,19})$"))]
    pub offset: LosslessU64,
    /// The raw bytes, canonical padded base64, at most [`MAX_CHUNK_BYTES`].
    #[schemars(length(max = 87384))]
    pub data_base64: String,
    /// The SHA-256 of the decoded bytes.
    #[schemars(with = "String")]
    #[schemars(regex(pattern = "^[0-9a-f]{64}$"))]
    pub sha256: Sha256Hex,
}

/// `item/tool/result/end`: echoes the begin metadata and closes the candidate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostUploadEndParams {
    /// The candidate being ended.
    pub candidate: HostUploadCandidate,
    /// The success flag, which must equal the begin's.
    pub success: bool,
    /// The item descriptors, which must equal the begin's.
    #[schemars(length(max = 1024))]
    pub items: Vec<HostUploadItem>,
    /// The declared total length, which must equal the sum of the items.
    #[schemars(with = "String")]
    #[schemars(regex(pattern = "^(0|[1-9][0-9]{0,19})$"))]
    pub total: LosslessU64,
    /// The result digest over the canonical layout.
    #[schemars(with = "String")]
    #[schemars(regex(pattern = "^[0-9a-f]{64}$"))]
    pub digest: Sha256Hex,
}

/// `item/tool/result/abort`: retires the candidate without a result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HostUploadAbortParams {
    /// The candidate being aborted.
    pub candidate: HostUploadCandidate,
}

/// One closed upload request, tagged by its JSON-RPC method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "method", content = "params", deny_unknown_fields)]
pub enum HostUploadMethod {
    /// Begins a candidate.
    #[serde(rename = "item/tool/result/begin")]
    Begin(HostUploadBeginParams),
    /// Appends one chunk.
    #[serde(rename = "item/tool/result/chunk")]
    Chunk(HostUploadChunkParams),
    /// Ends and verifies the candidate.
    #[serde(rename = "item/tool/result/end")]
    End(HostUploadEndParams),
    /// Aborts the candidate.
    #[serde(rename = "item/tool/result/abort")]
    Abort(HostUploadAbortParams),
}

/// The complete upload envelope: JSON-RPC version, correlation identifier, and
/// one closed method.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct HostUploadEnvelope {
    /// The JSON-RPC version.
    pub jsonrpc: JsonRpcVersion,
    /// The correlation identifier of this request.
    pub id: RequestId,
    /// The method and its parameters.
    #[serde(flatten)]
    pub call: HostUploadMethod,
}

impl HostUploadMethod {
    pub(crate) fn is_method(method: &str) -> bool {
        matches!(
            method,
            "item/tool/result/begin"
                | "item/tool/result/chunk"
                | "item/tool/result/end"
                | "item/tool/result/abort"
        )
    }

    pub(crate) fn candidate(&self) -> &HostUploadCandidate {
        match self {
            Self::Begin(params) => &params.candidate,
            Self::Chunk(params) => &params.candidate,
            Self::End(params) => &params.candidate,
            Self::Abort(params) => &params.candidate,
        }
    }
}

/// Why the broker or incremental verifier refused an upload step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "kebab-case")]
pub enum HostUploadRefusal {
    /// A receive slot limit was reached.
    Capacity,
    /// The fixed total or idle deadline ran out.
    Expired,
    /// A type tag is not defined by this contract.
    UnknownTag,
    /// A type tag is reserved and never valid.
    ReservedTag,
    /// More than [`MAX_ITEMS`] descriptors.
    TooManyItems,
    /// The declared total exceeds [`MAX_TOTAL_BYTES`] or overflows.
    TotalTooLarge,
    /// A chunk is empty, oversize, or not canonical padded base64.
    InvalidEncoding,
    /// The chunk digest or the end digest does not match.
    DigestMismatch,
    /// A chunk skipped ahead of the next sequence.
    Gap,
    /// A chunk's item or offset does not follow the bytes received.
    OutOfOrder,
    /// A replayed chunk older than the one-chunk evidence window.
    DuplicateOutsideWindow,
    /// A replay of the last sequence with different identity, or an overflow.
    Conflict,
    /// A chunk names an item that does not exist.
    Foreign,
    /// The candidate already ended or aborted.
    Late,
    /// An item ends inside a UTF-8 scalar, or the bytes are not UTF-8.
    InvalidUtf8,
    /// The end arrived before every item was complete.
    Incomplete,
    /// The end's success or descriptors differ from the begin's.
    MetadataMismatch,
}

impl fmt::Display for HostUploadRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{self:?}")
    }
}

/// The outcome of one accepted or replayed chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkAck {
    /// The chunk was appended; the next sequence is the one after it.
    Accepted,
    /// An identical replay of the last chunk: nothing was appended.
    Duplicate,
}

/// The trusted identity a slot admits. Every field is supplied by the real
/// pending request and recipient, never by the caller's assertion.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostUploadExpected {
    /// The Server callback incarnation that issued the request.
    pub callback_incarnation: OpaqueToken,
    /// The callback connection incarnation the request was delivered on.
    pub connection_incarnation: OpaqueToken,
    /// The selected subscription the request was delivered to.
    pub subscription_incarnation: OpaqueToken,
    /// The original request identifier.
    pub request_id: RequestId,
    /// The conversation of the tool call.
    pub thread_id: ConversationId,
    /// The attempt of the tool call.
    pub turn_id: AttemptId,
    /// The tool call.
    pub call_id: ToolCallId,
    /// The upload identifier this slot accepts.
    pub upload_id: UploadId,
}

impl HostUploadExpected {
    fn matches(&self, candidate: &HostUploadCandidate) -> bool {
        self.callback_incarnation == candidate.callback_incarnation
            && self.connection_incarnation == candidate.connection_incarnation
            && self.subscription_incarnation == candidate.subscription_incarnation
            && self.request_id == candidate.request_id
            && self.thread_id == candidate.thread_id
            && self.turn_id == candidate.turn_id
            && self.call_id == candidate.call_id
            && self.upload_id == candidate.upload_id
    }
}

/// A complete, verified End. Only [`HostUploadSlot::end`] constructs it, so a
/// claim cannot be fed a hand-built or unverified result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidatedEnd {
    candidate: HostUploadCandidate,
    success: bool,
    items: Vec<HostUploadItem>,
    total: u64,
    digest: [u8; 32],
    _sealed: (),
}

impl ValidatedEnd {
    /// The candidate identity the End verified under.
    #[must_use]
    pub const fn candidate(&self) -> &HostUploadCandidate {
        &self.candidate
    }

    /// The verified success flag.
    #[must_use]
    pub const fn success(&self) -> bool {
        self.success
    }

    /// The verified immutable descriptors.
    #[must_use]
    pub fn items(&self) -> &[HostUploadItem] {
        &self.items
    }

    /// The verified declared total.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.total
    }

    /// The verified digest.
    #[must_use]
    pub const fn digest(&self) -> &[u8; 32] {
        &self.digest
    }
}

/// One scoped upload slot for one trusted context: one active candidate, every
/// step checked against the trusted identity before any state changes.
#[derive(Debug, Clone)]
pub struct HostUploadSlot {
    expected: HostUploadExpected,
    active: Option<(HostUploadCandidate, HostUploadVerifier)>,
}

impl HostUploadSlot {
    /// A slot for exactly the trusted identity `expected`.
    #[must_use]
    pub const fn new(expected: HostUploadExpected) -> Self {
        Self {
            expected,
            active: None,
        }
    }

    fn active_for(
        &mut self,
        candidate: &HostUploadCandidate,
    ) -> Result<&mut HostUploadVerifier, HostUploadRefusal> {
        if !self.expected.matches(candidate) {
            return Err(HostUploadRefusal::Foreign);
        }
        match &mut self.active {
            Some((active, verifier)) if active == candidate => Ok(verifier),
            _ => Err(HostUploadRefusal::Foreign),
        }
    }

    /// Begins the one candidate of this slot. An identical begin with the same
    /// metadata replays; any other begin conflicts.
    ///
    /// # Errors
    ///
    /// [`HostUploadRefusal::Foreign`] for a foreign identity, otherwise the
    /// verifier's descriptor refusal or [`HostUploadRefusal::Conflict`].
    pub fn begin(
        &mut self,
        candidate: HostUploadCandidate,
        success: bool,
        items: &[HostUploadItem],
    ) -> Result<(), HostUploadRefusal> {
        if !self.expected.matches(&candidate) {
            return Err(HostUploadRefusal::Foreign);
        }
        if let Some((active, verifier)) = &self.active {
            if verifier.closed {
                return Err(HostUploadRefusal::Late);
            }
            return if *active == candidate && verifier.same_metadata(success, items) {
                Ok(())
            } else {
                Err(HostUploadRefusal::Conflict)
            };
        }
        let verifier = HostUploadVerifier::begin(success, items)?;
        self.active = Some((candidate, verifier));
        Ok(())
    }

    /// Feeds one chunk to the active candidate.
    ///
    /// # Errors
    ///
    /// [`HostUploadRefusal::Foreign`] for a foreign candidate (before any state
    /// change), otherwise the verifier's refusal.
    pub fn chunk(
        &mut self,
        candidate: &HostUploadCandidate,
        item: u32,
        sequence: u64,
        offset: u64,
        data: &[u8],
        sha256: &[u8; 32],
    ) -> Result<ChunkAck, HostUploadRefusal> {
        self.active_for(candidate)?
            .chunk(item, sequence, offset, data, sha256)
    }

    /// Ends the active candidate and returns the opaque verified evidence.
    ///
    /// # Errors
    ///
    /// [`HostUploadRefusal::Foreign`] for a foreign candidate, otherwise the
    /// verifier's refusal.
    pub fn end(
        &mut self,
        candidate: &HostUploadCandidate,
        success: bool,
        items: &[HostUploadItem],
        total: u64,
        digest: &[u8; 32],
    ) -> Result<ValidatedEnd, HostUploadRefusal> {
        let verified = self
            .active_for(candidate)?
            .end(success, items, total, digest)?;
        Ok(ValidatedEnd {
            candidate: candidate.clone(),
            success,
            items: items.to_vec(),
            total,
            digest: verified,
            _sealed: (),
        })
    }

    /// Aborts the active candidate; nothing is ever published from it.
    ///
    /// # Errors
    ///
    /// [`HostUploadRefusal::Foreign`] for a foreign candidate, otherwise the
    /// verifier's refusal.
    pub fn abort(&mut self, candidate: &HostUploadCandidate) -> Result<(), HostUploadRefusal> {
        self.active_for(candidate)?.abort()
    }
}

/// Decodes one chunk's canonical padded base64 at the wire boundary. The
/// encoded length is bounded before decoding, and the decode must re-encode to
/// exactly the received text.
///
/// # Errors
///
/// [`HostUploadRefusal::InvalidEncoding`] for anything else.
pub fn decode_chunk_data(encoded: &str) -> Result<Vec<u8>, HostUploadRefusal> {
    let engine = base64::engine::general_purpose::STANDARD;
    if encoded.len() > MAX_CHUNK_BYTES.div_ceil(3) * 4 {
        return Err(HostUploadRefusal::InvalidEncoding);
    }
    let bytes = engine
        .decode(encoded)
        .map_err(|_| HostUploadRefusal::InvalidEncoding)?;
    if bytes.is_empty() || bytes.len() > MAX_CHUNK_BYTES || engine.encode(&bytes) != encoded {
        return Err(HostUploadRefusal::InvalidEncoding);
    }
    Ok(bytes)
}

/// The canonical result digest, computed directly from the full item list. The
/// verifier must agree with this for every accepted sequence of chunks.
#[must_use]
pub fn canonical_digest(items: &[(u16, &[u8])]) -> [u8; 32] {
    let mut digest = Sha256::new();
    digest.update(u32::try_from(items.len()).unwrap_or(u32::MAX).to_le_bytes());
    for (tag, bytes) in items {
        digest.update(tag.to_le_bytes());
        digest.update(u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_le_bytes());
        digest.update(bytes);
    }
    let total: u64 = items
        .iter()
        .map(|(_, bytes)| u64::try_from(bytes.len()).unwrap_or(u64::MAX))
        .sum();
    digest.update(total.to_le_bytes());
    digest.finalize().into()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LastChunk {
    sequence: u64,
    item: usize,
    offset: u64,
    length: u64,
    sha256: [u8; 32],
}

/// The pure verifier of one candidate. It keeps one incremental digest, the
/// current item cursor, up to three incomplete UTF-8 bytes, and the last chunk's
/// identity. It never retains the content.
#[derive(Debug, Clone)]
pub struct HostUploadVerifier {
    success: bool,
    items: Vec<HostUploadItem>,
    lengths: Vec<u64>,
    total: u64,
    index: usize,
    offset: u64,
    headers: usize,
    received: u64,
    next_sequence: u64,
    digest: Sha256,
    pending: [u8; 3],
    pending_len: usize,
    last: Option<LastChunk>,
    closed: bool,
}

impl HostUploadVerifier {
    /// Begins a candidate, validating every descriptor and the bounded total.
    ///
    /// # Errors
    ///
    /// The first descriptor or budget refusal, in order.
    pub fn begin(success: bool, items: &[HostUploadItem]) -> Result<Self, HostUploadRefusal> {
        if items.len() > MAX_ITEMS {
            return Err(HostUploadRefusal::TooManyItems);
        }
        let mut lengths = Vec::with_capacity(items.len());
        let mut total: u64 = 0;
        for item in items {
            match item.type_tag {
                TYPE_TAG_INPUT_TEXT => {}
                0 => return Err(HostUploadRefusal::ReservedTag),
                _ => return Err(HostUploadRefusal::UnknownTag),
            }
            let length = item.length.get();
            total = total
                .checked_add(length)
                .filter(|total| *total <= MAX_TOTAL_BYTES)
                .ok_or(HostUploadRefusal::TotalTooLarge)?;
            lengths.push(length);
        }
        let mut digest = Sha256::new();
        digest.update(u32::try_from(items.len()).unwrap_or(u32::MAX).to_le_bytes());
        let mut verifier = Self {
            success,
            items: items.to_vec(),
            lengths,
            total,
            index: 0,
            offset: 0,
            headers: 0,
            received: 0,
            next_sequence: 1,
            digest,
            pending: [0; 3],
            pending_len: 0,
            last: None,
            closed: false,
        };
        verifier.skip_empty_items();
        Ok(verifier)
    }

    /// Advances the cursor past every zero-length item, so empty items carry
    /// headers but never bytes, and the cursor always names a non-empty item.
    fn skip_empty_items(&mut self) {
        while self.index < self.lengths.len() && self.lengths[self.index] == 0 {
            self.index += 1;
        }
    }

    /// Feeds one chunk. Refusals never change the state.
    ///
    /// # Errors
    ///
    /// The first refusal in the documented order.
    pub fn chunk(
        &mut self,
        item: u32,
        sequence: u64,
        offset: u64,
        data: &[u8],
        sha256: &[u8; 32],
    ) -> Result<ChunkAck, HostUploadRefusal> {
        if self.closed {
            return Err(HostUploadRefusal::Late);
        }
        if data.is_empty() || data.len() > MAX_CHUNK_BYTES {
            return Err(HostUploadRefusal::InvalidEncoding);
        }
        if Sha256::digest(data).as_slice() != sha256 {
            return Err(HostUploadRefusal::DigestMismatch);
        }
        let item = usize::try_from(item).map_err(|_| HostUploadRefusal::Foreign)?;
        let length = u64::try_from(data.len()).map_err(|_| HostUploadRefusal::InvalidEncoding)?;
        let candidate = LastChunk {
            sequence,
            item,
            offset,
            length,
            sha256: *sha256,
        };
        if let Some(last) = self.last
            && last.sequence == sequence
        {
            return if last == candidate {
                Ok(ChunkAck::Duplicate)
            } else {
                Err(HostUploadRefusal::Conflict)
            };
        }
        if sequence < self.next_sequence {
            return Err(HostUploadRefusal::DuplicateOutsideWindow);
        }
        if sequence > self.next_sequence {
            return Err(HostUploadRefusal::Gap);
        }
        if item >= self.lengths.len() {
            return Err(HostUploadRefusal::Foreign);
        }
        if item != self.index || offset != self.offset {
            return Err(HostUploadRefusal::OutOfOrder);
        }
        let remaining = self.lengths[item] - self.offset;
        if length > remaining {
            return Err(HostUploadRefusal::Conflict);
        }
        let (pending, pending_len) = utf8_tail(&self.pending[..self.pending_len], data)?;
        let completes = length == remaining;
        if completes && pending_len != 0 {
            return Err(HostUploadRefusal::InvalidUtf8);
        }
        let next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(HostUploadRefusal::Conflict)?;
        self.write_headers_through(item);
        self.digest.update(data);
        self.received += length;
        self.next_sequence = next_sequence;
        self.pending = pending;
        self.pending_len = pending_len;
        self.last = Some(candidate);
        if completes {
            self.index += 1;
            self.offset = 0;
            self.skip_empty_items();
        } else {
            self.offset += length;
        }
        Ok(ChunkAck::Accepted)
    }

    /// Ends the candidate. The end must echo the begin's success and
    /// descriptors, every item must be complete, and the digest must match.
    ///
    /// # Errors
    ///
    /// The first refusal in the documented order.
    pub fn end(
        &mut self,
        success: bool,
        items: &[HostUploadItem],
        total: u64,
        digest: &[u8; 32],
    ) -> Result<[u8; 32], HostUploadRefusal> {
        if self.closed {
            return Err(HostUploadRefusal::Late);
        }
        if success != self.success || items != self.items.as_slice() || total != self.total {
            return Err(HostUploadRefusal::MetadataMismatch);
        }
        if self.index != self.lengths.len() || self.received != self.total {
            return Err(HostUploadRefusal::Incomplete);
        }
        // Finish on a copy: a digest mismatch must leave the verifier unchanged.
        let mut finished = self.digest.clone();
        let mut headers = self.headers;
        while headers < self.lengths.len() {
            finished.update(self.items[headers].type_tag.to_le_bytes());
            finished.update(self.lengths[headers].to_le_bytes());
            headers += 1;
        }
        finished.update(total.to_le_bytes());
        let computed: [u8; 32] = finished.clone().finalize().into();
        if &computed != digest {
            return Err(HostUploadRefusal::DigestMismatch);
        }
        self.digest = finished;
        self.headers = headers;
        self.closed = true;
        Ok(computed)
    }

    /// Aborts the candidate. Nothing is ever published from an aborted candidate.
    ///
    /// # Errors
    ///
    /// [`HostUploadRefusal::Late`] when it already ended or aborted.
    pub fn abort(&mut self) -> Result<(), HostUploadRefusal> {
        if self.closed {
            return Err(HostUploadRefusal::Late);
        }
        self.closed = true;
        Ok(())
    }

    fn same_metadata(&self, success: bool, items: &[HostUploadItem]) -> bool {
        self.success == success && self.items == items
    }

    /// Hashes the canonical headers of every item up to and including `through`,
    /// in order, so each header precedes its own bytes.
    fn write_headers_through(&mut self, through: usize) {
        while self.headers <= through && self.headers < self.lengths.len() {
            let header = self.items[self.headers];
            self.digest.update(header.type_tag.to_le_bytes());
            self.digest.update(self.lengths[self.headers].to_le_bytes());
            self.headers += 1;
        }
    }
}

/// Incremental normalized UTF-8 for one text item: the combined pending and new
/// bytes must be valid up to an incomplete scalar of at most three bytes.
fn utf8_tail(pending: &[u8], bytes: &[u8]) -> Result<([u8; 3], usize), HostUploadRefusal> {
    let joined: Vec<u8> = pending.iter().chain(bytes).copied().collect();
    match std::str::from_utf8(&joined) {
        Ok(_) => Ok(([0; 3], 0)),
        Err(error) if error.error_len().is_none() => {
            let tail = &joined[error.valid_up_to()..];
            if tail.len() > 3 {
                return Err(HostUploadRefusal::InvalidUtf8);
            }
            let mut out = [0u8; 3];
            out[..tail.len()].copy_from_slice(tail);
            Ok((out, tail.len()))
        }
        Err(_) => Err(HostUploadRefusal::InvalidUtf8),
    }
}

/// Frozen examples of the future upload carrier; these are intentionally not
/// members of the currently admitted management message union.
#[must_use]
pub fn fixtures() -> Vec<HostUploadEnvelope> {
    let candidate = HostUploadCandidate {
        callback_incarnation: OpaqueToken("server-1".into()),
        connection_incarnation: OpaqueToken("host-1".into()),
        subscription_incarnation: OpaqueToken("subscription-1".into()),
        thread_id: ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e19"),
        turn_id: AttemptId::new("turn-1"),
        call_id: ToolCallId::new("call-1"),
        request_id: RequestId::String("server-1:42".into()),
        upload_id: UploadId("00000000-0000-4000-8000-000000000042".into()),
    };
    let bytes = b"receipt\n";
    let items = vec![HostUploadItem {
        type_tag: TYPE_TAG_INPUT_TEXT,
        length: LosslessU64(8),
    }];
    let calls = [
        HostUploadMethod::Begin(HostUploadBeginParams {
            candidate: candidate.clone(),
            success: true,
            items: items.clone(),
        }),
        HostUploadMethod::Chunk(HostUploadChunkParams {
            candidate: candidate.clone(),
            item_index: 0,
            sequence: LosslessU64(1),
            offset: LosslessU64(0),
            data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
            sha256: Sha256Hex(Sha256::digest(bytes).into()),
        }),
        HostUploadMethod::End(HostUploadEndParams {
            candidate: candidate.clone(),
            success: true,
            items,
            total: LosslessU64(8),
            digest: Sha256Hex(canonical_digest(&[(TYPE_TAG_INPUT_TEXT, bytes)])),
        }),
        HostUploadMethod::Abort(HostUploadAbortParams { candidate }),
    ];
    calls
        .into_iter()
        .zip(1..)
        .map(|(call, id)| HostUploadEnvelope {
            jsonrpc: JsonRpcVersion::V2,
            id: RequestId::Integer(id),
            call,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn upload_fixtures_round_trip_as_public_requests() {
        use crate::app_server::protocol::ProtocolMessage;

        for envelope in fixtures() {
            let json = serde_json::to_vec(&envelope).unwrap();
            assert_eq!(
                serde_json::from_slice::<HostUploadEnvelope>(&json).unwrap(),
                envelope
            );
            assert_eq!(
                serde_json::from_slice::<ProtocolMessage>(&json).unwrap(),
                ProtocolMessage::HostUpload(Box::new(envelope))
            );
        }
    }

    /// Independently computed by the canonical layout over the named example:
    /// items `"héllo"`, `""`, `" wörld"`, all type tag `0x0001`.
    const NAMED_DIGEST: &str = "b27b2e4baeffa699b1e15bfb05e987ab37e6a07f77c8b0c28cef9c58c45854f8";

    fn item(length: u64) -> HostUploadItem {
        HostUploadItem {
            type_tag: TYPE_TAG_INPUT_TEXT,
            length: LosslessU64::try_from(length.to_string()).unwrap(),
        }
    }

    fn sha(bytes: &[u8]) -> [u8; 32] {
        Sha256::digest(bytes).into()
    }

    fn named_items() -> Vec<HostUploadItem> {
        vec![item(6), item(0), item(7)]
    }

    /// The named example fed as split chunks: one UTF-8 scalar is split across
    /// chunks inside item 0 and item 2, and item 1 is empty.
    fn feed_named(verifier: &mut HostUploadVerifier) {
        let chunks: [(u32, u64, u64, &[u8]); 6] = [
            (0, 1, 0, b"h"),
            (0, 2, 1, &[0xC3]),
            (0, 3, 2, &[0xA9, b'l', b'l', b'o']),
            (2, 4, 0, b" w"),
            (2, 5, 2, &[0xC3]),
            (2, 6, 3, &[0xB6, b'r', b'l', b'd']),
        ];
        for (index, sequence, offset, bytes) in chunks {
            verifier
                .chunk(index, sequence, offset, bytes, &sha(bytes))
                .unwrap();
        }
    }

    #[test]
    fn the_named_example_digest_matches_the_independent_layout() {
        let mut verifier = HostUploadVerifier::begin(true, &named_items()).unwrap();
        feed_named(&mut verifier);
        let digest = verifier
            .end(true, &named_items(), 13, &sha(b""))
            .unwrap_err();
        // The wrong digest is refused; the independent one is accepted.
        assert_eq!(digest, HostUploadRefusal::DigestMismatch);
        let expected = decode_hex(NAMED_DIGEST);
        let mut verifier = HostUploadVerifier::begin(true, &named_items()).unwrap();
        feed_named(&mut verifier);
        assert_eq!(
            verifier.end(true, &named_items(), 13, &expected).unwrap(),
            expected
        );
        assert_eq!(
            canonical_digest(&[(1, "héllo".as_bytes()), (1, b""), (1, " wörld".as_bytes())]),
            expected
        );
    }

    #[test]
    fn headers_are_interleaved_per_item_not_concatenated_before_data() {
        // A layout that concatenated all headers before all data would differ.
        let items: [(u16, &[u8]); 2] = [(1, b"ab"), (1, b"cd")];
        let interleaved = canonical_digest(&items);
        let mut wrong = Sha256::new();
        wrong.update(2u32.to_le_bytes());
        wrong.update(1u16.to_le_bytes());
        wrong.update(2u64.to_le_bytes());
        wrong.update(1u16.to_le_bytes());
        wrong.update(2u64.to_le_bytes());
        wrong.update(b"abcd");
        wrong.update(4u64.to_le_bytes());
        let wrong: [u8; 32] = wrong.finalize().into();
        assert_ne!(interleaved, wrong);
        let items_desc = vec![item(2), item(2)];
        let mut verifier = HostUploadVerifier::begin(true, &items_desc).unwrap();
        verifier.chunk(0, 1, 0, b"ab", &sha(b"ab")).unwrap();
        verifier.chunk(1, 2, 0, b"cd", &sha(b"cd")).unwrap();
        assert_eq!(
            verifier.end(true, &items_desc, 4, &interleaved).unwrap(),
            interleaved
        );
    }

    #[test]
    fn a_replayed_last_chunk_is_idempotent_and_older_or_conflicting_replays_are_refused() {
        let items = vec![item(4)];
        let mut verifier = HostUploadVerifier::begin(true, &items).unwrap();
        verifier.chunk(0, 1, 0, b"ab", &sha(b"ab")).unwrap();
        assert_eq!(
            verifier.chunk(0, 1, 0, b"ab", &sha(b"ab")).unwrap(),
            ChunkAck::Duplicate
        );
        assert_eq!(
            verifier.chunk(0, 1, 0, b"zz", &sha(b"zz")),
            Err(HostUploadRefusal::Conflict)
        );
        verifier.chunk(0, 2, 2, b"cd", &sha(b"cd")).unwrap();
        assert_eq!(
            verifier.chunk(0, 1, 0, b"ab", &sha(b"ab")),
            Err(HostUploadRefusal::DuplicateOutsideWindow)
        );
    }

    #[test]
    fn utf8_split_within_an_item_is_accepted_and_an_invalid_scalar_is_refused() {
        // A lone continuation byte is invalid as soon as it arrives.
        let items = vec![item(3)];
        let mut verifier = HostUploadVerifier::begin(true, &items).unwrap();
        assert_eq!(
            verifier.chunk(0, 1, 0, &[0xA9, b'x', b'y'], &sha(&[0xA9, b'x', b'y'])),
            Err(HostUploadRefusal::InvalidUtf8)
        );
        // An incomplete scalar may be carried across chunks inside the item.
        let mut split = HostUploadVerifier::begin(true, &[item(3)]).unwrap();
        assert_eq!(
            split
                .chunk(0, 1, 0, &[b'a', 0xC3], &sha(&[b'a', 0xC3]))
                .unwrap(),
            ChunkAck::Accepted
        );
        assert_eq!(
            split.chunk(0, 2, 2, &[0xA9], &sha(&[0xA9])).unwrap(),
            ChunkAck::Accepted
        );
        // An item that completes while a scalar is still incomplete is refused,
        // and the refusal advances nothing.
        let mut ends = HostUploadVerifier::begin(true, &[item(2)]).unwrap();
        assert_eq!(
            ends.chunk(0, 1, 0, &[b'a', 0xC3], &sha(&[b'a', 0xC3])),
            Err(HostUploadRefusal::InvalidUtf8)
        );
        ends.chunk(0, 1, 0, b"ab", &sha(b"ab")).unwrap();
    }

    #[test]
    fn every_negative_descriptor_and_chunk_case_is_refused_without_advancing() {
        assert_eq!(
            HostUploadVerifier::begin(
                true,
                &[HostUploadItem {
                    type_tag: 2,
                    length: item(1).length
                }]
            )
            .err(),
            Some(HostUploadRefusal::UnknownTag)
        );
        assert_eq!(
            HostUploadVerifier::begin(
                true,
                &[HostUploadItem {
                    type_tag: 0,
                    length: item(1).length
                }]
            )
            .err(),
            Some(HostUploadRefusal::ReservedTag)
        );
        assert_eq!(
            HostUploadVerifier::begin(true, &vec![item(1); MAX_ITEMS + 1]).err(),
            Some(HostUploadRefusal::TooManyItems)
        );
        assert_eq!(
            HostUploadVerifier::begin(true, &[item(MAX_TOTAL_BYTES + 1)]).err(),
            Some(HostUploadRefusal::TotalTooLarge)
        );
        let mut verifier = HostUploadVerifier::begin(true, &[item(2)]).unwrap();
        assert_eq!(
            verifier.chunk(0, 1, 0, b"", &sha(b"")),
            Err(HostUploadRefusal::InvalidEncoding)
        );
        assert_eq!(
            verifier.chunk(0, 1, 0, b"ab", &sha(b"xx")),
            Err(HostUploadRefusal::DigestMismatch)
        );
        assert_eq!(
            verifier.chunk(0, 2, 0, b"ab", &sha(b"ab")),
            Err(HostUploadRefusal::Gap)
        );
        assert_eq!(
            verifier.chunk(0, 1, 1, b"ab", &sha(b"ab")),
            Err(HostUploadRefusal::OutOfOrder)
        );
        assert_eq!(
            verifier.chunk(3, 1, 0, b"ab", &sha(b"ab")),
            Err(HostUploadRefusal::Foreign)
        );
        assert_eq!(
            verifier.chunk(0, 1, 0, b"abc", &sha(b"abc")),
            Err(HostUploadRefusal::Conflict)
        );
        // Nothing above advanced the stream: the first valid chunk still lands.
        verifier.chunk(0, 1, 0, b"ab", &sha(b"ab")).unwrap();
    }

    #[test]
    fn the_end_must_echo_the_begin_metadata_and_be_complete() {
        let items = vec![item(2)];
        let digest = canonical_digest(&[(1, b"ab")]);
        let mut verifier = HostUploadVerifier::begin(true, &items).unwrap();
        assert_eq!(
            verifier.end(true, &items, 2, &digest),
            Err(HostUploadRefusal::Incomplete)
        );
        verifier.chunk(0, 1, 0, b"ab", &sha(b"ab")).unwrap();
        assert_eq!(
            verifier.end(false, &items, 2, &digest),
            Err(HostUploadRefusal::MetadataMismatch),
            "success is part of the begin metadata"
        );
        assert_eq!(
            verifier.end(true, &[item(3)], 2, &digest),
            Err(HostUploadRefusal::MetadataMismatch)
        );
        assert_eq!(
            verifier.end(true, &items, 3, &digest),
            Err(HostUploadRefusal::MetadataMismatch)
        );
        assert_eq!(
            verifier.end(true, &items, 2, &[0u8; 32]),
            Err(HostUploadRefusal::DigestMismatch)
        );
        assert_eq!(verifier.end(true, &items, 2, &digest).unwrap(), digest);
        assert_eq!(verifier.abort(), Err(HostUploadRefusal::Late));
        assert_eq!(
            verifier.chunk(0, 2, 2, b"x", &sha(b"x")),
            Err(HostUploadRefusal::Late)
        );
    }

    #[test]
    fn decoding_accepts_only_canonical_padded_bounded_base64() {
        assert_eq!(decode_chunk_data("YWI=").unwrap(), b"ab");
        assert_eq!(
            decode_chunk_data("YWI"),
            Err(HostUploadRefusal::InvalidEncoding)
        );
        assert_eq!(
            decode_chunk_data("YWJ="),
            Err(HostUploadRefusal::InvalidEncoding)
        );
        assert_eq!(
            decode_chunk_data(""),
            Err(HostUploadRefusal::InvalidEncoding)
        );
        let oversize =
            base64::engine::general_purpose::STANDARD.encode(vec![0u8; MAX_CHUNK_BYTES + 1]);
        assert_eq!(
            decode_chunk_data(&oversize),
            Err(HostUploadRefusal::InvalidEncoding)
        );
    }

    #[test]
    fn the_envelope_is_a_closed_json_rpc_shape_with_stable_method_names() {
        let text = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 7,
            "method": "item/tool/result/abort",
            "params": {"candidate": {
                "callbackIncarnation": "cb-1",
                "connectionIncarnation": "conn-1",
                "requestId": 7,
                "threadId": "conv_36524fd8-f674-7fc2-8125-06d01fee0e18",
                "turnId": "attempt-1",
                "callId": "call-1",
                "subscriptionIncarnation": "sub-1",
                "uploadId": "11111111-2222-4333-8444-555555555555"
            }}
        });
        let parsed: HostUploadEnvelope = serde_json::from_value(text.clone()).unwrap();
        assert!(matches!(parsed.call, HostUploadMethod::Abort(_)));
        let mut unknown = text;
        unknown["params"]["extra"] = serde_json::json!(true);
        assert!(serde_json::from_value::<HostUploadEnvelope>(unknown).is_err());
    }

    #[test]
    fn upload_envelopes_are_admitted_by_the_public_schema() {
        let schema = crate::app_server::schema::protocol_schema();
        assert!(
            schema["anyOf"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!({"$ref": "#/$defs/HostUploadEnvelope"}))
        );
        assert!(
            schema["$defs"]["HostUploadEnvelope"]
                .to_string()
                .contains("item/tool/result/begin")
        );
    }

    fn decode_hex(text: &str) -> [u8; 32] {
        let mut out = [0u8; 32];
        for (slot, pair) in out.iter_mut().zip(text.as_bytes().chunks(2)) {
            *slot = u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap();
        }
        out
    }
}

#[cfg(test)]
mod slot_tests {
    use super::*;

    fn item(length: u64) -> HostUploadItem {
        HostUploadItem {
            type_tag: TYPE_TAG_INPUT_TEXT,
            length: LosslessU64::try_from(length.to_string()).unwrap(),
        }
    }

    fn sha(bytes: &[u8]) -> [u8; 32] {
        Sha256::digest(bytes).into()
    }

    fn candidate(upload: &str) -> HostUploadCandidate {
        HostUploadCandidate {
            callback_incarnation: OpaqueToken::try_from("cb-1".to_owned()).unwrap(),
            connection_incarnation: OpaqueToken::try_from("conn-1".to_owned()).unwrap(),
            subscription_incarnation: OpaqueToken::try_from("sub-1".to_owned()).unwrap(),
            request_id: RequestId::Integer(7),
            thread_id: ConversationId::new("conv_36524fd8-f674-7fc2-8125-06d01fee0e18"),
            turn_id: AttemptId::new("attempt-1"),
            call_id: ToolCallId::new("call-1"),
            upload_id: UploadId::try_from(upload.to_owned()).unwrap(),
        }
    }

    fn slot() -> HostUploadSlot {
        let candidate = candidate("11111111-2222-4333-8444-555555555555");
        HostUploadSlot::new(HostUploadExpected {
            callback_incarnation: candidate.callback_incarnation,
            connection_incarnation: candidate.connection_incarnation,
            subscription_incarnation: candidate.subscription_incarnation,
            request_id: candidate.request_id,
            thread_id: candidate.thread_id,
            turn_id: candidate.turn_id,
            call_id: candidate.call_id,
            upload_id: candidate.upload_id,
        })
    }

    #[test]
    fn a_wrong_identity_on_every_step_is_refused_before_any_digest_advances() {
        let good = candidate("11111111-2222-4333-8444-555555555555");
        let mut slot = slot();
        let items = [item(2)];
        slot.begin(good.clone(), true, &items).unwrap();
        for wrong in [
            HostUploadCandidate {
                subscription_incarnation: OpaqueToken::try_from("sub-2".to_owned()).unwrap(),
                ..good.clone()
            },
            HostUploadCandidate {
                connection_incarnation: OpaqueToken::try_from("conn-2".to_owned()).unwrap(),
                ..good.clone()
            },
            HostUploadCandidate {
                upload_id: UploadId::try_from("aaaaaaaa-2222-4333-8444-555555555555".to_owned())
                    .unwrap(),
                ..good.clone()
            },
        ] {
            assert_eq!(
                slot.chunk(&wrong, 0, 1, 0, b"ab", &sha(b"ab")),
                Err(HostUploadRefusal::Foreign)
            );
            assert_eq!(
                slot.end(&wrong, true, &items, 2, &sha(b"")),
                Err(HostUploadRefusal::Foreign)
            );
            assert_eq!(slot.abort(&wrong), Err(HostUploadRefusal::Foreign));
        }
        // The right identity still starts at sequence 1: nothing advanced.
        slot.chunk(&good, 0, 1, 0, b"ab", &sha(b"ab")).unwrap();
    }

    #[test]
    fn a_second_begin_with_other_metadata_or_upload_conflicts_and_an_identical_one_replays() {
        let good = candidate("11111111-2222-4333-8444-555555555555");
        let mut slot = slot();
        slot.begin(good.clone(), true, &[item(2)]).unwrap();
        slot.begin(good.clone(), true, &[item(2)]).unwrap();
        assert_eq!(
            slot.begin(good.clone(), false, &[item(2)]),
            Err(HostUploadRefusal::Conflict)
        );
        assert_eq!(
            slot.begin(good, true, &[item(3)]),
            Err(HostUploadRefusal::Conflict)
        );
    }

    #[test]
    fn an_abort_after_seal_or_a_late_step_after_end_is_refused_and_end_is_opaque_evidence() {
        let good = candidate("11111111-2222-4333-8444-555555555555");
        let mut slot = slot();
        let items = [item(2)];
        slot.begin(good.clone(), true, &items).unwrap();
        slot.chunk(&good, 0, 1, 0, b"ab", &sha(b"ab")).unwrap();
        let digest = canonical_digest(&[(TYPE_TAG_INPUT_TEXT, b"ab")]);
        let end = slot.end(&good, true, &items, 2, &digest).unwrap();
        assert_eq!(end.total(), 2);
        assert_eq!(end.digest(), &digest);
        assert_eq!(slot.abort(&good), Err(HostUploadRefusal::Late));
        assert_eq!(
            slot.chunk(&good, 0, 2, 2, b"x", &sha(b"x")),
            Err(HostUploadRefusal::Late)
        );
    }

    #[test]
    fn a_digest_mismatch_at_end_leaves_the_candidate_open_for_a_correct_end() {
        let good = candidate("11111111-2222-4333-8444-555555555555");
        let mut slot = slot();
        let items = [item(2)];
        slot.begin(good.clone(), true, &items).unwrap();
        slot.chunk(&good, 0, 1, 0, b"ab", &sha(b"ab")).unwrap();
        assert_eq!(
            slot.end(&good, true, &items, 2, &[0u8; 32]).map(|_| ()),
            Err(HostUploadRefusal::DigestMismatch)
        );
        let digest = canonical_digest(&[(TYPE_TAG_INPUT_TEXT, b"ab")]);
        slot.end(&good, true, &items, 2, &digest).unwrap();
    }

    #[test]
    fn an_empty_descriptor_array_begins_and_ends_without_panicking() {
        let good = candidate("11111111-2222-4333-8444-555555555555");
        let mut slot = slot();
        slot.begin(good.clone(), true, &[]).unwrap();
        let digest = canonical_digest(&[]);
        let end = slot.end(&good, true, &[], 0, &digest).unwrap();
        assert_eq!(end.items(), []);
        assert_eq!(end.total(), 0);
    }
}
