//! Native bounded observation contracts.
//!
//! Every value here is prepared by a native owner — the ConversationStore at
//! its commit, the runtime at a published generation, or an interaction or
//! status source — and is read unchanged by every adapter. These are finite
//! read descriptors, never execution, settlement, or recovery authority:
//!
//! - [`bounded`] and [`preflight`] bound encoded display values by their
//!   selected budget, independent of source size; [`structural_json`] is
//!   the source-owned JSON encoding of typed native domains;
//! - [`content`] and [`cut`] are the exact byte locators, content
//!   references, read cuts and collection frontiers of one observation;
//! - [`capture`] copies finite descriptors under a byte budget;
//! - the descriptor modules ([`transcript`], [`tool`], [`interaction`],
//!   [`status`], [`turn`], [`statistics`], [`occupancy`], [`delivery`],
//!   [`goal`], [`head`], [`failure`], [`trace`]) are the shared finite
//!   vocabulary persisted in derived read layouts and published to clients;
//! - the read sources ([`capability_source`], [`resource_source`],
//!   [`workflow_source`]) borrow one immutable native generation and prepare
//!   its bounded display head once, shared by every consumer.
//!
//! The Runtime Client owns projection linearization, checkpoint and page
//! assembly, cursors and subscriptions over these values; the App Server owns
//! their wire framing. Neither re-derives a descriptor from its source.

pub mod bounded;
pub mod capability_source;
pub(crate) mod capture;
pub mod content;
pub mod cut;
pub mod delivery;
pub mod failure;
pub mod goal;
pub mod head;
pub mod interaction;
pub mod occupancy;
pub(crate) mod preflight;
pub mod resource_source;
pub mod statistics;
pub mod status;
pub(crate) mod structural_json;
pub mod tool;
pub mod trace;
pub mod transcript;
pub mod turn;
pub(crate) mod workflow_source;
