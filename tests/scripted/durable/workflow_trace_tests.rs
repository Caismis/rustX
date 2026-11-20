//! Workflow-trace store suites exercising the runtime-client evidence layer.
mod tests {
    use crate::durable::observation::{WorkflowNodeTraceState, trace::TraceFieldSource};

    use crate::durable::sqlite::test_support::{envelope, store};
    use crate::durable::{CanonicalStore, ConversationStore, JournalStore, ObservationStore};
    use rustx_types::events::types::RuntimeEvent as E;
    use rustx_types::runtime::workflow::{WorkflowExecutionOutcome as Outcome, WorkflowId};

    fn append_workflow_event(
        store: &crate::durable::SqliteConversationStore,
        event_id: &str,
        event: rustx_types::events::types::RuntimeEvent,
    ) -> u64 {
        let envelope = envelope(store.conversation_id(), event_id, None, event);
        match &envelope.event {
            rustx_types::events::types::RuntimeEvent::WorkflowStarted { workflow_id, .. } => {
                let snapshot =
                    crate::durable::workflow::snapshot::test_run_snapshot(workflow_id.clone());
                store
                    .publish_workflow_start(envelope, &snapshot)
                    .unwrap()
                    .sequence
            }
            _ => store.append_event(envelope).unwrap().sequence,
        }
    }

