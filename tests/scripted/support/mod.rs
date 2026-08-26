//! The fixtures of the in-crate scripted suites.
//!
//! The scripted [`ModelAdapter`](rustx::model::ModelAdapter) in [`fake`] and
//! the scripted [`ContextSummarizer`](rustx::context::ContextSummarizer) in
//! [`context`] are ordinary trait implementations, but binding either into a
//! real runtime object — [`model`] for a catalog binding,
//! [`runtime_client_fixture`] for a whole host — is only possible through a
//! seam that does not exist in the published API. Everything here is
//! therefore reachable only from the in-crate suites in [`super`], never
//! from an integration-test binary and never from a consumer of the library.
//!
//! Fixtures shared with the remaining integration-test binaries live in
//! [`super::common`] instead.

#![allow(dead_code)] // every helper is used only by some suites

pub(crate) mod context;
pub(crate) mod fake;
pub(crate) mod model;
pub(crate) mod runtime_client_conformance;
pub(crate) mod runtime_client_fixture;
pub(crate) mod todo;

/// The immutable attempt model snapshot of a loop suite.
///
/// The binding is resolved through a real fixture catalog — explicit
/// endpoint, explicit credential, validated limits and capabilities — so a
/// loop suite exercises the same selection path production uses; only the
/// adapter behind it is scripted.
pub fn attempt_model(
    adapter: std::sync::Arc<dyn rustx::model::ModelAdapter>,
    model: &str,
) -> rustx::model::AttemptModelSnapshot {
    attempt_model_with_window(adapter, model, 10_000_000, 512)
}

/// A redacted attempt model view of one model reference.
///
/// Wire-contract suites need a well-formed `AttemptModelView` without
/// standing up a catalog binding: the value is protocol payload there, not a
/// resolution under test.
pub fn attempt_model_view(reference: &str) -> rustx::model::AttemptModelView {
    let capabilities = rustx::model::ModelCapabilities::text_only(true, false);
    rustx::model::AttemptModelView {
        primary: rustx::model::ModelInvocationView {
            model: rustx::model::ModelRef::parse(reference).expect("a valid model reference"),
            protocol: rustx::model::ModelProtocol::OpenAiChatCompletions,
            context_window: 128_000,
            model_max_output_tokens: 4096,
            max_output_tokens: 4096,
            reasoning_profile: None,
            reasoning_enabled: false,
            request_params: rustx::model::RequestParams::new(),
            capabilities: capabilities.clone(),
            declared_capabilities: capabilities,
        },
        summary: rustx::model::SummaryModelView::Session,
    }
}

/// The attempt model snapshot of a loop suite with explicit limits.
pub fn attempt_model_with_window(
    adapter: std::sync::Arc<dyn rustx::model::ModelAdapter>,
    model: &str,
    context_window: u64,
    max_output_tokens: u32,
) -> rustx::model::AttemptModelSnapshot {
    use model::{FixtureModel, ScriptedAdapterFactory, fixture_session_model};
    let reference = format!("fixture/{model}");
    fixture_session_model(
        &[FixtureModel::text(
            &reference,
            rustx::model::ModelProtocol::OpenAiChatCompletions,
        )
        .with_context_window(context_window)
        .with_max_output_tokens(max_output_tokens)],
        &reference,
        &ScriptedAdapterFactory::new(adapter),
    )
    .snapshot()
}
