//! Tool inspection over the canonical native authorities.
//!
//! Three separate authorities meet here, and keeping them separate is the
//! point:
//!
//! ```text
//! canonical Assistant ToolCall   what the model proposed  (arguments, name)
//! ToolExecutionStarted           that execution began     (never implied)
//! canonical ToolMessage          what the execution produced
//! ```
//!
//! A proposal is not a start, and a start is not a result. Trace reads each
//! from its own authority and never manufactures one from another, so a call
//! the model proposed but the runtime never ran reads as `Proposed` with no
//! result rather than as a silent success.
//!
//! No Tool result is stored, cached or reassembled here. The canonical
//! `ToolMessage` in the Message Ledger remains the one result authority.

use super::bounds::{TraceText, identity_fits};
use super::content::{tool_outcome, tool_result_artifacts, tool_result_blocks, tool_status_detail};
use super::types::{
    TraceManagedOutput, TraceToolDefinition, TraceToolDetail, TraceToolLifecycle, TraceToolResult,
    TraceToolSource, TraceToolTruncation,
};
use crate::message::types::ToolMessageBlock;
use crate::tools::types::{ManagedOutputContinuation, ToolCall};

/// One native Tool contract that identifies an argument field as a program.
struct NativeSourceContract {
    /// The stable native `ToolId`, which is the contract's identity. The
    /// model-facing *name* is deliberately not used: a name is presentation,
    /// while the `ToolId` is what the capability set guarantees.
    tool_id: &'static str,
    /// The argument field the contract states is program source.
    field: &'static str,
    /// The language the contract fixes, when it fixes one.
    language: Option<&'static str>,
}

/// The closed list of rustX native Tool contracts carrying program source.
///
/// Membership is a statement about a *native contract*, not a guess about a
/// tool's purpose:
///
/// - `tool-bash` documents its `command` argument as the string handed to
///   one `/bin/bash -c` invocation, so the field is a shell program and the
///   language is fixed by the contract itself.
/// - `tool-write` documents its `content` argument as the complete new UTF-8
///   body of a file. That is source text, so it renders as source — but the
///   contract fixes no language, and rustX does not infer one from the
///   `path` extension, because a filename is not a type authority anywhere
///   else in this projection either.
///
/// An MCP or extension tool is absent on purpose: rustX has no
/// general-purpose semantic by which a third-party schema can declare a
/// field to be code, and inventing a speculative one for tools that do not
/// exist yet is exactly what #364 excludes.
const NATIVE_SOURCE_CONTRACTS: &[NativeSourceContract] = &[
    NativeSourceContract {
        tool_id: "tool-bash",
        field: "command",
        language: Some("shell"),
    },
    NativeSourceContract {
        tool_id: "tool-write",
        field: "content",
        language: None,
    },
];

/// Program source carried by this call's arguments, when a native contract
/// identifies one.
#[must_use]
pub(super) fn native_source(call: &ToolCall) -> Option<TraceToolSource> {
    let contract = NATIVE_SOURCE_CONTRACTS
        .iter()
        .find(|contract| contract.tool_id == call.tool_id.as_str())?;
    let text = call.arguments.get(contract.field)?.as_str()?;
    Some(TraceToolSource {
        field: contract.field.to_owned(),
        text: TraceText::detail(text),
        language: contract.language.map(str::to_owned),
    })
}

/// Projects the managed-output continuation's semantics without its locator.
fn managed_output(continuation: &ManagedOutputContinuation) -> TraceManagedOutput {
    match continuation {
        ManagedOutputContinuation::Complete { .. } => TraceManagedOutput {
            complete: true,
            available: true,
            diagnostic: None,
        },
        ManagedOutputContinuation::Partial { diagnostic, .. } => TraceManagedOutput {
            complete: false,
            available: true,
            diagnostic: Some(TraceText::detail(diagnostic)),
        },
        ManagedOutputContinuation::Unavailable { diagnostic } => TraceManagedOutput {
            complete: false,
            available: false,
            diagnostic: Some(TraceText::detail(diagnostic)),
        },
    }
}

/// Projects one canonical Tool message as inspectable result detail.
#[must_use]
pub(super) fn tool_result(message: &ToolMessageBlock) -> TraceToolResult {
    let result = &message.result;
    let (blocks, blocks_truncated) = tool_result_blocks(result);
    let (attachments, attachments_truncated) = tool_result_artifacts(result);
    TraceToolResult {
        outcome: tool_outcome(&result.status),
        detail: tool_status_detail(&result.status)
            .as_deref()
            .map(TraceText::detail),
        blocks,
        blocks_truncated: blocks_truncated || attachments_truncated,
        // Authoritative execution duration, measured by the execution itself
        // rather than derived from two presentation timestamps.
        duration_ms: result.duration_ms,
        exit_code: result.exit_code,
        attachments,
        truncation: result.truncation.as_ref().map(|state| TraceToolTruncation {
            truncated: state.truncated,
            original_bytes: state.original_bytes,
        }),
        managed_output: result.managed_output.as_ref().map(managed_output),
    }
}

