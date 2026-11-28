import { contentAdvance, utf8Length } from './fixture';
import { writeFileSync } from 'node:fs';
import { act, cleanup, render, fireEvent } from '@testing-library/react';
import { Profiler } from 'react';
import { afterEach, expect, it, vi } from 'vitest';
import { Server, snapshot } from './fixture';
import { ConversationLive } from '../src/app/agent/ConversationLive';
import type { RuntimeClientEvent, RuntimeClientSnapshot } from '../../protocol/app-server/v82';

afterEach(() => { cleanup(); vi.unstubAllGlobals(); vi.restoreAllMocks(); });
it('fixed 420 long response measurement', async () => {
  const frames = new Map<number, FrameRequestCallback>(); let frameId = 0;
  vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => { frames.set(++frameId, cb); return frameId; });
  vi.stubGlobal('cancelAnimationFrame', (id: number) => frames.delete(id));
  let resize = () => {};
  vi.stubGlobal('ResizeObserver', class { constructor(cb: () => void) { resize = cb; } observe() {} disconnect() {} });
  const flush = () => { const work = [...frames.values()]; frames.clear(); for (const cb of work) cb(0); };
  const s = new Server();
  const liveBodies = new Map<number, string>();
  s.handlers.set('conversation/contentRead', request => {
    if (request.method !== 'conversation/contentRead') throw new Error('Expected a content read');
    const { content_ref: reference, offset, max_bytes } = request.params;
    const source = reference.source;
    if (source.type !== 'live_publication_prefix' || source.attempt_id !== 'attempt-A' || source.message_id !== 'answer') throw new Error('Unknown incremental content source');
    const bytes = new TextEncoder().encode(liveBodies.get(source.block_index) ?? '');
    if (reference.readable_end > bytes.length || offset > reference.readable_end) throw new Error('Incremental read exceeds its emitted frontier');
    const end = Math.min(reference.readable_end, offset + max_bytes);
    return { type: 'content_chunk', chunk: { content_ref: reference, offset, data_base64: btoa(String.fromCharCode(...bytes.slice(offset, end))), end_offset: end, readable_end: reference.readable_end, next_offset: end < reference.readable_end ? end : null, complete: end === reference.readable_end } };
  });
  await s.attached('A');
  let commits = 0, writes = 0, bottomWrites = 0, anchorWrites = 0, top = 0, height = 800;
  const ui = render(<Profiler id="transcript" onRender={() => commits++}><ConversationLive client={s.client} sessionId="A" mode="chat" disabled={false} onHistorical={() => {}}/></Profiler>);
  const el = ui.container.querySelector('.conversation-scroll') as HTMLElement;
  Object.defineProperties(el, { scrollHeight: { get: () => height }, clientHeight: { get: () => 200 }, scrollTop: { get: () => top, set: value => { writes++; if (value === height - 200) bottomWrites++; else anchorWrites++; top = value; } } });
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(function(this: HTMLElement) {
    const y = this.dataset.chatAnchorKey ? 400 - top : 0;
    return { x: 0, y, top: y, bottom: y + 200, width: 500, height: 200, left: 0, right: 500, toJSON() {} };
  });
  await act(async () => { resize(); flush(); });
  const emit = async (event: RuntimeClientEvent, next: RuntimeClientSnapshot = s.snapshots.get('A')!) => {
    if (next.attempt?.in_flight && (event.type === 'tool_call_started' || event.type === 'block_content_advanced' && event.kind === 'tool_call_arguments' || event.type === 'tool_call_assembled')) {
      const blocks = [...next.attempt.in_flight.blocks ?? []];
      if (event.type === 'tool_call_started') blocks.push({ type: 'tool_call', block_index: event.block_index, call_id: event.call.id, tool_id: event.call.tool_id, name: event.call.name, arguments: '' });
      else { const index = blocks.findIndex(b => b.block_index === event.block_index); const block = blocks[index]; if (block?.type === 'tool_call') blocks[index] = { ...block, arguments: event.type === 'tool_call_assembled' ? event.arguments_json : event.type === 'block_content_advanced' ? block.arguments + (event.inline_delta ?? '') : block.arguments }; }
      const foreground = blocks.flatMap(block => block.type === 'tool_call' ? [{committed_result_deferred:false,occurrence:{locator:{type:'by_message_id' as const,message_id:next.attempt!.in_flight!.message_id},block_index:block.block_index},assembly_complete:true, message_id: next.attempt!.in_flight!.message_id, block_index: block.block_index, call_id: block.call_id, tool_id: block.tool_id, name: block.name, state: { type: 'assembled' as const, arguments: block.arguments } }] : []);
      next = { ...next, attempt: { ...next.attempt, foreground, in_flight: { ...next.attempt.in_flight, blocks } } };
    }
    for (const block of next.attempt?.in_flight?.blocks ?? []) {
      if (block.type === 'text' || block.type === 'reasoning' || block.type === 'refusal') liveBodies.set(block.block_index, block.text);
      else if (block.type === 'tool_call') liveBodies.set(block.block_index, block.arguments);
    }
    s.snapshots.set('A', next); s.cursor++;
    await act(async () => {
      const observed = new Promise<void>(resolve => {
        const stop = s.client.subscribe(() => { if (BigInt(s.client.getSnapshot().views.A.cursor!) >= s.cursor) { stop(); resolve(); } });
      });
      s.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target: s.target('A'), cursor: String(s.cursor), event } });
      await observed;
    });
  };
  const attempt_id = 'attempt-A', message_id = 'answer';
  let next: RuntimeClientSnapshot = { ...snapshot(), attempt: { attempt_id, phase: { type: 'running' }, turn: 0, in_flight: { message_id, blocks: [] } } };
  await emit({ type: 'attempt_started', attempt_id }, next);
  await emit({ type: 'assistant_message_started', attempt_id, message_id }, next);
  const reasoning = { type: 'reasoning' as const, block_index: 1, text: 'Reasoning before the long response.' };
  next = { ...next, attempt: { ...next.attempt!, in_flight: { message_id, blocks: [reasoning] } } };
  await emit(contentAdvance({attempt_id:attempt_id,message_id:message_id,block_index:1,kind:'reasoning',delta:reasoning.text}, 0, 1), next);
  let text = '';
  let seat: Element | null = null, replacements = 0;
  const startWrites = writes;
  for (let i = 0; i < 200; i++) {
    const delta = `paragraph ${i}: ${'fixed deterministic long response. '.repeat(8)}\n`;
    text += delta;
    next = { ...next, attempt: { ...next.attempt!, in_flight: { message_id, blocks: [{ type: 'text', block_index: 0, text }, reasoning] } } };
    await emit(contentAdvance({attempt_id:attempt_id,message_id:message_id,block_index:0,kind:'text',delta:delta}, utf8Length(text) - utf8Length(delta), i + 1), next);
    height += 10;
    await act(async () => { for (let n = 0; n < 10; n++) resize(); flush(); });
    const row = ui.container.querySelector('[data-chat-anchor-key="message:answer"]');
    if (seat && row !== seat) replacements++; seat = row;
    if (i === 99) {
      top = 210;
      fireEvent.scroll(el);
      // Settle the gesture between publications at a deterministic commit boundary.
      fireEvent(el, new Event('scrollend'));
    }
  }
  await emit({ type: 'tool_call_started', position: 2, foreground_position: 0, attempt_id, message_id, block_index: 2, call: { id: 'call', tool_id: 'bash', name: 'bash' } });
  let rawPrefix = '';
  for (const arguments_delta of ['{"command":', '"printf test"', '}']) { await emit(contentAdvance({attempt_id,message_id,block_index:2,kind:'tool_call_arguments',delta:arguments_delta}, utf8Length(rawPrefix), utf8Length(rawPrefix) + 1)); rawPrefix += arguments_delta; }
  await emit({ type: 'tool_call_assembled', position: 2, foreground_position: 0, arguments_json: '{"command":"printf test"}', attempt_id, message_id, block_index: 2, call: { id: 'call', tool_id: 'bash', name: 'bash', arguments: { arguments: { command: 'printf test' } } } });
  height += 123; await act(async () => { resize(); flush(); });
  next = s.snapshots.get('A')!;
  const message = { role: 'assistant' as const, id: message_id, content: [{ type: 'text' as const, text: 'Canonical final content.' }, { type: 'reasoning' as const, text: reasoning.text }, { type: 'tool_call' as const, id: 'call', tool_id: 'bash', name: 'bash', arguments: { arguments: { command: 'printf test' } } }] };
  await emit({ type: 'message_committed', attempt_id, message, transcript_cursor: '1' }, { ...next, messages: [message], attempt: { ...next.attempt!, in_flight: null }, transcript: { entries: [{ cursor: '1', item: { type: 'message', message }, tool_calls: next.attempt?.foreground }] } });
  if (ui.container.querySelector('[data-chat-anchor-key="message:answer"]') !== seat) replacements++;
  await act(async () => { flush(); });
  const measurement = { fixture: '200-long-reasoning-tool-v3', snapshots: s.requests.filter(x => x.request.method === 'session/checkpoint').length, commits, automaticWrites: writes - startWrites, bottomWrites, anchorWrites, rowReplacements: replacements };
  if (process.env.RUSTX_PERFORMANCE_OUTPUT) writeFileSync(process.env.RUSTX_PERFORMANCE_OUTPUT, JSON.stringify(measurement, null, 2));

  expect(measurement).toEqual({
    fixture: '200-long-reasoning-tool-v3',
    snapshots: 1,
    commits: 364,
    automaticWrites: 100,
    bottomWrites: 101,
    anchorWrites: 0,
    rowReplacements: 0,
  });
  expect(s.client.getSnapshot().views.A.cursor).toBe(String(s.cursor));
  s.client.disconnect();
});
