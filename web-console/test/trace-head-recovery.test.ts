import { afterEach, expect, it } from 'vitest';
import { Server, checkpoint } from './fixture';
import { traceRecord } from './trace-fixture';
import { RpcFailure } from '../src/client/errors';
import type { TracePage } from '../../protocol/app-server/v80';

const servers: Server[] = [];
const create = async () => { const s = new Server(); servers.push(s); await s.attached('A'); return s; };
afterEach(() => { for (const s of servers) s.client.disconnect(); servers.length = 0; });

const traceCalls = (s: Server) => s.requests.filter(row => row.request.method === 'session/trace');
const head = (s: Server, records: TracePage['records'], next_cursor: string | null = null): TracePage =>
  ({ read_cut: checkpoint(s.snapshots.get('A')!).read_cut, records, next_cursor });

it('recovers an empty live trace head from the authoritative session/trace read', async () => {
  const s = await create();
  // The observation checkpoint head is empty while canonical messages exist.
  s.snapshots.set('A', { ...s.snapshots.get('A')!, trace: { records: [], next_cursor: null },
    messages: [{ id: 'user-1', role: 'user', source: 'human', kind: 'message', content: [{ type: 'text', text: 'hello' }] }] });
  s.handlers.set('session/trace', () => ({ type: 'trace', refreshed: 0, page: head(s, [traceRecord(1), traceRecord(2)]) }));
  await s.client.loadInitialTrace('A');
  expect(traceCalls(s)).toHaveLength(1);
  expect(traceCalls(s)[0].request.params).toMatchObject({ before: null });
  expect(s.client.getSnapshot().views.A.trace?.page.records.map(record => record.id)).toEqual(['trace:1', 'trace:2']);
  expect(s.client.getSnapshot().views.A.trace?.loading).toBe(false);
  expect(s.client.getSnapshot().views.A.trace?.error).toBeUndefined();
});

it('confirms a genuinely empty head once per attachment scope without looping', async () => {
  const s = await create();
  s.handlers.set('session/trace', () => ({ type: 'trace', refreshed: 0, page: head(s, []) }));
  await s.client.loadInitialTrace('A');
  await s.client.loadInitialTrace('A');
  await s.client.loadInitialTrace('A');
  expect(traceCalls(s)).toHaveLength(1);
  expect(s.client.getSnapshot().views.A.trace?.page.records).toEqual([]);
  expect(s.client.getSnapshot().views.A.trace?.error).toBeUndefined();
});

it('surfaces a failed head recovery on the trace cache', async () => {
  const s = await create();
  s.handlers.set('session/trace', () => { throw new RpcFailure({ code: -32000, message: 'trace read failed', data: { kind: 'invalid_state' } }); });
  await s.client.loadInitialTrace('A');
  expect(s.client.getSnapshot().views.A.trace?.error).toContain('trace read failed');
  expect(s.client.getSnapshot().views.A.trace?.loading).toBe(false);
});