/// Assembles Tool detail from the three separate native authorities.
///
/// `call` is the canonical proposal, `started` is the durable start fact,
/// and `message` is the canonical result. Each may be absent independently,
/// and the resulting lifecycle states exactly what is proven.
#[must_use]
pub(super) fn tool_detail(
    call_id: &crate::runtime::identity::ToolCallId,
    tool_id: &crate::runtime::identity::ToolId,
    call: Option<&ToolCall>,
    definition: Option<TraceToolDefinition>,
    started: bool,
    message: Option<&ToolMessageBlock>,
) -> TraceToolDetail {
    TraceToolDetail {
        call_id: call_id.clone(),
        tool_id: tool_id.clone(),
        name: call.map(|call| call.name.clone()),
        lifecycle: if message.is_some() {
            TraceToolLifecycle::Settled
        } else if started {
            TraceToolLifecycle::Started
        } else {
            TraceToolLifecycle::Proposed
        },
        arguments: call.map(|call| super::bounds::TraceJson::bounded(&call.arguments)),
        source: call.and_then(native_source),
        definition,
        result: message.map(tool_result),
    }
}

/// The historical definition of one Tool inside one request's frozen catalog.
///
/// The definition comes from the request the call belongs to, so a Tool whose
/// schema changed after the call still inspects with the schema the model
/// actually saw.
#[must_use]
pub(super) fn historical_definition(
    snapshot: &crate::model::snapshot::RequestSnapshot,
    tool_id: &crate::runtime::identity::ToolId,
) -> Option<TraceToolDefinition> {
    snapshot
        .tool_definitions
        .iter()
        .find(|definition| definition.id == *tool_id)
        .filter(|definition| identity_fits(definition.id.as_str()))
        .map(|definition| TraceToolDefinition {
            tool_id: definition.id.clone(),
            name: definition.name.clone(),
            description: TraceText::detail(&definition.description),
            input_schema: super::bounds::TraceJson::bounded(&definition.input_schema),
        })
}

#[cfg(test)]
mod tests {
    use super::{NATIVE_SOURCE_CONTRACTS, native_source};
    use crate::runtime::identity::{ToolCallId, ToolId};
    use crate::tools::types::ToolCall;

    fn call(tool_id: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            id: ToolCallId::new("call-1"),
            tool_id: ToolId::new(tool_id),
            name: "any".to_owned(),
            arguments,
        }
    }

    /// The Bash contract fixes both the source field and its language.
    #[test]
    fn bash_command_is_recognized_shell_source() {
        let source = native_source(&call(
            "tool-bash",
            serde_json::json!({ "command": "ls -la\necho done", "timeout": 5 }),
        ))
        .expect("native shell source");
        assert_eq!(source.field, "command");
        assert_eq!(source.language.as_deref(), Some("shell"));
        assert_eq!(source.text.text, "ls -la\necho done");
    }

    /// The Write contract fixes a source field but no language.
    #[test]
    fn write_content_is_source_without_a_claimed_language() {
        let source = native_source(&call(
            "tool-write",
            serde_json::json!({ "path": "src/main.rs", "content": "fn main() {}" }),
        ))
        .expect("native source");
        assert_eq!(source.field, "content");
        assert_eq!(source.language, None);
        assert_eq!(source.text.text, "fn main() {}");
    }

    /// A tool outside the native contract list carries no source view.
    #[test]
    fn other_tools_have_no_source_contract() {
        assert!(native_source(&call("tool-read", serde_json::json!({ "path": "a" }))).is_none());
        assert!(
            native_source(&call(
                "mcp-vendor-run",
                serde_json::json!({ "code": "print(1)" })
            ))
            .is_none(),
            "no speculative code semantic is invented for third-party tools"
        );
    }

    /// A contract field holding a non-string value is not source.
    #[test]
    fn a_non_string_contract_field_is_not_source() {
        assert!(native_source(&call("tool-bash", serde_json::json!({ "command": 7 }))).is_none());
        assert!(native_source(&call("tool-bash", serde_json::json!({}))).is_none());
    }

    /// Every contract names a stable native Tool identity, never a name.
    #[test]
    fn contracts_are_keyed_by_native_tool_identity() {
        for contract in NATIVE_SOURCE_CONTRACTS {
            assert!(
                contract.tool_id.starts_with("tool-"),
                "{}",
                contract.tool_id
            );
        }
    }
}
