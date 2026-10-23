//! Bounded provider-independent facts for one auxiliary model Call.
//!
//! These types describe frozen execution coordinates and bounded consumer-defined attribution.
//! They carry no input bodies, generated output, credentials, budgets, Store
//! handles, or executor policy.

use serde::{Deserialize, Serialize};

use crate::context::resources::ResourceReadCut;
use crate::model::error::ModelErrorKind;
use crate::model::finish::ModelFinishReason;
use crate::model::generation_evidence::GenerationEvidence;
use crate::model::types::ModelUsage;
use crate::runtime::identity::{AuxiliaryCallId, CompactionOperationId, MessageId, RequestId};

/// Largest Call or request audit page returned by the durable Store.
pub const AUXILIARY_AUDIT_PAGE_LIMIT_MAX: usize = 32;

/// Largest encoded audit record the Store decodes into a typed projection.
///
/// Four KiB remains for the complete page-item identity/envelope and exact
/// decimal expansion of finite native integers.
pub const AUXILIARY_AUDIT_RECORD_ENCODED_MAX: usize = 12_288;

/// Largest borrowed encoding accepted for one Call start.
///
/// A Call start is also the eventual Call audit descriptor. Its 8 KiB budget
/// leaves at least 4 KiB for the 12 KiB audit-record limit after adding finite
/// sequence and terminal fields.
pub const AUXILIARY_CALL_START_ENCODED_MAX: usize = 8_192;

/// Largest borrowed encoding accepted for one request start.
pub const AUXILIARY_REQUEST_START_ENCODED_MAX: usize = 16_384;

/// Largest borrowed encoding accepted for one settlement envelope.
pub const AUXILIARY_TERMINAL_ENVELOPE_ENCODED_MAX: usize = 16_384;

/// Generation timing uses the common observation-safe millisecond domain.
pub const AUXILIARY_GENERATION_MS_MAX: u64 = 9_007_199_254_740_991;

pub(crate) fn generation_is_bounded(evidence: &GenerationEvidence) -> bool {
    [
        evidence.dispatch_after_start_ms,
        evidence.first_output_ms,
        evidence.last_output_ms,
        Some(evidence.terminal_ms),
    ]
    .into_iter()
    .flatten()
    .all(|value| value <= AUXILIARY_GENERATION_MS_MAX)
}

/// Largest number of explicitly selected `MessageId` references on one Call.
pub const AUXILIARY_INPUT_MESSAGE_REF_LIMIT_MAX: usize = 32;

/// Largest auxiliary `RequestId` accepted by the Store.
pub const AUXILIARY_REQUEST_ID_BYTES_MAX: usize = 512;

/// Largest provider-neutral auxiliary model name accepted by the Store.
pub const AUXILIARY_REQUEST_MODEL_BYTES_MAX: usize = 512;

/// Exact lowercase SHA-256 hexadecimal encoding width.
pub const AUXILIARY_FINGERPRINT_HEX_BYTES: usize = 64;

/// Bounded consumer-defined attribution, never execution authority.
///
/// Purpose names, activation causes and business references use this same text
/// contract. Consumers own their vocabulary; adding a consumer does not change
/// the Store schema. Values must not contain prompts, output or secrets.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(try_from = "String", into = "String")]
pub struct AuxiliaryAttribution(String);

impl AuxiliaryAttribution {
    /// Validates a nonempty attribution of at most 512 UTF-8 bytes.
    ///
    /// # Errors
    /// Rejects empty, oversized or control-character-containing values.
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if value.is_empty() || value.len() > 512 || value.chars().any(char::is_control) {
            return Err(
                "auxiliary attribution must contain 1..=512 UTF-8 bytes without control characters"
                    .into(),
            );
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AuxiliaryAttribution {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<AuxiliaryAttribution> for String {
    fn from(value: AuxiliaryAttribution) -> Self {
        value.0
    }
}

/// Terminal outcome of one auxiliary Call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuxiliaryCallOutcome {
    Completed,
    Failed,
    Cancelled,
}

/// Business relation retained for bounded attribution.
/// Native references retain their integrity checks; consumer references are
/// opaque attribution whose validation and authority belong to that consumer.
///
/// The relation grants no replay, execution, or inheritance authority.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuxiliaryBusinessSource {
    Independent,
    /// Exact external submission semantics. Temporary input/output bytes never
    /// enter this fact; the local Store index owns correlation for its lifetime.
    Submission {
        identity: AuxiliarySubmissionIdentity,
    },
    /// Consumer-owned business identity, not a native storage reference.
    Business {
        namespace: AuxiliaryAttribution,
        reference: AuxiliaryAttribution,
    },
    Compaction {
        operation_id: CompactionOperationId,
    },
    HumanMessage {
        message_id: MessageId,
    },
}

/// Versioned caller semantics, distinct from the frozen execution fingerprint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuxiliarySubmissionIdentity {
    pub encoding: u16,
    pub correlation: AuxiliaryAttribution,
    pub fingerprint: AuxiliaryFingerprint,
    pub scope: AuxiliaryAttribution,
}

