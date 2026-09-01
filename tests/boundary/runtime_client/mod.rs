//! Boundary conformance for the Runtime Client: capability projection over
//! real MCP stdio child servers.
//!
//! Transport-independent conformance over in-memory pipes stays in
//! `scripted_suites::runtime_client::conformance`; only the scenarios that
//! spawn a real child live here: the configured-server fixture in
//! [`mcp_capability`] (this binary re-executed in fixture mode) and the
//! managed Python package projection in [`python_capability`] (a real,
//! network-bound `uv` build serving a real `FastMCP` child, Issue #174).

mod mcp_capability;
mod python_capability;
