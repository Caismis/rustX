//! Native Edit tool.
//!
//! Edit is a precise, atomic mutation primitive. All anchors are resolved
//! against one original snapshot, exact matching is preferred, and a
//! NFKC-based fuzzy fallback is used only when exact matching cannot find an
//! anchor. Any validation failure leaves the target unchanged.

mod input;

use std::ops::Range;

use futures_util::future::BoxFuture;
use unicode_normalization::UnicodeNormalization;

use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::native::support::{atomic_commit, failed_result, resolve_path, success_text};
use crate::tools::types::ToolInvocationPolicy;
use crate::tools::types::{ToolExecutionResult, ToolInvocation};

use input::{EditInput, EditReplacement};

/// The canonical model-facing name of the tool.
pub const NAME: &str = "edit";

/// The tool-owned registration of the native Edit tool.
#[must_use]
pub(super) fn registration(policy: ToolInvocationPolicy) -> NativeToolRegistration {
    NativeToolRegistration::new(
        native_definition::<EditInput>(
            "tool-edit",
            NAME,
            "Apply precise text replacements to a UTF-8 file. Resolve relative paths from the execution cwd; absolute paths are used as host filesystem paths. All oldText values match the same original snapshot. Exact matches are preferred, with a cautious Unicode-normalized fallback; ambiguous, overlapping, missing, or no-op edits fail without changing the file.",
            policy,
        ),
        std::sync::Arc::new(EditTool),
    )
    .with_normalizer(input::normalize_arguments)
}

/// The native Edit executor.
pub struct EditTool;

impl ToolExecutor for EditTool {
    fn execute<'a>(
        &'a self,
        invocation: ToolInvocation,
        context: ToolExecutionContext<'a>,
    ) -> BoxFuture<'a, ToolExecutionResult> {
        Box::pin(async move { run_edit(&invocation, &context) })
    }
}

fn run_edit(
    invocation: &ToolInvocation,
    context: &ToolExecutionContext<'_>,
) -> ToolExecutionResult {
    let input = match EditInput::parse(&invocation.arguments) {
        Ok(input) => input,
        Err(error) => return failed_result(error),
    };
    let target = resolve_path(context.workspace.root(), &input.path);
    let bytes = match std::fs::read(&target) {
        Ok(bytes) => bytes,
        Err(error) => return failed_result(format!("cannot read {}: {error}", target.display())),
    };
    let Ok(original_with_bom) = String::from_utf8(bytes) else {
        return failed_result(format!(
            "{} is not a UTF-8 text file; Edit never operates on binary content",
            target.display()
        ));
    };
    let (bom, original_body) = strip_bom(&original_with_bom);
    let ending = detect_line_ending(original_body);
    let normalized = normalize_to_lf(original_body);
    let planned = match plan(&normalized, &input.edits, &input.path) {
        Ok(planned) => planned,
        Err(error) => return failed_result(error),
    };
    let updated_normalized = if planned.used_fuzzy {
        apply_replacements_preserving_unchanged_lines(
            &normalized,
            &normalize_for_fuzzy_match(&normalized),
            &planned.edits,
        )
    } else {
        apply_replacements(&normalized, &planned.edits)
    };
    let updated_body = restore_line_endings(&updated_normalized, ending);
    let updated = format!("{bom}{updated_body}");
    if updated == original_with_bom {
        return failed_result(no_change_error(&input.path, input.edits.len()));
    }
    if let Err(error) = atomic_commit(&target, updated.as_bytes()) {
        return failed_result(error);
    }
    success_text(
        format!(
            "Successfully replaced {} block(s) in {}.",
            input.edits.len(),
            input.path
        ),
        None,
    )
}

struct PlannedEdits {
    edits: Vec<PlannedEdit>,
    used_fuzzy: bool,
}

struct PlannedEdit {
    index: usize,
    range: Range<usize>,
    replacement: String,
}

enum MatchTarget {
    Missing,
    Ambiguous(usize),
    Unique(Range<usize>),
}

