//! The explicit execution state machine of one agent attempt.
//!
//! M3 execution semantics are expressed as a state machine instead of
//! ad-hoc async control flow. The states are:
//!
//! ```text
//! Idle
//!   ↓ start()
//! RunningModel
//!   ↓ model_finished(true)
//! WaitingForTool ──tools_finished()──▶ RunningModel ──model_finished──▶ ...
//! RunningModel
//!   ↓ complete()
//! Completed        (immediately before the terminal event append)
//!
//! any active state
//!   ↓ fail()
//! Failed           (failure and cancellation settle here)
//! ```
//!
//! The machine is the settlement authority: the loop settles the machine
//! (`complete()` or `fail()`) immediately before attempting the attempt
//! terminal `RuntimeEvent` append, so a successful append and the terminal
//! state represent the same settlement boundary. A terminal state can still
//! exist without a terminal Journal fact when that required append fails; the
//! typed execution result reports that condition. A terminal state is
//! absorbing: no transition leaves it, so execution facts cannot be
//! produced after the attempt settled. Cancellation terminates through the
//! failure path ([`ExecutionStateMachine::fail`]) and is reported
//! distinctly by the terminal runtime event.

use crate::runtime::types::RuntimeError;

/// The execution phase of one agent attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionState {
    /// The attempt has not started executing.
    Idle,
    /// The attempt is consuming one provider-independent model stream.
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

    /// The model turn ended.
    ///
    /// With pending tool calls the machine moves
    /// [`ExecutionState::RunningModel`] → [`ExecutionState::WaitingForTool`].
    /// Without tool calls the machine stays in [`ExecutionState::RunningModel`]:
    /// the attempt is not settled until [`ExecutionStateMachine::complete`]
    /// runs immediately before the terminal event append, so the machine
    /// never reports `Completed` while non-terminal execution facts (message
    /// commit, turn completion) are still being produced.
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
                ExecutionState::RunningModel
            }));
        }
        if pending_tools {
            self.state = ExecutionState::WaitingForTool;
        }
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

    /// Settles the attempt successfully: [`ExecutionState::RunningModel`] →
    /// [`ExecutionState::Completed`]. This is the only successful settlement
    /// transition and must run immediately before the attempt terminal
    /// event is attempted. A failed durable append leaves the machine
    /// terminal without inventing an event in the local or observer
    /// projections.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::InvalidState`] unless the machine is running
    /// the model, which would mean the attempt settled twice or settled
    /// from a phase that cannot complete.
    pub fn complete(&mut self) -> Result<(), RuntimeError> {
        self.transition(ExecutionState::RunningModel, ExecutionState::Completed)
    }

    /// Settles the attempt by failure or cancellation: any active state →
    /// [`ExecutionState::Failed`]. Cancellation terminates through this
    /// failure path; the terminal runtime event reports the distinct
    /// cancellation reason.
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

    /// The happy path stays non-terminal after the model finishes and
    /// settles only through `complete`, which is then absorbing.
    #[test]
    fn happy_path_settles_only_through_complete() {
        let mut machine = ExecutionStateMachine::new();
        machine.start().expect("start");
        assert_eq!(machine.state(), ExecutionState::RunningModel);
        machine.model_finished(false).expect("finish without tools");
        assert_eq!(
            machine.state(),
            ExecutionState::RunningModel,
            "a turn without tools must not settle the machine yet"
        );
        assert!(
            !machine.is_terminal(),
            "non-terminal turn facts are still being produced"
        );
        machine.complete().expect("successful settlement");
        assert_eq!(machine.state(), ExecutionState::Completed);
        assert!(machine.is_terminal());
        assert!(
            machine.complete().is_err(),
            "successful settlement must happen exactly once"
        );
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

    /// A tool turn goes through `WaitingForTool` and back into the model;
    /// only `complete` settles the final turn.
    #[test]
    fn tool_turn_returns_to_running_model() {
        let mut machine = ExecutionStateMachine::new();
        machine.start().expect("start");
        machine.model_finished(true).expect("finish with tools");
        assert_eq!(machine.state(), ExecutionState::WaitingForTool);
        assert!(
            machine.model_finished(true).is_err(),
            "a second model turn cannot start before the tools completed"
        );
        machine.tools_finished().expect("tools completed");
        assert_eq!(machine.state(), ExecutionState::RunningModel);
        machine.model_finished(false).expect("final turn");
        assert_eq!(
            machine.state(),
            ExecutionState::RunningModel,
            "the final turn still does not settle the machine"
        );
        machine.complete().expect("settle");
        assert_eq!(machine.state(), ExecutionState::Completed);
    }

    /// `complete` requires the model phase: it cannot settle from `Idle` or
    /// `WaitingForTool`.
    #[test]
    fn complete_requires_running_model() {
        let mut idle = ExecutionStateMachine::new();
        assert!(idle.complete().is_err(), "an idle machine cannot complete");
        let mut waiting = ExecutionStateMachine::new();
        waiting.start().expect("start");
        waiting.model_finished(true).expect("finish with tools");
        assert!(
            waiting.complete().is_err(),
            "a machine waiting for tools cannot complete"
        );
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

    /// A model turn cannot finish before the attempt started.
    #[test]
    fn model_turn_cannot_finish_before_start() {
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
            assert!(
                machine.complete().is_err(),
                "a failed attempt must never complete afterwards"
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
