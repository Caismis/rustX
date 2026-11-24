//! Finite display of a native occupancy reading. Original models remain source-owned.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RuntimeClientContextOccupancy {
    pub estimated: bool,
    pub input_tokens: u64,
    pub context_window_tokens: u64,
    pub model: Option<String>,
    pub model_deferred: bool,
    /// Exact selected-field domain, read with the occupancy's captured checkpoint cut.
    pub model_source: Option<String>,
    pub breakdown: rustx_types::context::occupancy::ContextBreakdown,
}
impl RuntimeClientContextOccupancy {
    #[must_use]
    pub fn from_native(
        value: &rustx_types::context::occupancy::ContextOccupancy,
        model_source: Option<String>,
    ) -> Self {
        let model = (value.model.len() <= 512).then(|| value.model.clone());
        Self {
            estimated: value.estimated,
            input_tokens: value.input_tokens,
            context_window_tokens: value.context_window_tokens,
            model_deferred: model.is_none(),
            model_source: model.is_none().then_some(model_source).flatten(),
            model,
            breakdown: value.breakdown.clone(),
        }
    }
}
pub fn read(
    store: &dyn crate::ObservationStore,
    through: u64,
) -> Result<Option<RuntimeClientContextOccupancy>, crate::ConversationStoreError> {
    store.observation_occupancy(through)
}
