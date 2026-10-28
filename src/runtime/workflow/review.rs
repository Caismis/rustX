//! Fixed Review node: local values and progression belong to Workflow;
//! identity, publication and settlement belong to the invoking coordinator.
//!
//! A Review presents one bounded immutable structured subject and returns an
//! ordinary `{ accepted, feedback }` decision value. It acquires no
//! filesystem lease, certifies no workspace state and gates nothing by
//! itself: only an explicit downstream Branch on `accepted` does.
use super::{
    Deserialize, Serialize, Value, WorkflowNodeInstance, WorkflowRunError, WorkflowRuntime,
    WorkflowValue,
};
use crate::events::review::{ReviewDecision, ReviewSpecification, ReviewSubject};
use crate::runtime::interaction::{InteractionOutcome, InteractionResponse};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub enum WorkflowReviewSubject {
    Plan { value: WorkflowValue },
}
impl WorkflowReviewSubject {
    pub(super) fn value(&self) -> &WorkflowValue {
        match self {
            Self::Plan { value } => value,
        }
    }
}
pub(super) fn result_schema() -> Value {
    serde_json::json!({"type":"object","properties":{"accepted":{"type":"boolean"},"feedback":{"type":"string"}},"required":["accepted","feedback"],"additionalProperties":false})
}
impl WorkflowRuntime {
    pub(super) async fn review(
        &self,
        context: &crate::runtime::subagent::NativeExecutionContext,
        node: &WorkflowNodeInstance,
        subject: Value,
        checks: Vec<Value>,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<Value, WorkflowRunError> {
        let services = context.native.as_ref().ok_or_else(|| {
            WorkflowRunError::InvocationAuthority("missing native interaction services".into())
        })?;
        let coordinator = services
            .lifecycle
            .native_interaction_coordinator()
            .ok_or_else(|| {
                WorkflowRunError::InvocationAuthority(
                    "human interaction provider unavailable".into(),
                )
            })?;
        let specification = ReviewSpecification {
            instance: Box::new(node.clone()),
            subject: ReviewSubject::Plan { content: subject },
            context: checks,
        };
        self.read_model.node(node, |view| {
            view.state = super::read_model::WorkflowState::Waiting {
                reason: super::read_model::WorkflowWait::Review,
            };
        });
        let outcome = coordinator
            .request_review(
                specification,
                services.execution.clone(),
                cancellation.clone(),
            )
            .await;
        if cancellation.is_cancelled() {
            return Err(WorkflowRunError::from_cancellation(cancellation));
        }
        let outcome = outcome.map_err(|failure| {
            if !failure.is_unavailable() {
                cancellation.mark_interaction_failure();
            }
            WorkflowRunError::InvocationAuthority(failure.to_string())
        })?;
        match outcome {
            InteractionOutcome::Responded {
                response: InteractionResponse::Review { response },
            } => {
                let (accepted, feedback) = match response.decision {
                    ReviewDecision::Accepted => (true, String::new()),
                    ReviewDecision::Rejected { feedback } => (false, feedback),
                };
                self.read_model
                    .node(node, |view| view.review_accepted = Some(accepted));
                // Rejection is a normally produced decision, never a
                // Workflow failure, cancellation or implicit Return.
                Ok(serde_json::json!({"accepted": accepted, "feedback": feedback}))
            }
            InteractionOutcome::Cancelled { .. } | InteractionOutcome::DeadlineExpired { .. } => {
                Err(WorkflowRunError::from_cancellation(cancellation))
            }
            InteractionOutcome::Responded { .. } => Err(WorkflowRunError::InvalidValue(
                "Review interaction kind mismatch".into(),
            )),
        }
    }
}
