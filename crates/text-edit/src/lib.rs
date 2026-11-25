//! The pure rustX multi-edit algorithm shared by the Agent's Edit tool
//! and the Execution Client's conditional `PrepareEdit`.
//!
//! One [`apply`] call plans every replacement against the *original*
//! LF-normalized snapshot — BOM stripped and later restored, the
//! dominant line ending detected and later restored — preferring exact
//! unique anchors and falling back to a bounded NFKC-based fuzzy match
//! per edit. Ranges are expressed in the original snapshot, so source
//! representation outside the selected ranges is preserved byte for
//! byte, including text on a line that required fuzzy matching
//! elsewhere. Every failure is a stable typed [`EditFailure`]; user- or
//! wire-facing wording is the caller's business and never lives here.
//!
//! The resource contract is explicit and enforced *before* large
//! allocations: the source and the result are capped at
//! [`MAX_EDIT_FILE_BYTES`], a request at [`MAX_EDIT_REPLACEMENTS`]
//! replacements with at most [`MAX_EDIT_REQUEST_BYTES`] aggregate
//! old+new bytes, and each fuzzy provenance buffer — decomposition,
//! the combining-mark reorder window, the blocked list, the normalized
//! output — is individually capped at
//! [`MAX_FUZZY_PROJECTION_ENTRIES`] labeled scalars, so a pathological
//! NFKC expansion or combining-mark run refuses
//! [`EditFailure::TooLarge`] rather than growing a buffer without
//! bound. That is a per-buffer count bound, not a memory guarantee:
//! the mapping is a small constant number of such buffers.

use std::ops::Range;

use unicode_normalization::char::{canonical_combining_class, compose, decompose_compatible};

// The wire DTOs and the wire bounds are owned by the protocol crate —
// one definition, no second set of limits or conversion glue. This
// kernel's public API re-exports them under the algorithm's own names.
pub use rustx_execution_protocol::EditRefusal as EditFailure;
pub use rustx_execution_protocol::TextReplacement as Replacement;
pub use rustx_execution_protocol::{
    MAX_EDIT_FILE_BYTES, MAX_EDIT_REPLACEMENTS, MAX_EDIT_REQUEST_BYTES,
};

/// The entry bound on each fuzzy-projection provenance buffer —
/// decomposed, reordered, blocked, and normalized buffers alike. NFKC
/// expansions and combining-mark runs count against the budget before
/// they land in a buffer, so a pathological source refuses rather than
/// growing a mapping without bound (~24 bytes per entry).
pub const MAX_FUZZY_PROJECTION_ENTRIES: usize = 1_048_576;

/// Applies `edits` to `original`, returning the full updated content.
///
/// The request and source bounds are checked before any output or
/// fuzzy mapping is allocated; every typed failure leaves the caller
/// with no partial result.
///
/// # Errors
///
/// Every [`EditFailure`] variant is reachable as documented on the
/// variants; `TooLarge` covers the resource bounds described above.
#[allow(clippy::too_many_lines)] // one validated pipeline, kept linear
pub fn apply(original: &str, edits: &[Replacement]) -> Result<String, EditFailure> {
    if edits.is_empty() {
        return Err(EditFailure::EmptyEdits);
    }
    if edits.len() > MAX_EDIT_REPLACEMENTS {
        return Err(EditFailure::TooLarge);
    }
    let request_bytes = edits.iter().fold(0_usize, |total, edit| {
        total
            .saturating_add(edit.old_text.len())
            .saturating_add(edit.new_text.len())
    });
    if request_bytes > MAX_EDIT_REQUEST_BYTES {
        return Err(EditFailure::TooLarge);
    }
    if let Some(at) = edits.iter().position(|edit| edit.old_text.is_empty()) {
        return Err(EditFailure::EmptyAnchor { index: index(at) });
    }
    if original.len() > MAX_EDIT_FILE_BYTES {
        return Err(EditFailure::TooLarge);
    }
    let (bom, body) = strip_bom(original);
    let ending = detect_line_ending(body);
    let normalized = normalize_to_lf(body);
    let planned = plan(&normalized, edits)?;
    // One forward projection over the sorted disjoint ranges: the exact
    // output length — every unchanged segment and replacement, each
    // LF-normalized newline re-expanded to the dominant ending, plus the
    // BOM — is proven under the cap with checked sums before the output
    // is allocated. A newline inside a replaced range does not count:
    // it left the output with the rest of that range.
    let count_newlines = |text: &str| text.bytes().filter(|byte| *byte == b'\n').count();
    let segment_len = |text: &str| -> Option<usize> {
        text.len()
            .checked_add(count_newlines(text).checked_mul(newline_extra(ending))?)
    };
    let mut out_len = bom.len();
    let mut cursor = 0;
    for edit in &planned {
        out_len = out_len
            .checked_add(
                segment_len(&normalized[cursor..edit.range.start])
                    .and_then(|segment| segment.checked_add(segment_len(&edit.replacement)?))
                    .ok_or(EditFailure::TooLarge)?,
            )
            .ok_or(EditFailure::TooLarge)?;
        cursor = edit.range.end;
    }
    out_len = out_len
        .checked_add(segment_len(&normalized[cursor..]).ok_or(EditFailure::TooLarge)?)
        .ok_or(EditFailure::TooLarge)?;
    if out_len > MAX_EDIT_FILE_BYTES {
        return Err(EditFailure::TooLarge);
    }
    let mut updated = String::with_capacity(out_len);
    updated.push_str(bom);
    let mut cursor = 0;
    for edit in &planned {
        append_segment(&mut updated, &normalized[cursor..edit.range.start], ending);
        append_segment(&mut updated, &edit.replacement, ending);
        cursor = edit.range.end;
    }
    append_segment(&mut updated, &normalized[cursor..], ending);
    if updated == original {
        return Err(EditFailure::NoChange);
    }
    Ok(updated)
}

