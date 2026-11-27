//! Frozen prepared-output payloads shared with the types contract crate.

pub use rustx_types::model::structured::*;

use crate::runtime::identity::CompactionOperationId;

struct CompactionFallbackOwner;

pub(crate) fn authorize_prompt_fallback(
    request: &mut super::ModelRequest,
    operation_id: CompactionOperationId,
) {
    request.authorize_prompt_fallback::<CompactionFallbackOwner>(operation_id);
}

pub(crate) fn has_prompt_fallback_authorization(
    request: &super::ModelRequest,
    operation_id: &CompactionOperationId,
) -> bool {
    request.has_prompt_fallback_authorization::<CompactionFallbackOwner>(operation_id)
}

#[cfg(test)]
#[path = "structured_tests.rs"]
mod tests;
