import { cleanup, fireEvent, render } from '@testing-library/react';
import { afterEach, expect, it } from 'vitest';
import type { ContentBinding, ContentRef, RuntimeClientSnapshot, TurnProcessView } from '../../protocol/app-server/v59';
import { AgentTranscript } from '../src/app/agent/AgentTranscript';
import { JournalSourceProvider } from '../src/app/agent/ObservationSource';
import { Server, readCut, snapshot } from './fixture';

afterEach(cleanup);

it.each(['live', 'child', 'history', 'evicted-inline'] as const)('%s reply survives settlement when its text and reasoning are deferred', async mode => {
  const server = new Server(); await server.attached('A');
  try {
    const body = 'The complete answer remains visible after settlement.';
    const reasoning = 'The complete reasoning remains available inside the process.';
    const values = [reasoning, body];
    const message = { role: 'assistant' as const, id: 'final-answer', content: [{ type: 'reasoning' as const, text: '' }, { type: 'text' as const, text: '' }] };
    const owner: TurnProcessView = { conversation_id: 'conversation-A', attempt_id: 'attempt-A', attempt_id_source: null,
      final_message_id: message.id, final_message_locator: { type: 'by_message_id', message_id: message.id }, final_message_id_deferred: false, final_message_id_source: null,
      outcome: 'completed', control_cursor: '17', message_count: 1, tool_call_count: 0 };
    const historyCut = { read: readCut(), ledger_through: '17', transcript_total: '17', surface_revision: '0', inherited_through: '0' };
    server.handlers.set('session/tracePage', request => {
      if (request.method !== 'session/tracePage' || request.params.domain !== 'trace.records') throw Error('Expected historical Trace headers');
      return { type: 'checkpoint_page', page: { read_cut: request.params.cut, cursor: request.params.cut.revision, domain: 'trace.records', total: '0', items: [], next_offset: null, collections: [] } };
    });
    const refs: ContentRef[] = values.map((text, block_index) => ({ conversation_id: 'conversation-A', source: { type: 'canonical_message_block', component: 'payload',
      locator: mode === 'history' ? { type: 'by_ledger_position', position: 17 } : { type: 'by_message_id', message_id: message.id }, block_index },
      representation: 'utf8_text', version: 1, content_revision: 1, readable_end: new TextEncoder().encode(text).length }));
    const bindings: ContentBinding[] = refs.map((ref, index) => ({ path: mode === 'history' ? ['transcript', 'entries', '0', 'item', 'message', 'content', String(index), 'text'] : ['messages', '0', 'content', String(index), 'text'],
      content_ref: ref, deferred: mode !== 'evicted-inline' }));
    const method = mode === 'history' ? 'session/historyContentRead' : 'conversation/contentRead';
    server.handlers.set(method, request => {
      if (request.method !== 'session/historyContentRead' && request.method !== 'conversation/contentRead') throw Error('Expected original content read');
      const ref = request.params.content_ref, bytes = new TextEncoder().encode(values[ref.source.block_index]);
      expect(ref).toEqual(refs[ref.source.block_index]);
      if (request.method === 'session/historyContentRead') expect(request.params.cut).toEqual(historyCut);
      else expect(request.params.agent_id).toBe(mode === 'child' ? 'worker' : null);
      return { type: 'content_chunk', chunk: { content_ref: ref, offset: 0, end_offset: bytes.length, readable_end: bytes.length,
        data_base64: btoa(values[ref.source.block_index]), next_offset: null, complete: true } };
    });
    const source = mode === 'history' ? { client: server.client, sessionId: 'A', historyCut, cut: () => undefined, owned: () => true }
      : { client: server.client, target: server.target('A'), agentId: mode === 'child' ? 'worker' : undefined, cut: () => undefined, owned: () => true };
    const settled: RuntimeClientSnapshot = { ...snapshot(), messages: [message], transcript: { entries: [{ cursor: '17', turn_process: owner, item: { type: 'message', message } }] } };
    const running: RuntimeClientSnapshot = { ...snapshot(), messages: [], transcript: { entries: [] }, attempt: { attempt_id: 'attempt-A', phase: { type: 'running' }, turn: 1,
      in_flight: { message_id: message.id, blocks: [{ type: 'reasoning', block_index: 0, text: reasoning }, { type: 'text', block_index: 1, text: body }] } } };
    const view = (done: boolean) => <JournalSourceProvider source={source} content={done ? bindings : []} messages={done ? [message] : []} transcript={settled.transcript} history={mode === 'history' ? settled.transcript : undefined}>
      <AgentTranscript snapshot={done ? settled : running}/>
    </JournalSourceProvider>;
    const ui = render(view(false)); expect(ui.getByText(body).closest('[hidden]')).toBeNull();
    ui.rerender(view(true));
    const answer = await ui.findByText(body); expect(answer.closest('[hidden]')).toBeNull();
    fireEvent.click(ui.getByRole('button', { name: 'Worked' }));
    fireEvent.click(ui.getByRole('button', { name: 'Analysis completed' }));
    fireEvent.click(ui.getByRole('button', { name: /^Reasoning/ }));
    expect((await ui.findByText(reasoning)).closest('[hidden]')).toBeNull();
    expect(ui.queryByRole('alert')).toBeNull();
    expect(server.requests.filter(row => row.request.method === method)).toHaveLength(2);
  } finally { cleanup(); server.client.disconnect(); }
});
