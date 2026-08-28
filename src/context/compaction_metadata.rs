//! The deterministic file-operation metadata of one compaction (Issue #140).
//!
//! One compaction transition splits the selected retired canonical span into
//! two halves:
//!
//! ```text
//! retired canonical span
//!         |
//!         +--> semantic content
//!         |       -> ContextSummarizer
//!         |       -> structured Markdown summary body
//!         |
//!         +--> canonical tool facts
//!                 -> this module's deterministic extractor
//!                 -> CompactionSummaryMetadata (typed canonical authority)
//!                 -> read_files / modified_files
//! ```
//!
//! The summary model never contributes to the metadata: prose it generates —
//! including any file path it invents or mentions — is not parsed back into
//! facts. The metadata is accumulated over the compaction lineage only: the
//! earlier compaction summaries inside the selected retired span contribute
//! their typed metadata, and nothing else does. There is no
//! conversation-global accumulator, no sidecar file-history store, and no
//! hidden previous-summary channel.

use std::fmt::Write as _;

use crate::message::types::{AssistantContentBlock, CompactionSummaryMetadata, MessageBlock};
use crate::tools::native::{NativeFileOperation, native_file_operation};

/// Derives the cumulative typed file-operation metadata of one compaction
/// from the exact retired canonical span.
///
/// Extraction reads only canonical rustX-owned structure:
///
/// - an earlier compaction summary inside the span contributes its typed
///   [`CompactionSummaryMetadata`] unchanged;
/// - an `Assistant` tool call contributes one normalized
///   [`NativeFileOperation`] through the native Read/Edit/Write tool modules,
///   which own the decoding that identifies the path;
/// - everything else — user text, assistant prose, tool results, generated
///   summary prose — contributes nothing.
///
/// The merge is lineage set semantics owned by
/// [`CompactionSummaryMetadata::accumulate`]: union, then modification wins
/// over read, in deterministic ascending order.
pub(crate) fn retired_span_metadata(retired: &[MessageBlock]) -> CompactionSummaryMetadata {
    let inherited = retired.iter().filter_map(|message| match message {
        MessageBlock::User(user) => user.kind.compaction_summary_metadata(),
        MessageBlock::Assistant(_) | MessageBlock::Tool(_) => None,
    });
    let mut new_read = Vec::new();
    let mut new_modified = Vec::new();
    for message in retired {
        let MessageBlock::Assistant(assistant) = message else {
            continue;
        };
        for block in &assistant.content {
            let AssistantContentBlock::ToolCall(call) = block else {
                continue;
            };
            match native_file_operation(call) {
                Some(NativeFileOperation::Read { path }) => new_read.push(path),
                Some(NativeFileOperation::Modified { path }) => new_modified.push(path),
                None => {}
            }
        }
    }
    CompactionSummaryMetadata::accumulate(inherited, new_read, new_modified)
}

/// Renders the committed compaction summary text: the model-produced
/// structured summary body followed by the deterministic metadata-derived
/// file sections.
///
/// The `<read-files>`/`<modified-files>` sections are a pure derived
/// rendering of the typed metadata in its canonical order; an empty section
/// is omitted entirely. They are model-visible context, never a metadata
/// authority: recovery and lineage read the typed metadata, and nothing ever
/// parses these sections back into facts.
pub(crate) fn render_summary_text(body: &str, metadata: &CompactionSummaryMetadata) -> String {
    let mut text = body.trim_end().to_owned();
    render_section(&mut text, "read-files", metadata.read_files());
    render_section(&mut text, "modified-files", metadata.modified_files());
    text
}

/// Appends one XML file-list section, or nothing when the list is empty.
fn render_section(text: &mut String, tag: &str, paths: &[String]) {
    if paths.is_empty() {
        return;
    }
    let _ = write!(text, "\n\n<{tag}>");
    for path in paths {
        let _ = write!(text, "\n{path}");
    }
    let _ = write!(text, "\n</{tag}>");
}

