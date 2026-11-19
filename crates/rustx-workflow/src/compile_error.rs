//! The compile-time diagnostic contract of a Workflow definition.

use std::fmt;

#[derive(Debug, PartialEq, Eq)]
pub enum WorkflowCompileError {
    /// Authored field context supplied by the compiler, never inferred from prose.
    At { path: String, cause: Box<Self> },
    /// A required textual field is empty or over its bound.
    InvalidField(String),
    /// A JSON Schema is invalid or not a root object schema.
    InvalidSchema(String),
    /// A node or edge refers to an unknown id.
    DanglingReference(String),
    /// The graph has no single explicit entry.
    InvalidEntry(String),
    /// The graph contains a cycle; node is the smallest positive-indegree Kahn residual.
    Cycle { node: String },
    /// A node is unreachable from the explicit entry.
    Unreachable(String),
    /// A path can leave a node without a deterministic successor/terminal.
    Unterminated(String),
    /// A Branch has invalid cases or an incomplete set of exit connections.
    InvalidBranch(String),
    /// A workflow reference cannot be resolved on every relevant path.
    InvalidReference(String),
    /// A reference has an incompatible statically known schema.
    IncompatibleReference(String),
}

impl fmt::Display for WorkflowCompileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::At { path, cause } => write!(formatter, "{path}: {cause}"),
            Self::InvalidField(detail)
            | Self::InvalidSchema(detail)
            | Self::DanglingReference(detail)
            | Self::InvalidEntry(detail)
            | Self::Unterminated(detail)
            | Self::InvalidBranch(detail)
            | Self::InvalidReference(detail)
            | Self::IncompatibleReference(detail) => formatter.write_str(detail),
            Self::Cycle { node } => write!(
                formatter,
                "workflow graph contains a cycle; residual node {node:?}"
            ),
            Self::Unreachable(node) => write!(formatter, "workflow node {node:?} is unreachable"),
        }
    }
}

impl std::error::Error for WorkflowCompileError {}

impl WorkflowCompileError {
    pub fn at(self, prefix: impl AsRef<str>) -> Self {
        match self {
            Self::At { path, cause } => Self::At {
                path: format!("{}.{path}", prefix.as_ref()),
                cause,
            },
            cause => Self::At {
                path: prefix.as_ref().into(),
                cause: Box::new(cause),
            },
        }
    }

    /// The deepest native compiler rejection, independent of authored context.
    #[must_use]
    pub fn cause(&self) -> &Self {
        match self {
            Self::At { cause, .. } => cause.cause(),
            cause => cause,
        }
    }

    /// Authored field path, supplied by the validation owner.
    #[must_use]
    pub fn path(&self) -> &str {
        match self {
            Self::At { path, .. } => path,
            _ => "$",
        }
    }
}
