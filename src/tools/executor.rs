//! The runtime-owned tool execution contract (M3).
//!
//! M1 defined the declarative tool data contracts in [`types`]; M5 builds the
//! native/MCP/Python executors behind the interface defined here. The agent
//! loop (M3) owns tool execution: it resolves each model-issued
//! [`ToolCall`] against a [`ToolRegistry`] and invokes [`Tool::execute`].
//! Provider adapters describe tool calls; they never execute tools.
//!
//! The contract is intentionally minimal: a tool exposes its immutable
//! [`ToolDefinition`] (name and input schema contract) and executes one call
//! into a normalized [`ToolExecutionResult`]. There is no plugin framework,
//! no dynamic loading, and no parallel scheduling; the loop executes the
//! calls of one turn in deterministic block order.

use futures_util::future::BoxFuture;

use crate::tools::types::{ToolCall, ToolDefinition, ToolExecutionResult};

/// One executable tool.
///
/// Implementations must report the actual execution outcome: a failure is a
/// normalized [`ToolExecutionStatus::Failed`] result, never a fabricated
/// success. The runtime records the returned result verbatim.
///
/// [`ToolExecutionStatus::Failed`]: crate::tools::types::ToolExecutionStatus::Failed
pub trait Tool: Send + Sync {
    /// The immutable tool definition the model sees.
    fn definition(&self) -> &ToolDefinition;

    /// Executes one tool call.
    ///
    /// The returned future is owned by the tool: implementations clone or
    /// capture the data they need and never borrow past the call. A running
    /// tool is not force-aborted by the loop; cancellation is observed at
    /// the loop's check points.
    fn execute<'a>(&'a self, call: &'a ToolCall) -> BoxFuture<'a, ToolExecutionResult>;
}

/// The immutable set of tools visible to one attempt.
///
/// The registry owns the tool definitions the loop sends with every model
/// request and the executable implementations they resolve to. Resolution is
/// deterministic: the canonical [`ToolCall::tool_id`] is matched first, and
/// the model-facing [`ToolCall::name`] is the fallback for streams that
/// carry identity by name only.
#[derive(Default)]
pub struct ToolRegistry {
    tools: Vec<Box<dyn Tool>>,
}

impl ToolRegistry {
    /// Creates an empty registry.
    #[must_use]
    pub fn new() -> Self {
        Self { tools: Vec::new() }
    }

    /// Inserts a tool into the registry.
    pub fn insert(&mut self, tool: impl Tool + 'static) {
        self.tools.push(Box::new(tool));
    }

    /// The canonical definitions of every registered tool, in registration
    /// order. The loop sends these with every model request.
    #[must_use]
    pub fn definitions(&self) -> Vec<ToolDefinition> {
        self.tools
            .iter()
            .map(|tool| tool.definition().clone())
            .collect()
    }

    /// Resolves a tool call to its executable implementation.
    ///
    /// Returns `None` when neither the call's tool id nor its name match a
    /// registered tool; the loop fails the attempt explicitly then instead
    /// of fabricating a tool result.
    #[must_use]
    pub fn resolve(&self, call: &ToolCall) -> Option<&dyn Tool> {
        self.tools
            .iter()
            .find(|tool| tool.definition().id == call.tool_id)
            .or_else(|| {
                self.tools
                    .iter()
                    .find(|tool| tool.definition().name == call.name)
            })
            .map(std::convert::AsRef::as_ref)
    }
}

#[cfg(test)]
mod tests {
    use super::{Tool, ToolRegistry};
    use crate::runtime::identity::ToolCallId;
    use crate::tools::types::{
        ToolCall, ToolDefinition, ToolExecutionMode, ToolExecutionResult, ToolExecutionStatus,
        ToolOrigin, ToolReplayPolicy,
    };
    use futures_util::future::{BoxFuture, ready};
    use serde_json::json;

    fn definition(id: &str, name: &str) -> ToolDefinition {
        ToolDefinition {
            id: crate::runtime::identity::ToolId::new(id),
            name: name.to_owned(),
            description: String::new(),
            input_schema: json!({"type": "object"}),
            execution_mode: ToolExecutionMode::Sequential,
            replay_policy: ToolReplayPolicy::Never,
            origin: ToolOrigin::Builtin,
        }
    }

    struct StubTool {
        definition: ToolDefinition,
    }

    impl StubTool {
        fn new(id: &str, name: &str) -> Self {
            Self {
                definition: definition(id, name),
            }
        }
    }

    impl Tool for StubTool {
        fn definition(&self) -> &ToolDefinition {
            &self.definition
        }

        fn execute<'a>(&'a self, _call: &'a ToolCall) -> BoxFuture<'a, ToolExecutionResult> {
            Box::pin(ready(ToolExecutionResult {
                status: ToolExecutionStatus::Success,
                content: Vec::new(),
                duration_ms: 0,
                exit_code: None,
                artifacts: Vec::new(),
                truncation: None,
            }))
        }
    }

    fn call(tool_id: &str, name: &str) -> ToolCall {
        ToolCall {
            id: ToolCallId::new("call-1"),
            tool_id: crate::runtime::identity::ToolId::new(tool_id),
            name: name.to_owned(),
            arguments: json!({}),
        }
    }

    /// Resolution matches the canonical tool id first.
    #[test]
    fn resolves_by_tool_id_first() {
        let mut registry = ToolRegistry::new();
        registry.insert(StubTool::new("tool-a", "alpha"));
        registry.insert(StubTool::new("tool-b", "beta"));
        let resolved = registry.resolve(&call("tool-b", "alpha")).expect("resolve");
        assert_eq!(resolved.definition().name, "beta");
    }

    /// The model-facing name is the fallback resolution key.
    #[test]
    fn falls_back_to_name() {
        let mut registry = ToolRegistry::new();
        registry.insert(StubTool::new("tool-a", "alpha"));
        let resolved = registry
            .resolve(&call("missing", "alpha"))
            .expect("resolve");
        assert_eq!(resolved.definition().id.as_str(), "tool-a");
    }

    /// An unresolvable call resolves to `None`, never to a fabricated tool.
    #[test]
    fn unknown_call_resolves_to_none() {
        let registry = ToolRegistry::new();
        assert!(registry.resolve(&call("nope", "nope")).is_none());
    }

    /// Definitions are returned in registration order.
    #[test]
    fn definitions_follow_registration_order() {
        let mut registry = ToolRegistry::new();
        registry.insert(StubTool::new("tool-a", "alpha"));
        registry.insert(StubTool::new("tool-b", "beta"));
        let names: Vec<String> = registry
            .definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect();
        assert_eq!(names, vec!["alpha", "beta"]);
    }
}
