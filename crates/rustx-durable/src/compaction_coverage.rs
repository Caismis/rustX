//! Validate and bind a Compaction Summary to the captured prefix it covers.

use std::cmp::Ordering;
use std::collections::BTreeMap;

use rustx_types::context::checkpoint::SummaryGenerationRef;
use rustx_types::context::coverage::PrefixCoverage;
use rustx_types::context::error::{ContextError, ContextErrorKind};
use rustx_types::context::resources::ResourceReadCut;
use rustx_types::model::input::ModelInputMessage;
use rustx_types::model::projected::{
    CanonicalMessageRef, CheckpointSourceRef, DerivedModelMessage, ProjectedMessage,
    SemanticTransformation,
};
use rustx_types::model::selected_input::SelectedModelInput;
use rustx_types::runtime::identity::ConversationId;
use sha2::{Digest, Sha256};

/// Process-local generation proof. It is never deserialized from a checkpoint
/// or accepted from an external caller; a native generator seals it only from
/// an accepted complete `ChunkPlan` witness and its exact source/output values.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VerifiedPrefixCoverage {
    record: PrefixCoverage,
    summary_digest: String,
    generation: Option<SummaryGenerationRef>,
    current_reduction: Option<CheckpointSourceRef>,
}

impl VerifiedPrefixCoverage {
    /// Bind an accepted generation's record to its frozen source and exact Summary.
    /// The SQLite commit independently authenticates both against its durable cut.
    pub fn seal(
        record: PrefixCoverage,
        input: &SelectedModelInput,
        predecessor: Option<(&CheckpointSourceRef, Option<&PrefixCoverage>)>,
        summary: &DerivedModelMessage,
        generation: Option<&SummaryGenerationRef>,
        allow_current_reduction: bool,
    ) -> Result<Self, ContextError> {
        let cut = &record.capture_cut;
        let count = record.captured_prefix_items as usize;
        if capture_digest(cut, input, count)? != record.capture_digest
            || generation.is_some_and(|generation| {
                generation.conversation_id != cut.conversation_id
                    || generation.through_journal <= cut.journal.0
                    || generation.call_id.as_str().is_empty()
            })
        {
            return Err(ContextError::new(
                ContextErrorKind::MalformedHistory,
                "completed Summary witness disagrees with its frozen capture or generation",
            ));
        }
        validate_summary_sources(
            &cut.conversation_id,
            input,
            count,
            predecessor.map(|(source, _)| source),
            summary,
        )?;
        let current_reduction = predecessor
            .filter(|(_, prior)| {
                allow_current_reduction
                    && count == 1
                    && prior.is_some()
                    && matches!(input.first(), Some(ModelInputMessage::Derived(message))
                        if message.transformations.contains(&SemanticTransformation::Summary))
            })
            .map(|(source, _)| source.clone());
        Ok(Self {
            record,
            summary_digest: summary_digest(summary, generation)?,
            generation: generation.cloned(),
            current_reduction,
        })
    }

    /// Interpret a sealed current-owner reduction only at its exact Head.
    /// Serialized coverage alone can never authorize an equal-coverage rewrite.
    pub(crate) fn retained_offset(
        &self,
        head: Option<(&CheckpointSourceRef, Option<&PrefixCoverage>)>,
    ) -> Option<usize> {
        self.record.retained_offset(head).or_else(|| {
            (self.current_reduction.as_ref() == head.map(|(source, _)| source)
                && self.current_reduction.is_some()
                && self.record.compare_to_head(head) == Some(Ordering::Equal))
            .then_some(1)
        })
    }

    /// The bounded durable record described by this process-local proof.
    #[must_use]
    pub const fn record(&self) -> &PrefixCoverage {
        &self.record
    }

    /// Check the exact record, generated Summary and Call admitted at commit.
    pub fn authorizes(
        &self,
        record: &PrefixCoverage,
        summary: &DerivedModelMessage,
        generation: Option<&SummaryGenerationRef>,
    ) -> bool {
        &self.record == record
            && self.generation.as_ref() == generation
            && summary_digest(summary, generation).is_ok_and(|digest| digest == self.summary_digest)
    }
}

