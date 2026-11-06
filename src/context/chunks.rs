//! Bounded, ordered fragments of one captured historical projection.
//!
//! Each occurrence is rendered once. Its offsets address that rendering,
//! independently of the separators used when several occurrences are sent.
//! Only an accepted request can move the cursor.

use std::collections::BTreeMap;
use std::io::{self, Write};

use serde::{Deserialize, Serialize};

use super::checkpoint::SummaryGenerationRef;
use super::error::{ContextError, ContextErrorKind};
use super::resources::ResourceLimits;
use super::summarizer::{SummaryGeneration, SummaryRequest, append_summary_message};
use crate::message::types::AssistantContentBlock;
use crate::model::SelectedModelInput;
use crate::model::auxiliary::{
    AUXILIARY_SOURCE_RANGE_LIMIT_MAX, AuxiliarySourceRange, AuxiliarySourceRole,
};
use crate::model::projected::ModelMessageView;

/// Detached, ordered portions of captured projected sources.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryFragment {
    pub text: String,
    pub ranges: Vec<AuxiliarySourceRange>,
}

/// The separately attributed accepted result of the preceding Summary Call.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SummaryCarryover {
    pub text: String,
    pub generation: Option<SummaryGenerationRef>,
}

#[derive(Debug)]
struct Source {
    text: String,
    role: AuxiliarySourceRole,
    /// No `ToolCall` from the captured prefix remains without its result.
    group_end: bool,
}

#[derive(Clone, Copy)]
struct Endpoint {
    index: usize,
    end: usize,
}

/// Captured one-time renderings and the first unaccepted position.
pub(crate) struct ChunkPlan {
    sources: Vec<Source>,
    index: usize,
    offset: usize,
    bound: usize,
    accepted: usize,
}

/// Maximum exact searches for an estimator without a monotonicity contract.
const MAX_EXACT_SPLIT_CANDIDATES: usize = 256;
/// Compaction itself will never be allowed more than this many accepted chunks.
const MAX_PLANNED_CHUNKS: usize = 256;

fn failure(kind: ContextErrorKind, message: &str) -> ContextError {
    ContextError::new(kind, message)
}

impl ChunkPlan {
    /// Captures an ordered prefix within the same finite logical limits used by
    /// compaction resources. Neither message bodies nor spill files are cloned.
    pub(crate) fn new(
        input: &SelectedModelInput,
        retired_count: usize,
    ) -> Result<Self, ContextError> {
        let limits = ResourceLimits::default();
        if retired_count > input.len() || retired_count > limits.total_items {
            return Err(failure(
                ContextErrorKind::CannotFit,
                "compaction source count exceeds its captured bounds",
            ));
        }

        let mut sources = Vec::with_capacity(retired_count);
        let mut pending = BTreeMap::<String, usize>::new();
        let mut pending_count = 0_usize;
        let mut encoded_total = 0_usize;
        let mut rendered_total = 0_usize;
        let mut progress_units = 0_usize;
        for message in input.iter().take(retired_count) {
            if matches!(message.view(), ModelMessageView::RequestOnly(_)) {
                return Err(failure(
                    ContextErrorKind::MalformedHistory,
                    "request-only input cannot be retired as compaction history",
                ));
            }
            // Stream the encoding into a counting writer before rendering. This
            // bounds temporary render allocation as well as the retained text.
            let available = limits
                .record_bytes
                .min(limits.total_bytes.saturating_sub(encoded_total));
            let mut counter = BoundedWriter {
                bytes: 0,
                limit: available,
            };
            serde_json::to_writer(&mut counter, message).map_err(|_| {
                failure(
                    ContextErrorKind::CannotFit,
                    "compaction source body exceeds its captured bounds",
                )
            })?;
            encoded_total += counter.bytes;
            let role = match message.view() {
                ModelMessageView::User(_) => AuxiliarySourceRole::User,
                ModelMessageView::Assistant(content) => {
                    for block in content {
                        if let AssistantContentBlock::ToolCall(call) = block {
                            *pending.entry(call.id.to_string()).or_default() += 1;
                            pending_count += 1;
                        }
                    }
                    AuxiliarySourceRole::Assistant
                }
                ModelMessageView::Tool(call, _) => {
                    let Some(count) = pending.get_mut(call.as_str()) else {
                        return Err(failure(
                            ContextErrorKind::MalformedHistory,
                            "orphan projected Tool result in compaction source",
                        ));
                    };
                    *count -= 1;
                    pending_count -= 1;
                    if *count == 0 {
                        pending.remove(call.as_str());
                    }
                    AuxiliarySourceRole::Tool
                }
                ModelMessageView::RequestOnly(_) => unreachable!("request-only input was rejected"),
            };
            let mut text = String::new();
            append_summary_message(message, &mut text);
            rendered_total = rendered_total.checked_add(text.len()).ok_or_else(|| {
                failure(
                    ContextErrorKind::CannotFit,
                    "compaction source rendering exceeds its captured bounds",
                )
            })?;
            if text.len() > limits.record_bytes || rendered_total > limits.total_bytes {
                return Err(failure(
                    ContextErrorKind::CannotFit,
                    "compaction source rendering exceeds its captured bounds",
                ));
            }
            progress_units = progress_units.saturating_add(text.len().max(1));
            sources.push(Source {
                text,
                role,
                group_end: pending_count == 0,
            });
        }
        if pending_count != 0 {
            return Err(failure(
                ContextErrorKind::MalformedHistory,
                "compaction source ends inside a Tool call/result group",
            ));
        }
        Ok(Self {
            sources,
            index: 0,
            offset: 0,
            bound: progress_units.min(MAX_PLANNED_CHUNKS),
            accepted: 0,
        })
    }