#[cfg(test)]
mod tests {
    use super::{render_summary_text, retired_span_metadata};
    use crate::message::content::TextBlock;
    use crate::message::types::{
        AssistantContentBlock, AssistantMessageBlock, CompactionSummaryMetadata, InboundKind,
        MessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::runtime::identity::{MessageId, ToolCallId, ToolId};
    use crate::tools::types::ToolCall;

    fn assistant_with_calls(id: &str, calls: Vec<ToolCall>) -> MessageBlock {
        MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new(id),
            content: calls
                .into_iter()
                .map(AssistantContentBlock::ToolCall)
                .collect(),
        })
    }

    fn call(id: &str, tool_id: &str, name: &str, path: &str) -> ToolCall {
        ToolCall {
            id: ToolCallId::new(id),
            tool_id: ToolId::new(tool_id),
            name: name.to_owned(),
            arguments: serde_json::json!({ "path": path }),
        }
    }

    fn previous_summary(id: &str, read: Vec<String>, modified: Vec<String>) -> MessageBlock {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new(id),
            content: vec![UserContentBlock::Text(TextBlock {
                text: "earlier summary".to_owned(),
            })],
            source: UserSource::Runtime,
            kind: InboundKind::CompactionSummary(
                CompactionSummaryMetadata::new(read, modified).expect("valid metadata"),
            ),
            timestamp: None,
        })
    }

    /// The first-compaction extraction contract: reads and modifications are
    /// classified from canonical tool-call structure, deduplicated, ordered,
    /// and modification wins over read.
    #[test]
    fn extraction_classifies_native_file_calls() {
        let retired = vec![
            assistant_with_calls(
                "a1",
                vec![
                    call("c1", "tool-read", "read", "/src/a.rs"),
                    call("c2", "tool-read", "read", "/src/b.rs"),
                    call("c3", "tool-edit", "edit", "/src/b.rs"),
                    call("c4", "tool-write", "write", "/src/c.rs"),
                ],
            ),
            assistant_with_calls(
                "a2",
                vec![
                    // Repeated operations never duplicate a path.
                    call("c5", "tool-read", "read", "/src/a.rs"),
                    call("c6", "tool-edit", "edit", "/src/b.rs"),
                ],
            ),
        ];
        let metadata = retired_span_metadata(&retired);
        assert_eq!(metadata.read_files(), &["/src/a.rs".to_owned()]);
        assert_eq!(
            metadata.modified_files(),
            &["/src/b.rs".to_owned(), "/src/c.rs".to_owned()]
        );
    }

    /// Only the native tool identities classify: a foreign tool that happens
    /// to be named `read`, a native call without a decodable path, and
    /// assistant prose mentioning paths contribute nothing.
    #[test]
    fn extraction_ignores_foreign_tools_and_prose() {
        let retired = vec![
            assistant_with_calls(
                "a1",
                vec![
                    call("c1", "mcp-filesystem", "read", "/fake/foreign.rs"),
                    ToolCall {
                        id: ToolCallId::new("c2"),
                        tool_id: ToolId::new("tool-read"),
                        name: "read".to_owned(),
                        arguments: serde_json::json!({ "no_path": true }),
                    },
                ],
            ),
            MessageBlock::Assistant(AssistantMessageBlock {
                id: MessageId::new("a2"),
                content: vec![AssistantContentBlock::Text(TextBlock {
                    text: "I edited /fake/prose.rs for you.".to_owned(),
                })],
            }),
        ];
        assert_eq!(
            retired_span_metadata(&retired),
            CompactionSummaryMetadata::empty()
        );
    }

    /// Cumulative lineage: a previous summary inside the retired span merges
    /// its metadata, and a path it listed as read is reclassified when the
    /// new history modifies it.
    #[test]
    fn extraction_merges_inherited_metadata_over_the_lineage() {
        let retired = vec![
            previous_summary(
                "summary-1",
                vec!["/a".to_owned(), "/b".to_owned()],
                vec!["/c".to_owned()],
            ),
            assistant_with_calls(
                "a1",
                vec![
                    call("c1", "tool-read", "read", "/d"),
                    call("c2", "tool-edit", "edit", "/a"),
                ],
            ),
        ];
        let metadata = retired_span_metadata(&retired);
        assert_eq!(metadata.read_files(), &["/b".to_owned(), "/d".to_owned()]);
        assert_eq!(
            metadata.modified_files(),
            &["/a".to_owned(), "/c".to_owned()]
        );
    }

    /// A previous summary outside the retired span contributes nothing.
    #[test]
    fn extraction_never_reaches_outside_the_retired_span() {
        let outside = previous_summary("summary-1", vec!["/outside".to_owned()], Vec::new());
        let retired = vec![assistant_with_calls(
            "a1",
            vec![call("c1", "tool-read", "read", "/inside")],
        )];
        let metadata = retired_span_metadata(&retired);
        assert_eq!(metadata.read_files(), &["/inside".to_owned()]);
        assert!(metadata.modified_files().is_empty());
        drop(outside);
    }

    /// The rendering is the deterministic projection of the metadata: exact
    /// order, exact paths, empty sections omitted.
    #[test]
    fn rendering_projects_the_metadata_exactly() {
        let metadata = CompactionSummaryMetadata::new(
            vec!["/read-only-a".to_owned(), "/read-only-b".to_owned()],
            vec!["/edited-a".to_owned(), "/written-b".to_owned()],
        )
        .expect("valid metadata");
        assert_eq!(
            render_summary_text("## Goal\n\nbody", &metadata),
            "## Goal\n\nbody\n\n<read-files>\n/read-only-a\n/read-only-b\n</read-files>\n\n<modified-files>\n/edited-a\n/written-b\n</modified-files>"
        );
        // An empty section is omitted entirely.
        let read_only = CompactionSummaryMetadata::new(vec!["/a".to_owned()], Vec::new())
            .expect("valid metadata");
        assert_eq!(
            render_summary_text("body", &read_only),
            "body\n\n<read-files>\n/a\n</read-files>"
        );
        // Empty metadata renders no sections at all.
        assert_eq!(
            render_summary_text("body", &CompactionSummaryMetadata::empty()),
            "body"
        );
    }
}
