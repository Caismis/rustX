//! Read and control authority are selected by the attachment before entry.
use super::{ClientInner, RuntimeClientError};
use crate::runtime::{
    auxiliary::{
        delivery::ResultCopy,
        submission::{BtwSubmission, ClassificationSubmission, CorrelationId, SubmissionPermit},
    },
    conversation_runtime::auxiliary::{
        AuxiliaryAdmission, AuxiliaryStatus, AuxiliarySubmissionLookup,
    },
    identity::AuxiliaryCallId,
};
pub(crate) fn error(error: &crate::context::ContextError) -> RuntimeClientError {
    use crate::{context::ContextErrorKind, runtime::auxiliary::submission::AuxiliaryFailure};
    RuntimeClientError::Auxiliary {
        reason: match error.kind {
            ContextErrorKind::AuxiliaryBusy | ContextErrorKind::AuxiliaryLifecycleBusy => {
                AuxiliaryFailure::Busy
            }
            ContextErrorKind::CannotFit => AuxiliaryFailure::InputTooLarge,
            ContextErrorKind::AuxiliaryConflict => AuxiliaryFailure::Conflict,
            ContextErrorKind::AuxiliaryScopeClosed => AuxiliaryFailure::ScopeClosed,
            _ => AuxiliaryFailure::Unavailable,
        },
    }
}
impl ClientInner {
    fn auxiliary_runtime(
        &self,
    ) -> Result<&crate::runtime::conversation_runtime::ConversationRuntime, RuntimeClientError>
    {
        self.runtime.as_ref().ok_or(RuntimeClientError::Auxiliary {
            reason: crate::runtime::auxiliary::submission::AuxiliaryFailure::Unavailable,
        })
    }
    pub(crate) fn classify(
        &self,
        submission: ClassificationSubmission,
        permit: &SubmissionPermit,
    ) -> Result<AuxiliaryAdmission, RuntimeClientError> {
        self.auxiliary_runtime()?
            .classify(submission, permit)
            .map_err(|cause| error(&cause))
    }
    pub(crate) fn btw(
        &self,
        submission: BtwSubmission,
        permit: &SubmissionPermit,
    ) -> Result<AuxiliaryAdmission, RuntimeClientError> {
        self.auxiliary_runtime()?
            .btw(submission, permit)
            .map_err(|cause| error(&cause))
    }
    pub(crate) fn auxiliary_lookup(
        &self,
        correlation: &CorrelationId,
        scope: Option<&crate::model::AuxiliaryAttribution>,
    ) -> Result<AuxiliarySubmissionLookup, RuntimeClientError> {
        Ok(self
            .auxiliary_runtime()?
            .auxiliary_lookup(correlation, scope))
    }
    pub(crate) fn auxiliary_status(
        &self,
        call: &AuxiliaryCallId,
    ) -> Result<AuxiliaryStatus, RuntimeClientError> {
        self.auxiliary_runtime()?
            .auxiliary_status(call)
            .map_err(|cause| error(&cause))
    }
    pub(crate) fn auxiliary_result(
        &self,
        call: &AuxiliaryCallId,
    ) -> Result<Option<ResultCopy>, RuntimeClientError> {
        self.auxiliary_runtime()?
            .auxiliary_result(call)
            .map_err(|cause| error(&cause))
    }
    pub(crate) fn auxiliary_cancel(
        &self,
        call: &AuxiliaryCallId,
    ) -> Result<bool, RuntimeClientError> {
        Ok(self.auxiliary_runtime()?.auxiliary_cancel(call))
    }
    pub(crate) fn auxiliary_discard(
        &self,
        call: &AuxiliaryCallId,
    ) -> Result<bool, RuntimeClientError> {
        Ok(self.auxiliary_runtime()?.auxiliary_discard(call))
    }
}
