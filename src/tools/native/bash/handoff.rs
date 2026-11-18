//! Foreground Bash wait that promotes a still-running command to a background job.
//!
//! The process is staged privately. A command that finishes within the budget
//! returns a normal foreground result: the registry listing never contained a
//! job and no terminal inbound is published. Only a command still running at
//! the budget is adopted into the conversation-owned registry. Cancellation
//! still kills that one process.

use crate::runtime::cancellation::CancellationSignal;
use crate::runtime::types::CancellationReason;
use crate::tools::background::{BackgroundDispatchOutcome, PrivateBashStage};
use crate::tools::executor::ProgressReporter;
use crate::tools::native::support::{cancelled_result, failed_result};
use crate::tools::types::ToolExecutionResult;
use std::time::Duration;

/// Drives one private Bash stage until it finishes, the caller cancels, or the
/// foreground budget expires.
pub(crate) async fn drive_private_bash(
    mut stage: PrivateBashStage,
    budget: Duration,
    cancellation: &CancellationSignal,
    cancel_reason: CancellationReason,
    progress: &dyn ProgressReporter,
) -> ToolExecutionResult {
    let sleep = tokio::time::sleep(budget);
    tokio::pin!(sleep);
    let mut stopping = false;
    loop {
        if let Some(result) = stage.take_result() {
            return stage.release_finished(result);
        }
        tokio::select! {
            biased;
            () = cancellation.cancelled(), if !stopping => {
                stopping = true;
                stage.cancel(cancel_reason);
            }
            () = &mut sleep, if !stopping => {
                return match stage.adopt(cancellation).await {
                    Ok(BackgroundDispatchOutcome::Accepted { result, .. }
                        | BackgroundDispatchOutcome::ForegroundFinished(result)) => result,
                    Ok(BackgroundDispatchOutcome::RolledBack) => cancelled_result(cancel_reason),
                    Err(error) => failed_result(error.to_string()),
                };
            }
            progress_event = stage.wait_event() => {
                if let Some(update) = progress_event {
                    progress.report(update);
                }
            }
        }
    }
}
