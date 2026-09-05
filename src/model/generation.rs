//! Provider-independent single-generation safety: budgets and integrity.
//!
//! One physical model generation is subject to three *different* contracts,
//! and this module owns the last two:
//!
//! ```text
//! transport liveness   !=   generation budget   !=   generation integrity
//! (src/model/deadline)      (this module)            (this module)
//! ```
//!
//! - **transport liveness** asks whether the provider is still producing
//!   anything at all. It is owned by [`crate::model::deadline`] and is not
//!   affected by *what* the provider produces.
//! - **generation budget** asks whether one generation has produced more
//!   than it is allowed to. A stream that emits reasoning forever is
//!   perfectly live, so liveness cannot bound it.
//! - **generation integrity** asks whether what is being produced is still a
//!   generation at all, rather than a deterministic repetition loop. A
//!   degenerate stream is both live and inside its budget for a long time.
//!
//! Everything here is provider-independent. It observes the *normalized*
//! adapter→kernel stream ([`ModelStreamItem`]), never provider wire data, and
//! it never inspects a provider name, a model name, a hostname, or sampling
//! configuration. It never calls another model and it never judges answer
//! quality: it recognizes deterministic repetition with strong evidence, and
//! nothing else.
//!
//! The [`GenerationGuard`] is request-local execution state, exactly like
//! [`ModelRequestDeadline`](crate::model::deadline::ModelRequestDeadline). It
//! decides *nothing*: it reports one typed [`GenerationFailure`] to its
//! caller, and the caller — the Agent Loop for a primary request, the
//! context-plane summarizer for a summary request — owns discarding the
//! generation, cancellation arbitration, the recovery budget, and the
//! canonical commit decision.

use serde::{Deserialize, Serialize};

use crate::model::adapter::ModelStreamItem;
use crate::model::event::ModelEvent;
use crate::model::finish::ModelFinishReason;

// ---------------------------------------------------------------------------
// Typed provider-independent facts
// ---------------------------------------------------------------------------

/// The model-generated text channel one generation fact belongs to.
///
/// Only channels the model *generates as text* exist here. Usage updates,
/// continuation metadata, lifecycle events, and tool-call argument streams
/// are deliberately not a channel: tool arguments are structured wire data
/// that legitimately repeats, and they belong to the `ToolCall`
/// proposal/acceptance contract instead.
///
/// Refusal deltas are part of [`GenerationChannel::Content`]. A refusal is
/// visible model-generated output that assembles into the canonical
/// Assistant message, so a refusal repetition loop is exactly as degenerate
/// as a text repetition loop; the choice is explicit rather than accidental.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationChannel {
    /// Model reasoning output.
    Reasoning,
    /// Visible assistant output: text and refusal.
    Content,
}

impl GenerationChannel {
    /// The human-readable channel name used in runtime diagnostics. The
    /// stable wire value is the serde representation, not this string.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Reasoning => "reasoning",
            Self::Content => "content",
        }
    }
}

/// Which generation budget was exhausted.
///
/// The two are semantically distinct and are never collapsed: a model may
/// legitimately produce a long answer, and it may legitimately think before
/// answering, but "may think without bound" is not implied by "may answer at
/// length".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GenerationBudgetKind {
    /// The whole generation's output budget.
    TotalOutput,
    /// The reasoning share of that budget.
    Reasoning,
}

impl GenerationBudgetKind {
    /// The human-readable budget name used in runtime diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::TotalOutput => "total output",
            Self::Reasoning => "reasoning",
        }
    }
}

/// The request-local deadline phase that expired.
///
/// It is a typed fact rather than a substring of a diagnostic message, so no
/// consumer above the model layer has to read prose to learn which liveness
/// contract was violated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelTimeoutPhase {
    /// The provider produced no generation progress at all.
    ResponseStart,
    /// The provider stopped producing progress after generation began.
    StreamIdle,
}

impl ModelTimeoutPhase {
    /// The human-readable phase name used in runtime diagnostics.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::ResponseStart => "response-start",
            Self::StreamIdle => "stream-idle",
        }
    }
}

/// The typed provider-independent detail of one single-generation safety
/// failure.
///
/// Every payload is a small enumeration or an integer, so a provider can
/// never author unbounded durable diagnostic data through this type: the
/// bound is structural rather than a truncation rule. The rendered
/// [`Self::message`] is likewise runtime-authored and never echoes model
/// output.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum GenerationFailure {
    /// One generated text channel deterministically degenerated into
    /// repetition with strong evidence.
    Degenerated {
        /// Which generated channel repeated.
        channel: GenerationChannel,
        /// The byte length of the repeating unit.
        period_bytes: u32,
        /// How many consecutive repetitions of that unit were verified.
        repetitions: u32,
        /// The verified repeated span, in bytes.
        span_bytes: u32,
    },
    /// The provider itself terminated the generation at its own token limit,
    /// which the adapter normalized as [`ModelFinishReason::Length`].
    ///
    /// The provider knows the exact token count and this runtime does not, so
    /// this variant deliberately carries no measurement rather than an
    /// invented one. It always concerns the total output budget: no provider
    /// reports a separate reasoning termination.
    ProviderLengthLimit,
    /// The runtime-side byte safeguard bounded a runaway generation.
    ///
    /// The measurements are UTF-8 byte counts of normalized generated data,
    /// never model tokens. They are bounded diagnostics and saturate at
    /// [`u32::MAX`]; the authoritative bound is [`GenerationBudget`].
    RuntimeBudgetExceeded {
        /// Which budget was exhausted.
        budget: GenerationBudgetKind,
        /// The runtime byte bound that was exceeded.
        limit_bytes: u32,
        /// The generated bytes observed when it was exceeded.
        observed_bytes: u32,
    },
    /// The request produced no qualifying progress within its deadline.
    Timeout {
        /// The phase that expired.
        phase: ModelTimeoutPhase,
    },
}

