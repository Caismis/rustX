//! The explicit execution state machine of one agent attempt.
//!
//! M3 execution semantics are expressed as a state machine instead of
//! ad-hoc async control flow. The states are:
//!
//! ```text
//! Idle
//!   ↓ start()
//! RunningModel
//!   ↓ model_finished(pending_tools)
//! WaitingForTool ──or── Completed
//!   ↓ tools_finished()
//! RunningModel
//!   ↓ ...
//! Completed | Failed
//! ```
//!
//! A terminal state is absorbing: no transition leaves it, so the loop
//! cannot accidentally emit execution facts after the attempt settled.
//! Cancellation terminates through the failure path ([`ExecutionState::fail`])
//! and is reported distinctly by the terminal runtime event.

use crate::runtime::types::RuntimeError;

/// The execution phase of one agent attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionState {
    /// The attempt has not started executing.
    Idle,
    /// The attempt is consuming one canonical model event stream.
    RunningModel,
    /// The attempt is executing the tool calls of the current turn.
    WaitingForTool,
    /// The attempt settled successfully.
    Completed,
    /// The attempt settled by failure or cancellation.
    Failed,
}

impl ExecutionState {
    /// Whether this state is terminal: no execution continues from it.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed)
    }

    /// Whether this state is active: model or tool execution is possible.
    #[must_use]
    pub fn is_active(self) -> bool {
        !self.is_terminal()
    }
}

/// The execution state machine of one attempt.
///
/// Every transition is validated: the loop cannot move out of a terminal
/// state, cannot run tools before the model requested them, and cannot
/// continue the model before the requested tool calls completed. A rejected
/// transition is an explicit [`RuntimeError::InvalidState`], which the loop
/// converts into a terminal failure rather than proceeding.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExecutionStateMachine {
    state: ExecutionState,
}

impl ExecutionStateMachine {
    /// Creates a machine in the [`ExecutionState::Idle`] state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: ExecutionState::Idle,
        }
    }

    /// The current execution state.
    #[must_use]
    pub fn state(&self) -> ExecutionState {
        self.state
    }

    /// Whether the attempt already settled.
    #[must_use]
    pub fn is_terminal(&self) -> bool {
        self.state.is_terminal()
    }

    /// Starts the attempt: [`ExecutionState::Idle`] → [`ExecutionState::RunningModel`].
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::InvalidState`] unless the machine is idle.
    pub fn start(&mut self) -> Result<(), RuntimeError> {
        self.transition(ExecutionState::Idle, ExecutionState::RunningModel)
    }

    /// The model turn ended: [`ExecutionState::RunningModel`] →
    /// [`ExecutionState::WaitingForTool`] when the turn requested tool calls,
    /// otherwise → [`ExecutionState::Completed`].
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::InvalidState`] unless the machine is running
    /// the model.
    pub fn model_finished(&mut self, pending_tools: bool) -> Result<(), RuntimeError> {
        if self.state != ExecutionState::RunningModel {
            return Err(self.invalid_transition(if pending_tools {
                ExecutionState::WaitingForTool
            } else {
                ExecutionState::Completed
            }));
        }
        self.state = if pending_tools {
            ExecutionState::WaitingForTool
        } else {
            ExecutionState::Completed
        };
        Ok(())
    }

    /// All requested tool calls produced results and the model may continue:
    /// [`ExecutionState::WaitingForTool`] → [`ExecutionState::RunningModel`].
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::InvalidState`] unless the machine is waiting
    /// for tools.
    pub fn tools_finished(&mut self) -> Result<(), RuntimeError> {
        self.transition(ExecutionState::WaitingForTool, ExecutionState::RunningModel)
    }

    /// The attempt settled by failure or cancellation: any active state →
    /// [`ExecutionState::Failed`].
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::InvalidState`] if the machine already settled,
    /// which would mean a second terminal outcome.
    pub fn fail(&mut self) -> Result<(), RuntimeError> {
        if !self.state.is_active() {
            return Err(self.invalid_transition(ExecutionState::Failed));
        }
        self.state = ExecutionState::Failed;
        Ok(())
    }

    fn transition(&mut self, from: ExecutionState, to: ExecutionState) -> Result<(), RuntimeError> {
        if self.state != from {
            return Err(self.invalid_transition(to));
        }
        self.state = to;
        Ok(())
    }

    fn invalid_transition(self, to: ExecutionState) -> RuntimeError {
        RuntimeError::InvalidState {
            message: format!(
                "invalid agent execution transition from {} to {}",
                describe(self.state),
                describe(to)
            ),
        }
    }
}

