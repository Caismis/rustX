//! The task-list fixture of the in-crate suites.
//!
//! Every other native tool can be driven through [`common::run_tool`], which
//! hands an executor the ordinary conversation resources. `todo` cannot: a
//! mutation belongs to the `ToolResult` batch that publishes it, so the
//! executor writes through the batch-scoped authority the Agent Loop opens
//! and nothing else. That authority is crate-private, because settling a
//! batch asserts that canonical history already carries the list being
//! installed and only the loop is in a position to assert it.
//!
//! So a suite that drives the `todo` executor directly has to stand in for
//! the loop, and does it here, explicitly: this fixture opens one batch,
//! hands every call that batch's writer, and lets a suite end the batch the
//! way every non-commit exit of the loop ends one. It lives in `support`
//! rather than in [`common`] because it reaches a seam no integration-test
//! binary — and no consumer of the library — can reach.

use super::super::common::{self, NativeFixture};

use rustx::tools::todo::{TodoBatch, TodoSnapshot};
use rustx::tools::types::ToolExecutionResult;

/// Whether anything provisional is outstanding on `fixture`'s list.
///
/// The one fact about the list that is not published API: "nothing was left
/// behind" is a property of the transaction design rather than something the
/// runtime acts on, so no consumer needs to read it and only a suite does.
pub(crate) fn has_staged(fixture: &NativeFixture) -> bool {
    fixture.runtime.todos().has_staged()
}

/// One native tool fixture with one open `ToolResult` batch over its list.
pub(crate) struct TodoPlane {
    /// The underlying native fixture, for the assertions that are about the
    /// registry rather than about the list.
    pub(crate) fixture: NativeFixture,
    /// The batch every call below belongs to. `None` after [`Self::abandon`].
    batch: Option<TodoBatch>,
}

impl TodoPlane {
    /// A fresh conversation with one batch open over its empty list.
    pub(crate) fn open() -> Self {
        let fixture = common::native_fixture();
        let batch = fixture
            .runtime
            .todos()
            .open_batch()
            .expect("a fresh list opens one batch");
        Self {
            fixture,
            batch: Some(batch),
        }
    }

    /// Runs one `todo` call as a member of this batch.
    pub(crate) async fn run(&self, arguments: serde_json::Value) -> ToolExecutionResult {
        use rustx::runtime::identity::ToolCallId;
        use rustx::tools::executor::{PreflightOutcome, ToolExecutionContext};
        use rustx::tools::types::ToolCall;

        let definition = self
            .fixture
            .registry
            .definitions()
            .into_iter()
            .find(|definition| definition.name == "todo")
            .expect("todo is registered");
        let call = ToolCall {
            id: ToolCallId::new("call-todo"),
            tool_id: definition.id,
            name: "todo".to_owned(),
            arguments,
        };
        let PreflightOutcome::Ready(prepared) =
            self.fixture.registry.preflight(&call).expect("preflight")
        else {
            panic!("direct todo calls preflight as ready");
        };
        let executor = self.fixture.registry.executor(&prepared.invocation.tool_id);
        let reporter = common::NoopProgress;
        let context = ToolExecutionContext::new(
            self.fixture.runtime.conversation_id(),
            None,
            rustx::runtime::ExecutionCancellation::detached(
                rustx::runtime::CancellationSignal::new(),
                rustx::runtime::types::CancellationReason::UserRequested,
            ),
            self.fixture.runtime.workspace(),
            &reporter,
            self.fixture.runtime.artifacts(),
            self.fixture.runtime.tool_output(),
            self.fixture.runtime.environment(),
        );
        let context = match &self.batch {
            Some(batch) => context.with_todos(batch.writer()),
            None => context,
        };
        executor.execute(prepared.invocation, context).await
    }

    /// Ends the batch without committing anything, exactly as every
    /// non-commit exit of the Agent Loop does. Later calls belong to no
    /// batch.
    pub(crate) fn abandon(&mut self) {
        self.batch = None;
    }

    /// The authoritative list: what canonical history committed.
    pub(crate) fn committed(&self) -> TodoSnapshot {
        self.fixture.runtime.todo_snapshot()
    }

    /// The list this batch is building on top of the authority.
    pub(crate) fn working(&self) -> TodoSnapshot {
        self.fixture.runtime.todos().snapshot()
    }

    /// Whether anything provisional is outstanding.
    pub(crate) fn has_staged(&self) -> bool {
        self.fixture.runtime.todos().has_staged()
    }
}
