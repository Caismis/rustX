//! The optional save contract (EXEC-01): the wire vocabulary and an executable
//! pure reference for one normalized import.
//!
//! The state machine owns validation and delegates physical operations to its
//! Client storage owner. Requests carry no Server or Host path; the locator
//! first becomes visible on successful publication.

use std::time::Instant;

use base64::Engine as _;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::process::{OperationId, ServerIncarnation};
use crate::{
    ClientInstanceId, DecimalU64, Generation, MAX_RAW_CHUNK_BYTES, OPTIONAL_SAVE_DEADLINE,
    SessionId,
};

/// The longest opaque conversation identifier the owner binding carries.
pub const MAX_CONVERSATION_ID_BYTES: usize = 128;

/// The longest opaque locator token a publish may name.
pub const MAX_LOCATOR_TOKEN_BYTES: usize = 128;

/// One opaque, bounded conversation identifier: `[A-Za-z0-9._-]`, 1–128 bytes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct ConversationId(String);

impl ConversationId {
    /// Parses a conversation identifier; `None` outside the token grammar.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        is_token(value, MAX_CONVERSATION_ID_BYTES).then(|| Self(value.to_owned()))
    }
}

impl TryFrom<String> for ConversationId {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value).ok_or_else(|| String::from("invalid conversation identifier"))
    }
}

impl From<ConversationId> for String {
    fn from(value: ConversationId) -> Self {
        value.0
    }
}

/// A SHA-256 digest, carried on the wire as exactly 64 lowercase hex digits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Sha256Hex([u8; 32]);

impl Sha256Hex {
    /// The digest of `bytes`.
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        let mut out = [0u8; 32];
        out.copy_from_slice(&Sha256::digest(bytes));
        Self(out)
    }

    /// Wraps an already computed SHA-256 digest.
    #[must_use]
    pub const fn from_digest(digest: [u8; 32]) -> Self {
        Self(digest)
    }

    /// The raw digest bytes.
    #[must_use]
    pub const fn bytes(&self) -> &[u8; 32] {
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
        use std::fmt::Write as _;
        let mut out = String::with_capacity(64);
        for byte in value.0 {
            let _ = write!(out, "{byte:02x}");
        }
        out
    }
}

/// The Client-opaque locator of a published candidate: `save:` plus one bounded
/// token. It names storage the Client owns, never a Server or Host path.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct Locator(String);

impl Locator {
    /// Parses a locator; `None` for anything else.
    #[must_use]
    pub fn parse(value: &str) -> Option<Self> {
        value
            .strip_prefix("save:")
            .is_some_and(|token| is_token(token, MAX_LOCATOR_TOKEN_BYTES))
            .then(|| Self(value.to_owned()))
    }
}

impl TryFrom<String> for Locator {
    type Error = String;
    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::parse(&value).ok_or_else(|| String::from("invalid locator"))
    }
}

impl From<Locator> for String {
    fn from(value: Locator) -> Self {
        value.0
    }
}

/// A candidate identifier. Only the Client allocates these.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CandidateId(Uuid);

impl CandidateId {
    /// A fresh Client-assigned identifier.
    #[must_use]
    pub fn generate() -> Self {
        Self(Uuid::new_v4())
    }

    /// An identifier from explicit bytes (fixtures and Client records).
    #[must_use]
    pub const fn from_uuid(uuid: Uuid) -> Self {
        Self(uuid)
    }
}

/// The complete owner binding a save candidate belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveOwner {
    /// The process-wide Server incarnation.
    pub incarnation: ServerIncarnation,
    /// The Session the candidate belongs to.
    pub session: SessionId,
    /// The admitted Client instance.
    pub instance: ClientInstanceId,
    /// The binding generation.
    pub generation: Generation,
    /// The opaque conversation the candidate is saved for.
    pub conversation: ConversationId,
    /// The Server-assigned operation that owns the save.
    pub operation: OperationId,
}

/// One candidate, addressed by its owner and its Client-allocated identifier.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveKey {
    /// The owner binding the candidate was begun under.
    pub owner: SaveOwner,
    /// The Client-allocated candidate identifier.
    pub candidate: CandidateId,
}

/// The content format of a candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ContentFormat {
    /// Normalized UTF-8 text, version 1: valid UTF-8 at every accepted chunk
    /// boundary, with at most three incomplete scalar bytes carried across.
    #[serde(rename = "normalized-utf8-v1")]
    NormalizedUtf8V1,
}

/// How much of the candidate a seal claims.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SaveExtent {
    /// Every byte the owner produced is sealed.
    Complete,
    /// The retained bytes are a contiguous prefix of what was produced.
    ContiguousPrefix,
}

/// Whether a published candidate is still readable. Separate from publish.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Readability {
    /// The locator opened and the sealed bytes matched.
    Readable,
    /// The locator no longer names the candidate.
    Missing,
    /// The locator names bytes that no longer match the published evidence.
    Changed,
    /// The Client could not establish the answer.
    Unknown,
}

/// The truthful outcome of cleaning up an aborted candidate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Cleanup {
    /// The staged bytes were removed by this request.
    Removed,
    /// Nothing remained; a previous request removed it.
    AlreadyRemoved,
    /// Removal is still in progress.
    Pending,
    /// Removal could not be established.
    Unknown,
}

/// Stable domain codes for refused save steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum SaveCode {
    /// The owner binding is not the candidate's live owner.
    StaleOwner,
    /// The step contradicts the candidate's recorded state.
    Conflict,
    /// A chunk skipped ahead of the next expected sequence.
    Gap,
    /// A chunk's offset does not follow the bytes already received.
    OutOfOrder,
    /// A replayed chunk older than the one-chunk evidence window.
    DuplicateOutsideWindow,
    /// A chunk was empty, oversize, non-canonical, or invalid UTF-8, or a seal
    /// would leave an incomplete scalar.
    InvalidEncoding,
    /// A digest or seal evidence does not match the bytes.
    DigestMismatch,
    /// The candidate has not been sealed.
    NotSealed,
    /// The candidate already reached a terminal phase.
    AlreadyTerminal,
    /// A bounded resource would be exceeded.
    Capacity,
    /// The step's deadline elapsed.
    Deadline,
    /// Physical storage could not complete the step.
    StorageUnavailable,
    /// The Client does not offer managed output.
    CapabilityUnavailable,
}

