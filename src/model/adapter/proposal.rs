//! The `ToolCall` acceptance boundary.
//!
//! A model-emitted tool proposal is **non-authoritative** wire state. Each
//! adapter recognizes and assembles it in its own protocol terms — provider
//! envelopes, chunk indexes, snapshot merges, block ids — and then presents
//! the assembled proposal here exactly once:
//!
//! ```text
//! provider/model stream
//!     -> provider-specific proposal assembly   (adapter, protocol-aware)
//!     -> structural proposal validation        (this module)
//!     -> `ToolCall` acceptance                   (this module)
//!     -> canonical `ToolCall`                    (`ModelEvent::ToolCall*`)
//! ```
//!
//! Before acceptance a proposal cannot execute, cannot be settled, and
//! cannot enter canonical history. After acceptance the canonical
//! [`ToolCall`] exists and the ordinary exactly-once Tool settlement
//! contract applies unchanged — including Tool preflight/schema rejection,
//! which is deliberately **not** performed here.
//!
//! Acceptance validates only what makes an invocation structurally
//! trustworthy:
//!
//! - a usable correlation identity (the provider's call id);
//! - a usable tool identity (a declared name resolving to one [`ToolId`]);
//! - an argument representation that is one complete JSON value.
//!
//! Nothing is ever invented to make a proposal acceptable: no synthesized
//! id, no guessed name, no fuzzy tool matching, no repaired JSON. A
//! proposal that fails any check is refused as
//! [`ModelErrorKind::MalformedToolProposal`](crate::model::error::ModelErrorKind::MalformedToolProposal)
//! with provider-independent provenance, and the Agent Loop owns the single
//! bounded corrective regeneration that class authorizes.

use crate::model::adapter::validation::ValidatedTools;
use crate::model::error::{MalformedToolProposalSource, ModelError};
use crate::runtime::identity::ToolCallId;
use crate::tools::types::{ToolCall, ToolCallStart};

/// Accepts the identity half of one tool proposal.
///
/// The correlation identity and the tool identity must both be present and
/// usable: a missing one is a stream that never delivered a complete
/// invocation, and a name that resolves to no declared tool is an identity
/// this runtime cannot execute. Adapters call this as soon as a proposal's
/// identity is known, which is also when the canonical
/// [`ModelEvent::ToolCallStarted`](crate::model::event::ModelEvent::ToolCallStarted)
/// may be emitted.
///
/// # Errors
///
/// Returns a [`MalformedToolProposalSource::StreamAssembly`] failure for a
/// missing identity and a [`MalformedToolProposalSource::AdapterStructural`]
/// failure for an unusable one.
pub(crate) fn accept_tool_identity(
    call_id: Option<&str>,
    name: Option<&str>,
    tools: &ValidatedTools,
) -> Result<ToolCallStart, ModelError> {
    let call_id = call_id.filter(|id| !id.is_empty()).ok_or_else(|| {
        ModelError::malformed_tool_proposal(
            MalformedToolProposalSource::StreamAssembly,
            "the model tool proposal carries no usable invocation id",
        )
    })?;
    let name = name.filter(|name| !name.is_empty()).ok_or_else(|| {
        ModelError::malformed_tool_proposal(
            MalformedToolProposalSource::StreamAssembly,
            format!("tool proposal {call_id} carries no usable tool name"),
        )
    })?;
    let tool_id = tools.resolve(name).cloned().ok_or_else(|| {
        ModelError::malformed_tool_proposal(
            MalformedToolProposalSource::AdapterStructural,
            format!("the model proposed the unknown tool name {name:?}"),
        )
    })?;
    Ok(ToolCallStart {
        id: ToolCallId::new(call_id),
        tool_id,
        name: name.to_owned(),
    })
}

/// Completes acceptance of a proposal whose identity was already accepted.
///
/// The complete argument text is parsed exactly once, here. A truncated or
/// otherwise unparseable representation is refused rather than repaired.
///
/// # Errors
///
/// Returns a [`MalformedToolProposalSource::AdapterStructural`] failure when
/// the argument text is not one complete JSON value.
pub(crate) fn accept_tool_arguments(
    start: &ToolCallStart,
    arguments: &str,
) -> Result<ToolCall, ModelError> {
    let arguments = serde_json::from_str(arguments).map_err(|error| {
        ModelError::malformed_tool_proposal(
            MalformedToolProposalSource::AdapterStructural,
            format!(
                "tool proposal {:?} ({}) has a malformed argument representation: {error}",
                start.name, start.id
            ),
        )
    })?;
    Ok(ToolCall {
        id: start.id.clone(),
        tool_id: start.tool_id.clone(),
        name: start.name.clone(),
        arguments,
    })
}

/// Accepts one fully assembled tool proposal as a canonical [`ToolCall`].
///
/// # Errors
///
/// Returns the malformed-proposal failure of whichever structural check
/// refused the proposal.
pub(crate) fn accept_tool_call(
    call_id: Option<&str>,
    name: Option<&str>,
    arguments: &str,
    tools: &ValidatedTools,
) -> Result<ToolCall, ModelError> {
    let start = accept_tool_identity(call_id, name, tools)?;
    accept_tool_arguments(&start, arguments)
}