/// The extra bytes one `\n` costs when restored in `ending`: CRLF adds
/// a `\r` per newline; bare-CR and LF keep the width.
fn newline_extra(ending: LineEnding) -> usize {
    match ending {
        LineEnding::CrLf => 1,
        LineEnding::Lf | LineEnding::Cr => 0,
    }
}

/// Appends `text` to `out`, re-expanding each `\n` into `ending`.
fn append_segment(out: &mut String, text: &str, ending: LineEnding) {
    match ending {
        LineEnding::Lf => out.push_str(text),
        LineEnding::CrLf => {
            for (index, part) in text.split('\n').enumerate() {
                if index > 0 {
                    out.push_str("\r\n");
                }
                out.push_str(part);
            }
        }
        LineEnding::Cr => {
            for (index, part) in text.split('\n').enumerate() {
                if index > 0 {
                    out.push('\r');
                }
                out.push_str(part);
            }
        }
    }
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

fn index(index: usize) -> u32 {
    u32::try_from(index).unwrap_or(u32::MAX)
}

fn plan(original: &str, edits: &[Replacement]) -> Result<Vec<PlannedEdit>, EditFailure> {
    let normalized_edits: Vec<Replacement> = edits
        .iter()
        .map(|edit| Replacement {
            old_text: normalize_to_lf(&edit.old_text),
            new_text: normalize_to_lf(&edit.new_text),
        })
        .collect();

    let mut fuzzy_projection = None;
    let mut planned = Vec::with_capacity(normalized_edits.len());
    for (edit_index, edit) in normalized_edits.iter().enumerate() {
        // Matching strategy is selected independently for each edit. Once an
        // exact match is unique it is locked to the original snapshot and
        // never reconsidered in fuzzy space.
        let range = match unique_match(original, &edit.old_text) {
            MatchTarget::Unique(range) => range,
            MatchTarget::Ambiguous(count) => {
                return Err(EditFailure::Ambiguous {
                    index: index(edit_index),
                    occurrences: u32::try_from(count).unwrap_or(u32::MAX),
                });
            }
            MatchTarget::Missing => {
                let projection = fuzzy_projection
                    .get_or_insert_with(|| FuzzyProjection::new(original))
                    .as_ref()
                    .map_err(|failure| *failure)?;
                let fuzzy_old_text = normalize_for_fuzzy_match(&edit.old_text)?;
                if fuzzy_old_text.is_empty() {
                    return Err(EditFailure::Missing {
                        index: index(edit_index),
                    });
                }
                let fuzzy_range = match unique_match(&projection.text, &fuzzy_old_text) {
                    MatchTarget::Unique(range) => range,
                    MatchTarget::Missing => {
                        return Err(EditFailure::Missing {
                            index: index(edit_index),
                        });
                    }
                    MatchTarget::Ambiguous(count) => {
                        return Err(EditFailure::Ambiguous {
                            index: index(edit_index),
                            occurrences: u32::try_from(count).unwrap_or(u32::MAX),
                        });
                    }
                };
                match projection.original_range(&fuzzy_range) {
                    Ok(range) => range,
                    Err(FuzzyRangeError::Invalid) => {
                        return Err(EditFailure::Missing {
                            index: index(edit_index),
                        });
                    }
                    Err(FuzzyRangeError::Unsafe) => {
                        return Err(EditFailure::UnsafeNormalization {
                            index: index(edit_index),
                        });
                    }
                }
            }
        };
        planned.push(PlannedEdit {
            index: edit_index,
            range,
            replacement: edit.new_text.clone(),
        });
    }
    planned.sort_by_key(|edit| (edit.range.start, edit.index));
    for pair in planned.windows(2) {
        if pair[1].range.start < pair[0].range.end {
            return Err(EditFailure::Overlap {
                first: index(pair[0].index),
                second: index(pair[1].index),
            });
        }
    }
    Ok(planned)
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

#[derive(Clone, Copy)]
struct SourceSpan {
    start: usize,
    end: usize,
}

/// A fuzzy-normalized projection with an explicit mapping for every
/// normalized Unicode scalar back to the original LF-normalized snapshot.
/// The projection is used only to locate a missing exact anchor; replacements
/// are always applied to the original string through `original_range`.
///
/// Construction is bounded: building the provenance vectors counts
/// entries — NFKC expansions and combining-mark reorder buffers alike —
/// against [`MAX_FUZZY_PROJECTION_ENTRIES`] and refuses pathological
/// sources instead of growing the mapping without bound.
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
    fn new(source: &str) -> Result<Self, EditFailure> {
        let projected = nfkc_with_mapping(source)?;
        let projected: Vec<_> = projected
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
        Ok(Self {
            text,
            byte_starts,
            source_spans,
        })
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
///
/// Every buffer is counted against [`MAX_FUZZY_PROJECTION_ENTRIES`]:
/// decomposition output, the combining-mark reorder `pending` window,
/// the blocked list, and the normalized result. A source whose provenance
/// would exceed the bound refuses `TooLarge` before the mapping grows —
/// a count bound, not a memory guarantee.
/// # Errors
///
/// Returns [`EditFailure::TooLarge`] when any provenance buffer would
/// exceed [`MAX_FUZZY_PROJECTION_ENTRIES`].
#[allow(clippy::too_many_lines)] // the bounded reorder/recompose loop, kept linear
fn nfkc_with_mapping(source: &str) -> Result<Vec<LabeledCharacter>, EditFailure> {
    let mut entries = 0_usize;
    let mut decomposed = Vec::new();
    for (start, character) in source.char_indices() {
        let span = SourceSpan {
            start,
            end: start + character.len_utf8(),
        };
        decompose_compatible(character, |decomposed_character| {
            entries = entries.saturating_add(1);
            if entries <= MAX_FUZZY_PROJECTION_ENTRIES {
                decomposed.push(LabeledCharacter {
                    character: decomposed_character,
                    span,
                });
            }
        });
        if entries > MAX_FUZZY_PROJECTION_ENTRIES {
            return Err(EditFailure::TooLarge);
        }
    }

    // Match the normalization crate's stable canonical reordering while
    // preserving the source span carried by every decomposed scalar. The
    // `pending` window is the reorder buffer: a pathological run of
    // combining marks counts against the same entry bound rather than
    // growing a per-segment sort without limit.
    let mut ordered = Vec::with_capacity(decomposed.len());
    let mut pending: Vec<LabeledCharacter> = Vec::new();
    for labeled in decomposed {
        if canonical_combining_class(labeled.character) == 0 {
            pending.sort_by_key(|entry| canonical_combining_class(entry.character));
            ordered.append(&mut pending);
            ordered.push(labeled);
        } else {
            if pending.len() >= MAX_FUZZY_PROJECTION_ENTRIES {
                return Err(EditFailure::TooLarge);
            }
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
                if normalized.len() >= MAX_FUZZY_PROJECTION_ENTRIES {
                    return Err(EditFailure::TooLarge);
                }
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
                    if normalized.len() >= MAX_FUZZY_PROJECTION_ENTRIES {
                        return Err(EditFailure::TooLarge);
                    }
                    normalized.push(current);
                    composee = Some(labeled);
                }
                None => {
                    if blocked.len() >= MAX_FUZZY_PROJECTION_ENTRIES {
                        return Err(EditFailure::TooLarge);
                    }
                    blocked.push(labeled);
                    last_combining_class = Some(combining_class);
                }
            },
            Some(previous_class) => {
                if previous_class >= combining_class {
                    if combining_class == 0 {
                        if normalized.len() >= MAX_FUZZY_PROJECTION_ENTRIES {
                            return Err(EditFailure::TooLarge);
                        }
                        normalized.push(current);
                        normalized.append(&mut blocked);
                        composee = Some(labeled);
                        last_combining_class = None;
                    } else {
                        if blocked.len() >= MAX_FUZZY_PROJECTION_ENTRIES {
                            return Err(EditFailure::TooLarge);
                        }
                        blocked.push(labeled);
                        last_combining_class = Some(combining_class);
                    }
                } else if let Some(result) = compose(current.character, labeled.character) {
                    composee = Some(LabeledCharacter {
                        character: result,
                        span: merge_spans(current.span, labeled.span),
                    });
                } else {
                    if blocked.len() >= MAX_FUZZY_PROJECTION_ENTRIES {
                        return Err(EditFailure::TooLarge);
                    }
                    blocked.push(labeled);
                    last_combining_class = Some(combining_class);
                }
            }
        }
    }
    if let Some(current) = composee {
        if normalized.len() >= MAX_FUZZY_PROJECTION_ENTRIES {
            return Err(EditFailure::TooLarge);
        }
        normalized.push(current);
    }
    normalized.append(&mut blocked);
    Ok(normalized)
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

/// The dominant style is the first line ending observed: a mixed file
/// is normalized entirely to LF, so lines that were already LF join
/// the detected style on the way back out — `apply` never preserves a
/// per-line mixture.
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

/// Normalizes one edit anchor into the fuzzy view. A `TooLarge`
/// projection propagates as the resource refusal it is — never a
/// `Missing` that would hide an allocation refusal.
fn normalize_for_fuzzy_match(text: &str) -> Result<String, EditFailure> {
    FuzzyProjection::new(text).map(|projection| projection.text)
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
#[cfg(test)]
mod tests;
