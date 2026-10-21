import { useSyncExternalStore } from 'react';
import { act, cleanup, fireEvent, render, screen, waitFor } from '@testing-library/react';
import { afterEach, beforeEach, expect, it, vi } from 'vitest';
import { Trajectory } from '../src/app/trajectory/Trajectory';
import { AgentTraceReader } from '../src/client/agent-trace';
import type { TraceCache } from '../src/client/trace';
import { checkpoint, Server, snapshot } from './fixture';
import { nativeTraceFixture } from './native-trace-fixture';
import { requestDetail, traceRecord } from './trace-fixture';

const servers: Server[] = [];
beforeEach(() => {
  vi.spyOn(HTMLElement.prototype, 'offsetHeight', 'get').mockReturnValue(360);
  vi.spyOn(HTMLElement.prototype, 'offsetWidth', 'get').mockReturnValue(800);
});
afterEach(() => { cleanup(); servers.splice(0).forEach(server => server.client.disconnect()); vi.restoreAllMocks(); });

function Live({ subscribe, snapshot, load }: { subscribe: (listener: () => void) => () => void; snapshot: () => TraceCache; load: (id: string) => void }) {
  const cache = useSyncExternalStore(subscribe, snapshot);
  return <Trajectory cache={cache} onSelect={() => {}} loadEarlier={() => {}} onLoadDetail={load}/>;
}

it.each(['root', 'child'] as const)('%s inspector stays mounted across live updates and atomically replaces deferred content', async kind => {
  const server = new Server(); servers.push(server);
  let record = traceRecord(4, { state: 'running' });
  const base = { ...checkpoint(snapshot()).read_cut, ...(kind === 'child' ? { conversation_id: 'child', activation_id: 'child-activation' } : {}) };
  let revision = 1;
  const make = (model: string) => {
    const detail = requestDetail(4); detail.request!.model = model;
    return nativeTraceFixture(record, detail, { ...base, revision: String(revision) });
  };
  let native = make('ORIGINAL_MODEL');
  const sources = new Map([[native.page.read_cut.revision, native]]);
  const detailMethod = kind === 'root' ? 'session/traceDetail' : 'agent/traceDetail';
  server.handlers.set(detailMethod, () => ({ type: 'domain_page', page: native.page }));
  server.handlers.set('conversation/checkpointPage', request => {
    if (request.method !== 'conversation/checkpointPage') throw Error('page');
    return { type: 'checkpoint_page', page: sources.get(request.params.cut.revision)!.original.page(request.params.cut, request.params.domain, request.params.offset, () => true) };
  });
  server.handlers.set('conversation/checkpointValueRead', request => {
    if (request.method !== 'conversation/checkpointValueRead') throw Error('value');
    return { type: 'checkpoint_value_chunk', chunk: sources.get(request.params.cut.revision)!.original.value(request.params.cut, request.params, request.params.offset, () => true) };
  });
  server.snapshots.set('A', { ...snapshot(), trace: { records: [record], next_cursor: null } });
  await server.attached('A');
  let read: () => TraceCache, subscribe: (listener: () => void) => () => void, load: (id: string) => Promise<void>, pulse: () => Promise<void>;
  if (kind === 'root') {
    server.client.selectTrace('A', record.id);
    read = () => server.client.getSnapshot().views.A.trace!;
    subscribe = server.client.subscribe;
    load = id => server.client.loadTraceDetail('A', id);
    pulse = () => server.update('A', { ...snapshot(), trace: { records: [record], next_cursor: null } });
  } else {
    server.handlers.set('agent/trace', () => ({ type: 'trace', page: { read_cut: { ...base, revision: String(revision) }, records: [record], next_cursor: null } }));
    const reader = new AgentTraceReader(server.client, 'A', server.client.getSnapshot().views.A.attachmentObservation!, { agent_id: 'child', activation_id: 'child-activation', child_conversation_id: 'child', state: 'active' });
    await reader.refresh(); reader.select(record.id);
    read = reader.snapshot; subscribe = reader.subscribe; load = reader.detail; pulse = reader.refresh;
  }
  render(<Live subscribe={subscribe} snapshot={read} load={load}/>);
  fireEvent.click(screen.getByRole('tab', { name: 'Options' }));
  await screen.findByText('"ORIGINAL_MODEL"');
  const panel = screen.getByRole('tabpanel'), tree = screen.getByRole('tree');
  panel.scrollTop = 80;
  const first = read().details[record.id], originalCut = read().readCuts![record.id];
  const count = () => server.requests.filter(row => row.request.method === detailMethod).length;
  await act(async () => {
    for (let index = 0; index < 20; index++) {
      revision++;
      record = { ...record, timing: { ...record.timing, duration_ms: String(index) }, request: { ...record.request!, usage: { input_tokens: '10', output_tokens: String(index), total_tokens: String(10 + index), details: {} } } };
      await pulse();
    }
  });
  expect(count()).toBe(1);
  expect(read().details[record.id]).toBe(first);
  expect(read().readCuts![record.id]).toEqual(originalCut);
  expect(screen.getByRole('tabpanel')).toBe(panel);
  expect(screen.getByRole('tree')).toBe(tree);
  expect(panel.scrollTop).toBe(80);
  expect(screen.getByRole('tab', { name: 'Options' }).getAttribute('aria-selected')).toBe('true');

  server.held.add(detailMethod);
  record = { ...record, preview: { text: 'Updated native body', truncated: false } };
  revision++; native = make('UPDATED_MODEL'); sources.set(native.page.read_cut.revision, native);
  await act(pulse);
  const pending = await server.waitFor(detailMethod, 2);
  expect(screen.getByText('"ORIGINAL_MODEL"')).toBeTruthy();
  expect(screen.queryByRole('status')).toBeNull();
  expect(screen.getByRole('tree')).toBe(tree);
  for (let index = 0; index < 10; index++) {
    record = { ...record, timing: { ...record.timing, duration_ms: String(100 + index) } };
    await act(pulse);
  }
  expect(count()).toBe(2);
  const valuesBefore = server.requests.filter(row => row.request.method === 'conversation/checkpointValueRead').length;
  server.held.add('conversation/checkpointValueRead');
  await act(async () => server.reply(pending));
  const value = await server.waitFor('conversation/checkpointValueRead', valuesBefore + 1);
  expect(screen.getByText('"ORIGINAL_MODEL"')).toBeTruthy();
  expect(screen.queryByRole('status')).toBeNull();
  expect(screen.getByRole('tree')).toBe(tree);
  server.held.delete('conversation/checkpointValueRead');
  await act(async () => server.reply(value));
  await screen.findByText('"UPDATED_MODEL"');
  expect(screen.queryByText('"ORIGINAL_MODEL"')).toBeNull();
  expect(screen.getByRole('tree')).toBe(tree);
  expect(screen.getByRole('tabpanel')).toBe(panel);
  expect(panel.scrollTop).toBe(80);
  await waitFor(() => expect(read().details[record.id].stale).toBe(false));
  expect(count()).toBe(2);
});
