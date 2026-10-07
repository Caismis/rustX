import { createRoot } from 'react-dom/client';
import { flushSync } from 'react-dom';
import type { ReactNode } from 'react';
const uiRoot = document.getElementById('root')!;
const root = createRoot(uiRoot);
const render = (node: ReactNode) => { flushSync(() => root.render(node)); return { container: uiRoot }; };
const act = async (fn: () => void | Promise<void>) => { await fn(); flushSync(() => {}); };
import { Profiler } from 'react';
import { Server, snapshot } from '../fixture';
import { ConversationLive } from '../../src/app/agent/ConversationLive';
import type { RuntimeClientEvent, RuntimeClientSnapshot } from '../../../protocol/app-server/v37';

(window as any).run420 = async () => {
  const browserFrame = window.requestAnimationFrame.bind(window);
  const paint = () => new Promise<void>(resolve => browserFrame(() => resolve()));
  const frames = new Map<number, FrameRequestCallback>(); let frameId = 0;
  window.requestAnimationFrame = cb => { frames.set(++frameId, cb); return frameId; };
  window.cancelAnimationFrame = id => { frames.delete(id); };
  let resize = () => {};
  window.ResizeObserver = class { constructor(cb: () => void) { resize = cb; } observe() {} unobserve() {} disconnect() {} } as unknown as typeof ResizeObserver;
  const flush = () => { const work = [...frames.values()]; frames.clear(); for (const cb of work) cb(0); };
  const s = new Server(); await s.attached('A');
  let commits = 0, writes = 0, bottomWrites = 0, anchorWrites = 0, height = 800;
  const ui = render(<Profiler id="transcript" onRender={() => commits++}><ConversationLive client={s.client} sessionId="A" mode="chat" disabled={false} onHistorical={() => {}}/></Profiler>);
  const el = ui.container.querySelector('.conversation-scroll') as HTMLElement;
  el.style.cssText = 'flex:0 0 600px;height:600px;max-height:600px;overflow:auto;overflow-anchor:none';
  const scroll = Object.getOwnPropertyDescriptor(Element.prototype, 'scrollTop')!;
  Object.defineProperty(el, 'scrollTop', { get: () => scroll.get!.call(el), set: value => {
    writes++; if (Math.abs(value - Math.max(0, el.scrollHeight - el.clientHeight)) < 1) bottomWrites++; else anchorWrites++;
    scroll.set!.call(el, value);
  } });
  await act(async () => { resize(); flush(); });
  const emit = async (event: RuntimeClientEvent, next: RuntimeClientSnapshot = s.snapshots.get('A')!) => {
    if (next.attempt?.in_flight && (event.type === 'tool_call_started' || event.type === 'tool_call_arguments_delta' || event.type === 'tool_call_assembled')) {
      const blocks = [...next.attempt.in_flight.blocks ?? []];
      if (event.type === 'tool_call_started') blocks.push({ type: 'tool_call', block_index: event.block_index, call_id: event.call.id, tool_id: event.call.tool_id, name: event.call.name, arguments: '' });
      else { const index = blocks.findIndex(b => b.block_index === event.block_index); const block = blocks[index]; if (block?.type === 'tool_call') blocks[index] = { ...block, arguments: event.type === 'tool_call_assembled' ? JSON.stringify(event.call.arguments) : block.arguments + event.arguments_delta }; }
      const foreground = blocks.flatMap(block => block.type === 'tool_call' ? [{ message_id: next.attempt!.in_flight!.message_id, block_index: block.block_index, call_id: block.call_id, tool_id: block.tool_id, name: block.name, state: { type: 'assembled' as const, arguments: block.arguments } }] : []);
      next = { ...next, attempt: { ...next.attempt, foreground, in_flight: { ...next.attempt.in_flight, blocks } } };
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
  await emit({ type: 'assistant_reasoning_delta', attempt_id, message_id, block_index: 1, delta: reasoning.text }, next);
  let text = '';
  let seat: Element | null = null, replacements = 0, historyBottomStart = 0, historyTop = 0;
  const startWrites = writes;
  for (let i = 0; i < 200; i++) {
    const delta = `paragraph ${i}: ${'fixed deterministic long response. '.repeat(8)}\n`;
    text += delta;
    next = { ...next, attempt: { ...next.attempt!, in_flight: { message_id, blocks: [{ type: 'text', block_index: 0, text }, reasoning] } } };
    await emit({ type: 'assistant_text_delta', attempt_id, message_id, block_index: 0, delta }, next);
    height += 10;
    await act(async () => { for (let n = 0; n < 10; n++) resize(); flush(); });
    const row = ui.container.querySelector('[data-chat-anchor-key="message:answer"]');
    if (seat && row !== seat) replacements++; seat = row;
    if (i % 25 === 0) await paint();
    if (i === 99) { scroll.set!.call(el, 210); el.dispatchEvent(new Event('scroll')); historyBottomStart = bottomWrites; historyTop = el.scrollTop; }
  }
  await emit({ type: 'tool_call_started', attempt_id, message_id, block_index: 2, call: { id: 'call', tool_id: 'bash', name: 'bash' } });
  for (const arguments_delta of ['{"command":', '"printf test"', '}']) await emit({ type: 'tool_call_arguments_delta', attempt_id, message_id, block_index: 2, call_id: 'call', arguments_delta });
  await emit({ type: 'tool_call_assembled', arguments_json: '{"command":"printf test"}', attempt_id, message_id, block_index: 2, call: { id: 'call', tool_id: 'bash', name: 'bash', arguments: { command: 'printf test' } } });
  const historyBottomWrites = bottomWrites - historyBottomStart;
  const readingTopBeforeGrowth = el.scrollTop;
  const growth = document.createElement('div'); growth.style.height = '123px'; el.firstElementChild!.prepend(growth);
  height += 123; await act(async () => { resize(); flush(); });
  const readingTopAfterGrowth = el.scrollTop;
  next = s.snapshots.get('A')!;
  const message = { role: 'assistant' as const, id: message_id, content: [{ type: 'text' as const, text: 'Canonical final content.' }, { type: 'reasoning' as const, text: reasoning.text }, { type: 'tool_call' as const, id: 'call', tool_id: 'bash', name: 'bash', arguments: { command: 'printf test' } }] };
  await emit({ type: 'message_committed', attempt_id, message, transcript_cursor: '1' }, { ...next, messages: [message], attempt: { ...next.attempt!, in_flight: null }, transcript: { entries: [{ cursor: '1', item: { type: 'message', message }, tool_calls: next.attempt?.foreground }] } });
  if (ui.container.querySelector('[data-chat-anchor-key="message:answer"]') !== seat) replacements++;
  await act(async () => { flush(); });
  const measurement = { fixture: '200-long-reasoning-tool-v3', snapshots: s.requests.filter(x => x.request.method === 'session/snapshot').length, commits, automaticWrites: writes - startWrites, bottomWrites, anchorWrites, rowReplacements: replacements, historyTop, historyBottomWrites, readingTopBeforeGrowth, readingTopAfterGrowth };
  (window as any).measurement = measurement;
  return measurement;
};

import '../../src/presentation/theme/base.css';
import '../../src/presentation/theme/reset.css';
import '../../src/app/console.css';
