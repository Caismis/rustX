//! Canonical tool registry and executor contracts for native, MCP, and Python tools.
//!
//! M1 implements the canonical tool data contracts in [`types`]. Tool
//! registration, discovery, and executor implementations are later
//! milestones.

pub mod types;

pub use types::{
    ToolCall, ToolDefinition, ToolExecutionMode, ToolExecutionResult, ToolExecutionStatus,
    ToolOrigin, ToolReplayPolicy, ToolResultContent, TruncationState,
};
