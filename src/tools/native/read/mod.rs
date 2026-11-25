//! Native Read tool.
//!
//! Read is a sequential, pageable source. It resolves relative paths against
//! the execution cwd, returns a contiguous head from the requested offset,
//! and owns a complete-line 2000-line/50KB projection. The runtime's generic
//! 64KB safety bound remains a last-resort boundary for other tools.
//!
//! Ordinary targets are decoded as faithful UTF-8 text. A closed rustX-owned
//! whitelist of structured documents (`.pdf`, `.docx`, `.xlsx`, `.pptx`) is
//! instead decoded through the parser-only xberg backend into deterministic
//! Markdown (see [`document`]). Both paths converge into the same source →
//! logical-text boundary: after classification, one shared projection owns
//! line addressing, slicing, the 2000-line/50KB bounds, continuation
//! diagnostics, and result semantics.

#[cfg(test)]
mod document;
#[cfg(test)]
pub(crate) use document::read_hooks;
mod input;
#[cfg(test)]
mod testdata;

use std::fmt::Write as _;

use crate::tools::deadline::ToolProgressCapability;
use crate::tools::executor::{ToolExecutionContext, ToolExecutionHandle, ToolExecutor};
use crate::tools::limits::NATIVE_FILE_TOOL_MAX_BYTES;
use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::native::support::{
    cancelled_result, failed_result, interpret_path, success_text,
};
use crate::tools::types::ToolInvocationPolicy;
use crate::tools::types::{ToolExecutionResult, ToolInvocation, TruncationState};

use input::ReadInput;

// Compaction file-operation metadata decodes the path of a historical
// canonical Read call through this tool-owned boundary (Issue #140).
pub(super) use input::operation_path;

/// The canonical model-facing name of the tool.
pub const NAME: &str = "read";
/// The canonical identity of the rustX native Read capability.
pub(crate) const TOOL_ID: &str = "tool-read";

/// The tool-owned registration of the native Read tool.
#[must_use]
pub(super) fn definition(policy: ToolInvocationPolicy) -> crate::tools::types::ToolDefinition {
    native_definition::<ReadInput>(
        TOOL_ID,
        NAME,
        "Read a UTF-8 text file, or a supported PDF, DOCX, XLSX, or PPTX document, which is projected to deterministic Markdown text. Resolve relative paths from the execution cwd; absolute paths must stay within the Session's admitted Client workspace. Start at the 1-based offset (default 1). An optional positive limit bounds the returned lines; otherwise Read returns a contiguous prefix of at most 2000 complete lines and 50KB. For a saved output locator, byte_offset reads a UTF-8 page starting at a zero-based byte position. Use the continuation shown in the result to read more.",
        policy,
    )
}

pub(super) fn registration(policy: ToolInvocationPolicy) -> NativeToolRegistration {
    NativeToolRegistration::new(definition(policy), std::sync::Arc::new(ReadTool))
}

/// The native Read executor.
pub struct ReadTool;

impl ToolExecutor for ReadTool {
    fn start<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> ToolExecutionHandle<'a> {
        let cancellation = context.cancellation.clone();
        ToolExecutionHandle::settled_by_operation(
            Box::pin(async move { run_read(&invocation, &context).await }),
            cancellation,
        )
    }

    fn progress_capability(&self) -> ToolProgressCapability {
        ToolProgressCapability::None
    }
}

async fn run_read(
    invocation: &ToolInvocation,
    context: &ToolExecutionContext<'_>,
) -> ToolExecutionResult {
    let input = match ReadInput::parse(&invocation.arguments) {
        Ok(input) => input,
        Err(error) => return failed_result(error),
    };
    let target = interpret_path(context.workspace.root(), &input.path);
    // The shared whitelist picks the wire operation: a whitelisted
    // extension dispatches the document-page exchange, everything else
    // the plain text page. The same classification runs Client-side —
    // one policy, never a second opinion.
    let format = match rustx_execution_protocol::classify_source(&target) {
        rustx_execution_protocol::SourceKind::Document(format) => Some(format),
        rustx_execution_protocol::SourceKind::Text => None,
    };
    // Cancellation observable before the exchange is admitted means the
    // dispatch never starts: no request frame is ever written for a
    // cancelled read.
    if context.cancellation.is_cancelled() {
        return cancelled_result(context.cancellation.reason());
    }
    // There is no Server-local read path: the Session's admitted
    // Execution Client is the only physical reader, reached through the
    // non-owning route composition bound to this workspace. An unbound
    // route is an honest refusal, never a fallback.
    let Some(scope) = context.workspace.execution_scope().cloned() else {
        return failed_result("Session execution unavailable: no admitted Execution Client");
    };
    // The scope owns the whole dispatch: bounded admission, the single
    // writer wait under the absolute page deadline, and the blocking
    // exchange awaited to physical settlement even when cancellation
    // wins — a cancelled read never detaches the wire.
    let page = if let Some(locator) = crate::tools::managed_import::locator_at(&target) {
        let Some(conversation) =
            rustx_execution_protocol::save::ConversationId::parse(context.conversation_id.as_str())
        else {
            return failed_result("saved output is unavailable for this Conversation");
        };
        if let Some(offset) = input.byte_offset {
            return match scope
                .read_saved_output_range(locator, conversation, offset, &context.cancellation)
                .await
            {
                Ok(range) => project_saved_bytes(&range),
                Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
                    cancelled_result(context.cancellation.reason())
                }
                Err(error) => failed_result(read_failure(&target, None, &error)),
            };
        }
        scope
            .read_saved_output(
                locator,
                conversation,
                input.offset(),
                input.limit,
                &context.cancellation,
            )
            .await
    } else if input.byte_offset.is_some() {
        return failed_result("byte_offset requires a saved output locator");
    } else if let Some(operation) = crate::tools::native::bash_operation_at(&target) {
        scope
            .read_bash_output(
                operation,
                context.conversation_id.as_str().to_owned(),
                input.offset(),
                input.limit,
                &context.cancellation,
            )
            .await
    } else {
        scope
            .read_page(
                &target,
                input.offset(),
                input.limit,
                format.is_some(),
                &context.cancellation,
            )
            .await
    };
    let page = match page {
        Ok(page) => page,
        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {
            return cancelled_result(context.cancellation.reason());
        }
        Err(error) => return failed_result(read_failure(&target, format, &error)),
    };
    project_page(&page, &input)
}

fn project_saved_bytes(page: &rustx_execution_server::RangeRead) -> ToolExecutionResult {
    let text = match std::str::from_utf8(&page.bytes) {
        Ok(text) => text,
        Err(error) if error.error_len().is_none() && page.next_offset.is_some() => {
            std::str::from_utf8(&page.bytes[..error.valid_up_to()]).expect("validated UTF-8 prefix")
        }
        Err(_) => return failed_result("byte_offset must start at a valid UTF-8 boundary"),
    };
    let end = page.offset + text.len() as u64;
    let size = page.metadata.size.get();
    if end == size {
        return success_text(text.to_owned(), None);
    }
    let mut output = text.to_owned();
    let _ = write!(
        output,
        "\n\n[Showing bytes {}–{end} of {size}. Use byte_offset={end} to continue.]",
        page.offset
    );
    success_text(
        output,
        Some(TruncationState {
            truncated: true,
            original_bytes: Some(size),
        }),
    )
}

