//! Shared tool-runtime and execution-context fixtures for the adapter's
//! in-module unit tests.

use crate::runtime::identity::{ConversationId, ToolExecutionId};
use crate::runtime::types::CancellationReason;
use crate::runtime::{CancellationSignal, ExecutionCancellation};
use crate::tools::executor::{ProgressReporter, ToolExecutionContext};

pub(super) struct NoProgress;

impl ProgressReporter for NoProgress {
    fn report(&self, _progress: crate::tools::types::ToolProgress) {}
}

pub(super) fn runtime(
    name: &str,
) -> (
    tempfile::TempDir,
    crate::tools::runtime::ConversationToolRuntime,
) {
    let directory = tempfile::tempdir().expect("runtime directory");
    let root = directory.path().to_path_buf();
    std::fs::create_dir_all(root.join("workspace")).expect("workspace");
    let runtime = crate::tools::runtime::ConversationToolRuntime::new(
        ConversationId::new(name),
        root.join("workspace"),
        root.join("artifacts"),
    )
    .expect("tool runtime");
    (directory, runtime)
}

pub(super) fn context<'a>(
    runtime: &'a crate::tools::runtime::ConversationToolRuntime,
    execution_id: Option<&'a ToolExecutionId>,
    progress: &'a NoProgress,
) -> ToolExecutionContext<'a> {
    ToolExecutionContext::new(
        runtime.conversation_id(),
        execution_id,
        ExecutionCancellation::detached(
            CancellationSignal::new(),
            CancellationReason::UserRequested,
        ),
        runtime.workspace(),
        progress,
        runtime.artifacts(),
        runtime.tool_output(),
        runtime.environment(),
    )
}
