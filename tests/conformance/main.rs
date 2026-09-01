//! Composed conformance through the real provider-emulator boundary.
//!
//! Every suite here composes the **real** runtime — the real model catalog,
//! the resolved model binding, the production provider adapter, the real
//! HTTP client and streaming parser, the Agent Loop, the context engine, the
//! tool runtime, the capability plane, and the Runtime Client transport —
//! against the external [`common::provider_emulator::ProviderEmulator`]
//! process. The emulator's strict ordered scenarios and deterministic gates
//! are the synchronization; nothing here sleeps to manufacture a race.
//!
//! These suites prove that the generic runtime contracts survive composition
//! with a real provider boundary. The contracts themselves are owned by the
//! in-crate scripted suites; nothing here re-proves their state machines.
//!
//! The emulator is mandatory in CI: `RUSTX_REQUIRE_PROVIDER_EMULATOR=1`
//! turns a missing emulator toolchain into a hard failure instead of a skip.

#![allow(clippy::too_many_lines)] // deterministic scenario bodies stay linear

#[path = "../common/mod.rs"]
mod common;

mod agent_loop;
mod examples;
mod lifecycle;
mod workflow;
