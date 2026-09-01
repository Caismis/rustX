//! Native tool registry contracts and the conversation task list.
//!
//! Model-facing schema and registry-boundary preflight ([`native_contracts`]),
//! the native `todo` tool through the ordinary registry boundary
//! ([`todo_plane`]), and the task list's atomic settlement where its results
//! become canonical ([`todo_transaction`]).

mod native_contracts;
mod todo_plane;
mod todo_transaction;
