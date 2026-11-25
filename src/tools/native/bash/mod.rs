//! Native Agent Bash runs one non-interactive `/bin/bash -c` per invocation
//! under the Session's authorized Execution Client. Server admission and
//! durable ownership, physical START, and canonical result publication are
//! different commit points.
//!
//! The Client runs the shared process-supervisor kernel, with Client-side
//! cwd verification and cancellation/timeout settlement. Linux uses its
//! immutable process group and child-subreaper proof. macOS uses its own
//! process-group wait and does not claim Linux's containment guarantees.
//!
//! Capture decodes stdout and stderr independently before combining them,
//! keeps bounded previews, and reserves one Client-owned file before a job
//! is accepted. A foreground operation without a promotion stage allocates
//! a file only when the combined preview overflows. Read and Grep resolve
//! the continuation through the same Session and Conversation authority.
//! A failed capture can report only a readable prefix or unavailability.
//!
//! Foreground promotion transfers the Server's ownership of the existing
//! registration and capture. It does not restart Bash. Logical cancellation
//! and disconnect do not substitute for confirmed physical settlement.
//!
//! Explicit supervisor diagnostics fixtures exercise the same physical
//! supervisor helpers with instrumentation. The product tool registration
//! has no branch selecting those fixtures.

#[cfg(any(test, feature = "test-support"))]
mod capture;
mod executor;
#[cfg(any(test, feature = "test-support"))]
mod fixture;
pub(crate) mod handoff;
mod input;
mod remote;
#[cfg(test)]
mod tests;

use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::types::ToolInvocationPolicy;

use input::BashInput;

use executor::BashTool;
#[cfg(test)]
pub(crate) use fixture::{BashTestControl, FixtureBashTool};
pub(crate) use remote::{BashCaptureOwner, BashOutputLocator, PreparedBash, operation_at};

/// The canonical model-facing name of the tool.
pub const NAME: &str = "bash";

/// The tool-owned registration of the native Bash tool.
#[must_use]
/// The one JSON object a settled foreground command returns (see the
/// executor's result content).
pub(super) fn output_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "exit_code": {"type": "integer"},
            "stdout": {"type": "string"},
            "stderr": {"type": "string"},
            "combined": {"type": "string"},
        },
        "required": ["exit_code", "stdout", "stderr", "combined"],
        "additionalProperties": false,
    })
}

/// Constructs the explicitly trusted executor used by external regression
/// fixtures. The `BashTool` wiring stays root-owned; the fixture control
/// itself is the supervisor package's physical diagnostics contract.
#[cfg(any(test, feature = "test-support"))]
#[doc(hidden)]
#[must_use]
pub fn fixture_executor(
    control: rustx_process_supervisor::bash_supervisor::diagnostics::FixtureControl,
) -> std::sync::Arc<dyn crate::tools::executor::ToolExecutor> {
    std::sync::Arc::new(fixture::FixtureBashTool::with_diagnostic_fixture(control))
}

pub(super) fn definition(policy: ToolInvocationPolicy) -> crate::tools::types::ToolDefinition {
    native_definition::<BashInput>(
        "tool-bash",
        NAME,
        "Run one non-interactive /bin/bash command inside the workspace. Provide description \
             before command: a concise explanation of why you are running it, shown in the UI. No shell state \
             survives between calls. A foreground command still running after 120 seconds, or after its \
             timeout if set, moves to a background job instead of being killed. The optional timeout is in \
             seconds, overrides that 120 second budget, and is capped at 600 seconds. A background execution \
             immediately returns its live output file's absolute path; use Read or Grep on it to \
             inspect the output while the command runs.",
        policy,
    )
}

pub(super) fn registration(policy: ToolInvocationPolicy) -> NativeToolRegistration {
    NativeToolRegistration::new(definition(policy), std::sync::Arc::new(BashTool))
}
