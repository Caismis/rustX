//! Shared types for ZCode-derived Bash read-only analysis.

#![allow(dead_code)]

use std::collections::HashMap;
use std::sync::Arc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BashCommandOperator {
    And,
    Or,
    Pipe,
    PipeStderr,
    Sequence,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BashEnvAssignment {
    pub name: Option<String>,
    pub value: Option<String>,
    pub value_dynamic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BashRedirect {
    pub file_descriptor: Option<i32>,
    pub operator: String,
    pub target: String,
    pub dynamic: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BashCommandInvocation {
    pub argv: Vec<String>,
    pub command_text: String,
    pub env_assignments: Vec<BashEnvAssignment>,
    pub has_dynamic_words: bool,
    pub has_redirects: bool,
    pub name: String,
    pub operator_before: Option<BashCommandOperator>,
    pub redirects: Vec<BashRedirect>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BashAstAnalysis {
    pub commands: Vec<BashCommandInvocation>,
    pub has_dynamic_words: bool,
    pub has_parse_errors: bool,
    pub has_redirects: bool,
    pub has_unsupported_syntax: bool,
    pub unsupported_node_types: Vec<String>,
}

impl BashAstAnalysis {
    #[must_use]
    pub fn empty() -> Self {
        Self {
            commands: Vec::new(),
            has_dynamic_words: false,
            has_parse_errors: false,
            has_redirects: false,
            has_unsupported_syntax: false,
            unsupported_node_types: Vec::new(),
        }
    }

    #[must_use]
    pub fn is_permission_safe(&self) -> bool {
        !self.has_parse_errors && !self.has_unsupported_syntax && !self.has_dynamic_words
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SafeFlagValue {
    None,
    Number,
    String,
    OptionalString,
    Char,
    BracePair,
    Eof,
}

#[derive(Debug, Clone)]
pub struct BashReadonlyCommandPolicy {
    pub allow_any_args: bool,
    pub command_only: bool,
    pub allow_compact_numeric_count_flag: bool,
    pub respects_double_dash: bool,
    pub safe_flags: Option<Arc<HashMap<&'static str, SafeFlagValue>>>,
    pub danger_callback: Option<DangerCallbackId>,
}

impl Default for BashReadonlyCommandPolicy {
    fn default() -> Self {
        Self {
            allow_any_args: false,
            command_only: false,
            allow_compact_numeric_count_flag: false,
            respects_double_dash: true,
            safe_flags: None,
            danger_callback: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DangerCallbackId {
    Date,
    Jq,
    Lsof,
    Man,
    Ps,
    Pyright,
    Sed,
    Ss,
    Test,
    Tput,
    Xargs,
    Docker,
    Gh,
    GitBranch,
    GitRevisionFormat,
    GitReflog,
    GitLsRemote,
    GitRemoteShow,
    GitTag,
    GitRemoteOnlyVerbose,
}
