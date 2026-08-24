//! The backend-independent durable contract of the human interaction audit
//! (Issue #109).
//!
//! This module owns three things and nothing else:
//!
//! ```text
//! InteractionSubject / InteractionSettlement  the durable audit vocabulary
//! interaction_arguments_digest                the wire definition of the argument pin
//! validate_interaction_*                      the bounded-payload contract
//! ```
//!
//! It is deliberately provider-independent and storage-backend-independent.
//! The live [`InteractionCoordinator`](crate::runtime::InteractionCoordinator)
//! validates a request or a response through exactly these functions before it
//! publishes, and the durable authority validates the very same facts through
//! exactly these functions before it commits. There is therefore one semantic
//! source for every bound, and a future non-`SQLite` backend enforces the same
//! contract by calling the same functions rather than by restating the limits.
//!
//! The bounds are a **durable-store invariant**, not merely a coordinator
//! convention: [`InteractionSubject`] and [`InteractionSettlement`] are
//! deserializable event payloads, so a fact that bypassed the coordinator
//! entirely must still be refused by the store.

use serde::{Deserialize, Serialize};

use crate::runtime::identity::{ToolCallId, ToolId};
use crate::runtime::interaction::QuestionAnswer;
use crate::runtime::types::CancellationReason;

/// The longest human-facing Question prompt one interaction may carry.
pub const MAX_QUESTION_PROMPT_CHARS: usize = 4096;
/// The largest finite choice list one Question may offer.
pub const MAX_QUESTION_CHOICES: usize = 32;
/// The longest single choice label one Question may offer.
pub const MAX_QUESTION_CHOICE_CHARS: usize = 256;
/// The longest typed answer a Question settlement may retain.
pub const MAX_QUESTION_ANSWER_CHARS: usize = 4096;
/// The longest policy explanation an Approval **request** may carry.
///
/// This is the reason rustX shows the user when it asks; it is deliberately a
/// different bound from [`MAX_APPROVAL_DENIAL_REASON_CHARS`], which bounds the
/// reason the user gives back when refusing.
pub const MAX_APPROVAL_REQUEST_REASON_CHARS: usize = 1024;
/// The longest client-facing reason an Approval **denial** may carry.
pub const MAX_APPROVAL_DENIAL_REASON_CHARS: usize = 1024;
/// The longest model-facing tool name an Approval subject may name.
pub const MAX_APPROVAL_TOOL_NAME_CHARS: usize = 256;

/// The domain separation prefix of the interaction argument digest.
///
/// It is part of the durable wire contract: changing it changes every stored
/// `arguments_digest`, so it is versioned in the value itself.
const ARGUMENTS_DIGEST_DOMAIN: &[u8] = b"rustx-interaction-arguments-v1\n";

/// The exact character length of a lowercase hex SHA-256 digest.
const DIGEST_HEX_CHARS: usize = 64;

/// The bounded by-value audit subject of one interaction (Issue #109).
///
/// This is deliberately **not** the live
/// [`InteractionKind`](crate::runtime::InteractionKind). The live request
/// carries the complete validated tool arguments so a client can render the
/// exact invocation; the durable audit keeps a bounded record instead. The
/// approval subject names the exact call/tool identity by value and pins the
/// canonical model-issued argument value by digest, so the audit stays O(1) in
/// size while remaining **verifiable** against the canonical `ToolCall` that
/// the Message Ledger already owns by value. Nothing here is resolved through
/// the current Tool registry or the current approval policy, so the subject
/// stays readable after any resource reload or restart.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InteractionSubject {
    /// A tool invocation was held at the pre-tool policy boundary.
    Approval {
        /// The canonical model-issued call identity.
        call_id: ToolCallId,
        /// The registry-resolved tool identity, frozen at request time. It is
        /// the identity the canonical `ToolCall` froze.
        tool_id: ToolId,
        /// The model-facing tool name, frozen at request time. It is the name
        /// the canonical `ToolCall` froze.
        tool_name: String,
        /// The [`interaction_arguments_digest`] of the exact canonical
        /// model-issued `ToolCall` arguments this approval was asked about.
        arguments_digest: String,
        /// The bounded policy explanation shown to the client.
        reason: String,
    },
    /// One bounded question was asked of the user. The prompt and choices are
    /// already bounded by the Question contract, so they are stored by value.
    Question {
        /// The bounded human-facing prompt.
        prompt: String,
        /// The finite choice labels, when the Question offered any.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        choices: Option<Vec<String>>,
        /// Whether a free-text answer was accepted.
        allow_free_text: bool,
    },
}

