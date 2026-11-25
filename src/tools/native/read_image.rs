//! Read a bounded PNG snapshot into the existing managed artifact owner.
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
    /// PNG file path. Relative paths resolve from the execution cwd; absolute
    /// paths must stay within the Session's admitted Client workspace.
    #[schemars(length(min = 1))]
    path: String,
}

pub(super) fn definition() -> ToolDefinition {
    use crate::tools::types::ToolExecutionPolicy;
    native_definition::<Input>(
        "tool-read-image",
        "read_image",
        "Read a static PNG image and return the image itself. Maximum encoded size 256 KiB, dimension 4096 pixels, total pixels 4,194,304. Animated PNG and other formats are unsupported. Relative paths resolve from the execution cwd; absolute paths must stay within the Session's admitted Client workspace.",
        ToolInvocationPolicy::new(ToolExecutionPolicy::ForegroundOnly),
    )
}
pub(super) fn registration() -> NativeToolRegistration {
    NativeToolRegistration::new(definition(), Arc::new(ReadImage))
}
struct ReadImage;
impl ToolExecutor for ReadImage {
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        Self::start_inner(
            invocation,
            context,
            #[cfg(test)]
            || {},
            #[cfg(test)]
            || {},
        )
    }
    fn progress_capability(&self) -> crate::tools::deadline::ToolProgressCapability {
        crate::tools::deadline::ToolProgressCapability::None
    }
}

impl ReadImage {
    fn start_inner(
        invocation: ToolInvocation,
        context: ToolExecutionContext<'_>,
        #[cfg(test)] before_commit: impl FnOnce() + Send + 'static,
        #[cfg(test)] after_commit: impl FnOnce() + Send + 'static,
    ) -> ToolExecutionHandle<'_> {
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
                if path
                    .extension()
                    .is_some_and(|ext| !ext.eq_ignore_ascii_case("png"))
                {
                    return failed_result("read_image supports only PNG files");
                }
                let Some(scope) = context.workspace.execution_scope().cloned() else {
                    return failed_result(
                        "Session execution unavailable: no admitted Execution Client",
                    );
                };
                // The physical bytes cross the Session's admitted
                // Execution Client boundary in bounded range chunks; the
                // PNG constraint itself is validated Agent-side on the
                // assembled bytes, off the async reactor.
                let read = scope.image_bytes(&path, &context.cancellation).await;
                let read = match read {
                    Ok(bytes) => {
                        match tokio::task::spawn_blocking(move || {
                            crate::model::images::validate_png(&bytes).map(|()| bytes)
                        })
                        .await
                        {
                            Ok(Ok(bytes)) => Ok(bytes),
                            Ok(Err(error)) => Err(error),
                            Err(_) => Err("image validation failed".to_owned()),
                        }
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                        return cancelled_result(context.cancellation.reason());
                    }
                    Err(error) => {
                        return failed_result(format!("cannot read {}: {error}", path.display()));
                    }
                };
                #[cfg(test)]
                before_commit();
                if context.cancellation.is_cancelled() {
                    return cancelled_result(context.cancellation.reason());
                }
                let bytes = match read {
                    Ok(bytes) => bytes,
                    Err(error) => return failed_result(error),
                };
                // The durable store is atomic with the cancellation
                // grant: `with_active` admits the commit on the physical
                // writer thread or it never happens.
                let artifacts = context.artifacts.clone();
                let commit_cancellation = context.cancellation.clone();
                let committed = tokio::task::spawn_blocking(move || {
                    commit_cancellation.with_active(|| artifacts.put_bounded(&bytes))
                })
                .await;
                #[cfg(test)]
                after_commit();
                if context.cancellation.is_cancelled() {
                    // A reserved artifact remains owned by normal conversation retention;
                    // cancellation never publishes a successful image reference.
                    return cancelled_result(context.cancellation.reason());
                }
                let artifact_id = match committed {
                    Ok(Some(Ok(id))) => id,
                    Ok(Some(Err(_))) | Err(_) => {
                        return failed_result("image artifact storage failed");
                    }
                    Ok(None) => return cancelled_result(context.cancellation.reason()),
                };
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
}