fn plan(original: &str, edits: &[EditReplacement], path: &str) -> Result<PlannedEdits, String> {
    let normalized_edits: Vec<EditReplacementOwned> = edits
        .iter()
        .map(|edit| EditReplacementOwned {
            old_text: normalize_to_lf(&edit.old_text),
            new_text: normalize_to_lf(&edit.new_text),
        })
        .collect();

    let mut needs_fuzzy = false;
    for (index, edit) in normalized_edits.iter().enumerate() {
        match unique_match(original, &edit.old_text) {
            MatchTarget::Unique(_) => {}
            MatchTarget::Missing => needs_fuzzy = true,
            MatchTarget::Ambiguous(count) => {
                return Err(ambiguous_error(path, index, edits.len(), count));
            }
        }
    }

    let replacement_base = if needs_fuzzy {
        normalize_for_fuzzy_match(original)
    } else {
        original.to_owned()
    };
    let mut planned = Vec::with_capacity(normalized_edits.len());
    for (index, edit) in normalized_edits.iter().enumerate() {
        let old_text = if needs_fuzzy {
            normalize_for_fuzzy_match(&edit.old_text)
        } else {
            edit.old_text.clone()
        };
        let range = match unique_match(&replacement_base, &old_text) {
            MatchTarget::Unique(range) => range,
            MatchTarget::Missing => return Err(not_found_error(path, index, edits.len())),
            MatchTarget::Ambiguous(count) => {
                return Err(ambiguous_error(path, index, edits.len(), count));
            }
        };
        planned.push(PlannedEdit {
            index,
            range,
            replacement: edit.new_text.clone(),
        });
    }
    planned.sort_by_key(|edit| (edit.range.start, edit.index));
    for pair in planned.windows(2) {
        if pair[1].range.start < pair[0].range.end {
            return Err(format!(
                "edits[{}] and edits[{}] overlap in {path}. Merge them into one edit or target disjoint regions.",
                pair[0].index, pair[1].index
            ));
        }
    }
    Ok(PlannedEdits {
        edits: planned,
        used_fuzzy: needs_fuzzy,
    })
}

struct EditReplacementOwned {
    old_text: String,
    new_text: String,
}

/// Counts non-overlapping occurrences, matching `split(oldText).len() - 1`.
fn unique_match(content: &str, old_text: &str) -> MatchTarget {
    debug_assert!(!old_text.is_empty());
    let occurrences = content.split(old_text).count().saturating_sub(1);
    match occurrences {
        0 => MatchTarget::Missing,
        1 => content
            .find(old_text)
            .map_or(MatchTarget::Missing, |start| {
                MatchTarget::Unique(start..start + old_text.len())
            }),
        count => MatchTarget::Ambiguous(count),
    }
}

fn not_found_error(path: &str, index: usize, total: usize) -> String {
    if total == 1 {
        "Could not find the exact text in".to_owned()
            + &format!(
                " {path}. The old text must match exactly including all whitespace and newlines."
            )
    } else {
        format!(
            "Could not find edits[{index}] in {path}. The oldText must match exactly including all whitespace and newlines."
        )
    }
}

fn ambiguous_error(path: &str, index: usize, total: usize, occurrences: usize) -> String {
    if total == 1 {
        format!(
            "Found {occurrences} occurrences of the text in {path}. The text must be unique. Please provide more context to make it unique."
        )
    } else {
        format!(
            "Found {occurrences} occurrences of edits[{index}] in {path}. Each oldText must be unique. Please provide more context to make it unique."
        )
    }
}

fn no_change_error(path: &str, total: usize) -> String {
    if total == 1 {
        format!(
            "No changes made to {path}. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected."
        )
    } else {
        format!("No changes made to {path}. The replacements produced identical content.")
    }
}

fn apply_replacements(content: &str, replacements: &[PlannedEdit]) -> String {
    let mut result = content.to_owned();
    for replacement in replacements.iter().rev() {
        result.replace_range(replacement.range.clone(), replacement.replacement.as_str());
    }
    result
}

#[derive(Clone, Copy)]
struct LineSpan {
    start: usize,
    end: usize,
}

