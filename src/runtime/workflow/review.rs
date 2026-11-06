//! Human Input node, as Dify's: local values and progression belong to
//! Workflow; identity, publication and settlement belong to the invoking
//! coordinator.
//!
//! The node asks the person one questionnaire question, its rendered
//! content, whose options are its actions. The chosen action is the exit the
//! run takes; `timeout` is taken when nobody answers in time. It acquires no
//! filesystem lease and certifies no workspace state.
use super::{
    Deserialize, Serialize, Value, WorkflowNodeInstance, WorkflowPort, WorkflowRunError,
    WorkflowRuntime,
};
use crate::events::review::ReviewSpecification;
use crate::runtime::interaction::{InteractionOutcome, InteractionResponse};

/// The exit taken when nobody answers within the node's timeout.
pub const REVIEW_TIMEOUT_PORT: &str = "timeout";
pub const MAX_REVIEW_ACTIONS: usize = crate::events::interaction::MAX_CHOICE_OPTIONS;

/// One button the person may press, and the exit it takes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct WorkflowReviewAction {
    pub id: String,
    pub title: String,
}

/// `action` is the chosen action's ID, or `timeout`; `content` is what the
/// person was shown.
pub(super) fn result_schema(actions: &[WorkflowReviewAction]) -> Value {
    let ids = actions
        .iter()
        .map(|action| action.id.as_str())
        .chain([REVIEW_TIMEOUT_PORT])
        .collect::<Vec<_>>();
    serde_json::json!({"type":"object","properties":{"action":{"type":"string","enum":ids},"content":{"type":"string"}},"required":["action","content"],"additionalProperties":false})
}

/// The rendering frontier uses the same character contract as the published DTO.
pub(super) fn render_content(
    parts: &[super::WorkflowValue],
    mut resolve: impl FnMut(&super::WorkflowValue) -> Result<Value, WorkflowRunError>,
) -> Result<String, WorkflowRunError> {
    let limit = crate::events::interaction::MAX_QUESTION_TEXT_CHARS;
    let mut remaining = limit;
    super::model::render_parts(parts, |part| {
        let text = super::model::rendered_value(resolve(part)?);
        remaining = remaining.checked_sub(text.chars().count()).ok_or_else(|| {
            WorkflowRunError::InvalidValue(format!(
                "Human Input content exceeds {limit} Unicode characters"
            ))
        })?;
        Ok(Value::String(text))
    })
}

impl WorkflowRuntime {
    pub(super) async fn review(
        &self,
        context: &crate::runtime::subagent::NativeExecutionContext,
        node: &WorkflowNodeInstance,
        shown: String,
        actions: &[WorkflowReviewAction],
        timeout_ms: u64,
        cancellation: &crate::runtime::cancellation::ExecutionCancellation,
    ) -> Result<(Value, WorkflowPort), WorkflowRunError> {
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
        let specification = ReviewSpecification::new(
            node.clone(),
            shown.clone(),
            actions.iter().map(|action| action.title.as_str()),
        );
        specification.validate().map_err(|error| {
            WorkflowRunError::InvalidValue(format!("Human Input {:?}: {error}", node.node))
        })?;
        self.read_model.node(node, |view| {
            view.state = super::read_model::WorkflowState::Waiting {
                reason: super::read_model::WorkflowWait::Review,
            };
        });
        // The node's own deadline settles the question as expired; an
        // ancestor's cancellation still wins over it.
        let (deadline, waiting) = cancellation.child_execution();
        let request =
            coordinator.request_review(specification.clone(), services.execution.clone(), waiting);
        tokio::pin!(request);
        let outcome = tokio::select! {
            outcome = &mut request => outcome,
            () = tokio::time::sleep(std::time::Duration::from_millis(timeout_ms)) => {
                deadline.cancel(crate::tools::deadline::ToolCancellationCause::Deadline(
                    crate::tools::deadline::ToolDeadlineKind::Hard,
                ));
                request.await
            }
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
        let action = match outcome {
            InteractionOutcome::Responded {
                response: InteractionResponse::Review { response },
            } => {
                let index = specification
                    .validate_response(&response)
                    .map_err(WorkflowRunError::InvalidValue)?;
                actions[index].id.as_str()
            }
            InteractionOutcome::DeadlineExpired { .. } => REVIEW_TIMEOUT_PORT,
            InteractionOutcome::Cancelled { .. } => {
                return Err(WorkflowRunError::from_cancellation(cancellation));
            }
            InteractionOutcome::Responded { .. } => {
                return Err(WorkflowRunError::InvalidValue(
                    "Review interaction kind mismatch".into(),
                ));
            }
        };
        Ok((
            serde_json::json!({"action": action, "content": shown}),
            WorkflowPort::case(action),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn human_rendering_stops_before_building_an_oversized_question() {
        let parts = vec![
            super::super::WorkflowValue::Reference {
                path: vec!["args".into(), "report".into()]
            };
            64
        ];
        let mut resolved = 0;
        let result = render_content(&parts, |_| {
            resolved += 1;
            Ok(Value::String("x".repeat(1000)))
        });
        assert!(result.unwrap_err().to_string().contains("4096"));
        assert_eq!(resolved, 5);
    }
}
