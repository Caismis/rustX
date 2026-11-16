//! Explicit Session persistent settings reads and CAS updates.

use super::{SessionController, SessionError, SessionId, SessionPersistentState};

impl SessionController {
    /// Read explicit selections and their CAS revision without resolving them.
    /// # Errors
    /// Unknown Session identities are rejected.
    pub async fn read_settings(
        &self,
        id: &SessionId,
    ) -> Result<(u64, SessionPersistentState), SessionError> {
        let catalog = self.catalog.lock().await;
        Ok((catalog.settings_revision(id)?, catalog.lineage(id, None)?.1))
    }
    /// # Errors
    /// Concurrent settings edits fail with `StaleSettings`; visibility errors retain
    /// their pre/post commit distinction.
    pub async fn replace_settings(
        &self,
        id: &SessionId,
        expected: u64,
        settings: SessionPersistentState,
    ) -> Result<u64, SessionError> {
        self.catalog
            .lock()
            .await
            .replace_settings(id, expected, settings)
    }
}