/// Renders one physical-read failure as the user-meaningful diagnostic.
/// The stable [`rustx_execution_protocol::ReadError`] code is recovered by
/// downcast — message text is never parsed — and mapped onto the
/// historical Read diagnostics using only the interpreted target and the
/// whitelist's extension label. Parser details never cross the execution
/// boundary, so a decode refusal reports the rustX classification.
fn read_failure(
    target: &std::path::Path,
    format: Option<rustx_execution_protocol::DocumentFormat>,
    error: &std::io::Error,
) -> String {
    use rustx_execution_protocol::ReadError;
    // The unavailable condition carries its own complete diagnostic —
    // wrapping it in "cannot read" would only obscure it.
    if error.kind() == std::io::ErrorKind::NotConnected {
        return error.to_string();
    }
    let physical = error
        .get_ref()
        .and_then(|inner| inner.downcast_ref::<rustx_execution_server::PhysicalError>())
        .map(|physical| physical.0);
    match physical {
        Some(ReadError::InvalidUtf8) => format!(
            "{} is not a UTF-8 text file; binary content is never fabricated as text",
            target.display()
        ),
        Some(ReadError::DocumentTooLarge) => format!(
            "{} is at least {} bytes; document reads are bounded to {} bytes",
            target.display(),
            rustx_execution_protocol::MAX_DOCUMENT_SOURCE_BYTES + 1,
            rustx_execution_protocol::MAX_DOCUMENT_SOURCE_BYTES
        ),
        Some(ReadError::NoTextLayer) => format!(
            "{} has no extractable text layer; rustX never performs OCR or image inference",
            target.display()
        ),
        Some(ReadError::DocumentDecodeFailed | ReadError::UnsupportedDocument) => format!(
            "cannot decode {} as {}: document decode failed",
            target.display(),
            format.map_or("document", rustx_execution_protocol::DocumentFormat::label)
        ),
        Some(ReadError::NotFound) => {
            format!("cannot read {}: the path does not exist", target.display())
        }
        Some(ReadError::Denied) => {
            format!("cannot read {}: access is denied", target.display())
        }
        Some(ReadError::Conflict) => format!(
            "cannot read {}: the file changed while it was being read",
            target.display()
        ),
        _ => format!("cannot read {}: {error}", target.display()),
    }
}

/// The one shared Read projection over a scanned
/// [`rustx_confined_fs::TextPage`]: continuation diagnostics and result
/// semantics for every source that produced logical text, reproduced
/// exactly from the historical projection.
fn project_page(page: &rustx_confined_fs::TextPage, input: &ReadInput) -> ToolExecutionResult {
    let offset = input.offset();
    let total_lines = page.total_lines;
    let shown = u64::from(page.shown_lines);
    match page.end {
        rustx_confined_fs::PageEnd::OffsetPastEnd => {
            return failed_result(format!(
                "Offset {offset} is beyond end of file ({total_lines} lines total)"
            ));
        }
        rustx_confined_fs::PageEnd::FirstLineTooLarge { bytes } => {
            let size = format_size(bytes);
            let target = std::path::Path::new(&input.path);
            let text = if crate::tools::managed_import::locator_at(target).is_some() {
                format!(
                    "[Line {offset} is {size}, exceeds 50.0KB limit. Use Read with byte_offset=0 to page through this saved output.]"
                )
            } else if crate::tools::native::bash_operation_at(target).is_some() {
                format!("[Line {offset} is {size}, exceeds 50.0KB limit]")
            } else {
                format!(
                    "[Line {offset} is {size}, exceeds 50.0KB limit. Use bash: sed -n '{offset}p' {} | head -c {}]",
                    input.path, NATIVE_FILE_TOOL_MAX_BYTES
                )
            };
            return success_text(
                text,
                Some(TruncationState {
                    truncated: true,
                    original_bytes: Some(page.original_bytes),
                }),
            );
        }
        _ => {}
    }

    let mut output = page.text.clone();
    let continuation_offset = offset.saturating_add(shown);
    match page.end {
        rustx_confined_fs::PageEnd::LineLimit => {
            let _ = write!(
                output,
                "\n\n[Showing lines {offset}-{} of {total_lines}. Use offset={continuation_offset} to continue.]",
                continuation_offset.saturating_sub(1)
            );
        }
        rustx_confined_fs::PageEnd::ByteLimit => {
            let _ = write!(
                output,
                "\n\n[Showing lines {offset}-{} of {total_lines} (50KB limit). Use offset={continuation_offset} to continue.]",
                continuation_offset.saturating_sub(1)
            );
        }
        rustx_confined_fs::PageEnd::RequestedLimit => {
            let _ = write!(
                output,
                "\n\n[{} more lines in file. Use offset={continuation_offset} to continue.]",
                total_lines.saturating_sub(offset.saturating_sub(1).saturating_add(shown))
            );
        }
        rustx_confined_fs::PageEnd::Complete
        | rustx_confined_fs::PageEnd::FirstLineTooLarge { .. }
        | rustx_confined_fs::PageEnd::OffsetPastEnd => {}
    }

    let truncated = page.end != rustx_confined_fs::PageEnd::Complete;
    success_text(
        output,
        truncated.then_some(TruncationState {
            truncated: true,
            original_bytes: Some(page.original_bytes),
        }),
    )
}

fn format_size(bytes: u64) -> String {
    format!("{}.{:01}KB", bytes / 1024, bytes % 1024 * 10 / 1024)
}

#[cfg(test)]
mod tests {
    use std::io::Write;
    use std::path::Path;

    use super::{NAME, ReadTool, document, testdata};
    use crate::runtime::identity::{ConversationId, ToolCallId, ToolId};
    use crate::skills::{SkillDiscovery, SkillDiscoveryConfig, SkillSnapshot};
    use crate::tools::artifacts::ArtifactStore;
    use crate::tools::environment::ToolEnvironment;
    use crate::tools::executor::{ProgressReporter, ToolExecutionContext, ToolExecutor};
    use crate::tools::managed_output::ManagedToolOutput;
    use crate::tools::types::{
        ToolExecutionResult, ToolExecutionStatus, ToolInvocation, ToolInvocationMode,
        ToolInvocationPolicy, ToolProgress, ToolResultContent,
    };
    use crate::tools::workspace::Workspace;

    struct NoProgress;

    impl ProgressReporter for NoProgress {
        fn report(&self, _progress: ToolProgress) {}
    }

