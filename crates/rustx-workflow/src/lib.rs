//! The Workflow DSL contract: the serialized definition, file-document and
//! diagnostic vocabulary shared by the runtime engine and the durable
//! journal. No execution, compilation driver or runtime evidence lives here.

// Workspace-internal crates: error and panic conditions are documented in
// prose rather than `# Errors`/`# Panics` sections, and blanket must_use on
// internal APIs adds noise without callers.
#![allow(
    clippy::missing_errors_doc,
    clippy::missing_panics_doc,
    clippy::must_use_candidate
)]

pub mod code_language;
pub mod compile_error;
pub mod definition;
pub mod list;
pub mod nav;
pub mod review;
pub mod selections;
pub mod tool_result;
pub mod workflow_file;

pub use code_language::WorkflowCodeLanguage;
pub use compile_error::WorkflowCompileError;
pub use definition::*;
pub use list::*;
pub use review::{MAX_REVIEW_ACTIONS, REVIEW_TIMEOUT_PORT, WorkflowReviewAction};
pub use selections::{WorkflowModelPolicy, WorkflowToolSelector};
pub use tool_result::WorkflowToolResult;
pub use workflow_file::*;
