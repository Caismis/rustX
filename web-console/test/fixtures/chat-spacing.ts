import type { ContentRef, RuntimeClientMessageDescriptor, RuntimeClientSnapshot, TurnProcessView } from '../../../protocol/app-server/v82';
import type { Server } from '../fixture';
import { workflowRunsFixture } from './workflow-runs';
import { chatFlowFixture } from './chat-flow';
import { workflowTurnsFixture } from './workflow-turns';

/** Native layouts that put multiple visible surfaces in one canonical seat. */
export function chatSpacingFixture(server: Server, current: RuntimeClientSnapshot, kind: string | null) {
  if (kind === 'workflow-turns' || kind === 'workflow-turns-hidden' || kind === 'workflow-turns-status') {
    workflowTurnsFixture(server, current, kind === 'workflow-turns-hidden', kind === 'workflow-turns-status'); return;
  }
  if (kind === 'workflow-steer') {
    workflowRunsFixture(server, current);
    const entries = current.transcript.entries!.slice(0, 2);
    const call = entries[1];
    if (call.item.type !== 'message' || call.item.message.role !== 'assistant') throw Error('Expected workflow call');
    // Only the first call is inside the loaded window; its inline block is
    // available before the richer tool projection.
    call.item.message.content = call.item.message.content.slice(0, 1);
    const tool = call.tool_calls![0];
    server.handlers.set('conversation/checkpointPage', request => {
      if (request.method !== 'conversation/checkpointPage' || request.params.domain !== 'messages.message:workflow-work.tools') throw Error('Expected original tool projection');
      return { type: 'checkpoint_page', page: { read_cut: request.params.cut, cursor: request.params.cut.revision, domain: request.params.domain,
        total: '1', next_offset: null, collections: [], items: [{ offset: '0', identity: tool.call_id!, value: tool, content: [], deferred: false }] } };
    });
    call.tool_calls = [];
    const steer = { role: 'user' as const, id: 'steer', source: 'human' as const, content: [{ type: 'text' as const, text: 'Change direction' }] };
    const job = { role: 'user' as const, id: 'background-bash-terminal', source: 'runtime' as const,
      content: [{ type: 'text' as const, text: 'Background execution bash (bash) settled: succeeded' }] };
    entries.push({ cursor: '3', item: { type: 'message', message: steer } }, { cursor: '4', item: { type: 'message', message: job } });
    current.transcript.entries = entries;
    current.messages = entries.flatMap(entry => entry.item.type === 'message' ? [entry.item.message] : []);
    current.inbound = { pending: [
      { sequence: '1', revision: '0', input_id: 'input-steer', message: steer },
      { sequence: '2', revision: '0', message: { ...steer, id: 'next-steer', content: [{ type: 'text', text: 'One more instruction' }] } },
    ] };
    current.attempt = { attempt_id: 'next-attempt', phase: { type: 'running' }, turn: 2,
      in_flight: { message_id: 'current-answer', blocks: [{ block_index: 0, type: 'text', text: 'Current reply' }] } };
    return;
  }
  if (kind === 'workflow') {
    workflowRunsFixture(server, current);
    current.messages = current.messages.slice(0, 2);
    current.transcript.entries = current.transcript.entries!.slice(0, 2);
    const work = current.transcript.entries[1], message = current.messages[1];
    if (message.role !== 'assistant') throw Error('Expected native calls');
    message.content.push({ type: 'tool_call', id: 'spacing-bash', tool_id: 'tool-bash', name: 'bash', arguments: { arguments: { command: 'pwd' } } });
    work.tool_calls!.push({ assembly_complete: true, committed_result_deferred: false, message_id: message.id, block_index: 2,
      call_id: 'spacing-bash', tool_id: 'tool-bash', name: 'bash', occurrence: { locator: { type: 'by_message_id', message_id: message.id }, block_index: 2 },
      state: { type: 'settled', arguments: "{\"arguments\":{\"command\":\"pwd\"}}", result: { status: { type: 'success' }, duration_ms: '1', content: [{ type: 'text', text: '/workspace/A' }] } } });
    work.turn_process = { ...work.turn_process!, outcome: 'running', tool_call_count: 3, message_count: 1, final_message_id: null, final_message_locator: null };
    current.attempt = { attempt_id: work.turn_process.attempt_id, phase: { type: 'running' }, turn: 1 };
    return;
  }
  if (kind === 'status') {
    chatFlowFixture(server, current);
    current.statuses = [
      { attempt_id: 'flow-attempt', turn: 1, status_message_id: 'spacing-inbound-status', opportunities: { fresh_inbound: { target_message_id: 'flow-user' } },
        sections: [{ type: 'temporal', current_time: '2026-11-03T06:43:00Z', timezone: 'Asia/Shanghai' }], rendered: 'Native time observation' },
      { attempt_id: 'flow-attempt', turn: 1, status_message_id: 'spacing-tool-status', opportunities: { post_tool_batch: { transcript_anchor: '2' } },
        sections: [{ type: 'temporal', current_time: '2026-11-03T06:44:00Z', timezone: 'Asia/Shanghai' }], rendered: 'Native time observation after tools' },
    ];
    return;
  }
  const denied = kind === 'descriptor-denied' || kind === 'descriptor-denied-inline';
  if (!denied && kind !== 'descriptor' && kind !== 'descriptor-tools' && kind !== 'descriptor-approval') throw Error('Unknown spacing fixture');
  const id = 'spacing-deferred-answer', locator = { type: 'by_message_id' as const, message_id: id };
  const body = new TextEncoder().encode('## Canonical answer\n\nThe original bounded content remains readable.');
  const owner: TurnProcessView = { conversation_id: current.conversation_id, attempt_id: 'spacing-deferred-attempt', attempt_id_source: null,
    final_message_id: id, final_message_locator: locator, final_message_id_deferred: false, final_message_id_source: null,
    outcome: 'completed', control_cursor: '2', message_count: 1, tool_call_count: 0 };
  const content_ref: ContentRef = { conversation_id: current.conversation_id,
    source: { type: 'canonical_message_block', component: 'payload', locator, block_index: 0 },
    representation: 'utf8_text', version: 1, content_revision: 1, readable_end: body.length };
  const descriptor: RuntimeClientMessageDescriptor = { identity: id, locator, role: 'assistant', kind: 'message', block_count: '1', tool_call_count: '0',
    tool_calls: [], collections: [], turn_process: owner,
    content: [{ path: ['content', '0', 'text'], content_ref, deferred: true }],
    completed_response: { origin: { conversation_id: owner.conversation_id, attempt_id: owner.attempt_id, attempt_id_source: null, closing_message_id: id },
      closing_message_id: id, closing_locator: locator, deferred_fields: [], surface_revision: '1', completed_at: '2026-11-03T06:43:00Z' } };
  current.attempt = null; current.messages = current.messages.slice(0, 1);
  current.transcript.entries = current.transcript.entries!.slice(0, 1);
  current.message_descriptors = [descriptor];
  if (kind !== 'descriptor') {
    owner.message_count = 2; owner.tool_call_count = 1;
    const message_id = 'spacing-deferred-tools', locator = { type: 'by_message_id' as const, message_id };
    current.message_descriptors.unshift({ identity: message_id, locator, role: 'assistant', kind: 'message', block_count: '1', tool_call_count: '1',
      content: [], collections: [], turn_process: owner, tool_calls: [{ assembly_complete: true, committed_result_deferred: false, message_id, block_index: 0,
        call_id: 'spacing-deferred-bash', tool_id: 'tool-bash', name: 'bash', occurrence: { locator, block_index: 0 },
        state: { type: 'settled', arguments: "{\"arguments\":{\"command\":\"pwd\"}}", result: { status: { type: 'success' }, duration_ms: '1', content: [{ type: 'text', text: '/workspace/A' }] } } }] });
  }
  if (denied) {
    const [tools, answer] = current.message_descriptors;
    const tool = tools.tool_calls[0];
    const reason = 'Auxiliary permission review denied this write';
    tools.tool_calls[0] = { ...tool, call_id: 'spacing-denied-write', tool_id: 'tool-write', name: 'write',
      state: { type: 'settled', arguments: JSON.stringify({ arguments: { path: 'notes.txt', content: 'notes' } }),
        result: { status: { type: 'denied', reason }, duration_ms: '0', content: [{ type: 'text', text: reason }] } } };
    current.transcript.deferred_entries = [{ offset: '2', identity: tools.identity!, value: tools, deferred: true, content: [] }];
    if (kind === 'descriptor-denied-inline') {
      current.message_descriptors = [tools];
      current.transcript.entries.push({ cursor: '3', turn_process: owner, completed_response: answer.completed_response,
        item: { type: 'message', message: { role: 'assistant', id, content: [{ type: 'text', text: new TextDecoder().decode(body) }] } } });
    } else {
      current.transcript.deferred_entries.push({ offset: '3', identity: id, value: answer, deferred: true, content: [] });
    }
  }
  if (kind === 'descriptor-approval') {
    const [tools, answer] = current.message_descriptors;
    current.transcript.deferred_entries = [tools, answer].map(value => ({
      offset: value === tools ? '2' : '6', identity: value.identity!, value, deferred: true, content: [],
    }));
    const receipt = { attempt_id: owner.attempt_id!, interaction_id: 'approval', timestamp: '2026-11-03T06:42:00Z' };
    current.transcript.entries.push({ cursor: '3', item: { ...receipt, type: 'interaction_requested', event_id: 'requested',
      subject: { type: 'approval', invocation_id: { caller: 'agent', call_id: 'spacing-deferred-bash' },
        tool_id: 'tool-bash', tool_name: 'bash', arguments_digest: 'digest', reason: 'Run command' } } },
    { cursor: '4', item: { ...receipt, type: 'interaction_settled', event_id: 'settled', settlement: { type: 'approved' } } },
    { cursor: '5', turn_process: owner, item: { type: 'message', message: { role: 'tool', id: 'bash-result',
      occurrence: { assistant_message_id: tools.identity!, block_index: 0 }, tool_call_id: 'spacing-deferred-bash',
      tool_id: 'tool-bash', result: { status: { type: 'success' }, duration_ms: '1', content: [] } } } });
  }
  server.handlers.set('conversation/contentRead', request => {
    if (request.method !== 'conversation/contentRead') throw Error('Expected content read');
    const offset = request.params.offset, end = Math.min(offset + request.params.max_bytes, body.length);
    return { type: 'content_chunk', chunk: { content_ref, offset, end_offset: end, readable_end: body.length,
      data_base64: btoa(String.fromCharCode(...body.slice(offset, end))), next_offset: end < body.length ? end : null, complete: end === body.length } };
  });
}
