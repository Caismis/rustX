//! A captured Attempt is borrowed until a public snapshot actually needs ownership.
use super::super::{
    settings::AdmittedSettings,
    snapshot::{
        InFlightAssistantMessage, InFlightBlock, RuntimeClientAttempt, RuntimeClientAttemptPhase,
    },
};
use crate::observation::tool::ForegroundToolExecution;
use crate::observation::turn::AttemptIdentitySource;
use crate::{
    model::{ModelUsage, session::AttemptModelView},
    runtime::identity::{AttemptId, MessageId},
};
use serde::Serialize;
use std::{borrow::Cow, sync::Arc};

#[derive(Serialize)]
pub(super) struct AttemptView<'a> {
    #[serde(skip)]
    pub fit: Option<usize>,
    pub attempt_id: &'a Option<AttemptId>,
    pub attempt_id_source: &'a Option<AttemptIdentitySource>,
    pub phase: RuntimeClientAttemptPhase,
    pub turn: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_usage: &'a Option<ModelUsage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_flight: Option<InFlightView<'a>>,
    pub foreground: Vec<Cow<'a, ForegroundToolExecution>>,
    pub model: Arc<Option<Box<AttemptModelView>>>,
    pub execution_settings: Arc<Option<AdmittedSettings>>,
}
#[derive(Serialize)]
pub(super) struct InFlightView<'a> {
    #[serde(skip)]
    pub fit: Option<usize>,
    pub message_id: &'a MessageId,
    pub blocks: Vec<Cow<'a, InFlightBlock>>,
}
fn own<T: Clone>(value: Arc<T>) -> T {
    Arc::try_unwrap(value).unwrap_or_else(|value| value.as_ref().clone())
}
impl AttemptView<'_> {
    pub(super) fn into_owned(self) -> RuntimeClientAttempt {
        RuntimeClientAttempt {
            attempt_id: self.attempt_id.clone(),
            attempt_id_source: self.attempt_id_source.clone(),
            phase: self.phase,
            turn: self.turn,
            last_usage: self.last_usage.clone(),
            in_flight: self.in_flight.map(|flight| InFlightAssistantMessage {
                message_id: flight.message_id.clone(),
                blocks: flight.blocks.into_iter().map(Cow::into_owned).collect(),
            }),
            foreground: self.foreground.into_iter().map(Cow::into_owned).collect(),
            model: own(self.model),
            execution_settings: own(self.execution_settings),
        }
    }
}

impl AttemptView<'_> {
    pub(super) fn measure(
        &self,
        items: &super::prepared_attempt::Items,
        foreground: Option<usize>,
    ) -> Option<usize> {
        use super::{prepared_attempt, prepared_capture::FitBudget};
        let mut budget = FitBudget::new();
        budget.value("attempt_id", &self.attempt_id);
        budget.value("attempt_id_source", &self.attempt_id_source);
        budget.value("phase", &self.phase);
        budget.value("turn", &self.turn);
        if self.last_usage.is_some() {
            budget.value("last_usage", &self.last_usage);
        }
        if let Some(flight) = &self.in_flight {
            let mut nested = FitBudget::new();
            nested.value("message_id", &flight.message_id);
            nested.field("blocks", flight.fit);
            budget.field(
                "in_flight",
                nested.finish().and_then(|value| value.checked_add(1)),
            );
        }
        budget.field("foreground", foreground);
        budget.field(
            "model",
            prepared_attempt::shared_budget(&items.model, &self.model),
        );
        budget.field(
            "execution_settings",
            prepared_attempt::shared_budget(&items.settings, &self.execution_settings),
        );
        budget.finish()
    }
}
