import type { ContentRef, RuntimeClientMessageDescriptor, RuntimeClientSnapshot, TurnProcessView } from '../../../protocol/app-server/v72';
import type { Server } from '../fixture';
import { workflowRunsFixture } from './workflow-runs';
import { chatFlowFixture } from './chat-flow';
import { workflowTurnsFixture } from './workflow-turns';

/** Native layouts that put multiple visible surfaces in one canonical seat. */
export function chatSpacingFixture(server: Server, current: RuntimeClientSnapshot, kind: string | null) {
  if (kind === 'workflow-turns' || kind === 'workflow-turns-hidden' || kind === 'workflow-turns-status') {
    workflowTurnsFixture(server, current, kind === 'workflow-turns-hidden', kind === 'workflow-turns-status'); return;
  }
  if (kind === 'workflow') {
    workflowRunsFixture(server, current);
    current.messages = current.messages.slice(0, 2);
    current.transcript.entries = current.transcript.entries!.slice(0, 2);
    const work = current.transcript.entries[1], message = current.messages[1];
    if (message.role !== 'assistant') throw Error('Expected native calls');
    message.content.push({ type: 'tool_call', id: 'spacing-bash', tool_id: 'tool-bash', name: 'bash', arguments: { command: 'pwd' } });
    work.tool_calls!.push({ assembly_complete: true, committed_result_deferred: false, message_id: message.id, block_index: 2,
      call_id: 'spacing-bash', tool_id: 'tool-bash', name: 'bash', occurrence: { locator: { type: 'by_message_id', message_id: message.id }, block_index: 2 },
      state: { type: 'settled', arguments: '{"command":"pwd"}', result: { status: { type: 'success' }, duration_ms: '1', content: [{ type: 'text', text: '/workspace/A' }] } } });
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
  if (kind !== 'descriptor' && kind !== 'descriptor-tools') throw Error('Unknown spacing fixture');
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
  if (kind === 'descriptor-tools') {
    owner.message_count = 2; owner.tool_call_count = 1;
    const message_id = 'spacing-deferred-tools', locator = { type: 'by_message_id' as const, message_id };
    current.message_descriptors.unshift({ identity: message_id, locator, role: 'assistant', kind: 'message', block_count: '1', tool_call_count: '1',
      content: [], collections: [], turn_process: owner, tool_calls: [{ assembly_complete: true, committed_result_deferred: false, message_id, block_index: 0,
        call_id: 'spacing-deferred-bash', tool_id: 'tool-bash', name: 'bash', occurrence: { locator, block_index: 0 },
        state: { type: 'settled', arguments: '{"command":"pwd"}', result: { status: { type: 'success' }, duration_ms: '1', content: [{ type: 'text', text: '/workspace/A' }] } } }] });
  }
  server.handlers.set('conversation/contentRead', request => {
    if (request.method !== 'conversation/contentRead') throw Error('Expected content read');
    const offset = request.params.offset, end = Math.min(offset + request.params.max_bytes, body.length);
    return { type: 'content_chunk', chunk: { content_ref, offset, end_offset: end, readable_end: body.length,
      data_base64: btoa(String.fromCharCode(...body.slice(offset, end))), next_offset: end < body.length ? end : null, complete: end === body.length } };
  });
}
