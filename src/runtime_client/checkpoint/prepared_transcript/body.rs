//! Immutable body normalization, independent of the current native response facts.
use super::{
    Arc, CHECKPOINT_TARGET_BYTES, Capture, ConversationId, OnceLock, PreparedCapture,
    RuntimeClientTranscriptEntry,
};
use crate::runtime_client::snapshot::{
    ForegroundToolExecution, RuntimeClientTranscriptCursor, RuntimeClientTranscriptItem,
};
use serde::{Deserialize, Serialize, ser::SerializeStruct};

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct Value {
    tool_calls: Vec<ForegroundToolExecution>,
    cursor: RuntimeClientTranscriptCursor,
    item: RuntimeClientTranscriptItem,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(super) struct Decoration {
    #[serde(skip_serializing_if = "Option::is_none")]
    turn_process: Option<crate::runtime_client::response::TurnProcessView>,
    response_pending: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    completed_response: Option<crate::runtime_client::response::CompletedResponseView>,
}
impl Decoration {
    pub(super) fn from_entry(value: &RuntimeClientTranscriptEntry) -> Self {
        Self {
            turn_process: value.turn_process.clone(),
            response_pending: value.response_pending,
            completed_response: value.completed_response.clone(),
        }
    }
    fn fallback(&self) -> Self {
        Self {
            turn_process: super::super::bounded_clone(&self.turn_process, 2048).flatten(),
            response_pending: self.response_pending,
            completed_response: super::super::bounded_clone(&self.completed_response, 2048)
                .flatten(),
        }
    }
}

pub(super) struct Inline<T> {
    value: T,
    raw_required: usize,
    encoded: usize,
    required: usize,
    safe: Option<usize>,
}
fn inline<T: Serialize + serde::de::DeserializeOwned>(
    value: &T,
    raw_limit: usize,
) -> Option<Inline<T>> {
    // The body has already consumed its exact conservative part of the whole
    // row's admission. Reject an oversized decoration before encoding/parsing
    // a value that the original whole-row branch cannot admit.
    let raw_required = crate::runtime_client::preflight::required_budget(value, raw_limit)?;
    #[cfg(test)]
    INLINE_NORMALIZATIONS.with(|work| work.set(work.get() + 1));
    let (bytes, value) = super::super::bounded_item_admitted(value, raw_limit)?;
    let raw_required = raw_required.max(bytes.len());
    let value: T = serde_json::from_value(value).ok()?;
    let (encoded, required) = measure(&value, CHECKPOINT_TARGET_BYTES)?;
    let safe = crate::runtime_client::preflight::required_safe_integer_budget(
        &value,
        CHECKPOINT_TARGET_BYTES,
    );
    Some(Inline {
        value,
        raw_required,
        encoded,
        required,
        safe,
    })
}

pub(super) struct Owner {
    conversation: ConversationId,
    value: Value,
    inline: OnceLock<Option<Inline<Arc<Normalized>>>>,
    fallback: OnceLock<PreparedCapture<Option<Arc<Normalized>>>>,
}
impl std::fmt::Debug for Owner {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        out.debug_struct("TranscriptBody")
            .field("conversation", &self.conversation)
            .field("value", &self.value)
            .finish_non_exhaustive()
    }
}
#[derive(Clone, Serialize)]
#[serde(transparent)]
struct Normalized {
    value: Value,
    #[serde(skip)]
    measured: Option<(usize, usize)>,
}
fn measure(value: &impl Serialize, limit: usize) -> Option<(usize, usize)> {
    let required = crate::runtime_client::preflight::required_budget(value, limit)?;
    // The same admission walk was just completed; keep the original capped
    // JSON writer for exact length and representability without walking twice.
    let mut length = super::super::Length { bytes: 0, limit };
    serde_json::to_writer(&mut length, value).ok()?;
    Some((length.bytes, required.max(length.bytes)))
}
impl Owner {
    pub(super) fn new(
        conversation: ConversationId,
        value: RuntimeClientTranscriptEntry,
    ) -> (Arc<Self>, Decoration) {
        let RuntimeClientTranscriptEntry {
            tool_calls,
            cursor,
            item,
            turn_process,
            response_pending,
            completed_response,
        } = value;
        (
            Arc::new(Self {
                conversation,
                value: Value {
                    tool_calls,
                    cursor,
                    item,
                },
                inline: OnceLock::new(),
                fallback: OnceLock::new(),
            }),
            Decoration {
                turn_process,
                response_pending,
                completed_response,
            },
        )
    }
    fn inline(&self) -> Option<&Inline<Arc<Normalized>>> {
        self.inline
            .get_or_init(|| {
                #[cfg(test)]
                record_body_work(0);
                inline(&self.value, super::super::CHECKPOINT_ITEM_BYTES).map(|part| Inline {
                    value: Arc::new(Normalized {
                        value: part.value,
                        measured: Some((part.encoded, part.required)),
                    }),
                    raw_required: part.raw_required,
                    encoded: part.encoded,
                    required: part.required,
                    safe: part.safe,
                })
            })
            .as_ref()
    }
    #[cfg(test)]
    pub(super) fn entry(&self, decoration: &Decoration) -> RuntimeClientTranscriptEntry {
        self.value.entry(decoration)
    }
    pub(super) fn equal(&self, other: &Self) -> bool {
        self.conversation == other.conversation && self.value == other.value
    }
    fn fallback(&self, capture: &mut Capture) -> Option<Arc<Normalized>> {
        #[cfg(test)]
        record_body_work(1);
        let path = ["transcript".into(), "entries".into(), "0".into()];
        let item = match &self.value.item {
            RuntimeClientTranscriptItem::Message { message } => {
                let mut field = path.to_vec();
                field.extend(["item".into(), "message".into()]);
                RuntimeClientTranscriptItem::Message {
                    message: capture.message(message, &self.conversation, &field),
                }
            }
            RuntimeClientTranscriptItem::PublicationAudit { audit } => {
                RuntimeClientTranscriptItem::PublicationAudit {
                    audit: capture.audit(audit, &self.conversation, &path)?,
                }
            }
            other => super::super::bounded_clone(other, super::super::CHECKPOINT_ITEM_BYTES)?,
        };
        let value = Value {
            cursor: self.value.cursor,
            item,
            tool_calls: capture.page("transcript/entries/0/tool_calls", &self.value.tool_calls),
        };
        let measured = measure(&value, super::super::CHECKPOINT_ITEM_BYTES);
        Some(Arc::new(Normalized { value, measured }))
    }
}
impl Value {
    fn entry(&self, decoration: &Decoration) -> RuntimeClientTranscriptEntry {
        RuntimeClientTranscriptEntry {
            tool_calls: self.tool_calls.clone(),
            cursor: self.cursor,
            item: self.item.clone(),
            turn_process: decoration.turn_process.clone(),
            response_pending: decoration.response_pending,
            completed_response: decoration.completed_response.clone(),
        }
    }
    fn into_entry(self, decoration: Decoration) -> RuntimeClientTranscriptEntry {
        RuntimeClientTranscriptEntry {
            tool_calls: self.tool_calls,
            cursor: self.cursor,
            item: self.item,
            turn_process: decoration.turn_process,
            response_pending: decoration.response_pending,
            completed_response: decoration.completed_response,
        }
    }
}

