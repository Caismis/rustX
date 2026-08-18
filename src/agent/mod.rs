//! Agent kernel: attempts, turns, execution state machines, and loop semantics.
//!
//! M3 implements the deterministic agent execution loop on top of the
//! canonical runtime contracts:
//!
//! - [`AgentExecution`] executes one attempt: model request → canonical
//!   `ModelEvent` stream → message assembly → optional tool execution →
//!   continuation → one terminal settlement candidate, normally committed as
//!   exactly one terminal `RuntimeEvent`.
//! - [`ExecutionStateMachine`] makes the attempt lifecycle explicit
//!   (`Idle → RunningModel → WaitingForTool → RunningModel → Completed`,
//!   with failure/cancellation settling from any active state).
//! - [`AgentCancellation`] is the attempt-level cancellation trigger; every
//!   model invocation observes a child signal through the existing adapter
//!   cancellation mechanism.
//! - [`AttemptLifecycle`] carries the three typed phase-specific ownership
//!   seams of Issues #56/#64 — [`PreStepPolicy`], [`PreToolPolicy`], and
//!   [`ToolResultObserver`] — as one required immutable per-attempt
//!   configuration. The loop remains the
//!   lifecycle owner; neither seam receives canonical, tool-identity,
//!   cancellation, or terminal authority, and neither decides the semantic
//!   ownership of the context it proposes.
//!
//! The kernel operates only on canonical contracts: it never references a
//! provider protocol, a provider SDK type, or a provider concept.

mod assembly;
pub mod cancellation;
pub mod execution;
pub mod lifecycle;
pub mod observer;
pub mod state;

pub use crate::runtime::inbound::InitialTurnTrigger;
pub use cancellation::AgentCancellation;
pub use execution::{
    AgentExecution, AgentExecutionRequest, AgentExecutionResult, DurableFailureKind,
};
pub use lifecycle::{
    AlwaysAllow, AlwaysEnter, AttemptLifecycle, LifecycleError, NoDeferredContext,
    ObservedToolInvocation, PreStepBatch, PreStepDecision, PreStepPolicy, PreToolDecision,
    PreToolPolicy, PreToolView, RegisteredToolResultObserver, ToolResultObservation,
    ToolResultObserver,
};
pub use observer::{AgentExecutionObserver, AgentStatusObservation};
pub use state::{ExecutionState, ExecutionStateMachine};