    #[test]
    fn description_states_the_admitted_workspace_contract() {
        let description = super::registration(ToolInvocationPolicy::default())
            .definition
            .description;
        assert!(
            description.contains(
                "absolute paths must stay within the Session's admitted Client workspace"
            )
        );
    }

    #[test]
    fn description_states_the_document_projection_contract() {
        let description = super::registration(ToolInvocationPolicy::default())
            .definition
            .description;
        assert!(description.contains("PDF, DOCX, XLSX, or PPTX"));
        assert!(description.contains("deterministic Markdown"));
        // The tool schema stays Read-shaped: no document-specific arguments,
        // no xberg implementation details.
        assert!(!description.contains("xberg"));
        assert!(!description.contains("OCR"));
    }

    /// A Skill package is an ordinary host directory. Read reaches its
    /// `SKILL.md` through the exact published catalog location, and reaches a
    /// bundled asset through the same relative spelling `SKILL.md` uses —
    /// exactly what Bash would run. No virtual namespace participates.
    #[tokio::test]
    #[allow(clippy::too_many_lines)]
    async fn reads_skill_files_at_their_published_host_paths() {
        let directory = tempfile::tempdir().expect("temporary root");
        let execution = crate::session_execution_support::ExecutionFixture::new(directory.path());
        let mut workspace = Workspace::new(directory.path()).expect("workspace");
        workspace.bind_execution_scope(Some(execution.scope.clone()));
        let skill_root = directory.path().join(".agents/skills");
        let skill = skill_root.join("release-guide");
        std::fs::create_dir_all(skill.join("assets")).expect("Skill root");
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: release-guide\ndescription: Release guidance.\n---\nUse assets/checklist.md\n",
        )
        .expect("SKILL.md");
        let mut body = std::fs::OpenOptions::new()
            .append(true)
            .open(skill.join("SKILL.md"))
            .unwrap();
        let mut lines = vec![b'x'; 65_536];
        for index in (63..lines.len()).step_by(64) {
            lines[index] = b'\n';
        }
        for _ in 0..32 {
            body.write_all(&lines).unwrap();
        }
        body.write_all(b"past-the-old-file-bound\n").unwrap();
        drop(body);

        std::fs::write(skill.join("assets/checklist.md"), "procedure\n").expect("asset");
        let discovered =
            SkillDiscovery::with_config(SkillDiscoveryConfig::workspace_root(skill_root))
                .discover();
        let snapshot = SkillSnapshot::from_discovery(discovered);
        let location = snapshot.catalog_entries()[0].location.clone();
        let frozen_catalog = crate::skills::render_skill_catalog(
            snapshot.catalog_entries(),
            crate::skills::SkillReadTool::Read,
        );
        assert_eq!(
            Path::new(&location),
            // Canonical, so this holds on platforms whose temporary root is
            // itself reached through a symlink.
            std::fs::canonicalize(skill.join("SKILL.md"))
                .expect("canonical SKILL.md")
                .as_path(),
            "the catalog publishes the canonical host path of SKILL.md"
        );

        let conversation_id = ConversationId::new("conv_ad60d32d-eec8-76ec-895d-640e87778462");
        let artifacts_root = directory.path().join("artifacts");
        let artifacts =
            ArtifactStore::new(conversation_id.clone(), &artifacts_root).expect("artifacts");
        let tool_output =
            ManagedToolOutput::new(conversation_id.clone(), artifacts_root.join("tool-output"))
                .expect("managed output");
        let progress = NoProgress;
        let environment = ToolEnvironment::new();
        let context = |call: &str, path: &str| {
            (
                ToolInvocation {
                    id: crate::tools::types::ToolInvocationId::Agent {
                        call_id: ToolCallId::new(call),
                    },
                    tool_id: ToolId::new("tool-read"),
                    tool_name: NAME.to_owned(),
                    mode: ToolInvocationMode::Foreground,
                    arguments: serde_json::json!({ "path": path }),
                },
                ToolExecutionContext {
                    stdout_capture: None,
                    workspace_changes: None,
                    model_invocation: None,
                    attempt_id: None,
                    goal: None,
                    schedule: None,
                    conversation_id: &conversation_id,
                    execution_id: None,
                    cancellation: crate::runtime::ExecutionCancellation::detached(
                        crate::runtime::CancellationSignal::new(),
                        crate::runtime::types::CancellationReason::UserRequested,
                    ),
                    workspace: &workspace,
                    progress: &progress,
                    artifacts: &artifacts,
                    tool_output: &tool_output,
                    environment: &environment,
                    questionnaire_requester: None,
                    todos: None,
                    subagent: None,
                    bound_mutation: None,
                },
            )
        };

        let (invocation, execution) = context("read-skill-call", &location);
        let result = ReadTool.start(invocation, execution).completion.await;
        assert_eq!(result.status, ToolExecutionStatus::Success);
        let Some(ToolResultContent::Text(text)) = result.content.first() else {
            panic!("Read returned unexpected content: {result:?}");
        };
        assert!(text.text.contains("assets/checklist.md"));

