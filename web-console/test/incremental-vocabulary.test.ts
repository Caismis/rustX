import { expect, it } from 'vitest';
import { checkpointSnapshot, transitionFacts, foldRuntimeEvent as fold } from '../../protocol/app-server/projection';
import type { RuntimeClientEvent as Event, RuntimeClientCheckpoint } from '../../protocol/app-server/v57';
import capture from './fixtures/incremental-native.json';
import { checkpoint, interaction } from './fixture';

it('every Session event variant has an explicit transition or documented domain no-op', () => {
  const nativeCheckpoint = (capture.initial.result as unknown as {checkpoint:RuntimeClientCheckpoint}).checkpoint;
  let state = checkpointSnapshot(nativeCheckpoint);
  const covered = new Set<string>();
  for (const frame of capture.events) { const event = frame.params.event as unknown as Event; state = fold(state, event); covered.add(event.type); transitionFacts(event).forEach(fact=>covered.add(fact.type)); }
  const apply = (event: Event) => { covered.add(event.type); transitionFacts(event).forEach(fact=>covered.add(fact.type)); state = fold(state, event); };
  const transcript = state.transcript, messages = state.messages;
  const attempt_id = state.attempt?.attempt_id;
  expect(typeof attempt_id).toBe('string');
  if (typeof attempt_id !== 'string') throw new Error('Native vocabulary capture lacks its exact inline Attempt identity.');
  apply({ type: 'attempt_usage_updated', attempt_id, usage: { input_tokens: "11", output_tokens: "17", total_tokens: "28" } });
  expect(state.attempt?.last_usage).toMatchObject({ input_tokens: '11', output_tokens: '17' });
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
  apply({ type: 'interaction_audit_settled', transcript_cursor: '9', audit: { ...audit, settlement: { type: 'approved' } } });
  expect(state).toBe(beforeAudit); // Durable decorated entries arrive in read_domains_updated.
  apply({ type: 'context_compaction_started', context: { compaction_in_progress: true, compaction_count: 0 } }); expect(state.context?.compaction_in_progress).toBe(true);
  apply({ type: 'context_compaction_failed', context: { compaction_in_progress: false, compaction_count: 0 }, error: 'native failure' }); expect(state.context?.compaction_in_progress).toBe(false);
  apply({ type: 'context_compacted', context: { compaction_in_progress: false, compaction_count: 3 } }); expect(state.context?.compaction_count).toBe(3);
  apply({ type: 'job_updated', position:0, job: { job_id: 'e', tool_id: 'bash', tool_name: 'bash', state: 'succeeded' } }); expect(state.jobs?.[0].job_id).toBe('e');
  apply({ type: 'agent_updated', position:0, agent: { title: 'worker', activation_id: 's', agent_id: 'child', parent_agent_id: 'parent', child_conversation_id: 'c', agent: 'worker', definition_digest: 'd', profile_digest: 'p', state: 'inactive', activation_state: 'succeeded', observation: { attempt_id: null, revision: '1', activity: { type: 'awaiting_activity' }, counters: { model_requests: 1, model_retries: 0, tool_executions: 0 } }, started_at: audit.timestamp, workspace: '/workspace' } }); expect(state.agents?.[0].activation_id).toBe('s');
  apply({ type: 'capability_updated', capabilities: { configured_tools: [], revision: '12' } }); expect(state.capabilities.revision).toBe('12');
  const native = nativeCheckpoint.snapshot;
  apply({ type: 'resource_generation_updated', plugins: native.effective_plugins, model: native.model!, model_status: {available:true,body_deferred:false,binding_revision:'1'}, approval_mode: 'full_access', capabilities: native.capabilities, resources: native.resources! }); expect(state.effective_approval_mode).toBe('full_access');
  apply({ type: 'session_model_changed', model: native.model!, model_status: {available:true,body_deferred:false,binding_revision:'1'} }); expect(state.model).toBe(native.model);
  expect(state.transcript).toBe(transcript); expect(state.messages).toBe(messages);
  apply({ type: 'assistant_message_started', attempt_id, message_id: 'unaccepted' });
  apply({ type: 'tool_call_started', position:0, foreground_position:0, attempt_id, message_id: 'unaccepted', block_index: 0, call: { id: 'exact', tool_id: 'bash', name: 'bash' } });
  apply({ type: 'tool_call_assembled', position:0, foreground_position:0, attempt_id, message_id: 'unaccepted', block_index: 0, call: { id: 'exact', tool_id: 'bash', name: 'bash', arguments: { n: 1 } }, arguments_json: '{"n":1.0}' });
  expect(state.attempt?.in_flight?.blocks?.[0]).toMatchObject({ arguments: '{"n":1.0}' });
  apply({ type: 'tool_execution_started', occurrence:{locator:{type:'by_message_id',message_id:'unaccepted'},block_index:0}, attempt_id, tool_call_id: 'exact', tool_id: 'bash' });
  apply({ type: 'tool_execution_progress', occurrence:{locator:{type:'by_message_id',message_id:'unaccepted'},block_index:0}, attempt_id, tool_call_id: 'exact', tool_id: 'bash', progress: { completed: 1, total: 2 } });
  expect(state.attempt?.foreground?.at(-1)?.state).toMatchObject({ type: 'running', progress: { completed: 1, total: 2 } });
  apply({ type: 'assistant_publication_settled', settlement_head:[], attempt_id, transcript_cursor: '10', audit: { stream_id: 's', attempt_id, turn_id: 't', request_id: 'r', message_id: 'unaccepted', kind: 'unaccepted', content: [], settled_at: audit.timestamp } }); expect(state.attempt?.in_flight).toBeUndefined();
  apply({type:'assistant_publication_deferred_settled',settlement_head:[],attempt_id,transcript_cursor:'10',settlement:{stream_id:'s',attempt_id,turn_id:'t',request_id:'r',message_id:'unaccepted',kind:'unaccepted',settled_at:audit.timestamp,block_count:'0'},audit:null,read_cut:nativeCheckpoint.read_cut.source_cut!,content:[],collections:[]});
  apply({ type: 'runtime_durability_failed', operation: 'commit', diagnostic: 'failure' }); expect(state.durability_failure).toEqual({ operation: 'commit', diagnostic: 'failure' });
  apply({ type: 'configuration_adoption_eligibility_changed', eligibility: { status: 'busy' } }); expect(state.configuration_adoption_eligibility).toEqual({ status: 'busy' });
  apply({type:'projection_transition',events:[{type:'trace_changed'}],metadata:{read_cut:checkpoint(state).read_cut,content:[],collections:[]}});
  apply({type:'checkpoint_advanced',transition:'display-bounded',checkpoint:checkpoint(state,'0')});
  apply({type:'pending_head_changed',owner:'primary',cut:{generation:'1',total:1},head:[pending]});expect(state.pending_interactions).toEqual([pending]);
  apply({type:'tool_occurrence_rebound',attempt_id,previous_locator:{type:'by_message_id',message_id:'unaccepted'},occurrence:{locator:{type:'by_ledger_position',position:9},block_index:0}});expect(state.attempt?.foreground?.at(-1)?.occurrence.locator).toEqual({type:'by_ledger_position',position:9});
  apply({result_deferred:false,type:'tool_result_committed',attempt_id,occurrence:{locator:{type:'by_ledger_position',position:9},block_index:0},result:{status:{type:'success'},content:[{type:'text',text:'canonical body'}],duration_ms:"0",exit_code:null},result_metadata_ref:null});
  expect(state.attempt?.foreground?.at(-1)?.committed_result?.content?.[0]).toEqual({type:'text',text:'canonical body'});
  expect(state.attempt?.foreground?.at(-1)?.state.type).toBe('running');
  apply({type:'assistant_message_started',attempt_id,message_id:'expired-live'});
  apply({type:'block_content_advanced',position:32,attempt_id,message_id:'expired-live',block_index:0,kind:'text',base_length:'0',end_length:'0',content_revision:'1',content_ref:{conversation_id:state.conversation_id,source:{type:'live_publication_prefix',component:'payload',stream_id:'expired-stream',message_id:'expired-live',attempt_id,turn_id:'t',publication_request_id:'r',block_index:0},representation:'utf8_text',version:1,content_revision:1,readable_end:0}});
  const descriptor={locator:{type:'by_ledger_position' as const,position:10},role:'assistant',kind:'message',block_count:'1',tool_call_count:'0',tool_calls:[],content:[],collections:[]};
  apply({type:'message_descriptor_committed',descriptor,head:[descriptor]});expect(state.message_descriptors).toEqual([descriptor]);
  apply({type:'collection_changed',domain:'agents',cut:{generation:'1',total:0}});
  apply({ type: 'runtime_shutdown' }); expect(state.shutting_down).toBe(true);
  const vocabulary: Record<Event['type'], true> = {
    pending_head_changed:true,tool_occurrence_rebound:true,tool_result_committed:true,message_descriptor_committed:true,collection_changed:true,projection_transition:true,checkpoint_advanced:true,block_content_advanced:true,
    read_domains_updated: true, trace_changed: true, goal_changed: true, workflows_updated: true,
    attempt_started: true, attempt_settled: true, attempt_turn_updated: true, attempt_usage_updated: true,
    interaction_pending: true, interaction_settled: true, interaction_removed: true, interaction_audit_requested: true, interaction_audit_settled: true,
    context_compaction_started: true, context_compaction_failed: true, context_compacted: true,
    assistant_message_started: true,
    tool_call_started: true, tool_call_assembled: true, assistant_publication_settled: true, assistant_publication_deferred_settled: true,
    tool_execution_started: true, tool_execution_progress: true, tool_execution_settled: true, message_committed: true,
    agent_status_composed: true, pending_inbound_changed: true, inbound_enqueued: true, inbound_drained: true,
    job_updated: true, agent_updated: true, capability_updated: true, resource_generation_updated: true,
    session_model_changed: true, runtime_shutdown: true, runtime_durability_failed: true,
    configuration_adoption_eligibility_changed: true,
  };
  expect([...covered].sort()).toEqual(Object.keys(vocabulary).sort());
});