    #[test]
    fn workflow_nodes_keep_real_visits_original_cuts_and_bounded_bodies() {
        let store = store();
        store.initialize(&[]).unwrap();
        let mut first = rustx_types::runtime::workflow::test_instance("trace_flow", "transform");
        first.block.run.conversation_id = store.conversation_id().clone();
        let run = first.block.run.clone();
        let workflow_id = WorkflowId::parse("trace_flow").unwrap();
        let mut ordinal = 0;
        let mut append = |event| {
            ordinal += 1;
            append_workflow_event(&store, &format!("wf-trace-{ordinal}"), event)
        };
        let start = append(E::WorkflowStarted {
            tool_call_id: None,
            workflow_id: workflow_id.clone(),
            run_id: run.clone(),
        });
        append(E::WorkflowNodeStarted {
            instance: first.clone(),
        });
        let before = store.observation_read_cut().unwrap();
        let input = r#"{"args":{"n":18446744073709551615},"variables":{}}"#.to_owned();
        append(E::WorkflowNodeInputRecorded {
            instance: first.clone(),
            kind: rustx_types::runtime::workflow::read_model::WorkflowNodeKind::Template,
            value: Some(input.clone()),
        });
        append(E::WorkflowNodeOutputRecorded {
            instance: first.clone(),
            value: Some(r#"{"value":"rendered"}"#.into()),
        });
        append(E::WorkflowNodeSettled {
            instance: first.clone(),
            outcome: Outcome::Completed,
        });
        let mut second = first.clone();
        second.block.invocations.push(2);
        append(E::WorkflowNodeStarted {
            instance: second.clone(),
        });
        append(E::WorkflowNodeInputRecorded {
            instance: second.clone(),
            kind: rustx_types::runtime::workflow::read_model::WorkflowNodeKind::Template,
            value: None,
        });
        append(E::WorkflowNodeSettled {
            instance: second.clone(),
            outcome: Outcome::Cancelled,
        });
        append(E::WorkflowCancelled {
            workflow_id,
            run_id: run,
            reason: rustx_types::runtime::types::CancellationReason::UserRequested,
        });
        let cut = store.observation_read_cut().unwrap();
        let old = store
            .observation_workflow_nodes(&before, start, 0, 32)
            .unwrap();
        assert_eq!(old.total, 1);
        assert_eq!(old.nodes[0].state, WorkflowNodeTraceState::Incomplete);
        assert!(old.nodes[0].input.is_none());
        let first_page = store.observation_workflow_nodes(&cut, start, 0, 1).unwrap();
        assert_eq!(first_page.total, 2);
        assert_eq!(first_page.next_offset, Some(1));
        assert_eq!(first_page.nodes[0].instance, first);
        assert_eq!(
            first_page.nodes[0].state,
            WorkflowNodeTraceState::Settled {
                outcome: Outcome::Completed
            }
        );
        let Some(TraceFieldSource::Value { encoded_bytes, .. }) = &first_page.nodes[0].input else {
            panic!("original input selector")
        };
        assert_eq!(
            *encoded_bytes,
            serde_json::to_vec(&input).unwrap().len() as u64
        );
        let last = store.observation_workflow_nodes(&cut, start, 1, 1).unwrap();
        assert_eq!(last.nodes[0].instance, second);
        assert!(last.nodes[0].input_omitted);
        assert!(last.nodes[0].output.is_none());
        assert_eq!(last.next_offset, None);
        assert!(
            store
                .observation_workflow_nodes(&cut, start, 0, 33)
                .is_err()
        );
        assert!(
            store
                .observation_workflow_nodes(&cut, start + 1, 0, 1)
                .is_err()
        );
    }

    #[test]
    fn reopened_workflow_nodes_require_terminals_and_preserve_original_cuts() {
        use crate::durable::SqliteConversationStore;
        use rustx_types::runtime::workflow::read_model::WorkflowWait;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workflow-trace.sqlite");
        let conversation = rustx_types::runtime::identity::ConversationId::generate();
        let owner = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        owner.initialize(&[]).unwrap();
        let mut started = rustx_types::runtime::workflow::test_instance("reopened", "start-only");
        started.block.run.conversation_id = conversation.clone();
        let mut waiting = started.clone();
        waiting.node = "wait-only".into();
        let mut completed = started.clone();
        completed.node = "completed".into();
        let mut ordinal = 0;
        let mut append = |event| {
            ordinal += 1;
            append_workflow_event(&owner, &format!("reopen-node-{ordinal}"), event)
        };
        let start = append(E::WorkflowStarted {
            tool_call_id: None,
            workflow_id: WorkflowId::parse("reopened").unwrap(),
            run_id: started.block.run.clone(),
        });
        for instance in [&started, &waiting, &completed] {
            append(E::WorkflowNodeStarted {
                instance: instance.clone(),
            });
        }
        append(E::WorkflowNodeWaiting {
            instance: waiting,
            reason: WorkflowWait::Review,
            auxiliary_call_id: None,
        });
        let before_terminal = owner.observation_read_cut().unwrap();
        append(E::WorkflowNodeSettled {
            instance: completed.clone(),
            outcome: Outcome::Completed,
        });
        let after_terminal = owner.observation_read_cut().unwrap();
        let finished = owner
            .observation_workflow_nodes(&after_terminal, start, 0, 32)
            .unwrap();
        assert_eq!(
            finished.nodes[2].state,
            WorkflowNodeTraceState::Settled {
                outcome: Outcome::Completed
            }
        );
        assert!(finished.nodes[2].timing.ended_at.is_some());
        drop(owner);

        // A reopened durable store has no surviving executor or recovery owner.
        let reopened = SqliteConversationStore::open_existing(conversation, &path).unwrap();
        let old = reopened
            .observation_workflow_nodes(&before_terminal, start, 0, 32)
            .unwrap();
        assert_eq!(old.nodes.len(), 3);
        for node in &old.nodes {
            assert_eq!(node.state, WorkflowNodeTraceState::Incomplete);
            assert!(node.timing.ended_at.is_none());
            assert!(node.timing.duration_ms.is_none());
        }
        let current = reopened
            .observation_workflow_nodes(&after_terminal, start, 0, 32)
            .unwrap();
        assert_eq!(current.nodes, finished.nodes);
        assert_eq!(current.nodes[0].state, WorkflowNodeTraceState::Incomplete);
        assert_eq!(current.nodes[1].state, WorkflowNodeTraceState::Incomplete);
    }

    #[test]
    #[allow(clippy::too_many_lines)] // One cut/reopen scenario keeps the exact child identity witnesses together.
    fn agent_node_child_references_follow_exact_visit_and_cut() {
        use crate::durable::SqliteConversationStore;
        use rustx_types::runtime::identity::{AgentId, SubagentId};
        use rustx_types::runtime::subagent::{AgentActivationOrigin, SubagentName};
        use rustx_types::runtime::workflow::read_model::WorkflowNodeKind;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workflow-agent-trace.sqlite");
        let conversation = rustx_types::runtime::identity::ConversationId::generate();
        let owner = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        owner.initialize(&[]).unwrap();
        let workflow_id = WorkflowId::parse("agent_visits").unwrap();
        let mut first = rustx_types::runtime::workflow::test_instance("agent_visits", "worker");
        first.block.run.conversation_id = conversation.clone();
        let mut second = first.clone();
        second.visit = 2;
        let mut non_agent = first.clone();
        non_agent.node = "template".into();

        let mut ordinal = 0;
        let mut append = |event| {
            ordinal += 1;
            let id = match &event {
                E::SubagentOwnershipCommitted { subagent_id, .. } => {
                    crate::durable::subagent_facts::subagent_ownership_event_id(subagent_id)
                        .to_string()
                }
                _ => format!("agent-trace-{ordinal}"),
            };
            append_workflow_event(&owner, &id, event)
        };
        let start = append(E::WorkflowStarted {
            tool_call_id: None,
            workflow_id: workflow_id.clone(),
            run_id: first.block.run.clone(),
        });
        append(E::WorkflowNodeStarted {
            instance: first.clone(),
        });
        append(E::WorkflowNodeInputRecorded {
            instance: first.clone(),
            kind: WorkflowNodeKind::Agent,
            value: Some("{}".into()),
        });
        let before_admission = owner.observation_read_cut().unwrap();

        let subagent_1 = SubagentId::for_conversation(&conversation, 1);
        let child_agent_1 = AgentId::new("child-worker-1");
        append(E::SubagentOwnershipCommitted {
            parent_agent_id: AgentId::new("parent"),
            admitted_authority: None,
            subagent_id: subagent_1.clone(),
            child_agent_id: child_agent_1.clone(),
            child_conversation_id: crate::durable::sqlite::test_support::child_conversation_id(
                subagent_1.as_str(),
            ),
            origin: AgentActivationOrigin::Workflow {
                node_id: Box::new(first.clone()),
            },
            agent: "worker".into(),
            definition_digest: "sha256:def".into(),
            profile_digest: "sha256:prof".into(),
            ownership: rustx_types::events::types::SubagentOwnershipKind::Workflow,
            workspace: directory.path().to_path_buf(),
        });
        let after_ownership = owner.observation_read_cut().unwrap();

        append(E::WorkflowAgentAdmitted {
            workflow_id: workflow_id.clone(),
            run_id: first.block.run.clone(),
            node_id: first.clone(),
            subagent_id: subagent_1.clone(),
            profile: SubagentName::parse("worker").unwrap(),
        });
        append(E::WorkflowNodeSettled {
            instance: first.clone(),
            outcome: Outcome::Completed,
        });

        // Second visit to the same named node gets its own distinct child reference.
        append(E::WorkflowNodeStarted {
            instance: second.clone(),
        });
        append(E::WorkflowNodeInputRecorded {
            instance: second.clone(),
            kind: WorkflowNodeKind::Agent,
            value: Some("{}".into()),
        });
        let subagent_2 = SubagentId::for_conversation(&conversation, 2);
        let child_agent_2 = AgentId::new("child-worker-2");
        append(E::SubagentOwnershipCommitted {
            parent_agent_id: AgentId::new("parent"),
            admitted_authority: None,
            subagent_id: subagent_2.clone(),
            child_agent_id: child_agent_2.clone(),
            child_conversation_id: crate::durable::sqlite::test_support::child_conversation_id(
                subagent_2.as_str(),
            ),
            origin: AgentActivationOrigin::Workflow {
                node_id: Box::new(second.clone()),
            },
            agent: "worker".into(),
            definition_digest: "sha256:def".into(),
            profile_digest: "sha256:prof".into(),
            ownership: rustx_types::events::types::SubagentOwnershipKind::Workflow,
            workspace: directory.path().to_path_buf(),
        });
        append(E::WorkflowAgentAdmitted {
            workflow_id,
            run_id: second.block.run.clone(),
            node_id: second.clone(),
            subagent_id: subagent_2.clone(),
            profile: SubagentName::parse("worker").unwrap(),
        });
        append(E::WorkflowNodeSettled {
            instance: second.clone(),
            outcome: Outcome::Completed,
        });
        append(E::WorkflowNodeStarted {
            instance: non_agent.clone(),
        });
        append(E::WorkflowNodeInputRecorded {
            instance: non_agent.clone(),
            kind: WorkflowNodeKind::Template,
            value: Some("{}".into()),
        });
        let after_all = owner.observation_read_cut().unwrap();
        drop(owner);

        let reopened = SqliteConversationStore::open_existing(conversation, &path).unwrap();
        let before = reopened
            .observation_workflow_nodes(&before_admission, start, 0, 32)
            .unwrap();
        assert_eq!(before.nodes.len(), 1);
        assert_eq!(before.nodes[0].child, None);
        assert_eq!(before.nodes[0].child_agent_id, None);

        let mid = reopened
            .observation_workflow_nodes(&after_ownership, start, 0, 32)
            .unwrap();
        assert_eq!(mid.nodes[0].child.as_ref(), Some(&subagent_1));
        assert_eq!(mid.nodes[0].child_agent_id.as_ref(), Some(&child_agent_1));

        let final_nodes = reopened
            .observation_workflow_nodes(&after_all, start, 0, 32)
            .unwrap();
        assert_eq!(final_nodes.nodes.len(), 3);
        assert_eq!(final_nodes.nodes[0].instance, first);
        assert_eq!(final_nodes.nodes[0].child.as_ref(), Some(&subagent_1));
        assert_eq!(
            final_nodes.nodes[0].child_agent_id.as_ref(),
            Some(&child_agent_1)
        );
        assert_eq!(final_nodes.nodes[1].instance, second);
        assert_eq!(final_nodes.nodes[1].child.as_ref(), Some(&subagent_2));
        assert_eq!(
            final_nodes.nodes[1].child_agent_id.as_ref(),
            Some(&child_agent_2)
        );
        assert_eq!(final_nodes.nodes[2].instance, non_agent);
        assert_eq!(final_nodes.nodes[2].child, None);
        assert_eq!(final_nodes.nodes[2].child_agent_id, None);
    }
}

mod workflow_tests {
    use crate::durable::observation::trace::TraceCursor;
    use crate::durable::sqlite::test_support::envelope;
    use crate::durable::{CanonicalStore, JournalStore, ObservationStore, SqliteConversationStore};
    use crate::runtime_client::trace::TraceProjection;
    use rustx_types::events::types::RuntimeEvent;
    use rustx_types::runtime::identity::{AttemptId, ConversationId};
    use rustx_types::runtime::workflow::scheduled::lifecycle::ScheduledWorkflowStartId;
    use rustx_types::runtime::workflow::{WorkflowId, WorkflowRunId, WorkflowRunOrigin};