    /// A finite upper bound on accepted chunks, derived from captured source
    /// bytes (or one unit per empty occurrence), with an explicit global cap.
    #[must_use]
    pub(crate) fn planned_bound(&self) -> usize {
        self.bound
    }

    #[must_use]
    pub(crate) fn is_complete(&self) -> bool {
        self.index == self.sources.len()
    }

    /// Selects a complete-message/group boundary first, then a UTF-8-safe
    /// source prefix when no complete boundary fits. `monotone` is a promise
    /// from the estimator that extending this exact request cannot lower cost.
    /// Unknown estimators are searched exactly within a finite query budget.
    #[allow(clippy::too_many_arguments, clippy::too_many_lines)] // Keep ordered boundary and UTF-8 fallback search together.
    pub(crate) fn next(
        &self,
        previous: Option<&SummaryGeneration>,
        focus: Option<&str>,
        output_tokens: Option<u32>,
        input_limit: u64,
        monotone: bool,
        estimate: impl Fn(&SummaryRequest) -> Result<u64, ContextError>,
    ) -> Result<SummaryRequest, ContextError> {
        if self.is_complete() {
            return Err(failure(
                ContextErrorKind::NoProgress,
                "compaction source is already complete",
            ));
        }
        if self.accepted >= self.bound {
            return Err(failure(
                ContextErrorKind::CompactionBudgetExceeded,
                "compaction source chunk limit exhausted",
            ));
        }
        let max_body = ResourceLimits::default().record_bytes;
        if previous.is_some_and(|value| value.text.len() > max_body)
            || focus.is_some_and(|value| value.len() > max_body)
        {
            return Err(failure(
                ContextErrorKind::CannotFit,
                "compaction carryover or focus exceeds its captured body bound",
            ));
        }
        let end_index = self
            .sources
            .len()
            .min(self.index + AUXILIARY_SOURCE_RANGE_LIMIT_MAX);
        let endpoints: Vec<_> = (self.index..end_index)
            .map(|index| Endpoint {
                index,
                end: self.sources[index].text.len(),
            })
            .collect();
        let request = |end: Endpoint| self.request(end, previous, focus, output_tokens);
        let fits = |end: Endpoint| -> Result<bool, ContextError> {
            Ok(estimate(&request(end))? <= input_limit)
        };

        // Complete tool groups are preferred. If none fit, a complete message
        // remains preferable to splitting detached text.
        let groups: Vec<_> = endpoints
            .iter()
            .copied()
            .filter(|end| self.sources[end.index].group_end)
            .collect();
        if let Some(end) = best_endpoint(&groups, monotone, &fits)? {
            return Ok(request(end));
        }
        let messages: Vec<_> = endpoints
            .iter()
            .copied()
            .filter(|end| !self.sources[end.index].group_end)
            .collect();
        if let Some(end) = best_endpoint(&messages, monotone, &fits)? {
            return Ok(request(end));
        }

        if monotone {
            // The first whole source is known not to fit. Search its interior
            // byte boundaries without splitting a UTF-8 codepoint.
            let source = &self.sources[self.index].text;
            let mut low = self.offset;
            let mut high = source.len();
            while low < high {
                let mut next = low + 1;
                while next < high && !source.is_char_boundary(next) {
                    next += 1;
                }
                if next >= high {
                    break;
                }
                let mut middle = low + (high - low) / 2;
                while middle > low && !source.is_char_boundary(middle) {
                    middle -= 1;
                }
                if middle == low {
                    middle = next;
                }
                if fits(Endpoint {
                    index: self.index,
                    end: middle,
                })? {
                    low = middle;
                } else {
                    high = middle;
                }
            }
            if low > self.offset {
                return Ok(request(Endpoint {
                    index: self.index,
                    end: low,
                }));
            }
        } else {
            // Scan every possible interior endpoint in descending order. When
            // the budget runs out, no claim about unchecked larger prefixes is
            // justified, even if a smaller sampled candidate would have fit.
            let mut examined = 0;
            for index in (self.index..end_index).rev() {
                let source = &self.sources[index].text;
                let start = if index == self.index { self.offset } else { 0 };
                for (end, _) in source.char_indices().rev() {
                    if end <= start {
                        break;
                    }
                    if examined == MAX_EXACT_SPLIT_CANDIDATES {
                        return Err(failure(
                            ContextErrorKind::SearchIncomplete,
                            "compaction fragment exact-search budget exhausted",
                        ));
                    }
                    examined += 1;
                    let end = Endpoint { index, end };
                    if fits(end)? {
                        return Ok(request(end));
                    }
                }
            }
        }
        Err(failure(
            ContextErrorKind::CannotFit,
            "no positive compaction source range fits the complete summary request",
        ))
    }