/// Explicit authority used to interpret frozen input.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "mode", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuxiliaryInputEvidence {
    /// Complete native coordinates of the historical source selection.
    Historical { cut: ResourceReadCut },
    /// Input supplied directly to the Call. Its bytes are not retained here.
    Temporary,
}

/// Exact lowercase SHA-256 fingerprint of frozen semantic execution metadata.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(try_from = "String", into = "String")]
pub struct AuxiliaryFingerprint(String);

impl AuxiliaryFingerprint {
    /// Validates one exact SHA-256 hexadecimal fingerprint.
    ///
    /// # Errors
    /// Rejects noncanonical width, uppercase, or non-hexadecimal bytes.
    pub fn new(value: impl Into<String>) -> Result<Self, String> {
        let value = value.into();
        if value.len() != AUXILIARY_FINGERPRINT_HEX_BYTES
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return Err(format!(
                "auxiliary fingerprint must be exactly {AUXILIARY_FINGERPRINT_HEX_BYTES} lowercase hexadecimal bytes"
            ));
        }
        Ok(Self(value))
    }

    /// Returns the canonical lowercase hexadecimal value.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl TryFrom<String> for AuxiliaryFingerprint {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        Self::new(value)
    }
}

impl From<AuxiliaryFingerprint> for String {
    fn from(value: AuxiliaryFingerprint) -> Self {
        value.0
    }
}

/// Frozen identity, source relation, and input authority of one auxiliary Call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuxiliaryCallStart {
    pub call_id: AuxiliaryCallId,
    pub purpose: AuxiliaryAttribution,
    /// Consumer-defined activation cause, independent of transport and business source.
    pub trigger: AuxiliaryAttribution,
    pub business_source: AuxiliaryBusinessSource,
    pub input: AuxiliaryInputEvidence,
    /// Fingerprint of provider-visible frozen semantic input, never its bytes.
    pub semantic_input_fingerprint: AuxiliaryFingerprint,
    /// Fingerprint of the frozen invocation and resource revisions.
    pub invocation_fingerprint: AuxiliaryFingerprint,
    /// Bounded, unique canonical references explicitly selected by the caller.
    pub selected_message_ids: Vec<MessageId>,
}

/// Identity of one physical request inside an auxiliary Call.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuxiliaryRequestStart {
    pub call_id: AuxiliaryCallId,
    pub ordinal: u32,
    pub request_id: RequestId,
    /// Provider-neutral model name, byte-bounded by the Store.
    pub model: String,
}

/// Closed provider completion kind retained in auxiliary audit evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuxiliaryFinishReason {
    Stop,
    ToolCalls,
    Length,
    ContentFilter,
    Refusal,
    Other,
}

impl From<&ModelFinishReason> for AuxiliaryFinishReason {
    fn from(reason: &ModelFinishReason) -> Self {
        match reason {
            ModelFinishReason::Stop => Self::Stop,
            ModelFinishReason::ToolCalls => Self::ToolCalls,
            ModelFinishReason::Length => Self::Length,
            ModelFinishReason::ContentFilter => Self::ContentFilter,
            ModelFinishReason::Refusal => Self::Refusal,
            ModelFinishReason::Other { .. } => Self::Other,
        }
    }
}

/// Observed provider terminal, independent of accepted local execution.
///
/// Completion uses compact audit evidence rather than replacing the common
/// [`ModelFinishReason`]. Unknown provider prose remains only in original
/// diagnostics and is never copied into minimal audit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum AuxiliaryObservedProviderTerminal {
    Completed {
        finish_reason: AuxiliaryFinishReason,
    },
    Failed {
        kind: ModelErrorKind,
    },
}

/// Completeness of the request-scoped Usage evidence.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuxiliaryUsageCompleteness {
    Complete,
    Partial,
    Unreported,
}

/// Bounded request evidence carried by the final Call settlement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AuxiliaryRequestEvidence {
    pub request_id: RequestId,
    /// Absent only when no provider terminal was genuinely observed.
    pub observed_provider_terminal: Option<AuxiliaryObservedProviderTerminal>,
    /// Observed Usage; absence never means zero.
    pub usage: Option<ModelUsage>,
    pub usage_completeness: AuxiliaryUsageCompleteness,
    /// The physical request may have executed remotely despite local failure.
    pub remote_execution_uncertainty: bool,
}

/// Stable Journal cut for bounded auxiliary audit reads.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(transparent)]
pub struct AuxiliaryAuditCut {
    pub journal: u64,
}

/// Retained Call audit projection at one Journal cut.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AuxiliaryCallAudit {
    pub call_id: AuxiliaryCallId,
    pub purpose: AuxiliaryAttribution,
    pub trigger: AuxiliaryAttribution,
    pub business_source: AuxiliaryBusinessSource,
    pub input: AuxiliaryInputEvidence,
    pub semantic_input_fingerprint: AuxiliaryFingerprint,
    pub invocation_fingerprint: AuxiliaryFingerprint,
    pub selected_message_ids: Vec<MessageId>,
    pub started_sequence: u64,
    pub settled_sequence: Option<u64>,
    pub outcome: Option<AuxiliaryCallOutcome>,
}

