//! Canonical tool registry and executor contracts for native, MCP, and Python tools.
//!
//! M1 implements the canonical tool data contracts in [`types`]. M3 adds the
//! runtime-owned execution contract ([`Tool`], [`ToolRegistry`]): the agent
//! loop resolves model-issued calls against the registry and executes them.
//! Native, MCP, and Python executor implementations are milestone M5+.

pub mod executor;
pub mod types;

pub use executor::{Tool, ToolRegistry};
pub use types::{
    ToolCall, ToolCallStart, ToolDefinition, ToolExecutionMode, ToolExecutionResult,
    ToolExecutionStatus, ToolOrigin, ToolReplayPolicy, ToolResultContent, TruncationState,
};
