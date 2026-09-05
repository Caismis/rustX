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
//! The separation is enforced by the type system, not only by prose. No
//! liveness vocabulary is defined or re-exported here, and this module does
//! not depend on [`crate::model::deadline`]; a transport timeout is
//! representable only as its own typed fact and never as a
//! [`GenerationFailure`].
//!
//! This module also does not decide *what* the limits are. It defines the
//! contract it enforces — [`GenerationSafetyPolicy`] — and leaves resolving
//! one to the layer that already resolves a model invocation
//! (`ResolvedModelInvocation::generation_safety_policy`). Total output and
//! reasoning are independent inputs of that policy, so no ratio between them
//! is embedded in the accounting.
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

/// The typed provider-independent detail of one single-generation safety
/// failure.
///
/// A transport timeout is deliberately **not** a variant here. It is a
/// liveness fact owned by [`ModelTimeoutPhase`](crate::model::deadline::ModelTimeoutPhase)
/// and carried by its own field of `ModelError`, because "the provider
/// stopped producing" and "what the provider produced is unusable" are
/// different contracts with different recovery architectures.
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
    /// [`u32::MAX`]; the authoritative bound is [`GenerationSafetyPolicy`].
    RuntimeBudgetExceeded {
        /// Which budget was exhausted.
        budget: GenerationBudgetKind,
        /// The runtime byte bound that was exceeded.
        limit_bytes: u32,
        /// The generated bytes observed when it was exceeded.
        observed_bytes: u32,
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
        }
    }
}

// ---------------------------------------------------------------------------
// Budgets
// ---------------------------------------------------------------------------

/// The resolved runtime generation-safety policy one physical generation is
/// enforced against.
///
/// This type is the **contract the guard enforces**, not the decision about
/// what the limits should be. Resolving a policy belongs to the layer that
/// already resolves a model invocation — see
/// [`ResolvedModelInvocation::generation_safety_policy`](crate::model::invocation::ResolvedModelInvocation::generation_safety_policy)
/// — so no relationship between the two bounds is embedded in the accounting
/// code. They are independent inputs:
///
/// ```text
/// max total output   !=   max reasoning
/// ```
///
/// means two dimensions a policy owner sets independently, not one derived
/// from the other.
///
/// The guarantee the guard provides for a resolved policy is exactly this,
/// and nothing more:
///
/// > One physical generation cannot stream more than `max_generated_bytes`
/// > of normalized generated data, nor — when a reasoning bound is present —
/// > more than `max_reasoning_bytes` of normalized reasoning text, before the
/// > runtime terminates it. Both numbers are UTF-8 **byte** counts of the
/// > adapter's normalized output, **not model tokens**, and neither replaces
/// > the provider-enforced `max_output_tokens`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GenerationSafetyPolicy {
    /// The byte bound of all normalized generated data of one generation:
    /// reasoning, text, refusal, and streamed tool-call arguments.
    pub max_generated_bytes: u64,
    /// The byte bound of normalized reasoning text alone, when the policy
    /// owner resolves a separate one.
    ///
    /// `None` has explicit semantics: **no separate runtime reasoning
    /// bound**. Reasoning bytes still count against
    /// [`Self::max_generated_bytes`] like every other generated byte, so the
    /// generation stays bounded; what is absent is the tighter, separately
    /// attributed reasoning limit. It is not "unlimited reasoning", and it is
    /// not zero.
    pub max_reasoning_bytes: Option<u64>,
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

/// The number of candidate periods one scan considers.
///
/// Each is answered from the prefix table in constant time, so this is the
/// entire per-scan candidate cost.
pub const DEGENERATION_CANDIDATE_PERIODS: usize =
    DEGENERATION_MAX_PERIOD_BYTES - DEGENERATION_MIN_PERIOD_BYTES + 1;