/// One Server→Client save request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum SaveRequest {
    /// Allocates the candidate under `owner`. No path is named.
    Begin {
        /// The owner binding the candidate is begun under.
        owner: SaveOwner,
        /// The content format of the candidate.
        content_format: ContentFormat,
    },
    /// One raw chunk at the candidate's next sequence and offset.
    Chunk {
        /// The candidate the chunk feeds.
        key: SaveKey,
        /// The strictly increasing chunk ordinal, starting at `1`.
        sequence: DecimalU64,
        /// The byte offset the chunk starts at.
        offset: DecimalU64,
        /// The raw bytes, canonical padded base64, at most
        /// [`MAX_RAW_CHUNK_BYTES`] decoded.
        data_base64: String,
        /// The SHA-256 of the decoded bytes.
        sha256: Sha256Hex,
    },
    /// Seals the candidate with its complete evidence.
    Seal {
        /// The candidate to seal.
        key: SaveKey,
        /// The total decoded length the seal claims.
        length: DecimalU64,
        /// The SHA-256 of the sealed bytes.
        sha256: Sha256Hex,
        /// Whether the seal covers the whole candidate or a prefix.
        extent: SaveExtent,
    },
    /// Publishes a sealed candidate. Only an approved owner may authorize it.
    Publish {
        /// The candidate to publish.
        key: SaveKey,
        /// The seal evidence the publish must match exactly.
        length: DecimalU64,
        /// The SHA-256 the publish must match exactly.
        sha256: Sha256Hex,
        /// The extent the publish must match exactly.
        extent: SaveExtent,
    },
    /// Observes whether a published candidate is still readable.
    Probe {
        /// The published candidate.
        key: SaveKey,
    },
    /// Aborts the candidate; no later repair or continuation is possible.
    Abort {
        /// The candidate to abort.
        key: SaveKey,
    },
}

/// One Client→Server save reply. Receipts never carry a locator before publish.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum SaveReply {
    /// The candidate is allocated under the requested owner.
    Begun {
        /// The allocated candidate.
        key: SaveKey,
    },
    /// The chunk was accepted, or replayed as an identical duplicate.
    Chunked {
        /// The candidate the chunk fed.
        key: SaveKey,
        /// The chunk's sequence.
        sequence: DecimalU64,
        /// The offset the next chunk must start at.
        next_offset: DecimalU64,
        /// Whether this was an identical duplicate that appended nothing.
        duplicate: bool,
    },
    /// The candidate is sealed with the claimed evidence.
    Sealed {
        /// The sealed candidate.
        key: SaveKey,
        /// The sealed length.
        length: DecimalU64,
        /// The sealed digest.
        sha256: Sha256Hex,
        /// The sealed extent.
        extent: SaveExtent,
    },
    /// The candidate is published at a real locator, with its evidence.
    Published {
        /// The published candidate.
        key: SaveKey,
        /// The published locator.
        locator: Locator,
        /// The published length.
        length: DecimalU64,
        /// The published digest.
        sha256: Sha256Hex,
        /// The published extent.
        extent: SaveExtent,
    },
    /// The candidate was aborted; cleanup is reported truthfully.
    Aborted {
        /// The aborted candidate.
        key: SaveKey,
        /// The cleanup outcome.
        cleanup: Cleanup,
    },
    /// The readability of a published candidate, observed from its storage.
    Readability {
        /// The probed candidate.
        key: SaveKey,
        /// The observed readability.
        outcome: Readability,
    },
    /// The step was refused with a stable domain code.
    Refused {
        /// The stable refusal code.
        code: SaveCode,
    },
    /// The Client does not offer managed output; no path or queue was touched.
    Unavailable {
        /// The negotiated output capability is unavailable.
        code: SaveCode,
    },
}

/// The selection decision for one import. Owned by a single invocation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// The retained total has not crossed the threshold.
    NotCrossed,
    /// The threshold crossed with no owner present: terminal, never rebinds.
    Absent,
    /// The threshold crossed and selected this exact owner until `deadline`.
    Bound {
        /// The selected owner.
        owner: SaveOwner,
        /// The one deadline tagged at the crossing.
        deadline: Instant,
    },
}

/// What one selection observation reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    /// Still below the threshold.
    NotCrossed,
    /// Crossed and bound to the owner, within the deadline.
    Bound,
    /// Crossed with no owner: terminal.
    Absent,
    /// The bound owner's deadline has passed.
    Deadline,
}

/// The single threshold selection of one import. It looks the owner up at most
/// once, at the first crossing, and never again afterwards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveSelection {
    threshold: u64,
    total: u64,
    decision: Decision,
}

impl SaveSelection {
    /// A fresh selection for `threshold` bytes.
    #[must_use]
    pub const fn new(threshold: u64) -> Self {
        Self {
            threshold,
            total: 0,
            decision: Decision::NotCrossed,
        }
    }

    /// The retained total observed so far.
    #[must_use]
    pub const fn total(&self) -> u64 {
        self.total
    }

    /// The current decision.
    #[must_use]
    pub const fn decision(&self) -> &Decision {
        &self.decision
    }

    /// Observes `additional` bytes at `now` under the caller's `call_deadline`.
    /// A total exactly equal to the threshold does not cross. At the first
    /// crossing `lookup` runs once; its owner (or its absence) is final. The
    /// deadline is `min(now + OPTIONAL_SAVE_DEADLINE, call_deadline)`, tagged
    /// once and never renewed.
    ///
    /// # Errors
    ///
    /// [`SaveCode::Capacity`] when the total would overflow.
    pub fn observe(
        &mut self,
        additional: u64,
        now: Instant,
        call_deadline: Instant,
        lookup: impl FnOnce() -> Option<SaveOwner>,
    ) -> Result<Step, SaveCode> {
        let total = self
            .total
            .checked_add(additional)
            .ok_or(SaveCode::Capacity)?;
        if matches!(self.decision, Decision::NotCrossed) {
            if total <= self.threshold {
                self.total = total;
                return Ok(Step::NotCrossed);
            }
            self.total = total;
            self.decision = match lookup() {
                Some(owner) => {
                    let window = now
                        .checked_add(OPTIONAL_SAVE_DEADLINE)
                        .map_or(call_deadline, |end| end.min(call_deadline));
                    Decision::Bound {
                        owner,
                        deadline: window,
                    }
                }
                None => Decision::Absent,
            };
        } else {
            self.total = total;
        }
        Ok(self.step(now))
    }

    fn step(&self, now: Instant) -> Step {
        match &self.decision {
            Decision::NotCrossed => Step::NotCrossed,
            Decision::Absent => Step::Absent,
            Decision::Bound { deadline, .. } if now >= *deadline => Step::Deadline,
            Decision::Bound { .. } => Step::Bound,
        }
    }
}

/// Decodes one chunk's canonical padded base64 at the wire boundary: the encoded
/// length is checked before any allocation, the decode must re-encode to exactly
/// the received text, and the decoded bytes must be non-empty and within the
/// raw chunk limit.
///
/// # Errors
///
/// [`SaveCode::InvalidEncoding`] for anything else.
pub fn decode_chunk(data_base64: &str) -> Result<Vec<u8>, SaveCode> {
    let engine = base64::engine::general_purpose::STANDARD;
    if data_base64.len() > MAX_RAW_CHUNK_BYTES.div_ceil(3) * 4 {
        return Err(SaveCode::InvalidEncoding);
    }
    let bytes = engine
        .decode(data_base64)
        .map_err(|_| SaveCode::InvalidEncoding)?;
    if bytes.is_empty() || bytes.len() > MAX_RAW_CHUNK_BYTES || engine.encode(&bytes) != data_base64
    {
        return Err(SaveCode::InvalidEncoding);
    }
    Ok(bytes)
}

