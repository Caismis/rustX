//! Explicit phase-only profiling, absent from ordinary builds and the protocol.
use super::ResidentRuntime;
use crate::runtime::identity::ConversationId;
use std::path::PathBuf;
use std::sync::Arc;

pub(super) struct Sample {
    path: Option<PathBuf>,
    rows: Vec<serde_json::Value>,
}
impl Sample {
    pub(super) fn requested() -> Self {
        let path = std::env::var_os("RUSTX_RETENTION_PROFILE").and_then(|path| {
            let path = PathBuf::from(path);
            std::fs::remove_file(path.with_extension("request"))
                .is_ok()
                .then_some(path)
        });
        Self {
            path,
            rows: Vec::new(),
        }
    }
    pub(super) fn capture(
        &mut self,
        conversation: &ConversationId,
        resident: Option<&Arc<ResidentRuntime>>,
    ) {
        if self.path.is_none() {
            return;
        }
        let profile = resident.and_then(|resident| {
            let composition = resident.composition.try_lock().ok()?;
            Some(composition.as_ref()?.host().retention_profile())
        });
        self.rows
            .push(serde_json::json!({"conversation": conversation, "profile": profile}));
    }
    pub(super) fn finish(self) {
        if let Some(path) = self.path {
            let temporary = path.with_extension("tmp");
            std::fs::write(
                &temporary,
                serde_json::to_vec(&self.rows).expect("finite retention counters"),
            )
            .expect("write requested diagnostic retention profile");
            std::fs::rename(temporary, path).expect("complete diagnostic retention profile");
        }
    }
}
