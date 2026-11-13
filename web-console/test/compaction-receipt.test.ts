import { afterEach, expect, it } from 'vitest';
import { createElement } from 'react';
import { cleanup, render } from '@testing-library/react';
import type { RuntimeClientCompactionView } from '../../protocol/app-server/v79';
import { RpcFailure } from '../src/client/errors';
import { compactionPresentation, ContextSeat } from '../src/app/agent/ContextSeat';
import { Server, snapshot, projectionHead } from './fixture';

let server: Server;
afterEach(() => { cleanup(); server?.client.disconnect(); });
const receipt = (generation: string): RuntimeClientCompactionView => ({
  generation, checkpoint_id:`conv_8498648a-66ba-716f-8243-8cf147533e5${generation}`, activation_id:generation,
  tokens_before:{input_tokens:'8400',source:'estimated'}, estimated_tokens_after:'1900',
});

it('accepts a correlated skipped response without inventing a checkpoint', async () => {
  server = new Server();
  await server.attached('A');
  server.handlers.set('context/compact', () => ({ type: 'context_compaction_skipped' }));
  server.held.add('context/compact');
  expect(server.client.compact('A')).toBe(true);
  const request = await server.waitFor('context/compact', 1);
  const finished = new Promise<void>(resolve => {
    const stop = server.client.subscribe(() => {
      if (server.client.getSnapshot().views.A.compactionRequest?.status === 'skipped') { stop(); resolve(); }
    });
  });
  server.reply(request); await finished;
  const view = server.client.getSnapshot().views.A;
  expect(view.compactionRequest?.receipt).toBeUndefined();
  expect(view.snapshot?.context?.latest_compaction).toBeUndefined();
  expect(compactionPresentation(view.snapshot?.context, view.compactionRequest)).toEqual({ state: 'skipped' });
  const presentation = render(createElement(ContextSeat, { client: server.client, sessionId: 'A' }));
  expect(presentation.container.textContent).toContain('No eligible history to compact');
  expect(presentation.container.querySelector('[data-compaction-marker]')).toBeNull();
  expect(server.client.getSnapshot().connection).toBe('connected');
  expect(server.client.compact('A')).toBe(true);
});

it('reconciles a lost skipped response only from the matching native request', async () => {
  server = new Server();
  await server.attached('A');
  server.held.add('context/compact');
  expect(server.client.compact('A')).toBe(true);
  await server.waitFor('context/compact', 1);
  const requestId = server.client.getSnapshot().views.A.compactionRequest!.requestId;
  await server.client.disconnect();
  const skipped = { compaction_count: 0, compaction_in_progress: false,
    manual_compaction: { request_id: 'other-request', released: true, skipped: true, cancelled: false, error: null } };
  server.snapshots.set('A', { ...snapshot(), context: skipped });
  await server.connect();
  await server.client.waitForAttachment('A', () => true);
  expect(server.client.getSnapshot().views.A.compactionRequest?.status).toBe('uncertain');
  expect(server.client.getSnapshot().uncertain.some(item => item.compactionRequestId === requestId)).toBe(true);
  await server.update('A', { ...snapshot(), context: { ...skipped,
    manual_compaction: { ...skipped.manual_compaction, request_id: requestId } } });
  const view = server.client.getSnapshot().views.A;
  expect(view.compactionRequest?.status).toBe('skipped');
  expect(view.compactionRequest?.receipt).toBeUndefined();
  expect(view.snapshot?.context?.latest_compaction).toBeUndefined();
  expect(server.client.getSnapshot().uncertain.some(item => item.compactionRequestId === requestId)).toBe(false);
});

it('reconciles a lost cancellation only from the matching released native request', async () => {
  server = new Server();
  await server.attached('A');
  server.held.add('context/compact');
  expect(server.client.compact('A')).toBe(true);
  await server.waitFor('context/compact', 1);
  const requestId = server.client.getSnapshot().views.A.compactionRequest!.requestId;
  await server.client.disconnect();
  const cancelled = { compaction_count: 0, compaction_in_progress: false,
    manual_compaction: { request_id: 'other-request', released: true, skipped: false, cancelled: true, error: null } };
  server.snapshots.set('A', { ...snapshot(), context: cancelled });
  await server.connect();
  await server.client.waitForAttachment('A', () => true);
  expect(server.client.getSnapshot().views.A.compactionRequest?.status).toBe('uncertain');
  expect(server.client.getSnapshot().uncertain.some(item => item.compactionRequestId === requestId)).toBe(true);
  await server.update('A', { ...snapshot(), context: { ...cancelled,
    manual_compaction: { ...cancelled.manual_compaction, request_id: requestId } } });
  const view = server.client.getSnapshot().views.A;
  expect(view.compactionRequest?.status).toBe('cancelled');
  expect(view.compactionRequest?.receipt).toBeUndefined();
  expect(view.snapshot?.context?.latest_compaction).toBeUndefined();
  expect(server.client.getSnapshot().uncertain.some(item => item.compactionRequestId === requestId)).toBe(false);
  const presentation = render(createElement(ContextSeat, { client: server.client, sessionId: 'A' }));
  expect(presentation.container.textContent).toContain('Compaction cancelled');
  expect(presentation.container.querySelector('[data-compaction-marker]')).toBeNull();
  expect(server.client.compact('A')).toBe(true);
});

