//! Declare existing files through ordinary foreground Tool settlement.
use super::registration::{NativeToolRegistration, native_definition};
use super::support::{cancelled_result, failed_result, success_text};
use crate::tools::executor::{
    ToolExecutionContext, ToolExecutionHandle, ToolExecutor, WorkspaceUse,
};
use crate::tools::types::{
    ToolApprovalPolicy, ToolConcurrencyPolicy, ToolDefinition, ToolExecutionPolicy, ToolInvocation,
    ToolInvocationPolicy,
};
use schemars::JsonSchema;
use serde::Deserialize;
use std::collections::BTreeSet;
use std::sync::Arc;

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    /// Ordered list of existing regular files. First normalized duplicate wins.
    #[schemars(length(min = 1, max = 8))]
    files: Vec<Entry>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Entry {
    /// Relative to the native cwd, or absolute inside that cwd. No symlinks or '..'.
    #[schemars(length(min = 1, max = 4096))]
    path: String,
    /// Optional brief human-facing description; preserved verbatim.
    #[schemars(length(max = 512))]
    description: Option<String>,
}
pub(super) fn definition() -> ToolDefinition {
    native_definition::<Input>(
        "tool-present",
        "present",
        "Declare existing files as deliverables for the user. Files may be created by Write, Edit, Bash or any process in the Session cwd. Use for separate file deliverables, not ordinary code edits. Accepts 1–8 files inside the native cwd, in input order; first normalized duplicate wins, including its description. Each successful call is a separate declaration. All entries must be regular files, with no symlink components or parent traversal. Does not copy or mutate bytes: reopening reads current accessible files. Files over 512 KiB may be declared but cannot be previewed/downloaded.",
        ToolInvocationPolicy::new(
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Sequential,
            ToolApprovalPolicy::Never,
        ),
    )
}
pub(super) fn registration() -> NativeToolRegistration {
    NativeToolRegistration::new(definition(), Arc::new(Present::default()))
}
#[derive(Default)]
struct Present {
    #[cfg(test)]
    before_success: Option<Arc<DeclarationGate>>,
}
#[cfg(test)]
struct DeclarationGate {
    arrived: std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
    release: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}
