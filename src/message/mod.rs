//! Canonical message blocks, content blocks, provenance, and conversation history types.
//!
//! M1 implements the frozen three-role message model in [`types`] and the
//! shared content/reference blocks in [`content`]. History assembly,
//! compaction, and context compilation are owned by the conversation/context
//! layers; this module remains the canonical message vocabulary.

pub mod content;
pub mod types;

pub use content::{FileReference, ImageReference, TextBlock};
pub use types::{
    AgentStatusEmission, AgentStatusGenerationMetadata, AgentStatusMetadataError,
    AgentStatusModuleId, AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex,
    ContextKind, InboundKind, MessageBlock, ReasoningBlock, RefusalBlock, ToolMessageBlock,
    UserContentBlock, UserMessageBlock, UserSource,
};
