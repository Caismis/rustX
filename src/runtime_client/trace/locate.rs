//! Exact Tool-occurrence navigation from indexed native commit evidence.
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{TRACE_PAGE_LIMIT, TraceCursor, TracePage, TraceProjection};
use crate::durable::ConversationStoreError;
use crate::runtime::identity::{ToolCallId, ToolId};

/// Native occurrence identity owns navigation; optional inline IDs are checks.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceToolLocator {
    pub occurrence: crate::runtime_client::snapshot::RuntimeClientToolOccurrence,
    pub call_id: Option<ToolCallId>,
    pub tool_id: Option<ToolId>,
}

/// A contiguous native history window ending at the selected execution.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TraceToolLocation {
    pub record_id: String,
    pub page: TracePage,
}

impl TraceProjection<'_> {
    /// Resolves an exact occurrence against a durable source-owned read cut.
    /// # Errors
    /// A modified locator or unavailable read cut is an explicit source error.
    pub(crate) fn locate_tool_at(
        &self,
        locator: &TraceToolLocator,
        cut: &crate::durable::observation::ObservationReadCut,
    ) -> Result<Option<TraceToolLocation>, ConversationStoreError> {
        if locator
            .call_id
            .as_ref()
            .is_some_and(|id| id.as_str().len() > 256)
            || locator
                .tool_id
                .as_ref()
                .is_some_and(|id| id.as_str().len() > 256)
            || matches!(&locator.occurrence.locator,crate::runtime_client::content::CanonicalContentLocator::ByMessageId {message_id} if message_id.as_str().len()>256)
            || cut != self.cut()?
        {
            return Err(ConversationStoreError::InvalidReference(
                "invalid Tool occurrence read evidence".into(),
            ));
        }
        let page = self.store.tool_association_headers_by_locator(
            cut,
            &locator.occurrence.locator,
            u64::from(locator.occurrence.block_index.get()),
            1,
        )?;
        let Some(header) = page
            .headers
            .first()
            .filter(|header| header.block_index == locator.occurrence.block_index)
        else {
            return Ok(None);
        };
        if locator
            .call_id
            .as_ref()
            .is_some_and(|id| header.call_id.as_ref() != Some(id))
            || locator
                .tool_id
                .as_ref()
                .is_some_and(|id| header.tool_id.as_ref() != Some(id))
        {
            return Ok(None);
        }
        let crate::runtime_client::content::CanonicalContentLocator::ByLedgerPosition { position } =
            page.locator
        else {
            return Err(ConversationStoreError::InvalidReference(
                "Tool occurrence has no native canonical position".into(),
            ));
        };
        let Some(sequence) = self.store.observation_tool_trace_sequence(
            cut,
            position,
            locator.occurrence.block_index,
        )?
        else {
            return Ok(None);
        };
        let cursor = TraceCursor::at(sequence);
        let end = TraceCursor::at(sequence.checked_add(1).ok_or_else(|| {
            ConversationStoreError::InvalidReference("Trace cursor exhausted".into())
        })?);
        Ok(Some(TraceToolLocation {
            record_id: cursor.0,
            page: self.page(Some(&end), TRACE_PAGE_LIMIT)?,
        }))
    }

    /// Locates a canonical occurrence at the complete cut captured by `new`.
    /// A J-only `through` projection requires explicit `locate_tool_at` evidence.
    /// # Errors
    /// Invalid identities and durable read failures are explicit errors.
    pub fn locate_tool(
        &self,
        locator: &TraceToolLocator,
    ) -> Result<Option<TraceToolLocation>, ConversationStoreError> {
        let cut = self.source_cut.as_ref().ok_or_else(|| {
            ConversationStoreError::InvalidReference(
                "Tool occurrence requires complete native source cut".into(),
            )
        })?;
        self.locate_tool_at(locator, cut)
    }
}
