//! Internal attempt domains detach independently from issued snapshot cuts.
use super::shared_snapshot::CowDomain;
use crate::model::ModelUsage;
use crate::model::session::AttemptModelView;
use crate::observation::tool::ForegroundToolExecution;
use crate::runtime::identity::AttemptId;
use crate::runtime_client::settings::AdmittedSettings;
use crate::runtime_client::snapshot::{
    InFlightAssistantMessage, RuntimeClientAttempt, RuntimeClientAttemptPhase,
};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SharedAttempt {
    pub attempt_id: Option<AttemptId>,
    pub attempt_id_source: Option<crate::observation::turn::AttemptIdentitySource>,
    pub phase: RuntimeClientAttemptPhase,
    pub turn: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_usage: Option<ModelUsage>,
    #[serde(skip_serializing_if = "CowDomain::is_none")]
    pub in_flight: CowDomain<Option<SharedInFlight>>,
    pub foreground: CowDomain<Vec<ForegroundToolExecution>>,
    pub model: CowDomain<Option<Box<AttemptModelView>>>,
    pub execution_settings: CowDomain<Option<AdmittedSettings>>,
}
impl From<RuntimeClientAttempt> for SharedAttempt {
    fn from(value: RuntimeClientAttempt) -> Self {
        let RuntimeClientAttempt {
            attempt_id,
            attempt_id_source,
            phase,
            turn,
            last_usage,
            in_flight,
            foreground,
            model,
            execution_settings,
        } = value;
        Self {
            attempt_id,
            attempt_id_source,
            phase,
            turn,
            last_usage,
            in_flight: in_flight.into(),
            foreground: foreground.into(),
            model: model.into(),
            execution_settings: execution_settings.into(),
        }
    }
}
impl SharedAttempt {
    pub(crate) fn owned(&self) -> RuntimeClientAttempt {
        RuntimeClientAttempt {
            attempt_id: self.attempt_id.clone(),
            attempt_id_source: self.attempt_id_source.clone(),
            phase: self.phase.clone(),
            turn: self.turn,
            last_usage: self.last_usage.clone(),
            in_flight: self.in_flight.as_ref().map(SharedInFlight::owned),
            foreground: self.foreground.owned(),
            model: self.model.owned(),
            execution_settings: self.execution_settings.owned(),
        }
    }
}
impl From<Option<RuntimeClientAttempt>> for CowDomain<Option<SharedAttempt>> {
    fn from(value: Option<RuntimeClientAttempt>) -> Self {
        value.map(SharedAttempt::from).into()
    }
}
impl CowDomain<Option<SharedAttempt>> {
    pub(crate) fn public(&self) -> Option<RuntimeClientAttempt> {
        self.as_ref().map(SharedAttempt::owned)
    }
}
impl<T> CowDomain<Option<T>> {
    pub(super) fn is_none(&self) -> bool {
        self.value().is_none()
    }
}

#[cfg(test)]
impl PartialEq<Option<RuntimeClientAttempt>> for CowDomain<Option<SharedAttempt>> {
    fn eq(&self, other: &Option<RuntimeClientAttempt>) -> bool {
        &self.public() == other
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SharedInFlight {
    pub message_id: crate::runtime::identity::MessageId,
    pub blocks: SharedBlocks,
}
impl From<InFlightAssistantMessage> for SharedInFlight {
    fn from(value: InFlightAssistantMessage) -> Self {
        Self {
            message_id: value.message_id,
            blocks: SharedBlocks(value.blocks.into_iter().map(Into::into).collect()),
        }
    }
}
impl SharedInFlight {
    pub(super) fn owned(&self) -> InFlightAssistantMessage {
        InFlightAssistantMessage {
            message_id: self.message_id.clone(),
            blocks: self.blocks.iter().cloned().collect(),
        }
    }
}
impl From<Option<InFlightAssistantMessage>> for CowDomain<Option<SharedInFlight>> {
    fn from(value: Option<InFlightAssistantMessage>) -> Self {
        value.map(SharedInFlight::from).into()
    }
}
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(transparent)]
pub(crate) struct SharedBlocks(Vec<CowDomain<crate::runtime_client::snapshot::InFlightBlock>>);
impl SharedBlocks {
    pub(super) fn iter(
        &self,
    ) -> impl ExactSizeIterator<Item = &crate::runtime_client::snapshot::InFlightBlock> + Clone
    {
        self.0.iter().map(CowDomain::value)
    }
    pub(super) fn len(&self) -> usize {
        self.0.len()
    }
    pub(super) fn push(&mut self, block: crate::runtime_client::snapshot::InFlightBlock) {
        self.0.push(block.into());
    }
    pub(super) fn get(
        &self,
        index: usize,
    ) -> Option<&crate::runtime_client::snapshot::InFlightBlock> {
        self.0.get(index).map(CowDomain::value)
    }
    pub(super) fn find_mut(
        &mut self,
        predicate: impl Fn(&crate::runtime_client::snapshot::InFlightBlock) -> bool,
    ) -> Option<&mut crate::runtime_client::snapshot::InFlightBlock> {
        let i = self.iter().position(predicate)?;
        Some(&mut self[i])
    }
    #[cfg(test)]
    pub(super) fn truncate(&mut self, length: usize) {
        self.0.truncate(length);
    }
    #[cfg(test)]
    pub(super) fn swap(&mut self, a: usize, b: usize) {
        self.0.swap(a, b);
    }
    pub(super) fn same_block(&self, other: &Self, index: usize) -> bool {
        self.0
            .get(index)
            .zip(other.0.get(index))
            .is_some_and(|(left, right)| left.same_owner(right))
    }
}
impl std::ops::Index<usize> for SharedBlocks {
    type Output = crate::runtime_client::snapshot::InFlightBlock;
    fn index(&self, index: usize) -> &Self::Output {
        &self.0[index]
    }
}
impl std::ops::IndexMut<usize> for SharedBlocks {
    fn index_mut(&mut self, index: usize) -> &mut Self::Output {
        &mut self.0[index]
    }
}
