//! Native Edit tool.
//!
//! Edit is a precise, atomic mutation primitive. All anchors are resolved
//! against one original snapshot, exact matching is preferred, and a
//! NFKC-based fuzzy fallback is used only when exact matching cannot find an
//! anchor for that individual edit. Any validation failure leaves the target
//! unchanged; runtime-owned `ManagedToolOutput` paths are read-only.

mod input;

use std::ops::Range;

use futures_util::future::BoxFuture;
use unicode_normalization::char::{canonical_combining_class, compose, decompose_compatible};

use crate::tools::executor::{ToolExecutionContext, ToolExecutor};
use crate::tools::native::registration::{NativeToolRegistration, native_definition};
use crate::tools::native::support::{
    atomic_commit, failed_result, interpret_path, prepare_mutation_target, success_text,
};
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
    let requested = interpret_path(context.workspace.root(), &input.path);
    let target = match prepare_mutation_target(&requested, context.tool_output) {
        Ok(target) => target,
        Err(error) => return failed_result(error),
    };
    let bytes = match std::fs::read(target.path()) {
        Ok(bytes) => bytes,
        Err(error) => {
            return failed_result(format!("cannot read {}: {error}", target.path().display()));
        }
    };
    let Ok(original_with_bom) = String::from_utf8(bytes) else {
        return failed_result(format!(
            "{} is not a UTF-8 text file; Edit never operates on binary content",
            target.path().display()
        ));
    };
    let (bom, original_body) = strip_bom(&original_with_bom);
    let ending = detect_line_ending(original_body);
    let normalized = normalize_to_lf(original_body);
    let planned = match plan(&normalized, &input.edits, &input.path) {
        Ok(planned) => planned,
        Err(error) => return failed_result(error),
    };
    // Every planned range is expressed in the original LF-normalized
    // snapshot. Applying directly to that snapshot preserves all source
    // representation outside the selected ranges, including text on a line
    // that required fuzzy matching elsewhere.
    let updated_normalized = apply_replacements(&normalized, &planned.edits);
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

    let mut fuzzy_projection = None;
    let mut planned = Vec::with_capacity(normalized_edits.len());
    for (index, edit) in normalized_edits.iter().enumerate() {
        // Matching strategy is selected independently for each edit. Once an
        // exact match is unique it is locked to the original snapshot and
        // never reconsidered in fuzzy space.
        let range = match unique_match(original, &edit.old_text) {
            MatchTarget::Unique(range) => range,
            MatchTarget::Ambiguous(count) => {
                return Err(ambiguous_error(path, index, edits.len(), count));
            }
            MatchTarget::Missing => {
                let projection =
                    fuzzy_projection.get_or_insert_with(|| FuzzyProjection::new(original));
                let fuzzy_old_text = normalize_for_fuzzy_match(&edit.old_text);
                if fuzzy_old_text.is_empty() {
                    return Err(not_found_error(path, index, edits.len()));
                }
                let fuzzy_range = match unique_match(&projection.text, &fuzzy_old_text) {
                    MatchTarget::Unique(range) => range,
                    MatchTarget::Missing => {
                        return Err(not_found_error(path, index, edits.len()));
                    }
                    MatchTarget::Ambiguous(count) => {
                        return Err(ambiguous_error(path, index, edits.len(), count));
                    }
                };
                match projection.original_range(&fuzzy_range) {
                    Ok(range) => range,
                    Err(FuzzyRangeError::Invalid) => {
                        return Err(not_found_error(path, index, edits.len()));
                    }
                    Err(FuzzyRangeError::Unsafe) => {
                        return Err(unsafe_fuzzy_error(path, index, edits.len()));
                    }
                }
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
    Ok(PlannedEdits { edits: planned })
}

struct EditReplacementOwned {
    old_text: String,
    new_text: String,
}

/// Counts non-overlapping occurrences, matching `split(oldText).len() - 1`.
fn unique_match(content: &str, old_text: &str) -> MatchTarget {
    if old_text.is_empty() {
        return MatchTarget::Missing;
    }
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

fn unsafe_fuzzy_error(path: &str, index: usize, total: usize) -> String {
    if total == 1 {
        format!(
            "Could not safely map the fuzzy match in {path} to one original source range. Please provide more context."
        )
    } else {
        format!(
            "Could not safely map edits[{index}] in {path} to one original source range. Please provide more context."
        )
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
struct SourceSpan {
    start: usize,
    end: usize,
}

/// A fuzzy-normalized projection with an explicit mapping for every
/// normalized Unicode scalar back to the original LF-normalized snapshot.
/// The projection is used only to locate a missing exact anchor; replacements
/// are always applied to the original string through `original_range`.
struct FuzzyProjection {
    text: String,
    byte_starts: Vec<usize>,
    source_spans: Vec<SourceSpan>,
}

enum FuzzyRangeError {
    Invalid,
    Unsafe,
}

impl FuzzyProjection {
    fn new(source: &str) -> Self {
        let projected: Vec<_> = nfkc_with_mapping(source)
            .into_iter()
            .map(|normalized| (normalized.character, normalized.span))
            .collect();

        let mut trimmed = Vec::with_capacity(projected.len());
        let mut line: Vec<(char, SourceSpan)> = Vec::new();
        for (character, span) in projected {
            if character == '\n' {
                while line
                    .last()
                    .is_some_and(|(character, _)| character.is_whitespace())
                {
                    line.pop();
                }
                trimmed.append(&mut line);
                trimmed.push((character, span));
            } else {
                line.push((character, span));
            }
        }
        while line
            .last()
            .is_some_and(|(character, _)| character.is_whitespace())
        {
            line.pop();
        }
        trimmed.append(&mut line);

        let mut text = String::new();
        let mut byte_starts = Vec::with_capacity(trimmed.len());
        let mut source_spans = Vec::with_capacity(trimmed.len());
        for (character, span) in trimmed {
            byte_starts.push(text.len());
            text.push(fuzzy_character(character));
            source_spans.push(span);
        }
        Self {
            text,
            byte_starts,
            source_spans,
        }
    }

    fn original_range(
        &self,
        normalized_range: &Range<usize>,
    ) -> Result<Range<usize>, FuzzyRangeError> {
        if normalized_range.is_empty() || normalized_range.end > self.text.len() {
            return Err(FuzzyRangeError::Invalid);
        }
        let start = self
            .byte_starts
            .binary_search(&normalized_range.start)
            .map_err(|_| FuzzyRangeError::Invalid)?;
        let end = if normalized_range.end == self.text.len() {
            self.source_spans.len()
        } else {
            self.byte_starts
                .binary_search(&normalized_range.end)
                .map_err(|_| FuzzyRangeError::Invalid)?
        };
        if start >= end {
            return Err(FuzzyRangeError::Invalid);
        }
        let matched_spans = &self.source_spans[start..end];
        let Some(candidate_start) = matched_spans.iter().map(|span| span.start).min() else {
            return Err(FuzzyRangeError::Invalid);
        };
        let Some(candidate_end) = matched_spans.iter().map(|span| span.end).max() else {
            return Err(FuzzyRangeError::Invalid);
        };

        // A normalized scalar outside the selected match must not claim any
        // source byte that the candidate replacement would remove. This is
        // essential for compatibility expansions such as `ﬃ` -> `ffi` and
        // for canonical reordering, where a normalized substring can span
        // source material represented by another normalized scalar.
        let unsafe_overlap = self.source_spans.iter().enumerate().any(|(index, span)| {
            let outside_match = index < start || index >= end;
            outside_match && span.start < candidate_end && candidate_start < span.end
        });
        if unsafe_overlap {
            return Err(FuzzyRangeError::Unsafe);
        }

        Ok(candidate_start..candidate_end)
    }
}

#[derive(Clone, Copy)]
struct LabeledCharacter {
    character: char,
    span: SourceSpan,
}

/// Produces NFKC while retaining an explicit source span for every normalized
/// scalar. Decomposition is labeled before canonical reordering and
/// recomposition, so both compatibility expansion (for example `ﬃ` -> `ffi`)
/// and composition across source-character boundaries retain enough mapping
/// information to address the original snapshot.
fn nfkc_with_mapping(source: &str) -> Vec<LabeledCharacter> {
    let mut decomposed = Vec::new();
    for (start, character) in source.char_indices() {
        let span = SourceSpan {
            start,
            end: start + character.len_utf8(),
        };
        decompose_compatible(character, |decomposed_character| {
            decomposed.push(LabeledCharacter {
                character: decomposed_character,
                span,
            });
        });
    }

    // Match the normalization crate's stable canonical reordering while
    // preserving the source span carried by every decomposed scalar.
    let mut ordered = Vec::with_capacity(decomposed.len());
    let mut pending: Vec<LabeledCharacter> = Vec::new();
    for labeled in decomposed {
        if canonical_combining_class(labeled.character) == 0 {
            pending.sort_by_key(|entry| canonical_combining_class(entry.character));
            ordered.append(&mut pending);
            ordered.push(labeled);
        } else {
            pending.push(labeled);
        }
    }
    pending.sort_by_key(|entry| canonical_combining_class(entry.character));
    ordered.append(&mut pending);

    let mut normalized = Vec::with_capacity(ordered.len());
    let mut composee = None;
    let mut last_combining_class = None;
    let mut blocked: Vec<LabeledCharacter> = Vec::new();
    for labeled in ordered {
        let combining_class = canonical_combining_class(labeled.character);
        let Some(current) = composee else {
            if combining_class == 0 {
                composee = Some(labeled);
            } else {
                normalized.push(labeled);
            }
            continue;
        };

        match last_combining_class {
            None => match compose(current.character, labeled.character) {
                Some(result) => {
                    composee = Some(LabeledCharacter {
                        character: result,
                        span: merge_spans(current.span, labeled.span),
                    });
                }
                None if combining_class == 0 => {
                    normalized.push(current);
                    composee = Some(labeled);
                }
                None => {
                    blocked.push(labeled);
                    last_combining_class = Some(combining_class);
                }
            },
            Some(previous_class) => {
                if previous_class >= combining_class {
                    if combining_class == 0 {
                        normalized.push(current);
                        normalized.append(&mut blocked);
                        composee = Some(labeled);
                        last_combining_class = None;
                    } else {
                        blocked.push(labeled);
                        last_combining_class = Some(combining_class);
                    }
                } else if let Some(result) = compose(current.character, labeled.character) {
                    composee = Some(LabeledCharacter {
                        character: result,
                        span: merge_spans(current.span, labeled.span),
                    });
                } else {
                    blocked.push(labeled);
                    last_combining_class = Some(combining_class);
                }
            }
        }
    }
    if let Some(current) = composee {
        normalized.push(current);
    }
    normalized.append(&mut blocked);
    normalized
}

fn merge_spans(left: SourceSpan, right: SourceSpan) -> SourceSpan {
    SourceSpan {
        start: left.start.min(right.start),
        end: left.end.max(right.end),
    }
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
    FuzzyProjection::new(text).text
}

fn fuzzy_character(character: char) -> char {
    match character {
        '\u{2018}' | '\u{2019}' | '\u{201A}' | '\u{201B}' => '\'',
        '\u{201C}' | '\u{201D}' | '\u{201E}' | '\u{201F}' => '"',
        '\u{2010}'..='\u{2015}' | '\u{2212}' => '-',
        '\u{00A0}' | '\u{2002}'..='\u{200A}' | '\u{202F}' | '\u{205F}' | '\u{3000}' => ' ',
        other => other,
    }
}
