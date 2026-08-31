//! Tool-plane boundary tests.
//!
//! Native tool contracts (Bash process supervision, the
//! Read/Write/Edit/Grep/Glob filesystem boundary), Skill discovery and
//! packaging, the MCP configuration/runtime boundary, and the uv package
//! backend. These suites own tool-facing contracts; the Agent Loop's
//! scheduling, ordering, and cancellation of tool calls is owned by the
//! in-crate scripted agent suites and is not re-proven here.
//!
//! Bash is Unix-first: those tests are `#[cfg(unix)]` and use controlled
//! temporary workspaces with deterministic subprocess fixtures.

#![allow(clippy::too_many_lines)] // deterministic scenario bodies stay linear

#[path = "../common/mod.rs"]
mod common;

mod bash;
mod mcp;
mod mcp_config;
mod mcp_runtime;
mod skills;
mod tool_packages;
mod tool_plane;
mod uv;