/// The bounded terminal settlement of one interaction (Issue #109).
///
/// There is deliberately no "unavailable" settlement: an interaction that
/// never found a capable client is refused before the requested fact commits,
/// so no half-open audit record is created for a prompt no user ever saw.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InteractionSettlement {
    /// A client allowed the exact approval subject. This is audit evidence of
    /// a decision that existed; it is never restart authorization.
    Approved,
    /// A client denied the exact approval subject. The canonical result slot
    /// of the call carries the matching denied `ToolResult`.
    Denied {
        /// The bounded client-facing denial reason.
        reason: String,
    },
    /// The user answered the exact Question subject. The answer is retained
    /// by value: it is the audit evidence of what the user actually said,
    /// independent of the canonical tool result that carries it to the model.
    Answered {
        /// The exact typed answer accepted by the coordinator.
        answer: QuestionAnswer,
    },
    /// The owning attempt's cancellation authority won the rendezvous before
    /// any user decision was accepted.
    Cancelled {
        /// The first-winner cancellation cause of the owning attempt.
        reason: CancellationReason,
    },
}

/// The lowercase hex SHA-256 that one approval subject pins its arguments by.
///
/// The audit pins the argument value rather than copying it: the Journal is
/// the low-frequency plane, and the argument value itself is already durable
/// by-value in the canonical `ToolCall` the Message Ledger owns. That is only
/// an honest claim if the digest is taken over **that** value, so this function
/// is the single definition used by the coordinator that writes the subject and
/// by the durable authority that verifies it against the Ledger.
///
/// `serde_json::Value` serializes object keys in sorted order, so the same
/// logical arguments always produce the same digest.
#[must_use]
pub fn interaction_arguments_digest(arguments: &serde_json::Value) -> String {
    use sha2::{Digest, Sha256};
    use std::fmt::Write as _;

    let mut hasher = Sha256::new();
    hasher.update(ARGUMENTS_DIGEST_DOMAIN);
    hasher.update(arguments.to_string().as_bytes());
    let mut digest = String::with_capacity(DIGEST_HEX_CHARS);
    for byte in hasher.finalize() {
        let _ = write!(digest, "{byte:02x}");
    }
    digest
}