    #[test]
    fn indexed_workflow_location_preserves_old_long_ids_origins_cuts_and_reopen_without_writes() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("workflow-locate.sqlite");
        let conversation = ConversationId::generate();
        let store = SqliteConversationStore::open(conversation.clone(), &path).unwrap();
        store.initialize(&[]).unwrap();
        let before = store.observation_read_cut().unwrap();
        let mut run = WorkflowRunId {
            conversation_id: conversation.clone(),
            origin: WorkflowRunOrigin::Tool {
                attempt_id: AttemptId::new("\"".repeat(256)),
            },
            invocation: 1,
        };
        let append = |run: &WorkflowRunId, ordinal| {
            let workflow_id = WorkflowId::parse("located").unwrap();
            store
                .publish_workflow_start(
                    envelope(
                        &conversation,
                        &format!("locate-run-{ordinal}"),
                        None,
                        RuntimeEvent::WorkflowStarted {
                            tool_call_id: None,
                            workflow_id: workflow_id.clone(),
                            run_id: run.clone(),
                        },
                    ),
                    &crate::durable::workflow::snapshot::test_run_snapshot(workflow_id),
                )
                .unwrap()
                .sequence
        };
        let tool_sequence = append(&run, 1);
        let tool = run.clone();
        run.origin = WorkflowRunOrigin::Scheduled {
            start_id: ScheduledWorkflowStartId::try_from(format!("swf_{}", "a".repeat(64)))
                .unwrap(),
        };
        let scheduled_sequence = append(&run, 2);
        let scheduled = run.clone();
        for ordinal in 3..=100 {
            run.invocation = ordinal;
            append(&run, ordinal);
        }
        assert!(serde_json::to_vec(&tool).unwrap().len() > 512);
        let current = store.observation_read_cut().unwrap();
        let canonical = store.load_canonical().unwrap();
        let journal = store.read_events(None, 256).unwrap();
        assert!(
            TraceProjection::at_cut(&store, &before)
                .unwrap()
                .locate_workflow(&tool)
                .unwrap()
                .is_none()
        );
        let projection = TraceProjection::at_cut(&store, &current).unwrap();
        for (id, sequence) in [(&tool, tool_sequence), (&scheduled, scheduled_sequence)] {
            let location = projection.locate_workflow(id).unwrap().unwrap();
            assert_eq!(location.record_id, TraceCursor::at(sequence).0);
            let root = location
                .page
                .records
                .iter()
                .find(|record| record.id == location.record_id)
                .unwrap();
            assert_eq!(
                root.kind,
                crate::durable::observation::trace::TraceKind::Workflow
            );
            if id == &tool {
                assert!(root.native_id.is_none());
            }
            assert!(
                root.collections.iter().any(|frontier| frontier.domain
                    == format!("trace.records.{sequence}.workflow.nodes"))
            );
            assert!(projection.locate_workflow_at(id, &before).is_err());
        }
        let mut foreign = tool.clone();
        foreign.conversation_id = ConversationId::generate();
        assert!(projection.locate_workflow(&foreign).is_err());
        assert_eq!(store.load_canonical().unwrap(), canonical);
        assert_eq!(store.read_events(None, 256).unwrap().events, journal.events);
        drop(projection);
        drop(store);
        let reopened = SqliteConversationStore::open_existing(conversation, &path).unwrap();
        assert_eq!(
            TraceProjection::new(&reopened)
                .unwrap()
                .locate_workflow(&tool)
                .unwrap()
                .unwrap()
                .record_id,
            TraceCursor::at(tool_sequence).0
        );
        assert_eq!(reopened.load_canonical().unwrap(), canonical);
        assert_eq!(
            reopened.read_events(None, 256).unwrap().events,
            journal.events
        );
    }

