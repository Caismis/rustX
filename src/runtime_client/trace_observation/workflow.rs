use crate::durable::ConversationStore;
use crate::durable::observation::{WorkflowNodeTraceState, WorkflowTraceNode};
use crate::observation::cut::{CheckpointPageItem, CheckpointReadCut, CollectionFrontier};
use crate::runtime_client::{RuntimeClientCursor, RuntimeClientError, checkpoint::CheckpointPage};

pub(crate) fn workflow_selection(domain: &str) -> Option<u64> {
    domain
        .strip_prefix("trace.records.")?
        .strip_suffix(".workflow.nodes")?
        .parse::<u64>()
        .ok()
        .filter(|sequence| *sequence > 0)
}
pub(crate) fn workflow_page(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    domain: &str,
    offset: u64,
    limit: usize,
    live: Option<&crate::observation::workflow_source::WorkflowReadSource>,
) -> Result<CheckpointPage, RuntimeClientError> {
    let start = workflow_selection(domain).ok_or(RuntimeClientError::ReadCutExpired)?;
    if limit == 0 || limit > 32 || offset > 9_007_199_254_740_991 {
        return Err(RuntimeClientError::ReadCutExpired);
    }
    let native = validate_source_cut(cut)?;
    let page = store
        .observation_workflow_nodes(native, start, offset, limit)
        .map_err(RuntimeClientError::observation_store_error)?;
    let items = page
        .nodes
        .into_iter()
        .enumerate()
        .map(|(index, mut node)| {
            if let Some(live) = live {
                repair_live(&mut node, live);
            }
            Ok(CheckpointPageItem {
                offset: offset + index as u64,
                identity: node.id.clone(),
                value: Some(super::inspection::value(&node)?),
                deferred: false,
                content: Vec::new(),
                body_complete: Some(true),
                key: None,
            })
        })
        .collect::<Result<Vec<_>, RuntimeClientError>>()?;
    Ok(CheckpointPage {
        domain: domain.into(),
        cursor: RuntimeClientCursor::new(cut.revision),
        read_cut: cut.clone(),
        total: page.total,
        collections: vec![CollectionFrontier {
            domain: domain.into(),
            revision: page.revision,
            total: page.total,
            loaded: items.len() as u64,
            next_offset: page.next_offset,
        }],
        items,
        next_offset: page.next_offset,
    })
}

/// A live label requires the complete run/block/visit identity in a current
/// native owner. Retained finished runs and nodes are never positive evidence.
fn repair_live(
    node: &mut WorkflowTraceNode,
    live: &crate::observation::workflow_source::WorkflowReadSource,
) {
    use crate::runtime::workflow::read_model::WorkflowState as W;
    if node.state != WorkflowNodeTraceState::Incomplete {
        return;
    }
    let Some(current) = live.active_node_state(&node.instance) else {
        return;
    };
    node.state = match *current {
        W::Pending => WorkflowNodeTraceState::Pending,
        W::Running => WorkflowNodeTraceState::Running,
        W::Waiting { reason } => WorkflowNodeTraceState::Waiting { reason },
        W::Draining => WorkflowNodeTraceState::Draining,
        W::Settled { .. } => return,
    };
}

fn validate_source_cut(
    cut: &CheckpointReadCut,
) -> Result<&crate::durable::observation::ObservationReadCut, RuntimeClientError> {
    cut.source_cut
        .as_ref()
        .filter(|source| {
            source.read.conversation_id == cut.conversation_id && source.read.journal == cut.through
        })
        .ok_or(RuntimeClientError::ReadCutExpired)
}

pub(crate) fn workflow_node(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    instance: &crate::runtime::workflow::WorkflowNodeInstance,
    live: Option<&crate::observation::workflow_source::WorkflowReadSource>,
) -> Result<Option<WorkflowTraceNode>, RuntimeClientError> {
    if instance.block.run.conversation_id != cut.conversation_id {
        return Err(RuntimeClientError::InvalidRequest {
            message: "workflow node belongs to another conversation".into(),
        });
    }
    let native = validate_source_cut(cut)?;
    let mut node = store
        .observation_workflow_node(native, instance)
        .map_err(RuntimeClientError::observation_store_error)?;
    if let (Some(node), Some(live)) = (&mut node, live) {
        repair_live(node, live);
    }
    Ok(node)
}

pub(crate) fn history_workflow_node(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    instance: &crate::runtime::workflow::WorkflowNodeInstance,
) -> Result<Option<WorkflowTraceNode>, RuntimeClientError> {
    super::validate_history_cut(store, cut)?;
    workflow_node(store, cut, instance, None)
}

fn validate_snapshot_source_cut<'a>(
    cut: &'a CheckpointReadCut,
    run_id: &crate::runtime::workflow::WorkflowRunId,
) -> Result<&'a crate::durable::observation::ObservationReadCut, RuntimeClientError> {
    if run_id.conversation_id != cut.conversation_id {
        return Err(RuntimeClientError::InvalidRequest {
            message: "workflow run belongs to another conversation".into(),
        });
    }
    validate_source_cut(cut)
}

