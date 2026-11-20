//! Bounded scheduled-start contracts over native immutable resources.
//!
//! Preparation performs no graph effects and grants no execution authority.
//! The Workflow owner must still retain/revalidate these resources through its
//! configuration/lifecycle fence and durably claim the exact start before use.
pub mod lifecycle;

use crate::scheduler::ScheduleBinding;
use rustx_types::capabilities::ToolSourceId;
use rustx_types::capabilities::selection::ExactToolSelector;
use rustx_types::runtime::identity::{AgentId, RuntimeResourceRevision};
use rustx_types::runtime::subagent::InheritedExecutionPolicy;
use rustx_types::runtime::types::ApprovalMode;
use rustx_types::runtime::workflow::WorkflowToolIdentity;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Changes to compilation or dependency framing require a new contract.
pub const SCHEDULED_WORKFLOW_COMPILER_CONTRACT: u32 = 3;
/// Complete persisted descriptor bound, independent of argument payloads.
pub const MAX_SCHEDULED_WORKFLOW_CONTRACT_BYTES: usize = 64 * 1024;
/// Same initial argument budget as Scheduler reminder text.
pub const MAX_SCHEDULED_WORKFLOW_ARGUMENT_BYTES: usize = 16 * 1024;

/// Derived native graph limits, checked independently of the definition digest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduledWorkflowLimits {
    pub steps: u32,
    pub agents: u32,
    pub model_calls: u32,
    pub retained_bytes: u32,
    pub timeout_ms: u64,
}

/// Provider and endpoint semantics, excluding credential source and value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduledProviderIdentity {
    pub provider: String,
    pub endpoint_digest: String,
}

/// Selected source's native transport, policy and credential-reference contract.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ScheduledSourceIdentity {
    pub source: ToolSourceId,
    pub materialization_digest: String,
}

/// Complete effective dependencies at one static graph location.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ScheduledWorkflowDependency {
    Tool {
        path: Vec<String>,
        selector: ExactToolSelector,
        definition_digest: String,
        source: Option<ScheduledSourceIdentity>,
    },
    Model {
        path: Vec<String>,
        invocation_digest: String,
        provider: ScheduledProviderIdentity,
    },
    Agent {
        path: Vec<String>,
        definition_digest: String,
        profile_digest: String,
        primary_provider: ScheduledProviderIdentity,
        summary_provider: Option<ScheduledProviderIdentity>,
        sources: Vec<ScheduledSourceIdentity>,
    },
}

/// Inert persistence contract; this value never authorizes a start or Claim.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PinnedScheduledWorkflow {
    pub compiler_contract: u32,
    pub binding: ScheduleBinding,
    pub workspace: PathBuf,
    pub root_agent_id: AgentId,
    pub root_extensions_digest: String,
    pub environment_digest: String,
    pub definition: WorkflowToolIdentity,
    pub arguments_digest: String,
    pub limits: ScheduledWorkflowLimits,
    pub execution_policy: InheritedExecutionPolicy,
    pub approval_mode: ApprovalMode,
    pub dependencies: Vec<ScheduledWorkflowDependency>,
    /// Audit evidence only; unrelated generation changes are not mismatches.
    pub observed_resource_revision: RuntimeResourceRevision,
}
