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

mod document;
mod input;
#[cfg(test)]
mod testdata;

use futures_util::future::BoxFuture;
use std::fmt::Write as _;

use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::limits::{MAX_READ_LINES, NATIVE_FILE_TOOL_MAX_BYTES};
use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::native::support::{
    cancelled_result, failed_result, interpret_path, success_text,
};
use crate::tools::types::ToolInvocationPolicy;
use crate::tools::types::{ToolExecutionResult, ToolInvocation, TruncationState};

use input::ReadInput;

/// The canonical model-facing name of the tool.
pub const NAME: &str = "read";
/// The canonical identity of the rustX native Read capability.
const TOOL_ID: &str = "tool-read";

/// The tool-owned registration of the native Read tool.
#[must_use]
pub(super) fn registration(policy: ToolInvocationPolicy) -> NativeToolRegistration {
    NativeToolRegistration::new(
        native_definition::<ReadInput>(
            TOOL_ID,
            NAME,
            "Read a UTF-8 text file, or a supported PDF, DOCX, XLSX, or PPTX document, which is projected to deterministic Markdown text. Resolve relative paths from the execution cwd; absolute paths are used as host filesystem paths. Start at the 1-based offset (default 1). An optional positive limit bounds the returned lines; otherwise Read returns a contiguous prefix of at most 2000 complete lines and 50KB. Use the continuation offset shown in the result to read more.",
            policy,
        ),
        std::sync::Arc::new(ReadTool),
    )
    .mandatory()
}

/// The native Read executor.
pub struct ReadTool;

impl ToolExecutor for ReadTool {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        Box::pin(async move { run_read(&invocation, &context).await })
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
    match document::classify_source(&target) {
        document::SourceKind::Document(format) => {
            // Admission: cancellation observable before the decoder is
            // admitted means the decode never starts.
            if context.cancellation.is_cancelled() {
                return cancelled_result(context.cancellation.reason());
            }
            // Decoding is materially more expensive than UTF-8 validation,
            // so it runs on the runtime's blocking-pool boundary instead of
            // an async reactor thread. A blocking task cannot be safely
            // abandoned once started, so cancellation never detaches it:
            // when cancellation wins after admission, the same join handle
            // is awaited to physical settlement and the decode's semantic
            // result is discarded in favor of the normalized cancelled
            // result. There is no separate document cancellation model: the
            // invocation observes the runtime cancellation through the
            // normal Tool Plane contract, with the Agent Loop as the
            // authority.
            let decode_target = target.clone();
            let mut decode = tokio::task::spawn_blocking(move || {
                document::decode_document(&decode_target, format)
            });
            let settled = tokio::select! {
                biased;
                // The cancellation authority wins the race, including ties
                // with a decode that completed in the same wakeup.
                () = context.cancellation.cancelled() => None,
                joined = &mut decode => Some(joined),
            };
            let Some(joined) = settled else {
                // Physically settle the blocking decode, discard whatever
                // it produced, and settle the tool as cancelled.
                let _ = decode.await;
                return cancelled_result(context.cancellation.reason());
            };
            let text = match joined {
                Ok(Ok(text)) => text,
                Ok(Err(error)) => return failed_result(error),
                Err(error) => {
                    return failed_result(format!(
                        "cannot decode {}: document decode task failed: {error}",
                        target.display()
                    ));
                }
            };
            // The logical text of a document source is its projected
            // Markdown: `original_bytes` reports the size of the complete
            // untruncated output, not the size of the compressed binary
            // source.
            let original_bytes = text.len() as u64;
            project_read_text(&text, original_bytes, &input)
        }
        document::SourceKind::Text => {
            let bytes = match std::fs::read(&target) {
                Ok(bytes) => bytes,
                Err(error) => {
                    return failed_result(format!("cannot read {}: {error}", target.display()));
                }
            };
            let original_bytes = bytes.len() as u64;
            let Ok(text) = String::from_utf8(bytes) else {
                return failed_result(format!(
                    "{} is not a UTF-8 text file; binary content is never fabricated as text",
                    target.display()
                ));
            };
            project_read_text(&text, original_bytes, &input)
        }
    }
}