    #[test]
    fn workflow_started_only_exposes_exact_typed_run_and_workflow_identity_without_nodes() {
        let directory = tempfile::tempdir().unwrap();
        let conversation = ConversationId::generate();
        let owner = SqliteConversationStore::open(
            conversation.clone(),
            &directory.path().join("start-only.sqlite"),
        )
        .unwrap();
        owner.initialize(&[]).unwrap();
        let mut run = rustx_types::runtime::workflow::test_instance("start_only_flow", "start")
            .block
            .run;
        run.conversation_id = conversation.clone();
        run.origin = WorkflowRunOrigin::Tool {
            attempt_id: AttemptId::new("\"".repeat(256)),
        };
        run.invocation = 9;
        let workflow_id = WorkflowId::parse("start_only_flow").unwrap();
        let start = owner
            .publish_workflow_start(
                envelope(
                    &conversation,
                    "start-only-1",
                    None,
                    RuntimeEvent::WorkflowStarted {
                        tool_call_id: None,
                        workflow_id: workflow_id.clone(),
                        run_id: run.clone(),
                    },
                ),
                &crate::durable::workflow::snapshot::test_run_snapshot(workflow_id),
            )
            .unwrap()
            .sequence;
        let cut = owner.observation_read_cut().unwrap();
        let nodes = owner
            .observation_workflow_nodes(&cut, start, 0, 32)
            .unwrap();
        assert_eq!(nodes.run_id, run);
        assert_eq!(
            nodes.workflow_id,
            WorkflowId::parse("start_only_flow").unwrap()
        );
        assert_eq!(nodes.total, 0);
        assert_eq!(
            nodes.nodes,
            [] as [crate::durable::observation::WorkflowTraceNode; 0]
        );
        assert!(nodes.next_offset.is_none());
    }

