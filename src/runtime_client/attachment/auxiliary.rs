//! Named native methods share the original close-fenced submission owner.
use super::{RuntimeAttachment, RuntimeClientError};
use crate::runtime::{
    auxiliary::{
        delivery::ResultCopy,
        submission::{BtwSubmission, ClassificationSubmission, CorrelationId, SubmissionPermit},
    },
    conversation_runtime::auxiliary::{
        AuxiliaryStatus, AuxiliarySubmissionLookup, AuxiliarySubmissionReceipt,
    },
    identity::AuxiliaryCallId,
};
impl RuntimeAttachment {
    /// Submit an explicit classification without waiting for inference.
    /// # Errors
    /// Rejects stale/read-only attachments, invalid inputs and exhausted admission.
    pub async fn classify(
        &self,
        submission: ClassificationSubmission,
    ) -> Result<AuxiliarySubmissionReceipt, RuntimeClientError> {
        let authority = self.access(true)?;
        submission
            .fingerprint(&authority.conversation_id)
            .map_err(invalid)?;
        let permit = SubmissionPermit::new_for(
            authority.conversation_id.clone(),
            submission.correlation.clone(),
        )
        .map_err(invalid)?;
        permit.bind_authority(self.authority.clone());
        let guard = OriginalSubmission(permit.clone());
        let admission =
            tokio::task::spawn_blocking(move || authority.classify(submission, &permit))
                .await
                .map_err(|_| invalid("auxiliary admission worker failed".into()))??;
        drop(guard);
        admission
            .confirmed()
            .await
            .map_err(|error| super::super::host::auxiliary::error(&error))
    }
    pub(crate) fn classify_admitted(
        &self,
        submission: ClassificationSubmission,
        permit: &SubmissionPermit,
    ) -> Result<
        crate::runtime::conversation_runtime::auxiliary::AuxiliaryAdmission,
        RuntimeClientError,
    > {
        let authority = self.access(true)?;
        permit.bind_authority(self.authority.clone());
        authority.classify(submission, permit)
    }
    /// Ask a temporary question about the committed conversation.
    /// # Errors
    /// Rejects stale/read-only attachments, invalid inputs and exhausted admission.
    pub async fn btw(
        &self,
        submission: BtwSubmission,
    ) -> Result<AuxiliarySubmissionReceipt, RuntimeClientError> {
        let authority = self.access(true)?;
        submission
            .fingerprint(&authority.conversation_id)
            .map_err(invalid)?;
        let permit = SubmissionPermit::new_for(
            authority.conversation_id.clone(),
            submission.correlation.clone(),
        )
        .map_err(invalid)?;
        permit.bind_authority(self.authority.clone());
        let guard = OriginalSubmission(permit.clone());
        let admission = tokio::task::spawn_blocking(move || authority.btw(submission, &permit))
            .await
            .map_err(|_| invalid("auxiliary admission worker failed".into()))??;
        drop(guard);
        admission
            .confirmed()
            .await
            .map_err(|error| super::super::host::auxiliary::error(&error))
    }
    pub(crate) fn btw_admitted(
        &self,
        submission: BtwSubmission,
        permit: &SubmissionPermit,
    ) -> Result<
        crate::runtime::conversation_runtime::auxiliary::AuxiliaryAdmission,
        RuntimeClientError,
    > {
        let authority = self.access(true)?;
        permit.bind_authority(self.authority.clone());
        authority.btw(submission, permit)
    }
    /// Read exact submission evidence without repeating execution.
    /// # Errors
    /// Rejects detached attachments and unavailable native owners.
    pub fn auxiliary_lookup(
        &self,
        correlation: &CorrelationId,
        scope: Option<&crate::model::AuxiliaryAttribution>,
    ) -> Result<AuxiliarySubmissionLookup, RuntimeClientError> {
        self.access(false)?.auxiliary_lookup(correlation, scope)
    }
    /// Read bounded durable status independently of result retention.
    /// # Errors
    /// Rejects detached attachments and unavailable evidence.
    pub fn auxiliary_status(
        &self,
        call: &AuxiliaryCallId,
    ) -> Result<AuxiliaryStatus, RuntimeClientError> {
        self.access(false)?.auxiliary_status(call)
    }
    /// Obtain a counted, repeatable result copy. Dropping it releases copy capacity.
    /// # Errors
    /// Rejects detached attachments and exhausted delivery capacity.
    pub fn auxiliary_result(
        &self,
        call: &AuxiliaryCallId,
    ) -> Result<Option<ResultCopy>, RuntimeClientError> {
        self.access(false)?.auxiliary_result(call)
    }
    /// Accept exact native cancellation without reading the observation Source.
    /// # Errors
    /// Rejects stale or read-only controllers.
    /// # Panics
    /// Panics if a prior mutation poisoned the attachment authority lock.
    pub fn auxiliary_cancel(&self, call: &AuxiliaryCallId) -> Result<bool, RuntimeClientError> {
        let authority = self.access(true)?;
        let gate = self.authority.lock().expect("attachment authority");
        if !*gate {
            return Err(RuntimeClientError::NotAttached);
        }
        authority.auxiliary_cancel(call)
    }
    /// Discard retained output without cancelling execution.
    /// # Errors
    /// Rejects stale or read-only controllers.
    /// # Panics
    /// Panics if a prior mutation poisoned the attachment authority lock.
    pub fn auxiliary_discard(&self, call: &AuxiliaryCallId) -> Result<bool, RuntimeClientError> {
        let authority = self.access(true)?;
        let gate = self.authority.lock().expect("attachment authority");
        if !*gate {
            return Err(RuntimeClientError::NotAttached);
        }
        authority.auxiliary_discard(call)
    }
}
fn invalid(message: String) -> RuntimeClientError {
    RuntimeClientError::InvalidState { message }
}

// Dropping a direct caller before native transfer closes the same permit that
// the physical blocking read retains; its late continuation cannot register.
struct OriginalSubmission(SubmissionPermit);
impl Drop for OriginalSubmission {
    fn drop(&mut self) {
        self.0.close();
    }
}
