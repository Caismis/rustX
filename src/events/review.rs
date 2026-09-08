//! Bounded business review facts. Decisions cannot replace a frozen subject.
use crate::runtime::workflow::WorkflowNodeInstance;
use crate::runtime::workspace::CandidateReference;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_REVIEW_PLAN_BYTES: usize = 32 * 1024;
pub const MAX_REVIEW_CONTEXT_BYTES: usize = 8 * 1024;
pub const MAX_REVIEW_FEEDBACK_CHARS: usize = 2_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReviewSubject {
    Plan {
        content: Value,
    },
    /// The native owner keeps this checkout exclusively borrowed while pending.
    /// This is an inspection reference, not an inlined or truncated diff.
    Candidate {
        reference: CandidateReference,
        inspection_path: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewSpecification {
    pub instance: Box<WorkflowNodeInstance>,
    pub subject: ReviewSubject,
    pub context: Vec<Value>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ReviewDecision {
    Accepted,
    Rejected { feedback: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewResponse {
    pub instance: Box<WorkflowNodeInstance>,
    pub subject_digest: String,
    pub decision: ReviewDecision,
}

impl ReviewSpecification {
    /// Digest of the complete immutable presentation and concrete instance.
    /// # Panics
    /// Only if serialization of these JSON-compatible domain fields fails.
    #[must_use]
    pub fn digest(&self) -> String {
        super::interaction::interaction_arguments_digest(
            &serde_json::to_value(self).expect("review facts"),
        )
    }
    /// # Errors
    /// Refuses oversized facts and malformed candidate identity.
    pub fn validate(&self) -> Result<(), String> {
        if serde_json::to_vec(&self.instance)
            .map_err(|e| e.to_string())?
            .len()
            > 8192
        {
            return Err("Review instance exceeds 8192 bytes".into());
        }
        match &self.subject {
            ReviewSubject::Plan { content } => {
                if !content.is_object()
                    || serde_json::to_vec(content)
                        .map_err(|e| e.to_string())?
                        .len()
                        > MAX_REVIEW_PLAN_BYTES
                {
                    return Err(
                        "Review plan must be a complete structured object of at most 32768 bytes"
                            .into(),
                    );
                }
            }
            ReviewSubject::Candidate {
                reference,
                inspection_path,
            } => {
                if reference.run != self.instance.block.run
                    || reference.content.len() != 64
                    || !reference.content.bytes().all(|b| b.is_ascii_hexdigit())
                    || inspection_path.is_empty()
                    || inspection_path.len() > 4096
                {
                    return Err("invalid candidate Review reference".into());
                }
            }
        }
        if self.context.len() > 8
            || serde_json::to_vec(&self.context)
                .map_err(|e| e.to_string())?
                .len()
                > MAX_REVIEW_CONTEXT_BYTES
        {
            return Err("Review context exceeds eight entries or 8192 bytes".into());
        }
        Ok(())
    }
    /// # Errors
    /// Refuses a different instance/digest or oversized feedback.
    pub fn validate_response(&self, response: &ReviewResponse) -> Result<(), String> {
        if response.instance != self.instance || response.subject_digest != self.digest() {
            return Err("Review response names a different instance or subject".into());
        }
        if let ReviewDecision::Rejected { feedback } = &response.decision
            && feedback.chars().count() > MAX_REVIEW_FEEDBACK_CHARS
        {
            return Err("Review feedback exceeds 2000 characters".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn specification() -> ReviewSpecification {
        ReviewSpecification {
            instance: Box::new(crate::runtime::workflow::test_instance("review", "human")),
            subject: ReviewSubject::Plan {
                content: serde_json::json!({"steps":["inspect"]}),
            },
            context: vec![],
        }
    }
    #[test]
    fn review_bounds_and_subject_identity_are_exact_not_label_interpretation() {
        let spec = specification();
        spec.validate().unwrap();
        let mut response = ReviewResponse {
            instance: spec.instance.clone(),
            subject_digest: spec.digest(),
            decision: ReviewDecision::Rejected {
                feedback: "accepted approved YES".into(),
            },
        };
        spec.validate_response(&response).unwrap();
        assert!(matches!(response.decision, ReviewDecision::Rejected { .. }));
        response.decision = ReviewDecision::Rejected {
            feedback: "🙂".repeat(MAX_REVIEW_FEEDBACK_CHARS + 1),
        };
        assert!(spec.validate_response(&response).is_err());
        let mut oversized = spec.clone();
        oversized.subject = ReviewSubject::Plan {
            content: serde_json::json!({"plan":"x".repeat(MAX_REVIEW_PLAN_BYTES)}),
        };
        assert!(oversized.validate().is_err());
        oversized = spec.clone();
        oversized.context = vec![Value::Null; 9];
        assert!(oversized.validate().is_err());
        oversized.context = vec![Value::String("x".repeat(MAX_REVIEW_CONTEXT_BYTES))];
        assert!(oversized.validate().is_err());
        let mut changed = spec.clone();
        changed.subject = ReviewSubject::Plan {
            content: serde_json::json!({"steps":["replace"]}),
        };
        assert_ne!(spec.digest(), changed.digest());
    }
}