        assert!(text.text.contains("Use offset="));
        assert!(!text.text.contains("past-the-old-file-bound"));
        let (mut invocation, execution) = context("read-skill-continuation", &location);
        invocation.arguments["offset"] = serde_json::json!(32 * 1024 + 6);
        let continued = ReadTool.start(invocation, execution).completion.await;
        assert_eq!(continued.status, ToolExecutionStatus::Success);
        let Some(ToolResultContent::Text(continued)) = continued.content.first() else {
            panic!("continuation")
        };
        assert!(continued.text.contains("past-the-old-file-bound"));
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: release-guide\ndescription: Release guidance.\n---\nCurrent body.\n",
        )
        .expect("edit known SKILL.md body");
        let (invocation, execution) = context("read-skill-current", &location);
        let current = ReadTool.start(invocation, execution).completion.await;
        assert_eq!(current.status, ToolExecutionStatus::Success);
        let Some(ToolResultContent::Text(current_text)) = current.content.first() else {
            panic!("Read returned unexpected current content: {current:?}");
        };
        assert!(current_text.text.contains("Current body."));
        assert!(
            text.text.contains("assets/checklist.md"),
            "the earlier ToolResult value is not rewritten"
        );
        assert_eq!(
            crate::skills::render_skill_catalog(
                snapshot.catalog_entries(),
                crate::skills::SkillReadTool::Read
            ),
            frozen_catalog,
            "editing a known body cannot mutate the frozen catalog"
        );

        std::fs::remove_file(skill.join("SKILL.md")).expect("remove known SKILL.md");
        let (invocation, execution) = context("read-skill-removed", &location);
        let removed = ReadTool.start(invocation, execution).completion.await;
        assert!(matches!(removed.status, ToolExecutionStatus::Failed { .. }));

        // The Skill's own relative reference, resolved against the package
        // directory the location names.
        let asset = skill.join("assets/checklist.md");
        let (invocation, execution) = context("read-skill-asset", &asset.to_string_lossy());
        let asset_result = ReadTool.start(invocation, execution).completion.await;
        assert_eq!(asset_result.status, ToolExecutionStatus::Success);
        let Some(ToolResultContent::Text(text)) = asset_result.content.first() else {
            panic!("Read returned unexpected content: {asset_result:?}");
        };
        assert!(text.text.contains("procedure"));
        // A symlink inside the workspace is still refused: the confined
        // read opens sources no-follow at the leaf, so link indirection —
        // even to an in-root target — is an honest denial, never a
        // Server-local dereference.
        let shared = directory.path().join("shared-reference.md");
        std::fs::write(&shared, "shared reference").unwrap();
        let linked = skill.join("reference.md");
        std::os::unix::fs::symlink(shared, &linked).unwrap();
        let (invocation, execution) =
            context("read-skill-reference-link", &linked.to_string_lossy());
        let linked = ReadTool.start(invocation, execution).completion.await;
        let crate::tools::types::ToolExecutionStatus::Failed { error } = &linked.status else {
            panic!("a leaf symlink must be refused: {linked:?}");
        };
        assert!(
            !error.contains("shared reference"),
            "no linked body may leak: {error}"
        );

        // A missing workspace-relative path still fails; Skill locations are
        // ordinary host paths, not virtual aliases.
        let (invocation, execution) = context(
            "read-skill-missing",
            ".agents/skills/release-guide/SKILL.md",
        );
        let missing_result = ReadTool.start(invocation, execution).completion.await;
        assert!(matches!(
            missing_result.status,
            ToolExecutionStatus::Failed { .. }
        ));
    }

    /// A Skill package whose published location escapes the Session's
    /// admitted workspace root is honestly refused: discovery canonicalizes
    /// the instruction link to the external file it names, and Read —
    /// confined to the admitted root with no Server-local fallback —
    /// reports the denial without leaking a byte of either linked file.
    #[cfg(unix)]
    #[tokio::test]
    async fn skill_locations_escaping_the_admitted_root_are_denied() {
        let reader = TestRead::new();
        let skill_root = reader.workspace.root().join(".agents/skills");
        let skill = skill_root.join("linked-guide");
        std::fs::create_dir_all(&skill).unwrap();

        let external = tempfile::tempdir().unwrap();
        let instructions = external.path().join("guide.md");
        let reference = external.path().join("reference.md");
        std::fs::write(
            &instructions,
            "---\nname: linked-guide\ndescription: Guide\n---\nRead references.md\n",
        )
        .unwrap();
        std::fs::write(&reference, "Linked resource.\n").unwrap();
        std::os::unix::fs::symlink(instructions, skill.join("SKILL.md")).unwrap();
        std::os::unix::fs::symlink(reference, skill.join("references.md")).unwrap();

        let outcome = SkillDiscovery::with_config(SkillDiscoveryConfig::workspace_root(skill_root))
            .discover();
        assert_eq!(
            outcome.diagnostics,
            [] as [crate::skills::SkillDiagnostic; 0]
        );
        let location = outcome.packages[0].location();
        let result = reader
            .read("read-linked-skill", serde_json::json!({ "path": location }))
            .await;
        let error = failure_of(result);
        assert!(
            !error.contains("Read references.md"),
            "the escaping instruction body must not leak: {error}"
        );
        let reference = Path::new(location).with_file_name("references.md");
        let result = reader
            .read(
                "read-linked-resource",
                serde_json::json!({ "path": reference }),
            )
            .await;
        let error = failure_of(result);
        assert!(
            !error.contains("Linked resource."),
            "the escaping resource body must not leak: {error}"
        );
    }

    // -----------------------------------------------------------------------
    // Document projection (Issue #48)
    // -----------------------------------------------------------------------

    /// The committed corpus of deterministic document fixtures.
    const FIXTURE_ROOT: &str = "tests/fixtures/read/documents";

    /// A minimal end-to-end harness: one workspace bound to a real
    /// Session execution route (an admitted staged Execution Client)
    /// plus the full `ToolExecutionContext`, exactly the native-tool
    /// execution boundary. Native Read has no Server-local path, so a
    /// harness without the route would only prove the unavailable
    /// diagnostic.
    struct TestRead {
        _directory: tempfile::TempDir,
        execution: crate::session_execution_support::ExecutionFixture,
        workspace: Workspace,
        conversation_id: ConversationId,
        artifacts: ArtifactStore,
        tool_output: ManagedToolOutput,
        environment: ToolEnvironment,
        progress: NoProgress,
    }

    impl TestRead {
        fn new() -> Self {
            let directory = tempfile::tempdir().expect("temporary root");
            let execution =
                crate::session_execution_support::ExecutionFixture::new(directory.path());
            let mut workspace = Workspace::new(directory.path()).expect("workspace");
            workspace.bind_execution_scope(Some(execution.scope.clone()));
            let conversation_id = ConversationId::new("conv_82600fb2-6716-7582-89ab-f9b94f09c78f");
            let artifacts =
                ArtifactStore::new(conversation_id.clone(), directory.path().join("artifacts"))
                    .expect("artifacts");
            let tool_output = ManagedToolOutput::new(
                conversation_id.clone(),
                directory.path().join("tool-output"),
            )
            .expect("managed output");
            Self {
                _directory: directory,
                execution,
                workspace,
                conversation_id,
                artifacts,
                tool_output,
                environment: ToolEnvironment::new(),
                progress: NoProgress,
            }
        }

        /// Runs one `read` invocation through the real executor boundary.
        async fn read(&self, call: &str, arguments: serde_json::Value) -> ToolExecutionResult {
            let signal = crate::runtime::CancellationSignal::new();
            self.execute_with_signal(call, arguments, &signal).await
        }

        /// Builds the executor future with a caller-controlled cancellation
        /// signal, for tests that must observe intermediate states.
        fn execute_with_signal<'a>(
            &'a self,
            call: &str,
            arguments: serde_json::Value,
            signal: &'a crate::runtime::CancellationSignal,
        ) -> futures_util::future::BoxFuture<'a, ToolExecutionResult> {
            let invocation = ToolInvocation {
                id: crate::tools::types::ToolInvocationId::Agent {
                    call_id: ToolCallId::new(call),
                },
                tool_id: ToolId::new(super::TOOL_ID),
                tool_name: super::NAME.to_owned(),
                mode: ToolInvocationMode::Foreground,
                arguments,
            };
            let context = ToolExecutionContext {
                stdout_capture: None,
                workspace_changes: None,
                model_invocation: None,
                attempt_id: None,
                goal: None,
                schedule: None,
                conversation_id: &self.conversation_id,
                execution_id: None,
                cancellation: crate::runtime::ExecutionCancellation::detached(
                    signal.clone(),
                    crate::runtime::types::CancellationReason::UserRequested,
                ),
                workspace: &self.workspace,
                progress: &self.progress,
                artifacts: &self.artifacts,
                tool_output: &self.tool_output,
                environment: &self.environment,
                questionnaire_requester: None,
                todos: None,
                subagent: None,
                bound_mutation: None,
            };
            ReadTool.start(invocation, context).completion
        }

        /// Writes `bytes` into the workspace and returns its absolute host
        /// path spelling.
        fn write(&self, name: &str, bytes: &[u8]) -> String {
            let path = self.workspace.root().join(name);
            std::fs::write(&path, bytes).expect("fixture bytes");
            path.to_str().expect("workspace path is UTF-8").to_owned()
        }

        /// Copies one committed corpus fixture into the authorized
        /// workspace under its exact name and returns the absolute host
        /// path — the execution boundary can only serve files inside the
        /// Session's admitted workspace root.
        fn copy_fixture(&self, relative: &str) -> String {
            let bytes = std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join(FIXTURE_ROOT)
                    .join(relative),
            )
            .unwrap_or_else(|error| panic!("committed fixture {relative}: {error}"));
            let target = self.workspace.root().join("documents").join(relative);
            std::fs::create_dir_all(target.parent().expect("fixture parent"))
                .expect("fixture directory");
            std::fs::write(&target, bytes).expect("fixture copy");
            target.to_str().expect("workspace path is UTF-8").to_owned()
        }
    }

    #[tokio::test]
    async fn saved_output_pages_a_long_utf8_line_without_losing_bytes() {
        let reader = TestRead::new();
        let scope = reader.workspace.execution_scope().unwrap().clone();
        let signal = crate::runtime::CancellationSignal::new();
        let cancellation = crate::runtime::ExecutionCancellation::detached(
            signal,
            crate::runtime::types::CancellationReason::UserRequested,
        );
        let boundary = rustx_execution_protocol::MAX_TEXT_BYTES - 1;
        let prefix = "x".repeat(boundary);
        let data = format!("{prefix}🦀tail\n");
        let mut import = crate::tools::managed_import::Import::new(
            scope,
            reader.conversation_id.clone(),
            cancellation,
        );
        import.push(&data);
        let path = import.publish().await.expect("published Client output");
        let initial = reader
            .read("saved-lines", serde_json::json!({"path": path}))
            .await;
        assert_eq!(initial.status, ToolExecutionStatus::Success);
        assert!(
            initial
                .model_facing_projection()
                .as_text()
                .contains("byte_offset=0")
        );
        assert!(!initial.model_facing_projection().as_text().contains("sed"));

        let first = reader
            .read(
                "saved-bytes-1",
                serde_json::json!({"path": path, "byte_offset": 0}),
            )
            .await;
        assert_eq!(first.status, ToolExecutionStatus::Success);
        let [ToolResultContent::Text(first_text)] = first.content.as_slice() else {
            panic!("range text");
        };
        assert_eq!(&first_text.text[..boundary], prefix);
        assert!(first_text.text.contains(&format!("byte_offset={boundary}")));
        assert!(!first_text.text.contains('🦀'));
        assert_eq!(
            first.truncation.unwrap().original_bytes,
            Some(data.len() as u64)
        );

        let last = reader
            .read(
                "saved-bytes-2",
                serde_json::json!({"path": path, "byte_offset": boundary}),
            )
            .await;
        assert_eq!(last.status, ToolExecutionStatus::Success);
        let [ToolResultContent::Text(last_text)] = last.content.as_slice() else {
            panic!("last range text");
        };
        assert_eq!(last_text.text, "🦀tail\n");
        assert_eq!(last.truncation, None);

        let split = reader
            .read(
                "saved-invalid-byte",
                serde_json::json!({"path": path, "byte_offset": boundary + 1}),
            )
            .await;
        assert!(failure_of(split).contains("valid UTF-8 boundary"));
        reader.execution.settle().await;
    }

    fn text_of(result: &ToolExecutionResult) -> &str {
        let Some(ToolResultContent::Text(text)) = result.content.first() else {
            panic!("Read returned unexpected content: {result:?}");
        };
        &text.text
    }

    fn failure_of(result: ToolExecutionResult) -> String {
        match result.status {
            ToolExecutionStatus::Failed { error } => error,
            other => panic!("expected a failed result, got {other:?}"),
        }
    }

    /// The committed binary fixtures are byte-for-byte the output of the
    /// in-repo generator, so every fixture stays reviewable and
    /// reproducible.
    #[test]
    fn committed_fixture_corpus_matches_the_in_repo_generator() {
        for (name, bytes) in testdata::corpus() {
            let committed = std::fs::read(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join(FIXTURE_ROOT)
                    .join(name),
            )
            .unwrap_or_else(|error| panic!("committed fixture {name} is missing: {error}"));
            assert_eq!(
                committed, bytes,
                "fixture {name} diverged from the generator"
            );
        }
    }

    /// Each whitelisted format projects to deterministic Markdown through
    /// the real tool boundary: a repeated read returns exactly equal
    /// model-facing text of the ordinary textual Read kind.
    #[tokio::test]
    async fn whitelisted_documents_project_deterministic_markdown() {
        let expected_fragments: &[(&str, &[&str])] = &[
            (
                "pdf/small-text.pdf",
                &["The rustX document projection. Hello from a text-layer PDF."],
            ),
            (
                "docx/small.docx",
                &[
                    "## Quarterly Report",
                    "Revenue grew in every region this quarter.",
                    "- North America led growth",
                    "| Region | Revenue |",
                    "| North | 120 |",
                ],
            ),
            (
                "xlsx/small.xlsx",
                &[
                    "## Summary",
                    "| Total revenue | 215 |",
                    "## Data",
                    "| Region | Q1 |",
                    "| North | 120 |",
                ],
            ),
            (
                "pptx/small.pptx",
                &[
                    "## Project Kickoff",
                    "Milestone one lands in March",
                    "## Budget Review",
                    "Infrastructure stays flat",
                ],
            ),
        ];
        let read = TestRead::new();
        for (fixture, fragments) in expected_fragments {
            let path = read.copy_fixture(fixture);
            let first = read
                .read("first", serde_json::json!({ "path": path }))
                .await;
            assert_eq!(first.status, ToolExecutionStatus::Success, "{fixture}");
            // The model-facing result is an ordinary textual Read result.
            assert!(matches!(
                first.content.as_slice(),
                [ToolResultContent::Text(_)]
            ));
            let text = text_of(&first).to_owned();
            let fragments: &[&str] = fragments;
            for fragment in fragments {
                assert!(
                    text.contains(fragment),
                    "{fixture}: missing `{fragment}` in:\n{text}"
                );
            }
            let second = read
                .read("second", serde_json::json!({ "path": path }))
                .await;
            assert_eq!(
                text_of(&second),
                text,
                "{fixture} must project deterministically"
            );
        }
    }

    #[tokio::test]
    async fn multipage_pdf_preserves_a_useful_page_boundary() {
        let read = TestRead::new();
        let result = read
            .read(
                "multi",
                serde_json::json!({ "path": read.copy_fixture("pdf/multi-page-text.pdf") }),
            )
            .await;
        assert_eq!(result.status, ToolExecutionStatus::Success);
        let text = text_of(&result);
        let one = text
            .find("Page one heading. First page body text.")
            .expect("page one text");
        let two = text
            .find("Page two heading. Second page body text.")
            .expect("page two text");
        assert!(one < two, "page one must precede page two");
        // The projector separates the pages with a blank line.
        assert!(text.contains("body text.\n\nPage two heading."));
    }

    #[tokio::test]
    async fn table_heavy_pdf_projects_a_useful_table() {
        let read = TestRead::new();
        let result = read
            .read(
                "table",
                serde_json::json!({ "path": read.copy_fixture("pdf/table-heavy.pdf") }),
            )
            .await;
        assert_eq!(result.status, ToolExecutionStatus::Success);
        let text = text_of(&result);
        for fragment in [
            "| Item | Price |",
            "| --- | --- |",
            "| Widget | 9.99 |",
            "| Gadget | 24.50 |",
        ] {
            assert!(text.contains(fragment), "missing `{fragment}` in:\n{text}");
        }
    }

    /// A scanned (text-less) PDF must fail honestly. OCR and every other
    /// model-backed inference path are excluded at compile time and
    /// disabled at runtime, so no text can be fabricated.
    #[tokio::test]
    async fn scanned_pdf_fails_explicitly_without_ocr() {
        let read = TestRead::new();
        let result = read
            .read(
                "scan",
                serde_json::json!({ "path": read.copy_fixture("pdf/scanned-no-text.pdf") }),
            )
            .await;
        let error = failure_of(result);
        assert!(
            error.contains("no extractable text layer"),
            "unexpected failure: {error}"
        );
        assert!(
            error.contains("never performs OCR"),
            "unexpected failure: {error}"
        );
    }

    /// `offset` and `limit` address the lines of the projected Markdown,
    /// with the ordinary continuation diagnostics.
    #[tokio::test]
    async fn offset_and_limit_address_projected_markdown_lines() {
        let read = TestRead::new();
        let path = read.copy_fixture("xlsx/small.xlsx");
        // The projected workbook has 12 lines: `## Summary`, blank, table,
        // blank, `## Data`, blank, table.
        let page = read
            .read(
                "page",
                serde_json::json!({ "path": path, "offset": 3, "limit": 2 }),
            )
            .await;
        assert_eq!(page.status, ToolExecutionStatus::Success);
        assert_eq!(
            text_of(&page),
            "| Metric | Value |\n| --- | --- |\n\n[8 more lines in file. Use offset=5 to continue.]"
        );
        // Zero normalizes to one, exactly as for text files.
        let zero = read
            .read(
                "zero",
                serde_json::json!({ "path": path, "offset": 0, "limit": 1 }),
            )
            .await;
        assert_eq!(
            text_of(&zero),
            "## Summary\n\n[11 more lines in file. Use offset=2 to continue.]"
        );
        // Offset validation sees the same line accounting as text.
        let beyond = read
            .read("beyond", serde_json::json!({ "path": path, "offset": 13 }))
            .await;
        assert_eq!(
            failure_of(beyond),
            "Offset 13 is beyond end of file (12 lines total)"
        );
    }

    /// The 2000 complete-line Read bound applies to projected document
    /// text, and the continuation offset resumes inside the projection.
    #[tokio::test]
    async fn document_projection_honors_the_2000_line_bound() {
        let read = TestRead::new();
        let path = read.write("rows.xlsx", &testdata::xlsx_with_rows(2100));
        let head = read.read("head", serde_json::json!({ "path": path })).await;
        assert_eq!(head.status, ToolExecutionStatus::Success);
        let text = text_of(&head);
        assert_eq!(
            text.lines().count(),
            2002,
            "2000 lines plus the two-line diagnostic"
        );
        assert!(text.contains("[Showing lines 1-2000 of 2104. Use offset=2001 to continue.]"));
        let state = head.truncation.expect("truncation metadata");
        assert!(state.truncated);

        let tail = read
            .read("tail", serde_json::json!({ "path": path, "offset": 2001 }))
            .await;
        assert_eq!(tail.status, ToolExecutionStatus::Success);
        assert!(
            text_of(&tail).starts_with("| row-001997 | x |"),
            "line 2001 skips the 4 sheet-header lines: {}",
            text_of(&tail)
        );
        assert!(text_of(&tail).ends_with("| row-002100 | x |"));
    }

    /// The 50KB complete-line Read bound applies to projected document
    /// text. `original_bytes` reports the size of the complete untruncated
    /// projection — the logical output — never the compressed source size.
    #[tokio::test]
    async fn document_projection_honors_the_50kb_byte_bound_and_honest_original_bytes() {
        let read = TestRead::new();
        let path = read.write("notes.docx", &testdata::docx_with_paragraphs(900));
        let result = read
            .read("bound", serde_json::json!({ "path": path }))
            .await;
        assert_eq!(result.status, ToolExecutionStatus::Success);
        let text = text_of(&result);
        assert!(
            text.contains("(50KB limit). Use offset="),
            "unexpected: {text}"
        );
        let state = result.truncation.expect("truncation metadata");
        assert!(state.truncated);
        // The honest logical size: the complete projected Markdown, not the
        // stored (compressed) .docx bytes. The decode runs on the same
        // blocking boundary the tool itself uses.
        // The decoder drives its own current-thread runtime, so it runs
        // on the blocking pool here just as it does inside the Client.
        let notes_path = read.workspace.root().join("notes.docx");
        let projected = tokio::task::spawn_blocking(move || {
            rustx_document_reader::decode(
                std::fs::read(&notes_path).expect("fixture bytes"),
                rustx_document_reader::DocumentFormat::Docx,
            )
        })
        .await
        .expect("decode task")
        .expect("full projection");
        assert_eq!(state.original_bytes, Some(projected.len() as u64));
        let stored_bytes = std::fs::metadata(read.workspace.root().join("notes.docx"))
            .expect("fixture")
            .len();
        assert_ne!(state.original_bytes, Some(stored_bytes));
    }

    /// A malformed whitelisted document fails explicitly in exactly the
    /// decoder its extension selects — never as UTF-8 text (these bytes are
    /// valid UTF-8, so a text fallback would "succeed") and never through
    /// another decoder.
    #[tokio::test]
    async fn malformed_whitelisted_documents_fail_explicitly() {
        let read = TestRead::new();
        let cases: &[(&str, &[u8], &str)] = &[
            ("broken.pdf", b"this is not a pdf at all", "as PDF"),
            ("broken.docx", b"this is not a zip at all", "as DOCX"),
            ("broken.xlsx", b"neither is this", "as XLSX"),
            ("broken.pptx", b"nor this", "as PPTX"),
        ];
        for (name, bytes, label) in cases {
            let path = read.write(name, bytes);
            let result = read.read(name, serde_json::json!({ "path": path })).await;
            let error = failure_of(result);
            assert!(
                error.contains(&format!("cannot decode {path} {label}")),
                "{name}: unexpected failure: {error}"
            );
        }
    }

    /// PDF bytes named `.docx` are decoded only by the DOCX decoder and
    /// fail there; they are never reinterpreted as PDF or as text.
    #[tokio::test]
    async fn mislabeled_documents_are_never_reinterpreted() {
        let read = TestRead::new();
        let pdf_bytes = testdata::pdf(&[&["definitely a pdf"]]);
        let path = read.write("mislabeled.docx", &pdf_bytes);
        let result = read
            .read("mislabeled", serde_json::json!({ "path": path }))
            .await;
        let error = failure_of(result);
        assert!(
            error.contains("cannot decode") && error.contains("as DOCX"),
            "unexpected failure: {error}"
        );
    }

    /// An unsupported binary format stays on the text path and fails as
    /// binary even though xberg itself could decode the legacy format;
    /// the whitelist is rustX policy, not xberg capability.
    #[tokio::test]
    async fn unsupported_binary_formats_stay_unsupported() {
        let read = TestRead::new();
        // An OLE/CFB magic header: the legacy .doc container xberg's office
        // support can parse, which rustX's whitelist deliberately excludes.
        let ole2: &[u8] = &[0xD0, 0xCF, 0x11, 0xE0, 0xA1, 0xB1, 0x1A, 0xE1, 0x00, 0x01];
        let path = read.write("legacy.doc", ole2);
        let result = read
            .read("legacy", serde_json::json!({ "path": path }))
            .await;
        let error = failure_of(result);
        assert!(
            error.contains("is not a UTF-8 text file"),
            "unexpected failure: {error}"
        );
    }

    /// Document reads and text reads share one confinement contract: both
    /// dispatch only inside the Session's admitted workspace root. An
    /// absolute host path outside that root — spelled directly or reached
    /// through a symlink inside the root — is refused by the Execution
    /// Client, the tool reports the denial, and no body bytes can leak.
    /// There is no Server-local read to fall back to.
    #[cfg(unix)]
    #[tokio::test]
    async fn reads_outside_the_admitted_workspace_are_denied() {
        let read = TestRead::new();
        let external = tempfile::tempdir().expect("external directory");
        let document_path = external.path().join("briefing.docx");
        std::fs::write(&document_path, testdata::docx()).expect("document fixture");
        let text_path = external.path().join("notes.txt");
        std::fs::write(&text_path, "ordinary text\n").expect("text fixture");
        let escape_link = read.workspace.root().join("escape.txt");
        std::os::unix::fs::symlink(&text_path, &escape_link).expect("escape symlink");

        for target in [&document_path, &text_path, &escape_link] {
            let result = read
                .read(
                    "outside",
                    serde_json::json!({ "path": target.to_str().expect("UTF-8 path") }),
                )
                .await;
            let error = failure_of(result);
            assert!(
                !error.contains("Quarterly Report") && !error.contains("ordinary text"),
                "no out-of-root body may leak: {error}"
            );
        }
    }

    /// The rustX-owned source-size bound rejects an oversized document
    /// before any decoder runs, deterministically.
    #[tokio::test]
    async fn oversized_document_sources_fail_before_decoding() {
        let read = TestRead::new();
        let bound = crate::tools::limits::MAX_DOCUMENT_SOURCE_BYTES;
        let path = read.write("huge.pdf", &vec![0u8; bound + 1]);
        let result = read.read("huge", serde_json::json!({ "path": path })).await;
        let error = failure_of(result);
        assert!(
            error.contains(&format!("document reads are bounded to {bound} bytes")),
            "unexpected failure: {error}"
        );
    }

    /// Top-level OOXML member accounting: two otherwise equivalent valid
    /// packages from the same generator differ only in member count, and
    /// only the one above the decoder's 10_000-member package bound fails —
    /// with the decoder's own security-limit classification, not an
    /// unrelated malformed-package error. (The configured
    /// `max_files_in_archive` guards xberg's standalone archive extractor,
    /// which rustX never routes to; the OOXML decoders enforce their own
    /// internal package bound.)
    #[tokio::test]
    async fn archive_member_accounting_rejects_packages_over_the_decoder_bound() {
        let read = TestRead::new();

        // Below the bound: the equivalent package decodes normally.
        let under = read.write(
            "under-members.docx",
            &testdata::docx_with_member_count(testdata::DOCX_BASE_MEMBERS),
        );
        let under_result = read
            .read("under", serde_json::json!({ "path": under }))
            .await;
        assert_eq!(under_result.status, ToolExecutionStatus::Success);
        assert!(text_of(&under_result).contains("member accounting control"));

        // Above the bound: rejected deterministically by the member bound.
        // xberg 1.0.14 pins its DOCX package bound at 10_000 entries and
        // reports a stable security-limit failure for it.
        let over = read.write(
            "over-members.docx",
            &testdata::docx_with_member_count(10_001),
        );
        let over_result = read.read("over", serde_json::json!({ "path": over })).await;
        let error = failure_of(over_result);
        // The Agent boundary reports the stable rustX classification; the
        // parser's detailed member-limit reason stays at the parser
        // boundary (see the document-reader test) and never crosses the
        // execution wire.
        assert!(
            error.contains("cannot decode") && error.contains("as DOCX"),
            "the member bound must surface as a decode refusal: {error}"
        );
        assert!(
            !error.contains("exceeds limit"),
            "parser detail must not cross the Agent boundary: {error}"
        );
    }

    /// Recursive embedded-object extraction is structurally disabled
    /// (`max_archive_depth: 0`): a whitelisted DOCX carrying a real
    /// embedded object under `word/embeddings/` — the exact prefix xberg's
    /// recursive path scans — projects only the main body, never content
    /// found only inside the embedded object.
    #[tokio::test]
    async fn embedded_objects_never_reach_the_projection() {
        let read = TestRead::new();
        let embedded_pdf = testdata::pdf(&[&["SECRET EMBEDDED PAYLOAD must stay buried"]]);
        let path = read.write(
            "embedded.docx",
            &testdata::docx_with_embedded_object(
                "<w:p><w:r><w:t>Embedded boundary control</w:t></w:r></w:p>",
                "payload.pdf",
                &embedded_pdf,
            ),
        );
        let result = read
            .read("embedded", serde_json::json!({ "path": path }))
            .await;
        assert_eq!(result.status, ToolExecutionStatus::Success);
        let text = text_of(&result);
        // Positive control: the main body is projected.
        assert!(text.contains("Embedded boundary control"));
        // The invariant: embedded-only content never surfaces.
        assert!(
            !text.contains("SECRET EMBEDDED PAYLOAD"),
            "embedded object content leaked into the projection: {text}"
        );
    }

    /// Cancellation observable before dispatch means the exchange is
    /// never admitted — no request frame is ever written — and the tool
    /// settles as cancelled.
    // The session guard deliberately spans the awaits: it serializes the
    // hook-using tests within this single-threaded test runtime.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_cancelled_read_is_never_dispatched() {
        let read = TestRead::new();
        let path = read.write("pre-cancelled.docx", &testdata::docx());
        let _session = document::read_hooks::lock_session();
        // The post-send cut: `starts` literally counts request frames
        // written to the wire.
        let hook = document::read_hooks::install_sent(std::path::Path::new(&path));

        let signal = crate::runtime::CancellationSignal::new();
        signal.cancel();
        let result = read
            .execute_with_signal("doc", serde_json::json!({ "path": path }), &signal)
            .await;

        assert_eq!(
            result.status,
            ToolExecutionStatus::Cancelled {
                reason: crate::runtime::types::CancellationReason::UserRequested,
                phase: crate::tools::types::ToolCancellationPhase::BeforeStart,
            }
        );
        assert_eq!(hook.starts(), 0, "a cancelled read writes no request frame");
    }

    /// Cancellation observed while the exchange is parked inside its
    /// acceptance gate — before the linearized `with_active` grant —
    /// still yields zero request frames: releasing the gate lets the
    /// grant run, and the grant refuses. The connection itself is never
    /// touched.
    // The session guard deliberately spans the awaits: it serializes the
    // hook-using tests within this single-threaded test runtime.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn a_cancelled_admission_writes_no_request_frame() {
        let read = TestRead::new();
        let path = read.write("gate-cancelled.docx", &testdata::docx());
        let _session = document::read_hooks::lock_session();
        let mut hook = document::read_hooks::install_gated(std::path::Path::new(&path));
        // A second watcher on the post-send cut is impossible (one hook
        // at a time), so the reply counter is the evidence no frame ever
        // reached the Client: an answered request would count a reply.
        let signal = crate::runtime::CancellationSignal::new();
        let mut read_future =
            read.execute_with_signal("doc", serde_json::json!({ "path": path }), &signal);

        // The worker reaches the acceptance gate and parks there.
        let probed =
            std::future::poll_fn(|cx| std::task::Poll::Ready(read_future.as_mut().poll(cx))).await;
        assert!(probed.is_pending(), "read must not settle immediately");
        hook.wait_started().await;

        // Cancellation becomes observable while the exchange is gated
        // inside its own admission — before `with_active` runs.
        signal.cancel();
        hook.release();

        let result = read_future.await;
        assert_eq!(
            result.status,
            ToolExecutionStatus::Cancelled {
                reason: crate::runtime::types::CancellationReason::UserRequested,
                phase: crate::tools::types::ToolCancellationPhase::DuringExecution,
            }
        );
        assert_eq!(hook.starts(), 1, "the admission cut was reached");
        assert_eq!(
            hook.replies(),
            0,
            "a refused admission writes no request frame, so no reply exists"
        );
    }

    /// Cancellation observed after dispatch admission: Read stays pending
    /// at the exact cut where cancellation is observable and the admitted
    /// blocking exchange is still physically gated, waits for the accepted
    /// operation's reply before settling, discards its successful semantic
    /// result, and settles as the normalized cancelled result. (Blocking-
    /// task physical settlement itself is established by the production
    /// executor's `exchange.await`; this test proves Read cannot settle
    /// before that await has done its job.)
    ///
    /// The synchronization is fully deterministic (rendezvous edges and
    /// explicit probes, no sleeps): an implementation that returned
    /// `Cancelled` immediately without awaiting the blocking task would be
    /// `Ready` at the post-cancellation pre-release probe and fail here.
    // The session guard deliberately spans the awaits: it serializes the
    // hook-using tests within this single-threaded test runtime.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn document_read_settles_cancelled_after_dispatch_started() {
        let read = TestRead::new();
        // A valid document: the decode, if allowed to finish, would succeed.
        let path = read.write("gated.docx", &testdata::docx());
        let _session = document::read_hooks::lock_session();
        // The post-send cut: the exchange is gated with its request frame
        // already on the wire and the reply unread — an accepted exchange
        // the settlement await must still run to its physical end.
        let mut hook = document::read_hooks::install_sent_gated(std::path::Path::new(&path));

        let signal = crate::runtime::CancellationSignal::new();
        let mut read_future =
            read.execute_with_signal("doc", serde_json::json!({ "path": path }), &signal);

        // 1. First poll: admits the exchange (spawns the blocking task).
        let probed =
            std::future::poll_fn(|cx| std::task::Poll::Ready(read_future.as_mut().poll(cx))).await;
        assert!(probed.is_pending(), "read must not settle immediately");

        // 2. Start rendezvous: the dispatch worker has definitely entered
        //    the exchange and is about to block in the gate.
        hook.wait_started().await;

        // 3. While the exchange is physically gated, Read must not report
        //    any semantic result.
        let probed =
            std::future::poll_fn(|cx| std::task::Poll::Ready(read_future.as_mut().poll(cx))).await;
        assert!(
            probed.is_pending(),
            "a gated exchange must not settle: {probed:?}"
        );

        // 4. Cancellation becomes observable while the exchange is gated.
        signal.cancel();

        // 5. The critical cut: cancellation observable AND the admitted
        //    blocking exchange still physically gated. Read must remain
        //    pending here — it may only settle after the blocking task has
        //    physically settled.
        let probed =
            std::future::poll_fn(|cx| std::task::Poll::Ready(read_future.as_mut().poll(cx))).await;
        assert!(
            probed.is_pending(),
            "Read must not settle while the admitted blocking exchange is still physically running: {probed:?}"
        );

        // 6. The accepted exchange has not replied yet.
        assert!(!hook.replied());

        // 7–8. Release the gate, then rendezvous on the reply the
        //       accepted exchange must produce. Without the executor's
        //       settlement await this rendezvous would race an
        //       already-settled (abandoned) tool.
        hook.release();
        hook.wait_replied().await;

        // 9. Read settles as the normalized cancelled result: the
        //    exchange's success is discarded.
        let result = read_future.await;
        assert_eq!(
            result.status,
            ToolExecutionStatus::Cancelled {
                reason: crate::runtime::types::CancellationReason::UserRequested,
                phase: crate::tools::types::ToolCancellationPhase::DuringExecution,
            }
        );

        // 10. Exactly one dispatch and exactly one reply.
        assert_eq!(hook.starts(), 1);
        assert_eq!(hook.replies(), 1);
    }
}
