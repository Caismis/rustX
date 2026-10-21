//! Auxiliary Call/request ownership, settlement, audit growth, supervision and
//! adversarial contracts at the SQLite boundary. The children share the
//! store-level test scope: the SQLite owner and its common fixtures.
use crate::durable::sqlite::test_support::*;
use crate::durable::sqlite::*;

mod adversarial;
#[allow(clippy::too_many_lines, clippy::type_complexity)]
mod contract;
#[allow(clippy::too_many_lines)]
mod growth;
#[allow(clippy::too_many_lines, clippy::type_complexity)]
mod ownership;
mod supervision;