/// Retained physical-request audit projection at one Journal cut.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AuxiliaryRequestAudit {
    pub call_id: AuxiliaryCallId,
    pub ordinal: u32,
    pub request_id: RequestId,
    pub model: String,
    pub started_sequence: u64,
    pub terminal_sequence: Option<u64>,
    pub local_terminal: Option<AuxiliaryRequestTerminal>,
    pub normalized_failure_kind: Option<ModelErrorKind>,
    pub generation: Option<GenerationEvidence>,
    /// Observed Usage from the physical terminal, never an inferred zero.
    pub usage: Option<ModelUsage>,
    /// Present once the paired retry or final Call settlement is visible at this cut.
    pub request_evidence: Option<AuxiliaryRequestEvidence>,
}

/// Closed physical terminal classes for an auxiliary request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AuxiliaryRequestTerminal {
    Completed,
    Failed,
}

/// Typed auxiliary fact in one audit page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub enum AuxiliaryAuditFact {
    CallStarted(AuxiliaryCallAudit),
    RequestStarted(AuxiliaryRequestAudit),
    RequestSettled(AuxiliaryRequestAudit),
    RetryScheduled(AuxiliaryRequestAudit),
    CallSettled(AuxiliaryCallAudit),
}

/// One audit fact with its dense offset and original Journal coordinate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AuxiliaryAuditRecord {
    pub audit_offset: u64,
    pub journal_sequence: u64,
    pub fact: AuxiliaryAuditFact,
}

/// Bounded page at one stable auxiliary audit cut.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AuxiliaryAuditPage {
    pub cut: AuxiliaryAuditCut,
    /// Last dense audit offset visible at the captured Journal cut.
    pub total: u64,
    pub records: Vec<AuxiliaryAuditRecord>,
    /// Cursor for the next page, present only when another fact exists.
    pub next_offset: Option<u64>,
}

/// Whether a semantic transition allocated new facts or replayed an exact one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuxiliaryCommitDisposition {
    NewlyCommitted,
    IdempotentReplay,
}

/// Typed result of authoritative confirmation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuxiliaryCommitConfirmation<T> {
    ConfirmedCommitted(T),
    ConfirmedNotCommitted,
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::{
        AUXILIARY_FINGERPRINT_HEX_BYTES, AuxiliaryAttribution, AuxiliaryFingerprint,
        AuxiliaryFinishReason,
    };
    use crate::model::finish::ModelFinishReason;

    #[test]
    fn fingerprints_are_exact_lowercase_sha256_hex() {
        let canonical = "ab".repeat(AUXILIARY_FINGERPRINT_HEX_BYTES / 2);
        assert_eq!(
            AuxiliaryFingerprint::new(canonical.clone())
                .unwrap()
                .as_str(),
            canonical
        );
        for invalid in [
            String::new(),
            "A".repeat(AUXILIARY_FINGERPRINT_HEX_BYTES),
            "g".repeat(AUXILIARY_FINGERPRINT_HEX_BYTES),
            "a".repeat(AUXILIARY_FINGERPRINT_HEX_BYTES - 1),
            "a".repeat(AUXILIARY_FINGERPRINT_HEX_BYTES + 1),
        ] {
            assert!(AuxiliaryFingerprint::new(invalid).is_err());
        }
    }

    #[test]
    fn consumer_attribution_round_trips_and_rejects_invalid_values() {
        for name in [
            "summary",
            "translation",
            "scheduled_refresh",
            "consumer:source",
        ] {
            let value = AuxiliaryAttribution::new(name).unwrap();
            let encoded = serde_json::to_string(&value).unwrap();
            assert_eq!(
                serde_json::from_str::<AuxiliaryAttribution>(&encoded).unwrap(),
                value
            );
        }
        for value in [
            String::new(),
            "x".repeat(513),
            "界".repeat(171),
            "line\nbreak".into(),
        ] {
            assert!(AuxiliaryAttribution::new(value.clone()).is_err());
            assert!(
                serde_json::from_value::<AuxiliaryAttribution>(serde_json::json!(value)).is_err()
            );
        }
        assert!(AuxiliaryAttribution::new("x".repeat(512)).is_ok());
    }

    #[test]
    fn observed_finish_reason_is_closed_and_drops_unknown_prose() {
        assert_eq!(
            AuxiliaryFinishReason::from(&ModelFinishReason::ToolCalls),
            AuxiliaryFinishReason::ToolCalls
        );
        assert_eq!(
            AuxiliaryFinishReason::from(&ModelFinishReason::Other {
                reason: "PRIVATE_PROVIDER_FINISH".into(),
            }),
            AuxiliaryFinishReason::Other
        );
        assert_eq!(
            serde_json::to_value(AuxiliaryFinishReason::Other).unwrap(),
            "other"
        );
    }
}