it.each((['skipped', 'cancelled'] as const).flatMap(status =>
  [false, true].flatMap(reconnect => [false, true].map(newerCommit => ({ status, reconnect, newerCommit }))),
))('reconciles $status presentation (reconnect=$reconnect, newerCommit=$newerCommit)', async ({ status, reconnect, newerCommit }) => {
  server = new Server();
  await server.attached('A');
  server.held.add('context/compact');
  expect(server.client.compact('A')).toBe(true);
  await server.waitFor('context/compact', 1);
  const requestId = server.client.getSnapshot().views.A.compactionRequest!.requestId;
  await server.update('A', { ...snapshot(), context: { compaction_count: 0, compaction_in_progress: false,
    manual_compaction: { request_id: requestId, released: true, skipped: status === 'skipped', cancelled: status === 'cancelled', error: null } } });
  expect(server.client.getSnapshot().views.A.compactionRequest?.status).toBe(status);

  // An idle snapshot passes through real client reconciliation. No running
  // notification arrives to clear the earlier terminal request.
  const generation = server.client.getSnapshot().generation;
  const latest = newerCommit ? receipt('1') : undefined;
  const next = { ...snapshot(), context: { compaction_count: newerCommit ? 1 : 0,
    compaction_in_progress: false, latest_compaction: latest,
    projection_head: latest ? { ...projectionHead(), checkpoint_id: latest.checkpoint_id, activation_id: latest.activation_id } : undefined } };
  if (reconnect) {
    await server.client.disconnect();
    server.snapshots.set('A', next);
    server.handlers.set('session/subscribe', () => {
      server.handlers.delete('session/subscribe');
      throw new RpcFailure({ code: -32000, message: 'History advanced while disconnected', data: { kind: 'resync_required' } });
    });
    await server.connect();
    await server.client.waitForAttachment('A', () => true);
    expect(server.client.getSnapshot().generation).toBeGreaterThan(generation);
  } else {
    await server.update('A', next);
    expect(server.client.getSnapshot().generation).toBe(generation);
  }
  const view = server.client.getSnapshot().views.A;
  expect(view.compactionRequest?.status).toBe(status);
  expect(view.compactionRequest?.generation).toBe(generation);
  expect(view.snapshot?.context?.compaction_count).toBe(newerCommit ? 1 : 0);
  expect(view.snapshot?.context?.projection_head).toEqual(next.context.projection_head);
  expect(view.compactionRequest?.receipt).toBeUndefined();
  expect(view.snapshot?.context?.latest_compaction).toEqual(latest);
  expect(compactionPresentation(view.snapshot?.context, view.compactionRequest)).toEqual({ state: newerCommit ? 'succeeded' : status });
  const presentation = render(createElement(ContextSeat, { client: server.client, sessionId: 'A' }));
  const terminalText = status === 'skipped' ? 'No eligible history to compact' : 'Compaction cancelled';
  if (newerCommit) expect(presentation.container.textContent).not.toContain(terminalText);
  else expect(presentation.container.textContent).toContain(terminalText);
  expect(presentation.container.querySelector('[data-compaction-marker]')).toBeNull();
  presentation.unmount();
});

it('still rejects a skipped discriminator for an unrelated request', async () => {
  server = new Server();
  await server.attached('A');
  server.held.add('session/list');
  const next = server.requests.filter(row => row.request.method === 'session/list').length + 1;
  const pending = server.client.request({ method: 'session/list', params: { offset: 0, limit: 32, query: null } }, 'sessions');
  const request = await server.waitFor('session/list', next);
  server.socket.success(request, { type: 'context_compaction_skipped' });
  await expect(pending).rejects.toThrow();
  expect(server.client.getSnapshot().connection).not.toBe('connected');
});

it.each([false,true])('exact compaction receipt is retained independently of newer published context (unavailable=%s)', async unavailable => {
  server = new Server();
  server.snapshots.set('A', {...snapshot(),context:{compaction_count:9,compaction_in_progress:false,latest_compaction:receipt('9')}});
  await server.attached('A');
  const target = server.target('A'), registration = server.registrations.get(JSON.stringify(target))!;
  if (unavailable) server.socket.deliver({jsonrpc:'2.0',method:'session/observationUnavailable',params:{target,registration_id:registration,message:'held original source'}});
  const before = server.client.getSnapshot().views.A;
  const reads = server.requests.filter(row => row.request.method === 'session/checkpoint').length;
  const original = receipt('7');
  server.handlers.set('context/compact', () => ({type:'context_compacted',compaction:original}));
  server.held.add('context/compact');
  expect(server.client.compact('A')).toBe(true);
  const request = await server.waitFor('context/compact',1);
  const finished = new Promise<void>(resolve => {
    const stop = server.client.subscribe(() => {
      if (server.client.getSnapshot().views.A.compactionRequest?.status === 'succeeded') {stop();resolve();}
    });
  });
  server.reply(request); await finished;
  const after = server.client.getSnapshot().views.A;
  expect(after.compactionRequest?.receipt).toEqual(original);
  expect(after.snapshot).toBe(before.snapshot); expect(after.cursor).toBe(before.cursor);
  expect(after.snapshot?.context?.latest_compaction?.generation).toBe('9');
  expect(after.observationState).toBe(before.observationState);
  expect(server.requests.filter(row => row.request.method === 'session/checkpoint')).toHaveLength(reads);
  expect(server.requests.filter(row => row.request.method === 'context/compact')).toHaveLength(1);
});
