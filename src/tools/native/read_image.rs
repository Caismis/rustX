//! Read a bounded PNG snapshot into the existing managed artifact owner.
use std::io::Read;
use std::os::unix::fs::OpenOptionsExt;
use std::sync::Arc;

use schemars::JsonSchema;
use serde::Deserialize;

use crate::message::content::ImageReference;
use crate::tools::executor::{ToolExecutionContext, ToolExecutionHandle, ToolExecutor};
use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::native::support::{
    cancelled_result, failed_result, interpret_path, success_text,
};
use crate::tools::types::{
    ToolDefinition, ToolInvocation, ToolInvocationPolicy, ToolResultContent,
};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Input {
    /// PNG file path. Relative paths resolve from the execution cwd.
    #[schemars(length(min = 1))]
    path: String,
}

pub(super) fn definition() -> ToolDefinition {
    use crate::tools::types::{ToolApprovalPolicy, ToolConcurrencyPolicy, ToolExecutionPolicy};
    native_definition::<Input>(
        "tool-read-image",
        "read_image",
        "Read a static PNG image and return the image itself. Maximum encoded size 256 KiB, dimension 4096 pixels, total pixels 4,194,304. Animated PNG and other formats are unsupported. Relative paths resolve from the execution cwd; absolute paths use the host filesystem.",
        ToolInvocationPolicy::new(
            ToolExecutionPolicy::ForegroundOnly,
            ToolConcurrencyPolicy::Parallel,
            ToolApprovalPolicy::Never,
        ),
    )
}
pub(super) fn registration() -> NativeToolRegistration {
    NativeToolRegistration::new(definition(), Arc::new(ReadImage))
}
struct ReadImage;
impl ToolExecutor for ReadImage {
    fn workspace_use(&self) -> crate::tools::executor::WorkspaceUse {
        crate::tools::executor::WorkspaceUse::ConsumesProvided
    }
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        let cancellation = context.cancellation.clone();
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move {
                // Admission authority is supplied by the owning Attempt, never a catalog lookup.
                if !context.model_invocation.is_some_and(|model| {
                    model
                        .capabilities()
                        .input_modalities
                        .contains(&crate::model::catalog::Modality::Image)
                }) {
                    return failed_result(
                        "read_image requires an image-capable admitted invocation",
                    );
                }
                let input: Input = match super::input::decode("read_image", &invocation.arguments) {
                    Ok(input) => input,
                    Err(error) => return failed_result(error),
                };
                if input.path.trim().is_empty() {
                    return failed_result("read_image requires a non-empty path");
                }
                if context.cancellation.is_cancelled() {
                    return cancelled_result(context.cancellation.reason());
                }
                let path = interpret_path(context.workspace.root(), &input.path);
                let read = tokio::task::spawn_blocking(move || {
                    if path
                        .extension()
                        .is_some_and(|ext| !ext.eq_ignore_ascii_case("png"))
                    {
                        return Err("read_image supports only PNG files".to_owned());
                    }
                    let mut file = std::fs::File::options()
                        .read(true)
                        .custom_flags(libc::O_NONBLOCK)
                        .open(path)
                        .map_err(|_| "image path is unreadable".to_owned())?;
                    if !file
                        .metadata()
                        .map_err(|_| "image metadata is unavailable".to_owned())?
                        .is_file()
                    {
                        return Err("image path is not a regular file".to_owned());
                    }
                    let mut bytes = Vec::new();
                    Read::by_ref(&mut file)
                        .take((crate::tools::artifacts::ARTIFACT_TRANSFER_MAX + 1) as u64)
                        .read_to_end(&mut bytes)
                        .map_err(|_| "image read failed".to_owned())?;
                    crate::model::images::validate_png(&bytes)?;
                    Ok(bytes)
                })
                .await;
                if context.cancellation.is_cancelled() {
                    return cancelled_result(context.cancellation.reason());
                }
                let bytes = match read {
                    Ok(Ok(bytes)) => bytes,
                    Ok(Err(error)) => return failed_result(error),
                    Err(_) => return failed_result("image validation failed"),
                };
                let Ok(artifact_id) = context.artifacts.put_bounded(&bytes) else {
                    return failed_result("image artifact storage failed");
                };
                if context.cancellation.is_cancelled() {
                    // A reserved artifact remains owned by normal conversation retention;
                    // cancellation never publishes a successful image reference.
                    return cancelled_result(context.cancellation.reason());
                }
                let mut result = success_text("Read static PNG image", None);
                result
                    .content
                    .push(ToolResultContent::Image(ImageReference {
                        artifact_id,
                        alt: None,
                    }));
                result
            }),
            cancellation,
        )
    }
    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::catalog::{Modality, ModelCapabilities, ModelRef};
    use crate::model::invocation::{ModelSelection, effective_capabilities};
    use crate::model::types::ModelProtocol;
    use crate::scripted_suites::{common, support::model};
    use crate::tools::executor::PreflightOutcome;
    use crate::tools::types::{ToolCall, ToolExecutionStatus};

    fn invocation(image: bool) -> crate::model::invocation::ResolvedModelInvocation {
        let mut definition =
            model::FixtureModel::text("fixture/image", ModelProtocol::AnthropicMessages);
        if image {
            definition.extra_input_modalities.push("image");
        }
        model::fixture_registry(
            &[definition],
            &model::ScriptedAdapterFactory::new(Arc::new(model::NullAdapter)),
        )
        .resolve(&ModelSelection::of(
            ModelRef::parse("fixture/image").unwrap(),
        ))
        .unwrap()
    }

    async fn run(
        fixture: &common::NativeFixture,
        model: &crate::model::invocation::ResolvedModelInvocation,
        path: &str,
        cancelled: bool,
    ) -> crate::tools::types::ToolExecutionResult {
        let call = ToolCall {
            id: crate::runtime::identity::ToolCallId::new("image"),
            tool_id: definition().id,
            name: "read_image".into(),
            arguments: serde_json::json!({"path":path}),
        };
        let PreflightOutcome::Ready(prepared) = fixture.registry.preflight(&call).unwrap() else {
            panic!("foreground")
        };
        let signal = crate::runtime::CancellationSignal::new();
        if cancelled {
            signal.cancel();
        }
        let progress = common::NoopProgress;
        let mut context = ToolExecutionContext::new(
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
        context.model_invocation = Some(model);
        ReadImage
            .start(prepared.invocation, context)
            .completion
            .await
    }

    #[test]
    fn catalog_requires_effective_image_and_selected_implementation() {
        let fixture = common::native_fixture();
        let mut declared = ModelCapabilities::text_only(true, true);
        declared.input_modalities.insert(Modality::Image);
        for protocol in [
            ModelProtocol::AnthropicMessages,
            ModelProtocol::OpenAiChatCompletions,
            ModelProtocol::OpenAiResponses,
        ] {
            let caps = effective_capabilities(&declared, protocol);
            assert_eq!(
                fixture
                    .registry
                    .for_model(&caps)
                    .definitions()
                    .iter()
                    .any(|tool| tool.name == "read_image"),
                protocol == ModelProtocol::AnthropicMessages
            );
        }
        let text = ModelCapabilities::text_only(true, true);
        assert!(
            !fixture
                .registry
                .for_model(&text)
                .definitions()
                .iter()
                .any(|tool| tool.name == "read_image")
        );
        assert!(
            crate::tools::executor::ToolRegistry::new()
                .for_model(&declared)
                .definitions()
                .is_empty(),
            "absent or disabled implementation is never synthesized"
        );
        assert!(
            !fixture
                .registry
                .for_model(&declared.intersect(&text))
                .definitions()
                .iter()
                .any(|tool| tool.name == "read_image"),
            "unavailable runtime image path narrows the admitted intersection"
        );
    }

    #[tokio::test]
    async fn native_validation_cancellation_storage_and_immutable_snapshot() {
        let fixture = common::native_fixture();
        let image = invocation(true);
        let text = invocation(false);
        let root = fixture.runtime.workspace().root();
        let mut bytes = Vec::new();
        {
            let encoder = png::Encoder::new(&mut bytes, 1, 1);
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[128]).unwrap();
        }
        std::fs::write(root.join("image.png"), &bytes).unwrap();
        for (path, data) in [
            ("mismatch.png", b"GIF89a".as_slice()),
            ("empty.png", b""),
            ("truncated.png", &bytes[..40]),
            ("wrong.jpg", &bytes),
        ] {
            std::fs::write(root.join(path), data).unwrap();
            assert!(matches!(
                run(&fixture, &image, path, false).await.status,
                ToolExecutionStatus::Failed { .. }
            ));
        }
        std::fs::create_dir(root.join("directory.png")).unwrap();
        for path in ["missing.png", "directory.png"] {
            assert!(matches!(
                run(&fixture, &image, path, false).await.status,
                ToolExecutionStatus::Failed { .. }
            ));
        }
        assert!(matches!(
            run(&fixture, &text, "missing.png", false).await.status,
            ToolExecutionStatus::Failed { .. }
        ));
        assert_ne!(
            run(&fixture, &image, "image.png", true).await.status,
            ToolExecutionStatus::Success
        );
        assert_eq!(
            std::fs::read_dir(fixture.runtime.artifacts().root())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("artifact_"))
                .count(),
            0
        );
        let result = run(&fixture, &image, "image.png", false).await;
        assert_eq!(result.status, ToolExecutionStatus::Success);
        let ToolResultContent::Image(reference) = &result.content[1] else {
            panic!("canonical image")
        };
        std::fs::remove_file(root.join("image.png")).unwrap();
        assert_eq!(
            fixture
                .runtime
                .artifacts()
                .read_bounded(&reference.artifact_id)
                .unwrap(),
            bytes
        );
        std::fs::write(root.join("image.png"), &bytes).unwrap();
        // Exhaust the owner's finite capacity without destroying its live SQLite/output roots.
        while fixture.runtime.artifacts().create_artifact().is_ok() {}
        let failed = run(&fixture, &image, "image.png", false).await;
        assert!(matches!(failed.status, ToolExecutionStatus::Failed { .. }));
        assert!(
            failed
                .content
                .iter()
                .all(|block| !matches!(block, ToolResultContent::Image(_)))
        );
    }
}
