//! Native Read tool.
//!
//! Read is a sequential, pageable source. It resolves relative paths against
//! the execution cwd, returns a contiguous head from the requested offset,
//! and owns a complete-line 2000-line/50KB projection. The runtime's generic
//! 64KB safety bound remains a last-resort boundary for other tools.

mod input;

use futures_util::future::BoxFuture;
use std::fmt::Write as _;

use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::limits::{MAX_READ_LINES, NATIVE_FILE_TOOL_MAX_BYTES};
use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::native::support::{failed_result, interpret_path, success_text};
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
            "Read a UTF-8 text file. Resolve relative paths from the execution cwd; absolute paths are used as host filesystem paths. Start at the 1-based offset (default 1). An optional positive limit bounds the returned lines; otherwise Read returns a contiguous prefix of at most 2000 complete lines and 50KB. Use the continuation offset shown in the result to read more.",
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
        Box::pin(async move { run_read(&invocation, &context) })
    }
}

fn run_read(
    invocation: &ToolInvocation,
    context: &ToolExecutionContext<'_>,
) -> ToolExecutionResult {
    let input = match ReadInput::parse(&invocation.arguments) {
        Ok(input) => input,
        Err(error) => return failed_result(error),
    };
    let target = interpret_path(context.workspace.root(), &input.path);
    let bytes = match std::fs::read(&target) {
        Ok(bytes) => bytes,
        Err(error) => return failed_result(format!("cannot read {}: {error}", target.display())),
    };
    let original_bytes = bytes.len() as u64;
    let Ok(text) = String::from_utf8(bytes) else {
        return failed_result(format!(
            "{} is not a UTF-8 text file; binary content is never fabricated as text",
            target.display()
        ));
    };

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

    use super::{NAME, ReadTool};
    use crate::runtime::identity::{ConversationId, ToolCallId, ToolId};
    use crate::skills::{SkillDiscovery, SkillDiscoveryConfig, SkillPackageError, SkillSnapshot};
    use crate::tools::artifacts::ArtifactStore;
    use crate::tools::environment::ToolEnvironment;
    use crate::tools::executor::{ProgressReporter, ToolExecutionContext, ToolExecutor};
    use crate::tools::managed_output::ManagedToolOutput;
    use crate::tools::types::{
        ToolExecutionStatus, ToolInvocation, ToolInvocationMode, ToolInvocationPolicy,
        ToolProgress, ToolResultContent,
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

    /// A Skill package is an ordinary host directory. Read reaches its
    /// `SKILL.md` through the exact published catalog location, and reaches a
    /// bundled asset through the same relative spelling `SKILL.md` uses —
    /// exactly what Bash would run. No virtual namespace participates.
    #[tokio::test]
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
                    question_requester: None,
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
}