    fn request(
        &self,
        endpoint: Endpoint,
        previous: Option<&SummaryGeneration>,
        focus: Option<&str>,
        output_tokens: Option<u32>,
    ) -> SummaryRequest {
        let mut ranges = Vec::with_capacity(endpoint.index - self.index + 1);
        let mut text = String::new();
        for index in self.index..=endpoint.index {
            let source = &self.sources[index];
            let start = if index == self.index { self.offset } else { 0 };
            let end = if index == endpoint.index {
                endpoint.end
            } else {
                source.text.len()
            };
            let slice = &source.text[start..end];
            if !slice.is_empty() {
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(slice);
            }
            ranges.push(AuxiliarySourceRange {
                projection_index: u32::try_from(index).expect("source count is bounded"),
                role: source.role,
                byte_start: u32::try_from(start).expect("source body is bounded"),
                byte_end: u32::try_from(end).expect("source body is bounded"),
            });
        }
        SummaryRequest {
            retired: Vec::new(),
            fragment: Some(SummaryFragment { text, ranges }),
            previous: previous.map(|value| SummaryCarryover {
                text: value.text.clone(),
                generation: value.generation.clone(),
            }),
            focus: focus.map(str::to_owned),
            output_tokens,
        }
    }

    /// Advances only after the exact contiguous source slices were accepted.
    pub(crate) fn accept(&mut self, request: &SummaryRequest) -> Result<(), ContextError> {
        let invalid = || {
            failure(
                ContextErrorKind::NoProgress,
                "accepted compaction fragment does not match the next contiguous source ranges",
            )
        };
        let fragment = request.fragment.as_ref().ok_or_else(invalid)?;
        if !request.retired.is_empty()
            || fragment.ranges.is_empty()
            || fragment.ranges.len() > AUXILIARY_SOURCE_RANGE_LIMIT_MAX
            || self.is_complete()
            || self.accepted >= self.bound
        {
            return Err(invalid());
        }
        let mut index = self.index;
        let mut offset = self.offset;
        let mut text = String::new();
        for (position, range) in fragment.ranges.iter().enumerate() {
            let Some(source) = self.sources.get(index) else {
                return Err(invalid());
            };
            let start = range.byte_start as usize;
            let end = range.byte_end as usize;
            if range.projection_index as usize != index
                || range.role != source.role
                || start != offset
                || end > source.text.len()
                || end < start
                || !source.text.is_char_boundary(start)
                || !source.text.is_char_boundary(end)
                || (end == start && !source.text.is_empty())
            {
                return Err(invalid());
            }
            let slice = &source.text[start..end];
            if !slice.is_empty() {
                if !text.is_empty() {
                    text.push_str("\n\n");
                }
                text.push_str(slice);
            }
            if end == source.text.len() {
                index += 1;
                offset = 0;
            } else {
                if position + 1 != fragment.ranges.len() {
                    return Err(invalid());
                }
                offset = end;
            }
        }
        if text != fragment.text {
            return Err(invalid());
        }
        self.index = index;
        self.offset = offset;
        self.accepted += 1;
        Ok(())
    }
}

