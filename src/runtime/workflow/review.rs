//! Fixed Review node: local values and progression belong to Workflow;
//! identity, publication and settlement belong to the invoking coordinator.
use super::{
    Deserialize, Serialize, Value, WorkflowNodeInstance, WorkflowRun, WorkflowRunError,
    WorkflowRuntime, WorkflowValue, expressions,
};
use crate::events::review::{ReviewDecision, ReviewFact, ReviewSpecification, ReviewSubject};
use crate::runtime::interaction::{InteractionOutcome, InteractionResponse};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
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
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)] // One freeze/request/settlement ownership scope.
    pub(super) async fn review(
        &self,
        run: &WorkflowRun,
        context: &crate::runtime::subagent::AttemptSubagentContext,
        node: &WorkflowNodeInstance,
        subject: &WorkflowReviewSubject,
        bound: expressions::CommittedValue,
        checks: Vec<ReviewFact>,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<
        (
            expressions::CommittedValue,
            Option<crate::runtime::workspace::CandidateReference>,
        ),
        WorkflowRunError,
    > {
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
        let subject = match subject {
            WorkflowReviewSubject::Plan { .. } => ReviewSubject::Plan {
                content: bound.value,
                candidate: bound.candidate,
            },
            WorkflowReviewSubject::Candidate { .. } => ReviewSubject::Candidate {
                reference: bound.candidate.ok_or_else(|| {
                    WorkflowRunError::InvalidValue(
                        "Review needs a runtime-owned candidate value".into(),
                    )
                })?,
                inspection_path: String::new(),
            },
        };
        let mut specification = ReviewSpecification {
            instance: Box::new(node.clone()),
            subject,
            context: checks,
        };
        let candidate = specification
            .candidate()
            .map_err(WorkflowRunError::InvalidValue)?
            .cloned();
        let freeze = if let Some(reference) = &candidate {
            let scope = run
                .candidate
                .as_ref()
                .ok_or_else(|| WorkflowRunError::InvalidValue("missing candidate scope".into()))?;
            let access = scope
                .borrow(node.clone(), Some(reference), &cancellation.child_signal())
                .await
                .map_err(WorkflowRunError::InvocationAuthority)?;
            if let ReviewSubject::Candidate {
                inspection_path, ..
            } = &mut specification.subject
            {
                *inspection_path = access
                    .snapshot()
                    .logical_workspace
                    .to_string_lossy()
                    .into_owned();
            }
            Some(crate::runtime::workspace::CandidateFreeze::new(access))
        } else {
            None
        };
        self.read_model.node(node, |view| {
            view.state = super::read_model::WorkflowState::Waiting {
                reason: super::read_model::WorkflowWait::Review,
            };
            view.candidate.clone_from(&candidate);
        });
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
                self.read_model
                    .node(node, |view| view.review_accepted = Some(accepted));
                Ok((
                    expressions::CommittedValue::from(
                        serde_json::json!({"accepted":accepted,"feedback":feedback}),
                    ),
                    if accepted { candidate } else { None },
                ))
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