/// The one shared Read projection: line addressing, slicing, the
/// complete-line 2000-line/50KB bounds, continuation diagnostics, and the
/// result semantics for every source that produced logical text.
fn project_read_text(text: &str, original_bytes: u64, input: &ReadInput) -> ToolExecutionResult {
    // `split('\n')` intentionally preserves the trailing empty addressable
    // line, matching the model-facing line accounting used by pi. Thus an
    // empty file has one addressable empty line and a final newline creates
    // one trailing empty line for offset validation.
    let all_lines: Vec<&str> = text.split('\n').collect();
    let total_lines = all_lines.len();
    let offset = input.offset();
    let start = usize::try_from(offset.saturating_sub(1)).unwrap_or(usize::MAX);
    if start >= total_lines {
        return failed_result(format!(
            "Offset {offset} is beyond end of file ({total_lines} lines total)"
        ));
    }

    let requested_end = input
        .limit
        .and_then(|limit| usize::try_from(limit).ok())
        .map_or(total_lines, |limit| {
            start.saturating_add(limit).min(total_lines)
        });
    let selected = &all_lines[start..requested_end];
    let projection = truncate_head(selected);
    if let Some(size) = projection.first_line_too_large {
        let line = offset;
        let text = format!(
            "[Line {line} is {}, exceeds 50.0KB limit. Use bash: sed -n '{line}p' {} | head -c {}]",
            format_size(size),
            input.path,
            NATIVE_FILE_TOOL_MAX_BYTES
        );
        return success_text(
            text,
            Some(TruncationState {
                truncated: true,
                original_bytes: Some(original_bytes),
            }),
        );
    }

    let shown = projection.shown_lines;
    let mut output = projection.lines.join("\n");
    let continuation_offset = offset.saturating_add(shown as u64);
    if projection.stopped_by_line {
        let _ = write!(
            output,
            "\n\n[Showing lines {offset}-{} of {total_lines}. Use offset={continuation_offset} to continue.]",
            offset.saturating_add(shown as u64).saturating_sub(1)
        );
    } else if projection.stopped_by_bytes {
        let _ = write!(
            output,
            "\n\n[Showing lines {offset}-{} of {total_lines} (50KB limit). Use offset={continuation_offset} to continue.]",
            offset.saturating_add(shown as u64).saturating_sub(1)
        );
    } else if requested_end < total_lines {
        let _ = write!(
            output,
            "\n\n[{} more lines in file. Use offset={continuation_offset} to continue.]",
            total_lines.saturating_sub(requested_end)
        );
    }

    let truncated =
        projection.stopped_by_line || projection.stopped_by_bytes || requested_end < total_lines;
    success_text(
        output,
        truncated.then_some(TruncationState {
            truncated: true,
            original_bytes: Some(original_bytes),
        }),
    )
}

struct ReadProjection<'a> {
    lines: Vec<&'a str>,
    shown_lines: usize,
    stopped_by_line: bool,
    stopped_by_bytes: bool,
    first_line_too_large: Option<usize>,
}

/// Returns a complete-line prefix of `lines`, never splitting UTF-8 or
/// removing a middle section. Newline bytes between returned lines count
/// toward the 50KB payload budget.
fn truncate_head<'a>(lines: &[&'a str]) -> ReadProjection<'a> {
    // A final newline is a separator, not an additional content line for
    // the truncation budget. Keep it in an untruncated result, however, so
    // the file's representation remains faithful. Offset validation still
    // uses the full split result above, matching pi's addressable-line
    // behavior for a trailing empty line.
    let counted = if lines.last().is_some_and(|line| line.is_empty()) {
        &lines[..lines.len().saturating_sub(1)]
    } else {
        lines
    };
    let content_bytes = lines
        .iter()
        .map(|line| line.len())
        .sum::<usize>()
        .saturating_add(lines.len().saturating_sub(1));
    if counted.len() <= MAX_READ_LINES && content_bytes <= NATIVE_FILE_TOOL_MAX_BYTES {
        return ReadProjection {
            lines: lines.to_vec(),
            shown_lines: lines.len(),
            stopped_by_line: false,
            stopped_by_bytes: false,
            first_line_too_large: None,
        };
    }
    if let Some(first) = counted.first()
        && first.len() > NATIVE_FILE_TOOL_MAX_BYTES
    {
        return ReadProjection {
            lines: Vec::new(),
            shown_lines: 0,
            stopped_by_line: false,
            stopped_by_bytes: true,
            first_line_too_large: Some(first.len()),
        };
    }
    let mut result = Vec::new();
    let mut bytes = 0usize;
    let mut stopped_by_line = false;
    let mut stopped_by_bytes = false;
    for line in counted {
        if result.len() >= MAX_READ_LINES {
            stopped_by_line = true;
            break;
        }
        let separator = usize::from(!result.is_empty());
        let cost = line.len().saturating_add(separator);
        if bytes.saturating_add(cost) > NATIVE_FILE_TOOL_MAX_BYTES {
            stopped_by_bytes = true;
            break;
        }
        result.push(*line);
        bytes = bytes.saturating_add(cost);
    }
    if !stopped_by_line && !stopped_by_bytes && content_bytes > NATIVE_FILE_TOOL_MAX_BYTES {
        // The only remaining byte can be the terminal newline that was
        // excluded from `counted`; omit that separator in the bounded
        // projection rather than returning a payload over the byte budget.
        stopped_by_bytes = true;
    }
    ReadProjection {
        shown_lines: result.len(),
        lines: result,
        stopped_by_line,
        stopped_by_bytes,
        first_line_too_large: None,
    }
}