#[cfg(test)]
mod tests {
    use super::{accept_tool_arguments, accept_tool_call, accept_tool_identity};
    use crate::model::adapter::validation::{ValidatedTools, validate_request};
    use crate::model::error::{MalformedToolProposalSource, ModelErrorKind};
    use crate::model::types::{ModelProtocol, ModelRequest};
    use crate::runtime::identity::ToolId;
    use crate::tools::types::ModelToolDefinition;

    fn tools() -> ValidatedTools {
        let request = ModelRequest {
            invocation: crate::model::invocation::ModelInvocationConfig {
                model: "m".to_owned(),
                protocol: ModelProtocol::OpenAiChatCompletions,
                max_output_tokens: 512,
                request_params: crate::model::invocation::RequestParams::new(),
                capabilities: crate::model::catalog::ModelCapabilities::text_only(true, true),
                compat: crate::model::catalog::ModelCompat::default(),
            },
            messages: Vec::new(),
            tools: vec![ModelToolDefinition {
                id: ToolId::new("tool-write"),
                name: "write_file".to_owned(),
                description: "d".to_owned(),
                input_schema: serde_json::json!({"type": "object"}),
            }],
            effective_system_prompt: String::new(),
            continuation: None,
        };
        validate_request(&request, ModelProtocol::OpenAiChatCompletions).expect("valid request")
    }

    /// A complete proposal crosses acceptance into a canonical call whose
    /// arguments are the parsed provider JSON, unmodified.
    #[test]
    fn a_complete_proposal_becomes_a_canonical_call() {
        let call = accept_tool_call(
            Some("call_1"),
            Some("write_file"),
            r#"{"path":"a.txt"}"#,
            &tools(),
        )
        .expect("acceptance succeeds");
        assert_eq!(call.id.as_str(), "call_1");
        assert_eq!(call.tool_id.as_str(), "tool-write");
        assert_eq!(call.name, "write_file");
        assert_eq!(call.arguments, serde_json::json!({"path": "a.txt"}));
    }

    /// A missing or empty correlation identity is a stream-assembly refusal,
    /// never a fabricated id.
    #[test]
    fn a_missing_invocation_id_is_refused() {
        for id in [None, Some("")] {
            let error = accept_tool_identity(id, Some("write_file"), &tools())
                .expect_err("acceptance must fail");
            assert_eq!(error.kind, ModelErrorKind::MalformedToolProposal);
            assert_eq!(
                error.malformed_tool_proposal,
                Some(MalformedToolProposalSource::StreamAssembly)
            );
        }
    }

    /// A missing tool name is refused rather than guessed.
    #[test]
    fn a_missing_tool_name_is_refused() {
        for name in [None, Some("")] {
            let error =
                accept_tool_identity(Some("call_1"), name, &tools()).expect_err("must fail");
            assert_eq!(
                error.malformed_tool_proposal,
                Some(MalformedToolProposalSource::StreamAssembly)
            );
        }
    }

    /// An undeclared tool name is refused; nothing is fuzzy-matched onto a
    /// declared tool.
    #[test]
    fn an_unknown_tool_name_is_refused_without_fuzzy_matching() {
        let error = accept_tool_identity(Some("call_1"), Some("write_fil"), &tools())
            .expect_err("must fail");
        assert_eq!(error.kind, ModelErrorKind::MalformedToolProposal);
        assert_eq!(
            error.malformed_tool_proposal,
            Some(MalformedToolProposalSource::AdapterStructural)
        );
    }

    /// Truncated or absent argument JSON is refused; no brace is appended and
    /// no empty object is substituted.
    #[test]
    fn truncated_arguments_are_refused_without_repair() {
        let start =
            accept_tool_identity(Some("call_1"), Some("write_file"), &tools()).expect("identity");
        for arguments in ["", "{\"path\":\"a.txt\"", "{\"path\":"] {
            let error = accept_tool_arguments(&start, arguments).expect_err("must fail");
            assert_eq!(error.kind, ModelErrorKind::MalformedToolProposal);
            assert_eq!(
                error.malformed_tool_proposal,
                Some(MalformedToolProposalSource::AdapterStructural)
            );
        }
    }

    /// Structurally valid JSON that violates a declared Tool schema is still
    /// accepted here: schema validation belongs to Tool preflight, not to
    /// `ToolCall` acceptance.
    #[test]
    fn schema_violating_arguments_still_cross_acceptance() {
        let call = accept_tool_call(
            Some("call_1"),
            Some("write_file"),
            r#"{"unexpected":[1,2,3]}"#,
            &tools(),
        )
        .expect("structural acceptance succeeds");
        assert_eq!(call.arguments, serde_json::json!({"unexpected": [1, 2, 3]}));
    }
}