#[cfg(test)]
mod tests {
    use std::io::Read as _;

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
        run_at_commit(fixture, model, path, cancelled, false, false).await
    }

    async fn run_at_commit(
        fixture: &common::NativeFixture,
        model: &crate::model::invocation::ResolvedModelInvocation,
        path: &str,
        cancelled: bool,
        cancel_before_commit: bool,
        cancel_after_commit: bool,
    ) -> crate::tools::types::ToolExecutionResult {
        let call = ToolCall {
            id: crate::runtime::identity::ToolCallId::new("image"),
            tool_id: definition().id,
            name: "read_image".into(),
            arguments: crate::scripted_suites::common::tool_call_envelope(
                serde_json::json!({"path":path}),
            ),
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
                signal.clone(),
                crate::runtime::types::CancellationReason::UserRequested,
            ),
            fixture.runtime.workspace(),
            &progress,
            fixture.runtime.artifacts(),
            fixture.runtime.tool_output(),
            fixture.runtime.environment(),
        );
        context.model_invocation = Some(model);
        let before_signal = signal.clone();
        let artifacts = fixture.runtime.artifacts().clone();
        ReadImage::start_inner(
            prepared.invocation,
            context,
            move || {
                if cancel_before_commit {
                    before_signal.cancel();
                }
            },
            move || {
                // put_bounded returned: the durable snapshot is already
                // readable through the settled-writer admission boundary.
                let lengths = crate::tools::artifacts::ArtifactStore::archive_lengths(
                    artifacts.root(),
                    || Ok(()),
                )
                .unwrap();
                assert!(!lengths.is_empty());
                assert!(lengths.values().all(Option::is_some));
                if cancel_after_commit {
                    signal.cancel();
                }
            },
        )
        .completion
        .await
    }

    #[test]
    fn catalog_requires_effective_image_and_selected_implementation() {
        let fixture = common::native_fixture();
        let mut declared = ModelCapabilities::text_only();
        declared.input_modalities.insert(Modality::Image);
        for protocol in [
            ModelProtocol::AnthropicMessages,
            ModelProtocol::OpenAiChatCompletions,
            ModelProtocol::OpenAiResponses,
        ] {
            let caps = effective_capabilities(&declared, protocol);
            assert!(
                fixture
                    .registry
                    .for_model(&caps)
                    .definitions()
                    .iter()
                    .any(|tool| tool.name == "read_image")
            );
            let text = effective_capabilities(&ModelCapabilities::text_only(), protocol);
            assert!(
                !fixture
                    .registry
                    .for_model(&text)
                    .definitions()
                    .iter()
                    .any(|tool| tool.name == "read_image")
            );
        }
        let text = ModelCapabilities::text_only();
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
    async fn image_cancellation_on_each_side_of_artifact_commit_is_deterministic() {
        use crate::runtime::identity::ArtifactId;
        use crate::tools::artifacts::ArtifactStore;

        for after_commit in [false, true] {
            let fixture = common::native_fixture_with_execution(Vec::new());
            let model = invocation(true);
            let mut bytes = Vec::new();
            {
                let encoder = png::Encoder::new(&mut bytes, 1, 1);
                encoder
                    .write_header()
                    .unwrap()
                    .write_image_data(&[128])
                    .unwrap();
            }
            std::fs::write(fixture.runtime.workspace().root().join("image.png"), &bytes).unwrap();
            let result = run_at_commit(
                &fixture,
                &model,
                "image.png",
                false,
                !after_commit,
                after_commit,
            )
            .await;
            assert!(matches!(
                result.status,
                ToolExecutionStatus::Cancelled { .. }
            ));
            assert!(
                result
                    .content
                    .iter()
                    .all(|block| !matches!(block, ToolResultContent::Image(_)))
            );
            let reopened = ArtifactStore::new(
                fixture.runtime.conversation_id().clone(),
                fixture.runtime.artifacts().root(),
            )
            .unwrap();
            let id = ArtifactId::new("artifact_1");
            if after_commit {
                assert_eq!(reopened.read_bounded(&id).unwrap(), bytes);
                assert_eq!(reopened.create_artifact().unwrap().as_str(), "artifact_2");
                let mut reader = ArtifactStore::open_archive_reader(reopened.root(), &id).unwrap();
                let mut restored = Vec::new();
                reader.read_to_end(&mut restored).unwrap();
                assert_eq!(
                    restored, bytes,
                    "committed cancellation residue is settled and replayable"
                );
            } else {
                assert!(reopened.read_bounded(&id).is_err());
                assert_eq!(
                    reopened.create_artifact().unwrap(),
                    id,
                    "no reservation before commit"
                );
            }
        }
    }

    /// A Session with no admitted Execution Client has no Server-local
    /// fallback: even a physically valid workspace PNG is refused before
    /// any artifact is allocated, and the refusal leaks no file bytes.
    #[tokio::test]
    async fn an_unbound_session_refuses_image_reads_without_touching_artifacts() {
        let fixture = common::native_fixture();
        let model = invocation(true);
        let mut bytes = Vec::new();
        {
            let encoder = png::Encoder::new(&mut bytes, 1, 1);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&[128])
                .unwrap();
        }
        std::fs::write(fixture.runtime.workspace().root().join("image.png"), &bytes).unwrap();
        let result = run(&fixture, &model, "image.png", false).await;
        let ToolExecutionStatus::Failed { error } = &result.status else {
            panic!("an unbound Session must refuse image reads: {result:?}");
        };
        assert!(
            error.contains("Session execution unavailable"),
            "honest unavailable, got: {error}"
        );
        assert!(
            result
                .content
                .iter()
                .all(|block| !matches!(block, ToolResultContent::Image(_))),
            "no image block leaks through the unavailable route"
        );
        assert_eq!(
            std::fs::read_dir(fixture.runtime.artifacts().root())
                .unwrap()
                .filter_map(Result::ok)
                .filter(|entry| entry.file_name().to_string_lossy().starts_with("artifact_"))
                .count(),
            0,
            "a refused read allocates no artifact"
        );
    }

    /// A valid PNG larger than one 64 KiB wire chunk is assembled from
    /// bounded `ReadRange` exchanges on the real admitted Client and
    /// committed byte-exact; the immutable snapshot outlives the source and Client.
    #[tokio::test]
    async fn a_multichunk_png_commits_exact_artifact_bytes() {
        let mut fixture = common::native_fixture_with_execution(Vec::new());
        let model = invocation(true);
        let root = fixture.runtime.workspace().root();
        let mut bytes = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut bytes, 384, 384);
            encoder.set_color(png::ColorType::Grayscale);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(png::Compression::NoCompression);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&vec![0xAB; 384 * 384])
                .unwrap();
        }
        assert!(
            bytes.len() > rustx_execution_protocol::MAX_RAW_CHUNK_BYTES,
            "the fixture image spans multiple wire chunks"
        );
        assert!(bytes.len() <= crate::tools::artifacts::ARTIFACT_TRANSFER_MAX);
        std::fs::write(root.join("large.png"), &bytes).unwrap();
        let result = run(&fixture, &model, "large.png", false).await;
        assert_eq!(result.status, ToolExecutionStatus::Success);
        let ToolResultContent::Image(reference) = &result.content[1] else {
            panic!("canonical image")
        };
        std::fs::write(root.join("large.png"), b"source replaced after commit").unwrap();
        assert_eq!(
            fixture
                .runtime
                .artifacts()
                .read_bounded(&reference.artifact_id)
                .unwrap(),
            bytes
        );
        std::fs::remove_file(root.join("large.png")).unwrap();
        assert_eq!(
            fixture
                .runtime
                .artifacts()
                .read_bounded(&reference.artifact_id)
                .unwrap(),
            bytes,
            "the committed artifact is immutable once the source is gone"
        );
        fixture.disconnect_execution();
        assert_eq!(
            fixture
                .runtime
                .artifacts()
                .read_bounded(&reference.artifact_id)
                .unwrap(),
            bytes,
            "the committed artifact is independent of the reaped Client"
        );
    }

    #[tokio::test]
    async fn native_validation_cancellation_storage_and_immutable_snapshot() {
        let fixture = common::native_fixture_with_execution(Vec::new());
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
        std::fs::write(root.join("image.png"), b"source replaced after commit").unwrap();
        assert_eq!(
            fixture
                .runtime
                .artifacts()
                .read_bounded(&reference.artifact_id)
                .unwrap(),
            bytes
        );
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