fn format_size(bytes: usize) -> String {
    format!("{}.{:01}KB", bytes / 1024, bytes % 1024 * 10 / 1024)
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use super::{NAME, ReadTool, document, testdata};
    use crate::runtime::identity::{ConversationId, ToolCallId, ToolId};
    use crate::skills::{SkillDiscovery, SkillDiscoveryConfig, SkillPackageError, SkillSnapshot};
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
    fn description_states_the_ordinary_host_path_contract() {
        let description = super::registration(ToolInvocationPolicy::default())
            .definition
            .description;
        assert!(description.contains("absolute paths are used as host filesystem paths"));
        assert!(!description.contains(".rustx/skills"));
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
        let workspace = Workspace::new(directory.path()).expect("workspace");
        let skill_root = directory.path().join("configured-skills");
        let skill = skill_root.join("release-guide");
        std::fs::create_dir_all(skill.join("assets")).expect("Skill root");
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: release-guide\ndescription: Release guidance.\n---\nUse assets/checklist.md\n",
        )
        .expect("SKILL.md");
        std::fs::write(skill.join("assets/checklist.md"), "procedure\n").expect("asset");
        let packages = SkillDiscovery::with_config(
            &workspace,
            SkillDiscoveryConfig {
                automatic_roots: Vec::new(),
                explicit_paths: vec![skill_root],
            },
        )
        .discover()
        .expect("Skill discovery");
        let snapshot = SkillSnapshot::new(packages.into_iter().map(Arc::new).collect());
        let location = snapshot.catalog_entries()[0].location.clone();
        let frozen_catalog = crate::skills::render_skill_catalog(snapshot.catalog_entries());
        assert_eq!(
            Path::new(&location),
            // Canonical, so this holds on platforms whose temporary root is
            // itself reached through a symlink.
            std::fs::canonicalize(skill.join("SKILL.md"))
                .expect("canonical SKILL.md")
                .as_path(),
            "the catalog publishes the canonical host path of SKILL.md"
        );

        let conversation_id = ConversationId::new("read-skill");
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
                    call_id: ToolCallId::new(call),
                    tool_id: ToolId::new("tool-read"),
                    tool_name: NAME.to_owned(),
                    mode: ToolInvocationMode::Foreground,
                    arguments: serde_json::json!({ "path": path }),
                },
                ToolExecutionContext {
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
                },
            )
        };

        let (invocation, execution) = context("read-skill-call", &location);
        let result = ReadTool.execute(invocation, execution).await;
        assert_eq!(result.status, ToolExecutionStatus::Success);
        let Some(ToolResultContent::Text(text)) = result.content.first() else {
            panic!("Read returned unexpected content: {result:?}");
        };
        assert!(text.text.contains("assets/checklist.md"));

        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: release-guide\ndescription: Release guidance.\n---\nCurrent body.\n",
        )
        .expect("edit known SKILL.md body");
        let (invocation, execution) = context("read-skill-current", &location);
        let current = ReadTool.execute(invocation, execution).await;
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
            crate::skills::render_skill_catalog(snapshot.catalog_entries()),
            frozen_catalog,
            "editing a known body cannot mutate the frozen catalog"
        );

        std::fs::remove_file(skill.join("SKILL.md")).expect("remove known SKILL.md");
        let (invocation, execution) = context("read-skill-removed", &location);
        let removed = ReadTool.execute(invocation, execution).await;
        assert!(matches!(removed.status, ToolExecutionStatus::Failed { .. }));

        // The Skill's own relative reference, resolved against the package
        // directory the location names.
        let asset = skill.join("assets/checklist.md");
        let (invocation, execution) = context("read-skill-asset", &asset.to_string_lossy());
        let asset_result = ReadTool.execute(invocation, execution).await;
        assert_eq!(asset_result.status, ToolExecutionStatus::Success);
        let Some(ToolResultContent::Text(text)) = asset_result.content.first() else {
            panic!("Read returned unexpected content: {asset_result:?}");
        };
        assert!(text.text.contains("procedure"));

        // The retired virtual spelling is now an ordinary workspace-relative
        // path, and resolves to nothing.
        let (invocation, execution) =
            context("read-skill-virtual", ".rustx/skills/release-guide/SKILL.md");
        let virtual_result = ReadTool.execute(invocation, execution).await;
        assert!(matches!(
            virtual_result.status,
            ToolExecutionStatus::Failed { .. }
        ));
    }

    #[cfg(unix)]
    #[test]
    fn skill_resource_symlink_escape_is_rejected_at_discovery() {
        let directory = tempfile::tempdir().expect("temporary root");
        let workspace = Workspace::new(directory.path()).expect("workspace");
        let skill_root = directory.path().join("configured-skills");
        let skill = skill_root.join("escape-guide");
        std::fs::create_dir_all(&skill).expect("Skill root");
        std::fs::write(
            skill.join("SKILL.md"),
            "---\nname: escape-guide\ndescription: Escape guidance.\n---\nbody\n",
        )
        .expect("SKILL.md");
        let outside = directory.path().join("outside.txt");
        std::fs::write(&outside, "outside\n").expect("outside resource");
        std::os::unix::fs::symlink(&outside, skill.join("references.md"))
            .expect("resource symlink");

        let error = SkillDiscovery::with_config(
            &workspace,
            SkillDiscoveryConfig {
                automatic_roots: Vec::new(),
                explicit_paths: vec![skill_root],
            },
        )
        .discover()
        .expect_err("Skill discovery must reject an escaping resource symlink");
        assert!(matches!(
            error,
            SkillPackageError::UnsupportedSymlink { .. }
        ));
    }

    // -----------------------------------------------------------------------
    // Document projection (Issue #48)
    // -----------------------------------------------------------------------

    /// The committed corpus of deterministic document fixtures.
    const FIXTURE_ROOT: &str = "tests/fixtures/read/documents";

    /// The absolute host path of one committed fixture. Absolute host
    /// filesystem paths are the current Read path contract, so the fixtures
    /// are addressed exactly the way a model would address them.
    fn fixture_path(relative: &str) -> String {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join(FIXTURE_ROOT)
            .join(relative)
            .to_str()
            .expect("fixture path is UTF-8")
            .to_owned()
    }

    /// A minimal end-to-end harness: one workspace plus the full
    /// `ToolExecutionContext`, exactly the native-tool execution boundary.
    struct TestRead {
        _directory: tempfile::TempDir,
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
            let workspace = Workspace::new(directory.path()).expect("workspace");
            let conversation_id = ConversationId::new("read-document");
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
                call_id: ToolCallId::new(call),
                tool_id: ToolId::new(super::TOOL_ID),
                tool_name: super::NAME.to_owned(),
                mode: ToolInvocationMode::Foreground,
                arguments,
            };
            let context = ToolExecutionContext {
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
            };
            ReadTool.execute(invocation, context)
        }

        /// Writes `bytes` into the workspace and returns its absolute host
        /// path spelling.
        fn write(&self, name: &str, bytes: &[u8]) -> String {
            let path = self.workspace.root().join(name);
            std::fs::write(&path, bytes).expect("fixture bytes");
            path.to_str().expect("workspace path is UTF-8").to_owned()
        }
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
            let path = fixture_path(fixture);
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
                serde_json::json!({ "path": fixture_path("pdf/multi-page-text.pdf") }),
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
                serde_json::json!({ "path": fixture_path("pdf/table-heavy.pdf") }),
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
                serde_json::json!({ "path": fixture_path("pdf/scanned-no-text.pdf") }),
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
        let path = fixture_path("xlsx/small.xlsx");
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
        let workspace_root = read.workspace.root().to_path_buf();
        let projected = tokio::task::spawn_blocking(move || {
            document::decode_document(
                &workspace_root.join("notes.docx"),
                document::DocumentFormat::Docx,
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

    /// Document reads and text reads follow the same current path
    /// interpretation: both accept absolute host filesystem paths outside
    /// the workspace root. Both fixtures live in a separate temporary
    /// directory (tempdir B) while the tool's workspace is another one
    /// (tempdir A); the repository checkout is never written.
    #[tokio::test]
    async fn document_and_text_reads_share_the_absolute_path_contract() {
        let read = TestRead::new();
        let external = tempfile::tempdir().expect("external directory");
        let document_path = external.path().join("briefing.docx");
        std::fs::write(&document_path, testdata::docx()).expect("document fixture");
        let text_path = external.path().join("notes.txt");
        std::fs::write(&text_path, "ordinary text\n").expect("text fixture");

        let document = read
            .read(
                "doc",
                serde_json::json!({ "path": document_path.to_str().expect("UTF-8 path") }),
            )
            .await;
        assert_eq!(document.status, ToolExecutionStatus::Success);
        assert!(text_of(&document).contains("## Quarterly Report"));

        // The same contract for an ordinary text file in the same external
        // location, read through its absolute host path.
        let text_read = read
            .read(
                "txt",
                serde_json::json!({ "path": text_path.to_str().expect("UTF-8 path") }),
            )
            .await;
        assert_eq!(text_read.status, ToolExecutionStatus::Success);
        assert_eq!(text_of(&text_read), "ordinary text\n");
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
        assert!(
            error.contains("cannot decode") && error.contains("exceeds limit of 10000"),
            "the member bound, not package malformation, must reject the package: {error}"
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

    /// Cancellation observable before decode admission means the blocking
    /// decoder is never started, and the tool settles as cancelled.
    // The session guard deliberately spans the awaits: it serializes the
    // hook-using tests within this single-threaded test runtime.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn document_decode_is_never_admitted_after_cancellation() {
        let read = TestRead::new();
        let path = read.write("pre-cancelled.docx", &testdata::docx());
        let _session = document::decode_hooks::lock_session();
        let hook = document::decode_hooks::install(std::path::Path::new(&path));

        let signal = crate::runtime::CancellationSignal::new();
        signal.cancel();
        let result = read
            .execute_with_signal("doc", serde_json::json!({ "path": path }), &signal)
            .await;

        assert_eq!(
            result.status,
            ToolExecutionStatus::Cancelled {
                reason: crate::runtime::types::CancellationReason::UserRequested,
            }
        );
        assert_eq!(hook.starts(), 0, "a cancelled read must never decode");
    }

    /// Cancellation observed after decode admission: the same blocking
    /// task is awaited to physical settlement, its successful semantic
    /// result is discarded, and the tool settles as cancelled.
    // The session guard deliberately spans the awaits: it serializes the
    // hook-using tests within this single-threaded test runtime.
    #[allow(clippy::await_holding_lock)]
    #[tokio::test]
    async fn document_read_settles_cancelled_after_decode_started() {
        let read = TestRead::new();
        // A valid document: the decode, if allowed to finish, would succeed.
        let path = read.write("gated.docx", &testdata::docx());
        let _session = document::decode_hooks::lock_session();
        let mut hook = document::decode_hooks::install_gated(std::path::Path::new(&path));

        let signal = crate::runtime::CancellationSignal::new();
        let mut read_future =
            read.execute_with_signal("doc", serde_json::json!({ "path": path }), &signal);

        // Drive the future until the decoder has demonstrably started and
        // is blocked in the gate. No sleeps: every iteration is a yield.
        let started = loop {
            if let std::task::Poll::Ready(early) =
                std::future::poll_fn(|cx| std::task::Poll::Ready(read_future.as_mut().poll(cx)))
                    .await
            {
                panic!("read settled before the decoder was released: {early:?}");
            }
            if hook.starts() >= 1 {
                break true;
            }
            tokio::task::yield_now().await;
        };
        assert!(started, "the decoder never started");

        // While the decoder is still physically paused, Read must not
        // report any semantic result.
        let probed =
            std::future::poll_fn(|cx| std::task::Poll::Ready(read_future.as_mut().poll(cx))).await;
        assert!(
            probed.is_pending(),
            "a gated decode must not settle: {probed:?}"
        );

        // Cancellation wins while the decode is paused; releasing it lets
        // the decode complete successfully — and the success must be
        // discarded in favor of the normalized cancelled result.
        signal.cancel();
        hook.release();
        let result = read_future.await;
        assert_eq!(
            result.status,
            ToolExecutionStatus::Cancelled {
                reason: crate::runtime::types::CancellationReason::UserRequested,
            }
        );
        assert_eq!(hook.starts(), 1);
    }
}
