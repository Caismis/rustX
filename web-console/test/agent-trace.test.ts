import { afterEach, expect, it } from 'vitest';
import { Server } from './fixture';
import { AgentTraceReader } from '../src/client/agent-trace';
import { traceRecord, requestDetail } from './trace-fixture';
let server: Server;
afterEach(() => server?.client.disconnect());
async function setup() {
  server = new Server(); await server.attached('A');
  return server.client.getSnapshot().views.A.attachmentObservation!;
}
it('child identities own independent trace rows and details even when record ids coincide', async () => {
  const proof = await setup();
  server.handlers.set('agent/trace', request => {
    if (request.method !== 'agent/trace') throw Error('Wrong method');
    return { type: 'trace', page: { records: [traceRecord(1, { preview: { text: request.params.agent_id, truncated: false } })], next_cursor: null } };
  });
  server.handlers.set('agent/traceDetail', request => {
    if (request.method !== 'agent/traceDetail') throw Error('Wrong method');
    return { type: 'trace_detail', detail: requestDetail(1, { messages: [{ message_id: request.params.agent_id, role: 'assistant', blocks: [], truncated: false }] }) };
  });
  const a = new AgentTraceReader(server.client, 'A', proof, 'child-a'), b = new AgentTraceReader(server.client, 'A', proof, 'child-b');
  await Promise.all([a.refresh(), b.refresh()]);
  await Promise.all([a.detail('trace:1'), b.detail('trace:1')]);
  expect(a.snapshot().page.records[0].preview?.text).toBe('child-a');
  expect(b.snapshot().page.records[0].preview?.text).toBe('child-b');
  expect(a.snapshot().details['trace:1'].detail?.messages[0].message_id).toBe('child-a');
  expect(b.snapshot().details['trace:1'].detail?.messages[0].message_id).toBe('child-b');
  expect(server.requests.filter(row => row.request.method === 'session/traceDetail')).toHaveLength(0);
});
it('older child records survive live refresh and use only the child cursor', async () => {
  const proof = await setup();
  server.handlers.set('agent/trace', request => {
    if (request.method !== 'agent/trace') throw Error('Wrong method');
    return { type: 'trace', page: { records: request.params.before ? [traceRecord(1)] : [traceRecord(2)], next_cursor: request.params.before ? null : 'trace:2' } };
  });
  const reader = new AgentTraceReader(server.client, 'A', proof, 'child');
  await reader.refresh(); await reader.earlier();
  expect(reader.snapshot().page.records.map(record => record.id)).toEqual(['trace:1', 'trace:2']);
  expect(reader.snapshot().page.next_cursor).toBeNull();
  expect(server.requests.filter(row => row.request.method === 'agent/trace').map(row => row.request.params)).toMatchObject([
    { agent_id: 'child', records: [] }, { agent_id: 'child', before: 'trace:2' }, { agent_id: 'child', records: ['trace:1', 'trace:2'] },
  ]);
});
it('refresh coalesces child activity and retired domains reject late replies', async () => {
  const proof = await setup();
  server.held.add('agent/trace');
  server.handlers.set('agent/trace', () => ({ type: 'trace', page: { records: [traceRecord(1)], next_cursor: null } }));
  const reader = new AgentTraceReader(server.client, 'A', proof, 'child');
  const first = reader.refresh(); await server.waitFor('agent/trace', 1);
  await Promise.all([reader.refresh(), reader.refresh(), reader.refresh()]);
  server.reply(server.requests.find(row => row.request.method === 'agent/trace')!.request);
  const second = await server.waitFor('agent/trace', 2); reader.retire(); server.reply(second);
  await first;
  expect(server.requests.filter(row => row.request.method === 'agent/trace')).toHaveLength(2);
  expect(reader.snapshot().page.records).toHaveLength(1);
});
it('attachment replacement cannot adopt a late child detail', async () => {
  const proof = await setup();
  server.handlers.set('agent/trace', () => ({ type: 'trace', page: { records: [traceRecord(1)], next_cursor: null } }));
  server.handlers.set('agent/traceDetail', () => ({ type: 'trace_detail', detail: requestDetail(1) }));
  const reader = new AgentTraceReader(server.client, 'A', proof, 'child'); await reader.refresh();
  server.held.add('agent/traceDetail'); const work = reader.detail('trace:1');
  const request = await server.waitFor('agent/traceDetail', 1), old = server.socket;
  server.client.disconnect(); server.reply(request, old); await work;
  expect(reader.snapshot().details['trace:1'].detail).toBeUndefined();
});
it('a live rebase retires old paging without stranding or clearing the new page owner', async () => {
  const proof = await setup();
  server.handlers.set('agent/trace', () => ({ type: 'trace', page: { records: [traceRecord(10)], next_cursor: 'trace:10' } }));
  const reader = new AgentTraceReader(server.client, 'A', proof, 'child'); await reader.refresh();
  server.held.add('agent/trace'); const older = reader.earlier();
  const oldRequest = await server.waitFor('agent/trace', 2);
  const live = reader.refresh(); const liveRequest = await server.waitFor('agent/trace', 3);
  server.handlers.set('agent/trace', () => ({ type: 'trace', page: { records: [traceRecord(20)], next_cursor: 'trace:20' } }));
  server.reply(liveRequest); await live;
  expect(reader.snapshot().loading).toBe(false);
  const newOlder = reader.earlier(); const newRequest = await server.waitFor('agent/trace', 4);
  server.handlers.set('agent/trace', () => ({ type: 'trace', page: { records: [traceRecord(9)], next_cursor: 'trace:9' } }));
  server.reply(oldRequest); await older;
  expect(reader.snapshot().loading).toBe(true);
  expect(reader.snapshot().page.records.map(record => record.id)).toEqual(['trace:20']);
  reader.retire(); server.reply(newRequest); await newOlder;
});
