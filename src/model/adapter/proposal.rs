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
//!     -> proposal identity resolution          (this module)
//!         -> `ModelEvent::ToolCallStarted`, argument streaming
//!     -> complete argument validation          (this module)
//!     -> `ToolCall` ACCEPTANCE  <- linearization point
//!         -> canonical `ToolCall` in `ModelEvent::ToolCallCompleted`
//! ```
//!
//! # There is exactly one acceptance point
//!
//! The two stages are deliberately not the same event, and only the second
//! is acceptance.
//!
//! [`resolve_tool_identity`] establishes that a proposal is *attributable*:
//! it has a usable correlation identity and a tool identity that resolves to
//! one declared [`ToolId`]. That is what makes it legal to emit the
//! canonical [`ModelEvent::ToolCallStarted`] and stream argument deltas —
//! observability of a proposal in flight. It is **not** acceptance: the
//! resulting [`ToolCallStart`] carries no arguments, so no executable
//! [`ToolCall`] exists yet and nothing may be preflighted, approved, or run
//! from it.
//!
//! [`accept_tool_call_arguments`] is the acceptance linearization point. The
//! complete argument representation is parsed exactly once, and only on
//! success does the full canonical
//! `ToolCall { id, tool_id, name, arguments }` exist. Everything downstream
//! of that value — Tool preflight, approval, execution, exactly-once
//! `ToolResult` settlement — begins here, including Tool schema rejection,
//! which is deliberately **not** performed in this module.
//!
//! # Nothing is invented
//!
//! Neither stage repairs a proposal to make it fit: no synthesized
//! invocation id, no guessed or fuzzy-matched tool name, no repaired or
//! brace-completed JSON, no reconstruction of a call from reasoning text or
//! leaked protocol markup. A proposal that fails either stage is refused as
//! [`ModelErrorKind::MalformedToolProposal`](crate::model::error::ModelErrorKind::MalformedToolProposal)
//! with provider-independent provenance, and the Agent Loop owns the single
//! bounded corrective regeneration that class authorizes.
//!
//! [`ModelEvent::ToolCallStarted`]: crate::model::event::ModelEvent::ToolCallStarted

use crate::model::adapter::validation::ValidatedTools;
use crate::model::error::{MalformedToolProposalSource, ModelError};
use crate::runtime::identity::ToolCallId;
use crate::tools::types::{ToolCall, ToolCallStart};

/// Resolves the identity of one tool proposal, making it attributable.
///
/// The correlation identity and the tool identity must both be present and
/// usable: a missing one is a stream that never delivered a complete
/// invocation, and a name that resolves to no declared tool is an identity
/// this runtime cannot execute. Adapters call this as soon as a proposal's
/// identity is known, which is also when the canonical
/// [`ModelEvent::ToolCallStarted`](crate::model::event::ModelEvent::ToolCallStarted)
/// may be emitted and argument deltas may stream.
///
/// This is **not** `ToolCall` acceptance. The returned [`ToolCallStart`] is
/// a resolved identity with no arguments; the executable canonical
/// [`ToolCall`] does not exist until [`accept_tool_call_arguments`]
/// succeeds.
///
/// # Errors
///
/// Returns a [`MalformedToolProposalSource::StreamAssembly`] failure for a
/// missing identity and a [`MalformedToolProposalSource::AdapterStructural`]
/// failure for an unusable one.
pub(crate) fn resolve_tool_identity(
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

/// The `ToolCall` acceptance linearization point.
///
/// The complete argument text of a proposal whose identity is already
/// resolved is parsed exactly once, here. A truncated or otherwise
/// unparseable representation is refused rather than repaired. On success
/// the full canonical `ToolCall { id, tool_id, name, arguments }` exists for
/// the first time, and the ordinary Tool contract — preflight, approval,
/// execution, exactly-once settlement — applies to it unchanged.
///
/// # Errors
///
/// Returns a [`MalformedToolProposalSource::AdapterStructural`] failure when
/// the argument text is not one complete JSON value.
pub(crate) fn accept_tool_call_arguments(
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

/// Runs both stages for a proposal that was assembled complete, so identity
/// resolution and `ToolCall` acceptance happen back to back.
///
/// # Errors
///
/// Returns the malformed-proposal failure of whichever stage refused the
/// proposal.
pub(crate) fn accept_tool_call(
    call_id: Option<&str>,
    name: Option<&str>,
    arguments: &str,
    tools: &ValidatedTools,
) -> Result<ToolCall, ModelError> {
    let start = resolve_tool_identity(call_id, name, tools)?;
    accept_tool_call_arguments(&start, arguments)
}

#[cfg(test)]
mod tests {
    use super::{accept_tool_call, accept_tool_call_arguments, resolve_tool_identity};
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
            let error = resolve_tool_identity(id, Some("write_file"), &tools())
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
                resolve_tool_identity(Some("call_1"), name, &tools()).expect_err("must fail");
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
        let error = resolve_tool_identity(Some("call_1"), Some("write_fil"), &tools())
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
            resolve_tool_identity(Some("call_1"), Some("write_file"), &tools()).expect("identity");
        for arguments in ["", "{\"path\":\"a.txt\"", "{\"path\":"] {
            let error = accept_tool_call_arguments(&start, arguments).expect_err("must fail");
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
