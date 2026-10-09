// @vitest-environment jsdom
import { afterEach, beforeEach, expect, it } from 'vitest';
import { act, cleanup, render, screen } from '@testing-library/react';
import { ContextSeat } from '../src/app/agent/ContextSeat';
import { Server, snapshot } from './fixture';
import type { RuntimeClientContextView } from '../../protocol/app-server/v38';
let server: Server;
beforeEach(async () => { server = new Server(); await server.attached('A'); });
afterEach(() => { cleanup(); server.client.disconnect(); });
const base = { compaction_count: 0, compaction_in_progress: false };
async function observe(context: RuntimeClientContextView) {
  await act(() => server.update('A', { ...snapshot(), context }));
}
function compact() { act(() => { expect(server.client.compact('A')).toBe(true); }); }
const count = () => server.requests.filter(item => item.request.method === 'context/compact').length;
it.each([null, 'Exact pre-commit failure'])('exact native release frees only A while its RPC remains held: %s', async error => {
  server.held.add('context/compact');
  server.handlers.set('context/compact', () => ({ type: 'context', context: base }));
  const view = render(<ContextSeat client={server.client} sessionId="A"/>);
  compact(); expect(server.client.compact('A')).toBe(false);
  const a = await server.waitFor('context/compact', 1);
  expect(screen.getByRole('status').textContent).toContain('Submitting');
  const requestId = server.client.getSnapshot().views.A.compactionRequest!.requestId;
  await observe({ ...base, compaction_in_progress: true, manual_compaction: { request_id: requestId, released: false, error: null } });
  view.unmount(); render(<ContextSeat client={server.client} sessionId="A"/>);
  expect(screen.getByRole('status').textContent).toContain('Compacting');
  await observe({ ...base, compaction_count: 1, compaction_in_progress: true, manual_compaction: { request_id: requestId, released: false, error: null } });
  expect(screen.getByRole('status').textContent).toBe('Compacting context…');
  expect(server.client.compact('A')).toBe(false);
  await observe({ ...base, compaction_count: 2, manual_compaction: { request_id: 'other-client', released: true, error: null } });
  expect(server.client.compact('A')).toBe(false); // Neither idle nor unrelated release owns A.
  expect(count()).toBe(1);
  await observe({ ...base, compaction_count: error ? 0 : 1, manual_compaction: { request_id: requestId, released: true, error } });
  expect(screen.getByRole('status').textContent).toContain(error ?? 'Context compacted');
  compact(); const b = await server.waitFor('context/compact', 2);
  const current = server.client.getSnapshot().views.A.compactionRequest!;
  expect(current.requestId).not.toBe(requestId);
  expect(current.status).toBe('submitting');
  await act(async () => {
    if (error) server.socket.deliver({ jsonrpc: '2.0', id: a.id, error: { code: -32000, message: error } });
    else server.reply(a);
  });
  expect(server.client.getSnapshot().views.A.compactionRequest).toBe(current);
  expect(server.client.compact('A')).toBe(false); // A's transport settlement must not release B.
  expect(count()).toBe(2);
  await act(async () => server.reply(b));
});
it.each(['missing', 'other-client', 'matching', 'failed'] as const)('lost reply repairs only exact surviving correlation: %s', async evidence => {
  server.held.add('context/compact');
  render(<ContextSeat client={server.client} sessionId="A"/>); compact();
  await server.waitFor('context/compact', 1);
  const requestId = server.client.getSnapshot().views.A.compactionRequest!.requestId;
  await act(async () => { await server.client.disconnect(); });
  const context = { ...base, compaction_count: evidence === 'failed' ? 0 : 8, ...(evidence === 'missing' ? {} : { manual_compaction: { request_id: ['matching', 'failed'].includes(evidence) ? requestId : 'other-client', released: true, error: evidence === 'failed' ? 'Exact native failure' : null } }) };
  server.snapshots.set('A', { ...snapshot(), context });
  await act(async () => server.connect());
  if (evidence === 'other-client') {
    await observe({ ...context, compaction_in_progress: true, manual_compaction: { request_id: 'other-client', released: false, error: null } });
    expect(screen.getByRole('status').textContent).toContain('Compacting context');
    expect(screen.getByRole('status').textContent).toContain('not yet confirmed');
    await observe(context);
  }
  expect(count()).toBe(1);
  expect(server.client.getSnapshot().uncertain.some(item => item.method === 'context/compact')).toBe(!['matching', 'failed'].includes(evidence));
  expect(screen.getByRole('status').textContent).toContain(evidence === 'matching' ? 'Context compacted' : evidence === 'failed' ? 'Exact native failure' : 'not yet confirmed');
});
it('observes native operations and preserves the exact pre-commit diagnostic', async () => {
  render(<ContextSeat client={server.client} sessionId="A"/>);
  await observe({ ...base, compaction_in_progress: true });
  expect(screen.getByRole('status').textContent).toBe('Compacting context…');
  await observe({ ...base, compaction_error: 'Summary provider rejected model fixture/exact' });
  expect(screen.getByRole('status').textContent).toContain('Summary provider rejected model fixture/exact');
  expect(count()).toBe(0);
});
it.each([undefined, { input_tokens: 25, context_window_tokens: 100, model: 'frozen/model', breakdown: { system_tokens: 5, tool_tokens: 10, message_tokens: 10 } }])('an idle seat renders nothing and never discloses request occupancy: %j', async occupancy => {
  const view = render(<ContextSeat client={server.client} sessionId="A"/>);
  await observe({ ...base, last_request_occupancy: occupancy });
  expect(view.container.innerHTML).toBe('');
  await observe({ ...base, compaction_in_progress: true, last_request_occupancy: occupancy });
  expect(screen.getByRole('status').textContent).toBe('Compacting context…');
  expect(view.container.querySelector('meter')).toBeNull();
  expect(view.container.textContent).not.toMatch(/Last request context|frozen\/model/);
  await observe({ ...base, last_request_occupancy: occupancy });
  expect(view.container.innerHTML).toBe('');
});

