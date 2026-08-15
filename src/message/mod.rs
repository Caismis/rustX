//! Canonical message blocks, content blocks, provenance, and conversation history types.
//!
//! M1 implements the frozen four-role message model in [`types`] and the
//! shared content/reference blocks in [`content`]. History assembly,
//! compaction, and context compilation are later milestones.

pub mod content;
pub mod types;

pub use content::{FileReference, ImageReference, TextBlock};
pub use types::{
    AssistantContentBlock, AssistantMessageBlock, ContentBlockIndex, InboundKind, MessageBlock,
    ReasoningBlock, RefusalBlock, SystemAuthority, SystemMessageBlock, ToolMessageBlock,
    UserContentBlock, UserMessageBlock, UserSource,
};