    #[test]
    fn exact_visit_lookup_preserves_cut_omitted_and_cross_conversation_fences() {
        use crate::durable::observation::WorkflowNodeTraceState;
        use rustx_types::runtime::workflow::WorkflowExecutionOutcome as Outcome;
        let directory = tempfile::tempdir().unwrap();
        let conversation = ConversationId::new("conv_19000000-0000-7000-8000-000000000777");
        let owner = SqliteConversationStore::open(
            conversation.clone(),
            &directory.path().join("exact-visit.sqlite"),
        )
        .unwrap();
        owner.initialize(&[]).unwrap();
        let mut instance = rustx_types::runtime::workflow::test_instance("exact_visit", "worker");
        instance.block.run.conversation_id = conversation.clone();
        let mut ordinal = 0;
        let mut append = |event| {
            ordinal += 1;
            let envelope = envelope(
                &conversation,
                &format!("exact-visit-{ordinal}"),
                None,
                event,
            );
            match &envelope.event {
                RuntimeEvent::WorkflowStarted { workflow_id, .. } => {
                    let snapshot =
                        crate::durable::workflow::snapshot::test_run_snapshot(workflow_id.clone());
                    owner
                        .publish_workflow_start(envelope, &snapshot)
                        .unwrap()
                        .sequence
                }
                _ => owner.append_event(envelope).unwrap().sequence,
            }
        };
        append(RuntimeEvent::WorkflowStarted {
            tool_call_id: None,
            workflow_id: WorkflowId::parse("exact_visit").unwrap(),
            run_id: instance.block.run.clone(),
        });
        let before_node = owner.observation_read_cut().unwrap();
        append(RuntimeEvent::WorkflowNodeStarted {
            instance: instance.clone(),
        });
        let after_start = owner.observation_read_cut().unwrap();
        append(RuntimeEvent::WorkflowNodeInputRecorded {
            instance: instance.clone(),
            kind: rustx_types::runtime::workflow::read_model::WorkflowNodeKind::Template,
            value: Some(r#"{"a":1}"#.into()),
        });
        append(RuntimeEvent::WorkflowNodeOutputRecorded {
            instance: instance.clone(),
            value: None,
        });
        let after_io = owner.observation_read_cut().unwrap();
        append(RuntimeEvent::WorkflowNodeSettled {
            instance: instance.clone(),
            outcome: Outcome::Completed,
        });
        let settled_cut = owner.observation_read_cut().unwrap();

        assert!(
            owner
                .observation_workflow_node(&before_node, &instance)
                .unwrap()
                .is_none()
        );
        let at_start = owner
            .observation_workflow_node(&after_start, &instance)
            .unwrap()
            .expect("node started at cut");
        assert_eq!(at_start.state, WorkflowNodeTraceState::Incomplete);
        assert!(at_start.input.is_none() && !at_start.input_omitted);
        assert!(at_start.output.is_none() && !at_start.output_omitted);

        let with_io = owner
            .observation_workflow_node(&after_io, &instance)
            .unwrap()
            .expect("node I/O at cut");
        assert_eq!(with_io.state, WorkflowNodeTraceState::Incomplete);
        assert!(with_io.input.is_some() && !with_io.input_omitted);
        assert!(with_io.output.is_none() && with_io.output_omitted);

        let finished = owner
            .observation_workflow_node(&settled_cut, &instance)
            .unwrap()
            .expect("settled node at cut");
        assert_eq!(
            finished.state,
            WorkflowNodeTraceState::Settled {
                outcome: Outcome::Completed
            }
        );

        let mut foreign = instance.clone();
        foreign.block.run.conversation_id = ConversationId::generate();
        assert!(matches!(
            owner.observation_workflow_node(&settled_cut, &foreign),
            Err(crate::durable::ConversationStoreError::InvalidReference(_))
        ));
        let mut too_long = instance;
        too_long.node = "n".repeat(crate::durable::sqlite::workflow_trace::INSTANCE_JSON_MAX + 1);
        assert!(matches!(
            owner.observation_workflow_node(&settled_cut, &too_long),
            Err(crate::durable::ConversationStoreError::InvalidReference(_))
        ));
    }

    #[test]
    fn admitted_depth_eight_visit_matches_between_paged_and_exact_lookup_at_same_cut() {
        use crate::durable::ConversationStore;
        use crate::durable::sqlite::test_support::store;
        use crate::runtime::workflow::WorkflowProgram;
        use rustx_types::events::types::RuntimeEvent as E;
        use rustx_types::runtime::workflow::{
            MAX_BLOCK_DEPTH, WorkflowBlockInstance, WorkflowDefinitionPath,
            WorkflowExecutionOutcome as Outcome, WorkflowNodeInstance,
        };
        let fixtures: serde_json::Value =
            serde_json::from_str(include_str!("../../fixtures/workflow-run-bounds.json")).unwrap();
        let document: rustx_workflow::workflow_file::WorkflowFileDocument =
            serde_json::from_value(fixtures["maximum"].clone()).unwrap();
        let workflow_id = WorkflowId::parse("maximum").unwrap();
        let program =
            WorkflowProgram::compile_document(workflow_id.clone(), "sha256:max-depth", document)
                .unwrap();

        let store = store();
        store.initialize(&[]).unwrap();
        let mut run = rustx_types::runtime::workflow::test_instance("maximum", "start_d8")
            .block
            .run;
        run.conversation_id = store.conversation_id().clone();
        let start = store
            .publish_workflow_start(
                envelope(
                    store.conversation_id(),
                    "max-depth-start",
                    None,
                    E::WorkflowStarted {
                        tool_call_id: None,
                        workflow_id: workflow_id.clone(),
                        run_id: run.clone(),
                    },
                ),
                program.run_snapshot(),
            )
            .unwrap()
            .sequence;

        let depth_eight_blocks: Vec<String> = (0..MAX_BLOCK_DEPTH)
            .flat_map(|depth| [format!("loop_d{depth}"), "body".to_owned()])
            .collect();
        assert_eq!(depth_eight_blocks.len(), 16);
        let depth_eight_invocations: Vec<u32> = std::iter::once(0)
            .chain(std::iter::repeat_n(1, MAX_BLOCK_DEPTH))
            .collect();
        assert_eq!(depth_eight_invocations.len(), 9);
        let deepest = WorkflowNodeInstance {
            block: WorkflowBlockInstance {
                run,
                definition: WorkflowDefinitionPath {
                    workflow_id,
                    blocks: depth_eight_blocks,
                },
                invocations: depth_eight_invocations,
            },
            node: "start_d8".into(),
            visit: 0,
        };
        store
            .append_event(envelope(
                store.conversation_id(),
                "max-depth-node-start",
                None,
                E::WorkflowNodeStarted {
                    instance: deepest.clone(),
                },
            ))
            .unwrap();
        store
            .append_event(envelope(
                store.conversation_id(),
                "max-depth-node-settled",
                None,
                E::WorkflowNodeSettled {
                    instance: deepest.clone(),
                    outcome: Outcome::Completed,
                },
            ))
            .unwrap();

        let cut = store.observation_read_cut().unwrap();
        let paged = store
            .observation_workflow_nodes(&cut, start, 0, 32)
            .unwrap();
        assert_eq!(paged.nodes.len(), 1);
        assert_eq!(paged.nodes[0].instance, deepest);
        let exact = store
            .observation_workflow_node(&cut, &deepest)
            .unwrap()
            .expect("depth-8 start_d8 must resolve at exact lookup");
        assert_eq!(exact, paged.nodes[0]);
        assert_invalid_depth_eight_variants(&store, &cut, &deepest);
    }

    fn assert_invalid_depth_eight_variants(
        store: &crate::durable::sqlite::SqliteConversationStore,
        cut: &crate::durable::observation::ObservationReadCut,
        deepest: &rustx_types::runtime::workflow::WorkflowNodeInstance,
    ) {
        let mut over_depth = deepest.clone();
        over_depth
            .block
            .definition
            .blocks
            .extend(["loop_d8".to_owned(), "body".to_owned()]);
        over_depth.block.invocations.push(1);
        assert!(matches!(
            store.observation_workflow_node(cut, &over_depth),
            Err(crate::durable::ConversationStoreError::InvalidReference(_))
        ));

        let mut over_invocations = deepest.clone();
        over_invocations.block.invocations.push(1);
        assert!(matches!(
            store.observation_workflow_node(cut, &over_invocations),
            Err(crate::durable::ConversationStoreError::InvalidReference(_))
        ));

        let mut under_invocations = deepest.clone();
        under_invocations.block.invocations.pop();
        assert!(matches!(
            store.observation_workflow_node(cut, &under_invocations),
            Err(crate::durable::ConversationStoreError::InvalidReference(_))
        ));

        let mut odd_blocks = deepest.clone();
        odd_blocks.block.definition.blocks.pop();
        assert!(matches!(
            store.observation_workflow_node(cut, &odd_blocks),
            Err(crate::durable::ConversationStoreError::InvalidReference(_))
        ));
    }
}