pub(crate) fn history_workflow_run_snapshot_header(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    run_id: &crate::runtime::workflow::WorkflowRunId,
) -> Result<Option<crate::runtime::workflow::WorkflowRunSnapshotHeader>, RuntimeClientError> {
    let native = validate_snapshot_source_cut(cut, run_id)?;
    store
        .observation_workflow_run_snapshot_header(native, run_id)
        .map_err(RuntimeClientError::observation_store_error)
}

pub(crate) fn history_workflow_run_snapshot_document(
    store: &dyn ConversationStore,
    cut: &CheckpointReadCut,
    run_id: &crate::runtime::workflow::WorkflowRunId,
) -> Result<
    Option<(
        crate::runtime::workflow::WorkflowRunSnapshotHeader,
        crate::workflow_file::WorkflowFileDocument,
    )>,
    RuntimeClientError,
> {
    let native = validate_snapshot_source_cut(cut, run_id)?;
    store
        .observation_workflow_run_snapshot_document(native, run_id)
        .map_err(RuntimeClientError::observation_store_error)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::workflow::read_model::{WorkflowSnapshot, WorkflowState, WorkflowWait};

    fn repair(node: &mut WorkflowTraceNode, live: &WorkflowSnapshot) {
        repair_live(
            node,
            &crate::observation::workflow_source::WorkflowReadSource::new(std::sync::Arc::new(
                live.clone(),
            )),
        );
    }

    fn fixture() -> (WorkflowTraceNode, WorkflowSnapshot) {
        let instance = crate::runtime::workflow::test_instance("trace_live", "review");
        let node = WorkflowTraceNode {
            id: "workflow-node:2".into(),
            instance: instance.clone(),
            kind: None,
            state: WorkflowNodeTraceState::Incomplete,
            timing: crate::observation::trace::TraceTiming {
                started_at: chrono::DateTime::UNIX_EPOCH,
                ended_at: None,
                duration_ms: None,
            },
            input: None,
            output: None,
            input_preview: None,
            output_preview: None,
            input_omitted: false,
            output_omitted: false,
            child: None,
            child_agent_id: None,
        };
        let run = serde_json::from_value(serde_json::json!({
            "id": instance.block.run,
            "workflow_id": "trace_live",
            "program_digest": "digest",
            "resource_revision": 1,
            "state": { "type": "running" },
            "instances": [{
                "block": instance.block, "node": instance.node, "visit": instance.visit,
                "kind": "review", "state": { "type": "waiting", "reason": "review" }
            }],
            "omitted_instances": 0, "steps_consumed": 1, "steps_max": 10,
            "agents_consumed": 0, "model_calls_consumed": 0
        }))
        .unwrap();
        (
            node,
            WorkflowSnapshot {
                runs: vec![run],
                ..WorkflowSnapshot::default()
            },
        )
    }

    #[test]
    fn workflow_live_labels_require_every_native_identity_component() {
        let (node, live) = fixture();
        let mut exact = node.clone();
        repair(&mut exact, &live);
        assert_eq!(
            exact.state,
            WorkflowNodeTraceState::Waiting {
                reason: WorkflowWait::Review
            }
        );
        assert_eq!(exact.timing, node.timing, "live labels never invent timing");
        let mut variants = vec![node.clone(); 7];
        variants[0].instance.block.run.conversation_id =
            crate::runtime::identity::ConversationId::generate();
        variants[1].instance.block.run.invocation += 1;
        variants[2].instance.block.run.origin = crate::runtime::workflow::WorkflowRunOrigin::Tool {
            attempt_id: crate::runtime::identity::AttemptId::new("other-attempt"),
        };
        variants[3]
            .instance
            .block
            .definition
            .blocks
            .push("other-block".into());
        variants[4].instance.block.invocations.push(1);
        variants[5].instance.node = "other-node".into();
        variants[6].instance.visit += 1;
        for mut stale in variants {
            repair(&mut stale, &live);
            assert_eq!(stale.state, WorkflowNodeTraceState::Incomplete);
        }
        let mut other_definition = node;
        other_definition.instance.block.definition.workflow_id =
            crate::runtime::workflow::WorkflowId::parse("other-definition").unwrap();
        repair(&mut other_definition, &live);
        assert_eq!(other_definition.state, WorkflowNodeTraceState::Incomplete);
    }

    #[test]
    fn workflow_live_labels_never_promote_retained_terminals_or_rewrite_durable_outcomes() {
        use crate::runtime::workflow::WorkflowExecutionOutcome as Outcome;
        let (node, mut live) = fixture();
        let terminal = WorkflowNodeTraceState::Settled {
            outcome: Outcome::Completed,
        };
        let mut completed = node.clone();
        completed.state = terminal.clone();
        repair(&mut completed, &live);
        assert_eq!(completed.state, terminal, "durable terminal always wins");
        live.runs[0].state = WorkflowState::Settled {
            outcome: Outcome::Completed,
        };
        let mut retained = node.clone();
        repair(&mut retained, &live);
        assert_eq!(retained.state, WorkflowNodeTraceState::Incomplete);
        live.runs[0].state = WorkflowState::Running;
        live.runs[0].instances[0].state = WorkflowState::Settled {
            outcome: Outcome::Completed,
        };
        let mut retained = node;
        repair(&mut retained, &live);
        assert_eq!(retained.state, WorkflowNodeTraceState::Incomplete);
    }
}