/// The original six fields and order, borrowing the complete normalized body.
/// No public DTO, schema or disclosure boundary changes.
#[derive(Clone)]
pub(super) struct Captured {
    body: Arc<Normalized>,
    decoration: Decoration,
}
impl Serialize for Captured {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut value = serializer.serialize_struct(
            "RuntimeClientTranscriptEntry",
            4 + usize::from(self.decoration.turn_process.is_some())
                + usize::from(self.decoration.completed_response.is_some()),
        )?;
        if self.decoration.turn_process.is_some() {
            value.serialize_field("turn_process", &self.decoration.turn_process)?;
        }
        value.serialize_field("response_pending", &self.decoration.response_pending)?;
        if self.decoration.completed_response.is_some() {
            value.serialize_field("completed_response", &self.decoration.completed_response)?;
        }
        value.serialize_field("tool_calls", &self.body.value.tool_calls)?;
        value.serialize_field("cursor", &self.body.value.cursor)?;
        value.serialize_field("item", &self.body.value.item)?;
        value.end()
    }
}
impl Captured {
    pub(super) fn entry(&self) -> RuntimeClientTranscriptEntry {
        self.body.value.entry(&self.decoration)
    }
    pub(super) fn into_entry(self) -> RuntimeClientTranscriptEntry {
        Arc::unwrap_or_clone(self.body)
            .value
            .into_entry(self.decoration)
    }
}

