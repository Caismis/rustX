//! Boundary conformance for the Runtime Client: capability projection over a
//! real MCP stdio child server.
//!
//! Transport-independent conformance over in-memory pipes stays in
//! `scripted_suites::runtime_client::conformance`; only the scenarios that
//! spawn a real fixture child live here.

mod mcp_capability;