fn best_endpoint(
    endpoints: &[Endpoint],
    monotone: bool,
    fits: &impl Fn(Endpoint) -> Result<bool, ContextError>,
) -> Result<Option<Endpoint>, ContextError> {
    if monotone {
        let (mut low, mut high) = (0, endpoints.len());
        while low < high {
            let middle = low + (high - low) / 2;
            if fits(endpoints[middle])? {
                low = middle + 1;
            } else {
                high = middle;
            }
        }
        Ok(low.checked_sub(1).map(|index| endpoints[index]))
    } else {
        for &endpoint in endpoints.iter().rev() {
            if fits(endpoint)? {
                return Ok(Some(endpoint));
            }
        }
        Ok(None)
    }
}

struct BoundedWriter {
    bytes: usize,
    limit: usize,
}
impl Write for BoundedWriter {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        if buffer.len() > self.limit.saturating_sub(self.bytes) {
            return Err(io::Error::other("compaction source limit"));
        }
        self.bytes += buffer.len();
        Ok(buffer.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::content::TextBlock;
    use crate::message::types::{
        AssistantMessageBlock, ContentBlockIndex, InboundKind, MessageBlock, ToolCallOccurrenceRef,
        ToolMessageBlock, UserContentBlock, UserMessageBlock, UserSource,
    };
    use crate::runtime::identity::{MessageId, ToolCallId, ToolId};
    use crate::tools::types::{
        ToolCall, ToolExecutionResult, ToolExecutionStatus, ToolResultContent,
    };

    fn user(text: &str) -> MessageBlock {
        MessageBlock::User(UserMessageBlock {
            id: MessageId::new("user"),
            content: vec![UserContentBlock::Text(TextBlock { text: text.into() })],
            source: UserSource::Human,
            kind: InboundKind::Message,
            timestamp: None,
        })
    }

    fn input(messages: Vec<MessageBlock>) -> SelectedModelInput {
        let mut input = SelectedModelInput::default();
        for message in messages {
            input.push(message.into());
        }
        input
    }

    #[allow(clippy::unnecessary_wraps)] // Match the fallible estimator seam exercised by the planner.
    fn cost(request: &SummaryRequest) -> Result<u64, ContextError> {
        Ok(request.fragment.as_ref().unwrap().text.len() as u64)
    }

    #[test]
    fn oversized_unicode_source_has_exact_contiguous_coverage() {
        let messages = input(vec![user(&"x界🦀".repeat(400)), user("last")]);
        let mut plan = ChunkPlan::new(&messages, 2).unwrap();
        let expected: Vec<_> = plan
            .sources
            .iter()
            .map(|source| source.text.clone())
            .collect();
        let mut rendered = vec![String::new(); expected.len()];
        let mut calls = 0;
        while !plan.is_complete() {
            let request = plan.next(None, None, None, 113, true, cost).unwrap();
            let fragment = request.fragment.as_ref().unwrap();
            assert!(fragment.text.len() <= 113);
            let last = fragment.ranges.last().unwrap();
            let last_source = &expected[last.projection_index as usize];
            if (last.byte_end as usize) < last_source.len() {
                let following = last_source[last.byte_end as usize..]
                    .chars()
                    .next()
                    .unwrap();
                assert!(fragment.text.len() + following.len_utf8() > 113);
            }
            for range in &fragment.ranges {
                let index = range.projection_index as usize;
                assert_eq!(range.byte_start as usize, rendered[index].len());
                let slice = &expected[index][range.byte_start as usize..range.byte_end as usize];
                rendered[index].push_str(slice);
            }
            plan.accept(&request).unwrap();
            calls += 1;
            assert!(calls <= plan.planned_bound());
        }
        assert_eq!(rendered, expected);
        assert!(calls > 1);
    }

    #[test]
    fn tool_call_and_result_stay_together_when_the_group_fits() {
        let assistant = MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("assistant"),
            content: vec![AssistantContentBlock::ToolCall(ToolCall {
                id: ToolCallId::new("call"),
                tool_id: ToolId::new("tool"),
                name: "read".into(),
                arguments: serde_json::json!({}),
            })],
        });
        let tool = MessageBlock::Tool(ToolMessageBlock {
            id: MessageId::new("result"),
            occurrence: ToolCallOccurrenceRef::new(
                MessageId::new("assistant"),
                ContentBlockIndex::new(0),
            ),
            tool_call_id: ToolCallId::new("call"),
            tool_id: ToolId::new("tool"),
            result: ToolExecutionResult {
                deliveries: Vec::new(),
                status: ToolExecutionStatus::Success,
                content: vec![ToolResultContent::Text(TextBlock { text: "ok".into() })],
                duration_ms: 0,
                exit_code: Some(0),
                artifacts: Vec::new(),
                truncation: None,
                workflow: None,
                managed_output: None,
            },
        });
        let messages = input(vec![user("first"), assistant, tool, user("last")]);
        let mut plan = ChunkPlan::new(&messages, 4).unwrap();
        let estimator = |request: &SummaryRequest| {
            Ok(if request.fragment.as_ref().unwrap().ranges.len() <= 2 {
                1
            } else {
                2
            })
        };
        let first = plan.next(None, None, None, 1, true, estimator).unwrap();
        assert_eq!(first.fragment.as_ref().unwrap().ranges.len(), 1);
        plan.accept(&first).unwrap();
        let second = plan.next(None, None, None, 1, true, estimator).unwrap();
        let ranges = &second.fragment.as_ref().unwrap().ranges;
        assert_eq!(ranges.len(), 2);
        assert_eq!(ranges[0].role, AuxiliarySourceRole::Assistant);
        assert_eq!(ranges[1].role, AuxiliarySourceRole::Tool);
    }

    #[test]
    fn acceptance_rejects_altered_or_stale_fragments_and_covers_empty_occurrences() {
        let empty = MessageBlock::Assistant(AssistantMessageBlock {
            id: MessageId::new("empty"),
            content: Vec::new(),
        });
        let messages = input(vec![empty, user("body")]);
        let mut plan = ChunkPlan::new(&messages, 2).unwrap();
        let previous = SummaryGeneration {
            text: "prior summary".into(),
            generation: None,
        };
        let request = plan
            .next(Some(&previous), Some("focus"), Some(50), 1000, true, cost)
            .unwrap();
        assert_eq!(request.previous.as_ref().unwrap().text, "prior summary");
        assert_eq!(request.focus.as_deref(), Some("focus"));
        assert_eq!(request.output_tokens, Some(50));
        let first = &request.fragment.as_ref().unwrap().ranges[0];
        assert_eq!((first.byte_start, first.byte_end), (0, 0));

        let mut altered = request.clone();
        altered.fragment.as_mut().unwrap().text.push('!');
        assert!(plan.accept(&altered).is_err());
        let mut skipped = request.clone();
        skipped.fragment.as_mut().unwrap().ranges.remove(0);
        assert!(plan.accept(&skipped).is_err());
        plan.accept(&request).unwrap();
        assert!(plan.is_complete());
        assert!(plan.accept(&request).is_err());
    }

    #[test]
    fn arbitrary_estimates_search_exactly_or_report_incomplete() {
        let messages = input(vec![user("abcdefghijklmnop")]);
        let plan = ChunkPlan::new(&messages, 1).unwrap();
        let target = plan.sources[0].text.len() - 5;
        let request = plan
            .next(None, None, None, 1, false, |request| {
                Ok(if request.fragment.as_ref().unwrap().text.len() == target {
                    1
                } else {
                    2
                })
            })
            .unwrap();
        assert_eq!(request.fragment.unwrap().text.len(), target);

        let large = input(vec![user(&"x".repeat(1000))]);
        let plan = ChunkPlan::new(&large, 1).unwrap();
        let error = plan
            .next(None, None, None, 1, false, |request| {
                Ok(if request.fragment.as_ref().unwrap().text.len() <= 1 {
                    1
                } else {
                    2
                })
            })
            .unwrap_err();
        assert_eq!(error.kind, ContextErrorKind::SearchIncomplete);
    }

    #[test]
    fn range_and_source_limits_are_finite() {
        let messages = input((0..40).map(|_| user("x")).collect());
        let mut plan = ChunkPlan::new(&messages, 40).unwrap();
        assert_eq!(
            plan.planned_bound(),
            256.min(
                plan.sources
                    .iter()
                    .map(|s| s.text.len().max(1))
                    .sum::<usize>()
            )
        );
        let first = plan.next(None, None, None, u64::MAX, true, cost).unwrap();
        assert_eq!(
            first.fragment.as_ref().unwrap().ranges.len(),
            AUXILIARY_SOURCE_RANGE_LIMIT_MAX
        );
        plan.accept(&first).unwrap();
        let last = plan.next(None, None, None, u64::MAX, true, cost).unwrap();
        assert_eq!(last.fragment.as_ref().unwrap().ranges.len(), 8);
        plan.accept(&last).unwrap();
        assert!(plan.is_complete());

        let too_large = input(vec![user(
            &"x".repeat(ResourceLimits::default().record_bytes),
        )]);
        assert!(ChunkPlan::new(&too_large, 1).is_err());
    }

    #[test]
    fn request_only_context_cannot_be_retired() {
        let mut input = SelectedModelInput::default();
        input.push(crate::model::ModelInputMessage::RequestOnly(
            crate::model::input::RequestOnlyModelContext::AuxiliaryQuestion {
                text: "protected".into(),
            },
        ));
        assert!(matches!(
            ChunkPlan::new(&input, 1),
            Err(ContextError {
                kind: ContextErrorKind::MalformedHistory,
                ..
            })
        ));
    }

    #[test]
    fn render_once_and_report_chunk_cap_exhaustion() {
        super::super::summarizer::take_summary_rendered_messages();
        let messages = input(vec![user(&"x".repeat(300))]);
        let mut plan = ChunkPlan::new(&messages, 1).unwrap();
        assert_eq!(
            super::super::summarizer::take_summary_rendered_messages(),
            1
        );
        for _ in 0..plan.planned_bound() {
            let request = plan.next(None, None, None, 1, true, cost).unwrap();
            plan.accept(&request).unwrap();
        }
        assert!(!plan.is_complete());
        assert_eq!(
            plan.next(None, None, None, 1, true, cost).unwrap_err().kind,
            ContextErrorKind::CompactionBudgetExceeded
        );
        assert_eq!(
            super::super::summarizer::take_summary_rendered_messages(),
            0
        );
    }
}