/// Incremental normalized-utf8-v1 check: `pending ++ bytes` must be valid UTF-8
/// up to an incomplete scalar of at most three bytes, which is returned as the
/// new pending tail. Any other invalid byte refuses.
fn utf8_tail(pending: &[u8], bytes: &[u8]) -> Result<([u8; 3], usize), SaveCode> {
    let joined: Vec<u8> = pending.iter().chain(bytes).copied().collect();
    match std::str::from_utf8(&joined) {
        Ok(_) => Ok(([0; 3], 0)),
        Err(error) if error.error_len().is_none() => {
            let tail = &joined[error.valid_up_to()..];
            if tail.len() > 3 {
                return Err(SaveCode::InvalidEncoding);
            }
            let mut out = [0u8; 3];
            out[..tail.len()].copy_from_slice(tail);
            Ok((out, tail.len()))
        }
        Err(_) => Err(SaveCode::InvalidEncoding),
    }
}

/// The sealed evidence a publish is keyed to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SealRecord {
    /// The sealed decoded length.
    pub length: u64,
    /// The sealed digest.
    pub sha256: Sha256Hex,
    /// The sealed extent.
    pub extent: SaveExtent,
}

/// The Client's physical side of a save. Publish is real or refused, readability
/// is observed from storage, and cleanup reports what actually happened.
pub trait SaveStorage {
    /// Appends one validated, previously unapplied chunk. Failure leaves the
    /// acknowledged prefix intact or poisons the candidate until cleanup.
    ///
    /// # Errors
    ///
    /// A byte limit or physical write was refused.
    fn append(&mut self, key: &SaveKey, offset: u64, bytes: &[u8]) -> Result<(), SaveCode>;

    /// Publishes exactly the sealed bytes of `key`, returning the locator the
    /// Client physically created. An error publishes nothing.
    ///
    /// # Errors
    ///
    /// Whatever refusal the physical publish reports.
    fn publish(&mut self, key: &SaveKey, seal: &SealRecord) -> Result<Locator, SaveCode>;

    /// Whether the published locator still names the sealed bytes.
    fn readability(&mut self, locator: &Locator) -> Readability;

    /// Removes the staged bytes of an aborted candidate and reports the outcome.
    fn cleanup(&mut self, key: &SaveKey) -> Cleanup;
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Phase {
    Open,
    Sealed(SealRecord),
    Published(Locator),
    Aborted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct LastChunk {
    sequence: u64,
    offset: u64,
    length: u64,
    sha256: Sha256Hex,
}

/// The byte ledger of one candidate: an incremental digest, a count, the
/// trailing incomplete scalar, and the last chunk's identity. Never the bytes.
#[derive(Debug, Clone)]
struct Stream {
    phase: Phase,
    next_sequence: u64,
    count: u64,
    digest: Sha256,
    pending: [u8; 3],
    pending_len: usize,
    last: Option<LastChunk>,
}

/// The outcome of one accepted or replayed chunk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChunkOutcome {
    Appended { next_offset: u64 },
    Duplicate { next_offset: u64 },
}

impl Stream {
    fn new() -> Self {
        Self {
            phase: Phase::Open,
            next_sequence: 1,
            count: 0,
            digest: Sha256::new(),
            pending: [0; 3],
            pending_len: 0,
            last: None,
        }
    }

    fn chunk(
        &mut self,
        sequence: u64,
        offset: u64,
        data: &[u8],
        sha256: &Sha256Hex,
    ) -> Result<ChunkOutcome, SaveCode> {
        match &self.phase {
            Phase::Open => {}
            Phase::Sealed(_) => return Err(SaveCode::Conflict),
            Phase::Published(_) | Phase::Aborted => return Err(SaveCode::AlreadyTerminal),
        }
        if Sha256Hex::of(data) != *sha256 {
            return Err(SaveCode::DigestMismatch);
        }
        let length = u64::try_from(data.len()).map_err(|_| SaveCode::InvalidEncoding)?;
        if let Some(last) = self.last
            && sequence == last.sequence
        {
            let identical =
                last.offset == offset && last.length == length && last.sha256 == *sha256;
            return if identical {
                Ok(ChunkOutcome::Duplicate {
                    next_offset: self.count,
                })
            } else {
                Err(SaveCode::Conflict)
            };
        }
        if sequence < self.next_sequence {
            return Err(SaveCode::DuplicateOutsideWindow);
        }
        if sequence > self.next_sequence {
            return Err(SaveCode::Gap);
        }
        if offset != self.count {
            return Err(SaveCode::OutOfOrder);
        }
        let (pending, pending_len) = utf8_tail(&self.pending[..self.pending_len], data)?;
        let count = self.count.checked_add(length).ok_or(SaveCode::Capacity)?;
        let next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or(SaveCode::Capacity)?;
        self.digest.update(data);
        self.count = count;
        self.next_sequence = next_sequence;
        self.pending = pending;
        self.pending_len = pending_len;
        self.last = Some(LastChunk {
            sequence,
            offset,
            length,
            sha256: *sha256,
        });
        Ok(ChunkOutcome::Appended { next_offset: count })
    }

    fn seal(
        &mut self,
        length: u64,
        sha256: &Sha256Hex,
        extent: SaveExtent,
    ) -> Result<SealRecord, SaveCode> {
        let record = SealRecord {
            length,
            sha256: *sha256,
            extent,
        };
        match &self.phase {
            Phase::Open => {
                if self.pending_len != 0 {
                    return Err(SaveCode::InvalidEncoding);
                }
                let mut digest = [0u8; 32];
                digest.copy_from_slice(&self.digest.clone().finalize());
                if length != self.count || *sha256 != Sha256Hex(digest) {
                    return Err(SaveCode::DigestMismatch);
                }
                self.phase = Phase::Sealed(record);
                Ok(record)
            }
            Phase::Sealed(existing) if *existing == record => Ok(record),
            Phase::Sealed(_) => Err(SaveCode::Conflict),
            Phase::Published(_) | Phase::Aborted => Err(SaveCode::AlreadyTerminal),
        }
    }
}

/// The lifecycle of the one candidate a normalized import may own. It is bound
/// to one selection and one client-allocated candidate. Every request carries
/// the instant it is applied at; a bound selection's deadline gates every
/// upload step before any state changes, and only physical storage can
/// publish, read back, or clean up.
#[derive(Debug, Clone)]
pub struct SaveCapture {
    selection: SaveSelection,
    candidate: CandidateId,
    stream: Option<(SaveOwner, Stream)>,
    approved: bool,
}

impl SaveCapture {
    /// The capture for `candidate` under `selection`. Nothing is allocated yet.
    #[must_use]
    pub const fn new(selection: SaveSelection, candidate: CandidateId) -> Self {
        Self {
            selection,
            candidate,
            stream: None,
            approved: false,
        }
    }

    /// The selection this capture is bound to.
    #[must_use]
    pub const fn selection(&self) -> &SaveSelection {
        &self.selection
    }

    fn bound_owner(&self) -> Option<&SaveOwner> {
        match self.selection.decision() {
            Decision::Bound { owner, .. } => Some(owner),
            _ => None,
        }
    }

    fn deadline_passed(&self, now: Instant) -> bool {
        matches!(
            self.selection.decision(),
            Decision::Bound { deadline, .. } if now >= *deadline
        )
    }

    /// Records business-valid completion of the bound owner, the only authority
    /// that may publish. Refused for any other owner.
    ///
    /// # Errors
    ///
    /// [`SaveCode::StaleOwner`] unless `owner` is the bound owner.
    pub fn approve(&mut self, owner: &SaveOwner) -> Result<(), SaveCode> {
        if self.bound_owner() != Some(owner) {
            return Err(SaveCode::StaleOwner);
        }
        self.approved = true;
        Ok(())
    }

