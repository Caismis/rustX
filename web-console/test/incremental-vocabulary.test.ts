import { expect, it } from 'vitest';
import { foldRuntimeEvent as fold } from '../../protocol/app-server/projection';
import type { RuntimeClientEvent as Event, RuntimeClientSnapshot as Snapshot } from '../../protocol/app-server/v34';
import capture from './fixtures/incremental-native.json';
import { interaction } from './fixture';

it('every Session event variant has an explicit transition or documented domain no-op', () => {
  let state = capture.initial.result.snapshot as unknown as Snapshot;
  const covered = new Set<string>();
  for (const frame of capture.events) { const event = frame.params.event as unknown as Event; state = fold(state, event); covered.add(event.type); }
  const apply = (event: Event) => { covered.add(event.type); state = fold(state, event); };
  const transcript = state.transcript, messages = state.messages;
  apply({ type: 'attempt_usage_updated', attempt_id: state.attempt!.attempt_id, usage: { input_tokens: 11, output_tokens: 17, total_tokens: 28 } });
  expect(state.attempt?.last_usage).toMatchObject({ input_tokens: 11, output_tokens: 17 });
  apply({ type: 'goal_changed', view: { current: null } }); expect(state.goal).toEqual({ current: null });
  apply({ type: 'workflows_updated', workflows: { revision: '7', runs: [], omitted_runs: 0 } }); expect(state.workflows?.revision).toBe('7');
  apply({ type: 'pending_inbound_changed', pending: [] }); expect(state.inbound.pending).toEqual([]);
  const pending = interaction('approval');
  apply({ type: 'interaction_pending', interaction: pending }); expect(state.pending_interactions).toContain(pending);
  apply({ type: 'interaction_settled', interaction: pending.interaction, outcome: { type: 'cancelled', reason: 'user_requested' } }); expect(state.pending_interactions).toEqual([]);
  apply({ type: 'interaction_pending', interaction: pending });
  apply({ type: 'interaction_removed', interaction: pending.interaction }); expect(state.pending_interactions).toEqual([]);
  const audit = { event_id: 'audit', timestamp: '2026-09-27T00:00:00Z', attempt_id: 'a', turn_id: 't', interaction_id: 'i' };
  const beforeAudit = state;
  apply({ type: 'interaction_audit_requested', transcript_cursor: '8', audit: { ...audit, subject: { type: 'approval', invocation_id: { caller: 'agent', call_id: 'c' }, tool_id: 'bash', tool_name: 'bash', arguments_digest: 'd', reason: 'approval' } } });
  apply({ type: 'interaction_audit_settled', transcript_cursor: '9', audit: { ...audit, settlement: { type: 'review_invalidated' } } });
  expect(state).toBe(beforeAudit); // Durable decorated entries arrive in read_domains_updated.
  apply({ type: 'context_compaction_started', context: { compaction_in_progress: true, compaction_count: 0 } }); expect(state.context?.compaction_in_progress).toBe(true);
  apply({ type: 'context_compaction_failed', context: { compaction_in_progress: false, compaction_count: 0 }, error: 'native failure' }); expect(state.context?.compaction_in_progress).toBe(false);
  apply({ type: 'context_compacted', context: { compaction_in_progress: false, compaction_count: 3 } }); expect(state.context?.compaction_count).toBe(3);
  apply({ type: 'job_updated', job: { job_id: 'e', tool_id: 'bash', tool_name: 'bash', state: 'succeeded' } }); expect(state.jobs?.[0].job_id).toBe('e');
  apply({ type: 'agent_updated', agent: { activation_id: 's', agent_id: 'child', parent_agent_id: 'parent', child_conversation_id: 'c', agent: 'worker', definition_digest: 'd', profile_digest: 'p', state: 'inactive', activation_state: 'succeeded', observation: { revision: '1', activity: { type: 'awaiting_activity' }, counters: { model_requests: 1, model_retries: 0, tool_executions: 0 } }, started_at: audit.timestamp, workspace: { logical_workspace: '/workspace', isolation: { type: 'shared' }, resource_state: 'none' } } }); expect(state.agents?.[0].activation_id).toBe('s');
  apply({ type: 'capability_updated', capabilities: { configured_tools: [], revision: '12' } }); expect(state.capabilities.revision).toBe('12');
  const native = capture.initial.result.snapshot as unknown as Snapshot;
  apply({ type: 'resource_generation_updated', plugins: native.effective_plugins, model: native.model!, approval_mode: 'full_access', capabilities: native.capabilities, resources: native.resources! }); expect(state.effective_approval_mode).toBe('full_access');
  apply({ type: 'session_model_changed', model: native.model! }); expect(state.model).toBe(native.model);
  expect(state.transcript).toBe(transcript); expect(state.messages).toBe(messages);
  const attempt_id = state.attempt!.attempt_id;
  apply({ type: 'assistant_message_started', attempt_id, message_id: 'unaccepted' });
  apply({ type: 'tool_call_started', attempt_id, message_id: 'unaccepted', block_index: 0, call: { id: 'exact', tool_id: 'bash', name: 'bash' } });
  apply({ type: 'tool_call_assembled', attempt_id, message_id: 'unaccepted', block_index: 0, call: { id: 'exact', tool_id: 'bash', name: 'bash', arguments: { n: 1 } }, arguments_json: '{"n":1.0}' });
  expect(state.attempt?.in_flight?.blocks?.[0]).toMatchObject({ arguments: '{"n":1.0}' });
  apply({ type: 'tool_execution_started', attempt_id, tool_call_id: 'exact', tool_id: 'bash' });
  apply({ type: 'tool_execution_progress', attempt_id, tool_call_id: 'exact', tool_id: 'bash', progress: { completed: 1, total: 2 } });
  expect(state.attempt?.foreground?.at(-1)?.state).toMatchObject({ type: 'running', progress: { completed: 1, total: 2 } });
  apply({ type: 'assistant_publication_settled', attempt_id, transcript_cursor: '10', audit: { stream_id: 's', attempt_id, turn_id: 't', request_id: 'r', message_id: 'unaccepted', kind: 'unaccepted', content: [], settled_at: audit.timestamp } }); expect(state.attempt?.in_flight).toBeUndefined();
  apply({ type: 'runtime_durability_failed', operation: 'commit', diagnostic: 'failure' }); expect(state.durability_failure).toEqual({ operation: 'commit', diagnostic: 'failure' });
  apply({ type: 'runtime_shutdown' }); expect(state.shutting_down).toBe(true);
  const vocabulary: Record<Event['type'], true> = {
    read_domains_updated: true, trace_changed: true, goal_changed: true, workflows_updated: true,
    attempt_started: true, attempt_settled: true, attempt_turn_updated: true, attempt_usage_updated: true,
    interaction_pending: true, interaction_settled: true, interaction_removed: true, interaction_audit_requested: true, interaction_audit_settled: true,
    context_compaction_started: true, context_compaction_failed: true, context_compacted: true,
    assistant_message_started: true, assistant_text_delta: true, assistant_reasoning_delta: true, assistant_refusal_delta: true,
    tool_call_started: true, tool_call_arguments_delta: true, tool_call_assembled: true, assistant_publication_settled: true,
    tool_execution_started: true, tool_execution_progress: true, tool_execution_settled: true, message_committed: true,
    agent_status_composed: true, pending_inbound_changed: true, inbound_enqueued: true, inbound_drained: true,
    job_updated: true, agent_updated: true, capability_updated: true, resource_generation_updated: true,
    session_model_changed: true, runtime_shutdown: true, runtime_durability_failed: true,
  };
  expect([...covered].sort()).toEqual(Object.keys(vocabulary).sort());
});