/// Return the original row decision and its safe-integer charge. The body and
/// decoration are independent objects with at most three fields each. Joining
/// them removes one outer-container charge (two) and replaces `}{` with a
/// comma in the JSON length (one); nested limits and normalization are unchanged.
pub(super) fn capture(
    owner: &Owner,
    decoration: &Decoration,
    capture: &mut Capture,
) -> (Option<Arc<Captured>>, Option<usize>) {
    let content = capture.content.len();
    if let Some(body) = owner.inline()
        && let Some(decoration) = inline(
            decoration,
            super::super::CHECKPOINT_ITEM_BYTES + 2 - body.raw_required,
        )
    {
        let required = body.required + decoration.required - 2;
        let encoded = body.encoded + decoration.encoded - 1;
        // The first, whole-row bounded_clone branch succeeds. Its result
        // then uses the original final item_budget without new metadata.
        if required > capture.item_budget() {
            return (None, Some(4));
        }
        capture.remaining -= encoded;
        let safe = body
            .safe
            .zip(decoration.safe)
            .map(|(body, decoration)| body + decoration - 1);
        return (
            Some(Arc::new(Captured {
                body: body.value.clone(),
                decoration: decoration.value,
            })),
            safe,
        );
    }
    // Decoration admission is a fixed 2048-byte operation with no Capture
    // side effects. Body replay retains item -> Tool-page admission order.
    let (body, body_fit) = capture.prepared(&owner.fallback, |capture| owner.fallback(capture));
    let Some(body) = body.into_owned() else {
        capture.content.truncate(content);
        return (None, Some(4));
    };
    let decoration = decoration.fallback();
    let projected = Captured { body, decoration };
    let limit = capture.item_budget();
    let measured = projected.body.measured.zip(measure(
        &projected.decoration,
        super::super::CHECKPOINT_ITEM_BYTES,
    ));
    let Some(((body_bytes, _), (decoration_bytes, _))) =
        measured.filter(|((_, body), (_, decoration))| body + decoration - 2 <= limit)
    else {
        capture.content.truncate(content);
        return (None, Some(4));
    };
    capture.remaining -= body_bytes + decoration_bytes - 1;
    // The fallback's normalized body was measured once. Only the current
    // scalar decoration needs another safe-integer charge.
    let decoration_fit = crate::runtime_client::preflight::required_safe_integer_budget(
        &projected.decoration,
        CHECKPOINT_TARGET_BYTES,
    );
    let fit = body_fit
        .zip(decoration_fit)
        .map(|(body, decoration)| body + decoration - 2);
    (Some(Arc::new(projected)), fit)
}

