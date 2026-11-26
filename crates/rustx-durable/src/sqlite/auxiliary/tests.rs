//! Auxiliary Call/request ownership, settlement, audit growth, supervision and
//! adversarial contracts at the SQLite boundary. The children share the
//! store-level test scope: the SQLite owner and its common fixtures.
use crate::sqlite::test_support::*;
use crate::sqlite::*;

mod adversarial;
#[allow(clippy::too_many_lines, clippy::type_complexity)]
mod contract;
#[allow(clippy::too_many_lines)]
mod growth;
#[allow(clippy::too_many_lines, clippy::type_complexity)]
mod ownership;

mod retry_title;
