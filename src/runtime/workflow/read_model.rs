//! Native, process-local Workflow read authority. No journal input or executor retention.
//!
//! Mutations and revision installation commit under this leaf mutex. Readers
//! copy one coherent cut; wakeups carry no state and run after unlocking.
//! The client may coalesce cuts, but must invalidate replay when revisions
//! were skipped. Workflow revision is neither a journal sequence nor a cursor.

use std::sync::{Arc, Mutex};

use serde::{Deserialize, Serialize};

use super::{
    WorkflowBlockInstance, WorkflowBlockProgram, WorkflowExecutionOutcome, WorkflowId,
    WorkflowLoopExit, WorkflowNodeInstance, WorkflowNodeProgram, WorkflowRunId,
};
use crate::runtime::identity::{SubagentId, ToolCallId};
use crate::runtime::workspace::CandidateReference;

pub const MAX_PROJECTED_RUNS: usize = 8;
pub const MAX_PROJECTED_INSTANCES: usize = 512;
pub const MAX_PROJECTED_FINISHED: usize = 128;
pub const MAX_PROJECTED_RUN_BYTES: usize = 256 * 1024;
pub const MAX_WORKFLOW_CUTS: usize = 128;
pub const MAX_WORKFLOW_CUT_BYTES: usize = 8 * 1024 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct WorkflowRevision(pub u64);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowSnapshot {
    pub revision: WorkflowRevision,
    pub runs: Vec<WorkflowRunView>,
    pub omitted_runs: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowRunView {
    pub id: WorkflowRunId,
    pub workflow_id: WorkflowId,
    pub program_digest: String,
    pub resource_revision: crate::runtime::identity::RuntimeResourceRevision,
    pub tool_call_id: ToolCallId,
    pub state: WorkflowState,
    pub instances: Vec<WorkflowInstanceView>,
    pub omitted_instances: u64,
    pub steps_consumed: usize,
    pub steps_max: usize,
    pub agents_consumed: usize,
    pub candidate: Option<CandidateReference>,
    /// Admitted candidate users, including serialized workspace waiters.
    /// While nonzero no historical check certifies the mutable workspace.
    pub candidate_users: usize,
    pub handoff: Option<WorkflowHandoff>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowHandoff {
    pub state: String,
    pub path: String,
    pub truncated: bool,
}

impl From<&crate::runtime::workspace::WorkspaceSettlement> for WorkflowHandoff {
    fn from(value: &crate::runtime::workspace::WorkspaceSettlement) -> Self {
        use crate::runtime::workspace::WorkspaceSettlementDisposition as D;
        let state = match value.disposition {
            D::Borrowed => "borrowed",
            D::Shared => "shared",
            D::Removed => "removed",
            D::Retained { .. } => "retained",
            D::PreservedUnresolved { .. } => "preserved_unresolved",
        };
        let path = value.snapshot.logical_workspace.to_string_lossy();
        let truncated = path.len() > 1024;
        Self {
            state: state.into(),
            path: super::bound_workflow_diagnostic(path.into_owned()),
            truncated,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum WorkflowState {
    Pending,
    Running,
    Waiting { reason: WorkflowWait },
    Draining,
    Settled { outcome: WorkflowExecutionOutcome },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowWait {
    Tool,
    Agent,
    Capacity,
    Workspace,
    Questionnaire,
    Approval,
    Review,
    Settlement,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkflowNodeKind {
    Block,
    Agent,
    Tool,
    Branch,
    Parallel,
    Review,
    Loop,
    Return,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WorkflowInstanceView {
    pub block: WorkflowBlockInstance,
    pub node: Option<String>,
    pub visit: Option<u32>,
    pub kind: WorkflowNodeKind,
    pub state: WorkflowState,
    pub child: Option<SubagentId>,
    pub invocation: Option<crate::tools::types::ToolInvocationId>,
    pub tool_id: Option<crate::runtime::identity::ToolId>,
    pub interaction: Option<crate::runtime::interaction::InteractionRef>,
    pub iteration: Option<u32>,
    pub iterations_max: Option<u32>,
    pub loop_exit: Option<WorkflowLoopExit>,
    pub candidate: Option<CandidateReference>,
    pub checks_passed: Option<bool>,
    pub review_accepted: Option<bool>,
}

impl WorkflowInstanceView {
    fn new(block: WorkflowBlockInstance, node: Option<String>, kind: WorkflowNodeKind) -> Self {
        Self {
            block,
            visit: node.as_ref().map(|_| 0),
            node,
            kind,
            state: WorkflowState::Pending,
            child: None,
            invocation: None,
            tool_id: None,
            interaction: None,
            iteration: None,
            iterations_max: None,
            loop_exit: None,
            candidate: None,
            checks_passed: None,
            review_accepted: None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct WorkflowReadModel {
    conversation_id: crate::runtime::identity::ConversationId,
    state: Arc<Mutex<NativeReadState>>,
    wake: tokio::sync::watch::Sender<()>,
}

#[derive(Debug, Default)]
struct NativeReadState {
    current: WorkflowSnapshot,
    cuts: std::collections::VecDeque<(WorkflowSnapshot, usize)>,
    bytes: usize,
}

impl WorkflowReadModel {
    /// Constructs an empty process-owned read model for one conversation.
    #[must_use]
    pub fn new(conversation_id: crate::runtime::identity::ConversationId) -> Self {
        Self {
            conversation_id,
            state: Arc::new(Mutex::new(NativeReadState::default())),
            wake: tokio::sync::watch::Sender::new(()),
        }
    }
}

impl WorkflowReadModel {
    /// Copies one coherent native cut.
    ///
    /// # Panics
    /// Panics if a prior native mutation poisoned the leaf state mutex.
    #[must_use]
    pub fn snapshot(&self) -> WorkflowSnapshot {
        self.state
            .lock()
            .expect("Workflow read state")
            .current
            .clone()
    }

    /// Complete native cuts, never deltas reconstructed from evidence. The
    /// latest cut is repeated when unchanged so independent interaction
    /// projection facts can be correlated under the client's own lock.
    pub(crate) fn cuts_after(&self, revision: WorkflowRevision) -> Vec<WorkflowSnapshot> {
        let state = self.state.lock().expect("Workflow read state");
        let cuts: Vec<_> = state
            .cuts
            .iter()
            .filter(|(cut, _)| cut.revision > revision)
            .map(|(cut, _)| cut.clone())
            .collect();
        if cuts.is_empty() {
            vec![state.current.clone()]
        } else {
            cuts
        }
    }

    pub(crate) fn subscribe(&self) -> tokio::sync::watch::Receiver<()> {
        self.wake.subscribe()
    }

    fn commit(&self, mutate: impl FnOnce(&mut WorkflowSnapshot) -> bool) {
        let changed = {
            let mut state = self.state.lock().expect("Workflow read state");
            if mutate(&mut state.current) {
                state.current.revision.0 = state
                    .current
                    .revision
                    .0
                    .checked_add(1)
                    .expect("Workflow revision exhausted");
                let cut = state.current.clone();
                let bytes = serde_json::to_vec(&cut)
                    .expect("native cut serialization")
                    .len();
                state.bytes += bytes;
                state.cuts.push_back((cut, bytes));
                while state.cuts.len() > MAX_WORKFLOW_CUTS || state.bytes > MAX_WORKFLOW_CUT_BYTES {
                    let (_, bytes) = state.cuts.pop_front().expect("over capacity native cuts");
                    state.bytes -= bytes;
                }
                true
            } else {
                false
            }
        };
        if changed {
            self.wake.send_replace(());
        }
    }

    pub(super) fn register(&self, run: WorkflowRunView) {
        if run.id.conversation_id != self.conversation_id {
            return;
        }
        self.commit(|state| {
            if serde_json::to_vec(&run)
                .expect("Workflow view serialization")
                .len()
                > MAX_PROJECTED_RUN_BYTES
            {
                state.omitted_runs = state.omitted_runs.saturating_add(1);
                return true;
            }
            if state.runs.iter().any(|old| old.id == run.id) {
                return false;
            }
            if state.runs.len() == MAX_PROJECTED_RUNS {
                let index = state
                    .runs
                    .iter()
                    .position(|run| matches!(run.state, WorkflowState::Settled { .. }))
                    .unwrap_or(0);
                state.runs.remove(index);
                state.omitted_runs = state.omitted_runs.saturating_add(1);
            }
            state.runs.push(run);
            true
        });
    }

    pub(super) fn update(&self, id: &WorkflowRunId, mutate: impl FnOnce(&mut WorkflowRunView)) {
        self.commit(|state| {
            let Some(run) = state.runs.iter_mut().find(|run| &run.id == id) else {
                return false;
            };
            if matches!(run.state, WorkflowState::Settled { .. }) {
                return false;
            }
            mutate(run);
            bound(run);
            true
        });
    }

    pub(super) fn block(
        &self,
        id: &WorkflowRunId,
        block: &WorkflowBlockProgram,
        instance: &WorkflowBlockInstance,
    ) {
        self.update(id, |run| {
            let mut view =
                WorkflowInstanceView::new(instance.clone(), None, WorkflowNodeKind::Block);
            view.state = WorkflowState::Running;
            run.instances.push(view);
            for (key, node) in &block.nodes {
                let kind = match node {
                    WorkflowNodeProgram::Agent(_) => WorkflowNodeKind::Agent,
                    WorkflowNodeProgram::Tool { .. } => WorkflowNodeKind::Tool,
                    WorkflowNodeProgram::Branch { .. } => WorkflowNodeKind::Branch,
                    WorkflowNodeProgram::Parallel { .. } => WorkflowNodeKind::Parallel,
                    WorkflowNodeProgram::Review { .. } => WorkflowNodeKind::Review,
                    WorkflowNodeProgram::Loop { .. } => WorkflowNodeKind::Loop,
                    WorkflowNodeProgram::Return { .. } => WorkflowNodeKind::Return,
                };
                let mut view = WorkflowInstanceView::new(instance.clone(), Some(key.clone()), kind);
                if let WorkflowNodeProgram::Loop { max_iterations, .. } = node {
                    view.iterations_max = Some(*max_iterations);
                }
                run.instances.push(view);
            }
        });
    }

    pub(super) fn node(
        &self,
        node: &WorkflowNodeInstance,
        mutate: impl FnOnce(&mut WorkflowInstanceView),
    ) {
        self.update(&node.block.run, |run| {
            if let Some(view) = run.instances.iter_mut().find(|view| {
                view.block == node.block
                    && view.node.as_ref() == Some(&node.node)
                    && view.visit == Some(node.visit)
            }) {
                mutate(view);
            }
        });
    }

    pub(super) fn draining(&self, id: &WorkflowRunId) {
        self.commit(|state| {
            let Some(run) = state.runs.iter_mut().find(|run| &run.id == id) else {
                return false;
            };
            if matches!(
                run.state,
                WorkflowState::Draining | WorkflowState::Settled { .. }
            ) {
                return false;
            }
            run.state = WorkflowState::Draining;
            true
        });
    }
}

fn bound(run: &mut WorkflowRunView) {
    loop {
        let finished = run
            .instances
            .iter()
            .filter(|node| matches!(node.state, WorkflowState::Settled { .. }))
            .count();
        if run.instances.len() <= MAX_PROJECTED_INSTANCES
            && finished <= MAX_PROJECTED_FINISHED
            && serde_json::to_vec(run)
                .expect("Workflow view serialization")
                .len()
                <= MAX_PROJECTED_RUN_BYTES
        {
            break;
        }
        let index = run
            .instances
            .iter()
            .position(|node| matches!(node.state, WorkflowState::Settled { .. }))
            .or_else(|| {
                run.instances
                    .iter()
                    .position(|node| matches!(node.state, WorkflowState::Pending))
            })
            .or_else(|| (!run.instances.is_empty()).then_some(0));
        let Some(index) = index else {
            break;
        };
        run.instances.remove(index);
        run.omitted_instances = run.omitted_instances.saturating_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(invocation: u64) -> WorkflowRunView {
        let mut id = super::super::test_instance("projection", "node").block.run;
        id.invocation = invocation;
        WorkflowRunView {
            id,
            workflow_id: WorkflowId::parse("projection").unwrap(),
            program_digest: "a".repeat(64),
            resource_revision: crate::runtime::identity::RuntimeResourceRevision::new(1),
            tool_call_id: ToolCallId::new("outer"),
            state: WorkflowState::Running,
            instances: vec![],
            omitted_instances: 0,
            steps_consumed: 0,
            steps_max: 4096,
            agents_consumed: 0,
            candidate_users: 0,
            candidate: None,
            handoff: None,
        }
    }

    #[test]
    fn bounded_retirement_drops_records_not_execution_owners_and_rejects_stale_updates() {
        let owner = WorkflowReadModel::new(run(1).id.conversation_id);
        let first = run(1).id;
        for invocation in 1..=32 {
            let view = run(invocation);
            let id = view.id.clone();
            owner.register(view);
            owner.update(&id, |run| {
                run.state = WorkflowState::Settled {
                    outcome: WorkflowExecutionOutcome::Completed,
                }
            });
        }
        let snapshot = owner.snapshot();
        assert_eq!(snapshot.runs.len(), MAX_PROJECTED_RUNS);
        let mut foreign = run(99);
        foreign.id.conversation_id =
            crate::runtime::identity::ConversationId::new("another-conversation");
        owner.register(foreign);
        assert_eq!(
            owner.snapshot(),
            snapshot,
            "foreign runs cannot enter this conversation view"
        );
        assert_eq!(snapshot.omitted_runs, 24);
        owner.update(&first, |_| panic!("evicted run cannot be mutated"));
        owner.update(&run(32).id, |_| panic!("terminal run cannot be mutated"));
        assert_eq!(snapshot, owner.snapshot());
        assert!(
            WorkflowReadModel::new(run(1).id.conversation_id)
                .snapshot()
                .runs
                .is_empty(),
            "process reopen has no execution recovery input"
        );
    }

    #[test]
    fn stale_run_block_iteration_and_visit_cannot_mutate_new_instance() {
        let owner = WorkflowReadModel::new(run(2).id.conversation_id);
        let mut node = super::super::test_instance("projection", "node");
        node.block.run = run(2).id;
        node.block.invocations = vec![0, 2];
        let mut view = run(2);
        view.instances.push(WorkflowInstanceView::new(
            node.block.clone(),
            Some(node.node.clone()),
            WorkflowNodeKind::Tool,
        ));
        owner.register(view);
        let mut old_run = node.clone();
        old_run.block.run = run(1).id;
        let mut old_block = node.clone();
        old_block
            .block
            .definition
            .blocks
            .push("other-branch".into());
        let mut old_iteration = node.clone();
        old_iteration.block.invocations = vec![0, 1];
        let mut old_visit = node.clone();
        old_visit.visit = 1;
        for stale in [old_run, old_block, old_iteration, old_visit] {
            owner.node(&stale, |_| panic!("stale identity reached current row"));
        }
        assert_eq!(
            owner.snapshot().runs[0].instances[0].state,
            WorkflowState::Pending
        );
        owner.node(&node, |row| row.state = WorkflowState::Running);
        assert_eq!(
            owner.snapshot().runs[0].instances[0].state,
            WorkflowState::Running
        );
    }

    #[test]
    fn count_bytes_and_finished_retention_are_explicit() {
        let owner = WorkflowReadModel::new(run(1).id.conversation_id);
        let view = run(1);
        let id = view.id.clone();
        owner.register(view);
        owner.update(&id, |run| {
            for index in 0..4096 {
                let mut instance = super::super::test_instance("projection", "node").block;
                instance.run = id.clone();
                instance.invocations = vec![0, index];
                let mut node = WorkflowInstanceView::new(
                    instance,
                    Some("node".into()),
                    WorkflowNodeKind::Tool,
                );
                node.state = WorkflowState::Settled {
                    outcome: WorkflowExecutionOutcome::Completed,
                };
                run.instances.push(node);
            }
        });
        let snapshot = owner.snapshot();
        assert_eq!(snapshot.runs[0].instances.len(), MAX_PROJECTED_FINISHED);
        assert_eq!(
            snapshot.runs[0].omitted_instances,
            (4096 - MAX_PROJECTED_FINISHED) as u64
        );
        assert!(serde_json::to_vec(&snapshot.runs[0]).unwrap().len() <= MAX_PROJECTED_RUN_BYTES);
    }

    #[tokio::test]
    async fn snapshot_prepared_before_mutation_keeps_later_revision_observable() {
        let owner = WorkflowReadModel::new(run(1).id.conversation_id);
        owner.register(run(1));
        let mut wake = owner.subscribe();
        let prepared = owner.snapshot();
        let (entered, gate) = tokio::sync::oneshot::channel();
        let (release, released) = tokio::sync::oneshot::channel();
        let native = owner.clone();
        let task = tokio::spawn(async move {
            entered.send(()).unwrap();
            released.await.unwrap();
            native.draining(&run(1).id);
        });
        gate.await.unwrap();
        release.send(()).unwrap();
        task.await.unwrap();
        // The prepared cut is intentionally handed off after the mutation.
        assert_eq!(prepared.runs[0].state, WorkflowState::Running);
        wake.changed().await.unwrap();
        let current = owner.snapshot();
        assert_eq!(current.revision.0, prepared.revision.0 + 1);
        assert_eq!(current.runs[0].state, WorkflowState::Draining);
        owner.draining(&run(1).id);
        assert_eq!(
            current,
            owner.snapshot(),
            "repeated cancellation is idempotent"
        );
        owner.update(&run(1).id, |run| {
            run.state = WorkflowState::Settled {
                outcome: WorkflowExecutionOutcome::Cancelled,
            }
        });
        assert!(matches!(
            owner.snapshot().runs[0].state,
            WorkflowState::Settled {
                outcome: WorkflowExecutionOutcome::Cancelled
            }
        ));
    }

    #[test]
    fn observation_window_loss_preserves_authoritative_current_cut() {
        let owner = WorkflowReadModel::new(run(1).id.conversation_id);
        owner.register(run(1));
        let before = owner.snapshot();
        // No observer consumes any of these committed native transitions.
        for steps in 1..=512 {
            owner.update(&run(1).id, |view| view.steps_consumed = steps);
        }
        let current = owner.snapshot();
        let cuts = owner.cuts_after(before.revision);
        assert!(
            cuts[0].revision.0 > before.revision.0 + 1,
            "the consumer must detect expired revisions"
        );
        assert_eq!(cuts.last(), Some(&current));
        assert_eq!(current.runs[0].steps_consumed, 512);
        assert!(cuts.len() <= MAX_WORKFLOW_CUTS);
        assert!(owner.state.lock().unwrap().bytes <= MAX_WORKFLOW_CUT_BYTES);
        assert_eq!(owner.snapshot(), current, "resync never executes work");
    }

    #[tokio::test]
    async fn node_completion_between_native_read_and_client_cursor_has_no_gap() {
        use crate::runtime_client::projection::{RuntimeClientProjection, SubscriberPoll};
        let owner = WorkflowReadModel::new(run(1).id.conversation_id);
        let node = super::super::test_instance("projection", "node");
        let mut view = run(1);
        view.id = node.block.run.clone();
        let mut row = WorkflowInstanceView::new(
            node.block.clone(),
            Some(node.node.clone()),
            WorkflowNodeKind::Tool,
        );
        row.state = WorkflowState::Running;
        view.instances.push(row);
        owner.register(view);
        let prepared = owner.snapshot();
        let (committed, commit) = tokio::sync::oneshot::channel();
        let native = owner.clone();
        tokio::spawn(async move {
            native.node(&node, |row| {
                row.state = WorkflowState::Settled {
                    outcome: WorkflowExecutionOutcome::Completed,
                }
            });
            committed.send(()).unwrap();
        });
        commit.await.unwrap();
        // Only now hand the old coherent cut to the cursor owner.
        let model = crate::scripted_suites::support::model::scripted_session_model(Arc::new(
            crate::scripted_suites::support::model::NullAdapter,
        ))
        .view();
        let mut client = RuntimeClientProjection::new(
            prepared.runs[0].id.conversation_id.clone(),
            vec![],
            crate::runtime_client::CapabilityView {
                revision: crate::runtime::identity::CapabilityRevision::new(1),
                tools: vec![],
                available_tools: vec![],
                skills: vec![],
                sources: vec![],
            },
            Some(model),
            64,
        );
        client.fold_workflows(prepared.clone());
        let (snapshot, cursor) = client.snapshot().unwrap();
        assert_eq!(snapshot.workflows, prepared);
        let (subscriber, _) = client.subscribe(cursor).unwrap();
        for cut in owner.cuts_after(prepared.revision) {
            client.fold_workflows(cut);
        }
        assert!(matches!(
            client.poll_subscriber(subscriber),
            SubscriberPoll::Event(_)
        ));
        assert!(matches!(
            client.snapshot().unwrap().0.workflows.runs[0].instances[0].state,
            WorkflowState::Settled {
                outcome: WorkflowExecutionOutcome::Completed
            }
        ));
        assert!(client.snapshot().unwrap().0.messages.is_empty());
    }
}
