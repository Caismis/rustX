/** Pure Runtime Client read-model fold. Routing/cursor continuity belongs to
 * the attachment owner. No I/O, providers, React, or execution authority. */
import type { RuntimeClientSnapshot as Snapshot, RuntimeClientEvent as Event, InFlightBlock, ForegroundToolExecution } from './v32.ts';

const upsert = <T>(rows: readonly T[] | undefined, value: T, key: (row: T) => string): T[] => {
  const index = (rows ?? []).findIndex(row => key(row) === key(value));
  return index < 0 ? [...rows ?? [], value] : rows!.map((row, i) => i === index ? value : row);
};
const interactionKey = (ref: { conversation_id: string; interaction_id: string }) => JSON.stringify([ref.conversation_id, ref.interaction_id]);

export function foldRuntimeEvent(state: Snapshot, event: Event): Snapshot {
  const attempt = state.attempt;
  const withAttempt = (patch: Partial<NonNullable<Snapshot['attempt']>>) => attempt && 'attempt_id' in event && attempt.attempt_id === event.attempt_id
    ? { ...state, attempt: { ...attempt, ...patch } } : state;
  const withBlocks = (update: (blocks: InFlightBlock[]) => InFlightBlock[]) => attempt?.in_flight && 'message_id' in event && attempt.in_flight.message_id === event.message_id
    ? withAttempt({ in_flight: { ...attempt.in_flight, blocks: update(attempt.in_flight.blocks ?? []) } }) : state;
  const withTool = (call: string, update: (tool: ForegroundToolExecution) => ForegroundToolExecution) => withAttempt({ foreground: (attempt?.foreground ?? []).map(tool => tool.call_id === call ? update(tool) : tool) });
  switch (event.type) {
    case 'trace_changed': return state; // Separate read domain; cursor still advances.
    case 'read_domains_updated': return { ...state, transcript: event.transcript, todos: event.todos, context: { ...state.context!, last_request_occupancy: event.occupancy ?? undefined } };
    case 'goal_changed': return { ...state, goal: event.view };
    case 'workflows_updated': return { ...state, workflows: event.workflows };
    case 'attempt_started': return { ...state, attempt: { attempt_id: event.attempt_id, phase: { type: 'running' }, turn: 0, model: event.model, execution_settings: event.execution_settings, last_usage: undefined, in_flight: undefined, foreground: [] } };
    case 'attempt_settled': return { ...withAttempt({ phase: { type: 'settled', outcome: event.outcome } }), context: { ...state.context!, compaction_in_progress: false } };
    case 'attempt_turn_updated': return withAttempt({ turn: event.turn });
    case 'attempt_usage_updated': return withAttempt({ last_usage: event.usage });
    case 'assistant_message_started':
      return state.messages.some(message => message.id === event.message_id) ? state : withAttempt({ in_flight: { message_id: event.message_id, blocks: [] } });
    case 'assistant_text_delta':
    case 'assistant_reasoning_delta':
    case 'assistant_refusal_delta': {
      const type = event.type === 'assistant_text_delta' ? 'text' : event.type === 'assistant_reasoning_delta' ? 'reasoning' : 'refusal';
      return withBlocks(blocks => {
        const existing = blocks.find(block => block.block_index === event.block_index);
        const block: InFlightBlock = { type, block_index: event.block_index, text: (existing && 'text' in existing ? existing.text : '') + event.delta };
        return upsert(blocks, block, b => String(b.block_index)).sort((a, b) => a.block_index - b.block_index);
      });
    }
    case 'tool_call_started': {
      if (!attempt?.in_flight || attempt.in_flight.message_id !== event.message_id) return state;
      const next = withBlocks(blocks => [...blocks, { type: 'tool_call', block_index: event.block_index, call_id: event.call.id, tool_id: event.call.tool_id, name: event.call.name, arguments: '' }]);
      return { ...next, attempt: { ...next.attempt!, foreground: [...attempt.foreground ?? [], { message_id: event.message_id, block_index: event.block_index, call_id: event.call.id, tool_id: event.call.tool_id, name: event.call.name, state: { type: 'assembled', arguments: '' } }] } };
    }
    case 'tool_call_arguments_delta':
    case 'tool_call_assembled': {
      if (!attempt?.in_flight || attempt.in_flight.message_id !== event.message_id) return state;
      const call = event.type === 'tool_call_assembled' ? event.call.id : event.call_id;
      const argumentsText = (previous: string) => event.type === 'tool_call_assembled' ? event.arguments_json : previous + event.arguments_delta;
      const next = withBlocks(blocks => blocks.map(block => block.type === 'tool_call' && block.call_id === call ? { ...block, arguments: argumentsText(block.arguments) } : block));
      return { ...next, attempt: { ...next.attempt!, foreground: (attempt.foreground ?? []).map(tool => tool.call_id === call ? { ...tool, state: { ...tool.state, arguments: argumentsText(tool.state.arguments) } } : tool) } };
    }
    case 'tool_execution_started': return withTool(event.tool_call_id, tool => tool.state.type === 'settled' ? tool : { ...tool, state: { type: 'running', arguments: tool.state.arguments, progress: null } });
    case 'tool_execution_progress': return withTool(event.tool_call_id, tool => tool.state.type === 'running' ? { ...tool, state: { ...tool.state, progress: event.progress } } : tool);
    case 'tool_execution_settled': return withTool(event.tool_call_id, tool => tool.state.type === 'settled' ? tool : { ...tool, state: { type: 'settled', arguments: tool.state.arguments, result: event.result } });
    case 'message_committed': {
      // Native canonical content always wins; a suffix never creates a seat.
      const next = event.message.role === 'assistant' ? withAttempt({ in_flight: undefined }) : state;
      return { ...next, messages: upsert(state.messages, event.message, message => message.id),
        transcript: event.transcript_cursor == null ? state.transcript : { ...state.transcript, entries: upsert(state.transcript.entries, { cursor: event.transcript_cursor, item: { type: 'message', message: event.message } }, row => row.cursor) } };
    }
    case 'assistant_publication_settled': return withAttempt({ in_flight: undefined });
    case 'interaction_pending': return { ...state, pending_interactions: upsert(state.pending_interactions, event.interaction, row => interactionKey(row.interaction)) };
    case 'interaction_settled':
    case 'interaction_removed': return { ...state, pending_interactions: (state.pending_interactions ?? []).filter(row => interactionKey(row.interaction) !== interactionKey(event.interaction)) };
    // Durable audit/response decorations arrive in the native read-domain cut.
    case 'interaction_audit_requested':
    case 'interaction_audit_settled': return state;
    case 'context_compaction_started': return { ...state, context: { ...state.context!, compaction_in_progress: true } };
    case 'context_compaction_failed': return { ...state, context: { ...state.context!, compaction_in_progress: false } };
    case 'context_compacted': return { ...state, context: event.context };
    case 'agent_status_composed': return { ...state, statuses: upsert((state.statuses ?? []).filter(status => status.status_message_id !== event.evicted_status_message_id), event.status, status => status.status_message_id) };
    case 'pending_inbound_changed': return { ...state, inbound: { ...state.inbound, pending: event.pending } };
    case 'inbound_enqueued': return { ...state, inbound: { ...state.inbound, pending: [...state.inbound.pending ?? [], { sequence: event.sequence, revision: "0", message: event.message }] } };
    case 'inbound_drained': return { ...state, inbound: { pending: (state.inbound.pending ?? []).filter(item => BigInt(item.sequence) > BigInt(event.watermark)), last_drain: { watermark: event.watermark, count: event.count } } };
    case 'job_updated': return { ...state, jobs: upsert(state.jobs, event.job, row => row.job_id) };
    case 'agent_updated': return { ...state, agents: upsert(state.agents, event.agent, row => row.agent_id) };
    case 'capability_updated': return { ...state, capabilities: event.capabilities };
    case 'resource_generation_updated': return { ...state, effective_plugins: event.plugins, model: event.model, effective_approval_mode: event.approval_mode, capabilities: event.capabilities, resources: event.resources };
    case 'session_model_changed': return { ...state, model: event.model };
    case 'runtime_shutdown': return { ...state, shutting_down: true };
    case 'runtime_durability_failed': return { ...state, durability_failure: { operation: event.operation, diagnostic: event.diagnostic } };
    default: { const exhaustive: never = event; throw new Error(`Unknown Runtime Client event: ${exhaustive}`); }
  }
}