impl ToolExecutor for Present {
    fn workspace_use(&self) -> WorkspaceUse {
        WorkspaceUse::ConsumesProvided
    }
    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        let cancellation = context.cancellation.clone();
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                let input: Input = match super::input::decode("present", &invocation.arguments) {
                    Ok(input) => input,
                    Err(error) => return failed_result(error),
                };
                if input.files.is_empty()
                    || input.files.len() > crate::tools::session_files::PRESENT_MAX_FILES
                {
                    return failed_result("present accepts 1–8 files");
                }
                let root = context.workspace.root().to_owned();
                let conversation = context.conversation_id.clone();
                if context.cancellation.is_cancelled() {
                    return cancelled_result(context.cancellation.reason());
                }
                #[cfg(test)]
                let gate = self.before_success.clone();
                let declarations = tokio::task::spawn_blocking(move || {
                    let mut seen = BTreeSet::new();
                    let mut files = Vec::new();
                    for entry in input.files {
                        if entry
                            .description
                            .as_ref()
                            .is_some_and(|v| v.chars().count() > 512)
                        {
                            return Err("present description exceeds 512 characters".to_owned());
                        }
                        let path = crate::tools::session_files::relative_path(&root, &entry.path)
                            .map_err(|e| e.to_string())?;
                        // Validate every entry, even duplicates; no malformed tail can publish a partial success.
                        let file = crate::tools::session_files::declare(
                            &root,
                            &conversation,
                            &path,
                            entry.description,
                        )
                        .map_err(|e| e.to_string())?;
                        if seen.insert(path) {
                            files.push(file);
                        }
                    }
                    #[cfg(test)]
                    if let Some(gate) = gate {
                        gate.arrived
                            .lock()
                            .unwrap()
                            .take()
                            .unwrap()
                            .send(())
                            .unwrap();
                        gate.release.lock().unwrap().recv().unwrap();
                    }
                    Ok(files)
                })
                .await;
                if context.cancellation.is_cancelled() {
                    return cancelled_result(context.cancellation.reason());
                }
                match declarations {
                    Ok(Ok(files)) => {
                        let mut result =
                            success_text(format!("Declared {} deliverable(s)", files.len()), None);
                        result.deliveries = files;
                        result
                    }
                    Ok(Err(error)) => failed_result(error),
                    Err(_) => failed_result("delivery validation failed"),
                }
            }),
            cancellation,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scripted_suites::common;
    use crate::tools::types::{ToolCall, ToolExecutionStatus};
    async fn run(
        fixture: &common::NativeFixture,
        arguments: serde_json::Value,
        cancelled: bool,
    ) -> crate::tools::types::ToolExecutionResult {
        let call = ToolCall {
            id: crate::runtime::identity::ToolCallId::new("declaration"),
            tool_id: definition().id,
            name: "present".into(),
            arguments,
        };
        let crate::tools::executor::PreflightOutcome::Ready(prepared) =
            fixture.registry.preflight(&call).unwrap()
        else {
            panic!("foreground")
        };
        let signal = crate::runtime::CancellationSignal::new();
        if cancelled {
            signal.cancel();
        }
        let progress = common::NoopProgress;
        let context = ToolExecutionContext::new(
            fixture.runtime.conversation_id(),
            None,
            crate::runtime::ExecutionCancellation::detached(
                signal,
                crate::runtime::types::CancellationReason::UserRequested,
            ),
            fixture.runtime.workspace(),
            &progress,
            fixture.runtime.artifacts(),
            fixture.runtime.tool_output(),
            fixture.runtime.environment(),
        );
        Present::default()
            .start(prepared.invocation, context)
            .completion
            .await
    }
    #[tokio::test]
    async fn order_first_duplicate_description_and_repeated_declarations() {
        let fixture = common::native_fixture();
        std::fs::write(
            fixture.runtime.workspace().root().join("报告 file.md"),
            "# Report",
        )
        .unwrap();
        std::fs::write(
            fixture.runtime.workspace().root().join("second.txt"),
            "second",
        )
        .unwrap();
        let arguments = serde_json::json!({"files":[{"path":"second.txt","description":"first"},{"path":"./second.txt","description":"ignored"},{"path":"报告 file.md"}]});
        for _ in 0..2 {
            let result = run(&fixture, arguments.clone(), false).await;
            assert_eq!(result.status, ToolExecutionStatus::Success);
            assert_eq!(
                result
                    .deliveries
                    .iter()
                    .map(|v| v.path.as_str())
                    .collect::<Vec<_>>(),
                ["second.txt", "报告 file.md"]
            );
            assert_eq!(result.deliveries[0].description.as_deref(), Some("first"));
            assert!(result.artifacts.is_empty());
            assert!(
                result
                    .deliveries
                    .iter()
                    .all(|v| &v.scope.conversation_id == fixture.runtime.conversation_id())
            );
        }
    }
    #[tokio::test]
    async fn missing_tail_cancellation_and_malformed_inputs_publish_no_delivery() {
        let fixture = common::native_fixture();
        std::fs::write(fixture.runtime.workspace().root().join("ok.txt"), "ok").unwrap();
        let failed = run(
            &fixture,
            serde_json::json!({"files":[{"path":"ok.txt"},{"path":"missing"}]}),
            false,
        )
        .await;
        assert!(matches!(failed.status, ToolExecutionStatus::Failed { .. }));
        assert!(failed.deliveries.is_empty());
        let cancelled = run(
            &fixture,
            serde_json::json!({"files":[{"path":"ok.txt"}]}),
            true,
        )
        .await;
        assert!(matches!(
            cancelled.status,
            ToolExecutionStatus::Cancelled { .. }
        ));
        assert!(cancelled.deliveries.is_empty());
        for arguments in [
            serde_json::json!({"files":[]}),
            serde_json::json!({"files":[{"path":"ok.txt","extra":true}]}),
            serde_json::json!({"files":[{"path":4}]}),
            serde_json::json!({"path":"ok.txt"}),
        ] {
            let call = ToolCall {
                id: crate::runtime::identity::ToolCallId::new("bad"),
                tool_id: definition().id,
                name: "present".into(),
                arguments,
            };
            assert!(matches!(
                fixture.registry.preflight(&call),
                Ok(crate::tools::executor::PreflightOutcome::Rejected { .. })
            ));
        }
    }

    #[tokio::test]
    async fn cancellation_after_validation_settles_once_without_deliveries() {
        let fixture = common::native_fixture();
        std::fs::write(
            fixture.runtime.workspace().root().join("ok.txt"),
            b"ORIGINAL",
        )
        .unwrap();
        let call = ToolCall {
            id: crate::runtime::identity::ToolCallId::new("gated-present"),
            tool_id: definition().id,
            name: "present".into(),
            arguments: serde_json::json!({"files":[{"path":"ok.txt"}]}),
        };
        let crate::tools::executor::PreflightOutcome::Ready(prepared) =
            fixture.registry.preflight(&call).unwrap()
        else {
            panic!()
        };
        let (arrived, ready) = tokio::sync::oneshot::channel();
        let (release, gate) = std::sync::mpsc::channel();
        let executor = Present {
            before_success: Some(Arc::new(DeclarationGate {
                arrived: std::sync::Mutex::new(Some(arrived)),
                release: std::sync::Mutex::new(gate),
            })),
        };
        let signal = crate::runtime::CancellationSignal::new();
        let progress = common::NoopProgress;
        let context = ToolExecutionContext::new(
            fixture.runtime.conversation_id(),
            None,
            crate::runtime::ExecutionCancellation::detached(
                signal.clone(),
                crate::runtime::types::CancellationReason::UserRequested,
            ),
            fixture.runtime.workspace(),
            &progress,
            fixture.runtime.artifacts(),
            fixture.runtime.tool_output(),
            fixture.runtime.environment(),
        );
        let mut completion = executor.start(prepared.invocation, context).completion;
        tokio::select! {
            result = &mut completion => panic!("settled before release: {result:?}"),
            result = ready => result.unwrap(),
        }
        signal.cancel();
        release.send(()).unwrap();
        let result = completion.await;
        assert!(matches!(
            result.status,
            ToolExecutionStatus::Cancelled { .. }
        ));
        assert!(result.deliveries.is_empty());
        assert!(result.artifacts.is_empty());
        assert_eq!(
            std::fs::read(fixture.runtime.workspace().root().join("ok.txt")).unwrap(),
            b"ORIGINAL"
        );
    }
}