#[cfg(test)]
thread_local! { static BODY_WORK: std::cell::Cell<[usize; 2]> = const { std::cell::Cell::new([0; 2]) }; }
#[cfg(test)]
fn record_body_work(index: usize) {
    BODY_WORK.with(|work| {
        let mut value = work.get();
        value[index] += 1;
        work.set(value);
    });
}
#[cfg(test)]
pub(super) fn take_body_work() -> [usize; 2] {
    BODY_WORK.with(|work| work.replace([0; 2]))
}

#[cfg(test)]
thread_local! { static INLINE_NORMALIZATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
#[cfg(test)]
pub(super) fn take_inline_normalizations() -> usize {
    INLINE_NORMALIZATIONS.with(|work| work.replace(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    struct Counted<'a, T> {
        value: &'a T,
        calls: &'a Cell<usize>,
    }
    impl<T: Serialize> Serialize for Counted<'_, T> {
        fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
            self.calls.set(self.calls.get() + 1);
            self.value.serialize(serializer)
        }
    }

    #[test]
    fn transcript_normalized_measure_visits_budget_once_and_keeps_exact_json_length() {
        #[derive(Serialize)]
        struct Native {
            quantity: u64,
            opaque: serde_json::Value,
            text: String,
        }
        for quantity in [1, u64::MAX] {
            let value = Native {
                quantity,
                opaque: serde_json::json!({"z": ["水", "\\\n\""], "a": null}),
                text: "escaped\n水\"\\".repeat(3),
            };
            let required =
                crate::runtime_client::preflight::required_budget(&value, CHECKPOINT_TARGET_BYTES)
                    .unwrap();
            for limit in [required - 1, required, required + 1] {
                let calls = Cell::new(0);
                let actual = measure(
                    &Counted {
                        value: &value,
                        calls: &calls,
                    },
                    limit,
                );
                let expected = super::super::super::bounded_len(&value, limit)
                    .map(|encoded| (encoded, required.max(encoded)));
                assert_eq!(actual, expected);
                assert_eq!(calls.get(), if limit < required { 1 } else { 2 });
                if let Some((encoded, _)) = actual {
                    assert_eq!(encoded, serde_json::to_vec(&value).unwrap().len());
                }
            }
        }
    }

    #[test]
    fn transcript_admitted_normalization_keeps_container_and_json_rejection() {
        // The prepared raw pass and the capped writer each visit the value
        // once; the original parse/count/typed normalization still run.
        let calls = Cell::new(0);
        let value = vec!["water 水", "escaped\n\""];
        let counted = Counted {
            value: &value,
            calls: &calls,
        };
        let required =
            crate::runtime_client::preflight::required_budget(&counted, CHECKPOINT_TARGET_BYTES)
                .unwrap();
        let (bytes, normalized) =
            super::super::super::bounded_item_admitted(&counted, required).unwrap();
        assert_eq!(calls.get(), 2);
        assert_eq!(bytes, serde_json::to_vec(&value).unwrap());
        assert_eq!(normalized, serde_json::to_value(&value).unwrap());
        for value in [
            serde_json::json!([vec![0; super::super::super::CHECKPOINT_PAGE_ITEMS + 1]]),
            serde_json::json!({"unsafe": u64::MAX}),
        ] {
            let expected = super::super::super::bounded_item(&value, CHECKPOINT_TARGET_BYTES);
            let actual =
                crate::runtime_client::preflight::required_budget(&value, CHECKPOINT_TARGET_BYTES)
                    .and_then(|_| {
                        super::super::super::bounded_item_admitted(&value, CHECKPOINT_TARGET_BYTES)
                    });
            assert!(expected.is_none());
            assert_eq!(actual, expected);
        }
        let invalid_key = std::collections::BTreeMap::from([(vec![1, 2], "value")]);
        assert!(
            crate::runtime_client::preflight::required_budget(
                &invalid_key,
                CHECKPOINT_TARGET_BYTES
            )
            .is_some()
        );
        assert!(
            super::super::super::bounded_item_admitted(&invalid_key, CHECKPOINT_TARGET_BYTES)
                .is_none()
        );
    }
}
