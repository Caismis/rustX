//! Fixed Review node: local values and progression belong to Workflow;
//! identity, publication and settlement belong to the invoking coordinator.
use super::{
    Deserialize, Serialize, Value, WorkflowNodeInstance, WorkflowRun, WorkflowRunError,
    WorkflowRuntime, WorkflowValue, expressions,
};
use crate::events::review::{ReviewDecision, ReviewSpecification, ReviewSubject};
use crate::runtime::interaction::{InteractionOutcome, InteractionResponse};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowReviewSubject {
    Plan { value: WorkflowValue },
    Candidate { value: WorkflowValue },
}
impl WorkflowReviewSubject {
    pub(super) fn value(&self) -> &WorkflowValue {
        match self {
            Self::Plan { value } | Self::Candidate { value } => value,
        }
    }
}
pub(super) fn result_schema() -> Value {
    serde_json::json!({"type":"object","properties":{"accepted":{"type":"boolean"},"feedback":{"type":"string"}},"required":["accepted","feedback"],"additionalProperties":false})
}
impl WorkflowRuntime {
    #[allow(clippy::too_many_arguments)]
    pub(super) async fn review(
        &self,
        run: &WorkflowRun,
        context: &crate::runtime::subagent::AttemptSubagentContext,
        node: &WorkflowNodeInstance,
        subject: &WorkflowReviewSubject,
        bound: expressions::CommittedValue,
        checks: Vec<Value>,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<expressions::CommittedValue, WorkflowRunError> {
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
        let (subject, freeze) = match subject {
            WorkflowReviewSubject::Plan { .. } => (
                ReviewSubject::Plan {
                    content: bound.value,
                },
                None,
            ),
            WorkflowReviewSubject::Candidate { .. } => {
                let reference = bound.candidate.as_ref().ok_or_else(|| {
                    WorkflowRunError::InvalidValue(
                        "Review needs a runtime-owned candidate value".into(),
                    )
                })?;
                let scope = run.candidate.as_ref().ok_or_else(|| {
                    WorkflowRunError::InvalidValue("missing candidate scope".into())
                })?;
                let access = scope
                    .borrow(node.clone(), Some(reference), &cancellation.child_signal())
                    .await
                    .map_err(WorkflowRunError::InvocationAuthority)?;
                let subject = ReviewSubject::Candidate {
                    reference: access.input().clone(),
                    inspection_path: access
                        .snapshot()
                        .logical_workspace
                        .to_string_lossy()
                        .into_owned(),
                };
                (
                    subject,
                    Some(crate::runtime::workspace::CandidateFreeze::new(access)),
                )
            }
        };
        let specification = ReviewSpecification {
            instance: Box::new(node.clone()),
            subject,
            context: checks,
        };
        let outcome = coordinator
            .request_review(
                specification,
                services.turn,
                cancellation.clone(),
                freeze.clone(),
            )
            .await;
        let valid = match freeze {
            Some(freeze) => freeze.finish().await.map(|_| ()),
            None => Ok(()),
        };
        if cancellation.is_cancelled() {
            return Err(WorkflowRunError::from_cancellation(cancellation));
        }
        let outcome = outcome.map_err(|failure| {
            if !failure.is_unavailable() {
                cancellation.mark_interaction_failure();
            }
            WorkflowRunError::InvocationAuthority(failure.to_string())
        })?;
        valid.map_err(WorkflowRunError::InvalidValue)?;
        match outcome {
            InteractionOutcome::Responded {
                response: InteractionResponse::Review { response },
            } => {
                let (accepted, feedback) = match response.decision {
                    ReviewDecision::Accepted => (true, String::new()),
                    ReviewDecision::Rejected { feedback } => (false, feedback),
                };
                Ok(expressions::CommittedValue {
                    value: serde_json::json!({"accepted":accepted,"feedback":feedback}),
                    candidate: bound.candidate,
                })
            }
            InteractionOutcome::Cancelled { .. } | InteractionOutcome::DeadlineExpired { .. } => {
                Err(WorkflowRunError::from_cancellation(cancellation))
            }
            _ => Err(WorkflowRunError::InvalidValue(
                "Review subject invalidated or interaction kind mismatch".into(),
            )),
        }
    }
}