fn summary_digest(
    summary: &DerivedModelMessage,
    generation: Option<&SummaryGenerationRef>,
) -> Result<String, ContextError> {
    struct Writer<'a>(&'a mut Sha256, &'a mut usize);
    impl std::io::Write for Writer<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            *self.1 = self
                .1
                .checked_sub(bytes.len())
                .ok_or_else(|| std::io::Error::other("summary proof exceeds byte bound"))?;
            self.0.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut hash = Sha256::new();
    let mut remaining = 8 * 1024 * 1024_usize;
    serde_json::to_writer(
        Writer(&mut hash, &mut remaining),
        &("rustx-summary-output-v1", summary, generation),
    )
    .map_err(|_| {
        ContextError::new(
            ContextErrorKind::CannotFit,
            "summary proof exceeds byte bound",
        )
    })?;
    Ok(format!("{:x}", hash.finalize()))
}

fn validate_summary_sources(
    scope: &ConversationId,
    input: &SelectedModelInput,
    count: usize,
    predecessor: Option<&CheckpointSourceRef>,
    summary: &DerivedModelMessage,
) -> Result<(), ContextError> {
    let invalid = || {
        ContextError::new(
            ContextErrorKind::MalformedHistory,
            "generated Summary does not name its complete captured prefix sources",
        )
    };
    if !matches!(
        &summary.message,
        ProjectedMessage::User {
            source: rustx_types::message::types::UserSource::Runtime,
            kind: rustx_types::model::projected::ProjectedUserKind::CompactionContext,
            ..
        }
    ) || summary.transformations != [SemanticTransformation::Summary]
        || count > input.len()
    {
        return Err(invalid());
    }
    let mut originals = BTreeMap::new();
    let mut checkpoints = BTreeMap::new();
    let mut includes_previous_summary = false;
    for message in input.iter().take(count) {
        match message {
            ModelInputMessage::Canonical(original) => {
                let source = CanonicalMessageRef::of(scope, original);
                originals.insert(source.message_id.clone(), source);
            }
            ModelInputMessage::Derived(derived)
                if derived
                    .transformations
                    .contains(&SemanticTransformation::Summary) =>
            {
                includes_previous_summary = true;
            }
            ModelInputMessage::Derived(derived) => {
                for source in &derived.sources {
                    originals.insert(source.message_id.clone(), source.clone());
                }
                for source in &derived.checkpoint_sources {
                    checkpoints.insert(source.checkpoint_id.clone(), source.clone());
                }
            }
            ModelInputMessage::RequestOnly(_) => return Err(invalid()),
        }
    }
    if includes_previous_summary {
        let prior = predecessor.ok_or_else(invalid)?;
        checkpoints.insert(prior.checkpoint_id.clone(), prior.clone());
    }
    if summary.sources != originals.into_values().collect::<Vec<_>>()
        || summary.checkpoint_sources != checkpoints.into_values().collect::<Vec<_>>()
    {
        return Err(invalid());
    }
    Ok(())
}

/// Hash the typed original projection occurrences and capture cut without
/// materializing encoded source copies. The same value binds a completed
/// chunk witness to the exact prefix that the Store can independently read.
pub fn capture_digest(
    cut: &ResourceReadCut,
    input: &SelectedModelInput,
    count: usize,
) -> Result<String, ContextError> {
    struct DigestBudget<'a> {
        digest: &'a mut Sha256,
        remaining: usize,
    }
    impl std::io::Write for DigestBudget<'_> {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            self.remaining = self
                .remaining
                .checked_sub(bytes.len())
                .ok_or_else(|| std::io::Error::other("capture identity exceeds its byte budget"))?;
            self.digest.update(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let limit = crate::context_resources::ResourceLimits::default();
    if count == 0 || count > limit.total_items || count > input.len() {
        return Err(ContextError::new(
            ContextErrorKind::CannotFit,
            "captured prefix item count exceeds its source or resource bounds",
        ));
    }
    let prefix = input.range(0..count);
    let mut hash = Sha256::new();
    serde_json::to_writer(
        DigestBudget {
            digest: &mut hash,
            remaining: limit.total_bytes,
        },
        &("rustx-compaction-prefix-v1", cut, prefix),
    )
    .map_err(|_| {
        ContextError::new(
            ContextErrorKind::CannotFit,
            "capture identity exceeds its byte budget",
        )
    })?;
    Ok(format!("{:x}", hash.finalize()))
}
