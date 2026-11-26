//! Bounded Human Input facts: one Workflow node visit asks a questionnaire.
//!
//! The digest binds the node visit and the complete presented questionnaire,
//! so an answer can only settle the exact question the person saw.
use super::interaction::{
    AnswerSpecification, OptionAnswer, OptionSpecification, QuestionSpecification,
    QuestionnaireAnswer, QuestionnaireAnswerEntry, QuestionnaireSpecification,
    QuestionnaireSubmission, SingleChoiceSpecification, normalize_questionnaire_submission,
    validate_questionnaire,
};
use crate::runtime::workflow::WorkflowNodeInstance;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct ReviewSpecification {
    pub instance: Box<WorkflowNodeInstance>,
    /// The node's content as the question and its actions as the options.
    pub questionnaire: QuestionnaireSpecification,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[derive(schemars::JsonSchema)]
pub struct ReviewResponse {
    pub instance: Box<WorkflowNodeInstance>,
    pub subject_digest: String,
    /// Every question answered; the action is an option of the last one.
    pub submission: QuestionnaireSubmission,
}

impl ReviewSpecification {
    /// One question, the node's rendered content, whose options are its
    /// action titles in order. The node ID labels the question.
    #[must_use]
    pub fn new<'a>(
        instance: WorkflowNodeInstance,
        content: String,
        actions: impl IntoIterator<Item = &'a str>,
    ) -> Self {
        let header = instance.node.chars().take(16).collect();
        Self {
            instance: Box::new(instance),
            questionnaire: QuestionnaireSpecification {
                questions: vec![QuestionSpecification {
                    question: content,
                    header,
                    answer: AnswerSpecification::SingleChoice(SingleChoiceSpecification {
                        options: actions
                            .into_iter()
                            .map(|title| OptionSpecification {
                                label: title.to_owned(),
                                description: String::new(),
                                preview: None,
                            })
                            .collect(),
                        allow_custom: false,
                    }),
                }],
            },
        }
    }

    /// The response that presses the action at `index`.
    #[must_use]
    pub fn choose(&self, index: usize) -> ReviewResponse {
        ReviewResponse {
            instance: self.instance.clone(),
            subject_digest: self.digest(),
            submission: QuestionnaireSubmission {
                answers: vec![QuestionnaireAnswerEntry {
                    question_index: self.questionnaire.questions.len().saturating_sub(1),
                    answer: QuestionnaireAnswer::Option(OptionAnswer {
                        option_index: index,
                    }),
                }],
            },
        }
    }

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
    /// Refuses an oversized instance or an invalid questionnaire.
    pub fn validate(&self) -> Result<(), String> {
        if serde_json::to_vec(&self.instance)
            .map_err(|e| e.to_string())?
            .len()
            > 8192
        {
            return Err("Review instance exceeds 8192 bytes".into());
        }
        validate_questionnaire(&self.questionnaire)
    }

    /// The chosen action: the option picked in the last question.
    /// # Errors
    /// Refuses a different instance or digest, or an incomplete submission.
    pub fn validate_response(&self, response: &ReviewResponse) -> Result<usize, String> {
        if response.instance != self.instance || response.subject_digest != self.digest() {
            return Err("Review response names a different instance or subject".into());
        }
        let submission =
            normalize_questionnaire_submission(&self.questionnaire, &response.submission)?;
        if submission.answers.len() != self.questionnaire.questions.len() {
            return Err("Review response must answer every question".into());
        }
        match submission.answers.last().map(|entry| &entry.answer) {
            Some(QuestionnaireAnswer::Option(option)) => Ok(option.option_index),
            _ => Err("Review response must choose one action".into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn specification() -> ReviewSpecification {
        ReviewSpecification::new(
            crate::runtime::workflow::test_instance("review", "human"),
            "Ship it?".into(),
            ["Approve", "Reject"],
        )
    }
    fn choose(spec: &ReviewSpecification, index: usize) -> ReviewResponse {
        spec.choose(index)
    }
    #[test]
    fn a_response_names_the_chosen_action() {
        let spec = specification();
        spec.validate().unwrap();
        assert_eq!(spec.validate_response(&choose(&spec, 1)), Ok(1));
        assert!(spec.validate_response(&choose(&spec, 2)).is_err());
        let mut unanswered = choose(&spec, 0);
        unanswered.submission.answers.clear();
        assert!(spec.validate_response(&unanswered).is_err());
    }

    /// Issue #462 C14: a wrong instance (node visit) or a stale subject digest
    /// is refused; a changed question changes the digest.
    #[test]
    fn review_response_is_bound_to_exact_visit_and_presented_subject() {
        let spec = specification();
        let mut other_visit = choose(&spec, 0);
        other_visit.instance.visit += 1;
        let mut stale = choose(&spec, 0);
        stale.subject_digest = "0".repeat(64);
        for response in [other_visit, stale] {
            assert!(spec.validate_response(&response).is_err());
        }
        let mut changed = spec.clone();
        changed.questionnaire.questions[0].question = "Ship it now?".into();
        assert_ne!(spec.digest(), changed.digest());
        assert!(changed.validate_response(&choose(&spec, 0)).is_err());
    }
}