impl GenerationFailure {
    /// The runtime-authored diagnostic of this fact.
    #[must_use]
    pub fn message(&self) -> String {
        match *self {
            Self::Degenerated {
                channel,
                period_bytes,
                repetitions,
                span_bytes,
            } => format!(
                "model {} degenerated: a {period_bytes}-byte unit repeated {repetitions} times across {span_bytes} bytes",
                channel.as_str()
            ),
            Self::ProviderLengthLimit => format!(
                "the provider terminated the generation at its {} limit before it completed",
                GenerationBudgetKind::TotalOutput.as_str()
            ),
            Self::RuntimeBudgetExceeded {
                budget,
                limit_bytes,
                observed_bytes,
            } => format!(
                "the generation exceeded its {} safeguard of {limit_bytes} bytes after \
                 {observed_bytes} bytes",
                budget.as_str()
            ),
            Self::Timeout { phase } => {
                format!("model request exceeded its {} deadline", phase.as_str())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Budgets
// ---------------------------------------------------------------------------

/// The runtime byte allowance granted per resolved output token.
///
/// **This is not a token count and must never be described as one.** The
/// provider-neutral output limit of a request is
/// [`ModelRequest::max_output_tokens`](crate::model::types::ModelRequest::max_output_tokens);
/// it is resolved before the adapter boundary and the provider is expected to
/// enforce it. The runtime safeguard exists only because a self-hosted
/// backend may ignore its own request limit, so it is deliberately generous:
/// no honest generation reaches thirty-two bytes of normalized output per
/// permitted output token in any script this runtime supports, and a
/// generous multiplier is the right trade: the safeguard exists to stop a
/// runaway stream, not to shape an answer, so a bound that could plausibly
/// reject a legitimate long generation would be the worse error.
///
/// Deriving the safeguard from the already-resolved limit is what keeps this
/// from becoming a second output-limit mechanism: there is one configured
/// output budget, and this is a bound on how far a backend may overrun it.
pub const RUNTIME_GENERATED_BYTES_PER_OUTPUT_TOKEN: u64 = 32;

/// The floor of the runtime output safeguard, in bytes.
///
/// Below roughly this size the byte-per-token uncertainty dominates the
/// bound, and a safeguard that fires there would be deciding ordinary
/// outcomes rather than bounding runaway ones. A deliberately small resolved
/// output budget — a narrow summary cap, a small configured maximum —
/// therefore still gets a usable floor, and the provider's own
/// `max_output_tokens` remains the mechanism that actually shapes an answer.
///
/// The repetition detector, not this floor, is what catches the failure mode
/// Issue #203 actually observed: a generation stuck in a loop is classified
/// after roughly one kilobyte of evidence, long before any byte budget is
/// relevant. This bound is the backstop for the rarer non-repeating runaway.
pub const RUNTIME_MIN_GENERATED_BYTES: u64 = 262_144;

/// The reasoning share of the runtime output safeguard (numerator).
pub const RUNTIME_REASONING_BYTE_SHARE_NUMERATOR: u64 = 3;

/// The reasoning share of the runtime output safeguard (denominator).
pub const RUNTIME_REASONING_BYTE_SHARE_DENOMINATOR: u64 = 4;

/// The deterministic runtime byte budget of one physical generation.
///
/// The guarantee is exactly this, and nothing more:
///
/// > One physical generation cannot stream more than `max_generated_bytes`
/// > of normalized generated data, nor more than `max_reasoning_bytes` of
/// > normalized reasoning text, before the runtime terminates it. Both
/// > numbers are UTF-8 byte counts of the adapter's normalized output, not
/// > model tokens, and neither replaces the provider-enforced
/// > `max_output_tokens`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerationBudget {
    /// The byte bound of all normalized generated data of one generation:
    /// reasoning, text, refusal, and streamed tool-call arguments.
    pub max_generated_bytes: u64,
    /// The byte bound of normalized reasoning text alone.
    pub max_reasoning_bytes: u64,
}

impl GenerationBudget {
    /// Derives the runtime safeguard of one request from its already
    /// resolved provider-neutral output-token budget.
    #[must_use]
    pub fn for_output_tokens(max_output_tokens: u32) -> Self {
        let generated = u64::from(max_output_tokens)
            .saturating_mul(RUNTIME_GENERATED_BYTES_PER_OUTPUT_TOKEN)
            .max(RUNTIME_MIN_GENERATED_BYTES);
        Self {
            max_generated_bytes: generated,
            max_reasoning_bytes: generated / RUNTIME_REASONING_BYTE_SHARE_DENOMINATOR
                * RUNTIME_REASONING_BYTE_SHARE_NUMERATOR,
        }
    }
}

// ---------------------------------------------------------------------------
// The deterministic degeneration detector
// ---------------------------------------------------------------------------

/// The shortest *primitive* repeating unit that can be evidence.
///
/// A candidate span is rejected when its primitive period is shorter than
/// this, so a run of `,` or `},` is never evidence — not even through the
/// longer period that trivially divides it. `}`, `]`, `,`, and the two-byte
/// `0,` of a numeric array are structure, not generation failure, and
/// excluding them is a deliberate false-positive control rather than an
/// accident of the threshold arithmetic.
pub const DEGENERATION_MIN_PERIOD_BYTES: usize = 4;

/// The longest repeating unit the detector recognizes.
///
/// It bounds both the detector's memory and its per-scan work. A repeated
/// unit longer than this is not classified: the detector prefers missing an
/// ambiguous shape over rejecting a valid generation.
pub const DEGENERATION_MAX_PERIOD_BYTES: usize = 512;

/// The minimum number of consecutive verified repetitions.
pub const DEGENERATION_MIN_REPETITIONS: usize = 4;

/// The minimum verified repeated span, in bytes.
///
/// Together with [`DEGENERATION_MIN_REPETITIONS`] and
/// [`DEGENERATION_MIN_PERIOD_BYTES`] this is the whole evidence threshold:
///
/// > at least four consecutive exact repetitions of a unit whose primitive
/// > period is at least four bytes, spanning at least 1024 bytes.
///
/// `foo foo foo …` qualifies after 256 repetitions; a handful of repeated
/// JSON or XML delimiters never qualifies.
pub const DEGENERATION_MIN_SPAN_BYTES: usize = 1024;

/// The byte interval at which the detector scans.
///
/// Scanning happens at cumulative-byte checkpoints of the channel, never at
/// provider delta boundaries: the detector buffers a partial checkpoint and
/// commits only whole [`DEGENERATION_SCAN_STRIDE_BYTES`] blocks. The scanned
/// prefix at checkpoint `k` is therefore exactly the first `k * stride`
/// bytes of the channel however the provider chunked them, so a re-chunked
/// but byte-identical generation produces a byte-identical classification.
pub const DEGENERATION_SCAN_STRIDE_BYTES: usize = 64;

/// The detector's rolling window, in bytes.
///
/// It is the largest span any candidate period can require, so the window is
/// an exact bound rather than a guess: no candidate can need more evidence
/// than the window holds.
const DEGENERATION_WINDOW_BYTES: usize =
    DEGENERATION_MAX_PERIOD_BYTES * DEGENERATION_MIN_REPETITIONS;

/// The verified repetitions required for one candidate period.
const fn required_repetitions(period: usize) -> usize {
    let by_span = DEGENERATION_MIN_SPAN_BYTES.div_ceil(period);
    if by_span > DEGENERATION_MIN_REPETITIONS {
        by_span
    } else {
        DEGENERATION_MIN_REPETITIONS
    }
}

/// The bounded evidence of one classified repetition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RepetitionEvidence {
    period_bytes: u32,
    repetitions: u32,
    span_bytes: u32,
}

/// One channel's bounded deterministic repetition detector.
///
/// State is a fixed [`DEGENERATION_WINDOW_BYTES`] ring buffer plus one
/// partial checkpoint block and three counters: nothing here grows with the
/// length of the generation.
///
/// Work is bounded **per generated byte**, so total detector work is linear
/// in the generated bytes and never quadratic. One scan runs once per
/// [`DEGENERATION_SCAN_STRIDE_BYTES`], tries at most
/// `DEGENERATION_MAX_PERIOD_BYTES` candidates, and each candidate is either
/// rejected by three constant-time probes or verified across at most
/// [`DEGENERATION_WINDOW_BYTES`] comparisons. The worst case is therefore a
/// fixed constant per byte rather than a function of how much has been
/// generated, and the shapes that would approach it — long runs of one
/// repeating delimiter — end the scan at the primitive-period rule instead.
#[derive(Debug)]
struct RepetitionDetector {
    window: Box<[u8; DEGENERATION_WINDOW_BYTES]>,
    /// The next write position in the ring.
    head: usize,
    /// How much of the ring holds committed bytes.
    filled: usize,
    /// The partially accumulated checkpoint block.
    pending: [u8; DEGENERATION_SCAN_STRIDE_BYTES],
    pending_len: usize,
}

impl Default for RepetitionDetector {
    fn default() -> Self {
        Self {
            window: Box::new([0_u8; DEGENERATION_WINDOW_BYTES]),
            head: 0,
            filled: 0,
            pending: [0_u8; DEGENERATION_SCAN_STRIDE_BYTES],
            pending_len: 0,
        }
    }
}

impl RepetitionDetector {
    /// Appends one normalized delta of this channel and returns the evidence
    /// when a checkpoint scan classifies the channel as degenerated.
    fn push(&mut self, text: &str) -> Option<RepetitionEvidence> {
        for byte in text.as_bytes() {
            self.pending[self.pending_len] = *byte;
            self.pending_len += 1;
            if self.pending_len == DEGENERATION_SCAN_STRIDE_BYTES {
                self.commit_checkpoint();
                if let Some(evidence) = self.scan() {
                    return Some(evidence);
                }
            }
        }
        None
    }

    /// Commits the accumulated checkpoint block into the ring window.
    fn commit_checkpoint(&mut self) {
        for index in 0..self.pending_len {
            self.window[self.head] = self.pending[index];
            self.head = (self.head + 1) % DEGENERATION_WINDOW_BYTES;
            if self.filled < DEGENERATION_WINDOW_BYTES {
                self.filled += 1;
            }
        }
        self.pending_len = 0;
    }

    /// The committed byte `distance` positions back from the end of the
    /// window; `0` is the most recently committed byte.
    fn byte_at(&self, distance: usize) -> u8 {
        let index =
            (self.head + DEGENERATION_WINDOW_BYTES - 1 - distance) % DEGENERATION_WINDOW_BYTES;
        self.window[index]
    }

    /// Whether the window's `span`-byte suffix is exactly `period`-periodic.
    fn is_periodic(&self, span: usize, period: usize) -> bool {
        (0..span - period).all(|offset| self.byte_at(offset) == self.byte_at(offset + period))
    }

    /// Classifies the committed window suffix.
    ///
    /// For every candidate period the suffix must be exactly periodic across
    /// the span the evidence threshold requires, and its primitive period
    /// must not be shorter than [`DEGENERATION_MIN_PERIOD_BYTES`]. A bounded
    /// set of sample probes rejects almost every candidate in constant time
    /// before the linear verification runs, so the practical cost of one scan
    /// is proportional to the number of candidate periods rather than to the
    /// window.
    fn scan(&self) -> Option<RepetitionEvidence> {
        if self.filled < DEGENERATION_MIN_SPAN_BYTES {
            return None;
        }
        for period in DEGENERATION_MIN_PERIOD_BYTES..=DEGENERATION_MAX_PERIOD_BYTES {
            let repetitions = required_repetitions(period);
            let span = period * repetitions;
            if span > self.filled {
                continue;
            }
            if !self.probe(period, span - period) {
                continue;
            }
            if !self.is_periodic(span, period) {
                continue;
            }
            // A run of one delimiter is also periodic at every multiple of
            // its own length, so the minimum period alone would not exclude
            // it. The evidence is the *primitive* period: a span that repeats
            // something shorter than the minimum unit is structure, not
            // degeneration.
            //
            // The scan ends here rather than continuing, and that is both the
            // false-positive control and the cost bound. A suffix this short
            // a period covers is a delimiter run whatever longer period is
            // tried, and continuing would run the linear verification for
            // every remaining multiple of it — the one input shape that could
            // make a scan expensive.
            if (1..DEGENERATION_MIN_PERIOD_BYTES).any(|shorter| self.is_periodic(span, shorter)) {
                return None;
            }
            return Some(RepetitionEvidence {
                period_bytes: u32::try_from(period).unwrap_or(u32::MAX),
                repetitions: u32::try_from(repetitions).unwrap_or(u32::MAX),
                span_bytes: u32::try_from(span).unwrap_or(u32::MAX),
            });
        }
        None
    }

    /// Constant-time rejection probes for one candidate period. Every probe
    /// is a comparison the full verification also makes, so a probe can only
    /// reject a candidate the verification would have rejected.
    fn probe(&self, period: usize, comparisons: usize) -> bool {
        let probes = [comparisons - 1, comparisons / 2, comparisons / 4];
        probes
            .iter()
            .all(|offset| self.byte_at(*offset) == self.byte_at(*offset + period))
    }
}

// ---------------------------------------------------------------------------
// The request-local guard
// ---------------------------------------------------------------------------

/// The request-local generation-safety state of one physical generation.
///
/// The guard observes the normalized stream and reports the first typed
/// [`GenerationFailure`] it can prove. It owns no cancellation authority, no
/// recovery budget, and no commit decision; it never mutates the stream and
/// never emits an event.
///
/// Evaluation order inside one observed item is fixed and documented:
/// degeneration is evaluated before the budget, because when both would fire
/// at the same delta the repetition is the more specific diagnosis.
#[derive(Debug)]
pub struct GenerationGuard {
    budget: GenerationBudget,
    generated_bytes: u64,
    reasoning_bytes: u64,
    reasoning: RepetitionDetector,
    content: RepetitionDetector,
}

impl GenerationGuard {
    /// Creates the guard of one physical generation.
    #[must_use]
    pub fn new(budget: GenerationBudget) -> Self {
        Self {
            budget,
            generated_bytes: 0,
            reasoning_bytes: 0,
            reasoning: RepetitionDetector::default(),
            content: RepetitionDetector::default(),
        }
    }

    /// The budget this guard enforces.
    #[must_use]
    pub const fn budget(&self) -> GenerationBudget {
        self.budget
    }

    /// Observes one normalized stream item of the generation.
    ///
    /// Returns the typed fact when this item proves the generation must not
    /// reach a canonical commit. Items that are not model-generated data —
    /// lifecycle, usage, continuation state, terminals, ephemeral progress —
    /// are charged to nothing and inspected for nothing.
    pub fn observe(&mut self, item: &ModelStreamItem) -> Option<GenerationFailure> {
        let ModelStreamItem::Event(event) = item else {
            return None;
        };
        match event {
            ModelEvent::ReasoningDelta { text, .. } => {
                self.observe_text(GenerationChannel::Reasoning, text)
            }
            ModelEvent::TextDelta { text, .. } | ModelEvent::RefusalDelta { text, .. } => {
                self.observe_text(GenerationChannel::Content, text)
            }
            // Tool-call arguments are structured wire data whose repetition
            // is ordinary, and they already belong to the `ToolCall`
            // proposal/acceptance contract. They are charged against the
            // total-output safeguard — an unbounded argument stream is still
            // an unbounded generation — and are never inspected as text.
            ModelEvent::ToolCallArgumentsDelta {
                arguments_delta, ..
            } => self.charge_total(byte_len(arguments_delta)),
            ModelEvent::Started
            | ModelEvent::ToolCallStarted { .. }
            | ModelEvent::ToolCallCompleted { .. }
            | ModelEvent::UsageUpdate { .. }
            | ModelEvent::ContinuationState { .. }
            | ModelEvent::Completed { .. }
            | ModelEvent::Failed { .. } => None,
        }
    }

    /// Classifies one successful provider terminal.
    ///
    /// A generation the provider itself stopped at its token limit is
    /// incomplete by construction: the provider is stating that it stopped
    /// because of the limit rather than because the answer was finished.
    /// Accepting that as an ordinary successful assistant completion would
    /// commit a truncated turn, so it is a typed budget fact instead.
    #[must_use]
    pub fn classify_completion(finish_reason: &ModelFinishReason) -> Option<GenerationFailure> {
        match finish_reason {
            ModelFinishReason::Length => Some(GenerationFailure::ProviderLengthLimit),
            ModelFinishReason::Stop
            | ModelFinishReason::ToolCalls
            | ModelFinishReason::ContentFilter
            | ModelFinishReason::Refusal
            | ModelFinishReason::Other { .. } => None,
        }
    }

    fn observe_text(
        &mut self,
        channel: GenerationChannel,
        text: &str,
    ) -> Option<GenerationFailure> {
        let detector = match channel {
            GenerationChannel::Reasoning => &mut self.reasoning,
            GenerationChannel::Content => &mut self.content,
        };
        if let Some(evidence) = detector.push(text) {
            // The detector is channel-agnostic; attribution is owned here,
            // where the channel of the delta is known.
            return Some(GenerationFailure::Degenerated {
                channel,
                period_bytes: evidence.period_bytes,
                repetitions: evidence.repetitions,
                span_bytes: evidence.span_bytes,
            });
        }
        let bytes = byte_len(text);
        if channel == GenerationChannel::Reasoning {
            self.reasoning_bytes = self.reasoning_bytes.saturating_add(bytes);
            if self.reasoning_bytes > self.budget.max_reasoning_bytes {
                return Some(GenerationFailure::RuntimeBudgetExceeded {
                    budget: GenerationBudgetKind::Reasoning,
                    limit_bytes: bounded_count(self.budget.max_reasoning_bytes),
                    observed_bytes: bounded_count(self.reasoning_bytes),
                });
            }
        }
        self.charge_total(bytes)
    }

    fn charge_total(&mut self, bytes: u64) -> Option<GenerationFailure> {
        self.generated_bytes = self.generated_bytes.saturating_add(bytes);
        if self.generated_bytes <= self.budget.max_generated_bytes {
            return None;
        }
        Some(GenerationFailure::RuntimeBudgetExceeded {
            budget: GenerationBudgetKind::TotalOutput,
            limit_bytes: bounded_count(self.budget.max_generated_bytes),
            observed_bytes: bounded_count(self.generated_bytes),
        })
    }
}

/// The byte length of one normalized delta, saturating rather than wrapping.
fn byte_len(text: &str) -> u64 {
    u64::try_from(text.len()).unwrap_or(u64::MAX)
}

/// One diagnostic byte count, saturating so the typed fact stays small.
fn bounded_count(bytes: u64) -> u32 {
    u32::try_from(bytes).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::{
        DEGENERATION_MAX_PERIOD_BYTES, DEGENERATION_MIN_SPAN_BYTES, GenerationBudget,
        GenerationBudgetKind, GenerationChannel, GenerationFailure, GenerationGuard,
        ModelTimeoutPhase, RUNTIME_MIN_GENERATED_BYTES,
    };
    use crate::message::types::ContentBlockIndex;
    use crate::model::adapter::{ModelStreamItem, ModelStreamProgress};
    use crate::model::event::ModelEvent;
    use crate::model::finish::ModelFinishReason;
    use crate::model::types::ModelUsage;
    use crate::runtime::identity::ToolCallId;

    fn reasoning_delta(text: &str) -> ModelStreamItem {
        ModelStreamItem::Event(ModelEvent::ReasoningDelta {
            block_index: ContentBlockIndex::new(0),
            text: text.to_owned(),
        })
    }

    fn text_delta(text: &str) -> ModelStreamItem {
        ModelStreamItem::Event(ModelEvent::TextDelta {
            block_index: ContentBlockIndex::new(0),
            text: text.to_owned(),
        })
    }

    /// A guard whose budget is large enough that only degeneration can fire.
    fn integrity_guard() -> GenerationGuard {
        GenerationGuard::new(GenerationBudget {
            max_generated_bytes: u64::MAX,
            max_reasoning_bytes: u64::MAX,
        })
    }

    /// Feeds chunks through one channel and returns the first reported fact.
    fn drive(
        guard: &mut GenerationGuard,
        chunks: &[&str],
        reasoning: bool,
    ) -> Option<GenerationFailure> {
        for chunk in chunks {
            let item = if reasoning {
                reasoning_delta(chunk)
            } else {
                text_delta(chunk)
            };
            if let Some(failure) = guard.observe(&item) {
                return Some(failure);
            }
        }
        None
    }

    fn degenerated(failure: Option<GenerationFailure>) -> (GenerationChannel, u32, u32) {
        match failure {
            Some(GenerationFailure::Degenerated {
                channel,
                period_bytes,
                repetitions,
                ..
            }) => (channel, period_bytes, repetitions),
            other => panic!("expected a degeneration fact, got {other:?}"),
        }
    }

    /// The motivating shape: a short unit repeated far past the threshold.
    #[test]
    fn short_period_repetition_is_degeneration() {
        let mut guard = integrity_guard();
        let failure = drive(&mut guard, &["foo "; 600], false);
        let (channel, period, repetitions) = degenerated(failure);
        assert_eq!(channel, GenerationChannel::Content);
        assert_eq!(period, 4);
        assert_eq!(repetitions, 256);
    }

    /// Reasoning degeneration is attributed to the reasoning channel, and
    /// content degeneration to the content channel, by the same detector.
    #[test]
    fn channel_attribution_follows_the_delta() {
        let mut reasoning = integrity_guard();
        let (channel, ..) = degenerated(drive(&mut reasoning, &["thinking. "; 300], true));
        assert_eq!(channel, GenerationChannel::Reasoning);

        let mut content = integrity_guard();
        let (channel, ..) = degenerated(drive(&mut content, &["thinking. "; 300], false));
        assert_eq!(channel, GenerationChannel::Content);
    }

    /// A refusal is visible generated content and shares its channel.
    #[test]
    fn refusal_deltas_are_content() {
        let mut guard = integrity_guard();
        let mut observed = None;
        for _ in 0..300 {
            let item = ModelStreamItem::Event(ModelEvent::RefusalDelta {
                block_index: ContentBlockIndex::new(0),
                text: "I cannot help. ".to_owned(),
            });
            if let Some(failure) = guard.observe(&item) {
                observed = Some(failure);
                break;
            }
        }
        let (channel, ..) = degenerated(observed);
        assert_eq!(channel, GenerationChannel::Content);
    }

    /// The channels are tracked separately: a degenerate reasoning stream
    /// interleaved with distinct content is still attributed to reasoning.
    #[test]
    fn the_two_channels_are_tracked_separately() {
        let mut guard = integrity_guard();
        let mut observed = None;
        for index in 0..600_u32 {
            if let Some(failure) = guard.observe(&reasoning_delta("loop ")) {
                observed = Some(failure);
                break;
            }
            assert!(
                guard
                    .observe(&text_delta(&format!("distinct content {index} ")))
                    .is_none()
            );
        }
        let (channel, period, _) = degenerated(observed);
        assert_eq!(channel, GenerationChannel::Reasoning);
        assert_eq!(period, 5);
    }

    /// Chunk framing cannot change the classification: the detector scans
    /// cumulative-byte checkpoints, never provider delta boundaries.
    #[test]
    fn chunk_boundaries_do_not_change_the_result() {
        let text = "foo ".repeat(600);
        let per_unit = vec!["foo "; 600];
        let mut ragged: Vec<&str> = Vec::new();
        let mut rest = text.as_str();
        for width in [8_usize, 3, 1, 13, 64, 5, 127].iter().cycle() {
            if rest.is_empty() {
                break;
            }
            let (head, tail) = rest.split_at((*width).min(rest.len()));
            ragged.push(head);
            rest = tail;
        }

        let mut per_unit_guard = integrity_guard();
        let mut ragged_guard = integrity_guard();
        let mut whole_guard = integrity_guard();
        let first = drive(&mut per_unit_guard, &per_unit, false);
        let second = drive(&mut ragged_guard, &ragged, false);
        let third = drive(&mut whole_guard, &[text.as_str()], false);
        assert!(first.is_some());
        assert_eq!(first, second);
        assert_eq!(second, third);
    }

    /// Ordinary repetitive structure stays below the evidence threshold even
    /// when far more of it is generated than the span threshold requires.
    #[test]
    fn repetitive_structure_is_not_degeneration() {
        let json = "{\n  \"a\": [],\n  \"b\": [],\n  \"c\": []\n}\n";
        let xml = "<row><cell>alpha</cell></row>\n<row><cell>beta</cell></row>\n";
        let code = "    if (a == 1) { return 1; }\n    if (b == 2) { return 2; }\n";
        for corpus in [json, xml, code] {
            let mut guard = integrity_guard();
            let mut fed = 0_usize;
            for index in 0..64_u32 {
                // Ordinary structured output repeats its syntax while its
                // payload changes; only the syntax is periodic.
                let chunk = corpus.replace('a', &format!("a{index}"));
                fed += chunk.len();
                assert_eq!(
                    guard.observe(&text_delta(&chunk)),
                    None,
                    "ordinary structured output must not be classified as degeneration"
                );
            }
            assert!(fed > DEGENERATION_MIN_SPAN_BYTES * 2);
        }
    }

    /// A long run of one delimiter is exactly periodic at every multiple of
    /// its length, and is excluded by the primitive-period rule rather than
    /// by luck.
    #[test]
    fn repeated_delimiters_are_below_the_minimum_primitive_period() {
        for delimiter in ["}", "]", ",", "},", "}\n", "  "] {
            let mut guard = integrity_guard();
            for _ in 0..4_000 {
                assert_eq!(
                    guard.observe(&text_delta(delimiter)),
                    None,
                    "a repeated delimiter is structure, not degeneration"
                );
            }
        }
    }

    /// A repeated unit longer than the recognized maximum period is not
    /// classified: the detector prefers a miss to a false positive.
    #[test]
    fn a_unit_longer_than_the_maximum_period_is_not_classified() {
        let mut block = String::new();
        for index in 0..DEGENERATION_MAX_PERIOD_BYTES + 32 {
            block.push(char::from(b'a' + u8::try_from(index % 26).expect("small")));
        }
        let mut guard = integrity_guard();
        for _ in 0..16 {
            assert_eq!(guard.observe(&text_delta(&block)), None);
        }
    }

    /// A repeated paragraph inside the recognized period range classifies.
    #[test]
    fn a_repeated_paragraph_is_degeneration() {
        let paragraph = "The build failed because the lockfile drifted; regenerate it and retry the pipeline from the top.\n";
        assert!(paragraph.len() < DEGENERATION_MAX_PERIOD_BYTES);
        let mut guard = integrity_guard();
        let failure = drive(&mut guard, &[paragraph; 24], false);
        let (_, period, _) = degenerated(failure);
        assert_eq!(
            usize::try_from(period).expect("period fits"),
            paragraph.len()
        );
    }

    /// Tool-call argument streams are never inspected as text, however
    /// repetitive their JSON is.
    #[test]
    fn tool_argument_streams_are_not_inspected_for_degeneration() {
        let mut guard = integrity_guard();
        for _ in 0..4_000 {
            let item = ModelStreamItem::Event(ModelEvent::ToolCallArgumentsDelta {
                block_index: ContentBlockIndex::new(0),
                call_id: ToolCallId::new("call-1"),
                arguments_delta: "{\"row\": [0, 0, 0]}, ".to_owned(),
            });
            assert_eq!(guard.observe(&item), None);
        }
    }

    /// Non-generated stream facts are charged to nothing, even under a zero
    /// budget.
    #[test]
    fn lifecycle_and_metadata_are_not_generation() {
        let mut guard = GenerationGuard::new(GenerationBudget {
            max_generated_bytes: 0,
            max_reasoning_bytes: 0,
        });
        let items = [
            ModelStreamItem::Event(ModelEvent::Started),
            ModelStreamItem::Event(ModelEvent::UsageUpdate {
                usage: ModelUsage {
                    input_tokens: 1,
                    output_tokens: 1,
                    total_tokens: 2,
                    details: None,
                },
            }),
            ModelStreamItem::Progress(ModelStreamProgress::Liveness),
            ModelStreamItem::Progress(ModelStreamProgress::Generation),
            ModelStreamItem::Event(ModelEvent::Completed {
                finish_reason: ModelFinishReason::Stop,
                usage: None,
            }),
        ];
        for item in &items {
            assert_eq!(guard.observe(item), None);
        }
    }

    /// Continuous non-repeating reasoning cannot evade the reasoning budget.
    #[test]
    fn continuous_reasoning_exhausts_the_reasoning_budget() {
        let mut guard = GenerationGuard::new(GenerationBudget {
            max_generated_bytes: 4_096,
            max_reasoning_bytes: 1_024,
        });
        let mut observed = None;
        for index in 0..10_000_u32 {
            if let Some(failure) = guard.observe(&reasoning_delta(&format!("step {index}; "))) {
                observed = Some(failure);
                break;
            }
        }
        match observed {
            Some(GenerationFailure::RuntimeBudgetExceeded {
                budget,
                limit_bytes,
                observed_bytes,
            }) => {
                assert_eq!(budget, GenerationBudgetKind::Reasoning);
                assert_eq!(limit_bytes, 1_024);
                assert!(observed_bytes > 1_024);
            }
            other => panic!("expected a reasoning budget fact, got {other:?}"),
        }
    }

    /// Continuous non-repeating content cannot evade the total-output budget.
    #[test]
    fn continuous_content_exhausts_the_total_output_budget() {
        let mut guard = GenerationGuard::new(GenerationBudget {
            max_generated_bytes: 1_024,
            max_reasoning_bytes: 1_024,
        });
        let mut observed = None;
        for index in 0..10_000_u32 {
            if let Some(failure) = guard.observe(&text_delta(&format!("sentence {index}. "))) {
                observed = Some(failure);
                break;
            }
        }
        match observed {
            Some(GenerationFailure::RuntimeBudgetExceeded { budget, .. }) => {
                assert_eq!(budget, GenerationBudgetKind::TotalOutput);
            }
            other => panic!("expected a total-output budget fact, got {other:?}"),
        }
    }

    /// A tool-argument stream is still bounded by the total-output budget.
    #[test]
    fn tool_argument_streams_consume_the_total_output_budget() {
        let mut guard = GenerationGuard::new(GenerationBudget {
            max_generated_bytes: 256,
            max_reasoning_bytes: 256,
        });
        let mut observed = None;
        for _ in 0..1_000 {
            let item = ModelStreamItem::Event(ModelEvent::ToolCallArgumentsDelta {
                block_index: ContentBlockIndex::new(0),
                call_id: ToolCallId::new("call-1"),
                arguments_delta: "0123456789".to_owned(),
            });
            if let Some(failure) = guard.observe(&item) {
                observed = Some(failure);
                break;
            }
        }
        match observed {
            Some(GenerationFailure::RuntimeBudgetExceeded { budget, .. }) => {
                assert_eq!(budget, GenerationBudgetKind::TotalOutput);
            }
            other => panic!("expected a total-output budget fact, got {other:?}"),
        }
    }

    /// Degeneration is diagnosed before the budget when both would fire, and
    /// it is reached well before a realistic output budget is consumed.
    #[test]
    fn degeneration_is_diagnosed_before_the_budget() {
        let mut guard = GenerationGuard::new(GenerationBudget {
            max_generated_bytes: u64::try_from(DEGENERATION_MIN_SPAN_BYTES).expect("fits") + 8,
            max_reasoning_bytes: u64::MAX,
        });
        let failure = drive(&mut guard, &["foo "; 600], false);
        assert!(matches!(
            failure,
            Some(GenerationFailure::Degenerated { .. })
        ));
    }

    /// A provider length termination is a typed budget fact; every other
    /// terminal reason stays an ordinary completion.
    #[test]
    fn provider_length_termination_is_a_budget_fact() {
        assert_eq!(
            GenerationGuard::classify_completion(&ModelFinishReason::Length),
            Some(GenerationFailure::ProviderLengthLimit)
        );
        for reason in [
            ModelFinishReason::Stop,
            ModelFinishReason::ToolCalls,
            ModelFinishReason::ContentFilter,
            ModelFinishReason::Refusal,
            ModelFinishReason::Other {
                reason: "unknown".to_owned(),
            },
        ] {
            assert_eq!(GenerationGuard::classify_completion(&reason), None);
        }
    }

    /// The runtime safeguard derives from the resolved output-token budget,
    /// and the reasoning share is strictly smaller than the total.
    #[test]
    fn the_budget_derives_from_the_resolved_output_limit() {
        let budget = GenerationBudget::for_output_tokens(32_768);
        assert_eq!(budget.max_generated_bytes, 1_048_576);
        assert_eq!(budget.max_reasoning_bytes, 786_432);
        assert!(budget.max_reasoning_bytes < budget.max_generated_bytes);
        // A deliberately small resolved budget still gets the documented
        // floor, so the safeguard never decides an ordinary outcome.
        let tiny = GenerationBudget::for_output_tokens(1);
        assert_eq!(tiny.max_generated_bytes, RUNTIME_MIN_GENERATED_BYTES);
        assert_eq!(tiny.max_reasoning_bytes, 196_608);
        let huge = GenerationBudget::for_output_tokens(u32::MAX);
        assert!(huge.max_reasoning_bytes < huge.max_generated_bytes);
    }

    /// Every typed fact round-trips and renders a bounded runtime-authored
    /// diagnostic that never echoes model output.
    #[test]
    fn generation_facts_round_trip_with_stable_discriminators() {
        let cases = [
            (
                GenerationFailure::Degenerated {
                    channel: GenerationChannel::Reasoning,
                    period_bytes: 4,
                    repetitions: 256,
                    span_bytes: 1_024,
                },
                "degenerated",
            ),
            (
                GenerationFailure::ProviderLengthLimit,
                "provider_length_limit",
            ),
            (
                GenerationFailure::RuntimeBudgetExceeded {
                    budget: GenerationBudgetKind::Reasoning,
                    limit_bytes: 1_024,
                    observed_bytes: 1_030,
                },
                "runtime_budget_exceeded",
            ),
            (
                GenerationFailure::Timeout {
                    phase: ModelTimeoutPhase::StreamIdle,
                },
                "timeout",
            ),
        ];
        for (failure, discriminator) in cases {
            let value = serde_json::to_value(failure).expect("serialize generation fact");
            assert_eq!(value["type"], discriminator);
            let decoded: GenerationFailure =
                serde_json::from_value(value).expect("deserialize generation fact");
            assert_eq!(decoded, failure);
            let message = failure.message();
            assert!(!message.is_empty());
            assert!(message.len() < 200);
        }
    }
}