    /// Applies one wire request at `now` and returns its reply. Begin, chunk,
    /// seal, and publish are refused with `Deadline` once the bound deadline has
    /// passed, before any state changes. Abort and probe are never gated by it.
    pub fn apply(
        &mut self,
        request: &SaveRequest,
        now: Instant,
        storage: &mut impl SaveStorage,
    ) -> SaveReply {
        match request {
            SaveRequest::Abort { key } => self.abort(key, storage),
            SaveRequest::Probe { key } => self.probe(key, storage),
            _ if self.deadline_passed(now) => SaveReply::Refused {
                code: SaveCode::Deadline,
            },
            SaveRequest::Begin {
                owner,
                content_format: ContentFormat::NormalizedUtf8V1,
            } => self.begin(owner),
            SaveRequest::Chunk {
                key,
                sequence,
                offset,
                data_base64,
                sha256,
            } => self.chunk(
                key,
                sequence.get(),
                offset.get(),
                data_base64,
                sha256,
                storage,
            ),
            SaveRequest::Seal {
                key,
                length,
                sha256,
                extent,
            } => self.seal(key, length.get(), sha256, *extent),
            SaveRequest::Publish {
                key,
                length,
                sha256,
                extent,
            } => self.publish(key, length.get(), sha256, *extent, storage),
        }
    }

    fn begin(&mut self, owner: &SaveOwner) -> SaveReply {
        let key = SaveKey {
            owner: owner.clone(),
            candidate: self.candidate,
        };
        if self.bound_owner() != Some(owner) {
            return SaveReply::Refused {
                code: SaveCode::StaleOwner,
            };
        }
        if self.stream.is_none() {
            self.stream = Some((owner.clone(), Stream::new()));
            return SaveReply::Begun { key };
        }
        let Some((existing, stream)) = &self.stream else {
            return SaveReply::Refused {
                code: SaveCode::Conflict,
            };
        };
        if existing != owner {
            return SaveReply::Refused {
                code: SaveCode::Conflict,
            };
        }
        match &stream.phase {
            Phase::Open | Phase::Sealed(_) => SaveReply::Begun { key },
            Phase::Published(_) | Phase::Aborted => SaveReply::Refused {
                code: SaveCode::AlreadyTerminal,
            },
        }
    }

    fn chunk(
        &mut self,
        key: &SaveKey,
        sequence: u64,
        offset: u64,
        data_base64: &str,
        sha256: &Sha256Hex,
        storage: &mut impl SaveStorage,
    ) -> SaveReply {
        let stream = match self.live(key) {
            Ok(stream) => stream,
            Err(code) => return SaveReply::Refused { code },
        };
        let data = match decode_chunk(data_base64) {
            Ok(data) => data,
            Err(code) => return SaveReply::Refused { code },
        };
        let mut staged = stream.clone();
        match staged.chunk(sequence, offset, &data, sha256) {
            Ok(ChunkOutcome::Appended { next_offset }) => {
                if let Err(code) = storage.append(key, offset, &data) {
                    return SaveReply::Refused { code };
                }
                *stream = staged;
                SaveReply::Chunked {
                    key: key.clone(),
                    sequence: DecimalU64::new(sequence),
                    next_offset: DecimalU64::new(next_offset),
                    duplicate: false,
                }
            }
            Ok(ChunkOutcome::Duplicate { next_offset }) => SaveReply::Chunked {
                key: key.clone(),
                sequence: DecimalU64::new(sequence),
                next_offset: DecimalU64::new(next_offset),
                duplicate: true,
            },
            Err(code) => SaveReply::Refused { code },
        }
    }

    fn seal(
        &mut self,
        key: &SaveKey,
        length: u64,
        sha256: &Sha256Hex,
        extent: SaveExtent,
    ) -> SaveReply {
        match self.live(key) {
            Ok(stream) => match stream.seal(length, sha256, extent) {
                Ok(_) => SaveReply::Sealed {
                    key: key.clone(),
                    length: DecimalU64::new(length),
                    sha256: *sha256,
                    extent,
                },
                Err(code) => SaveReply::Refused { code },
            },
            Err(code) => SaveReply::Refused { code },
        }
    }

    /// Publishes only an approved, sealed candidate whose evidence matches the
    /// seal exactly. The physical callback runs after every check and never on a
    /// rejected request; a refused receipt leaves the candidate sealed.
    fn publish(
        &mut self,
        key: &SaveKey,
        length: u64,
        sha256: &Sha256Hex,
        extent: SaveExtent,
        storage: &mut impl SaveStorage,
    ) -> SaveReply {
        if !self.approved {
            return SaveReply::Refused {
                code: SaveCode::Conflict,
            };
        }
        let stream = match self.live(key) {
            Ok(stream) => stream,
            Err(code) => return SaveReply::Refused { code },
        };
        let seal = match stream.phase {
            Phase::Open => {
                return SaveReply::Refused {
                    code: SaveCode::NotSealed,
                };
            }
            Phase::Sealed(seal) => seal,
            Phase::Published(_) | Phase::Aborted => {
                return SaveReply::Refused {
                    code: SaveCode::AlreadyTerminal,
                };
            }
        };
        if seal.length != length || seal.sha256 != *sha256 || seal.extent != extent {
            return SaveReply::Refused {
                code: SaveCode::DigestMismatch,
            };
        }
        if extent == SaveExtent::ContiguousPrefix && length == 0 {
            return SaveReply::Refused {
                code: SaveCode::Conflict,
            };
        }
        match storage.publish(key, &seal) {
            Ok(locator) => {
                stream.phase = Phase::Published(locator.clone());
                SaveReply::Published {
                    key: key.clone(),
                    locator,
                    length: DecimalU64::new(length),
                    sha256: *sha256,
                    extent,
                }
            }
            Err(code) => SaveReply::Refused { code },
        }
    }

    /// Reports readability of a published candidate, observed from storage
    /// independently of any earlier verdict.
    fn probe(&mut self, key: &SaveKey, storage: &mut impl SaveStorage) -> SaveReply {
        let stream = match self.live(key) {
            Ok(stream) => stream,
            Err(code) => return SaveReply::Refused { code },
        };
        let locator = match &stream.phase {
            Phase::Published(locator) => locator.clone(),
            _ => {
                return SaveReply::Refused {
                    code: SaveCode::Conflict,
                };
            }
        };
        SaveReply::Readability {
            key: key.clone(),
            outcome: storage.readability(&locator),
        }
    }

    /// Aborts a live candidate; cleanup is whatever storage reports. A published
    /// candidate is terminal. Repeating Abort may finish pending cleanup.
    fn abort(&mut self, key: &SaveKey, storage: &mut impl SaveStorage) -> SaveReply {
        let stream = match self.live(key) {
            Ok(stream) => stream,
            Err(code) => return SaveReply::Refused { code },
        };
        match stream.phase {
            Phase::Open | Phase::Sealed(_) | Phase::Aborted => {}
            Phase::Published(_) => {
                return SaveReply::Refused {
                    code: SaveCode::AlreadyTerminal,
                };
            }
        }
        let cleanup = storage.cleanup(key);
        stream.phase = Phase::Aborted;
        SaveReply::Aborted {
            key: key.clone(),
            cleanup,
        }
    }