impl Default for ExecutionStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

fn describe(state: ExecutionState) -> &'static str {
    match state {
        ExecutionState::Idle => "idle",
        ExecutionState::RunningModel => "running_model",
        ExecutionState::WaitingForTool => "waiting_for_tool",
        ExecutionState::Completed => "completed",
        ExecutionState::Failed => "failed",
    }
}

#[cfg(test)]
mod tests {
    use super::{ExecutionState, ExecutionStateMachine};
    use crate::runtime::types::RuntimeError;

    /// The initial state is idle and active.
    #[test]
    fn machine_starts_idle() {
        let machine = ExecutionStateMachine::new();
        assert_eq!(machine.state(), ExecutionState::Idle);
        assert!(machine.state().is_active());
        assert!(!machine.state().is_terminal());
    }

    /// The happy path reaches `Completed` and rejects further transitions.
    #[test]
    fn happy_path_completes_and_is_absorbing() {
        let mut machine = ExecutionStateMachine::new();
        machine.start().expect("start");
        assert_eq!(machine.state(), ExecutionState::RunningModel);
        machine.model_finished(false).expect("finish without tools");
        assert_eq!(machine.state(), ExecutionState::Completed);
        assert!(machine.is_terminal());
        assert!(
            machine.model_finished(false).is_err(),
            "a completed machine must reject further model turns"
        );
        assert!(
            machine.fail().is_err(),
            "a completed attempt must never fail afterwards"
        );
        assert!(
            machine.start().is_err(),
            "a terminal machine must never restart"
        );
    }

    /// A tool turn goes through `WaitingForTool` and back into the model.
    #[test]
    fn tool_turn_returns_to_running_model() {
        let mut machine = ExecutionStateMachine::new();
        machine.start().expect("start");
        machine.model_finished(true).expect("finish with tools");
        assert_eq!(machine.state(), ExecutionState::WaitingForTool);
        assert!(
            machine.model_finished(false).is_err(),
            "a second model turn cannot start before the tools completed"
        );
        machine.tools_finished().expect("tools completed");
        assert_eq!(machine.state(), ExecutionState::RunningModel);
        machine.model_finished(false).expect("final turn");
        assert_eq!(machine.state(), ExecutionState::Completed);
    }

    /// Tools cannot run before the model requested them.
    #[test]
    fn tools_cannot_finish_before_the_model_requested_them() {
        let mut machine = ExecutionStateMachine::new();
        assert!(
            machine.tools_finished().is_err(),
            "tools cannot complete while idle"
        );
        machine.start().expect("start");
        assert!(
            machine.tools_finished().is_err(),
            "tools cannot complete while the model is running"
        );
    }

    /// A model turn cannot complete before the attempt started.
    #[test]
    fn model_turn_cannot_complete_before_start() {
        let mut machine = ExecutionStateMachine::new();
        let error = machine.model_finished(false).expect_err("must be rejected");
        assert!(matches!(error, RuntimeError::InvalidState { .. }));
    }

    /// Failure settles from every active state exactly once.
    #[test]
    fn failure_settles_exactly_once_from_any_active_state() {
        for state in [
            ExecutionState::Idle,
            ExecutionState::RunningModel,
            ExecutionState::WaitingForTool,
        ] {
            let mut machine = ExecutionStateMachine::new();
            match state {
                ExecutionState::Idle => {}
                ExecutionState::RunningModel => {
                    machine.start().expect("start");
                }
                ExecutionState::WaitingForTool => {
                    machine.start().expect("start");
                    machine.model_finished(true).expect("finish with tools");
                }
                ExecutionState::Completed | ExecutionState::Failed => unreachable!(),
            }
            machine.fail().expect("fail from active state");
            assert_eq!(machine.state(), ExecutionState::Failed);
            assert!(machine.is_terminal());
            assert!(
                machine.fail().is_err(),
                "a second terminal settlement must be rejected"
            );
        }
    }

    /// The rejected-transition error is a typed invalid-state runtime error.
    #[test]
    fn rejected_transitions_are_typed() {
        let mut machine = ExecutionStateMachine::new();
        let error = machine.tools_finished().expect_err("must be rejected");
        match error {
            RuntimeError::InvalidState { message } => {
                assert!(message.contains("idle"), "message: {message}");
                assert!(message.contains("running_model"), "message: {message}");
            }
            other => panic!("expected InvalidState, got {other:?}"),
        }
    }
}