/// The exact upper bound on byte comparisons performed by one scan.
///
/// One scan runs the Knuth–Morris–Pratt prefix function once over the
/// window, which performs strictly fewer than `2 * n` character comparisons
/// for `n` bytes, and then answers every candidate period from that table in
/// constant time. This is a *hard* bound, reached or not reached regardless
/// of what the model generated: there is no input-dependent verification
/// pass to inflate it.
pub const DEGENERATION_MAX_COMPARISONS_PER_SCAN: u64 = 2 * DEGENERATION_WINDOW_BYTES as u64;

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

/// Deterministic work accounting, so a regression can assert the detector's
/// bound instead of measuring elapsed time.
#[cfg(test)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct DetectorWork {
    /// Completed checkpoint scans.
    pub(crate) scans: u64,
    /// Byte comparisons performed by the prefix-function passes.
    pub(crate) comparisons: u64,
    /// Candidate periods answered from the prefix table.
    pub(crate) candidate_checks: u64,
}

/// One channel's bounded deterministic repetition detector.
///
/// # State
///
/// Fixed and complete: a [`DEGENERATION_WINDOW_BYTES`]-byte ring window, one
/// prefix-function table of `DEGENERATION_WINDOW_BYTES + 1` `u16` entries,
/// one partial checkpoint block, and three counters. Nothing here grows with
/// the length of the generation, and nothing is allocated per delta.
///
/// # Work
///
/// A scan runs once per [`DEGENERATION_SCAN_STRIDE_BYTES`] and costs at most
/// [`DEGENERATION_MAX_COMPARISONS_PER_SCAN`] byte comparisons plus at most
/// [`DEGENERATION_CANDIDATE_PERIODS`] constant-time table lookups. Per
/// generated byte that is a fixed constant — under 70 byte comparisons — and
/// it is a **hard** bound rather than a typical case: there is no candidate
/// that can cost more than another, and no input can make a scan expensive.
///
/// # Method
///
/// The detector computes the Knuth–Morris–Pratt prefix function of the
/// *reversed* window, whose prefixes are the window's suffixes. For a string
/// of length `L` the smallest period is `L - pi[L]`, exactly, so one linear
/// pass answers "what is the primitive period of the last `L` bytes?" for
/// every `L` at once.
///
/// A candidate period `p` requiring span `L` is then answered in constant
/// time. By the periodicity lemma, when the smallest period `q` of a string
/// satisfies `q <= L / 2` — which holds here because the evidence threshold
/// forces `p <= L / 4` and `q <= p` — the periods of that string are exactly
/// the multiples of `q`. So `p` is a period **if and only if** `q` divides
/// `p`, and the primitive period of the span is `q` itself.
///
/// The classification is therefore exact and deterministic, with no
/// probabilistic filter, no hashing, no verification budget that could cause
/// a miss, and no backtracking.
#[derive(Debug)]
struct RepetitionDetector {
    window: Box<[u8; DEGENERATION_WINDOW_BYTES]>,
    /// The prefix function of the reversed committed window. `prefix[i]` is
    /// the longest proper border of the window's `i`-byte suffix, so
    /// `i - prefix[i]` is that suffix's smallest period.
    prefix: Box<[u16; DEGENERATION_WINDOW_BYTES + 1]>,
    /// The next write position in the ring.
    head: usize,
    /// How much of the ring holds committed bytes.
    filled: usize,
    /// The partially accumulated checkpoint block.
    pending: [u8; DEGENERATION_SCAN_STRIDE_BYTES],
    pending_len: usize,
    #[cfg(test)]
    work: DetectorWork,
}

