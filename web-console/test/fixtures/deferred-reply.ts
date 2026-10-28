import type { ContentRef, RuntimeClientSnapshot } from '../../../protocol/app-server/v62';
import { checkpoint, type Server } from '../fixture';

/** Native settlement replaces the live text with bounded canonical headers. */
export function deferredReplyFixture(server: Server, snapshot: RuntimeClientSnapshot) {
  const reasoning = 'The full reasoning is available from the canonical content reference.';
  const body = '## Deferred answers remain visible\n\nThe complete answer survives native settlement and a fresh checkpoint read.\n\n'
    + Array.from({ length: 8 }, (_, i) => `${i + 1}. The runtime keeps the answer; the browser reads its original content reference.`).join('\n');
  const values = [reasoning, body].map(text => new TextEncoder().encode(text));
  const message = { role: 'assistant' as const, id: 'deferred-answer', content: [{ type: 'reasoning' as const, text: '' }, { type: 'text' as const, text: '' }] };
  const refs: ContentRef[] = values.map((bytes, block_index) => ({ conversation_id: snapshot.conversation_id,
    source: { type: 'canonical_message_block', component: 'payload', locator: { type: 'by_message_id', message_id: message.id }, block_index },
    representation: 'utf8_text', version: 1, content_revision: 1, readable_end: bytes.length }));
  snapshot.messages = snapshot.messages.slice(0, 1);
  snapshot.transcript.entries = snapshot.transcript.entries!.slice(0, 1);
  snapshot.attempt = { attempt_id: 'attempt-1', phase: { type: 'running' }, turn: 1,
    in_flight: { message_id: message.id, blocks: [{ type: 'reasoning', block_index: 0, text: reasoning }, { type: 'text', block_index: 1, text: body }] } };
  const currentCheckpoint = () => {
    const current = server.snapshots.get('A')!, cut = checkpoint(current, String(server.cursor));
    if (current.messages.some(row => row.id === message.id)) cut.content = refs.map((content_ref, i) => ({
      path: ['messages', '1', 'content', String(i), 'text'], content_ref, deferred: true,
    }));
    return cut;
  };
  server.handlers.set('session/checkpoint', () => ({ type: 'checkpoint', checkpoint: currentCheckpoint() }));
  server.handlers.set('conversation/contentRead', request => {
    if (request.method !== 'conversation/contentRead') throw Error('Expected original content read');
    const ref = request.params.content_ref, bytes = values[ref.source.block_index];
    if (JSON.stringify(ref) !== JSON.stringify(refs[ref.source.block_index])) throw Error('Unexpected content reference');
    const offset = request.params.offset, end = Math.min(offset + request.params.max_bytes, bytes.length);
    return { type: 'content_chunk', chunk: { content_ref: ref, offset, end_offset: end, readable_end: bytes.length,
      data_base64: btoa(String.fromCharCode(...bytes.slice(offset, end))), next_offset: end < bytes.length ? end : null, complete: end === bytes.length } };
  });
  window.deferredReplyFixture = {
    settle() {
      const next = structuredClone(server.snapshots.get('A')!);
      next.attempt = { attempt_id: 'attempt-1', turn: 1, phase: { type: 'settled', outcome: { type: 'completed', finish_reason: { type: 'stop' } } } };
      next.messages.push(message);
      next.transcript.entries!.push({ cursor: '2', item: { type: 'message', message }, turn_process: {
        conversation_id: next.conversation_id, attempt_id: 'attempt-1', attempt_id_source: null, final_message_id: message.id,
        final_message_locator: { type: 'by_message_id', message_id: message.id }, final_message_id_deferred: false, final_message_id_source: null,
        outcome: 'completed', control_cursor: '2', message_count: 1, tool_call_count: 0,
      } });
      server.snapshots.set('A', next); server.cursor++;
      server.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target: server.target('A'), cursor: String(server.cursor),
        event: { type: 'checkpoint_advanced', transition: 'fixture-settlement', checkpoint: currentCheckpoint() } } });
    },
    refresh: () => server.client.refresh('A'),
    reads: () => server.requests.filter(row => row.request.method === 'conversation/contentRead').length,
  };
}

declare global {
  interface Window { deferredReplyFixture: { settle(): void; refresh(): Promise<void>; reads(): number } }
}