/// Whether a string is the canonical lowercase hex representation of a
/// SHA-256 digest. An `arguments_digest` is a pin, not free text.
fn is_canonical_digest(value: &str) -> bool {
    value.len() == DIGEST_HEX_CHARS
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

/// Validates the bounded Question contract shared by the live `ask_user`
/// arguments, the coordinator's publication path, and the durable audit
/// subject.
///
/// # Errors
///
/// Returns a bounded diagnostic when the prompt, choice list, or answer mode
/// cannot produce an answerable Question.
pub fn validate_question_contract(
    prompt: &str,
    choices: Option<&[String]>,
    allow_free_text: bool,
) -> Result<(), String> {
    if prompt.is_empty() {
        return Err("prompt must not be empty".to_owned());
    }
    if prompt.chars().count() > MAX_QUESTION_PROMPT_CHARS {
        return Err(format!(
            "prompt exceeds {MAX_QUESTION_PROMPT_CHARS} characters"
        ));
    }
    let Some(choices) = choices else {
        if allow_free_text {
            return Ok(());
        }
        return Err("allow_free_text must be true when choices is omitted".to_owned());
    };
    if choices.is_empty() {
        return Err("choices must contain at least one value when present".to_owned());
    }
    if choices.len() > MAX_QUESTION_CHOICES {
        return Err(format!(
            "choices must contain at most {MAX_QUESTION_CHOICES} values"
        ));
    }
    if choices
        .iter()
        .any(|choice| choice.is_empty() || choice.chars().count() > MAX_QUESTION_CHOICE_CHARS)
    {
        return Err(format!(
            "each choice must be non-empty and at most {MAX_QUESTION_CHOICE_CHARS} characters"
        ));
    }
    let mut unique = std::collections::BTreeSet::new();
    if choices.iter().any(|choice| !unique.insert(choice)) {
        return Err("choices must not contain duplicates".to_owned());
    }
    Ok(())
}

/// Validates the bounded payload contract of one durable audit subject.
///
/// This is a pure structural contract: it proves the payload is bounded and
/// internally answerable. Whether an Approval subject actually corresponds to
/// the canonical `ToolCall` it names is a *cross-domain* check that only the
/// durable authority can make, because only the store holds the Message
/// Ledger.
///
/// # Errors
///
/// Returns a bounded diagnostic naming the violated bound.
pub fn validate_interaction_subject(subject: &InteractionSubject) -> Result<(), String> {
    match subject {
        InteractionSubject::Approval {
            call_id,
            tool_id,
            tool_name,
            arguments_digest,
            reason,
        } => {
            if call_id.as_str().is_empty() {
                return Err("approval subject must name a non-empty tool call".to_owned());
            }
            if tool_id.as_str().is_empty() {
                return Err("approval subject must name a non-empty tool id".to_owned());
            }
            if tool_name.is_empty() {
                return Err("approval subject must name a non-empty tool name".to_owned());
            }
            if tool_name.chars().count() > MAX_APPROVAL_TOOL_NAME_CHARS {
                return Err(format!(
                    "approval tool name exceeds {MAX_APPROVAL_TOOL_NAME_CHARS} characters"
                ));
            }
            if !is_canonical_digest(arguments_digest) {
                return Err(
                    "approval arguments digest must be a lowercase hex SHA-256 digest".to_owned(),
                );
            }
            if reason.chars().count() > MAX_APPROVAL_REQUEST_REASON_CHARS {
                return Err(format!(
                    "approval request reason exceeds {MAX_APPROVAL_REQUEST_REASON_CHARS} characters"
                ));
            }
            Ok(())
        }
        InteractionSubject::Question {
            prompt,
            choices,
            allow_free_text,
        } => validate_question_contract(prompt, choices.as_deref(), *allow_free_text),
    }
}

/// Validates one terminal settlement against the exact subject it settles.
///
/// This is stricter than "the variant is legal for this subject". The durable
/// audit claims to record the typed answer that was actually accepted, so a
/// `Choice` must be one the requested Question offered and a `FreeText` answer
/// requires a Question that accepted free text. A settlement that no live
/// coordinator could ever have produced is a semantically false audit record
/// and is refused.
///
/// # Errors
///
/// Returns a bounded diagnostic when the settlement is not a terminal this
/// exact subject could produce, or when its payload is unbounded.
pub fn validate_interaction_settlement(
    subject: &InteractionSubject,
    settlement: &InteractionSettlement,
) -> Result<(), String> {
    match (subject, settlement) {
        // Cancellation is the one terminal both subjects share, and its cause
        // is a finite enum, so it carries no unbounded payload.
        (_, InteractionSettlement::Cancelled { .. })
        | (InteractionSubject::Approval { .. }, InteractionSettlement::Approved) => Ok(()),
        (InteractionSubject::Approval { .. }, InteractionSettlement::Denied { reason }) => {
            if reason.chars().count() > MAX_APPROVAL_DENIAL_REASON_CHARS {
                return Err(format!(
                    "approval denial reason exceeds {MAX_APPROVAL_DENIAL_REASON_CHARS} characters"
                ));
            }
            Ok(())
        }
        (
            InteractionSubject::Question {
                choices,
                allow_free_text,
                ..
            },
            InteractionSettlement::Answered { answer },
        ) => {
            let (kind, value) = match answer {
                QuestionAnswer::Choice { value } => ("choice", value),
                QuestionAnswer::FreeText { value } => ("free text", value),
            };
            if value.is_empty() || value.chars().count() > MAX_QUESTION_ANSWER_CHARS {
                return Err(format!(
                    "{kind} answer is empty or exceeds {MAX_QUESTION_ANSWER_CHARS} characters"
                ));
            }
            match answer {
                QuestionAnswer::Choice { .. } => {
                    if choices
                        .as_ref()
                        .is_none_or(|choices| !choices.iter().any(|choice| choice == value))
                    {
                        return Err("question choice is not one of the offered choices".to_owned());
                    }
                    Ok(())
                }
                QuestionAnswer::FreeText { .. } if !allow_free_text => {
                    Err("question does not accept free text".to_owned())
                }
                QuestionAnswer::FreeText { .. } => Ok(()),
            }
        }
        _ => Err(
            "the interaction settlement is a terminal its requested subject cannot produce"
                .to_owned(),
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approval(reason: &str) -> InteractionSubject {
        InteractionSubject::Approval {
            call_id: ToolCallId::new("call-1"),
            tool_id: ToolId::new("tool.read"),
            tool_name: "read".to_owned(),
            arguments_digest: interaction_arguments_digest(&serde_json::json!({"path": "a"})),
            reason: reason.to_owned(),
        }
    }

    /// The digest is a stable wire contract: the domain prefix and the
    /// canonical JSON form are pinned by an independently computed expectation.
    #[test]
    fn the_arguments_digest_is_a_pinned_wire_contract() {
        use sha2::{Digest, Sha256};
        use std::fmt::Write as _;
        let arguments = serde_json::json!({"b": 2, "a": 1});
        let mut hasher = Sha256::new();
        hasher.update(b"rustx-interaction-arguments-v1\n");
        hasher.update(br#"{"a":1,"b":2}"#);
        let mut expected = String::new();
        for byte in hasher.finalize() {
            write!(expected, "{byte:02x}").expect("hex expectation");
        }
        assert_eq!(interaction_arguments_digest(&arguments), expected);
        // Key order in the source value cannot change the pin.
        assert_eq!(
            interaction_arguments_digest(&serde_json::json!({"a": 1, "b": 2})),
            expected
        );
        assert!(is_canonical_digest(&expected));
    }

    #[test]
    fn an_arguments_digest_must_be_canonical_lowercase_hex() {
        assert!(!is_canonical_digest(""));
        assert!(!is_canonical_digest(&"0".repeat(63)));
        assert!(!is_canonical_digest(&"A".repeat(64)));
        assert!(!is_canonical_digest(&"g".repeat(64)));
        assert!(is_canonical_digest(&"0".repeat(64)));
    }

    #[test]
    fn approval_subject_bounds_are_enforced() {
        assert!(validate_interaction_subject(&approval("policy asked")).is_ok());
        assert!(
            validate_interaction_subject(&approval(
                &"x".repeat(MAX_APPROVAL_REQUEST_REASON_CHARS + 1)
            ))
            .is_err()
        );
        let InteractionSubject::Approval { call_id, .. } = approval("r") else {
            unreachable!()
        };
        assert!(
            validate_interaction_subject(&InteractionSubject::Approval {
                call_id,
                tool_id: ToolId::new("tool.read"),
                tool_name: "read".to_owned(),
                arguments_digest: "not a digest".to_owned(),
                reason: "r".to_owned(),
            })
            .is_err()
        );
    }

    #[test]
    fn a_question_settlement_must_satisfy_the_exact_requested_question() {
        let subject = InteractionSubject::Question {
            prompt: "Which target?".to_owned(),
            choices: Some(vec!["a".to_owned(), "b".to_owned()]),
            allow_free_text: false,
        };
        assert!(
            validate_interaction_settlement(
                &subject,
                &InteractionSettlement::Answered {
                    answer: QuestionAnswer::Choice {
                        value: "a".to_owned()
                    },
                },
            )
            .is_ok()
        );
        assert!(
            validate_interaction_settlement(
                &subject,
                &InteractionSettlement::Answered {
                    answer: QuestionAnswer::Choice {
                        value: "c".to_owned()
                    },
                },
            )
            .is_err(),
            "a choice the Question never offered is a false audit record"
        );
        assert!(
            validate_interaction_settlement(
                &subject,
                &InteractionSettlement::Answered {
                    answer: QuestionAnswer::FreeText {
                        value: "typed".to_owned()
                    },
                },
            )
            .is_err(),
            "free text is impossible when the Question refused it"
        );
        assert!(
            validate_interaction_settlement(&subject, &InteractionSettlement::Approved).is_err()
        );
        assert!(
            validate_interaction_settlement(
                &subject,
                &InteractionSettlement::Cancelled {
                    reason: CancellationReason::RuntimeShutdown,
                },
            )
            .is_ok()
        );
    }

    #[test]
    fn an_approval_settlement_is_bounded_and_typed() {
        let subject = approval("policy asked");
        assert!(
            validate_interaction_settlement(&subject, &InteractionSettlement::Approved).is_ok()
        );
        assert!(
            validate_interaction_settlement(
                &subject,
                &InteractionSettlement::Denied {
                    reason: "x".repeat(MAX_APPROVAL_DENIAL_REASON_CHARS + 1),
                },
            )
            .is_err()
        );
        assert!(
            validate_interaction_settlement(
                &subject,
                &InteractionSettlement::Answered {
                    answer: QuestionAnswer::FreeText {
                        value: "not a decision".to_owned()
                    },
                },
            )
            .is_err()
        );
    }
}
