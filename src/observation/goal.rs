//! Finite Goal display metadata. Native Goal lifecycle and state stay in `goal`.
use crate::goal::{GoalOrigin, GoalPhase, GoalRef, GoalSnapshot};
use crate::runtime::identity::{AttemptId, MessageId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RuntimeClientGoalFieldRef {
    pub reference: GoalRef,
    /// Path in the existing indexed native `GoalView` value, not a credential.
    pub path: Vec<String>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum RuntimeClientGoalOrigin {
    HumanAttempt {
        message_id: Option<MessageId>,
        attempt_id: Option<AttemptId>,
    },
    RuntimeControl,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RuntimeClientGoalSnapshot {
    pub reference: GoalRef,
    pub objective: String,
    pub objective_deferred: bool,
    pub phase: GoalPhase,
    pub blocked_reason: Option<String>,
    pub blocked_reason_deferred: bool,
    pub autonomous_round_budget: u32,
    pub autonomous_rounds_consumed: u32,
    pub origin: RuntimeClientGoalOrigin,
    pub last_round_message_id: Option<MessageId>,
    pub last_round_message_id_deferred: bool,
    pub fields: Vec<RuntimeClientGoalFieldRef>,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RuntimeClientGoalView {
    pub current: Option<RuntimeClientGoalSnapshot>,
}
fn preview(text: &str) -> (String, bool) {
    let mut end = text.len().min(512);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    (text[..end].to_owned(), end < text.len())
}
impl RuntimeClientGoalView {
    #[must_use]
    pub fn from_native(current: Option<&GoalSnapshot>) -> Self {
        Self {
            current: current.map(RuntimeClientGoalSnapshot::from_native),
        }
    }
}
impl RuntimeClientGoalSnapshot {
    fn from_native(goal: &GoalSnapshot) -> Self {
        let (objective, objective_deferred) = preview(&goal.objective);
        let blocked = goal.blocked_reason.as_deref().map(preview);
        let blocked_reason_deferred = blocked.as_ref().is_some_and(|(_, deferred)| *deferred);
        let blocked_reason = blocked.map(|(value, _)| value);
        let mut fields = Vec::with_capacity(5);
        let mut field = |name: &str| {
            fields.push(RuntimeClientGoalFieldRef {
                reference: goal.reference.clone(),
                path: std::iter::once("current".to_owned())
                    .chain(name.split('.').map(str::to_owned))
                    .collect(),
            });
        };
        if objective_deferred {
            field("objective");
        }
        if blocked_reason_deferred {
            field("blocked_reason");
        }
        let origin = match &goal.origin {
            GoalOrigin::RuntimeControl => RuntimeClientGoalOrigin::RuntimeControl,
            GoalOrigin::HumanAttempt {
                message_id,
                attempt_id,
            } => {
                let message_fits = message_id.as_str().len() <= 256;
                let attempt_fits = attempt_id.as_str().len() <= 256;
                if !message_fits {
                    field("origin.message_id");
                }
                if !attempt_fits {
                    field("origin.attempt_id");
                }
                RuntimeClientGoalOrigin::HumanAttempt {
                    message_id: message_fits.then(|| message_id.clone()),
                    attempt_id: attempt_fits.then(|| attempt_id.clone()),
                }
            }
        };
        let last_round_message_id_deferred = goal
            .last_round_message_id
            .as_ref()
            .is_some_and(|id| id.as_str().len() > 256);
        if last_round_message_id_deferred {
            field("last_round_message_id");
        }
        Self {
            reference: goal.reference.clone(),
            objective,
            objective_deferred,
            phase: goal.phase,
            blocked_reason,
            blocked_reason_deferred,
            autonomous_round_budget: goal.autonomous_round_budget,
            autonomous_rounds_consumed: goal.autonomous_rounds_consumed,
            origin,
            last_round_message_id: goal
                .last_round_message_id
                .as_ref()
                .filter(|id| id.as_str().len() <= 256)
                .cloned(),
            last_round_message_id_deferred,
            fields,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn t15_t20_goal_display_preserves_exact_authority_without_growing_origin_identity() {
        let original = GoalSnapshot {
            reference: GoalRef {
                id: "goal-17".into(),
                revision: 9,
            },
            objective: "雪".repeat(2700),
            phase: GoalPhase::Blocked,
            blocked_reason: Some("😀".repeat(2000)),
            autonomous_round_budget: 73,
            autonomous_rounds_consumed: 19,
            origin: GoalOrigin::HumanAttempt {
                message_id: MessageId::new("m".repeat(2_000_000)),
                attempt_id: AttemptId::new("a".repeat(2_000_000)),
            },
            last_round_message_id: Some(MessageId::new("last".repeat(500_000))),
        };
        let view = RuntimeClientGoalView::from_native(Some(&original));
        let current = view.current.as_ref().unwrap();
        assert_eq!(current.reference, original.reference);
        assert_eq!(current.phase, GoalPhase::Blocked);
        assert_eq!(
            (
                current.autonomous_round_budget,
                current.autonomous_rounds_consumed
            ),
            (73, 19)
        );
        assert!(
            current.objective_deferred
                && current.blocked_reason_deferred
                && current.last_round_message_id_deferred
        );
        assert!(
            current.objective.len() <= 512 && current.blocked_reason.as_ref().unwrap().len() <= 512
        );
        assert_eq!(current.fields.len(), 5);
        assert!(matches!(
            current.origin,
            RuntimeClientGoalOrigin::HumanAttempt {
                message_id: None,
                attempt_id: None
            }
        ));
        assert!(serde_json::to_vec(&view).unwrap().len() < 4096);
        assert_eq!(original.objective.len(), 8100);
        assert_eq!(
            original
                .last_round_message_id
                .as_ref()
                .unwrap()
                .as_str()
                .len(),
            2_000_000
        );
    }
}
