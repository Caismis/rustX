//! Provider adapter boundary tests.
//!
//! Every suite here drives one real provider adapter against the low-level
//! in-process [`common::FixtureServer`]: request serialization, wire-shape
//! and protocol translation, stream parsing, continuation, and normalized
//! provider error mapping. No Agent Loop, no runtime, no external process.
//!
//! Composed Agent Loop conformance through a real external provider process
//! belongs to the `conformance` target instead. Live credentialed smoke
//! tests live in [`live`] and stay `#[ignore]`d: they are never correctness
//! authority for deterministic runtime semantics.

#![allow(clippy::too_many_lines)] // deterministic scenario bodies stay linear

#[path = "../common/mod.rs"]
mod common;

mod anthropic;
mod capability_boundary;
mod context_boundary;
mod live;
mod openai_chat;
mod openai_responses;
mod request_params;
mod transport;
