//! Real-process and composition boundary tests.
//!
//! These suites either spawn the **actual `rustx` binary** and drive it over
//! the stdio/JSONL Runtime Client transport, or compose the real local
//! runtime (`LocalConversationCore` and its final paths) in process. They
//! exist because the contract under test is a process/IPC/stdio or
//! composition fact that no in-process scripted seam can establish.
//!
//! Readiness is the protocol itself: the driver writes one request and
//! blocks on the correlated response. Wall-clock timeouts are outer liveness
//! guards only.

#![allow(clippy::too_many_lines)] // deterministic scenario bodies stay linear

#[path = "../common/mod.rs"]
mod common;

mod capability_startup;
mod composition;
mod examples;
mod runtime_config;
mod runtime_process;
mod sessions;
