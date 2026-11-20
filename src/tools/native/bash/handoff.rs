//! Foreground Bash wait that promotes a still-running command to a background job.
//!
//! The process is staged privately. A command that finishes within the budget
//! returns a normal foreground result: the registry listing never contained a
//! job and no terminal inbound is published. Only a command still running at
//! the budget is adopted into the conversation-owned registry. Cancellation
//! still kills that one process.

use crate::runtime::cancellation::ExecutionCancellation;
use crate::runtime::types::CancellationReason;
use crate::tools::background::{BackgroundDispatchOutcome, PrivateBashStage};
use crate::tools::executor::ProgressReporter;
use crate::tools::invocation::ForegroundHandoff;
use crate::tools::types::ToolExecutionResult;

/// Drives one private Bash stage until it finishes, the caller cancels, or the
/// foreground budget expires.
pub(crate) async fn drive_private_bash(
    mut stage: PrivateBashStage,
    handoff: &ForegroundHandoff,
    cancellation: &ExecutionCancellation,
    progress: &dyn ProgressReporter,
) -> ToolExecutionResult {
    let sleep = tokio::time::sleep(handoff.budget);
    tokio::pin!(sleep);
    let flush_progress = |stage: &mut PrivateBashStage| {
        while let Some(update) = stage.take_progress() {
            progress.report(update);
        }
    };
    let mut stopping = false;
    loop {
        if let Some(result) = stage.take_result() {
            flush_progress(&mut stage);
            return stage.release_finished(result);
        }
        tokio::select! {
            biased;
            () = cancellation.cancelled(), if !stopping => {
                stopping = true;
                stage.cancel(cancellation.reason());
            }
            () = &mut sleep, if !stopping => {
                match stage.adopt(cancellation.signal(), handoff.budget).await {
                    Ok(BackgroundDispatchOutcome::Accepted { result, .. }) => {
                        handoff.record_transfer();
                        flush_progress(&mut stage);
                        return result;
                    }
                    Ok(BackgroundDispatchOutcome::ForegroundFinished(result)) => {
                        flush_progress(&mut stage);
                        return result;
                    }
                    Ok(BackgroundDispatchOutcome::RolledBack) | Err(_) => {
                        // Ownership is still here. Ask the process to stop,
                        // then take its actual settlement and output.
                        stopping = true;
                        stage.cancel(if cancellation.is_cancelled() {
                            cancellation.reason()
                        } else {
                            CancellationReason::RuntimeShutdown
                        });
                    }
                }
            }
            progress_event = stage.wait_event() => {
                if let Some(update) = progress_event {
                    progress.report(update);
                }
            }
        }
    }
}