fn split_lines_with_endings(content: &str) -> Vec<&str> {
    content
        .match_indices('\n')
        .scan(0, |start, (index, _)| {
            let line = &content[*start..=index];
            *start = index + 1;
            Some(line)
        })
        .chain({
            let end = content.len();
            let last_start = content.rfind('\n').map_or(0, |index| index + 1);
            (last_start < end).then(|| &content[last_start..end])
        })
        .collect()
}

fn line_spans(content: &str) -> Vec<LineSpan> {
    let mut spans = Vec::new();
    let mut start = 0;
    for (index, _) in content.match_indices('\n') {
        spans.push(LineSpan {
            start,
            end: index + 1,
        });
        start = index + 1;
    }
    if start < content.len() {
        spans.push(LineSpan {
            start,
            end: content.len(),
        });
    }
    spans
}

fn replacement_line_range(lines: &[LineSpan], replacement: &PlannedEdit) -> (usize, usize) {
    let start = lines
        .iter()
        .position(|line| {
            replacement.range.start >= line.start && replacement.range.start < line.end
        })
        .expect("planned replacement starts inside the base content");
    let end_offset = replacement.range.end.saturating_sub(1);
    let end = lines
        .iter()
        .position(|line| end_offset >= line.start && end_offset < line.end)
        .expect("planned replacement ends inside the base content");
    (start, end + 1)
}

fn apply_replacements_preserving_unchanged_lines(
    original: &str,
    base: &str,
    replacements: &[PlannedEdit],
) -> String {
    let original_lines = split_lines_with_endings(original);
    let base_spans = line_spans(base);
    debug_assert_eq!(original_lines.len(), base_spans.len());
    let mut groups: Vec<(usize, usize, Vec<&PlannedEdit>)> = Vec::new();
    for replacement in replacements {
        let (start, end) = replacement_line_range(&base_spans, replacement);
        if let Some(group) = groups.last_mut()
            && start < group.1
        {
            group.1 = group.1.max(end);
            group.2.push(replacement);
        } else {
            groups.push((start, end, vec![replacement]));
        }
    }

    let mut result = String::new();
    let mut original_line = 0;
    for (start, end, group_replacements) in groups {
        result.push_str(&original_lines[original_line..start].concat());
        let group_start = base_spans[start].start;
        let group_end = base_spans[end - 1].end;
        let group = apply_replacements(
            &base[group_start..group_end],
            &group_replacements
                .iter()
                .map(|replacement| PlannedEdit {
                    index: replacement.index,
                    range: (replacement.range.start - group_start)
                        ..(replacement.range.end - group_start),
                    replacement: replacement.replacement.clone(),
                })
                .collect::<Vec<_>>(),
        );
        result.push_str(&group);
        original_line = end;
    }
    result.push_str(&original_lines[original_line..].concat());
    result
}

fn strip_bom(content: &str) -> (&str, &str) {
    content
        .strip_prefix('\u{FEFF}')
        .map_or(("", content), |body| ("\u{FEFF}", body))
}

#[derive(Clone, Copy)]
enum LineEnding {
    Lf,
    CrLf,
    Cr,
}

fn detect_line_ending(content: &str) -> LineEnding {
    let bytes = content.as_bytes();
    for index in 0..bytes.len() {
        match bytes[index] {
            b'\n' if index > 0 && bytes[index - 1] == b'\r' => return LineEnding::CrLf,
            b'\n' => return LineEnding::Lf,
            b'\r' if bytes.get(index + 1) != Some(&b'\n') => return LineEnding::Cr,
            _ => {}
        }
    }
    LineEnding::Lf
}

fn normalize_to_lf(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

fn restore_line_endings(text: &str, ending: LineEnding) -> String {
    match ending {
        LineEnding::Lf => text.to_owned(),
        LineEnding::CrLf => text.replace('\n', "\r\n"),
        LineEnding::Cr => text.replace('\n', "\r"),
    }
}

fn normalize_for_fuzzy_match(text: &str) -> String {
    let nfkc: String = text.nfkc().collect();
    nfkc.split('\n')
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .chars()
        .map(|character| match character {
            '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
            '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
            '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
            '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
            other => other,
        })
        .collect()
}
