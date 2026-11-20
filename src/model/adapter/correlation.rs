//! Wire correlation is independent of physical and projected occurrence identity.

pub(crate) use rustx_types::model::adapter::correlation::{Plan, visit_anthropic_block};

pub(super) fn request_plan(
    request: &crate::model::ModelRequest,
    boundary: Option<usize>,
) -> Result<Plan, String> {
    #[cfg(feature = "issue-387-profile")]
    let _profile = super::profile::phase(super::profile::Phase::RequestCorrelation);
    let mut cache = super::cache::lock(&request.messages);
    rustx_types::model::adapter::correlation::prepare_plan(
        &mut cache.correlation,
        request,
        boundary,
    )
}
#[cfg(test)]
pub(crate) use rustx_types::model::adapter::correlation::{
    encode_fields, observed_correlations, resolve_wire_labels,
};