impl Default for RepetitionDetector {
    fn default() -> Self {
        Self {
            window: Box::new([0_u8; DEGENERATION_WINDOW_BYTES]),
            prefix: Box::new([0_u16; DEGENERATION_WINDOW_BYTES + 1]),
            head: 0,
            filled: 0,
            pending: [0_u8; DEGENERATION_SCAN_STRIDE_BYTES],
            pending_len: 0,
            #[cfg(test)]
            work: DetectorWork::default(),
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
    /// window; `0` is the most recently committed byte. Reading the window
    /// backwards is what makes the prefix function below operate on the
    /// reversed string without ever materializing it.
    fn byte_at(&self, distance: usize) -> u8 {
        let index =
            (self.head + DEGENERATION_WINDOW_BYTES - 1 - distance) % DEGENERATION_WINDOW_BYTES;
        self.window[index]
    }

    /// Recomputes the prefix function of the reversed committed window.
    ///
    /// This is the standard Knuth–Morris–Pratt construction, which performs
    /// fewer than `2 * filled` character comparisons in total because the
    /// fallback variable decreases at least once for every comparison that
    /// fails and increases at most once per position.
    fn rebuild_prefix(&mut self) {
        self.prefix[0] = 0;
        if self.filled > 0 {
            self.prefix[1] = 0;
        }
        let mut border = 0_usize;
        for index in 1..self.filled {
            let current = self.byte_at(index);
            while border > 0 && current != self.byte_at(border) {
                #[cfg(test)]
                {
                    self.work.comparisons += 1;
                }
                border = usize::from(self.prefix[border]);
            }
            #[cfg(test)]
            {
                self.work.comparisons += 1;
            }
            if current == self.byte_at(border) {
                border += 1;
            }
            self.prefix[index + 1] = u16::try_from(border).unwrap_or(u16::MAX);
        }
    }

    /// The deterministic work this detector has performed.
    #[cfg(test)]
    const fn work(&self) -> DetectorWork {
        self.work
    }

    /// The smallest period of the window's `span`-byte suffix.
    fn smallest_period(&self, span: usize) -> usize {
        span - usize::from(self.prefix[span])
    }

    /// Classifies the committed window suffix.
    ///
    /// Every candidate is answered from the prefix table in constant time,
    /// so the scan's cost is the table construction plus one lookup per
    /// candidate — independent of what the model generated.
    fn scan(&mut self) -> Option<RepetitionEvidence> {
        if self.filled < DEGENERATION_MIN_SPAN_BYTES {
            return None;
        }
        self.rebuild_prefix();
        #[cfg(test)]
        {
            self.work.scans += 1;
        }
        for period in DEGENERATION_MIN_PERIOD_BYTES..=DEGENERATION_MAX_PERIOD_BYTES {
            let repetitions = required_repetitions(period);
            let span = period * repetitions;
            if span > self.filled {
                continue;
            }
            #[cfg(test)]
            {
                self.work.candidate_checks += 1;
            }
            let primitive = self.smallest_period(span);
            // The evidence threshold forces `span >= 4 * period`, and the
            // smallest period never exceeds a period, so the periodicity
            // lemma applies: the periods of this span are exactly the
            // multiples of its smallest period.
            if period % primitive != 0 {
                continue;
            }
            // A run of one delimiter is periodic at every multiple of its own
            // length, so the minimum period alone would not exclude it. The
            // evidence is the *primitive* period: a span that repeats
            // something shorter than the minimum unit is structure, not
            // degeneration, and no longer candidate over this window can be
            // anything else.
            if primitive < DEGENERATION_MIN_PERIOD_BYTES {
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
    policy: GenerationSafetyPolicy,
    generated_bytes: u64,
    reasoning_bytes: u64,
    reasoning: RepetitionDetector,
    content: RepetitionDetector,
}

impl GenerationGuard {
    /// Creates the guard of one physical generation from a resolved policy.
    #[must_use]
    pub fn new(policy: GenerationSafetyPolicy) -> Self {
        Self {
            policy,
            generated_bytes: 0,
            reasoning_bytes: 0,
            reasoning: RepetitionDetector::default(),
            content: RepetitionDetector::default(),
        }
    }

    /// The resolved policy this guard enforces.
    #[must_use]
    pub const fn policy(&self) -> GenerationSafetyPolicy {
        self.policy
    }

    /// The deterministic detector work of both channels, for the bounded-work
    /// regressions. It counts operations, never elapsed time.
    #[cfg(test)]
    pub(crate) fn detector_work(&self) -> DetectorWork {
        let reasoning = self.reasoning.work();
        let content = self.content.work();
        DetectorWork {
            scans: reasoning.scans + content.scans,
            comparisons: reasoning.comparisons + content.comparisons,
            candidate_checks: reasoning.candidate_checks + content.candidate_checks,
        }
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
            // A policy with no separate reasoning bound charges reasoning to
            // the total like any other generated byte and attributes nothing
            // to reasoning; it does not mean reasoning is unbounded.
            if let Some(limit) = self.policy.max_reasoning_bytes
                && self.reasoning_bytes > limit
            {
                return Some(GenerationFailure::RuntimeBudgetExceeded {
                    budget: GenerationBudgetKind::Reasoning,
                    limit_bytes: bounded_count(limit),
                    observed_bytes: bounded_count(self.reasoning_bytes),
                });
            }
        }
        self.charge_total(bytes)
    }

    fn charge_total(&mut self, bytes: u64) -> Option<GenerationFailure> {
        self.generated_bytes = self.generated_bytes.saturating_add(bytes);
        if self.generated_bytes <= self.policy.max_generated_bytes {
            return None;
        }
        Some(GenerationFailure::RuntimeBudgetExceeded {
            budget: GenerationBudgetKind::TotalOutput,
            limit_bytes: bounded_count(self.policy.max_generated_bytes),
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
        DEGENERATION_CANDIDATE_PERIODS, DEGENERATION_MAX_COMPARISONS_PER_SCAN,
        DEGENERATION_MAX_PERIOD_BYTES, DEGENERATION_MIN_SPAN_BYTES, DEGENERATION_SCAN_STRIDE_BYTES,
        GenerationBudgetKind, GenerationChannel, GenerationFailure, GenerationGuard,
        GenerationSafetyPolicy,
    };
    use std::fmt::Write as _;

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
        GenerationGuard::new(GenerationSafetyPolicy {
            max_generated_bytes: u64::MAX,
            max_reasoning_bytes: Some(u64::MAX),
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
        let mut guard = GenerationGuard::new(GenerationSafetyPolicy {
            max_generated_bytes: 0,
            max_reasoning_bytes: Some(0),
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
        let mut guard = GenerationGuard::new(GenerationSafetyPolicy {
            max_generated_bytes: 4_096,
            max_reasoning_bytes: Some(1_024),
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
        let mut guard = GenerationGuard::new(GenerationSafetyPolicy {
            max_generated_bytes: 1_024,
            max_reasoning_bytes: Some(1_024),
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
        let mut guard = GenerationGuard::new(GenerationSafetyPolicy {
            max_generated_bytes: 256,
            max_reasoning_bytes: Some(256),
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
        let mut guard = GenerationGuard::new(GenerationSafetyPolicy {
            max_generated_bytes: u64::try_from(DEGENERATION_MIN_SPAN_BYTES).expect("fits") + 8,
            max_reasoning_bytes: Some(u64::MAX),
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

    // -----------------------------------------------------------------
    // Policy semantics
    // -----------------------------------------------------------------

    /// The two bounds are independent inputs of the policy. A guard enforces
    /// whatever it is given, in either direction, and knows no ratio between
    /// them.
    #[test]
    fn total_output_and_reasoning_bounds_are_independent_inputs() {
        // Reasoning far below the total: the reasoning bound decides.
        let mut tight_reasoning = GenerationGuard::new(GenerationSafetyPolicy {
            max_generated_bytes: 1_000_000,
            max_reasoning_bytes: Some(64),
        });
        let mut observed = None;
        for index in 0..1_000_u32 {
            if let Some(failure) = tight_reasoning.observe(&reasoning_delta(&format!("t{index} ")))
            {
                observed = Some(failure);
                break;
            }
        }
        assert!(matches!(
            observed,
            Some(GenerationFailure::RuntimeBudgetExceeded {
                budget: GenerationBudgetKind::Reasoning,
                ..
            })
        ));

        // Reasoning *above* the total, which no ratio would ever produce: the
        // total decides, and the guard accepts the policy as given.
        let mut loose_reasoning = GenerationGuard::new(GenerationSafetyPolicy {
            max_generated_bytes: 64,
            max_reasoning_bytes: Some(1_000_000),
        });
        let mut observed = None;
        for index in 0..1_000_u32 {
            if let Some(failure) = loose_reasoning.observe(&reasoning_delta(&format!("t{index} ")))
            {
                observed = Some(failure);
                break;
            }
        }
        assert!(matches!(
            observed,
            Some(GenerationFailure::RuntimeBudgetExceeded {
                budget: GenerationBudgetKind::TotalOutput,
                ..
            })
        ));
    }

    /// A policy with no separate reasoning bound has explicit semantics:
    /// reasoning is still charged to the total, so the generation stays
    /// bounded, but nothing is attributed to a reasoning limit.
    #[test]
    fn an_absent_reasoning_bound_still_charges_reasoning_to_the_total() {
        let mut guard = GenerationGuard::new(GenerationSafetyPolicy {
            max_generated_bytes: 512,
            max_reasoning_bytes: None,
        });
        let mut observed = None;
        for index in 0..1_000_u32 {
            if let Some(failure) = guard.observe(&reasoning_delta(&format!("thought {index}; "))) {
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
                assert_eq!(
                    budget,
                    GenerationBudgetKind::TotalOutput,
                    "with no reasoning bound the total is the only bound, and it still binds"
                );
                assert_eq!(limit_bytes, 512);
                assert!(observed_bytes > 512);
            }
            other => panic!("expected a total-output budget fact, got {other:?}"),
        }
    }

    /// The runtime bound counts bytes, and says so: a multi-byte script
    /// consumes the bound by its UTF-8 length, exactly, with no tokenizer
    /// pretence anywhere.
    #[test]
    fn the_runtime_bound_counts_bytes_not_tokens() {
        // Four CJK characters are twelve UTF-8 bytes and would be roughly
        // four tokens. The bound is reached by bytes.
        let unit = "\u{6a21}\u{578b}\u{8f93}\u{51fa}";
        assert_eq!(unit.chars().count(), 4);
        assert_eq!(unit.len(), 12);
        let mut guard = GenerationGuard::new(GenerationSafetyPolicy {
            max_generated_bytes: 60,
            max_reasoning_bytes: None,
        });
        // Five deltas are 60 bytes and fit exactly; the sixth exceeds.
        for _ in 0..5 {
            assert_eq!(guard.observe(&text_delta(unit)), None);
        }
        match guard.observe(&text_delta(unit)) {
            Some(GenerationFailure::RuntimeBudgetExceeded { observed_bytes, .. }) => {
                assert_eq!(
                    observed_bytes, 72,
                    "the count is UTF-8 bytes, not characters"
                );
            }
            other => panic!("expected a total-output budget fact, got {other:?}"),
        }
    }

    // -----------------------------------------------------------------
    // Adversarial input and bounded work
    // -----------------------------------------------------------------

    /// A deliberately hostile near-periodic corpus.
    ///
    /// Every block is a long exactly-4-periodic run that stops just short of
    /// the evidence span and is then broken by one distinct byte. Any
    /// candidate-rejection scheme that samples a few positions sees a match
    /// for many candidate periods and only discovers the break late, which is
    /// exactly the shape that must not become a CPU surface. It is also
    /// genuinely non-degenerate: the breaks recur only every 1021 bytes, far
    /// beyond the longest recognized period.
    fn adversarial_near_periodic(blocks: usize) -> String {
        let mut corpus = String::new();
        for index in 0..blocks {
            corpus.push_str(&"abcd".repeat(255));
            corpus.push(char::from(b'A' + u8::try_from(index % 26).expect("small")));
        }
        corpus
    }

    /// The hostile corpus is not classified: it is near-periodic, never
    /// periodic across a full evidence span.
    #[test]
    fn adversarial_near_periodic_input_is_not_degeneration() {
        let corpus = adversarial_near_periodic(24);
        assert!(corpus.len() > DEGENERATION_MIN_SPAN_BYTES * 20);
        for chunk_size in [1_usize, 7, 64, 999] {
            let mut guard = integrity_guard();
            let mut rest = corpus.as_str();
            while !rest.is_empty() {
                let (head, tail) = rest.split_at(chunk_size.min(rest.len()));
                assert_eq!(
                    guard.observe(&text_delta(head)),
                    None,
                    "near-periodic output is not degeneration at chunk size {chunk_size}"
                );
                rest = tail;
            }
        }
    }

    /// A long run whose primitive period is below the minimum stays excluded
    /// even when it is broken and resumed, which is the other shape that used
    /// to force repeated candidate verification.
    #[test]
    fn a_broken_sub_minimal_period_run_is_not_degeneration() {
        let mut corpus = String::new();
        for index in 0..24_u32 {
            corpus.push_str(&"ab".repeat(600));
            write!(corpus, "<{index}>").expect("writing to a String cannot fail");
        }
        let mut guard = integrity_guard();
        assert_eq!(guard.observe(&text_delta(&corpus)), None);
    }

    /// The detector's work is bounded per scan by construction, and the bound
    /// does not depend on what the model generated.
    ///
    /// This is the regression that would catch reintroducing an
    /// input-dependent verification pass: the hostile corpus and an ordinary
    /// one of the same length must cost the same per scan, and both must stay
    /// inside the documented hard bound.
    #[test]
    fn detector_work_stays_inside_its_hard_bound_on_adversarial_input() {
        let adversarial = adversarial_near_periodic(24);
        let mut ordinary = String::new();
        for index in 0..2_000_u32 {
            write!(ordinary, "sentence number {index} of an ordinary answer. ")
                .expect("writing to a String cannot fail");
        }
        let ordinary = ordinary[..adversarial.len()].to_owned();

        let mut costs = Vec::new();
        for corpus in [&adversarial, &ordinary] {
            let mut guard = integrity_guard();
            assert_eq!(guard.observe(&text_delta(corpus)), None);
            let work = guard.detector_work();
            assert!(work.scans > 0, "the corpus must reach the scan threshold");
            assert!(
                work.comparisons <= work.scans * DEGENERATION_MAX_COMPARISONS_PER_SCAN,
                "prefix-function comparisons exceed the documented per-scan bound: {work:?}"
            );
            assert!(
                work.candidate_checks
                    <= work.scans * u64::try_from(DEGENERATION_CANDIDATE_PERIODS).expect("fits"),
                "candidate checks exceed one constant-time lookup per candidate: {work:?}"
            );
            costs.push(work);
        }

        // The hostile corpus costs no more per scan than the ordinary one.
        // An algorithm whose candidates are byte-verified on demand fails
        // here long before any wall-clock measurement would notice.
        let hostile_per_scan = costs[0].comparisons / costs[0].scans;
        let ordinary_per_scan = costs[1].comparisons / costs[1].scans;
        assert!(
            hostile_per_scan <= ordinary_per_scan * 2,
            "adversarial input must not cost materially more per scan: \
             {hostile_per_scan} vs {ordinary_per_scan}"
        );
    }

    /// Scans happen once per stride and only once the window can hold the
    /// evidence span, so the number of scans is a pure function of how many
    /// bytes were generated.
    #[test]
    fn scans_occur_once_per_checkpoint_after_the_span_threshold() {
        let bytes = DEGENERATION_SCAN_STRIDE_BYTES * 100;
        let mut guard = integrity_guard();
        // Aperiodic filler: a cycling alphabet would itself be a repeated
        // 26-byte unit and would classify, which is the detector working.
        let mut text = String::new();
        let mut index = 0_u32;
        while text.len() < bytes {
            write!(text, "segment {index} ").expect("writing to a String cannot fail");
            index += 1;
        }
        text.truncate(bytes);
        assert_eq!(guard.observe(&text_delta(&text)), None);
        let expected = (bytes / DEGENERATION_SCAN_STRIDE_BYTES)
            - (DEGENERATION_MIN_SPAN_BYTES / DEGENERATION_SCAN_STRIDE_BYTES)
            + 1;
        assert_eq!(
            guard.detector_work().scans,
            u64::try_from(expected).expect("fits")
        );
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