it('a replacement attachment rejects the held old response', async () => {
  server.held.add('context/compact');
  server.handlers.set('context/compact', () => ({ type: 'context', context: base }));
  compact(); const pending = await server.waitFor('context/compact', 1);
  const old = server.client.target('A');
  await server.client.release('A'); await server.client.attach('A');
  expect(server.client.target('A').attachment_id).not.toBe(old.attachment_id);
  expect(server.client.getSnapshot().views.A.compactionRequest).toBeUndefined();
  compact(); const newer = await server.waitFor('context/compact', 2);
  const newerId = server.client.getSnapshot().views.A.compactionRequest!.requestId;
  server.reply(pending); await Promise.resolve();
  expect(server.client.getSnapshot().views.A.compactionRequest?.requestId).toBe(newerId);
  const current = server.client.getSnapshot();
  server.socket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target: old, cursor: String(server.cursor + 1n), event: { type: 'context_compaction_failed', error: 'obsolete attachment', context: { ...base, compaction_error: 'obsolete attachment' } } } });
  expect(server.client.getSnapshot()).toBe(current);
  expect(server.client.getSnapshot().views.A.compactionRequest?.status).toBe('submitting');
  await act(async () => server.reply(newer));
  expect(server.client.getSnapshot().views.A.compactionRequest?.status).toBe('succeeded');
});
it.each(['conversation', 'incarnation', 'authority'] as const)('reconnect retires obsolete %s evidence and rejects the old socket', async replacement => {
  server.held.add('context/compact');
  server.handlers.set('context/compact', () => ({ type: 'context', context: base }));
  compact(); const pending = await server.waitFor('context/compact', 1);
  const response = server.commit(pending), oldSocket = server.socket;
  await server.client.disconnect();
  if (replacement === 'conversation') server.snapshots.set('A', { ...snapshot(), conversation_id: 'replacement' });
  if (replacement === 'incarnation') server.loaded.delete('A');
  if (replacement === 'authority') server.authorityId = 'replacement-server';
  await server.connect();
  expect(server.client.getSnapshot().views.A?.compactionRequest).toBeUndefined();
  const current = server.client.getSnapshot(); oldSocket.deliver(response);
  oldSocket.deliver({ jsonrpc: '2.0', method: 'session/event', params: { target: pending.method === 'context/compact' ? pending.params.target : server.client.target('A'), cursor: String(server.cursor + 1n), event: { type: 'context_compaction_failed', error: 'obsolete poison', context: { ...base, compaction_error: 'obsolete poison' } } } });
  expect(server.client.getSnapshot()).toBe(current);
});
it('confirmed commit survives a failed refresh', async () => {
  server.handlers.set('context/compact', () => ({ type: 'context', context: base }));
  server.held.add('session/snapshot');
  compact(); await server.waitFor('session/snapshot', 1);
  expect(server.client.getSnapshot().views.A.compactionRequest?.status).toBe('succeeded');
  await server.client.disconnect();
  expect(server.client.getSnapshot().views.A.compactionRequest?.status).toBe('succeeded');
});

it('real acknowledgement frees the gesture guard before its held read repair', async () => {
  server.handlers.set('context/compact', () => ({ type: 'context', context: base }));
  server.held.add('session/snapshot');
  compact(); await server.waitFor('session/snapshot', 1);
  const a = server.client.getSnapshot().views.A.compactionRequest!;
  expect(a.status).toBe('succeeded');
  server.held.add('context/compact');
  compact(); await server.waitFor('context/compact', 2);
  const b = server.client.getSnapshot().views.A.compactionRequest!;
  expect(b.requestId).not.toBe(a.requestId);
  expect(b.status).toBe('submitting');
  expect(server.client.compact('A')).toBe(false);
  expect(count()).toBe(2);
});
