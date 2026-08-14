//! Agent kernel: attempts, turns, execution state machines, and loop semantics.
//!
//! M3 implements the deterministic agent execution loop on top of the
//! canonical runtime contracts:
//!
//! - [`AgentExecution`] executes one attempt: model request → canonical
//!   `ModelEvent` stream → message assembly → optional tool execution →
//!   continuation → exactly one terminal `RuntimeEvent`.
//! - [`ExecutionStateMachine`] makes the attempt lifecycle explicit
//!   (`Idle → RunningModel → WaitingForTool → RunningModel → Completed`,
//!   with failure/cancellation settling from any active state).
//! - [`AgentCancellation`] is the attempt-level cancellation trigger; every
//!   model invocation observes a child signal through the existing adapter
//!   cancellation mechanism.
//!
//! The kernel operates only on canonical contracts: it never references a
//! provider protocol, a provider SDK type, or a provider concept.

mod assembly;
pub mod cancellation;
pub mod execution;
pub mod observer;
pub mod state;

pub use crate::runtime::inbound::InitialTurnTrigger;
pub use cancellation::AgentCancellation;
pub use execution::{AgentExecution, AgentExecutionRequest, AgentExecutionResult};
pub use observer::{AgentExecutionObserver, AgentStatusObservation};
pub use state::{ExecutionState, ExecutionStateMachine};