    fn live(&mut self, key: &SaveKey) -> Result<&mut Stream, SaveCode> {
        if key.candidate != self.candidate {
            return Err(SaveCode::StaleOwner);
        }
        match &mut self.stream {
            Some((owner, stream)) if *owner == key.owner => Ok(stream),
            _ => Err(SaveCode::StaleOwner),
        }
    }
}

fn is_token(value: &str, max: usize) -> bool {
    !value.is_empty()
        && value.len() <= max
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-'))
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;
    use std::collections::BTreeMap;
    use std::time::Duration;

    use super::*;
    use crate::transport::{encode_record, read_record};
    use crate::{ClientReply, ClientRequest};

    const FIXTURE: &str = include_str!("../fixtures/save-envelopes.json");

    fn owner() -> SaveOwner {
        SaveOwner {
            incarnation: ServerIncarnation::from_uuid(Uuid::from_u128(1)),
            session: SessionId::new("ses_00000000-0000-7000-8000-000000000001"),
            instance: ClientInstanceId::new("instance-1"),
            generation: Generation::new(std::num::NonZeroU64::new(1).unwrap()),
            conversation: ConversationId::parse("conv.alpha-1").unwrap(),
            operation: OperationId::from_uuid(Uuid::from_u128(2)),
        }
    }

    fn other_owner() -> SaveOwner {
        SaveOwner {
            operation: OperationId::from_uuid(Uuid::from_u128(3)),
            ..owner()
        }
    }

    fn candidate() -> CandidateId {
        CandidateId::from_uuid(Uuid::from_u128(42))
    }

    fn key(owner: &SaveOwner) -> SaveKey {
        SaveKey {
            owner: owner.clone(),
            candidate: candidate(),
        }
    }

    /// Serializes `value` to JSON text, parses it back, and drives the typed
    /// request through the capture — the outer envelope is what the wire carries.
    fn over_json(capture: &mut SaveCapture, request: &ClientRequest) -> SaveReply {
        let text = serde_json::to_string(request).unwrap();
        let parsed: ClientRequest = serde_json::from_str(&text).unwrap();
        let ClientRequest::Save(save) = parsed else {
            panic!("the save envelope is preserved");
        };
        capture.apply(&save, Instant::now(), &mut FixtureStorage)
    }

    /// Fixture storage double: the locator it returns is a fixture token,
    /// distinct from the candidate identifier, and it is never production.
    #[derive(Default)]
    struct FixtureStorage;

    impl SaveStorage for FixtureStorage {
        fn append(&mut self, _key: &SaveKey, _offset: u64, _bytes: &[u8]) -> Result<(), SaveCode> {
            Ok(())
        }

        fn publish(&mut self, _key: &SaveKey, _seal: &SealRecord) -> Result<Locator, SaveCode> {
            Ok(Locator::parse("save:fixture-locator").unwrap())
        }
        fn readability(&mut self, _locator: &Locator) -> Readability {
            Readability::Readable
        }
        fn cleanup(&mut self, _key: &SaveKey) -> Cleanup {
            Cleanup::Pending
        }
    }

    fn chunk_request(key: &SaveKey, sequence: u64, offset: u64, data: &[u8]) -> ClientRequest {
        ClientRequest::Save(SaveRequest::Chunk {
            key: key.clone(),
            sequence: DecimalU64::new(sequence),
            offset: DecimalU64::new(offset),
            data_base64: base64::engine::general_purpose::STANDARD.encode(data),
            sha256: Sha256Hex::of(data),
        })
    }

    fn seal_request(key: &SaveKey, data: &[u8], extent: SaveExtent) -> ClientRequest {
        ClientRequest::Save(SaveRequest::Seal {
            key: key.clone(),
            length: DecimalU64::new(data.len() as u64),
            sha256: Sha256Hex::of(data),
            extent,
        })
    }

    fn publish_request(key: &SaveKey, data: &[u8], extent: SaveExtent) -> ClientRequest {
        ClientRequest::Save(SaveRequest::Publish {
            key: key.clone(),
            length: DecimalU64::new(data.len() as u64),
            sha256: Sha256Hex::of(data),
            extent,
        })
    }

    /// A capture whose selection is bound to `owner` at a 4-byte threshold.
    fn bound_capture(owner: &SaveOwner) -> (SaveCapture, Instant) {
        let now = Instant::now();
        let mut selection = SaveSelection::new(4);
        let step = selection
            .observe(5, now, now + Duration::from_secs(60), || {
                Some(owner.clone())
            })
            .unwrap();
        assert_eq!(step, Step::Bound);
        (SaveCapture::new(selection, candidate()), now)
    }

    #[test]
    fn an_exact_threshold_does_not_cross_and_the_first_overflow_binds_once() {
        let now = Instant::now();
        let calls = Cell::new(0);
        let mut selection = SaveSelection::new(10);
        let lookup = || {
            calls.set(calls.get() + 1);
            Some(owner())
        };
        assert_eq!(
            selection
                .observe(10, now, now + Duration::from_secs(60), lookup)
                .unwrap(),
            Step::NotCrossed
        );
        assert_eq!(calls.get(), 0, "no lookup below or at the threshold");
        assert_eq!(
            selection
                .observe(1, now, now + Duration::from_secs(60), lookup)
                .unwrap(),
            Step::Bound
        );
        assert_eq!(calls.get(), 1);
    }

    #[test]
    fn absence_at_crossing_is_terminal_and_a_later_owner_never_rebinds() {
        let now = Instant::now();
        let calls = Cell::new(0);
        let mut selection = SaveSelection::new(4);
        assert_eq!(
            selection
                .observe(5, now, now + Duration::from_secs(60), || {
                    calls.set(calls.get() + 1);
                    None
                })
                .unwrap(),
            Step::Absent
        );
        // A replacement owner is offered on every later advance; the lookup is
        // never consulted again and the decision stays absent.
        for _ in 0..3 {
            let step = selection
                .observe(1, now, now + Duration::from_secs(60), || {
                    calls.set(calls.get() + 1);
                    Some(other_owner())
                })
                .unwrap();
            assert_eq!(step, Step::Absent);
        }
        assert_eq!(calls.get(), 1, "the owner lookup runs exactly once");
        assert_eq!(selection.decision(), &Decision::Absent);
    }

    #[test]
    fn the_bound_owner_survives_later_advances_and_replacements_unchanged() {
        let now = Instant::now();
        let calls = Cell::new(0);
        let mut selection = SaveSelection::new(4);
        selection
            .observe(5, now, now + Duration::from_secs(60), || {
                calls.set(calls.get() + 1);
                Some(owner())
            })
            .unwrap();
        for _ in 0..3 {
            selection
                .observe(2, now, now + Duration::from_secs(60), || {
                    calls.set(calls.get() + 1);
                    Some(other_owner())
                })
                .unwrap();
        }
        assert_eq!(calls.get(), 1);
        let Decision::Bound { owner: bound, .. } = selection.decision() else {
            panic!("bound");
        };
        assert_eq!(bound, &owner());
    }

    #[test]
    fn the_deadline_is_min_of_now_plus_five_seconds_and_the_call_deadline_and_never_renews() {
        let start = Instant::now();
        // A call deadline shorter than the five-second window wins.
        let mut shorter = SaveSelection::new(0);
        let call_deadline = start + Duration::from_secs(2);
        shorter
            .observe(1, start, call_deadline, || Some(owner()))
            .unwrap();
        let Decision::Bound { deadline, .. } = shorter.decision() else {
            panic!("bound");
        };
        assert_eq!(*deadline, call_deadline);

        // Otherwise the five-second window from the crossing wins.
        let mut longer = SaveSelection::new(0);
        longer
            .observe(1, start, start + Duration::from_secs(60), || Some(owner()))
            .unwrap();
        let Decision::Bound { deadline, .. } = longer.decision() else {
            panic!("bound");
        };
        assert_eq!(*deadline, start + OPTIONAL_SAVE_DEADLINE);

        // A slow advance, and a later call with a generous deadline, never renew it.
        let slow = start + OPTIONAL_SAVE_DEADLINE;
        assert_eq!(
            longer
                .observe(1, slow, start + Duration::from_secs(600), || None)
                .unwrap(),
            Step::Deadline
        );
        let Decision::Bound { deadline, .. } = longer.decision() else {
            panic!("bound");
        };
        assert_eq!(*deadline, start + OPTIONAL_SAVE_DEADLINE);
    }

    #[test]
    fn a_decode_accepts_only_canonical_padded_bounded_base64() {
        let data = b"ok";
        let canonical = base64::engine::general_purpose::STANDARD.encode(data);
        assert_eq!(decode_chunk(&canonical).unwrap(), data);
        assert_eq!(
            decode_chunk("b2"),
            Err(SaveCode::InvalidEncoding),
            "missing padding"
        );
        assert_eq!(
            decode_chunk("b2=="),
            Err(SaveCode::InvalidEncoding),
            "non-canonical trailing bits"
        );
        assert_eq!(decode_chunk(""), Err(SaveCode::InvalidEncoding), "empty");
        let oversize =
            base64::engine::general_purpose::STANDARD.encode(vec![0u8; MAX_RAW_CHUNK_BYTES + 1]);
        assert_eq!(decode_chunk(&oversize), Err(SaveCode::InvalidEncoding));
    }

    #[test]
    fn the_capture_refuses_a_foreign_owner_and_a_second_candidate() {
        let (mut capture, _) = bound_capture(&owner());
        let foreign = over_json(
            &mut capture,
            &ClientRequest::Save(SaveRequest::Begin {
                owner: other_owner(),
                content_format: ContentFormat::NormalizedUtf8V1,
            }),
        );
        assert_eq!(
            foreign,
            SaveReply::Refused {
                code: SaveCode::StaleOwner
            }
        );
        let begun = over_json(
            &mut capture,
            &ClientRequest::Save(SaveRequest::Begin {
                owner: owner(),
                content_format: ContentFormat::NormalizedUtf8V1,
            }),
        );
        assert!(matches!(begun, SaveReply::Begun { .. }));
        let replay = over_json(
            &mut capture,
            &ClientRequest::Save(SaveRequest::Begin {
                owner: owner(),
                content_format: ContentFormat::NormalizedUtf8V1,
            }),
        );
        assert!(
            matches!(replay, SaveReply::Begun { .. }),
            "one allocation, replayed"
        );
    }

    #[test]
    fn a_split_utf8_scalar_seals_and_publishes_while_an_incomplete_seal_is_refused() {
        let (mut capture, _) = bound_capture(&owner());
        let key = key(&owner());
        over_json(
            &mut capture,
            &ClientRequest::Save(SaveRequest::Begin {
                owner: owner(),
                content_format: ContentFormat::NormalizedUtf8V1,
            }),
        );
        // "hé!" with the two bytes of U+00E9 split across chunks.
        let first = [b'h', 0xC3];
        let second = [0xA9, b'!'];
        assert!(matches!(
            over_json(&mut capture, &chunk_request(&key, 1, 0, &first)),
            SaveReply::Chunked { next_offset, .. } if next_offset.get() == 2
        ));
        // Sealing while a scalar is incomplete is refused: the candidate is not complete.
        let whole = "hé!".as_bytes();
        assert_eq!(
            over_json(
                &mut capture,
                &seal_request(&key, &first, SaveExtent::ContiguousPrefix)
            ),
            SaveReply::Refused {
                code: SaveCode::InvalidEncoding
            }
        );
        assert!(matches!(
            over_json(&mut capture, &chunk_request(&key, 2, 2, &second)),
            SaveReply::Chunked { next_offset, .. } if next_offset.get() == 4
        ));
        assert!(matches!(
            over_json(
                &mut capture,
                &seal_request(&key, whole, SaveExtent::Complete)
            ),
            SaveReply::Sealed { .. }
        ));
        // Publish is refused until the bound owner approves.
        assert_eq!(
            over_json(
                &mut capture,
                &publish_request(&key, whole, SaveExtent::Complete)
            ),
            SaveReply::Refused {
                code: SaveCode::Conflict
            }
        );
        capture.approve(&owner()).unwrap();
        let published = over_json(
            &mut capture,
            &publish_request(&key, whole, SaveExtent::Complete),
        );
        let SaveReply::Published {
            locator, length, ..
        } = published
        else {
            panic!("approved publish succeeds");
        };
        assert!(String::from(locator.clone()).starts_with("save:"));
        assert_eq!(length.get(), whole.len() as u64);
    }

    #[test]
    fn invalid_utf8_gap_reorder_and_digest_mismatch_never_advance_the_stream() {
        let (mut capture, _) = bound_capture(&owner());
        let key = key(&owner());
        over_json(
            &mut capture,
            &ClientRequest::Save(SaveRequest::Begin {
                owner: owner(),
                content_format: ContentFormat::NormalizedUtf8V1,
            }),
        );
        let invalid = [0xFF];
        assert_eq!(
            over_json(&mut capture, &chunk_request(&key, 1, 0, &invalid)),
            SaveReply::Refused {
                code: SaveCode::InvalidEncoding
            }
        );
        assert_eq!(
            over_json(&mut capture, &chunk_request(&key, 2, 0, b"x")),
            SaveReply::Refused {
                code: SaveCode::Gap
            }
        );
        // A digest over the wrong bytes is a mismatch, and sequence 1 is still free.
        let wrong = ClientRequest::Save(SaveRequest::Chunk {
            key: key.clone(),
            sequence: DecimalU64::new(1),
            offset: DecimalU64::new(0),
            data_base64: base64::engine::general_purpose::STANDARD.encode(b"abc"),
            sha256: Sha256Hex::of(b"other"),
        });
        assert_eq!(
            over_json(&mut capture, &wrong),
            SaveReply::Refused {
                code: SaveCode::DigestMismatch
            }
        );
        assert!(matches!(
            over_json(&mut capture, &chunk_request(&key, 1, 0, b"abc")),
            SaveReply::Chunked { next_offset, .. } if next_offset.get() == 3
        ));
    }

    #[test]
    fn an_identical_last_chunk_is_idempotent_and_an_older_duplicate_is_refused() {
        let (mut capture, _) = bound_capture(&owner());
        let key = key(&owner());
        over_json(
            &mut capture,
            &ClientRequest::Save(SaveRequest::Begin {
                owner: owner(),
                content_format: ContentFormat::NormalizedUtf8V1,
            }),
        );
        over_json(&mut capture, &chunk_request(&key, 1, 0, b"aa"));
        over_json(&mut capture, &chunk_request(&key, 2, 2, b"bb"));
        assert!(matches!(
            over_json(&mut capture, &chunk_request(&key, 2, 2, b"bb")),
            SaveReply::Chunked { duplicate: true, next_offset, .. } if next_offset.get() == 4
        ));
        assert_eq!(
            over_json(&mut capture, &chunk_request(&key, 1, 0, b"aa")),
            SaveReply::Refused {
                code: SaveCode::DuplicateOutsideWindow
            }
        );
    }

    #[test]
    fn abort_is_terminal_and_nothing_publishes_afterwards() {
        let (mut capture, _) = bound_capture(&owner());
        let key = key(&owner());
        over_json(
            &mut capture,
            &ClientRequest::Save(SaveRequest::Begin {
                owner: owner(),
                content_format: ContentFormat::NormalizedUtf8V1,
            }),
        );
        assert!(matches!(
            over_json(
                &mut capture,
                &ClientRequest::Save(SaveRequest::Abort { key: key.clone() })
            ),
            SaveReply::Aborted { .. }
        ));
        capture.approve(&owner()).unwrap();
        assert_eq!(
            over_json(
                &mut capture,
                &publish_request(&key, b"", SaveExtent::Complete)
            ),
            SaveReply::Refused {
                code: SaveCode::AlreadyTerminal
            }
        );
    }

    /// The shown prefix, its separators, and the remainder make up one import
    /// whose digest is specified independently of the chunking.
    #[test]
    fn a_shown_prefix_with_quotes_slashes_and_separators_seals_to_the_whole_digest() {
        let shown = "Shown: \"quoted\" \\ path/to\n";
        let remainder = "café — end\n";
        let whole = format!("{shown}{remainder}");
        let independent = Sha256Hex::try_from(
            "9e2f6b0a0f5f1c8a2a3d0e5f4b7c6d9e8f1a2b3c4d5e6f708192a3b4c5d6e7f8".to_owned(),
        );
        assert!(independent.is_ok(), "fixture digest literal is well formed");
        let (mut capture, _) = bound_capture(&owner());
        let key = key(&owner());
        over_json(
            &mut capture,
            &ClientRequest::Save(SaveRequest::Begin {
                owner: owner(),
                content_format: ContentFormat::NormalizedUtf8V1,
            }),
        );
        over_json(&mut capture, &chunk_request(&key, 1, 0, shown.as_bytes()));
        over_json(
            &mut capture,
            &chunk_request(&key, 2, shown.len() as u64, remainder.as_bytes()),
        );
        assert!(matches!(
            over_json(
                &mut capture,
                &seal_request(&key, whole.as_bytes(), SaveExtent::Complete)
            ),
            SaveReply::Sealed { .. }
        ));
    }

    #[test]
    fn checked_in_envelopes_parse_from_json_and_round_trip_through_the_wire_framing() {
        let fixtures: BTreeMap<String, serde_json::Value> =
            serde_json::from_str(FIXTURE).expect("fixture file parses");
        for (name, value) in fixtures
            .iter()
            .filter(|(name, _)| name.starts_with("request/"))
        {
            let request: ClientRequest = serde_json::from_value(value.clone())
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(
                serde_json::to_value(&request).unwrap(),
                *value,
                "{name} drifted"
            );
            let mut framed = Vec::new();
            encode_record(&request)
                .unwrap()
                .write_to(&mut framed)
                .unwrap();
            let decoded: ClientRequest = read_record(&mut framed.as_slice()).unwrap();
            assert_eq!(decoded, request, "{name} does not round-trip");
        }
        for (name, value) in fixtures
            .iter()
            .filter(|(name, _)| name.starts_with("reply/"))
        {
            let reply: ClientReply = serde_json::from_value(value.clone())
                .unwrap_or_else(|error| panic!("{name}: {error}"));
            assert_eq!(
                serde_json::to_value(&reply).unwrap(),
                *value,
                "{name} drifted"
            );
        }
        let unavailable: ClientReply =
            serde_json::from_value(fixtures["reply/unavailable"].clone()).unwrap();
        assert_eq!(
            unavailable,
            ClientReply::Save(SaveReply::Unavailable {
                code: SaveCode::CapabilityUnavailable
            })
        );
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use std::time::Duration;

    use super::*;

    /// A storage double recording exactly how often each physical call ran.
    #[derive(Default)]
    struct Recording {
        publishes: usize,
        receipt: Option<Locator>,
        readable: Option<Readability>,
        cleanups: usize,
        cleanup: Option<Cleanup>,
    }

    impl SaveStorage for Recording {
        fn append(&mut self, _key: &SaveKey, _offset: u64, _bytes: &[u8]) -> Result<(), SaveCode> {
            Ok(())
        }

        fn publish(&mut self, _key: &SaveKey, _seal: &SealRecord) -> Result<Locator, SaveCode> {
            self.publishes += 1;
            self.receipt.clone().ok_or(SaveCode::Capacity)
        }
        fn readability(&mut self, _locator: &Locator) -> Readability {
            self.readable.unwrap_or(Readability::Unknown)
        }
        fn cleanup(&mut self, _key: &SaveKey) -> Cleanup {
            self.cleanups += 1;
            self.cleanup.unwrap_or(Cleanup::Unknown)
        }
    }

    fn owner() -> SaveOwner {
        SaveOwner {
            incarnation: ServerIncarnation::from_uuid(Uuid::from_u128(1)),
            session: SessionId::new("ses_00000000-0000-7000-8000-000000000001"),
            instance: ClientInstanceId::new("instance-1"),
            generation: Generation::new(std::num::NonZeroU64::new(1).unwrap()),
            conversation: ConversationId::parse("conv-1").unwrap(),
            operation: OperationId::from_uuid(Uuid::from_u128(2)),
        }
    }

    fn key() -> SaveKey {
        SaveKey {
            owner: owner(),
            candidate: CandidateId::from_uuid(Uuid::from_u128(9)),
        }
    }

    /// A bound capture whose deadline is `start + five seconds`.
    fn bound(start: Instant) -> SaveCapture {
        let mut selection = SaveSelection::new(0);
        selection
            .observe(1, start, start + Duration::from_secs(60), || Some(owner()))
            .unwrap();
        SaveCapture::new(selection, CandidateId::from_uuid(Uuid::from_u128(9)))
    }

    fn chunk(sequence: u64, offset: u64, data: &[u8]) -> SaveRequest {
        SaveRequest::Chunk {
            key: key(),
            sequence: DecimalU64::new(sequence),
            offset: DecimalU64::new(offset),
            data_base64: base64::engine::general_purpose::STANDARD.encode(data),
            sha256: Sha256Hex::of(data),
        }
    }

    fn seal(data: &[u8]) -> SaveRequest {
        SaveRequest::Seal {
            key: key(),
            length: DecimalU64::new(data.len() as u64),
            sha256: Sha256Hex::of(data),
            extent: SaveExtent::Complete,
        }
    }

    fn publish(data: &[u8]) -> SaveRequest {
        SaveRequest::Publish {
            key: key(),
            length: DecimalU64::new(data.len() as u64),
            sha256: Sha256Hex::of(data),
            extent: SaveExtent::Complete,
        }
    }

    #[test]
    fn a_publish_without_a_physical_receipt_never_reports_published() {
        let start = Instant::now();
        let mut capture = bound(start);
        let mut storage = Recording::default();
        let data = b"text";
        capture.apply(
            &SaveRequest::Begin {
                owner: owner(),
                content_format: ContentFormat::NormalizedUtf8V1,
            },
            start,
            &mut storage,
        );
        capture.apply(&chunk(1, 0, data), start, &mut storage);
        capture.apply(&seal(data), start, &mut storage);
        capture.approve(&owner()).unwrap();
        let reply = capture.apply(&publish(data), start, &mut storage);
        assert_eq!(
            reply,
            SaveReply::Refused {
                code: SaveCode::Capacity
            }
        );
        assert_eq!(
            storage.publishes, 1,
            "the physical publish was attempted once"
        );
        // The candidate stays sealed: no locator was ever produced, and a retry
        // still reaches storage rather than reporting a publication.
        let retry = capture.apply(&publish(data), start, &mut storage);
        assert!(matches!(retry, SaveReply::Refused { .. }));
    }

    #[test]
    fn a_publish_with_a_receipt_yields_a_locator_distinct_from_the_candidate_and_probe_reports_storage()
     {
        let start = Instant::now();
        let mut capture = bound(start);
        let receipt = Locator::parse("save:fixture-receipt").unwrap();
        let mut storage = Recording {
            receipt: Some(receipt.clone()),
            readable: Some(Readability::Readable),
            ..Recording::default()
        };
        let data = b"text";
        capture.apply(
            &SaveRequest::Begin {
                owner: owner(),
                content_format: ContentFormat::NormalizedUtf8V1,
            },
            start,
            &mut storage,
        );
        capture.apply(&chunk(1, 0, data), start, &mut storage);
        capture.apply(&seal(data), start, &mut storage);
        capture.approve(&owner()).unwrap();
        let SaveReply::Published { locator, .. } =
            capture.apply(&publish(data), start, &mut storage)
        else {
            panic!("a physical receipt publishes");
        };
        assert_eq!(locator, receipt);
        assert!(
            !String::from(locator).contains("00000000"),
            "not derived from the candidate id"
        );
        // Probe reports whatever storage observes, independently of any verdict.
        storage.readable = Some(Readability::Missing);
        assert_eq!(
            capture.apply(&SaveRequest::Probe { key: key() }, start, &mut storage),
            SaveReply::Readability {
                key: key(),
                outcome: Readability::Missing
            }
        );
    }

    #[test]
    fn a_probe_before_publication_is_refused() {
        let start = Instant::now();
        let mut capture = bound(start);
        let mut storage = Recording::default();
        capture.apply(
            &SaveRequest::Begin {
                owner: owner(),
                content_format: ContentFormat::NormalizedUtf8V1,
            },
            start,
            &mut storage,
        );
        assert!(matches!(
            capture.apply(&SaveRequest::Probe { key: key() }, start, &mut storage),
            SaveReply::Refused { .. }
        ));
        assert_eq!(storage.publishes, 0);
    }

    #[test]
    fn abort_reports_the_storage_cleanup_outcome_and_is_terminal() {
        let start = Instant::now();
        let mut capture = bound(start);
        let mut storage = Recording {
            cleanup: Some(Cleanup::Pending),
            ..Recording::default()
        };
        capture.apply(
            &SaveRequest::Begin {
                owner: owner(),
                content_format: ContentFormat::NormalizedUtf8V1,
            },
            start,
            &mut storage,
        );
        assert_eq!(
            capture.apply(&SaveRequest::Abort { key: key() }, start, &mut storage),
            SaveReply::Aborted {
                key: key(),
                cleanup: Cleanup::Pending
            }
        );
        assert_eq!(storage.cleanups, 1);
        assert!(matches!(
            capture.apply(
                &SaveRequest::Begin {
                    owner: owner(),
                    content_format: ContentFormat::NormalizedUtf8V1
                },
                start,
                &mut storage
            ),
            SaveReply::Refused {
                code: SaveCode::AlreadyTerminal
            }
        ));
    }

    #[test]
    fn expired_chunk_seal_and_publish_are_deadline_gated_before_any_state_change() {
        let start = Instant::now();
        let mut capture = bound(start);
        let mut storage = Recording {
            receipt: Some(Locator::parse("save:fixture-receipt").unwrap()),
            ..Recording::default()
        };
        let data = b"late";
        let expired = start + OPTIONAL_SAVE_DEADLINE;
        capture.apply(
            &SaveRequest::Begin {
                owner: owner(),
                content_format: ContentFormat::NormalizedUtf8V1,
            },
            start,
            &mut storage,
        );
        assert_eq!(
            capture.apply(&chunk(1, 0, data), expired, &mut storage),
            SaveReply::Refused {
                code: SaveCode::Deadline
            },
            "a chunk at the expired instant is refused"
        );
        // The refused chunk changed nothing: sequence 1 is still the next chunk.
        assert!(matches!(
            capture.apply(&chunk(1, 0, data), start, &mut storage),
            SaveReply::Chunked { .. }
        ));
        assert_eq!(
            capture.apply(&seal(data), expired, &mut storage),
            SaveReply::Refused {
                code: SaveCode::Deadline
            }
        );
        capture.apply(&seal(data), start, &mut storage);
        capture.approve(&owner()).unwrap();
        assert_eq!(
            capture.apply(&publish(data), expired, &mut storage),
            SaveReply::Refused {
                code: SaveCode::Deadline
            }
        );
        assert_eq!(
            storage.publishes, 0,
            "no physical publish after the deadline"
        );
        // Abort remains valid after the deadline and reports storage's cleanup.
        storage.cleanup = Some(Cleanup::Removed);
        assert_eq!(
            capture.apply(&SaveRequest::Abort { key: key() }, expired, &mut storage),
            SaveReply::Aborted {
                key: key(),
                cleanup: Cleanup::Removed
            }
        );
    }

    #[test]
    fn a_known_business_result_is_not_repaired_when_publication_is_refused() {
        let start = Instant::now();
        let mut capture = bound(start);
        let mut storage = Recording::default();
        let data = b"text";
        capture.apply(
            &SaveRequest::Begin {
                owner: owner(),
                content_format: ContentFormat::NormalizedUtf8V1,
            },
            start,
            &mut storage,
        );
        capture.apply(&chunk(1, 0, data), start, &mut storage);
        capture.apply(&seal(data), start, &mut storage);
        capture.approve(&owner()).unwrap();
        assert!(matches!(
            capture.apply(&publish(data), start, &mut storage),
            SaveReply::Refused { .. }
        ));
        // No later success: the publish keeps failing without a receipt.
        storage.receipt = Some(Locator::parse("save:late").unwrap());
        assert!(matches!(
            capture.apply(&publish(b"other"), start, &mut storage),
            SaveReply::Refused { .. }
        ));
    }
}
